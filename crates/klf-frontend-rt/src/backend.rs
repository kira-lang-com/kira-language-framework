use crate::{FrontendMetadata, arguments::Invocation};
use klf_services::DriverFlagValue;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Backend {
    Runtime,
    Native,
}

impl Backend {
    pub(crate) fn select(
        metadata: &FrontendMetadata,
        invocation: &mut Invocation,
    ) -> Result<Self, String> {
        if metadata.name != "kira" {
            return Ok(Self::Native);
        }
        let declared = metadata.flags.iter().find(|flag| flag.name == "backend");
        let name = invocation
            .flag_overrides
            .get("backend")
            .or_else(|| declared.map(|flag| &flag.default_value));
        let selected = match name {
            None => Self::Native,
            Some(DriverFlagValue::String(value)) if value == "native" => Self::Native,
            Some(DriverFlagValue::String(value)) if value == "runtime" => Self::Runtime,
            Some(DriverFlagValue::String(value)) => {
                return Err(format!(
                    "invalid `--backend {value}`; expected `runtime` or `native`"
                ));
            }
            _ => return Err("`--backend` must be a string option".into()),
        };

        if invocation.flag_overrides.contains_key("backend") {
            if !invocation.emit_explicit {
                match (selected, invocation.emit.as_str()) {
                    (Self::Runtime, "bin") => invocation.emit = "klbc".into(),
                    (Self::Runtime, "kir") if invocation.output.is_none() => {
                        invocation.emit = "klbc".into();
                    }
                    (Self::Native, "kir") if invocation.output.is_none() => {
                        invocation.emit = "bin".into();
                    }
                    _ => {}
                }
            }
            if matches!(
                (selected, invocation.emit.as_str()),
                (Self::Runtime, "bin" | "lib" | "shared") | (Self::Native, "klbc")
            ) {
                return Err(format!(
                    "`--backend {}` cannot emit `{}`; use `--emit {}`",
                    selected.name(),
                    invocation.emit,
                    match selected {
                        Self::Runtime => "klbc",
                        Self::Native => "bin",
                    },
                ));
            }
        }
        Ok(selected)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Native => "native",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_services::{DriverFlag, DriverFlagValue};

    fn metadata() -> FrontendMetadata {
        FrontendMetadata {
            name: "kira".into(),
            driver: "kira".into(),
            flags: vec![DriverFlag {
                name: "backend".into(),
                default_value: DriverFlagValue::String("native".into()),
            }],
            ..FrontendMetadata::default()
        }
    }

    fn invocation(backend: Option<&str>, emit: &str, explicit: bool) -> Invocation {
        let mut invocation = Invocation {
            emit: emit.into(),
            emit_explicit: explicit,
            ..Invocation::default()
        };
        if let Some(backend) = backend {
            invocation
                .flag_overrides
                .insert("backend".into(), DriverFlagValue::String(backend.into()));
        }
        invocation
    }

    #[test]
    fn backend_selects_native_or_runtime_without_changing_legacy_build_defaults() {
        let mut default = invocation(None, "kir", false);
        assert_eq!(
            Backend::select(&metadata(), &mut default),
            Ok(Backend::Native)
        );
        assert_eq!(default.emit, "kir");

        let mut native = invocation(Some("native"), "kir", false);
        assert_eq!(
            Backend::select(&metadata(), &mut native),
            Ok(Backend::Native)
        );
        assert_eq!(native.emit, "bin");

        let mut runtime = invocation(Some("runtime"), "kir", false);
        assert_eq!(
            Backend::select(&metadata(), &mut runtime),
            Ok(Backend::Runtime)
        );
        assert_eq!(runtime.emit, "klbc");

        let mut run = invocation(Some("runtime"), "run", false);
        assert_eq!(Backend::select(&metadata(), &mut run), Ok(Backend::Runtime));
        assert_eq!(run.emit, "run");
    }

    #[test]
    fn backend_rejects_invalid_and_conflicting_modes() {
        assert!(Backend::select(&metadata(), &mut invocation(Some("vm"), "run", false)).is_err());
        assert!(
            Backend::select(&metadata(), &mut invocation(Some("runtime"), "bin", true)).is_err()
        );
        assert!(
            Backend::select(&metadata(), &mut invocation(Some("native"), "klbc", true)).is_err()
        );
    }

    #[test]
    fn other_languages_keep_their_own_backend_options() {
        let mut other = metadata();
        other.name = "karc".into();
        let mut invocation = invocation(Some("llvm"), "kir", false);
        assert_eq!(
            Backend::select(&other, &mut invocation),
            Ok(Backend::Native)
        );
        assert_eq!(invocation.emit, "kir");
    }
}
