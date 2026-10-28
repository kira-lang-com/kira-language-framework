//! M6 conformance: a `.klffi` description of a C library is imported, its
//! `@FFI.Extern` functions become KIR foreign declarations, and a KIR program
//! that calls one through `call.foreign` compiles to native code, links against
//! libc, and returns the C function's real result.

use klf_driver::{Backend, run, toolchain_available};
use klf_klffi::{foreign_declarations, parse};
use klf_vm::Outcome;

const LIBC: &str = r"@FFI.Extern { library: c, symbol: abs, abi: c }
function abs(x: I32) -> I32

@FFI.Extern { library: c, symbol: strlen, abi: c }
function strlen(s: CString) -> USize
";

fn foreign_kir(source: &str) -> String {
    use std::fmt::Write;
    let module = parse(source).expect(".klffi parses");
    let mut out = String::new();
    let rules = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target")
        .c_layout()
        .expect("a 1.0 target has C rules");
    for function in foreign_declarations(&module, &rules) {
        let params: Vec<String> = function
            .parameters
            .iter()
            .map(|p| format!("%{}: {}", p.name, p.ty))
            .collect();
        let _ = writeln!(
            out,
            "func private foreign @{}({}) -> {} engine(native)",
            function.name,
            params.join(", "),
            function.result
        );
    }
    out
}

#[test]
fn klf_program_calls_libc_through_klffi() {
    let decls = foreign_kir(LIBC);
    let source = format!(
        "module @FfiDemo lang \"c\" target \"aarch64-apple-darwin\" stage canonical\n\n{decls}\n\
         func module @main() -> i64 engine(native) {{\n\
         entry():\n\
         \x20\x20%n = const.int i32, -7\n\
         \x20\x20%r = call.foreign @abs(%n)\n\
         \x20\x20%w = sext %r to i64\n\
         \x20\x20return %w\n\
         }}\n"
    );

    let module = klf_kir::parse_module(&source).expect("assembled module parses");
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(
        diagnostics.is_empty(),
        "the foreign call verifies: {diagnostics:?}"
    );

    if !toolchain_available() {
        eprintln!("skipping native run: no provisioned clang");
        return;
    }
    let outcome = run(&module, "main", &[], Backend::Native).expect("native run");
    assert_eq!(
        outcome,
        Outcome::Return(Some(7)),
        "abs(-7) == 7 through libc"
    );
}

const BY_VALUE: &str = r"@FFI.Struct { layout: c }
struct Point {
    let x: F64
    let y: F64
}

@FFI.Struct { layout: c }
struct Mixed {
    let tag: U8
    let value: I64
    let small: I16
}

@FFI.Extern { library: points, symbol: point_sum, abi: c }
function pointSum(p: Point, scale: I32) -> I64

@FFI.Extern { library: points, symbol: point_make, abi: c }
function pointMake(x: F64, y: F64) -> Point

@FFI.Extern { library: points, symbol: mixed_sum, abi: c }
function mixedSum(m: Mixed) -> I64
";

const POINTS_C: &str = "typedef struct { double x, y; } Point;
typedef struct { unsigned char tag; long long value; short small; } Mixed;
long long point_sum(Point p, int scale) { return (long long)((p.x + p.y) * scale); }
Point point_make(double x, double y) { Point p = { x, y }; return p; }
long long mixed_sum(Mixed m) { return m.tag + m.value + m.small; }
long long apply(long long (*f)(Point, int), Point p) { return f(p, 3); }
";

/// A function C calls back with a record by value: C enters it through its
/// shim, so the pointer the program hands C must be the shim's. It hands the
/// record on to C by value, so C reads what C passed.
const CALLBACK: &str = "func private foreign @apply(%f: fn(ptr, i32) -> i64, %p: ptr) -> i64 engine(native) c(ptr, %Point -> s64)

func private @scaled(%p: ptr, %k: i32) -> i64 engine(native) c(%Point, s32 -> s64) {
bb0(%p: ptr, %k: i32):
  %r = call.foreign @point_sum(%p, %k)
  return %r
}
";

const BY_VALUE_MAIN: &str = "func module @main() -> i64 engine(native) {
bb0():
  %p = stack.alloc %Point
  %px = field.addr %p, 0
  %x = const.float f64, 1.5
  store [init] %x to %px
  %py = field.addr %p, 1
  %y = const.float f64, 2.25
  store [init] %y to %py
  %four = const.int i32, 4
  %pp = addr.toptr %p
  %a = call.foreign @point_sum(%pp, %four)
  %q = stack.alloc %Point
  %qx = const.float f64, 10.0
  %qy = const.float f64, 20.5
  %qp = addr.toptr %q
  call.foreign @point_make(%qp, %qx, %qy)
  %qyaddr = field.addr %q, 1
  %made = load [trivial] %qyaddr
  %b = fptosi %made to i64
  %m = stack.alloc %Mixed
  %mtag = field.addr %m, 0
  %tag = const.int i8, 3
  store [init] %tag to %mtag
  %mvalue = field.addr %m, 1
  %value = const.int i64, 100
  store [init] %value to %mvalue
  %msmall = field.addr %m, 2
  %small = const.int i16, -7
  store [init] %small to %msmall
  %mp = addr.toptr %m
  %c = call.foreign @mixed_sum(%mp)
  %f = func.ref @scaled
  %d = call.foreign @apply(%f, %pp)
  %ab = add [wrap] %a, %b
  %abc = add [wrap] %ab, %c
  %r = add [wrap] %abc, %d
  return %r
}
";

/// Records cross a foreign call by value (section 26.2): their declarations
/// carry a C prototype, the call goes through the C shim the kernel writes
/// on both native backends, and the C library sees the values the program
/// stored — a homogeneous float aggregate passed and returned in registers,
/// and a record too large for registers passed in memory — and C calls back
/// into the program with a record by value through the shim it enters by.
#[test]
fn records_cross_a_foreign_call_by_value_through_the_c_shim() {
    let host = klf_driver::host_triple();
    let target: klf_target::Target = host.parse().expect("the host is a valid target");
    let rules = target.c_layout().expect("the host has C rules");
    let ffi = parse(BY_VALUE).expect(".klffi parses");
    let mut module = klf_kir::parse_module(&format!(
        "module @ByValue lang \"c\" target \"{host}\" stage canonical\n"
    ))
    .expect("the header parses");
    module.types = klf_klffi::type_declarations(&ffi, &rules).expect("the records lay out");
    module.functions = foreign_declarations(&ffi, &rules);
    let make = module
        .functions
        .iter()
        .find(|function| function.name.as_str() == "point_make")
        .expect("point_make is declared");
    assert_eq!(
        make.result,
        klf_kir::Type::Unit,
        "a record result is written through an address"
    );
    let module = klf_kir::parse_module(&format!("{module}\n{CALLBACK}\n{BY_VALUE_MAIN}"))
        .expect("the program parses");
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{module}");

    let Some(clang) = klf_driver::find_clang() else {
        eprintln!("skipping native run: no provisioned clang");
        return;
    };
    let prefix = if host.contains("apple") { "_" } else { "" };
    let shim = klf_kir::c_shim_source(&module, host, prefix, &|name| format!("kir.{name}"))
        .expect("the shims are spelled")
        .expect("records cross by value");
    let directory = std::env::temp_dir().join(format!("klf-by-value-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp directory");
    let object = klf_cranelift::emit_object(&module, "main", &[]).expect("the program compiles");
    std::fs::write(directory.join("program.o"), object).expect("object written");
    let ir = klf_llvm::emit_program(&module, "main", &[]).expect("the program emits");
    std::fs::write(directory.join("program.ll"), ir).expect("IR written");
    std::fs::write(directory.join("shim.c"), &shim).expect("shim written");
    std::fs::write(directory.join("points.c"), POINTS_C).expect("library written");
    let runtime = klf_driver::kernel_archive(klf_driver::KernelArchive::Runtime, host)
        .expect("the runtime archive");
    // Cranelift's object and the LLVM path's IR, each with the same shims.
    for program in ["program.o", "program.ll"] {
        let binary = directory.join(format!("{program}.out"));
        let mut link = std::process::Command::new(&clang);
        link.current_dir(&directory)
            .args([
                "-fuse-ld=lld",
                "-O1",
                "-Wno-override-module",
                program,
                "shim.c",
                "points.c",
            ])
            .arg(&runtime)
            .args(["-lm", "-o"])
            .arg(&binary);
        if host.contains("apple") {
            let sdk = std::process::Command::new("xcrun")
                .arg("--show-sdk-path")
                .output()
                .expect("xcrun runs");
            link.arg("-isysroot")
                .arg(String::from_utf8_lossy(&sdk.stdout).trim());
        }
        let linked = link.output().expect("clang runs");
        assert!(
            linked.status.success(),
            "{program}: {}",
            String::from_utf8_lossy(&linked.stderr)
        );
        let run = std::process::Command::new(&binary)
            .output()
            .expect("the program runs");
        let stdout = String::from_utf8_lossy(&run.stdout);
        // (1.5 + 2.25) * 4 + 20 + (3 + 100 - 7) + (1.5 + 2.25) * 3
        assert_eq!(
            stdout.lines().last(),
            Some("RET 142"),
            "{program}: {stdout}\n{shim}"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}
