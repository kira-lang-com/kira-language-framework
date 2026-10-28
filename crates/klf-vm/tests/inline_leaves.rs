use klf_kir::parse_module;
use klf_vm::{Instr, Opcode, Outcome, lower_module, run};
use std::fmt::Write as _;

const MODULE: &str = r#"module @Inline lang "test" target "aarch64-apple-darwin" stage raw
func private @subtract(%x: i64, %y: i64) -> i64 engine(runtime) {
bb0(%x: i64, %y: i64):
  %v = sub %x, %y
  return %v
}
func private @main(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  br bb1(%n)
bb1(%i: i64):
  %done = icmp eq %i, %zero
  cond.br %done, bb2, bb3
bb2:
  return %i
bb3:
  %next = call @subtract(%i, %one)
  br bb1(%next)
}
func private @divide(%x: i64, %y: i64) -> i64 engine(runtime) {
bb0(%x: i64, %y: i64):
  %v = sdiv %x, %y
  return %v
}
func private @trap(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %zero = const.int i64, 0
  %v = call @divide(%x, %zero)
  return %v
}
func private foreign @klfsvc_io_put(%c: i32) -> i32 engine(native)
func private @emit(%c: i32) -> i32 engine(runtime) {
bb0(%c: i32):
  br bb1
bb1:
  %result = call.foreign @klfsvc_io_put(%c)
  return %result
}
func private @emit_twice() -> i32 engine(runtime) {
bb0:
  %h = const.int i32, 72
  %i = const.int i32, 73
  %first = call @emit(%h)
  %second = call @emit(%i)
  return %second
}
"#;

#[test]
fn inlined_leaves_remap_registers_branches_and_traps() {
    let program = lower_module(&parse_module(MODULE).unwrap()).unwrap();
    let index = program.function_index("main").unwrap();
    assert!(
        program.functions[index]
            .code
            .iter()
            .all(|word| Instr::decode(*word).unwrap().op != Opcode::Call)
    );
    assert_eq!(run(&program, index, &[100]), Outcome::Return(Some(0)));
    let index = program.function_index("trap").unwrap();
    assert_eq!(
        run(&program, index, &[10]),
        Outcome::Trap("KLF-RT-0010".to_owned())
    );
}

#[test]
fn foreign_wrapper_inlining_keeps_capabilities_and_distinct_call_sites() {
    let program = lower_module(&parse_module(MODULE).unwrap()).unwrap();
    let index = program.function_index("emit_twice").unwrap();
    let (capabilities, sink) = klf_vm::kernel_services();
    assert!(matches!(
        klf_vm::run_with(&program, index, &[], &capabilities),
        Outcome::Return(_)
    ));
    assert_eq!(*sink.lock().unwrap(), b"HI");
    for word in &program.functions[index].code {
        let instruction = Instr::decode(*word).unwrap();
        if instruction.op == Opcode::Call {
            assert!(
                program.functions[index].call_sites[usize::try_from(instruction.bx()).unwrap()]
                    .foreign
                    .is_some()
            );
        }
    }
}

#[test]
fn small_call_wrappers_preserve_direct_and_indirect_targets() {
    for call in [
        "call @add(%x, %one)",
        "call.indirect %fn(%x, %one) : (i64, i64) -> i64",
    ] {
        let source = format!(
            r#"module @Wrappers lang "test" target "aarch64-apple-darwin" stage raw
func private @add(%x: i64, %y: i64) -> i64 engine(runtime) noinline {{
bb0(%x: i64, %y: i64):
  %v = add %x, %y
  return %v
}}
func private @wrapper(%x: i64) -> i64 engine(runtime) {{
bb0(%x: i64):
  %one = const.int i64, 1
  %fn = func.ref @add
  %v = {call}
  return %v
}}
func private @main(%x: i64) -> i64 engine(runtime) {{
bb0(%x: i64):
  %first = call @wrapper(%x)
  %second = call @wrapper(%first)
  return %second
}}
"#
        );
        let program = lower_module(&parse_module(&source).unwrap()).unwrap();
        let index = program.function_index("main").unwrap();
        let wrapper = program.function_index("wrapper").unwrap();
        assert!(program.functions[index].call_sites.iter().enumerate().all(
            |(site_index, site)| site.callee != wrapper
                || program.functions[index].code.iter().all(|word| {
                    let instruction = Instr::decode(*word).unwrap();
                    instruction.op != Opcode::Call
                        || usize::try_from(instruction.bx()).ok() != Some(site_index)
                })
        ));
        assert_eq!(run(&program, index, &[40]), Outcome::Return(Some(42)));
    }
}

#[test]
fn straight_line_scalar_helpers_inline_beyond_small_wrappers() {
    let mut body = String::new();
    let mut previous = "%x".to_owned();
    for index in 0..40 {
        let next = format!("%v{index}");
        writeln!(body, "  {next} = add {previous}, %one").unwrap();
        previous = next;
    }
    let source = format!(
        r#"module @LongLeaf lang "test" target "aarch64-apple-darwin" stage raw
func private @helper(%x: i64) -> i64 engine(runtime) {{
bb0(%x: i64):
  %one = const.int i64, 1
{body}  return {previous}
}}
func private @main(%x: i64) -> i64 engine(runtime) {{
bb0(%x: i64):
  %result = call @helper(%x)
  return %result
}}
"#
    );
    let program = lower_module(&parse_module(&source).unwrap()).unwrap();
    let index = program.function_index("main").unwrap();
    assert!(
        program.functions[index]
            .code
            .iter()
            .all(|word| Instr::decode(*word).unwrap().op != Opcode::Call)
    );
    assert_eq!(run(&program, index, &[2]), Outcome::Return(Some(42)));
}

#[test]
fn forward_control_flow_helpers_preserve_returns_edge_moves_and_traps() {
    let source = r#"module @ControlLeaf lang "test" target "aarch64-apple-darwin" stage raw
func private @helper(%x: i64, %y: i64) -> i64 engine(runtime) {
bb0(%x: i64, %y: i64):
  %condition = icmp slt %x, %y
  cond.br %condition, bb1(%y, %x), bb2(%x, %y)
bb1(%a: i64, %b: i64):
  %result = sub %a, %b
  return %result
bb2(%c: i64, %d: i64):
  %zero = const.int i64, 0
  %is_zero = icmp eq %d, %zero
  cond.br %is_zero, bb3, bb4
bb3:
  trap "zero_divisor"
bb4:
  %quotient = sdiv %c, %d
  return %quotient
}
func private @main(%x: i64, %y: i64) -> i64 engine(runtime) {
bb0(%x: i64, %y: i64):
  %first = call @helper(%x, %y)
  %second = call @helper(%y, %x)
  %result = add %first, %second
  return %result
}
"#;
    let program = lower_module(&parse_module(source).unwrap()).unwrap();
    let entry = program.function_index("main").unwrap();
    assert!(
        program.functions[entry]
            .code
            .iter()
            .all(|word| Instr::decode(*word).unwrap().op != Opcode::Call)
    );
    assert_eq!(run(&program, entry, &[3, 9]), Outcome::Return(Some(9)));
    assert_eq!(run(&program, entry, &[9, 3]), Outcome::Return(Some(9)));
    assert_eq!(
        run(&program, entry, &[9, 0]),
        Outcome::Trap("zero_divisor".to_owned())
    );
}

#[test]
fn forward_helpers_preserve_raw_allocation_and_free() {
    let source = r#"module @AllocationLeaf lang "test" target "aarch64-apple-darwin" stage raw
func private @allocate(%size: i64, %alignment: i64) -> ptr engine(runtime) {
bb0(%size: i64, %alignment: i64):
  %pointer = heap.alloc.raw %size, %alignment
  return %pointer
}
func private @release(%pointer: ptr, %when: i64) -> unit engine(runtime) {
bb0(%pointer: ptr, %when: i64):
  %zero = const.int i64, 0
  %condition = icmp eq %when, %zero
  cond.br %condition, bb1, bb2
bb1:
  heap.free.raw %pointer
  return
bb2:
  return
}
func private @main() -> i64 engine(runtime) {
bb0:
  %size = const.int i64, 16
  %alignment = const.int i64, 16
  %pointer = call @allocate(%size, %alignment)
  %slot = bitcast %pointer to *i64
  %value = const.int i64, 73
  store [init] %value to %slot
  %keep = const.int i64, 1
  call @release(%pointer, %keep)
  %loaded = load [trivial] %slot
  %free = const.int i64, 0
  call @release(%pointer, %free)
  return %loaded
}
"#;
    let program = lower_module(&parse_module(source).unwrap()).unwrap();
    let entry = program.function_index("main").unwrap();
    assert!(
        program.functions[entry]
            .code
            .iter()
            .all(|word| Instr::decode(*word).unwrap().op != Opcode::Call)
    );
    assert_eq!(run(&program, entry, &[]), Outcome::Return(Some(73)));
}
