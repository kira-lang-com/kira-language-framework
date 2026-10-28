//! The child processes a program starts and waits for, named by handle.
//!
//! A command is built in a table of its own, argument by argument, then
//! started: starting it releases the command and answers a child handle.
//! Every handle is a positive word and every failure a negative one from
//! [`Failure`], so one result word carries either.
//!
//! Each child has a waiter thread that reaps it and announces the ending, so
//! any thread can wait for one child or for whichever ends first without
//! polling. A piped output stream has a reader thread that drains it into a
//! buffer, so a child never blocks on a full pipe while the program is busy
//! with another one.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Why a process operation failed, as the negative word it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Failure {
    Missing = -1,
    Denied = -2,
    InvalidHandle = -4,
    InvalidPath = -6,
    Io = -7,
    NoProgram = -8,
    NotPiped = -9,
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::Missing,
            std::io::ErrorKind::PermissionDenied => Self::Denied,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidFilename => {
                Self::InvalidPath
            }
            _ => Self::Io,
        }
    }
}

/// What `child_wait` answers for a child still running when the wait ended.
pub const STILL_RUNNING: i64 = -100;

/// What `child_read` answers when a pipe has no bytes yet but is not over.
pub const WOULD_BLOCK: i64 = -101;

/// What a child ended by a signal answers: this minus the signal's number.
pub const SIGNALLED: i64 = -200;

/// Where one of a child's three standard streams goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stream {
    Inherit,
    Null,
    Pipe,
    File(PathBuf),
}

impl Stream {
    /// The stream a mode word and a path name: 0 inherit, 1 null, 2 pipe,
    /// 3 the file at `path`.
    #[must_use]
    pub fn from_word(mode: i64, path: &[u8]) -> Option<Self> {
        match mode {
            0 => Some(Self::Inherit),
            1 => Some(Self::Null),
            2 => Some(Self::Pipe),
            3 => os_text(path).map(|path| Self::File(path.into())),
            _ => None,
        }
    }
}

/// A signal `child_signal` sends, by the word the service passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Interrupt,
    Terminate,
    Kill,
}

impl Signal {
    #[must_use]
    pub const fn from_word(word: i64) -> Option<Self> {
        match word {
            1 => Some(Self::Interrupt),
            2 => Some(Self::Terminate),
            3 => Some(Self::Kill),
            _ => None,
        }
    }
}

#[cfg_attr(
    unix,
    expect(clippy::unnecessary_wraps, reason = "bytes name no text off Unix")
)]
fn os_text(bytes: &[u8]) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(OsString::from)
    }
}

#[derive(Default)]
struct Spec {
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    clear_environment: bool,
    directory: Option<PathBuf>,
    streams: [Option<Stream>; 3],
}

/// A piped output stream as its reader thread fills it.
#[derive(Default)]
struct Drained {
    bytes: VecDeque<u8>,
    over: bool,
}

#[derive(Default)]
struct Pipe {
    drained: Mutex<Drained>,
    arrived: Condvar,
}

struct Running {
    pid: u32,
    input: Mutex<Option<ChildStdin>>,
    outputs: [Option<Arc<Pipe>>; 2],
    ending: Mutex<Option<i64>>,
}

static SPECS: Mutex<Vec<Option<Spec>>> = Mutex::new(Vec::new());
static CHILDREN: Mutex<Vec<Option<Arc<Running>>>> = Mutex::new(Vec::new());

/// The children that ended and `child_wait_any` has not answered yet.
static ENDED: Mutex<VecDeque<i64>> = Mutex::new(VecDeque::new());
static ENDING: Condvar = Condvar::new();

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn word(result: Result<i64, Failure>) -> i64 {
    result.unwrap_or_else(|failure| failure as i64)
}

fn length(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The deadline a timeout word names: none for a negative one, which waits
/// forever.
fn deadline(timeout_nanos: i64) -> Option<Instant> {
    u64::try_from(timeout_nanos)
        .ok()
        .map(|nanos| Instant::now() + Duration::from_nanos(nanos))
}

/// Waits on `condvar` until `deadline`; false once it has passed.
fn wait_until<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    deadline: Option<Instant>,
) -> (MutexGuard<'a, T>, bool) {
    match deadline {
        None => (
            condvar.wait(guard).unwrap_or_else(PoisonError::into_inner),
            true,
        ),
        Some(deadline) => {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return (guard, false);
            }
            let (guard, _) = condvar
                .wait_timeout(guard, left)
                .unwrap_or_else(PoisonError::into_inner);
            (guard, true)
        }
    }
}

fn with_spec(handle: i64, change: impl FnOnce(&mut Spec)) -> i64 {
    let mut specs = locked(&SPECS);
    let slot = usize::try_from(handle - 1)
        .ok()
        .and_then(|index| specs.get_mut(index))
        .and_then(Option::as_mut);
    slot.map_or(Failure::InvalidHandle as i64, |spec| {
        change(spec);
        1
    })
}

fn insert(value: Spec) -> i64 {
    let mut table = locked(&SPECS);
    let index = table.iter().position(Option::is_none).unwrap_or_else(|| {
        table.push(None);
        table.len() - 1
    });
    table[index] = Some(value);
    length(index + 1)
}

/// An empty command, by handle.
#[must_use]
pub fn spawn_create() -> i64 {
    insert(Spec::default())
}

/// Appends an argument to command `handle`; the first is the program.
#[must_use]
pub fn spawn_argument(handle: i64, argument: &[u8]) -> i64 {
    let Some(argument) = os_text(argument) else {
        return Failure::InvalidPath as i64;
    };
    with_spec(handle, |spec| spec.arguments.push(argument))
}

/// Sets one `NAME=value` in command `handle`'s environment.
#[must_use]
pub fn spawn_environment(handle: i64, entry: &[u8]) -> i64 {
    let Some(split) = entry.iter().position(|byte| *byte == b'=') else {
        return Failure::InvalidPath as i64;
    };
    let (Some(name), Some(value)) = (os_text(&entry[..split]), os_text(&entry[split + 1..]))
    else {
        return Failure::InvalidPath as i64;
    };
    with_spec(handle, |spec| spec.environment.push((name, value)))
}

/// Makes command `handle` start from an empty environment.
#[must_use]
pub fn spawn_clear_environment(handle: i64) -> i64 {
    with_spec(handle, |spec| spec.clear_environment = true)
}

/// Sets command `handle`'s working directory.
#[must_use]
pub fn spawn_directory(handle: i64, path: &[u8]) -> i64 {
    let Some(path) = os_text(path) else {
        return Failure::InvalidPath as i64;
    };
    with_spec(handle, |spec| spec.directory = Some(path.into()))
}

/// Says where standard stream `stream` (0, 1 or 2) of command `handle` goes.
#[must_use]
pub fn spawn_stream(handle: i64, stream: i64, target: Stream) -> i64 {
    let Some(index) = usize::try_from(stream).ok().filter(|index| *index < 3) else {
        return Failure::InvalidHandle as i64;
    };
    with_spec(handle, |spec| spec.streams[index] = Some(target))
}

fn stdio(stream: Option<&Stream>, input: bool) -> Result<Stdio, Failure> {
    Ok(match stream {
        None | Some(Stream::Inherit) => Stdio::inherit(),
        Some(Stream::Null) => Stdio::null(),
        Some(Stream::Pipe) => Stdio::piped(),
        Some(Stream::File(path)) if input => std::fs::File::open(path)?.into(),
        Some(Stream::File(path)) => std::fs::File::create(path)?.into(),
    })
}

fn command(spec: &Spec) -> Result<Command, Failure> {
    let (program, arguments) = spec.arguments.split_first().ok_or(Failure::NoProgram)?;
    let mut command = Command::new(program);
    command.args(arguments);
    if spec.clear_environment {
        command.env_clear();
    }
    command.envs(spec.environment.iter().map(|(name, value)| (name, value)));
    if let Some(directory) = &spec.directory {
        command.current_dir(directory);
    }
    command.stdin(stdio(spec.streams[0].as_ref(), true)?);
    command.stdout(stdio(spec.streams[1].as_ref(), false)?);
    command.stderr(stdio(spec.streams[2].as_ref(), false)?);
    Ok(command)
}

fn drain(mut source: impl Read + Send + 'static) -> Arc<Pipe> {
    let pipe = Arc::new(Pipe::default());
    let filling = Arc::clone(&pipe);
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            let read = source.read(&mut chunk).unwrap_or(0);
            let mut drained = locked(&filling.drained);
            if read == 0 {
                drained.over = true;
            } else {
                drained.bytes.extend(&chunk[..read]);
            }
            filling.arrived.notify_all();
            if read == 0 {
                return;
            }
        }
    });
    pipe
}

fn ending_word(status: std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return i64::from(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return SIGNALLED - i64::from(signal);
        }
    }
    SIGNALLED
}

fn reap(handle: i64, mut child: Child, running: Arc<Running>) {
    std::thread::spawn(move || {
        let ending = child.wait().map_or(SIGNALLED, ending_word);
        *locked(&running.ending) = Some(ending);
        locked(&ENDED).push_back(handle);
        ENDING.notify_all();
    });
}

/// Starts command `handle` and answers the child's handle. The command is
/// released whether it started or not.
#[must_use]
pub fn spawn_start(handle: i64) -> i64 {
    let spec = {
        let mut specs = locked(&SPECS);
        usize::try_from(handle - 1)
            .ok()
            .and_then(|index| specs.get_mut(index))
            .and_then(Option::take)
    };
    let Some(spec) = spec else {
        return Failure::InvalidHandle as i64;
    };
    word((|| {
        let mut child = command(&spec)?.spawn()?;
        let outputs = [
            child.stdout.take().map(drain),
            child.stderr.take().map(drain),
        ];
        let running = Arc::new(Running {
            pid: child.id(),
            input: Mutex::new(child.stdin.take()),
            outputs,
            ending: Mutex::new(None),
        });
        // A child's slot is never reused: an ending announced for a released
        // child would otherwise name the next child given its handle.
        let child_handle = {
            let mut children = locked(&CHILDREN);
            children.push(Some(Arc::clone(&running)));
            length(children.len())
        };
        reap(child_handle, child, running);
        Ok(child_handle)
    })())
}

fn running(handle: i64) -> Result<Arc<Running>, Failure> {
    let children = locked(&CHILDREN);
    usize::try_from(handle - 1)
        .ok()
        .and_then(|index| children.get(index))
        .and_then(Option::clone)
        .ok_or(Failure::InvalidHandle)
}

/// Waits up to `timeout_nanos` (forever when negative) for child `handle`
/// to end, and answers its exit status, [`SIGNALLED`] minus the signal that
/// ended it, or [`STILL_RUNNING`].
#[must_use]
pub fn child_wait(handle: i64, timeout_nanos: i64) -> i64 {
    word((|| {
        let running = running(handle)?;
        let deadline = deadline(timeout_nanos);
        let mut ended = locked(&ENDED);
        loop {
            if let Some(ending) = *locked(&running.ending) {
                return Ok(ending);
            }
            let (guard, waiting) = wait_until(&ENDING, ended, deadline);
            ended = guard;
            if !waiting {
                return Ok(STILL_RUNNING);
            }
        }
    })())
}

/// Waits up to `timeout_nanos` (forever when negative) for any started
/// child to end, and answers its handle, or 0 when none ended in time. Each
/// ending is answered once; a released child's is skipped.
#[must_use]
pub fn child_wait_any(timeout_nanos: i64) -> i64 {
    let deadline = deadline(timeout_nanos);
    let mut ended = locked(&ENDED);
    loop {
        while let Some(handle) = ended.pop_front() {
            if running(handle).is_ok() {
                return handle;
            }
        }
        let (guard, waiting) = wait_until(&ENDING, ended, deadline);
        ended = guard;
        if !waiting {
            return 0;
        }
    }
}

/// Moves up to `destination.len()` bytes child `handle` wrote to piped
/// stream `stream` (1 or 2) into `destination`, waiting up to
/// `timeout_nanos` for the first. Answers the count, 0 once the stream is
/// over, or [`WOULD_BLOCK`].
#[must_use]
pub fn child_read(handle: i64, stream: i64, destination: &mut [u8], timeout_nanos: i64) -> i64 {
    word((|| {
        let running = running(handle)?;
        let index = match stream {
            1 => 0,
            2 => 1,
            _ => return Err(Failure::NotPiped),
        };
        let pipe = running.outputs[index].as_ref().ok_or(Failure::NotPiped)?;
        let deadline = deadline(timeout_nanos);
        let mut drained = locked(&pipe.drained);
        loop {
            if !drained.bytes.is_empty() {
                let count = destination.len().min(drained.bytes.len());
                for (slot, byte) in destination.iter_mut().zip(drained.bytes.drain(..count)) {
                    *slot = byte;
                }
                return Ok(length(count));
            }
            if drained.over || destination.is_empty() {
                return Ok(0);
            }
            let (guard, waiting) = wait_until(&pipe.arrived, drained, deadline);
            drained = guard;
            if !waiting {
                return Ok(WOULD_BLOCK);
            }
        }
    })())
}

/// Writes all of `data` to child `handle`'s piped input, and answers the
/// count.
#[must_use]
pub fn child_write(handle: i64, data: &[u8]) -> i64 {
    word((|| {
        let running = running(handle)?;
        let mut input = locked(&running.input);
        let pipe = input.as_mut().ok_or(Failure::NotPiped)?;
        pipe.write_all(data)?;
        Ok(length(data.len()))
    })())
}

/// Closes child `handle`'s piped input, so it reads the end of it.
#[must_use]
pub fn child_close_input(handle: i64) -> i64 {
    word(running(handle).map(|running| {
        locked(&running.input).take();
        1
    }))
}

/// Sends `signal` to child `handle`. Off Unix every signal kills.
#[must_use]
pub fn child_signal(handle: i64, signal: Signal) -> i64 {
    word((|| {
        let running = running(handle)?;
        if locked(&running.ending).is_some() {
            return Ok(1);
        }
        send(running.pid, signal)?;
        Ok(1)
    })())
}

#[cfg(unix)]
fn send(pid: u32, signal: Signal) -> Result<(), Failure> {
    use rustix::process::{Pid, Signal as Number, kill_process};
    let number = match signal {
        Signal::Interrupt => Number::INT,
        Signal::Terminate => Number::TERM,
        Signal::Kill => Number::KILL,
    };
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Failure::InvalidHandle)?;
    kill_process(pid, number).map_err(|error| std::io::Error::from(error).into())
}

#[cfg(not(unix))]
fn send(pid: u32, _signal: Signal) -> Result<(), Failure> {
    let status = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()?;
    if status.success() { Ok(()) } else { Err(Failure::Io) }
}

/// Forgets child `handle`. A child still running is left to run.
#[must_use]
pub fn child_release(handle: i64) -> i64 {
    let mut children = locked(&CHILDREN);
    let slot = usize::try_from(handle - 1)
        .ok()
        .and_then(|index| children.get_mut(index));
    match slot {
        Some(slot @ Some(_)) => {
            *slot = None;
            1
        }
        _ => Failure::InvalidHandle as i64,
    }
}

/// How many threads the host runs at once for this process: its processor
/// count, within any limit set on the process. At least 1.
#[must_use]
pub fn processor_count() -> i64 {
    std::thread::available_parallelism()
        .map_or(1, |count| i64::try_from(count.get()).unwrap_or(i64::MAX))
}

/// The process's working directory, as bytes.
#[must_use]
pub fn current_directory() -> Option<Vec<u8>> {
    std::env::current_dir()
        .ok()
        .map(|path| path.into_os_string().into_encoded_bytes())
}
