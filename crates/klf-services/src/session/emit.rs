//! The output kind a compilation produces, exposed to the definition as the
//! kernel-owned `emit` flag.
//!
//! `emit` is reserved for the uniform driver interface (section 5.5), so no
//! definition can declare it. The kernel seeds it with `bin` and the driver
//! replaces it with the requested `--emit`, which lets a definition tell a
//! program from a library, the way `main` is only required when linking.

use super::Session;
use crate::DriverFlagValue;
use std::collections::HashMap;

pub(super) const EMIT_FLAG: &str = "emit";

pub(super) fn initial_flags() -> HashMap<String, DriverFlagValue> {
    HashMap::from([(
        EMIT_FLAG.to_owned(),
        DriverFlagValue::String("bin".to_owned()),
    )])
}

impl Session {
    /// Records the output the driver was asked for (`bin`, `lib`, `kir`, ...).
    pub fn set_emit(&mut self, emit: &str) {
        self.flags.insert(
            EMIT_FLAG.to_owned(),
            DriverFlagValue::String(emit.to_owned()),
        );
    }
}
