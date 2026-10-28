//! The driver flags a language declares (section 5.5): their names checked
//! once, their defaults seeded into a session, and each value replaced only
//! by one of the same type.

use super::{Session, emit};
use crate::definition::{DriverFlag, DriverFlagValue};
use std::collections::{HashMap, HashSet};

impl Session {
    /// Validates the names and uniqueness of a language's declared flags.
    ///
    /// # Errors
    /// Returns a message naming the first invalid or duplicate flag.
    pub fn validate_driver_flags(flags: &[DriverFlag]) -> Result<(), String> {
        let mut names = HashSet::new();
        for flag in flags {
            let name = flag.name.as_str();
            let mut bytes = name.bytes();
            let valid = bytes.next().is_some_and(|first| first.is_ascii_lowercase())
                && bytes
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !name.ends_with('-')
                && !name.contains("--");
            if !valid || name.starts_with("no-") {
                return Err(format!(
                    "invalid driver flag name `{name}`; expected a lowercase hyphenated identifier not starting with `no-`"
                ));
            }
            if matches!(
                name,
                "emit"
                    | "help"
                    | "version"
                    | "target"
                    | "output"
                    | "export"
                    | "inputs"
                    | "program"
                    | "check"
                    | "stdin"
                    | "width"
                    | "tab-size"
                    | "tabs"
                    | "generate-completion-script"
            ) {
                return Err(format!("driver flag name `{name}` is reserved"));
            }
            if !names.insert(name) {
                return Err(format!("duplicate driver flag name `{name}`"));
            }
        }
        Ok(())
    }

    /// Replaces this session's flag set with the declared defaults.
    ///
    /// # Errors
    /// Returns a message when the declared flags are invalid.
    pub fn initialize_flags(&mut self, flags: &[DriverFlag]) -> Result<(), String> {
        Self::validate_driver_flags(flags)?;
        let emit = self.flags.remove(emit::EMIT_FLAG);
        self.flags = flags
            .iter()
            .map(|flag| (flag.name.clone(), flag.default_value.clone()))
            .chain(emit.map(|value| (emit::EMIT_FLAG.to_owned(), value)))
            .collect();
        Ok(())
    }

    /// Replaces one declared flag's effective value, preserving its type.
    ///
    /// # Errors
    /// Returns a message when the flag is unknown or the value has the wrong type.
    pub fn set_flag(&mut self, name: &str, value: DriverFlagValue) -> Result<(), String> {
        let Some(current) = self.flags.get(name) else {
            return Err(format!("unknown driver flag `{name}`"));
        };
        if std::mem::discriminant(current) != std::mem::discriminant(&value) {
            return Err(format!(
                "driver flag `{name}` expects {}, not {}",
                driver_flag_kind(current),
                driver_flag_kind(&value)
            ));
        }
        self.flags.insert(name.to_owned(), value);
        Ok(())
    }

    /// Returns the effective value of a declared driver flag.
    #[must_use]
    pub fn flag(&self, name: &str) -> Option<&DriverFlagValue> {
        self.flags.get(name)
    }

    /// Returns the effective driver flags for a phase-scoped service snapshot.
    #[must_use]
    pub fn driver_flags(&self) -> &HashMap<String, DriverFlagValue> {
        &self.flags
    }
}

fn driver_flag_kind(value: &DriverFlagValue) -> &'static str {
    match value {
        DriverFlagValue::Boolean(_) => "Boolean",
        DriverFlagValue::Integer(_) => "Integer",
        DriverFlagValue::String(_) => "String",
    }
}
