//! The types a name spells: the scalars and the named types of section 20.

use crate::types::{Scalar, Type};

pub(super) fn named_type(name: &str, args: Vec<Type>) -> Type {
    if args.is_empty() {
        if let Some(scalar) = scalar_of(name) {
            return Type::Scalar(scalar);
        }
        if name == "String" {
            return Type::String;
        }
    }
    match name {
        "Optional" if args.len() == 1 => Type::Optional(Box::new(args.into_iter().next().unwrap())),
        "Result" if args.len() == 2 => {
            let mut it = args.into_iter();
            Type::Result(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))
        }
        _ => Type::Nominal {
            name: name.to_owned(),
            args,
        },
    }
}

pub(super) fn scalar_of(name: &str) -> Option<Scalar> {
    Some(match name {
        "I8" => Scalar::I8,
        "I16" => Scalar::I16,
        "I32" => Scalar::I32,
        "I64" | "Int" => Scalar::I64,
        "I128" => Scalar::I128,
        "U8" => Scalar::U8,
        "U16" => Scalar::U16,
        "U32" => Scalar::U32,
        "U64" => Scalar::U64,
        "U128" => Scalar::U128,
        "F32" => Scalar::F32,
        "F64" | "Float" => Scalar::F64,
        "Bool" => Scalar::Bool,
        "ISize" => Scalar::ISize,
        "USize" => Scalar::USize,
        "RawPtr" => Scalar::RawPtr,
        "Void" => Scalar::Void,
        _ => return None,
    })
}
