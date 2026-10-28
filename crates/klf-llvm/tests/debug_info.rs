//! The debug information of section 16.3: the metadata the backend writes must
//! assemble, must become DWARF 5 in the language of the hosted language, and
//! must carry the line table, the locals and the aggregate layouts a debugger
//! needs.

use klf_kir::{Module, parse_module};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const MODULE: &str = r#"module @Debug lang "kira" target "x86_64-unknown-linux-gnu" stage raw

type %Point = struct ridl { x: i64, y: i64 }
policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

type %Shape = enum ridl { Circle(%Point), Square(%Point), Dot }
policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func module @compute(%base: i64, %shape: *%Shape) -> i64 engine(runtime) {
bb0(%base: i64, %shape: *%Shape):
  %slot = stack.alloc i64
  %one = const.int i64, 1 !loc("src/point.kir":4:9)
  %next = add %base, %one !loc("src/point.kir":4:18)
  debug.value %next, !var("total", i64, "compute") !loc("src/point.kir":5:3)
  store [init] %next to %slot !loc("src/point.kir":6:3)
  %sum = load [trivial] %slot !loc("src/point.kir":7:10)
  return %sum !loc("src/point.kir":8:3)
}
"#;

fn tools() -> &'static PathBuf {
    static TOOLS: OnceLock<PathBuf> = OnceLock::new();
    TOOLS.get_or_init(|| {
        klf_target::llvm_root()
            .expect("a RIDL LLVM under ~/.klf/llvm-ridl")
            .join("bin")
    })
}

fn host_module() -> Module {
    let mut module = parse_module(MODULE).expect("module parses");
    module.target = klf_target::host_triple().parse().expect("host triple");
    module
}

/// Assembles `ir` with the provisioned clang and returns what `llvm-dwarfdump`
/// makes of one of its sections.
fn dwarfdump(ir: &str, section: &str) -> String {
    let dir = work_dir(ir, section);
    let ll = dir.join("program.ll");
    let obj = dir.join("program.o");
    std::fs::write(&ll, ir).expect("write ir");
    let status = Command::new(tools().join("clang"))
        .args(["-g", "-c", "-Wno-override-module"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang rejected --{section}");
    let output = Command::new(tools().join("llvm-dwarfdump"))
        .arg(format!("--{section}"))
        .arg(&obj)
        .output()
        .expect("llvm-dwarfdump runs");
    assert!(output.status.success(), "llvm-dwarfdump --{section} failed");
    String::from_utf8(output.stdout).expect("dwarfdump output is utf8")
}

/// A directory of its own for one piece of IR, so tests running at once do not
/// read each other's object files.
fn work_dir(ir: &str, section: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(ir, &mut hasher);
    let dir = std::env::temp_dir().join(format!(
        "klf-debug-{section}-{section}{:x}",
        std::hash::Hasher::finish(&hasher)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn emit() -> String {
    klf_llvm::emit_module(&host_module()).expect("module emits")
}

/// The `DW_AT_language` line of the object `ir` builds into, as
/// `llvm-dwarfdump` prints it: a name for the codes it knows, such as
/// `DW_LANG_C11`, and the raw code for the others, such as Kira's vendor code.
/// The tool is the provisioned LLVM's, as the clang is: a macOS host has no
/// `readelf`, and the object is ELF whatever the host makes.
fn object_language(ir: &str) -> String {
    let dir = work_dir(ir, "language");
    let ll = dir.join("program.ll");
    let obj = dir.join("program.o");
    std::fs::write(&ll, ir).expect("write ir");
    let status = Command::new(tools().join("clang"))
        .args(["-g", "-c", "-Wno-override-module", "--target=aarch64-unknown-linux-gnu"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang rejected the module");
    let output = Command::new(tools().join("llvm-dwarfdump"))
        .arg("--debug-info")
        .arg(&obj)
        .output()
        .expect("llvm-dwarfdump runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("DW_AT_language"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_assembler_accepts_the_metadata() {
    let ir = emit();
    let dir = work_dir(&ir, "verify");
    let ll = dir.join("program.ll");
    std::fs::write(&ll, &ir).expect("write ir");
    let status = Command::new(tools().join("llvm-as"))
        .arg(&ll)
        .arg("-o")
        .arg(dir.join("program.bc"))
        .status()
        .expect("llvm-as runs");
    assert!(status.success(), "llvm-as rejected the module");
}

#[test]
fn variables_are_named_with_records_and_not_with_the_removed_intrinsics() {
    let ir = emit();
    assert!(ir.contains("#dbg_declare("), "{ir}");
    assert!(ir.contains("#dbg_value("), "{ir}");
    assert!(
        !ir.contains("llvm.dbg.value") && !ir.contains("llvm.dbg.addr"),
        "the debug intrinsics were removed in LLVM 17: {ir}"
    );
    // A record stands where an instruction would, so its location is its last
    // operand rather than a trailing `!dbg`.
    for line in ir.lines().filter(|line| line.contains("#dbg_")) {
        assert!(
            line.trim_start().starts_with('#') && line.trim_end().ends_with(')'),
            "malformed debug record: {line}"
        );
    }
}

/// The source line a debugger's line-table reader resolves an address back to,
/// read through `llvm-addr2line`. That is not the reader LLDB uses — LLDB has
/// its own DWARF plugin — so the Kira end-to-end test in `klf-kira-lower` is
/// what proves a debugger stops on the line; this one only proves the table.
fn resolve_line(ir: &str, line: &str) -> String {
    let dir = work_dir(ir, "addr2line");
    let ll = dir.join("program.ll");
    let obj = dir.join("program.o");
    std::fs::write(&ll, ir).expect("write ir");
    let status = Command::new(tools().join("clang"))
        .args(["-g", "-c", "-Wno-override-module"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang rejected the module");
    let table = dwarfdump(ir, "debug-line");
    let address = table
        .lines()
        .filter(|row| row.starts_with("0x"))
        .find_map(|row| {
            let mut fields = row.split_whitespace();
            let address = fields.next()?;
            (fields.next()? == line).then(|| address.to_owned())
        })
        .unwrap_or_else(|| panic!("no row on line {line}: {table}"));
    let output = Command::new(tools().join("llvm-addr2line"))
        .args(["-f", "-e", obj.to_str().expect("path"), &address])
        .output()
        .expect("llvm-addr2line runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace('\n', " ")
}

#[test]
fn the_line_table_resolves_back_to_the_statement_that_emitted_it() {
    for line in ["4", "6", "7", "8"] {
        let resolved = resolve_line(&emit(), line);
        assert!(
            resolved.ends_with(&format!("src/point.kir:{line}")),
            "line {line} resolved to {resolved}"
        );
    }
}

#[test]
fn the_unit_is_dwarf_5_under_the_vendor_language_of_kira() {
    let dump = dwarfdump(&emit(), "debug-info");
    assert!(dump.contains("version = 0x0005"), "{dump}");
    assert!(dump.contains("DW_AT_producer\t(\"klf\")"), "{dump}");
    // `llvm-dwarfdump` prints `0x8e57` by the name LLVM gives that vendor
    // code, Google's RenderScript, and an older one prints the code itself.
    let language = object_language(&emit());
    assert!(
        language.contains("(0x8e57)") || language.contains("(DW_LANG_GOOGLE_RenderScript)"),
        "{language}"
    );
}

#[test]
fn karc_and_c_carry_their_own_language_codes() {
    for (language, code) in [("karc", "(DW_LANG_C_plus_plus"), ("c", "(DW_LANG_C11)")] {
        let mut module = host_module();
        module.language = language.to_owned();
        let found = object_language(&klf_llvm::emit_module(&module).expect("module emits"));
        assert!(found.contains(code), "`{language}` should be {code}: {found}");
    }
}

#[test]
fn the_line_table_places_each_statement_in_its_source() {
    let dump = dwarfdump(&emit(), "debug-line");
    assert!(dump.contains("point.kir"), "{dump}");
    // Line 5 is the `debug.value`, and a debug intrinsic produces no code and
    // so no row of its own.
    for line in [4, 6, 7, 8] {
        let found = dump
            .lines()
            .filter(|row| row.starts_with("0x"))
            .any(|row| row.split_whitespace().nth(1) == Some(line.to_string().as_str()));
        assert!(found, "line {line} is missing from the line table: {dump}");
    }
}

#[test]
fn parameters_and_named_values_are_described() {
    let dump = dwarfdump(&emit(), "debug-info");
    for name in ["compute", "base", "shape", "total"] {
        assert!(
            dump.contains(&format!("DW_AT_name\t(\"{name}\")")),
            "{name} is missing: {dump}"
        );
    }
    assert!(dump.contains("DW_TAG_variable"), "{dump}");
    assert!(dump.contains("DW_AT_location"), "{dump}");
}

#[test]
fn a_record_keeps_its_members_and_an_enum_its_cases() {
    let ir = emit();
    assert!(ir.contains("name: \"Point\""), "{ir}");
    assert!(ir.contains("name: \"Circle\""), "{ir}");
    assert!(ir.contains("DW_TAG_variant_part"), "{ir}");
    assert!(ir.contains("extraData: i32 1"), "{ir}");
    let dump = dwarfdump(&ir, "debug-info");
    assert!(dump.contains("DW_TAG_structure_type"), "{dump}");
    assert!(dump.contains("DW_TAG_variant_part"), "{dump}");
}

#[test]
fn a_virtual_file_carries_its_text_in_the_source_extension() {
    let mut sources = klf_llvm::DebugSources::default();
    sources.virtual_sources.insert(
        "src/point.kir".to_owned(),
        "let total = base + 1;\n".to_owned(),
    );
    let ir = klf_llvm::emit_module_with(&host_module(), &sources).expect("module emits");
    let dump = dwarfdump(&ir, "debug-line");
    assert!(dump.contains("source:"), "{dump}");
    assert!(dump.contains("let total = base + 1;"), "{dump}");
}

#[test]
fn a_windows_target_gets_codeview_rather_than_dwarf() {
    let mut module = host_module();
    module.target = "x86_64-pc-windows-msvc".parse().expect("windows triple");
    let ir = klf_llvm::emit_module(&module).expect("module emits");
    assert!(ir.contains("Debug Info Format"), "{ir}");
    assert!(!ir.contains("Dwarf Version"), "{ir}");
}
