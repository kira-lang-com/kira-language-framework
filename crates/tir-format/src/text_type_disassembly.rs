use std::fmt::Write as _;

use crate::FormatError;
use crate::types::{
    BinaryCopyPolicy, BinaryDestroyPolicy, BinaryFunctionType, BinaryLayout, BinaryNominalKind,
    BinaryPolicy, BinaryRelocatePolicy, BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use tir_types::{
    Abi, AddressSpace, ClosureType, Convention, CooperativeMatrixUse, EntryKind, FunctionType,
    MatrixScope, ParameterType, TextureAccess, TextureDimension, Type,
};

pub(crate) fn format_type_declarations(
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Result<String, FormatError> {
    let mut output = String::new();
    for (index, declaration) in types.declarations.iter().enumerate() {
        let Some(definition) = types
            .definitions
            .iter()
            .find(|definition| definition.declaration == index as u64)
        else {
            return Err(malformed(
                "textual TIR cannot represent an opaque nominal declaration",
            ));
        };
        if !output.is_empty() {
            output.push('\n');
        }
        write!(output, "type %{}", declaration.name).map_err(|_| formatting_error())?;
        if !declaration.generic_parameters.is_empty() {
            output.push('<');
            for (parameter_index, parameter) in declaration.generic_parameters.iter().enumerate() {
                if parameter_index != 0 {
                    output.push_str(", ");
                }
                write!(output, "${}", parameter.name).map_err(|_| formatting_error())?;
                if !parameter.bounds.is_empty() {
                    output.push_str(": ");
                    for (bound_index, bound) in parameter.bounds.iter().enumerate() {
                        if bound_index != 0 {
                            output.push_str(" & ");
                        }
                        write!(output, "%{bound}").map_err(|_| formatting_error())?;
                    }
                }
            }
            output.push('>');
        }
        write!(
            output,
            " = {} {} {{",
            nominal_kind(declaration.kind),
            layout_text(definition.layout)
        )
        .map_err(|_| formatting_error())?;
        match declaration.kind {
            BinaryNominalKind::Struct | BinaryNominalKind::Explicit => {
                for (field_index, field) in definition.fields.iter().enumerate() {
                    if field_index != 0 {
                        output.push_str(", ");
                    }
                    write!(
                        output,
                        "{}: {}",
                        field.name,
                        format_type(types, field.ty, Some(index as u64), signatures, 0)?
                    )
                    .map_err(|_| formatting_error())?;
                    if let Some(offset) = field.offset {
                        write!(output, " @{offset}").map_err(|_| formatting_error())?;
                        if let (Some(bit_offset), Some(bit_width)) =
                            (field.bit_offset, field.bit_width)
                        {
                            write!(output, ":{bit_offset}:{bit_width}")
                                .map_err(|_| formatting_error())?;
                        }
                    }
                    for annotation in &field.annotations {
                        write!(output, " @{}", annotation.name).map_err(|_| formatting_error())?;
                        if !annotation.arguments.is_empty() {
                            write!(output, "({})", annotation.arguments.join(", "))
                                .map_err(|_| formatting_error())?;
                        }
                    }
                }
            }
            BinaryNominalKind::Enum => {
                for (variant_index, variant) in definition.variants.iter().enumerate() {
                    if variant_index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&variant.name);
                    if !variant.payload.is_empty() {
                        output.push('(');
                        for (payload_index, ty) in variant.payload.iter().enumerate() {
                            if payload_index != 0 {
                                output.push_str(", ");
                            }
                            output.push_str(&format_type(
                                types,
                                *ty,
                                Some(index as u64),
                                signatures,
                                0,
                            )?);
                        }
                        output.push(')');
                    }
                }
            }
        }
        write!(
            output,
            "}} policy {{{}}}",
            policy_text(&definition.policy, signatures)?
        )
        .map_err(|_| formatting_error())?;
    }
    Ok(output)
}

pub(crate) fn format_type_reference(
    types: &BinaryTypeTable,
    index: u64,
    signatures: &BinarySignatureTable,
) -> Result<String, FormatError> {
    format_type(types, index, None, signatures, 0)
}

fn policy_text(
    policy: &BinaryPolicy,
    signatures: &BinarySignatureTable,
) -> Result<String, FormatError> {
    let copy = match policy.copy {
        BinaryCopyPolicy::Trivial => "trivial".to_owned(),
        BinaryCopyPolicy::None => "none".to_owned(),
        BinaryCopyPolicy::Implicit(index) => {
            format!("implicit(@{})", function_name(signatures, index)?)
        }
        BinaryCopyPolicy::Explicit(index) => {
            format!("explicit(@{})", function_name(signatures, index)?)
        }
    };
    let destroy = match policy.destroy {
        BinaryDestroyPolicy::Trivial => "trivial".to_owned(),
        BinaryDestroyPolicy::Function(index) => {
            format!("fn(@{})", function_name(signatures, index)?)
        }
    };
    let relocate = match policy.relocate {
        BinaryRelocatePolicy::Bitwise => "bitwise".to_owned(),
        BinaryRelocatePolicy::Function(index) => {
            format!("fn(@{})", function_name(signatures, index)?)
        }
    };
    let exclusivity = match policy.exclusivity {
        0 => "static",
        1 => "dynamic",
        2 => "none",
        _ => return Err(malformed("invalid exclusivity policy")),
    };
    let pairing = match policy.pairing {
        0 => "counted",
        1 => "none",
        _ => return Err(malformed("invalid pairing policy")),
    };
    let lifetime = match policy.lifetime {
        0 => "eager",
        1 => "scoped",
        _ => return Err(malformed("invalid lifetime policy")),
    };
    Ok(format!(
        " copy: {copy}, destroy: {destroy}, relocate: {relocate}, exclusivity: {exclusivity}, pairing: {pairing}, lifetime: {lifetime} "
    ))
}

fn function_name(signatures: &BinarySignatureTable, index: u64) -> Result<&str, FormatError> {
    signatures
        .records
        .get(usize::try_from(index).map_err(|_| malformed("function index overflows memory"))?)
        .map(|record| record.name.as_str())
        .ok_or_else(|| malformed("policy function index is out of range"))
}

fn format_type(
    table: &BinaryTypeTable,
    index: u64,
    scope: Option<u64>,
    signatures: &BinarySignatureTable,
    depth: usize,
) -> Result<String, FormatError> {
    if depth >= 256 {
        return Err(malformed("TYPES nesting exceeds 255"));
    }
    let ty = table
        .entries
        .get(usize::try_from(index).map_err(|_| malformed("type index overflows memory"))?)
        .ok_or_else(|| malformed("type index is out of range"))?;
    let child = |index| format_type(table, index, scope, signatures, depth + 1);
    let value = match ty {
        BinaryType::Integer(bits) => Type::Integer(
            u16::try_from(*bits)
                .map_err(|_| malformed("integer width exceeds textual type limits"))?,
        ),
        BinaryType::Float(float) => Type::Float(*float),
        BinaryType::Pointer(space) => Type::Pointer(address_space(*space)?),
        BinaryType::Address { pointee, space } => Type::Address {
            pointee: Box::new(type_value(table, *pointee, scope, signatures, depth + 1)?),
            space: if *space == 0 {
                None
            } else {
                Some(address_space(*space)?)
            },
        },
        BinaryType::Unit => Type::Unit,
        BinaryType::Never => Type::Never,
        BinaryType::FixedVector { lanes, element } => Type::FixedVector {
            lanes: u32::try_from(*lanes)
                .map_err(|_| malformed("vector lane count exceeds textual type limits"))?,
            element: Box::new(type_value(table, *element, scope, signatures, depth + 1)?),
        },
        BinaryType::ScalableVector { element } => Type::ScalableVector(Box::new(type_value(
            table,
            *element,
            scope,
            signatures,
            depth + 1,
        )?)),
        BinaryType::Tuple(elements) => Type::Tuple(
            elements
                .iter()
                .map(|index| type_value(table, *index, scope, signatures, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        BinaryType::Array { count, element } => Type::Array {
            count: *count,
            element: Box::new(type_value(table, *element, scope, signatures, depth + 1)?),
        },
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let declaration = table
                .declarations
                .get(
                    usize::try_from(*declaration)
                        .map_err(|_| malformed("nominal index overflows memory"))?,
                )
                .ok_or_else(|| malformed("nominal declaration is missing"))?;
            Type::Nominal {
                path: declaration.name.clone(),
                arguments: arguments
                    .iter()
                    .map(|index| type_value(table, *index, scope, signatures, depth + 1))
                    .collect::<Result<_, _>>()?,
            }
        }
        BinaryType::Generic { parameter } => {
            let declaration = scope
                .and_then(|index| table.declarations.get(index as usize))
                .ok_or_else(|| malformed("generic parameter has no nominal scope"))?;
            let parameter = declaration
                .generic_parameters
                .get(
                    usize::try_from(*parameter)
                        .map_err(|_| malformed("generic parameter index overflows memory"))?,
                )
                .ok_or_else(|| malformed("generic parameter index is out of range"))?;
            Type::Generic(parameter.name.clone())
        }
        BinaryType::Reference { pointee } => Type::Reference(Box::new(type_value(
            table,
            *pointee,
            scope,
            signatures,
            depth + 1,
        )?)),
        BinaryType::Unique { pointee } => Type::Unique(Box::new(type_value(
            table,
            *pointee,
            scope,
            signatures,
            depth + 1,
        )?)),
        BinaryType::Weak { pointee } => Type::Weak(Box::new(type_value(
            table,
            *pointee,
            scope,
            signatures,
            depth + 1,
        )?)),
        BinaryType::Function(function) => Type::Function(function_value(
            table,
            function,
            scope,
            signatures,
            depth + 1,
        )?),
        BinaryType::Closure {
            function_type,
            borrowed,
        } => {
            let BinaryType::Function(function) = table
                .entries
                .get(*function_type as usize)
                .ok_or_else(|| malformed("closure function type is missing"))?
            else {
                return Err(malformed("closure signature is not a function type"));
            };
            Type::Closure(ClosureType {
                borrowed: *borrowed,
                signature: function_value(table, function, scope, signatures, depth + 1)?,
            })
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => Type::Coroutine {
            yield_type: Box::new(type_value(
                table,
                *yield_type,
                scope,
                signatures,
                depth + 1,
            )?),
            resume_type: Box::new(type_value(
                table,
                *resume_type,
                scope,
                signatures,
                depth + 1,
            )?),
            result_type: Box::new(type_value(
                table,
                *result_type,
                scope,
                signatures,
                depth + 1,
            )?),
        },
        BinaryType::Frame { function_index } => {
            Type::Frame(function_name(signatures, *function_index)?.to_owned())
        }
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => Type::Texture {
            dimension: match dimension {
                0 => TextureDimension::D1,
                1 => TextureDimension::D2,
                2 => TextureDimension::D3,
                3 => TextureDimension::Cube,
                4 => TextureDimension::D1Array,
                5 => TextureDimension::D2Array,
                6 => TextureDimension::CubeArray,
                _ => return Err(malformed("invalid texture dimension")),
            },
            component: Box::new(type_value(table, *component, scope, signatures, depth + 1)?),
            access: match access {
                0 => TextureAccess::Sample,
                1 => TextureAccess::Read,
                2 => TextureAccess::Write,
                3 => TextureAccess::ReadWrite,
                _ => return Err(malformed("invalid texture access")),
            },
            multisampled: *multisampled,
        },
        BinaryType::Sampler => Type::Sampler,
        BinaryType::AccelerationStructure => Type::AccelerationStructure,
        BinaryType::VisibleFunctionTable { function_type } => {
            let BinaryType::Function(function) = table
                .entries
                .get(*function_type as usize)
                .ok_or_else(|| malformed("visible function table signature is missing"))?
            else {
                return Err(malformed(
                    "visible function table signature is not a function type",
                ));
            };
            Type::VisibleFunctionTable(Box::new(function_value(
                table,
                function,
                scope,
                signatures,
                depth + 1,
            )?))
        }
        BinaryType::Tensor { rank, element } => Type::Tensor {
            count: u32::try_from(*rank)
                .map_err(|_| malformed("tensor rank exceeds textual type limits"))?,
            element: Box::new(type_value(table, *element, scope, signatures, depth + 1)?),
        },
        BinaryType::RayQuery => Type::RayQuery,
        BinaryType::HitObject => Type::HitObject,
        BinaryType::CooperativeMatrix {
            scope: matrix_scope,
            rows,
            columns,
            element,
            use_kind,
        } => Type::CooperativeMatrix {
            scope: match matrix_scope {
                0 => MatrixScope::Subgroup,
                1 => MatrixScope::Threadgroup,
                _ => return Err(malformed("invalid cooperative matrix scope")),
            },
            rows: u32::try_from(*rows)
                .map_err(|_| malformed("matrix row count exceeds textual type limits"))?,
            columns: u32::try_from(*columns)
                .map_err(|_| malformed("matrix column count exceeds textual type limits"))?,
            element: Box::new(type_value(table, *element, scope, signatures, depth + 1)?),
            use_kind: match use_kind {
                0 => CooperativeMatrixUse::A,
                1 => CooperativeMatrixUse::B,
                2 => CooperativeMatrixUse::Acc,
                _ => return Err(malformed("invalid cooperative matrix use")),
            },
        },
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => Type::Entry {
            kind: match kind {
                0 => EntryKind::Compute,
                1 => EntryKind::Object,
                2 => EntryKind::Mesh,
                3 => EntryKind::Vertex,
                4 => EntryKind::Fragment,
                5 => EntryKind::Tile,
                6 => EntryKind::Raygen,
                7 => EntryKind::Intersection,
                8 => EntryKind::Anyhit,
                9 => EntryKind::Closesthit,
                10 => EntryKind::Miss,
                11 => EntryKind::Callable,
                12 => EntryKind::Node,
                _ => return Err(malformed("invalid entry type kind")),
            },
            root: Box::new(type_value(table, *root, scope, signatures, depth + 1)?),
            immediate: Box::new(type_value(table, *immediate, scope, signatures, depth + 1)?),
        },
    };
    let _ = child;
    Ok(value.to_string())
}

fn type_value(
    table: &BinaryTypeTable,
    index: u64,
    scope: Option<u64>,
    signatures: &BinarySignatureTable,
    depth: usize,
) -> Result<Type, FormatError> {
    let text = format_type(table, index, scope, signatures, depth)?;
    crate::parse_type(&text).map_err(|error| malformed(error.message))
}

fn function_value(
    table: &BinaryTypeTable,
    function: &BinaryFunctionType,
    scope: Option<u64>,
    signatures: &BinarySignatureTable,
    depth: usize,
) -> Result<FunctionType, FormatError> {
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| {
            Ok(ParameterType {
                context: parameter.context,
                convention: match parameter.convention {
                    0 => Some(Convention::Owned),
                    1 => Some(Convention::Borrow),
                    2 => Some(Convention::BorrowMut),
                    3 => None,
                    _ => return Err(malformed("invalid function parameter convention")),
                },
                ty: type_value(table, parameter.ty, scope, signatures, depth + 1)?,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(FunctionType {
        parameters,
        result: Box::new(type_value(
            table,
            function.result,
            scope,
            signatures,
            depth + 1,
        )?),
        failure: function
            .error
            .map(|index| type_value(table, index, scope, signatures, depth + 1).map(Box::new))
            .transpose()?,
        asynchronous: function.asynchronous,
        abi: if function.c_abi { Abi::C } else { Abi::Ridl },
        foreign: function.foreign,
    })
}

fn layout_text(layout: BinaryLayout) -> String {
    match layout {
        BinaryLayout::Ridl => "ridl".to_owned(),
        BinaryLayout::C => "c".to_owned(),
        BinaryLayout::Packed(alignment) => format!("packed({alignment})"),
        BinaryLayout::Explicit { size, align } => format!("explicit(size {size}, align {align})"),
    }
}

fn nominal_kind(kind: BinaryNominalKind) -> &'static str {
    match kind {
        BinaryNominalKind::Struct => "struct",
        BinaryNominalKind::Enum => "enum",
        BinaryNominalKind::Explicit => "explicit",
    }
}

fn address_space(space: u8) -> Result<AddressSpace, FormatError> {
    match space {
        0 => Ok(AddressSpace::Generic),
        1 => Ok(AddressSpace::Device),
        2 => Ok(AddressSpace::Constant),
        3 => Ok(AddressSpace::Threadgroup),
        4 => Ok(AddressSpace::Thread),
        5 => Ok(AddressSpace::ObjectPayload),
        6 => Ok(AddressSpace::RayPayload),
        7 => Ok(AddressSpace::HitAttribute),
        8 => Ok(AddressSpace::Tile),
        9 => Ok(AddressSpace::NodeRecord),
        _ => Err(malformed("invalid address space")),
    }
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}

fn formatting_error() -> FormatError {
    malformed("TIR text formatting failed")
}
