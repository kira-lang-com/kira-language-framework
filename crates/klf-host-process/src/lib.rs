//! The host side of the `klf.process` kernel services: the arguments the
//! running program was started with, and the commands it runs.
//!
//! A native program's are its own command line. A runner that hosts the
//! program, as the VM runner does, records them first instead: its own
//! command line also names the bytecode it runs. The VM's capability table and
//! the native runtime's `klfsvc_process_*` symbols both answer from here, so a
//! program reads the same arguments whichever backend runs it.

use std::sync::{Mutex, OnceLock};

pub mod children;
pub mod time;

static ARGUMENTS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

static INVOKED_AS: OnceLock<Vec<u8>> = OnceLock::new();

/// Records the hosted program's arguments, its own path excluded. Only a
/// call before any argument is read counts: a program has one command line.
pub fn set_arguments(arguments: Vec<Vec<u8>>) {
    let _ = ARGUMENTS.set(arguments);
}

fn arguments() -> &'static [Vec<u8>] {
    ARGUMENTS.get_or_init(|| {
        std::env::args_os()
            .skip(1)
            .map(std::ffi::OsString::into_encoded_bytes)
            .collect()
    })
}

/// How many arguments the program was started with.
#[must_use]
pub fn argument_count() -> usize {
    arguments().len()
}

/// The argument at `index`, or `None` past the last.
#[must_use]
pub fn argument(index: usize) -> Option<&'static [u8]> {
    arguments().get(index).map(Vec::as_slice)
}

/// Records the path the hosted program was started as. Only a call before it
/// is read counts, as with [`set_arguments`].
pub fn set_invoked_as(path: Vec<u8>) {
    let _ = INVOKED_AS.set(path);
}

/// The path the program was started as: its `argv[0]`, which is what the
/// shell resolved or was handed, not necessarily absolute and not necessarily
/// the file's own name. A program installed under several names reads which
/// one it was run as from here.
#[must_use]
pub fn invoked_as() -> &'static [u8] {
    INVOKED_AS.get_or_init(|| {
        std::env::args_os()
            .next()
            .map(std::ffi::OsString::into_encoded_bytes)
            .unwrap_or_default()
    })
}

/// The command a program is building, and what the last one it ran wrote.
struct Commands {
    building: Vec<std::ffi::OsString>,
    output: Vec<u8>,
    errors: Vec<u8>,
}

static COMMANDS: Mutex<Commands> = Mutex::new(Commands {
    building: Vec::new(),
    output: Vec::new(),
    errors: Vec::new(),
});

fn commands<R>(action: impl FnOnce(&mut Commands) -> R) -> R {
    let mut commands = COMMANDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    action(&mut commands)
}

/// Appends `argument` to the command being built; the first is the program.
/// Returns false when the platform cannot name it, as a path or argument
/// that is not UTF-8 off Unix.
#[must_use]
pub fn command_push(argument: &[u8]) -> bool {
    #[cfg(unix)]
    let argument = Some(
        <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(argument).to_os_string(),
    );
    #[cfg(not(unix))]
    let argument = std::str::from_utf8(argument)
        .ok()
        .map(std::ffi::OsString::from);
    let Some(argument) = argument else {
        return false;
    };
    commands(|commands| commands.building.push(argument));
    true
}

/// How a command ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ending {
    /// It exited with this status.
    Exited(i64),
    /// It ended without a status, as a Unix process a signal stops does.
    Stopped,
    /// It could not be started, or there was no command to start.
    Unstarted,
}

impl Ending {
    /// The word `klf.process.command_run` answers with.
    #[must_use]
    pub const fn word(self) -> i64 {
        match self {
            Self::Exited(status) => status,
            Self::Stopped => -1,
            Self::Unstarted => -2,
        }
    }
}

/// Runs the command built so far to its end, then starts building the next
/// one. A `captured` command gets no input and its output and errors are
/// kept for [`command_output`]; any other shares this program's input and
/// output and leaves nothing to read.
#[must_use]
pub fn command_run(captured: bool) -> Ending {
    let command = commands(|commands| std::mem::take(&mut commands.building));
    let Some((program, arguments)) = command.split_first() else {
        return Ending::Unstarted;
    };
    let mut process = std::process::Command::new(program);
    process.args(arguments);
    let ran = if captured {
        process
            .stdin(std::process::Stdio::null())
            .output()
            .map(|output| (output.status, output.stdout, output.stderr))
    } else {
        process
            .status()
            .map(|status| (status, Vec::new(), Vec::new()))
    };
    commands(|commands| {
        if let Ok((status, output, errors)) = ran {
            commands.output = output;
            commands.errors = errors;
            status
                .code()
                .map_or(Ending::Stopped, |code| Ending::Exited(i64::from(code)))
        } else {
            commands.output.clear();
            commands.errors.clear();
            Ending::Unstarted
        }
    })
}

/// Calls `read` with what the last command wrote to its output (stream 1)
/// or its errors (stream 2); `None` for any other stream.
pub fn command_output<R>(stream: i64, read: impl FnOnce(&[u8]) -> R) -> Option<R> {
    commands(|commands| match stream {
        1 => Some(read(&commands.output)),
        2 => Some(read(&commands.errors)),
        _ => None,
    })
}
