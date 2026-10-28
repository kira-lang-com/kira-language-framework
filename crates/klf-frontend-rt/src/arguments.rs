//! The driver's command line (`KLF-CLI.md` section 5), read with the tree
//! `klf_defsvc::driver_cli` builds from the frontend's metadata.

use klf_defsvc::FrontendMetadata;
pub(crate) use klf_defsvc::driver_cli::Invocation;

/// Reads build-style driver arguments, as a language server workspace gives
/// them.
pub(crate) fn parse_arguments(
    arguments: &[String],
    metadata: &FrontendMetadata,
) -> Result<Invocation, String> {
    klf_defsvc::driver_cli::parse_build_arguments(arguments, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_args::Outcome;
    use klf_defsvc::driver_cli::{FileModeSplit, command_tree, invocation, split_file_modes};
    use klf_services::{DriverFlag, DriverFlagValue, FileMode, Session, select_driver_arguments};
    use klf_target::Target;
    use std::str::FromStr;

    fn metadata() -> FrontendMetadata {
        let flag = |name: &str, default_value| DriverFlag {
            name: name.to_owned(),
            default_value,
        };
        FrontendMetadata {
            name: "karc".to_owned(),
            driver: "karc".to_owned(),
            flags: vec![
                flag("warnings", DriverFlagValue::Boolean(false)),
                flag("jobs", DriverFlagValue::Integer(4)),
                flag("sysroot", DriverFlagValue::String("std".to_owned())),
                flag("cache", DriverFlagValue::Boolean(true)),
            ],
            file_modes: vec![FileMode {
                name: "arc".to_owned(),
                pragma: "karc arc".to_owned(),
                flag: "-fkarc-arc".to_owned(),
                extensions: vec!["krc".to_owned()],
                memory: None,
                convention: None,
            }],
            ..FrontendMetadata::default()
        }
    }

    fn parse(arguments: &[&str]) -> Result<Invocation, String> {
        let metadata = metadata();
        let arguments: Vec<String> = arguments.iter().map(|a| (*a).to_owned()).collect();
        let root = command_tree(&metadata);
        let FileModeSplit { rest, globs } = split_file_modes(&arguments, &metadata.file_modes)?;
        match klf_args::parse(&root, &rest) {
            Ok(Outcome::Run(matches)) => Ok(invocation(&root, &matches, &metadata, globs)),
            Ok(other) => Err(format!("{other:?}")),
            Err(error) => Err(error.render(&root)),
        }
    }

    #[test]
    fn declared_flags_parse_and_apply_defaults_and_overrides() {
        let arguments = [
            "--warnings",
            "--no-warnings",
            "--jobs=12",
            "--jobs",
            "14",
            "--sysroot=first",
            "--sysroot",
            "custom std",
            "-fkarc-arc=src/**/*.cpp",
            "input.cpp",
        ];
        let owned = arguments.map(str::to_owned);
        assert_eq!(select_driver_arguments(&owned, None), owned);
        assert_eq!(
            select_driver_arguments(
                &owned,
                Some(klf_services::DriverRequest {
                    arguments: vec!["input.mini".to_owned()],
                }),
            ),
            ["input.mini"]
        );
        let invocation = parse(&arguments).unwrap();
        let mut session = Session::new(Target::from_str("aarch64-apple-darwin").unwrap());
        session.initialize_flags(&metadata().flags).unwrap();
        for (name, value) in invocation.flag_overrides {
            session.set_flag(&name, value).unwrap();
        }
        assert_eq!(
            session.flag("warnings"),
            Some(&DriverFlagValue::Boolean(false))
        );
        assert_eq!(session.flag("jobs"), Some(&DriverFlagValue::Integer(14)));
        assert_eq!(
            session.flag("sysroot"),
            Some(&DriverFlagValue::String("custom std".to_owned()))
        );
        assert_eq!(session.flag("cache"), Some(&DriverFlagValue::Boolean(true)));
        assert_eq!(
            invocation.file_mode_globs,
            vec![("-fkarc-arc".to_owned(), "src/**/*.cpp".to_owned())]
        );
        assert!(parse(&["a.cpp", "--jobs=invalid"]).is_err());
        assert!(parse(&["a.cpp", "--jobs"]).is_err());
        assert!(parse(&["a.cpp", "--unknown"]).is_err());
    }

    #[test]
    fn subcommands_select_what_to_emit() {
        assert_eq!(parse(&["a.cpp"]).unwrap().emit, "kir");
        assert_eq!(parse(&["a.cpp", "-o", "a.klbc"]).unwrap().emit, "klbc");
        assert_eq!(parse(&["build", "a.cpp", "-o", "a"]).unwrap().emit, "bin");
        assert_eq!(
            parse(&["build", "a.cpp", "--emit", "lib"]).unwrap().emit,
            "lib"
        );
        let run = parse(&["run", "a.cpp", "--", "-x", "y"]).unwrap();
        assert_eq!((run.emit.as_str(), run.program_arguments.len()), ("run", 2));
        assert_eq!(parse(&["check", "a.cpp"]).unwrap().emit, "check");
        assert!(klf_defsvc::driver_cli::validate(&metadata()).is_ok());
    }
}
