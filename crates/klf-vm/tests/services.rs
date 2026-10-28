//! Kernel service bridge (section 8.12 / 15.7): a running KIR program calls a
//! real kernel service (`klf.io.put`) by name, and the service executes — the
//! transport a definition uses to reach kernel services once it is written in
//! KLF Kira and run, instead of being hosted in Rust.

use klf_vm::{
    Outcome, RuntimeDiagnostic, kernel_services, kernel_services_with_source,
    kernel_services_with_source_and_diagnostics, run_module_with_capabilities,
};

const IO: &str = r#"module @IoDemo lang "c" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_io_put(%c: i32) -> i32 engine(native)

func module @main() -> i64 engine(native) {
entry():
  %h = const.int i32, 72
  %r1 = call.foreign @klfsvc_io_put(%h)
  %i = const.int i32, 73
  %r2 = call.foreign @klfsvc_io_put(%i)
  %bang = const.int i32, 33
  %r3 = call.foreign @klfsvc_io_put(%bang)
  %z = const.int i64, 0
  return %z
}
"#;

#[test]
fn running_kir_calls_a_kernel_service() {
    let module = klf_kir::parse_module(IO).expect("module parses");
    let (capabilities, sink) = kernel_services();
    let outcome = run_module_with_capabilities(&module, "main", &[], &capabilities)
        .expect("module runs on the VM");
    assert_eq!(outcome, Outcome::Return(Some(0)));
    let written = sink.lock().expect("sink lock").clone();
    assert_eq!(
        written, b"HI!",
        "the kernel service wrote the program's output"
    );
}

#[test]
fn kernel_capabilities_cover_manifest_symbols() {
    let (capabilities, _) = kernel_services();
    for line in include_str!("../../../kernel/services.manifest").lines() {
        let content = line
            .split_once('#')
            .map_or(line, |(content, _)| content)
            .trim();
        let mut fields = content.split_whitespace();
        let Some(_) = fields.next() else {
            continue;
        };
        let Some(symbol) = fields.next() else {
            continue;
        };
        assert!(
            capabilities.contains(symbol),
            "manifest service is not registered: {symbol}"
        );
    }
}

#[test]
fn a_service_call_with_no_capability_is_not_resolved() {
    // Without the capability registered, the name falls through to the dynamic
    // path and finds no such symbol, so the program traps rather than silently
    // succeeding.
    let module = klf_kir::parse_module(IO).expect("module parses");
    let empty = klf_vm::Capabilities::new();
    let outcome = run_module_with_capabilities(&module, "main", &[], &empty).expect("module runs");
    assert!(
        matches!(outcome, Outcome::Trap(_)),
        "an unresolved service call traps: {outcome:?}"
    );
}

const SOURCE: &str = r#"module @SourceDemo lang "klfkira" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_source_len() -> i64 engine(native)
func private foreign @klfsvc_source_byte(%index: i64) -> i64 engine(native)
func private foreign @klfsvc_diag_error(%code: i64, %start: i64, %end: i64) -> unit engine(native)

func module @main() -> i64 engine(native) {
entry():
  %length = call.foreign @klfsvc_source_len()
  %code = const.int i64, 2001
  %start = const.int i64, 0
  call.foreign @klfsvc_diag_error(%code, %start, %length)
  %index = const.int i64, 1
  %byte = call.foreign @klfsvc_source_byte(%index)
  %sum = add [trap] s %length, %byte
  return %sum
}
"#;

#[test]
fn running_kir_reads_the_definition_source_buffer() {
    let module = klf_kir::parse_module(SOURCE).expect("module parses");
    let (capabilities, sink) = kernel_services_with_source("aZ");
    let outcome = run_module_with_capabilities(&module, "main", &[], &capabilities)
        .expect("module runs on the VM");
    assert_eq!(
        outcome,
        Outcome::Return(Some(92)),
        "2 + 'Z' reaches the definition"
    );
    assert_eq!(
        sink.lock().expect("sink lock").as_slice(),
        b"DIAG 2001 0 2 -1 0\n"
    );
}

#[test]
fn runtime_diagnostics_are_retained_as_typed_records() {
    let module = klf_kir::parse_module(SOURCE).expect("module parses");
    let (capabilities, _, diagnostics) = kernel_services_with_source_and_diagnostics("aZ");
    run_module_with_capabilities(&module, "main", &[], &capabilities).expect("module runs");
    assert_eq!(
        diagnostics.lock().expect("diagnostics").as_slice(),
        &[RuntimeDiagnostic::new(2001, 0, 2)]
    );
}

const HOST: &str = r#"module @HostDemo lang "c" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_host_key_length() -> i64 engine(native)
func private foreign @klfsvc_host_key_copy(%destination: ptr, %capacity: i64) -> i64 engine(native)

func module @main() -> i64 engine(native) {
entry():
  %length = call.foreign @klfsvc_host_key_length()
  %buffer = stack.alloc [32 x i8]
  %capacity = const.int i64, 32
  %copied = call.foreign @klfsvc_host_key_copy(%buffer, %capacity)
  %difference = sub [trap] s %copied, %length
  return %difference
}
"#;

#[test]
fn a_program_can_name_the_host_it_runs_on() {
    // The length/copy pair a program uses to learn its own host key: `copy` is
    // asked how many bytes the key is as well as for the bytes themselves, and a
    // well-behaved copy agrees with `length` when the buffer fits. The key's
    // spelling is the framework's own, so the expected length is that of
    // `klf_target::llvm_host_key` — the point of the service is that a program
    // and the framework agree on one name for a machine.
    let module = klf_kir::parse_module(HOST).expect("module parses");
    let (capabilities, _) = kernel_services();
    let outcome = run_module_with_capabilities(&module, "main", &[], &capabilities)
        .expect("module runs on the VM");
    assert_eq!(
        outcome,
        Outcome::Return(Some(0)),
        "the host key's length and copy agree"
    );
    assert!(
        !klf_target::llvm_host_key().is_empty(),
        "the framework names this host, so the pair has something to answer"
    );
}
