use super::*;

const REQUIRED: &str = "\
function klfFrontendUnits(a: I64, b: I64) -> I64\n\
function klfFrontendParse(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendElaborate(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendLower(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendBegin() -> I64\n\
function klfFrontendDispose() -> I64\n";

#[test]
fn repeated_frontend_reads_reuse_the_registered_metadata_source() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "klf-frontend-read-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("app")).expect("package directories");
    std::fs::write(root.join("package.kira"), "").expect("package manifest");
    std::fs::write(
        root.join("app/Language.kira"),
        r#"
construct Calc() extends Language<Tree, Program> {
    let name: String = "calc"
    let driver: String = "calc-driver"
    let version: String = "1.0.0"
    let extensions: [String] = ["calc"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Platform
    let stdRoot: String = "std"
    function units(session: Session, inputs: [SourceFile]) -> [Unit] { return [] }
    function parse(session: Session, unit: Unit) -> Tree { return [] }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program { return [] }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule { return [] }
}
"#,
    )
    .expect("language source");
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut session = Session::new(target);

    let (first, _) = read_package(&mut session, &root).expect("initial package read");
    let source_count = first.sources().len();
    let registered_file_count = session.files().len();
    let first_stats = session.query_stats();

    let (second, _) = read_package(&mut session, &root).expect("repeated package read");
    let second_stats = session.query_stats();
    assert_eq!(second.sources().len(), source_count);
    assert_eq!(session.files().len(), registered_file_count);
    assert_eq!(second_stats.executions, first_stats.executions);
    assert_eq!(second_stats.cache_hits - first_stats.cache_hits, 1);
    std::fs::remove_dir_all(root).expect("remove test package");
}

#[test]
fn format_entry_is_required_only_when_the_hook_is_declared() {
    let parse =
        |text: &str| klf_ridl::parse(FileId::from_u32(0), "KLFFrontend", text).expect("RIDL");
    assert!(validate_entries(&parse(REQUIRED), false, false, false, false, false).is_ok());
    assert!(
        validate_entries(&parse(REQUIRED), true, false, false, false, false)
            .unwrap_err()
            .contains("klfFrontendFormat")
    );
    let wrong = format!("{REQUIRED}function klfFrontendFormat(a: I64) -> I64\n");
    assert!(
        validate_entries(&parse(&wrong), true, false, false, false, false)
            .unwrap_err()
            .contains("wrong RIDL signature")
    );
    let correct = format!("{REQUIRED}function klfFrontendFormat(a: I64, b: I64, c: I64) -> I64\n");
    assert!(validate_entries(&parse(&correct), true, false, false, false, false).is_ok());
    assert!(
        validate_entries(&parse(&correct), true, true, false, false, false)
            .unwrap_err()
            .contains("klfFrontendCompatDriver")
    );
    let both = format!("{correct}function klfFrontendCompatDriver(count: I64) -> I64\n");
    assert!(validate_entries(&parse(&both), true, true, false, false, false).is_ok());
    assert!(
        validate_entries(&parse(&both), true, true, true, false, false)
            .unwrap_err()
            .contains("klfFrontendRunProgram")
    );
    let all = format!("{both}function klfFrontendRunProgram(count: I64, total: I64) -> I64\n");
    assert!(validate_entries(&parse(&all), true, true, true, false, false).is_ok());
}

#[test]
fn shader_emit_requires_a_matching_entry_and_dispatches_it() {
    let parse =
        |text: &str| klf_ridl::parse(FileId::from_u32(0), "KLFFrontend", text).expect("RIDL");
    assert!(
        validate_entries(&parse(REQUIRED), false, false, false, false, true)
            .unwrap_err()
            .contains("klfFrontendEmit")
    );
    let contract = format!("{REQUIRED}function klfFrontendEmit(unit: I64) -> I64\n");
    assert!(validate_entries(&parse(&contract), false, false, false, false, true).is_ok());
    let source = trampoline("metadata", false, false, false, false, true);
    assert!(source.contains("case 6: return KLFFrontend_klfFrontendEmit(a);"));
}

#[test]
fn package_module_after_kira_dependencies_overrides_construct_name() {
    let scratch = Scratch::new().unwrap();
    scratch
        .write(
            "package.kira",
            br#"construct Original extends Package {
            let dependencies: [Dependency] = [Dependency { name: "KLF", path: "../KLF" }]
            let moduleRoot: String = "AnotherFrontend"
        }"#,
        )
        .unwrap();
    let mut session = Session::new("x86_64-pc-windows-msvc".parse().unwrap());
    let manifest = DefinitionPackage::single(
        "package.kira",
        std::fs::read_to_string(scratch.0.join("package.kira")).unwrap(),
    );
    assert_eq!(
        package_module(&mut session, &scratch.0, &manifest).unwrap(),
        "AnotherFrontend"
    );
    let before_noop = session.query_stats();
    assert_eq!(
        package_module(&mut session, &scratch.0, &manifest).unwrap(),
        "AnotherFrontend"
    );
    let after_noop = session.query_stats();
    assert_eq!(after_noop.executions, before_noop.executions);
    assert_eq!(after_noop.cache_hits - before_noop.cache_hits, 1);

    let changed = r#"construct Original extends Package {
            let dependencies: [Dependency] = [Dependency { name: "KLF", path: "../KLF" }]
            let moduleRoot: String = "ChangedFrontend"
        }"#;
    scratch.write("package.kira", changed.as_bytes()).unwrap();
    let changed_package = DefinitionPackage::single("package.kira", changed);
    let before_edit = session.query_stats();
    assert_eq!(
        package_module(&mut session, &scratch.0, &changed_package).unwrap(),
        "ChangedFrontend"
    );
    let after_edit = session.query_stats();
    assert_eq!(after_edit.executions - before_edit.executions, 2);
    assert_eq!(after_edit.invalidated - before_edit.invalidated, 2);
}
