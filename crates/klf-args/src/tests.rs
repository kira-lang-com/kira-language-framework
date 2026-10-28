use super::*;

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

fn tool() -> Command {
    let build = Command::new("build")
        .about("Compile sources")
        .builtin("build")
        .arg(
            Arg::new("inputs", ArgKind::Positional, ValueType::String)
                .many()
                .required(),
        )
        .arg(Arg::new("output", ArgKind::Option, ValueType::String).short('o'))
        .arg(
            Arg::new(
                "emit",
                ArgKind::Option,
                ValueType::Choice(strings(&["kir", "bin"])),
            )
            .default_text("kir")
            .help("What to write"),
        )
        .arg(Arg::new("verbose", ArgKind::Flag, ValueType::Int).short('v'))
        .arg(Arg::new("test", ArgKind::Flag, ValueType::Bool).short('t'))
        .arg(Arg::new("feature", ArgKind::Option, ValueType::String).many())
        .arg(Arg::new("program", ArgKind::Rest, ValueType::String).many());
    let mut doc = Command::new("doc").about("Write documentation");
    doc.action = Action::Definition(0);
    doc.aliases.push("d".to_owned());
    doc.args
        .push(Arg::new("depth", ArgKind::Option, ValueType::Int).required());
    let mut root = Command::new("kira")
        .about("The Kira compiler")
        .subcommand(build)
        .subcommand(doc);
    root.default_subcommand = Some("build".to_owned());
    root.version = Some("0.1.0".to_owned());
    root
}

fn run(arguments: &[&str]) -> Matches {
    match parse(&tool(), &strings(arguments)) {
        Ok(Outcome::Run(matches)) => matches,
        other => panic!("expected a run, got {other:?}"),
    }
}

fn failure(arguments: &[&str]) -> Error {
    parse(&tool(), &strings(arguments)).expect_err("expected an error")
}

#[test]
fn reads_every_spelling_of_an_option() {
    let root = tool();
    let matches = run(&[
        "build",
        "a.kira",
        "-vvt",
        "--emit=bin",
        "-oout",
        "b.kira",
        "--no-test",
    ]);
    assert_eq!(matches.path, vec![0]);
    assert_eq!(
        matches.strings(&root, "inputs"),
        strings(&["a.kira", "b.kira"])
    );
    assert_eq!(matches.int(&root, "verbose"), Some(2));
    assert_eq!(matches.bool(&root, "test"), Some(false));
    assert_eq!(matches.string(&root, "emit").as_deref(), Some("bin"));
    assert_eq!(matches.string(&root, "output").as_deref(), Some("out"));
    let matches = run(&[
        "a.kira",
        "--feature",
        "x",
        "--feature",
        "y",
        "--",
        "-p",
        "q",
    ]);
    assert_eq!(matches.strings(&root, "feature"), strings(&["x", "y"]));
    assert_eq!(matches.strings(&root, "program"), strings(&["-p", "q"]));
}

#[test]
fn routes_through_the_default_subcommand() {
    assert_eq!(run(&["a.kira"]).path, vec![0]);
    assert_eq!(run(&["-o", "out", "a.kira"]).path, vec![0]);
    assert_eq!(run(&["d", "--depth", "2"]).path, vec![1]);
    let root = tool();
    assert_eq!(parse(&root, &[]), Ok(Outcome::Help(Vec::new())));
    assert_eq!(
        parse(&root, &strings(&["help", "doc"])),
        Ok(Outcome::Help(vec![1]))
    );
    assert_eq!(
        parse(&root, &strings(&["doc", "-h"])),
        Ok(Outcome::Help(vec![1]))
    );
    assert_eq!(parse(&root, &strings(&["--version"])), Ok(Outcome::Version));
}

#[test]
fn reports_errors_with_codes_and_suggestions() {
    let error = failure(&["build", "a.kira", "--emti", "kir"]);
    assert_eq!(error.kind, ErrorKind::UnknownOption);
    assert!(
        error.message.contains("did you mean '--emit'"),
        "{}",
        error.message
    );
    assert_eq!(
        failure(&["a.kira", "--emit", "exe"]).kind,
        ErrorKind::InvalidValue
    );
    assert_eq!(failure(&["a.kira", "-o"]).kind, ErrorKind::MissingValue);
    assert_eq!(failure(&["doc"]).kind, ErrorKind::MissingArgument);
    assert_eq!(
        failure(&["doc", "--depth", "2", "extra"]).kind,
        ErrorKind::UnexpectedArgument
    );
    let rendered = failure(&["doc", "--depth", "two"]).render(&tool());
    assert!(
        rendered.starts_with("error: the value 'two' is invalid"),
        "{rendered}"
    );
    assert!(rendered.contains("[KLF-DRV-0042]\nUsage: kira doc --depth <depth>\n"));
}

#[test]
fn lays_out_help_like_argument_parser() {
    let root = tool();
    let text = help::render(&root, &[], false);
    assert_eq!(
        text,
        "OVERVIEW: The Kira compiler\n\n\
         USAGE: kira [<subcommand>]\n\n\
         OPTIONS:\n\
         \x20 --version               Show the version.\n\
         \x20 -h, --help              Show help information.\n\n\
         SUBCOMMANDS:\n\
         \x20 build (default)         Compile sources\n\
         \x20 doc                     Write documentation\n\n\
         \x20 See 'kira help <subcommand>' for detailed help.\n"
    );
    let text = help::render(&root, &[0], false);
    assert!(
        text.contains("USAGE: kira build [<options>] <inputs> ... [-- <program> ...]\n"),
        "{text}"
    );
    assert!(
        text.contains("  --emit <emit>           What to write (values: kir, bin; default: kir)\n")
    );
    assert!(text.contains("  -t, --[no-]test\n"));
}

#[test]
fn round_trips_through_the_codec() {
    let mut root = tool();
    root.discussion = "Line one\n\tand\\two".to_owned();
    root.subcommands[0].args[1].aliases = strings(&["out", "o2"]);
    assert_eq!(codec::decode(&codec::encode(&root)), Some(root));
}

#[test]
fn completes_subcommands_options_and_choices() {
    let root = tool();
    let bash = completion_script(&root, Shell::Bash);
    assert!(bash.contains("':build') path='build' ;;"), "{bash}");
    assert!(
        bash.contains("'build:--emit') COMPREPLY=($(compgen -W 'kir bin' -- \"$cur\")); return ;;")
    );
    assert!(bash.contains("complete -F _kira kira"));
    let zsh = completion_script(&root, Shell::Zsh);
    assert!(zsh.starts_with("#compdef kira\n"));
    assert!(zsh.contains("'build:Compile sources'"));
    let fish = completion_script(&root, Shell::Fish);
    assert!(fish.contains("complete -c kira -n '__fish_seen_subcommand_from build' -l emit -x -a 'kir bin' -d 'What to write'"));
}

#[test]
fn names_follow_kebab_case_and_validate() {
    assert_eq!(kebab_case("includeCounter"), "include-counter");
    assert_eq!(kebab_case("HTMLOutput"), "html-output");
    assert_eq!(kebab_case("maxDepth2"), "max-depth2");
    assert_eq!(tool().validate(), Ok(()));
    let mut broken = tool();
    broken.subcommands[1]
        .args
        .push(Arg::new("help", ArgKind::Flag, ValueType::Bool));
    assert!(broken.validate().is_err());
}
