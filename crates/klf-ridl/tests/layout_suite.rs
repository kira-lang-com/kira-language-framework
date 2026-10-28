//! RIDL layout suite (section 21). Fixed values on a 64-bit target (W = 8).

use klf_ridl::layout::{Layout, LayoutEngine};
use klf_ridl::types::{Case, Decl, EnumDecl, Field, Registry, Scalar, StructDecl, Type};

const W: u64 = 8;

fn engine(registry: &Registry) -> LayoutEngine<'_> {
    LayoutEngine::new(registry, W)
}

fn s(scalar: Scalar) -> Type {
    Type::Scalar(scalar)
}

fn layout(registry: &Registry, ty: &Type) -> Layout {
    engine(registry).layout_of(ty).unwrap()
}

#[test]
fn scalars_match_the_table() {
    let r = Registry::new();
    assert_eq!(layout(&r, &s(Scalar::I8)), Layout::new(1, 1));
    assert_eq!(layout(&r, &s(Scalar::Bool)), Layout::new(1, 1));
    assert_eq!(layout(&r, &s(Scalar::I16)), Layout::new(2, 2));
    assert_eq!(layout(&r, &s(Scalar::F32)), Layout::new(4, 4));
    assert_eq!(layout(&r, &s(Scalar::F64)), Layout::new(8, 8));
    assert_eq!(layout(&r, &s(Scalar::U128)), Layout::new(16, 16));
    assert_eq!(layout(&r, &s(Scalar::RawPtr)), Layout::new(W, W));
    assert_eq!(layout(&r, &s(Scalar::USize)), Layout::new(W, W));
}

#[test]
fn a_sized_number_is_laid_out_as_its_storage_integer() {
    let module = klf_ridl::parse(
        klf_core::FileId::from_u32(0),
        "Sized",
        "function width(value: UN32) -> N8\n",
    )
    .unwrap();
    let r = module.registry();
    for (name, bytes) in [
        ("N8", 1),
        ("UN8", 1),
        ("N16", 2),
        ("UN16", 2),
        ("N32", 4),
        ("UN32", 4),
    ] {
        let ty = Type::Nominal {
            name: name.to_owned(),
            args: Vec::new(),
        };
        assert_eq!(layout(&r, &ty), Layout::new(bytes, bytes));
    }
}

#[test]
fn void_has_no_layout() {
    let r = Registry::new();
    assert!(engine(&r).layout_of(&s(Scalar::Void)).is_err());
}

fn field(name: &str, ty: Type) -> Field {
    Field {
        name: name.to_owned(),
        ty,
        default: None,
    }
}

fn struct_decl(name: &str, fields: Vec<Field>) -> Decl {
    Decl::Struct(StructDecl {
        name: name.to_owned(),
        generics: Vec::new(),
        fields: Some(fields),
    })
}

#[test]
fn struct_packs_in_declaration_order_with_padding() {
    let mut r = Registry::new();
    r.insert(struct_decl(
        "Mixed",
        vec![
            field("a", s(Scalar::U8)),
            field("b", s(Scalar::I64)),
            field("c", s(Scalar::U8)),
        ],
    ));
    // a@0, pad to 8, b@8, c@16, size rounds to 24, align 8.
    assert_eq!(layout(&r, &Type::nominal("Mixed")), Layout::new(24, 8));
    let placements = engine(&r).struct_fields("Mixed", &[]).unwrap();
    assert_eq!(placements[0].offset, 0);
    assert_eq!(placements[1].offset, 8);
    assert_eq!(placements[2].offset, 16);
}

#[test]
fn vec3_of_floats() {
    let mut r = Registry::new();
    r.insert(struct_decl(
        "Vec3",
        vec![
            field("x", s(Scalar::F32)),
            field("y", s(Scalar::F32)),
            field("z", s(Scalar::F32)),
        ],
    ));
    assert_eq!(layout(&r, &Type::nominal("Vec3")), Layout::new(12, 4));
}

#[test]
fn empty_struct_is_zero_sized() {
    let mut r = Registry::new();
    r.insert(struct_decl("Empty", Vec::new()));
    assert_eq!(layout(&r, &Type::nominal("Empty")), Layout::new(0, 1));
}

#[test]
fn opaque_struct_is_one_pointer() {
    let mut r = Registry::new();
    r.insert(Decl::Struct(StructDecl {
        name: "Scene".to_owned(),
        generics: Vec::new(),
        fields: None,
    }));
    assert_eq!(layout(&r, &Type::nominal("Scene")), Layout::new(W, W));
}

#[test]
fn tuple_lays_out_like_a_struct() {
    let r = Registry::new();
    let t = Type::Tuple(vec![s(Scalar::U8), s(Scalar::I32)]);
    assert_eq!(layout(&r, &t), Layout::new(8, 4));
}

fn enum_decl(name: &str, open: bool, cases: Vec<Case>) -> Decl {
    Decl::Enum(EnumDecl {
        name: name.to_owned(),
        generics: Vec::new(),
        open,
        cases,
    })
}

fn case(name: &str, payload: Vec<Type>) -> Case {
    Case {
        name: name.to_owned(),
        payload,
    }
}

#[test]
fn payloadless_enum_is_just_its_tag() {
    let mut r = Registry::new();
    r.insert(enum_decl(
        "Dir",
        false,
        vec![
            case("N", vec![]),
            case("E", vec![]),
            case("S", vec![]),
            case("W", vec![]),
        ],
    ));
    assert_eq!(layout(&r, &Type::nominal("Dir")), Layout::new(1, 1));
}

#[test]
fn open_enum_always_has_a_u32_tag() {
    let mut r = Registry::new();
    r.insert(enum_decl(
        "Kind",
        true,
        vec![case("A", vec![]), case("B", vec![])],
    ));
    assert_eq!(layout(&r, &Type::nominal("Kind")), Layout::new(4, 4));
}

#[test]
fn enum_with_payload_lays_tag_then_payload() {
    let mut r = Registry::new();
    r.insert(struct_decl(
        "Vec3",
        vec![
            field("x", s(Scalar::F32)),
            field("y", s(Scalar::F32)),
            field("z", s(Scalar::F32)),
        ],
    ));
    // Circle(F32) | Box(Vec3, Vec3) | Empty. Max payload = two Vec3 = 24 bytes,
    // align 4. Tag u8 rounds up to 4, size = 4 + 24 = 28, align 4.
    r.insert(enum_decl(
        "Shape",
        false,
        vec![
            case("Circle", vec![s(Scalar::F32)]),
            case("Box", vec![Type::nominal("Vec3"), Type::nominal("Vec3")]),
            case("Empty", vec![]),
        ],
    ));
    assert_eq!(layout(&r, &Type::nominal("Shape")), Layout::new(28, 4));
}

#[test]
fn string_and_array_are_four_words() {
    let r = Registry::new();
    assert_eq!(layout(&r, &Type::String), Layout::new(4 * W, W));
    let arr = Type::Array(Box::new(s(Scalar::I64)));
    assert_eq!(layout(&r, &arr), Layout::new(4 * W, W));
}

#[test]
fn trait_object_and_closure_are_two_words() {
    let r = Registry::new();
    assert_eq!(
        layout(&r, &Type::TraitObject("Drawable".to_owned())),
        Layout::new(2 * W, W)
    );
    let closure = Type::Function {
        params: vec![s(Scalar::I64)],
        result: Box::new(s(Scalar::I64)),
    };
    assert_eq!(layout(&r, &closure), Layout::new(2 * W, W));
}

#[test]
fn optional_of_opaque_uses_the_pointer_niche() {
    let mut r = Registry::new();
    r.insert(Decl::Struct(StructDecl {
        name: "Scene".to_owned(),
        generics: Vec::new(),
        fields: None,
    }));
    let opt = Type::Optional(Box::new(Type::nominal("Scene")));
    assert_eq!(layout(&r, &opt), Layout::new(W, W));
}

#[test]
fn optional_of_scalar_has_no_niche() {
    let r = Registry::new();
    let opt = Type::Optional(Box::new(s(Scalar::I64)));
    // tag u8 padded to 8, then 8-byte payload.
    assert_eq!(layout(&r, &opt), Layout::new(16, 8));
}

#[test]
fn result_lays_out_as_a_two_case_enum() {
    let r = Registry::new();
    let res = Type::Result(Box::new(s(Scalar::I32)), Box::new(s(Scalar::I32)));
    // tag u8 padded to 4, payload 4 -> size 8 align 4.
    assert_eq!(layout(&r, &res), Layout::new(8, 4));
}

#[test]
fn generic_struct_substitutes_arguments() {
    let mut r = Registry::new();
    r.insert(Decl::Struct(StructDecl {
        name: "Pair".to_owned(),
        generics: vec!["T".to_owned()],
        fields: Some(vec![
            field("a", Type::Generic("T".to_owned())),
            field("b", Type::Generic("T".to_owned())),
        ]),
    }));
    let concrete = Type::Nominal {
        name: "Pair".to_owned(),
        args: vec![s(Scalar::I64)],
    };
    assert_eq!(layout(&r, &concrete), Layout::new(16, 8));
}
