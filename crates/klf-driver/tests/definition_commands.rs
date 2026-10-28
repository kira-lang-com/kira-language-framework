//! A definition's own command line (`KLF-CLI.md`): what the kernel reads out
//! of a `ParsableCommand` and running it with parsed values on the VM.

use klf_args::{ArgKind, Outcome, ValueType};
use klf_driver::{Backend, LanguageDefinition};
use klf_services::{DefinitionPackage, DriverFlagValue, Session};

const LANGUAGE: &str = r#"
construct Docs() extends Language<Tree, Program> {
    let name: String = "docs"
    let driver: String = "docs"
    let version: String = "0.2.0"
    let extensions: [String] = ["doc"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Ridl
    let stdRoot: String = ""
    let configuration: CommandConfiguration = CommandConfiguration {
        abstract = "The docs toolchain",
        subcommands = [Doc]
    }

    @Flag(name: .ShortAndLong, help: "Compile test functions")
    let test: Bool = false

    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        var result: [Unit] = []
        for file in inputs {
            result.append(definition_unit(session, file))
        }
        return result
    }

    function parse(session: Session, unit: Unit) -> Tree {
        return definition_tree(session, unit, 0)
    }

    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, 0)
    }

    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        return kir_module_finish(kir_module_new(session, unit, program))
    }
}

enum DocFormat {
    Html
    PlainText
}

struct Doc: ParsableCommand {
    let configuration: CommandConfiguration = CommandConfiguration {
        abstract = "Write documentation",
        aliases = ["d"]
    }

    @Argument(help: "Package roots")
    let paths: [String] = []

    @Option(name: .ShortAndLong, help: "How deep to look")
    let depth: Int = 2

    @Option(help: "Output format")
    let format: DocFormat = .Html

    @Option
    let title: Optional<String>

    @Flag(name: .Short)
    let verbose: Int

    let scale: Int = 10

    function run(self) -> Int {
        var total = depth * 100 + verbose * scale
        for path in paths {
            total = total + utf8Count(path) * 1000
        }
        match format {
            Html -> {}
            PlainText -> { total = total + 5 }
        }
        match title {
            Some(text) -> { total = total + utf8Count(text) * 100000 }
            None -> {}
        }
        return total
    }
}
"#;

fn load() -> LanguageDefinition {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let package = DefinitionPackage::single("language.klfd", LANGUAGE);
    let definition = LanguageDefinition::from_package(&mut session, &package, Backend::Vm);
    assert!(
        session.diagnostics().is_empty(),
        "{:#?}",
        session.diagnostics()
    );
    definition.expect("the definition loads")
}

#[test]
fn commands_and_options_are_read_from_the_definition() {
    let definition = load();
    let descriptor = definition.descriptor();
    let cli = &descriptor.cli;
    assert_eq!(cli.abstract_text, "The docs toolchain");
    let [test] = cli.args.as_slice() else {
        panic!("one Language option, got {:?}", cli.args);
    };
    assert_eq!(
        (test.long.as_deref(), test.short),
        (Some("test"), Some('t'))
    );
    assert_eq!(test.help, "Compile test functions");
    assert!(
        descriptor.flags.iter().any(
            |flag| flag.name == "test" && flag.default_value == DriverFlagValue::Boolean(false)
        )
    );

    let [doc] = cli.subcommands.as_slice() else {
        panic!("one subcommand, got {:?}", cli.subcommands);
    };
    assert_eq!(doc.name, "doc");
    assert_eq!(doc.aliases, vec!["d".to_owned()]);
    let ids: Vec<(&str, ArgKind)> = doc
        .args
        .iter()
        .map(|arg| (arg.id.as_str(), arg.kind))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("paths", ArgKind::Positional),
            ("depth", ArgKind::Option),
            ("format", ArgKind::Option),
            ("title", ArgKind::Option),
            ("verbose", ArgKind::Flag),
        ]
    );
    assert_eq!(doc.args[1].short, Some('d'));
    assert_eq!(doc.args[1].default.as_deref(), Some("2"));
    assert_eq!(
        doc.args[2].value,
        ValueType::Choice(vec!["html".to_owned(), "plain-text".to_owned()])
    );
    assert_eq!(doc.args[2].default.as_deref(), Some("html"));
    assert!(!doc.args[3].required);
    assert_eq!(
        (doc.args[4].long.as_deref(), doc.args[4].short),
        (None, Some('v'))
    );
}

#[test]
fn a_command_runs_with_parsed_values_and_falls_back_to_initial_values() {
    let definition = load();
    let cli = &definition.descriptor().cli;
    let run = |words: &[&str]| {
        let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        let Ok(Outcome::Run(matches)) = klf_args::parse(cli, &words) else {
            panic!("`{words:?}` should run");
        };
        let command = matches.command(cli);
        let klf_args::Action::Definition(index) = command.action else {
            panic!("a definition command");
        };
        definition
            .run_command(index, matches.values)
            .expect("the command runs")
    };
    assert_eq!(run(&["doc"]), 200);
    assert_eq!(
        run(&[
            "d",
            "ab",
            "cde",
            "-d",
            "7",
            "--format",
            "plain-text",
            "-vvv"
        ]),
        5000 + 700 + 5 + 30
    );
    assert_eq!(run(&["doc", "--title", "four"]), 400_000 + 200);
}

#[test]
fn a_native_driver_dispatches_its_commands_help_and_completions() {
    if !klf_driver::toolchain_available() {
        eprintln!("native half skipped: clang toolchain unavailable");
        return;
    }
    let directory = std::env::temp_dir().join(format!("klf-commands-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("test package directory");
    let source = directory.join("language.klfd");
    std::fs::write(&source, LANGUAGE).expect("language source written");
    let binary = directory.join("docs");
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    klf_driver::build_frontend(
        &mut session,
        &source,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("native frontend builds: {error}"));
    let driver = |words: &[&str]| {
        std::process::Command::new(&binary)
            .args(words)
            .output()
            .expect("the driver runs")
    };

    let ran = driver(&["doc", "-d", "1", "--format", "plain-text"]);
    assert_eq!(
        ran.status.code(),
        Some(105),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(driver(&["d", "-vv"]).status.code(), Some(220));

    let help = driver(&["--help"]);
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.starts_with("OVERVIEW: The docs toolchain\n"), "{text}");
    assert!(
        text.contains("  build (default)         Compile sources.\n"),
        "{text}"
    );
    assert!(
        text.contains("  doc                     Write documentation\n"),
        "{text}"
    );
    assert!(!text.contains("lsp-worker"));
    let help = driver(&["help", "build"]);
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(
        text.contains("  -t, --[no-]test         Compile test functions\n"),
        "{text}"
    );

    let wrong = driver(&["doc", "--depth", "deep"]);
    assert_eq!(wrong.status.code(), Some(64));
    let text = String::from_utf8_lossy(&wrong.stderr);
    assert!(
        text.starts_with("docs: error: the value 'deep' is invalid for '--depth'"),
        "{text}"
    );
    assert!(text.contains("[KLF-DRV-0042]"));

    let version = driver(&["--version"]);
    assert_eq!(String::from_utf8_lossy(&version.stdout), "docs 0.2.0\n");
    let script = driver(&["--generate-completion-script", "bash"]);
    assert!(String::from_utf8_lossy(&script.stdout).contains("complete -F _docs docs"));
}
