//! Symbol mangling suite (section 24.2).

use klf_ridl::mangle::{
    descriptor_symbol, function_symbol, instance_method_symbol, instance_symbol, method_symbol,
    type_code, witness_symbol,
};
use klf_ridl::types::{Scalar, Type};

#[test]
fn free_function_matches_the_spec_example() {
    let tag = "6f1c2a9e44d07b31";
    let s = function_symbol("Geometry", "parse", Some(tag));
    assert_eq!(s, "_RIDL18Geometry5parseF_v6f1c2a9e44d07b31");
}

#[test]
fn free_function_without_abitag() {
    assert_eq!(
        function_symbol("Geometry", "parse", None),
        "_RIDL18Geometry5parseF"
    );
}

#[test]
fn method_symbol_ends_with_type_then_method() {
    assert_eq!(
        method_symbol("Geometry", "Scene", "add", None),
        "_RIDL18Geometry5Scene3addM"
    );
}

#[test]
fn descriptor_and_witness() {
    assert_eq!(
        descriptor_symbol("Geometry", "Scene"),
        "_RIDL18Geometry5SceneD"
    );
    assert_eq!(
        witness_symbol("Geometry", "Shape", "Geometry", "Drawable"),
        "_RIDL18Geometry5ShapeW8Geometry8Drawable"
    );
}

#[test]
fn scalar_type_codes() {
    assert_eq!(type_code(&Type::Scalar(Scalar::I64)), "d");
    assert_eq!(type_code(&Type::Scalar(Scalar::U128)), "l");
    assert_eq!(type_code(&Type::Scalar(Scalar::F32)), "f");
    assert_eq!(type_code(&Type::Scalar(Scalar::Bool)), "o");
    assert_eq!(type_code(&Type::String), "s");
}

#[test]
fn compound_type_codes() {
    let arr = Type::Array(Box::new(Type::Scalar(Scalar::I32)));
    assert_eq!(type_code(&arr), "Ac");
    let opt = Type::Optional(Box::new(Type::Scalar(Scalar::I64)));
    assert_eq!(type_code(&opt), "Od");
    let res = Type::Result(
        Box::new(Type::Scalar(Scalar::I64)),
        Box::new(Type::Scalar(Scalar::I32)),
    );
    assert_eq!(type_code(&res), "Rdc");
    let nominal = Type::nominal("Shape");
    assert_eq!(type_code(&nominal), "N5ShapeE");
}

#[test]
fn named_instance_symbol() {
    // @Instance(Shape) on sort<T> -> instance over the nominal Shape.
    let s = instance_symbol("Geometry", "sort", &[Type::nominal("Shape")], None);
    assert_eq!(s, "_RIDL18Geometry4sortIN5ShapeEE");
}

#[test]
fn generic_nominal_type_code() {
    let ty = Type::Nominal {
        name: "Pair".to_owned(),
        args: vec![Type::Scalar(Scalar::I64), Type::Scalar(Scalar::F64)],
    };
    assert_eq!(type_code(&ty), "N4PairIdgEE");
}

#[test]
fn a_scoped_type_spelling_mangles_as_its_identifier() {
    assert_eq!(
        witness_symbol("KiraUI", "MaterialLayer#3", "KiraUI", "Widget"),
        "_RIDL16KiraUI13MaterialLayerW6KiraUI6Widget"
    );
    assert_eq!(
        witness_symbol("KiraUI", "MaterialLayer#0", "KiraUI", "Widget"),
        witness_symbol("KiraUI", "MaterialLayer", "KiraUI", "Widget")
    );
}

#[test]
fn instance_method_is_kind_i_over_type_and_method() {
    assert_eq!(
        instance_method_symbol("Gen", "Holder", "read", &[Type::nominal("Number")], None),
        "_RIDL13Gen6Holder4readIN6NumberEE"
    );
}
