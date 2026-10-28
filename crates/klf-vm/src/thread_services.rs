//! The `klf.thread` kernel services on the VM (section 18.9).
//!
//! An instance runs its bytecode on one thread, so it starts no other: a
//! spawn is refused, the program is thread 1, and parking returns at once,
//! as a park may return spuriously. A program that needs threads runs
//! natively.

use crate::Capabilities;

/// The platform refused a thread.
const REFUSED: u64 = (-1_i64).cast_unsigned();
/// The handle or thread number names nothing.
const UNKNOWN: u64 = (-2_i64).cast_unsigned();
/// The instance's only thread.
const MAIN: u64 = 1;

pub(crate) fn register(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_thread_spawn", |_| REFUSED);
    capabilities.register("klfsvc_thread_join", |_| UNKNOWN);
    capabilities.register("klfsvc_thread_detach", |_| UNKNOWN);
    capabilities.register("klfsvc_thread_current", |_| MAIN);
    capabilities.register("klfsvc_thread_park", |arguments| {
        // A timed park waits out its time; one without a timeout has no other
        // thread to wake it.
        let timeout = arguments
            .first()
            .map_or(0, |nanos| i64::from_ne_bytes(nanos.to_ne_bytes()));
        if timeout > 0 {
            klf_host_process::time::sleep_nanos(timeout);
        }
        1
    });
    capabilities.register("klfsvc_thread_unpark", |arguments| {
        if arguments.first() == Some(&MAIN) { 1 } else { UNKNOWN }
    });
    capabilities.register("klfsvc_thread_yield", |_| 0);
    capabilities.register("klfsvc_thread_parallelism", |_| 1);
}
