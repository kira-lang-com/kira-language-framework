//! The line table a Kira program actually gets. The frontend fills in `!loc`
//! from the spans of the source it walks (section 12.15), the passes carry it,
//! and the backend turns it into the DWARF a debugger reads (section 16.3).

use klf_kir_passes::monomorphize;
use klf_kira_lower::{elaborate_file, lower_elaborated, parse_unit};
use klf_services::{DefinitionUnit, Session};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Line 1 is blank, line 2 opens `area`, line 3 is `let base = w * h` and
// line 4 is `return base + 1`.
const SOURCE: &str = "
function area(w: Int, h: Int) -> Int {
    let base = w * h
    return base + 1
}

function main() -> Int {
    return area(4, 5)
}
";

const LET_LINE: u32 = 3;
const RETURN_LINE: u32 = 4;

fn lower(source: &str, module_name: &str) -> (klf_kir::Module, Session) {
    let target = "x86_64-unknown-linux-gnu"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut session = Session::new(target.clone());
    let unit = DefinitionUnit::new(session.add_file("area.kira", source));
    let parsed = parse_unit(&mut session, &unit).expect("the source parses");
    assert!(
        session.diagnostics().is_empty(),
        "parse errors: {:?}",
        session.diagnostics()
    );
    let elaborated = elaborate_file(&mut session, &parsed.file).expect("elaborates");
    let module = lower_elaborated(&mut session, module_name, target, &elaborated).expect("lowers");
    (monomorphize(&module), session)
}

/// Every instruction of `name` with the source line it was lowered from.
fn lines_of(module: &klf_kir::Module, name: &str) -> Vec<u32> {
    module
        .functions
        .iter()
        .find(|function| function.name.as_str() == name)
        .unwrap_or_else(|| panic!("{name} is lowered"))
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| instruction.debug_location.as_deref().map(|l| l.line))
        .collect()
}

fn located(module: &klf_kir::Module) -> usize {
    module
        .functions
        .iter()
        .flat_map(|function| function.blocks.iter())
        .flat_map(|block| block.instructions.iter())
        .filter(|instruction| instruction.debug_location.is_some())
        .count()
}

#[test]
fn an_operand_does_not_claim_the_operation_its_parent_emits() {
    let (module, _) = lower(SOURCE, "Area");
    let lines = lines_of(&module, "area");
    assert_eq!(
        lines,
        vec![LET_LINE, RETURN_LINE, RETURN_LINE, RETURN_LINE],
        "the multiply belongs to the `let` line and the rest to the `return`"
    );
}

#[test]
fn every_lowered_instruction_names_the_file_it_came_from() {
    let (module, _) = lower(SOURCE, "Area");
    let area = module
        .functions
        .iter()
        .find(|function| function.name.as_str() == "area")
        .expect("area is lowered");
    let files: Vec<&str> = area
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| instruction.debug_location.as_deref())
        .map(|location| location.file.as_str())
        .collect();
    assert!(!files.is_empty(), "the frontend wrote no locations at all");
    assert!(
        files.iter().all(|file| file.ends_with("area.kira")),
        "{files:?}"
    );
}

#[test]
fn the_canonical_passes_keep_the_locations_the_frontend_wrote() {
    let target = "x86_64-unknown-linux-gnu"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut session = Session::new(target.clone());
    let unit = DefinitionUnit::new(session.add_file("area.kira", SOURCE));
    let parsed = parse_unit(&mut session, &unit).expect("the source parses");
    let elaborated = elaborate_file(&mut session, &parsed.file).expect("elaborates");
    let raw = lower_elaborated(&mut session, "Area", target, &elaborated).expect("lowers");
    let before = located(&raw);
    assert!(before > 0, "the frontend wrote no locations at all");
    let (canonical, diagnostics) = klf_kir_passes::canonicalize(&raw);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(
        located(&canonical) * 2 >= before,
        "canonicalization kept {} of {before}",
        located(&canonical)
    );
}

#[test]
fn the_backend_turns_them_into_a_line_table() {
    let (module, _) = lower(SOURCE, "Area");
    let ir = klf_llvm::emit_module(&module).expect("the module emits");
    assert!(
        ir.contains("!DIFile(filename: \"area.kira\""),
        "the source file is not in the debug info: {ir}"
    );
    assert!(
        ir.contains("!DISubprogram(name: \"area\""),
        "area has no subprogram: {ir}"
    );
    let locations = ir.matches("!DILocation(line: ").count();
    assert!(locations >= 8, "only {locations} line entries: {ir}");
    assert!(
        ir.contains("!DILocation(line: 8"),
        "`main` is not on its own source line: {ir}"
    );
}

#[test]
fn the_program_still_runs() {
    let (module, _) = lower(SOURCE, "Area");
    assert_eq!(
        klf_vm::run_module(&module, "main", &[]).expect("module runs"),
        klf_vm::Outcome::Return(Some(21))
    );
}

/// Where the native binary of `SOURCE` is written, along with the source file
/// its line table names, so stock LLDB can be pointed at it.
fn native_program(module: klf_kir::Module) -> Option<(PathBuf, PathBuf)> {
    if !clang().is_file() {
        return None;
    }
    let directory = std::env::temp_dir().join("klf-line-table-lldb");
    std::fs::create_dir_all(&directory).expect("temp directory");
    let source = directory.join("area.kira");
    std::fs::write(&source, SOURCE).expect("source written");
    let mut module = module;
    module.target = klf_target::host_triple().parse().expect("host triple");
    "c".clone_into(&mut module.language);
    let ir = klf_llvm::emit_program(&module, "main", &[]).expect("the program emits");
    let ll = directory.join("area.ll");
    std::fs::write(&ll, ir).expect("ir written");
    let binary = directory.join("area.out");
    let status = Command::new(clang())
        .args(["-g", "-Wno-override-module"])
        .arg(&ll)
        .arg(native_runtime())
        .arg("-lm")
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang rejected the program");
    Some((binary, source))
}

fn clang() -> PathBuf {
    klf_target::llvm_tool("clang").unwrap_or_else(|| PathBuf::from("clang"))
}

fn native_runtime() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "klf-native-rt"])
        .current_dir(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .expect("workspace root"),
        )
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the runtime does not build");
    // The build lands where `CARGO_TARGET_DIR` says, the workspace's own
    // `target` when it says nothing.
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            PathBuf::from,
        )
        .join("debug/libklf_native_rt.a")
        .canonicalize()
        .expect("the runtime archive")
}

/// How long one `lldb` session may run before the test gives up on it.
const LLDB_DEADLINE: Duration = Duration::from_secs(60);

/// Runs `lldb` over `commands` against `binary` and returns its transcript.
/// A session that does not return by [`LLDB_DEADLINE`] is killed and the test
/// fails with what it printed, so a stuck debugger cannot hang the suite.
fn run_lldb(lldb: &Path, commands: &Path, binary: &Path) -> String {
    let transcript = commands.with_extension("out");
    let output = std::fs::File::create(&transcript).expect("transcript created");
    let errors = output.try_clone().expect("transcript handle");
    let mut child = Command::new(lldb)
        .arg("-b")
        .arg("-s")
        .arg(commands)
        .arg(binary)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors)
        .spawn()
        .expect("lldb starts");
    let started = Instant::now();
    while child.try_wait().expect("lldb status").is_none() {
        if started.elapsed() >= LLDB_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            let printed = std::fs::read_to_string(&transcript).unwrap_or_default();
            panic!(
                "lldb did not return within {}s; transcript:\n{printed}\n\
                 (on macOS, `DevToolsSecurity -status` shows whether Developer Mode is \
                 enabled; lldb hangs at `run` while it is disabled)",
                LLDB_DEADLINE.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::read_to_string(&transcript).expect("transcript read")
}

fn lldb() -> Option<PathBuf> {
    which("lldb")
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|directory| Path::new(directory).join(program))
        .find(|candidate| candidate.is_file())
}

#[test]
fn stock_lldb_stops_on_a_kira_line_and_prints_the_locals() {
    let Some(lldb) = lldb() else {
        eprintln!("skipping: no lldb on PATH");
        return;
    };
    let (module, _) = lower(SOURCE, "Area");
    let Some((binary, source)) = native_program(module) else {
        eprintln!("skipping: no provisioned clang");
        return;
    };
    let name = source
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    let script = format!("breakpoint set --file {name} --line 4\nrun\nframe variable\n");
    let commands = std::env::temp_dir().join("klf-line-table-lldb.commands");
    std::fs::write(&commands, &script).expect("commands written");
    let session = run_lldb(&lldb, &commands, &binary);
    assert!(
        session.contains("kir.area") && session.contains("area.kira:4"),
        "the breakpoint did not resolve onto the Kira source: {session}"
    );
    assert!(
        session.contains("w = 4") && session.contains("h = 5"),
        "lldb printed no locals: {session}"
    );
}
