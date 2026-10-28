//! Bindgen suite (section 25): generated Rust bindings are correct and compile.

use std::process::Command;

use klf_core::FileId;
use klf_ridl::bindgen::{caller_symbol, kira_bindings, rust_bindings};
use klf_ridl::parse;

const SRC: &str = r#"
struct Vec3 {
    let x: F32
    let y: F32
    let z: F32
}

struct Scene

struct Tag {
    let raw: U32
}

function area(w: I64, h: I64) -> I64
function scale(v: F64) -> F64
function parse(text: borrow String) -> Result<I64, I64>
function largest<T>(of: I64) -> I64

let Tag.read: Tag = 4
let Tag.all: Tag = 0xffffffff
let pageSize: U64 = 4096
let root: String = "/Sys\"tem"
"#;

fn module() -> klf_ridl::module::Module {
    parse(FileId::from_u32(0), "Geometry", SRC).unwrap()
}

#[test]
fn binds_functions_to_their_adapters() {
    let out = rust_bindings(&module());
    let symbol = caller_symbol("Geometry", "area");
    assert!(out.contains(&format!("fn {symbol}(w: i64, h: i64) -> i64;")));
    assert!(out.contains("pub fn area(w: i64, h: i64) -> i64 {"));
    assert!(out.contains(&format!("unsafe {{ {symbol}(w, h) }}")));
}

#[test]
fn generates_repr_c_structs_and_opaque_handles() {
    let out = rust_bindings(&module());
    assert!(out.contains("#[repr(C)]\n#[derive(Clone, Debug)]\npub struct Vec3 {"));
    assert!(out.contains("pub x: f32,"));
    assert!(out.contains("pub struct Scene(pub core::ptr::NonNull<core::ffi::c_void>);"));
    assert!(out.contains("#[link_name = \"_RIDL18Geometry5SceneD\"]"));
}

#[test]
fn returns_failing_results_through_the_result_slot() {
    let out = rust_bindings(&module());
    assert!(out.contains(
        "fn Geometry_parse(result: *mut RidlResult<i64, i64>, text: *const RidlString);"
    ));
    assert!(out.contains("pub fn parse(text: &RidlString) -> Result<i64, i64> {"));
}

#[test]
fn kira_bindings_declare_ridl_imports() {
    let out = kira_bindings(&module());
    assert!(out.contains("struct Vec3 {\n    var x: F32\n"));
    assert!(out.contains("@Ridl(\"Geometry\") function area(w: Int, h: Int) -> Int"));
    assert!(
        out.contains("@Ridl(\"Geometry\") function parse(text: borrow String) -> Result<Int, Int>")
    );
    // An opaque handle reads back as a class with no fields of its own.
    assert!(out.contains("class Scene {}\n"));
    // The struct Kira lowers a `distinct` to reads back as the distinct.
    assert!(out.contains("distinct Tag = U32\n"));
}

#[test]
fn kira_bindings_preserve_open_enums() {
    let module = parse(FileId::from_u32(0), "Keys", "@Open enum Key { A B }").unwrap();
    let output = kira_bindings(&module);
    assert!(output.contains("@Open\nenum Key {\n    A\n    B\n}"));
    assert!(!output.contains("skipped enum"));
}

#[test]
fn kira_bindings_import_constants_into_their_namespaces() {
    let out = kira_bindings(&module());
    assert!(out.contains(
        "namespace Tag {\n    let read: Tag = Tag(4)\n    let all: Tag = Tag(0xffffffff)\n}\n"
    ));
    assert!(out.contains("let pageSize: U64 = 4096\n"));
    assert!(out.contains("let root: String = \"/Sys\\\"tem\"\n"));
}

#[test]
fn skips_the_shapes_that_need_shims() {
    let out = rust_bindings(&module());
    assert!(out.contains("// skipped function `largest`: generic functions need descriptors"));
}

#[test]
fn generated_bindings_are_valid_rust() {
    let Ok(rustc) = which_rustc() else {
        eprintln!("rustc not found; skipping compile check");
        return;
    };
    let out = rust_bindings(&module());
    let dir = std::env::temp_dir().join(format!("klf-bindgen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("bindings.rs");
    std::fs::write(&src, &out).unwrap();

    let status = Command::new(rustc)
        .args([
            "--edition",
            "2024",
            "--crate-type",
            "lib",
            "--emit",
            "metadata",
        ])
        .arg("--out-dir")
        .arg(&dir)
        .arg(&src)
        .status()
        .unwrap();
    assert!(status.success(), "generated bindings failed to type-check");
}

fn which_rustc() -> Result<String, ()> {
    let out = Command::new("rustc").arg("--version").output();
    match out {
        Ok(o) if o.status.success() => Ok("rustc".to_owned()),
        _ => Err(()),
    }
}

#[test]
fn kira_bindings_keep_field_defaults() {
    let text = "struct Tag {\n    let raw: U32\n}\n\nstruct Pen {\n    let width: Number = 1.5\n    let tag: Tag = Tag(0)\n    let kind: Kind = .Round\n    let marks: [U32] = []\n    let none: Optional<Tag> = .None\n}\n\nenum Kind {\n    Round\n    Square\n}\n";
    let module = parse(FileId::from_u32(0), "Pens", text).unwrap();
    let output = kira_bindings(&module);
    assert!(output.contains("    var width: Number = 1.5\n"));
    assert!(output.contains("    var tag: Tag = Tag(0)\n"));
    assert!(output.contains("    var kind: Kind = .Round\n"));
    assert!(output.contains("    var marks: [U32] = []\n"));
    assert!(output.contains("    var none: Optional<Tag> = .None\n"));
}

#[test]
fn aliases_stand_for_their_targets() {
    let text = "struct Paint {\n    let r: Number\n}\n\ntype Tint = Paint\n\nfunction tint(paint: borrow Tint) -> Tint\n";
    let module = parse(FileId::from_u32(0), "Paints", text).unwrap();
    let output = kira_bindings(&module);
    assert!(output.contains("type Tint = Paint\n"));
    let function = module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            klf_ridl::module::ItemKind::Function(function) => Some(function),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        function.params[0].ty,
        klf_ridl::types::Type::nominal("Paint")
    );
    assert_eq!(
        function.result,
        Some(klf_ridl::types::Type::nominal("Paint"))
    );
}

#[test]
fn reads_a_defaulted_family_field_after_a_slot_back_as_a_slot() {
    let source = r"
trait Widget {
    function body(borrow self) -> I64
}

struct EmptyView {
}
extend EmptyView: Widget

struct Section {
    let spacing: I64 = 4
    let children: [Any Widget]
    let header: Any Widget = EmptyView()
    let footer: Any Widget = EmptyView()
}
extend Section: Widget
";
    let out = kira_bindings(&parse(FileId::from_u32(0), "KiraUI", source).unwrap());
    assert!(
        out.contains("construct Section(spacing: Int = 4) extends Widget {"),
        "{out}"
    );
    assert!(out.contains("    let children: [some Widget]\n"), "{out}");
    assert!(
        out.contains("    let header: some Widget = EmptyView()\n"),
        "{out}"
    );
    assert!(
        out.contains("    let footer: some Widget = EmptyView()\n"),
        "{out}"
    );
}

#[test]
fn reads_a_field_of_another_family_back_as_an_input() {
    let source = r"
trait Widget {
    function body(borrow self) -> I64
}

trait NavigationValue {
    function key(borrow self) -> I64
}

struct NavigationLink {
    let value: Any NavigationValue
    let label: Any Widget
}
extend NavigationLink: Widget
";
    let out = kira_bindings(&parse(FileId::from_u32(0), "KiraUI", source).unwrap());
    assert!(
        out.contains("construct NavigationLink(value: Any NavigationValue) extends Widget {"),
        "{out}"
    );
    assert!(out.contains("    let label: some Widget\n"), "{out}");
}

#[test]
fn reads_a_last_family_parameter_back_as_a_slot() {
    let source = r"
trait Widget {
    function body(borrow self) -> I64
}

function stack(spacing: I64, children: [Any Widget]) -> Any Widget
function framed(content: Any Widget) -> Any Widget
function first(content: Any Widget, spacing: I64) -> Any Widget
";
    let out = kira_bindings(&parse(FileId::from_u32(0), "KiraUI", source).unwrap());
    assert!(
        out.contains("function stack(spacing: Int, children: [some Widget]) -> Any Widget"),
        "{out}"
    );
    assert!(
        out.contains("function framed(content: some Widget) -> Any Widget"),
        "{out}"
    );
    assert!(
        out.contains("function first(content: Any Widget, spacing: Int) -> Any Widget"),
        "{out}"
    );
}

const GENERIC: &str = r"
import Foundation

trait Shape {
    function area(borrow self) -> Number
}

@Instance(Number)
struct Square<Value> {
    let side: Number
    let tag: Value
}
extend Square<Value>: Shape

@Instance(Number)
struct Holder<Value> {
    let get: () -> Value
    let items: [Value]
}

extend Holder<Value> {
    function read(borrow self) -> Value
}

function make(n: Number) -> Holder<Number>
";

#[test]
fn kira_binds_generic_declarations_with_their_named_instances() {
    let module = parse(FileId::from_u32(0), "Gen", GENERIC).unwrap();
    let out = kira_bindings(&module);
    assert!(out.contains("@Instance(Number)\nstruct Holder<Value> {\n    var get: () -> Value\n    var items: [Value]\n}"), "{out}");
    assert!(
        out.contains(
            "@Instance(Number)\nconstruct Square<Value>(side: Number, tag: Value) extends Shape {}"
        ),
        "{out}"
    );
    assert!(
        out.contains("extend Holder {\n    @Ridl(\"Gen\") function read(borrow self) -> Value\n}"),
        "{out}"
    );
    assert!(
        out.contains("@Ridl(\"Gen\") function make(n: Number) -> Holder<Number>"),
        "{out}"
    );
}

#[test]
fn a_generic_declaration_binds_its_parameters() {
    let module = parse(FileId::from_u32(0), "Gen", GENERIC).unwrap();
    let holder = module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            klf_ridl::module::ItemKind::Struct(s) if s.name == "Holder" => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    let items = &holder.fields.unwrap()[1].ty;
    assert_eq!(
        *items,
        klf_ridl::types::Type::Array(Box::new(klf_ridl::types::Type::Generic("Value".to_owned())))
    );
}

#[test]
fn kira_bindings_keep_parameter_defaults() {
    let source = "struct Metric {\n    let points: F64\n}\nfunction pad(edges: U32, length: Metric = Metric { points: 8.0 }, centered: Bool = true)";
    let module = parse(FileId::from_u32(0), "Layout", source).unwrap();
    let output = kira_bindings(&module);
    assert!(output.contains(
        "@Ridl(\"Layout\") function pad(edges: U32, length: Metric = Metric { points: 8.0 }, centered: Bool = true)"
    ));
}

#[test]
fn reads_a_family_member_that_changes_its_value_back_with_its_receiver() {
    let source = r"
trait Platform {
    function deliver(borrow mut self, code: I64)
    function onConfigure(borrow mut self, width: I64)
}
extend Platform {
    function deliver(borrow mut self, code: I64)
}
";
    let out = kira_bindings(&parse(FileId::from_u32(0), "Display", source).unwrap());
    assert!(out.contains("construct Platform {"), "{out}");
    assert!(
        out.contains("    @Ridl(\"Display\") function deliver(borrow mut self, code: Int)"),
        "{out}"
    );
    assert!(
        out.contains("    @Required function onConfigure(borrow mut self, width: Int)"),
        "{out}"
    );
}
