use crate::leb::write_uleb;
use crate::types::{
    BinaryCopyPolicy, BinaryDestroyPolicy, BinaryField, BinaryFunctionParameter,
    BinaryFunctionType, BinaryLayout, BinaryNominalDeclaration, BinaryNominalDefinition,
    BinaryNominalKind, BinaryPolicy, BinaryRelocatePolicy, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable};
use tir_types::FloatType;

pub(super) fn encode_type_table(
    table: &BinaryTypeTable,
    strings: &StringTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_count(table.declarations.len(), &mut bytes)?;
    for declaration in &table.declarations {
        encode_declaration(declaration, strings, &mut bytes)?;
    }
    write_count(table.entries.len(), &mut bytes)?;
    for entry in &table.entries {
        encode_type(entry, &mut bytes)?;
    }
    write_count(table.definitions.len(), &mut bytes)?;
    for definition in &table.definitions {
        encode_definition(definition, &table.declarations, strings, &mut bytes)?;
    }
    super::decode_type_table(&bytes, strings)?;
    Ok(bytes)
}

fn encode_declaration(
    declaration: &BinaryNominalDeclaration,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&declaration.name, strings, bytes)?;
    write_count(declaration.generic_parameters.len(), bytes)?;
    for parameter in &declaration.generic_parameters {
        write_string(&parameter.name, strings, bytes)?;
        write_count(parameter.bounds.len(), bytes)?;
        for bound in &parameter.bounds {
            write_string(bound, strings, bytes)?;
        }
    }
    bytes.push(match declaration.kind {
        BinaryNominalKind::Struct => 0,
        BinaryNominalKind::Enum => 1,
        BinaryNominalKind::Explicit => 2,
    });
    Ok(())
}

fn encode_type(ty: &BinaryType, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    match ty {
        BinaryType::Integer(bits) => {
            bytes.push(0x01);
            write_index(*bits, bytes)?;
        }
        BinaryType::Float(float) => bytes.push(match float {
            FloatType::F16 => 0x02,
            FloatType::Bf16 => 0x03,
            FloatType::F32 => 0x04,
            FloatType::F64 => 0x05,
            FloatType::F128 => 0x06,
            FloatType::F8E4M3 => 0x07,
            FloatType::F8E5M2 => 0x08,
            FloatType::F6E2M3 => 0x09,
            FloatType::F6E3M2 => 0x0a,
            FloatType::F4E2M1 => 0x0b,
            FloatType::E8M0 => 0x0c,
        }),
        BinaryType::Pointer(space) => {
            bytes.push(0x10);
            bytes.push(*space);
        }
        BinaryType::Address { pointee, space } => {
            bytes.push(0x11);
            write_index(*pointee, bytes)?;
            bytes.push(*space);
        }
        BinaryType::Unit => bytes.push(0x12),
        BinaryType::Never => bytes.push(0x13),
        BinaryType::FixedVector { lanes, element } => {
            bytes.push(0x20);
            write_index(*lanes, bytes)?;
            write_index(*element, bytes)?;
        }
        BinaryType::ScalableVector { element } => {
            bytes.push(0x21);
            write_index(*element, bytes)?;
        }
        BinaryType::Tuple(elements) => {
            bytes.push(0x22);
            write_count(elements.len(), bytes)?;
            for element in elements {
                write_index(*element, bytes)?;
            }
        }
        BinaryType::Array { count, element } => {
            bytes.push(0x23);
            write_index(*count, bytes)?;
            write_index(*element, bytes)?;
        }
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            bytes.push(0x24);
            write_index(*declaration, bytes)?;
            write_count(arguments.len(), bytes)?;
            for argument in arguments {
                write_index(*argument, bytes)?;
            }
        }
        BinaryType::Generic { parameter } => {
            bytes.push(0x25);
            write_index(*parameter, bytes)?;
        }
        BinaryType::Reference { pointee } => encode_unary_type(0x26, *pointee, bytes)?,
        BinaryType::Unique { pointee } => encode_unary_type(0x27, *pointee, bytes)?,
        BinaryType::Weak { pointee } => encode_unary_type(0x28, *pointee, bytes)?,
        BinaryType::Function(function) => {
            bytes.push(0x30);
            encode_function_type(function, bytes)?;
        }
        BinaryType::Closure {
            function_type,
            borrowed,
        } => {
            bytes.push(0x31);
            write_index(*function_type, bytes)?;
            bytes.push(u8::from(*borrowed));
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            bytes.push(0x32);
            write_index(*yield_type, bytes)?;
            write_index(*resume_type, bytes)?;
            write_index(*result_type, bytes)?;
        }
        BinaryType::Frame { function_index } => {
            bytes.push(0x33);
            write_index(*function_index, bytes)?;
        }
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => {
            bytes.push(0x40);
            bytes.push(*dimension);
            write_index(*component, bytes)?;
            bytes.push(*access);
            bytes.push(u8::from(*multisampled));
        }
        BinaryType::Sampler => bytes.push(0x41),
        BinaryType::AccelerationStructure => bytes.push(0x42),
        BinaryType::VisibleFunctionTable { function_type } => {
            bytes.push(0x43);
            write_index(*function_type, bytes)?;
        }
        BinaryType::Tensor { rank, element } => {
            bytes.push(0x44);
            write_index(*rank, bytes)?;
            write_index(*element, bytes)?;
        }
        BinaryType::RayQuery => bytes.push(0x45),
        BinaryType::HitObject => bytes.push(0x46),
        BinaryType::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => {
            bytes.push(0x47);
            bytes.push(*scope);
            write_index(*rows, bytes)?;
            write_index(*columns, bytes)?;
            write_index(*element, bytes)?;
            bytes.push(*use_kind);
        }
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => {
            bytes.push(0x48);
            bytes.push(*kind);
            write_index(*root, bytes)?;
            write_index(*immediate, bytes)?;
        }
    }
    Ok(())
}

fn encode_function_type(
    function: &BinaryFunctionType,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(function.parameters.len(), bytes)?;
    for parameter in &function.parameters {
        encode_parameter(*parameter, bytes)?;
    }
    write_index(function.result, bytes)?;
    write_index(
        function
            .error
            .map_or(Some(0), |index| index.checked_add(1))
            .ok_or_else(|| FormatError::malformed(None, "function error type index overflows"))?,
        bytes,
    )?;
    bytes.push(
        u8::from(function.asynchronous)
            | (u8::from(function.c_abi) << 1)
            | (u8::from(function.foreign) << 2),
    );
    Ok(())
}

fn encode_parameter(
    parameter: BinaryFunctionParameter,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if parameter.convention > 3 {
        return Err(FormatError::malformed(
            None,
            "invalid function parameter convention",
        ));
    }
    bytes.push(parameter.convention | (u8::from(parameter.context) << 7));
    write_index(parameter.ty, bytes)
}

fn encode_unary_type(tag: u8, pointee: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    bytes.push(tag);
    write_index(pointee, bytes)
}

fn encode_definition(
    definition: &BinaryNominalDefinition,
    declarations: &[BinaryNominalDeclaration],
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let declaration_index = usize::try_from(definition.declaration)
        .map_err(|_| FormatError::malformed(None, "nominal declaration index overflows"))?;
    let declaration = declarations
        .get(declaration_index)
        .ok_or_else(|| FormatError::malformed(None, "nominal declaration index is invalid"))?;
    write_index(definition.declaration, bytes)?;
    encode_layout(definition.layout, bytes)?;
    match declaration.kind {
        BinaryNominalKind::Struct | BinaryNominalKind::Explicit => {
            if !definition.variants.is_empty() {
                return Err(FormatError::malformed(
                    None,
                    "struct definition cannot contain enum variants",
                ));
            }
            encode_fields(&definition.fields, strings, bytes)?;
        }
        BinaryNominalKind::Enum => {
            if !definition.fields.is_empty() {
                return Err(FormatError::malformed(
                    None,
                    "enum definition cannot contain fields",
                ));
            }
            write_count(definition.variants.len(), bytes)?;
            for variant in &definition.variants {
                write_string(&variant.name, strings, bytes)?;
                write_count(variant.payload.len(), bytes)?;
                for ty in &variant.payload {
                    write_index(*ty, bytes)?;
                }
            }
        }
    }
    encode_policy(&definition.policy, bytes)
}

fn encode_layout(layout: BinaryLayout, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    match layout {
        BinaryLayout::Ridl => bytes.push(0),
        BinaryLayout::C => bytes.push(1),
        BinaryLayout::Packed(alignment) => {
            bytes.push(2);
            write_index(alignment, bytes)?;
        }
        BinaryLayout::Explicit { size, align } => {
            bytes.push(3);
            write_index(size, bytes)?;
            write_index(align, bytes)?;
        }
    }
    Ok(())
}

fn encode_fields(
    fields: &[BinaryField],
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(fields.len(), bytes)?;
    for field in fields {
        write_string(&field.name, strings, bytes)?;
        write_index(field.ty, bytes)?;
        let bitfield = field.bit_offset.is_some() || field.bit_width.is_some();
        if bitfield != (field.bit_offset.is_some() && field.bit_width.is_some())
            || (bitfield && field.offset.is_none())
        {
            return Err(FormatError::malformed(None, "invalid bitfield offsets"));
        }
        bytes.push(u8::from(field.offset.is_some()) | (u8::from(bitfield) << 1));
        if let Some(offset) = field.offset {
            write_index(offset, bytes)?;
        }
        if bitfield {
            write_index(field.bit_offset.unwrap_or_default(), bytes)?;
            write_index(field.bit_width.unwrap_or_default(), bytes)?;
        }
        write_count(field.annotations.len(), bytes)?;
        for annotation in &field.annotations {
            write_string(&annotation.name, strings, bytes)?;
            write_count(annotation.arguments.len(), bytes)?;
            for argument in &annotation.arguments {
                write_string(argument, strings, bytes)?;
            }
        }
    }
    Ok(())
}

fn encode_policy(policy: &BinaryPolicy, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    match policy.copy {
        BinaryCopyPolicy::Trivial => bytes.push(0),
        BinaryCopyPolicy::Implicit(function) => {
            bytes.push(1);
            write_index(function, bytes)?;
        }
        BinaryCopyPolicy::Explicit(function) => {
            bytes.push(2);
            write_index(function, bytes)?;
        }
        BinaryCopyPolicy::None => bytes.push(3),
    }
    match policy.destroy {
        BinaryDestroyPolicy::Trivial => bytes.push(0),
        BinaryDestroyPolicy::Function(function) => {
            bytes.push(1);
            write_index(function, bytes)?;
        }
    }
    match policy.relocate {
        BinaryRelocatePolicy::Bitwise => bytes.push(0),
        BinaryRelocatePolicy::Function(function) => {
            bytes.push(1);
            write_index(function, bytes)?;
        }
    }
    bytes.extend_from_slice(&[policy.exclusivity, policy.pairing, policy.lifetime]);
    Ok(())
}

fn write_string(
    value: &str,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| FormatError::malformed(None, "TYPES string is absent from STRINGS"))?;
    let index = u64::try_from(index)
        .map_err(|_| FormatError::malformed(None, "TYPES string index overflows"))?;
    write_index(index, bytes)
}

fn write_count(count: usize, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    let count =
        u64::try_from(count).map_err(|_| FormatError::malformed(None, "TYPES count overflows"))?;
    write_index(count, bytes)
}

fn write_index(index: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes)
        .map_err(|error| FormatError::malformed(None, format!("TYPES encoding failed: {error}")))
}
