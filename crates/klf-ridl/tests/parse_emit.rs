//! Parse `.ridl` text, emit it, reparse, and check the module is unchanged.

use klf_core::FileId;
use klf_ridl::module::ItemKind;
use klf_ridl::{canonical_text, emit, parse, parse_spanned};

const EXAMPLE: &str = r"
import Canvas2D

struct Vec3 {
    let x: Float
    let y: Float
    let z: Float
}

enum Shape {
    Circle(Float)
    Box(Vec3, Vec3)
    Empty
}

struct Scene

trait Drawable {
    function draw(borrow self, on: borrow mut Canvas)
    function bounds(borrow self) -> Vec3
}

extend Scene {
    function add(borrow mut self, shape: Shape)
    function largest<T: Drawable>(borrow self, of: borrow [T]) -> Optional<T>
}

extend Shape: Drawable

function parse(text: borrow String) -> Result<Shape, ParseError>

@Instance(Shape)
function sort<T: Comparable>(items: borrow mut [T])
";

fn file() -> FileId {
    FileId::from_u32(0)
}

#[test]
fn emit_then_reparse_is_stable() {
    let module = parse(file(), "Geometry", EXAMPLE).unwrap();
    let text = emit(&module);
    let reparsed = parse(file(), "Geometry", &text).unwrap();
    assert_eq!(module, reparsed);
}

#[test]
fn canonical_text_is_deterministic_regardless_of_order() {
    let a = parse(file(), "M", EXAMPLE).unwrap();
    // A reordering of declarations must produce the same canonical text.
    let mut reordered = a.clone();
    reordered.items.reverse();
    assert_eq!(canonical_text(&a), canonical_text(&reordered));
}

#[test]
fn open_enum_flag_survives_round_trip() {
    let src = "@Open enum Kind { A B }";
    let module = parse(file(), "M", src).unwrap();
    if let ItemKind::Enum(e) = &module.items[0].kind {
        assert!(e.open);
    } else {
        panic!("expected enum");
    }
    let reparsed = parse(file(), "M", &emit(&module)).unwrap();
    assert_eq!(module, reparsed);
}

#[test]
fn source_annotations_leave_the_canonical_text_alone() {
    let plain = parse(file(), "M", "function area(r: Float) -> Float").unwrap();
    let pointed = parse(
        file(),
        "M",
        "@Source(\"../src/shapes.c:12:7\")\nfunction area(r: Float) -> Float",
    )
    .unwrap();
    assert_eq!(pointed.items[0].annotations[0].name, "Source");
    assert_eq!(canonical_text(&plain), canonical_text(&pointed));
    assert_eq!(parse(file(), "M", &emit(&pointed)).unwrap(), pointed);
}

#[test]
fn spans_name_each_declaration_and_its_source() {
    let text = "struct Point\n\n@Deprecated(\"old\") @Source(\"a.c:1:1\")\nfunction f()\n";
    let (module, spans) = parse_spanned(file(), "M", text).unwrap();
    assert_eq!(spans.len(), module.items.len());
    assert_eq!(spans[0].start, 0);
    let name = spans[0].name.unwrap();
    assert_eq!(&text[name.start as usize..name.end as usize], "Point");
    assert!(spans[0].sources.is_empty());
    assert_eq!(
        &text[spans[1].start as usize..],
        "@Deprecated(\"old\") @Source(\"a.c:1:1\")\nfunction f()\n"
    );
    let name = spans[1].name.unwrap();
    assert_eq!(&text[name.start as usize..name.end as usize], "f");
    let source = spans[1].sources[0];
    assert_eq!(
        &text[source.start as usize..source.end as usize],
        "\"a.c:1:1\""
    );
}

#[test]
fn parameter_defaults_survive_round_trip() {
    let source = "enum Edge { Top Bottom }\nfunction pad(edge: Edge = .Top, length: F64 = 8.0)";
    let module = parse(file(), "Layout", source).unwrap();
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected a function");
    };
    assert_eq!(function.params[0].default.as_deref(), Some(".Top"));
    assert_eq!(function.params[1].default.as_deref(), Some("8.0"));
    let reparsed = parse(file(), "Layout", &emit(&module)).unwrap();
    assert_eq!(module, reparsed);
}
