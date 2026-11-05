use tir_types::{FloatType, Type};

use crate::{DefinitionKind, Layout, LayoutEngine, LayoutField, LayoutRule, NominalDefinition};

#[test]
fn tir_8_2_integer_scalars_and_arbitrary_bitwidths() {
    let mut engine = LayoutEngine::new();
    for (bits, size, align) in [
        (1, 1, 1),
        (8, 1, 1),
        (16, 2, 2),
        (24, 4, 4),
        (32, 4, 4),
        (64, 8, 8),
        (65, 16, 16),
        (128, 16, 16),
        (129, 32, 16),
        (16384, 2048, 16),
    ] {
        assert_eq!(
            engine.layout(&Type::Integer(bits)).expect("valid scalar"),
            Layout::new(size, align),
            "i{bits} layout differs from TIR section 8.2"
        );
    }
}

#[test]
fn tir_8_3_vector_alignment_and_padding() {
    let mut engine = LayoutEngine::new();
    for (lanes, size, align) in [
        (2, 2, 2),
        (3, 4, 4),
        (4, 4, 4),
        (8, 8, 8),
        (16, 16, 16),
        (32, 32, 16),
        (64, 64, 16),
    ] {
        assert_eq!(
            engine
                .layout(&Type::FixedVector {
                    lanes,
                    element: Box::new(Type::Integer(1)),
                })
                .expect("valid vector of i1"),
            Layout::new(size, align),
            "vec<{lanes} x i1> has one byte per lane and follows the vector alignment rule"
        );
    }
    assert_eq!(
        engine
            .layout(&Type::FixedVector {
                lanes: 3,
                element: Box::new(Type::Float(FloatType::F32)),
            })
            .expect("valid vector"),
        Layout::new(16, 16),
    );
}

fn definition(rule: LayoutRule) -> NominalDefinition {
    NominalDefinition {
        path: "Smoke.Record".to_owned(),
        parameters: vec![],
        opaque: false,
        rule,
        kind: DefinitionKind::Struct,
        fields: vec![
            LayoutField::named("a", Type::Integer(8)),
            LayoutField::named("b", Type::Integer(64)),
            LayoutField::named("c", Type::Integer(16)),
        ],
        variants: vec![],
        tag_type: None,
        explicit_size: None,
        explicit_align: None,
    }
}

#[test]
fn tir_8_1_ridl_and_packed_field_layouts() {
    for (rule, size, align, offsets) in [
        (LayoutRule::Ridl, 24, 8, [0, 8, 16]),
        (LayoutRule::Packed(1), 11, 1, [0, 1, 9]),
        (LayoutRule::Packed(2), 12, 2, [0, 2, 10]),
        (LayoutRule::Packed(4), 16, 4, [0, 4, 12]),
    ] {
        let mut engine = LayoutEngine::new();
        engine
            .add_definition(definition(rule))
            .expect("valid nominal definition");
        let record = engine
            .nominal_layout(&Type::Nominal {
                path: "Smoke.Record".to_owned(),
                arguments: vec![],
            })
            .expect("nominal layout");
        assert_eq!(record.layout, Layout::new(size, align));
        assert_eq!(
            record
                .fields
                .iter()
                .map(|f| f.byte_offset)
                .collect::<Vec<_>>(),
            offsets
        );
    }
}

#[test]
fn tir_8_1_explicit_enum_layout_is_forbidden() {
    let mut engine = LayoutEngine::new();
    let mut enum_definition = definition(LayoutRule::Explicit);
    enum_definition.kind = DefinitionKind::Enum;
    enum_definition.fields.clear();
    enum_definition
        .variants
        .push(crate::EnumVariant { fields: vec![] });
    enum_definition.tag_type = Some(Type::Integer(8));
    enum_definition.explicit_size = Some(8);
    enum_definition.explicit_align = Some(8);
    assert!(matches!(
        engine.add_definition(enum_definition),
        Err(crate::LayoutError::InvalidType(message)) if message.contains("not supported for enums")
    ));
}

#[test]
fn tir_8_4_c_bitfields_respect_storage_boundaries() {
    let mut low = LayoutField::named("low", Type::Integer(8));
    low.bit_width = Some(3);
    let mut high = LayoutField::named("high", Type::Integer(8));
    high.bit_width = Some(6);
    let mut engine = LayoutEngine::new();
    let mut record = definition(LayoutRule::C);
    record.fields = vec![low, high];
    engine
        .add_definition(record)
        .expect("C bitfields are valid");
    let layout = engine
        .nominal_layout(&Type::Nominal {
            path: "Smoke.Record".to_owned(),
            arguments: vec![],
        })
        .expect("C bitfield layout");
    assert_eq!(layout.layout, Layout::new(2, 1));
    assert_eq!(
        layout
            .fields
            .iter()
            .map(|f| (f.byte_offset, f.bit_offset))
            .collect::<Vec<_>>(),
        [(0, Some(0)), (1, Some(0))]
    );
}

#[test]
fn tir_8_2_packed_narrow_floats_in_arrays_and_vectors() {
    let mut engine = LayoutEngine::new();
    for (float, count, bytes) in [
        (FloatType::F4E2M1, 3, 2),
        (FloatType::F4E2M1, 4, 2),
        (FloatType::F6E2M3, 4, 3),
        (FloatType::F6E3M2, 4, 3),
    ] {
        let element = Type::Float(float);
        assert_eq!(
            engine
                .layout(&Type::Array {
                    count,
                    element: Box::new(element.clone())
                })
                .expect("packed array"),
            Layout::new(bytes, 1)
        );
        if matches!(count, 4) {
            assert_eq!(
                engine
                    .layout(&Type::FixedVector {
                        lanes: count as u32,
                        element: Box::new(element)
                    })
                    .expect("packed vector"),
                Layout::new(bytes, 1)
            );
        }
    }
}

#[test]
fn tir_8_1_explicit_struct_overlaps_are_rejected() {
    let mut engine = LayoutEngine::new();
    let mut record = definition(LayoutRule::Explicit);
    record.fields.truncate(2);
    record.fields[0].ty = Type::Integer(32);
    record.fields[0].explicit_offset = Some(0);
    record.fields[1].ty = Type::Integer(32);
    record.fields[1].explicit_offset = Some(0);
    record.explicit_size = Some(8);
    record.explicit_align = Some(4);
    engine
        .add_definition(record)
        .expect("definition stores explicit fields");
    let layout = engine.nominal_layout(&Type::Nominal {
        path: "Smoke.Record".to_owned(),
        arguments: vec![],
    });
    assert!(matches!(
        layout,
        Err(crate::LayoutError::InvalidExplicitLayout(_))
    ));
}

#[test]
fn tir_7_10_address_only_and_register_only_have_no_memory_layout() {
    let mut engine = LayoutEngine::new();
    for ty in [
        Type::ScalableVector(Box::new(Type::Integer(32))),
        Type::Address {
            pointee: Box::new(Type::Integer(32)),
            space: None,
        },
    ] {
        assert!(matches!(
            engine.layout(&ty),
            Err(crate::LayoutError::AddressOnlyType)
        ));
    }
    for ty in [Type::RayQuery, Type::HitObject] {
        assert!(matches!(
            engine.layout(&ty),
            Err(crate::LayoutError::RegisterOnlyType)
        ));
    }
}
