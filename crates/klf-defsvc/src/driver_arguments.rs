use crate::ServiceSink;
use klf_rt::MAX_SERVICE_BUFFER_BYTES;
use klf_services::DriverRequest;
use std::sync::{Arc, Mutex};

/// The KIR entry the KLF Kira lowerer generates around a `compatDriver`
/// override. It takes the argument count and returns 0 for `None`, 1 for
/// `Some`, after pushing each normalized argument through
/// `klfsvc_driver_request_push`.
pub const COMPAT_DRIVER_ENTRY: &str = "__klf_compat_driver";

/// The KIR entry the KLF Kira lowerer generates around a `runProgram` hook.
/// It takes the command's argument count and the count of every argument the
/// services serve, the driver arguments following the command's, and
/// returns the hook's exit status.
pub const RUN_PROGRAM_ENTRY: &str = "__klf_run_program";

/// Registers a phase-call vector of owned UTF-8 arguments.
///
/// The returned word is the element count. The definition reconstructs its
/// `[String]` value by querying lengths and bulk-copying each element into
/// backend-owned memory.
pub fn register_driver_arguments(arguments: Vec<String>, services: &mut ServiceSink) -> u64 {
    let arguments = Arc::new(arguments);
    let count = u64::try_from(arguments.len()).unwrap_or(u64::MAX);
    let lengths = Arc::clone(&arguments);
    services.register("klfsvc_driver_argument_len", move |words| {
        let [index] = words else { return u64::MAX };
        let Ok(index) = usize::try_from(*index) else {
            return u64::MAX;
        };
        lengths
            .get(index)
            .filter(|value| value.len() <= MAX_SERVICE_BUFFER_BYTES)
            .and_then(|value| u64::try_from(value.len()).ok())
            .unwrap_or(u64::MAX)
    });
    services.register_with_memory("klfsvc_driver_argument_copy", move |words, memory| {
        let [index, address, length] = words else {
            return 0;
        };
        let Ok(index) = usize::try_from(*index) else {
            return 0;
        };
        let Some(value) = arguments.get(index) else {
            return 0;
        };
        if value.len() > MAX_SERVICE_BUFFER_BYTES
            || usize::try_from(*length).ok() != Some(value.len())
        {
            return 0;
        }
        u64::from(memory.write_buffer(*address, value.as_bytes()))
    });
    count
}

/// One call of a definition's `compatDriver` override: the services it needs
/// and the arguments it hands back.
pub struct CompatDriverCall {
    count: u64,
    pushed: Arc<Mutex<Result<Vec<String>, String>>>,
}

impl CompatDriverCall {
    /// Registers the argument and request services for `arguments`.
    pub fn register(arguments: Vec<String>, services: &mut ServiceSink) -> Self {
        let count = register_driver_arguments(arguments, services);
        let pushed = Arc::new(Mutex::new(Ok(Vec::new())));
        let sink = Arc::clone(&pushed);
        services.register_with_memory("klfsvc_driver_request_push", move |words, memory| {
            let [address, length] = words else {
                return 0;
            };
            let Ok(mut pushed) = sink.lock() else {
                return 0;
            };
            let Ok(arguments) = pushed.as_mut() else {
                return 0;
            };
            let Some(length) = usize::try_from(*length)
                .ok()
                .filter(|length| *length <= MAX_SERVICE_BUFFER_BYTES)
            else {
                *pushed = Err("compatDriver returned an argument over the buffer limit".into());
                return 0;
            };
            let mut bytes = vec![0; length];
            if !memory.read_buffer(*address, &mut bytes) {
                *pushed = Err("compatDriver returned an unreadable argument".into());
                return 0;
            }
            if let Ok(text) = String::from_utf8(bytes) {
                arguments.push(text);
                1
            } else {
                *pushed = Err("compatDriver returned an argument that is not UTF-8".into());
                0
            }
        });
        Self { count, pushed }
    }

    /// The word the generated entry takes: the argument count.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// Decodes the entry's result word.
    ///
    /// # Errors
    /// Returns a message when the entry returned something other than 0 or 1,
    /// or when a pushed argument could not be read.
    pub fn finish(self, word: u64) -> Result<Option<DriverRequest>, String> {
        let pushed = self
            .pushed
            .lock()
            .map_err(|_| "compatDriver request lock poisoned".to_owned())?
            .clone()?;
        match word {
            0 => Ok(None),
            1 => Ok(Some(DriverRequest { arguments: pushed })),
            other => Err(format!(
                "compatDriver entry returned {}, expected 0 or 1",
                other.cast_signed()
            )),
        }
    }
}

/// One call of a definition's `runProgram` hook: the command that runs the
/// built program and the driver arguments, served as one argument list.
pub struct RunProgramCall {
    words: [u64; 2],
}

impl RunProgramCall {
    /// Registers the argument services for `command` and then
    /// `driver_arguments`.
    pub fn register(
        command: &[String],
        driver_arguments: &[String],
        services: &mut ServiceSink,
    ) -> Self {
        let arguments = command.iter().chain(driver_arguments).cloned().collect();
        let total = register_driver_arguments(arguments, services);
        let command = u64::try_from(command.len()).unwrap_or(u64::MAX);
        Self {
            words: [command, total],
        }
    }

    /// The words the generated entry takes: the command's argument count and
    /// the total.
    #[must_use]
    pub const fn words(&self) -> [u64; 2] {
        self.words
    }

    /// The exit status the entry's result word carries; one past the range
    /// of a status is a failure.
    #[must_use]
    pub fn status(word: u64) -> i32 {
        i32::try_from(word.cast_signed()).unwrap_or(1)
    }
}
