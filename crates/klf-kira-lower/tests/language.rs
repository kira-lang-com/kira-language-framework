//! End-to-end: KLF Kira language features lowered to KIR and run on the VM.

mod support;

use klf_kir::parse_module;
use klf_kira_lower::{KiraDefinition, lower_file};
use klf_kira_syntax::parse;
use klf_services::{Definition, DefinitionPackage, Session};
use klf_vm::Outcome;
use support::run;

#[test]
fn calls_and_bindings() {
    let source = "
function add(a: Int, b: Int) -> Int { return a + b }
function main() -> Int {
    let x = 40
    var y = 1
    y = y + 1
    return add(x, y)
}";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(42)));
}

#[test]
fn ternary_max() {
    let source = "function max(a: Int, b: Int) -> Int { return a > b ? a : b }";
    assert_eq!(run(source, "max", &[3, 7]), Outcome::Return(Some(7)));
    assert_eq!(run(source, "max", &[9, 2]), Outcome::Return(Some(9)));
}

#[test]
fn overflow_traps() {
    // Int addition traps on overflow (KLF Kira: `+` traps, `&+` wraps).
    let source = "
function main() -> Int {
    var x = 1
    return add_max(x)
}
function add_max(x: Int) -> Int {
    let big = 9223372036854775807
    return big + x
}";
    assert!(matches!(run(source, "main", &[]), Outcome::Trap(_)));
}

#[test]
fn wrapping_does_not_trap() {
    let source = "
function main() -> Int {
    let big = 9223372036854775807
    return big &+ 1
}";
    // i64::MAX &+ 1 wraps to i64::MIN.
    assert_eq!(
        run(source, "main", &[]),
        Outcome::Return(Some(i64::MIN.cast_unsigned()))
    );
}

#[test]
fn klf_kira_calls_a_kernel_service() {
    use klf_vm::{kernel_services, run_module_with_capabilities};
    let source = "
function main() -> Int {
    let a: Int = putc(72)
    let b: Int = putc(73)
    let c: Int = putc(33)
    return a
}";
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let file_id = session.add_file("t.klfd", source);
    let parsed = parse(session.file(file_id));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse: {:?}",
        parsed.diagnostics
    );
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let module = lower_file(&mut session, "Svc", target, &parsed.file)
        .unwrap_or_else(|| panic!("lowering: {:?}", session.diagnostics()));

    let (capabilities, sink) = kernel_services();
    let outcome =
        run_module_with_capabilities(&module, "main", &[], &capabilities).expect("runs on the VM");
    assert_eq!(outcome, Outcome::Return(Some(72)), "putc returns the byte");
    assert_eq!(
        sink.lock().expect("sink").clone(),
        b"HI!",
        "the service wrote the output"
    );
}

#[test]
fn structs_field_read_write() {
    let source = "
struct Point { let x: Int  let y: Int }
function main() -> Int {
    var p = Point { x = 30, y = 12 }
    p.x = p.x + 5
    return p.x + p.y
}";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(47)));
}

#[test]
fn enums_literals_and_match() {
    let source = "
enum Value {
    Empty
    Number(Int)
}
function value(v: Value) -> Int {
    match v {
        Empty -> return 0
        Number(n) -> return n
    }
}
function main() -> Int { return value(.Number(42)) }
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(42)));
}

#[test]
fn match_on_a_string() {
    let source = "
function weight(name: String) -> Int {
    match name {
        \"one\" -> return 1
        \"two\", \"deux\" -> {
            if name == \"two\" { return 2 }
            return 20
        }
        else -> return 0
    }
}
function main() -> Int {
    return weight(\"one\") + weight(\"two\") * 10 + weight(\"deux\") * 100 + weight(\"three\") * 1000
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(2021)));
}

/// The messages lowering `source` reports; lowering must fail.
fn lowering_errors(source: &str) -> Vec<String> {
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let file_id = session.add_file("t.klfd", source);
    let parsed = parse(session.file(file_id));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse: {:?}",
        parsed.diagnostics
    );
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    assert!(lower_file(&mut session, "Errors", target, &parsed.file).is_none());
    session
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

#[test]
fn a_string_match_needs_one_else_and_distinct_texts() {
    let missing = "
function weight(name: String) -> Int {
    match name {
        \"one\" -> return 1
    }
}";
    assert!(
        lowering_errors(missing)
            .iter()
            .any(|message| message.contains("needs an `else` arm"))
    );
    let repeated = "
function weight(name: String) -> Int {
    match name {
        \"one\" -> return 1
        \"two\", \"one\" -> return 2
        else -> return 0
        else -> return 3
    }
}";
    let messages = lowering_errors(repeated);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("duplicate match text \"one\""))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("one `else` arm"))
    );
    let cased = "
enum Color { Red Green }
function weight(color: Color) -> Int {
    match color {
        \"Red\" -> return 1
        Green -> return 2
    }
}";
    assert!(
        lowering_errors(cased)
            .iter()
            .any(|message| message.contains("names its cases, not texts"))
    );
}

#[test]
fn array_literals_index_and_assignment() {
    let source = "
function main() -> Int {
    var values = [10, 20, 12]
    values[1] = values[1] + 1
    return values[0] + values[1] + values[2]
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(43)));
}

#[test]
fn string_literals_length_and_index() {
    let source = "
function length() -> Int {
    let text = \"hello\"
    return text.len
}
function byte() -> U8 {
    let text = \"abc\"
    return text[1]
}
function main() -> Int { return length() }
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(5)));
    assert_eq!(run(source, "byte", &[]), Outcome::Return(Some(98)));
}

#[test]
fn generic_identity_is_inferred_and_specialized() {
    let source = "
function identity<T>(value: T) -> T { return value }
function main() -> Int { return identity(42) }
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(42)));
}

#[test]
fn conformances_emit_reparseable_kir_witness_declarations() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::single(
        "main.klfd",
        "trait Equatable {}\nextend Int: Equatable {}\nfunction main() -> Int { return 42 }",
    );
    let mut session = Session::new(target.clone());
    let module = KiraDefinition
        .compile_package(&mut session, &package, "Test", target)
        .expect("package lowering");
    let witness = module.witnesses.first().expect("witness declaration");
    assert_eq!(witness.ty, klf_kir::Type::Integer(64));
    assert_eq!(witness.trait_name.as_str(), "Equatable");
    assert_eq!(witness.witnesses, Vec::<klf_core::Name>::new());
    let reparsed = parse_module(&module.to_string()).expect("witness KIR reparses");
    assert_eq!(reparsed.witnesses, module.witnesses);
}

#[test]
fn f64_arithmetic_lowers_and_runs() {
    let source = r"
function main() -> Int {
    let x: F64 = 3.5
    let y = x * 2.0
    let z = -y + 9.0
    return z as Int
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(2)));
}

#[test]
fn f64_comparison_selects_a_branch() {
    let source = r"
function main() -> Int {
    let a = 2.5
    let b = 4.0
    if a < b {
        return 1
    }
    return 0
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(1)));
}

#[test]
fn for_loop_over_fixed_array_sums_elements() {
    let source = r"
function main() -> Int {
    let xs = [10, 20, 30, 40]
    var total = 0
    for v in xs {
        total = total + v
    }
    return total
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(100)));
}

#[test]
fn for_loop_honors_break_and_continue() {
    let source = r"
function main() -> Int {
    let xs = [1, 2, 3, 4, 5, 6]
    var total = 0
    for v in xs {
        if v == 2 {
            continue
        }
        if v == 5 {
            break
        }
        total = total + v
    }
    return total
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(8)));
}

#[test]
fn for_loop_over_integer_range() {
    let source = r"
function main() -> Int {
    var total = 0
    for index in 2..7 {
        total = total + index
    }
    return total
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(20)));
}

#[test]
fn function_returns_struct_by_value() {
    let source = r"
struct Point {
    let x: Int
    let y: Int
}
function make(a: Int, b: Int) -> Point {
    return Point { x = a, y = b }
}
function main() -> Int {
    let p = make(30, 12)
    return p.x + p.y
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(42)));
}

#[test]
fn string_equality_compares_contents() {
    let source = r#"
function classify(word: String) -> Int {
    if word == "int" {
        return 1
    }
    if word == "return" {
        return 2
    }
    return 0
}
function main() -> Int {
    let a = classify("int")
    let b = classify("return")
    let c = classify("while")
    return a * 100 + b * 10 + c
}
"#;
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(120)));
}

#[test]
fn growable_list_append_and_index() {
    let source = r"
function main() -> Int {
    var xs: [Int] = []
    var i = 0
    while i < 10 {
        xs.append(i * i)
        i = i + 1
    }
    return xs[9] + xs.count
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(91)));
}

#[test]
fn list_of_structs_appends_and_reads() {
    let source = r"
struct Point {
    let x: Int
    let y: Int
}
function main() -> Int {
    var pts: [Point] = []
    pts.append(Point { x = 3, y = 4 })
    pts.append(Point { x = 10, y = 20 })
    var total = 0
    for p in pts {
        total = total + p.x + p.y
    }
    return total
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(37)));
}

#[test]
fn appending_to_a_list_copy_keeps_the_original() {
    let source = r"
struct Bag {
    var items: [Int]
}
function main() -> Int {
    var xs: [Int] = []
    xs.append(1)
    xs.append(2)
    var ys = xs
    ys.append(30)
    xs.append(40)
    var bag = Bag { items = xs }
    var other = bag
    other.items.append(500)
    bag.items.append(600)
    return ys[2] + xs[2] + other.items[3] + bag.items[3] + xs.count * 10000
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(31170)));
}

#[test]
fn writing_an_element_of_a_list_copy_keeps_the_original() {
    let source = r#"
struct P {
    var v: Int
    var names: [String]
}
struct Tree {
    var heads: [Int]
}
function mark(tree: borrow mut Tree) {
    tree.heads[1] = 7
}
function main() -> Int {
    var xs = [0, 0]
    var ys = xs
    ys[1] = 9
    var xss: [[Int]] = [[1, 2], [3, 4]]
    var yss = xss
    yss[1][0] = 30
    var ps: [P] = [P { v = 1, names = ["a"] }]
    var qs = ps
    qs[0].v = 9
    qs[0].names[0] = "zz"
    var tree = Tree { heads = [0, 0] }
    var twin = tree
    mark(twin)
    var total = xs[1] + ys[1] * 1000000 + xss[1][0] * 1000 + yss[1][0] * 10 + ps[0].v + qs[0].v * 100
    if ps[0].names[0] == "a" && qs[0].names[0] == "zz" { total = total + 100000 }
    return total + tree.heads[1] * 10000000 + twin.heads[1] * 100000000
}
"#;
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(709104201)));
}

#[test]
fn binding_an_enum_keeps_it_through_a_later_store() {
    let source = r"
enum Slot {
    Empty
    Filled(Int)
}
struct Holder {
    var slot: Slot
}
function read(slot: Slot) -> Int {
    match slot {
        Empty -> return 0
        Filled(n) -> return n
    }
}
function main() -> Int {
    var holder = Holder { slot = .Filled(40) }
    let saved = holder.slot
    holder.slot = .Filled(7)
    var current: Slot = .Filled(2)
    let before = current
    current = .Empty
    return read(saved) + read(before) + read(holder.slot) * 100 + read(current)
}
";
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(742)));
}

#[test]
fn text_builtins_count_and_byte() {
    let source = r#"
function main() -> Int {
    let s = "hello"
    return utf8Count(s) * 1000 + utf8Byte(s, 1)
}
"#;
    // len 5, byte[1]='e'=101
    assert_eq!(run(source, "main", &[]), Outcome::Return(Some(5101)));
}

#[test]
fn decimal_text_spells_the_minimum_int_inside_its_buffer() {
    let source = r#"
function main() -> Int {
    let lowest = 0 - 9223372036854775807 - 1
    let text = decimalText(lowest)
    let after = decimalText(42)
    return utf8Count(text) * 1000000 + utf8Byte(text, 0) * 1000 + utf8Byte(text, 19) * 10 + utf8Count(after)
}
"#;
    // "-9223372036854775808": 20 bytes, '-' = 45, '8' = 56; "42" is 2 bytes.
    assert_eq!(
        run(source, "main", &[]),
        Outcome::Return(Some(20 * 1_000_000 + 45 * 1000 + 56 * 10 + 2))
    );
}
