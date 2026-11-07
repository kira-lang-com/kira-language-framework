use tir_format::{
    BinaryFunctionParameter, BinaryFunctionType, BinaryInstruction, BinaryNominalKind, BinaryType,
    BinaryTypeTable,
};

use super::aggregate::payload_type;
use super::{address_type, index_operand, type_at, type_index};

pub(super) fn infer_field_address(
    instruction: &BinaryInstruction,
    address: Option<u64>,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let BinaryType::Address { pointee, space } = type_at(types, address?)? else {
        return None;
    };
    let field_index = usize::try_from(index_operand(instruction)?).ok()?;
    let field_type = match type_at(types, *pointee)? {
        BinaryType::Tuple(fields) => *fields.get(field_index)?,
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let declaration_record = types
                .declarations
                .get(usize::try_from(*declaration).ok()?)?;
            if declaration_record.kind == BinaryNominalKind::Enum
                || declaration_record.generic_parameters.len() != arguments.len()
            {
                return None;
            }
            let definition = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)?;
            let field_type = definition.fields.get(field_index)?.ty;
            substitute_type(types, field_type, arguments)?
        }
        _ => return None,
    };
    address_type(types, field_type, *space)
}

pub(super) fn infer_element_address(address: Option<u64>, types: &BinaryTypeTable) -> Option<u64> {
    let BinaryType::Address { pointee, space } = type_at(types, address?)? else {
        return None;
    };
    let BinaryType::Array { element, .. } = type_at(types, *pointee)? else {
        return None;
    };
    address_type(types, *element, *space)
}

pub(super) fn infer_enum_payload_address(
    instruction: &BinaryInstruction,
    address: Option<u64>,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let BinaryType::Address { pointee, space } = type_at(types, address?)? else {
        return None;
    };
    let BinaryType::Nominal {
        declaration,
        arguments,
    } = type_at(types, *pointee)?
    else {
        return None;
    };
    let declaration_record = types
        .declarations
        .get(usize::try_from(*declaration).ok()?)?;
    if declaration_record.kind != BinaryNominalKind::Enum
        || declaration_record.generic_parameters.len() != arguments.len()
    {
        return None;
    }
    let definition = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *declaration)?;
    let variant = definition
        .variants
        .get(usize::try_from(index_operand(instruction)?).ok()?)?;
    let payload = variant
        .payload
        .iter()
        .map(|ty| substitute_type(types, *ty, arguments))
        .collect::<Option<Vec<_>>>()?;
    address_type(types, payload_type(types, &payload)?, *space)
}

pub(super) fn has_enum_variant(
    instruction: &BinaryInstruction,
    address: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    let Some(BinaryType::Address { pointee, .. }) = address.and_then(|index| type_at(types, index))
    else {
        return false;
    };
    let Some(BinaryType::Nominal {
        declaration,
        arguments,
    }) = type_at(types, *pointee)
    else {
        return false;
    };
    if types
        .declarations
        .get(*declaration as usize)
        .is_none_or(|decl| {
            decl.kind != BinaryNominalKind::Enum || decl.generic_parameters.len() != arguments.len()
        })
    {
        return false;
    }
    types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *declaration)
        .and_then(|definition| {
            index_operand(instruction)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| definition.variants.get(index))
        })
        .is_some()
}

pub(super) fn substitute_type(
    types: &BinaryTypeTable,
    index: u64,
    arguments: &[u64],
) -> Option<u64> {
    let ty = type_at(types, index)?;
    let specialized = match ty {
        BinaryType::Generic { parameter } => {
            return arguments.get(usize::try_from(*parameter).ok()?).copied();
        }
        BinaryType::Address { pointee, space } => {
            let pointee = substitute_type(types, *pointee, arguments)?;
            BinaryType::Address {
                pointee,
                space: *space,
            }
        }
        BinaryType::FixedVector { lanes, element } => BinaryType::FixedVector {
            lanes: *lanes,
            element: substitute_type(types, *element, arguments)?,
        },
        BinaryType::ScalableVector { element } => BinaryType::ScalableVector {
            element: substitute_type(types, *element, arguments)?,
        },
        BinaryType::Tuple(elements) => BinaryType::Tuple(
            elements
                .iter()
                .map(|ty| substitute_type(types, *ty, arguments))
                .collect::<Option<Vec<_>>>()?,
        ),
        BinaryType::Array { count, element } => BinaryType::Array {
            count: *count,
            element: substitute_type(types, *element, arguments)?,
        },
        BinaryType::Nominal {
            declaration,
            arguments: nested,
        } => BinaryType::Nominal {
            declaration: *declaration,
            arguments: nested
                .iter()
                .map(|ty| substitute_type(types, *ty, arguments))
                .collect::<Option<Vec<_>>>()?,
        },
        BinaryType::Reference { pointee } => BinaryType::Reference {
            pointee: substitute_type(types, *pointee, arguments)?,
        },
        BinaryType::Unique { pointee } => BinaryType::Unique {
            pointee: substitute_type(types, *pointee, arguments)?,
        },
        BinaryType::Weak { pointee } => BinaryType::Weak {
            pointee: substitute_type(types, *pointee, arguments)?,
        },
        BinaryType::Function(function) => {
            BinaryType::Function(substitute_function(types, function, arguments)?)
        }
        BinaryType::Closure {
            function_type,
            borrowed,
        } => BinaryType::Closure {
            function_type: substitute_type(types, *function_type, arguments)?,
            borrowed: *borrowed,
        },
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => BinaryType::Coroutine {
            yield_type: substitute_type(types, *yield_type, arguments)?,
            resume_type: substitute_type(types, *resume_type, arguments)?,
            result_type: substitute_type(types, *result_type, arguments)?,
        },
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => BinaryType::Texture {
            dimension: *dimension,
            component: substitute_type(types, *component, arguments)?,
            access: *access,
            multisampled: *multisampled,
        },
        BinaryType::VisibleFunctionTable { function_type } => BinaryType::VisibleFunctionTable {
            function_type: substitute_type(types, *function_type, arguments)?,
        },
        BinaryType::Tensor { rank, element } => BinaryType::Tensor {
            rank: *rank,
            element: substitute_type(types, *element, arguments)?,
        },
        BinaryType::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => BinaryType::CooperativeMatrix {
            scope: *scope,
            rows: *rows,
            columns: *columns,
            element: substitute_type(types, *element, arguments)?,
            use_kind: *use_kind,
        },
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => BinaryType::Entry {
            kind: *kind,
            root: substitute_type(types, *root, arguments)?,
            immediate: substitute_type(types, *immediate, arguments)?,
        },
        _ => return Some(index),
    };
    type_index(types, |candidate| candidate == &specialized)
}

fn substitute_function(
    types: &BinaryTypeTable,
    function: &BinaryFunctionType,
    arguments: &[u64],
) -> Option<BinaryFunctionType> {
    Some(BinaryFunctionType {
        parameters: function
            .parameters
            .iter()
            .map(|parameter| {
                Some(BinaryFunctionParameter {
                    convention: parameter.convention,
                    context: parameter.context,
                    ty: substitute_type(types, parameter.ty, arguments)?,
                })
            })
            .collect::<Option<Vec<_>>>()?,
        result: substitute_type(types, function.result, arguments)?,
        error: match function.error {
            Some(ty) => Some(substitute_type(types, ty, arguments)?),
            None => None,
        },
        asynchronous: function.asynchronous,
        c_abi: function.c_abi,
        foreign: function.foreign,
    })
}
