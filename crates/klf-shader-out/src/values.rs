//! The `values` table: what the validator evaluated each constant, override
//! and global initializer to. Outs write the value rather than the
//! expression that computed it, as the text targets cannot all evaluate it.

use crate::record::Record;
use crate::tables::literal_value;
use klf_shader::print::half_to_f32;
use klf_shader::{Expression, Handle, Literal, Module, Type, TypeInner};
use klf_shader_valid::{ConstValue, ModuleInfo};

/// The fields every literal has: its value without the scalar suffix, the
/// scalar, and for a float the shortest text that reads back as the same
/// `f64`, whether it is finite, and its bits as an `f32`.
pub(crate) fn literal_fields(record: &mut Record, literal: Literal) {
    record
        .text("literal", literal_value(literal))
        .text("scalar", literal.scalar().text());
    let (value, bits) = match literal {
        Literal::F16(bits) => {
            let value = half_to_f32(bits);
            (f64::from(value), value.to_bits())
        }
        Literal::F32(bits) => (f64::from(f32::from_bits(bits)), bits),
        Literal::F64(bits) => {
            let value = f64::from_bits(bits);
            #[allow(clippy::cast_possible_truncation)]
            let single = value as f32;
            (value, single.to_bits())
        }
        _ => return,
    };
    record
        .text("float", format!("{value:?}"))
        .int("bits", bits)
        .flag("finite", value.is_finite());
}

/// The value `init` evaluated to, as type `ty`, added to `records`; `None`
/// when the validator could not evaluate it.
pub(crate) fn value(
    module: &Module,
    info: &ModuleInfo,
    init: Handle<Expression>,
    ty: Handle<Type>,
    records: &mut Vec<Record>,
) -> Option<i64> {
    let value = info.constants.get(init.index())?.as_ref()?;
    Some(push(module, Some(ty), value, records))
}

/// The type of item `index` of a composite of type `ty`, when the module
/// holds it.
fn element(module: &Module, ty: Handle<Type>, index: usize) -> Option<Handle<Type>> {
    match &module.types[ty].inner {
        TypeInner::Struct { members, .. } => members.get(index).map(|member| member.ty),
        TypeInner::Array { base, .. } => Some(*base),
        TypeInner::Matrix { rows, scalar, .. } => klf_shader::resolve::find(
            &module.types,
            &TypeInner::Vector {
                size: *rows,
                scalar: *scalar,
            },
        ),
        TypeInner::Vector { scalar, .. } => {
            klf_shader::resolve::find(&module.types, &TypeInner::Scalar(*scalar))
        }
        _ => None,
    }
}

fn push(
    module: &Module,
    ty: Option<Handle<Type>>,
    value: &ConstValue,
    records: &mut Vec<Record>,
) -> i64 {
    let mut record = match value {
        ConstValue::Scalar(scalar) => {
            let mut record = Record::new("scalar");
            literal_fields(&mut record, scalar.to_literal());
            record
        }
        ConstValue::Composite(parts) => {
            let items = parts
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    let item = ty.and_then(|ty| element(module, ty, index));
                    push(module, item, part, records)
                })
                .collect();
            let mut record = Record::new("composite");
            record.ints("items", items);
            record
        }
    };
    record.handle("type", ty);
    records.push(record.finish());
    records.len() as i64 - 1
}
