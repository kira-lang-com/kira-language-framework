use crate::leb::write_uleb;
use crate::types::{
    BinaryCopyPolicy, BinaryDestroyPolicy, BinaryField, BinaryLayout, BinaryNominalKind,
    BinaryPolicy, BinaryRelocatePolicy, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable, StringTableBuilder};
use tir_types::FloatType;

impl BinaryTypeTable {
    /// Adds TYPES strings in their section traversal order to the canonical table.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        for declaration in &self.declarations {
            intern_nonempty(strings, &declaration.name, "nominal declaration")?;
            for parameter in &declaration.generic_parameters {
                intern_nonempty(strings, &parameter.name, "generic parameter")?;
                for bound in &parameter.bounds {
                    intern_nonempty(strings, bound, "generic bound")?;
                }
            }
        }
        for definition in &self.definitions {
            for field in &definition.fields {
                intern_nonempty(strings, &field.name, "field")?;
                for annotation in &field.annotations {
                    intern_nonempty(strings, &annotation.name, "field annotation")?;
                    for argument in &annotation.arguments {
                        intern_nonempty(strings, argument, "field annotation argument")?;
                    }
                }
            }
            for variant in &definition.variants {
                intern_nonempty(strings, &variant.name, "enum variant")?;
            }
        }
        Ok(())
    }

    /// Encodes declarations, topologically ordered entries, and definitions.
    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, FormatError> {
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.declarations.len(), "nominal declaration")?;
        for declaration in &self.declarations {
            write_string(
                &mut bytes,
                strings,
                &declaration.name,
                "nominal declaration",
            )?;
            write_count(
                &mut bytes,
                declaration.generic_parameters.len(),
                "generic parameter",
            )?;
            for parameter in &declaration.generic_parameters {
                write_string(&mut bytes, strings, &parameter.name, "generic parameter")?;
                write_count(&mut bytes, parameter.bounds.len(), "generic bound")?;
                for bound in &parameter.bounds {
                    write_string(&mut bytes, strings, bound, "generic bound")?;
                }
            }
            bytes.push(match declaration.kind {
                BinaryNominalKind::Struct => 0,
                BinaryNominalKind::Enum => 1,
                BinaryNominalKind::Explicit => 2,
            });
        }

        write_count(&mut bytes, self.entries.len(), "type entry")?;
        for entry in &self.entries {
            write_type_entry(&mut bytes, entry)?;
        }

        write_count(&mut bytes, self.definitions.len(), "nominal definition")?;
        for definition in &self.definitions {
            let declaration = usize::try_from(definition.declaration)
                .ok()
                .and_then(|index| self.declarations.get(index))
                .ok_or_else(|| malformed("nominal definition declaration is out of range"))?;
            write_uleb(definition.declaration, &mut bytes).map_err(io_error)?;
            write_layout(&mut bytes, definition.layout)?;
            match declaration.kind {
                BinaryNominalKind::Struct | BinaryNominalKind::Explicit => {
                    if !definition.variants.is_empty() {
                        return Err(malformed("struct definition contains enum variants"));
                    }
                    write_fields(&mut bytes, strings, &definition.fields)?;
                }
                BinaryNominalKind::Enum => {
                    if !definition.fields.is_empty() {
                        return Err(malformed("enum definition contains fields"));
                    }
                    write_variants(&mut bytes, strings, &definition.variants)?;
                }
            }
            write_policy(&mut bytes, &definition.policy)?;
        }

        super::decode_type_table(&bytes, strings)?;
        Ok(bytes)
    }
}

fn write_type_entry(bytes: &mut Vec<u8>, ty: &BinaryType) -> Result<(), FormatError> {
    match ty {
        BinaryType::Integer(bits) => {
            bytes.push(0x01);
            write_uleb(*bits, bytes).map_err(io_error)?;
        }
        BinaryType::Float(float) => bytes.push(float_tag(*float)),
        BinaryType::Pointer(space) => {
            bytes.push(0x10);
            bytes.push(*space);
        }
        BinaryType::Address { pointee, space } => {
            bytes.push(0x11);
            write_uleb(*pointee, bytes).map_err(io_error)?;
            bytes.push(*space);
        }
        BinaryType::Unit => bytes.push(0x12),
        BinaryType::Never => bytes.push(0x13),
        BinaryType::FixedVector { lanes, element } => {
            bytes.push(0x20);
            write_uleb(*lanes, bytes).map_err(io_error)?;
            write_uleb(*element, bytes).map_err(io_error)?;
        }
        BinaryType::ScalableVector { element } => {
            bytes.push(0x21);
            write_uleb(*element, bytes).map_err(io_error)?;
        }
        BinaryType::Tuple(elements) => {
            bytes.push(0x22);
            write_count(bytes, elements.len(), "tuple element")?;
            write_indices(bytes, elements)?;
        }
        BinaryType::Array { count, element } => {
            bytes.push(0x23);
            write_uleb(*count, bytes).map_err(io_error)?;
            write_uleb(*element, bytes).map_err(io_error)?;
        }
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            bytes.push(0x24);
            write_uleb(*declaration, bytes).map_err(io_error)?;
            write_count(bytes, arguments.len(), "nominal type argument")?;
            write_indices(bytes, arguments)?;
        }
        BinaryType::Generic { parameter } => {
            bytes.push(0x25);
            write_uleb(*parameter, bytes).map_err(io_error)?;
        }
        BinaryType::Reference { pointee } => write_unary_type(bytes, 0x26, *pointee)?,
        BinaryType::Unique { pointee } => write_unary_type(bytes, 0x27, *pointee)?,
        BinaryType::Weak { pointee } => write_unary_type(bytes, 0x28, *pointee)?,
        BinaryType::Function(function) => {
            bytes.push(0x30);
            write_count(bytes, function.parameters.len(), "function parameter")?;
            for parameter in &function.parameters {
                bytes.push(parameter.convention | (u8::from(parameter.context) << 7));
                write_uleb(parameter.ty, bytes).map_err(io_error)?;
            }
            write_uleb(function.result, bytes).map_err(io_error)?;
            let encoded_error = function
                .error
                .map_or(Some(0), |index| index.checked_add(1))
                .ok_or_else(|| malformed("function error type index overflows"))?;
            write_uleb(encoded_error, bytes).map_err(io_error)?;
            bytes.push(
                u8::from(function.asynchronous)
                    | (u8::from(function.c_abi) << 1)
                    | (u8::from(function.foreign) << 2),
            );
        }
        BinaryType::Closure {
            function_type,
            borrowed,
        } => {
            bytes.push(0x31);
            write_uleb(*function_type, bytes).map_err(io_error)?;
            bytes.push(u8::from(*borrowed));
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            bytes.push(0x32);
            write_uleb(*yield_type, bytes).map_err(io_error)?;
            write_uleb(*resume_type, bytes).map_err(io_error)?;
            write_uleb(*result_type, bytes).map_err(io_error)?;
        }
        BinaryType::Frame { function_index } => {
            bytes.push(0x33);
            write_uleb(*function_index, bytes).map_err(io_error)?;
        }
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => {
            bytes.push(0x40);
            bytes.push(*dimension);
            write_uleb(*component, bytes).map_err(io_error)?;
            bytes.push(*access);
            bytes.push(u8::from(*multisampled));
        }
        BinaryType::Sampler => bytes.push(0x41),
        BinaryType::AccelerationStructure => bytes.push(0x42),
        BinaryType::VisibleFunctionTable { function_type } => {
            bytes.push(0x43);
            write_uleb(*function_type, bytes).map_err(io_error)?;
        }
        BinaryType::Tensor { rank, element } => {
            bytes.push(0x44);
            write_uleb(*rank, bytes).map_err(io_error)?;
            write_uleb(*element, bytes).map_err(io_error)?;
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
            write_uleb(*rows, bytes).map_err(io_error)?;
            write_uleb(*columns, bytes).map_err(io_error)?;
            write_uleb(*element, bytes).map_err(io_error)?;
            bytes.push(*use_kind);
        }
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => {
            bytes.push(0x48);
            bytes.push(*kind);
            write_uleb(*root, bytes).map_err(io_error)?;
            write_uleb(*immediate, bytes).map_err(io_error)?;
        }
    }
    Ok(())
}

fn write_unary_type(bytes: &mut Vec<u8>, tag: u8, pointee: u64) -> Result<(), FormatError> {
    bytes.push(tag);
    write_uleb(pointee, bytes).map_err(io_error)
}

fn write_indices(bytes: &mut Vec<u8>, indices: &[u64]) -> Result<(), FormatError> {
    for index in indices {
        write_uleb(*index, bytes).map_err(io_error)?;
    }
    Ok(())
}

fn float_tag(float: FloatType) -> u8 {
    match float {
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
    }
}

fn write_layout(bytes: &mut Vec<u8>, layout: BinaryLayout) -> Result<(), FormatError> {
    match layout {
        BinaryLayout::Ridl => bytes.push(0),
        BinaryLayout::C => bytes.push(1),
        BinaryLayout::Packed(alignment) => {
            bytes.push(2);
            write_uleb(alignment, bytes).map_err(io_error)?;
        }
        BinaryLayout::Explicit { size, align } => {
            bytes.push(3);
            write_uleb(size, bytes).map_err(io_error)?;
            write_uleb(align, bytes).map_err(io_error)?;
        }
    }
    Ok(())
}

fn write_fields(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    fields: &[BinaryField],
) -> Result<(), FormatError> {
    write_count(bytes, fields.len(), "field")?;
    for field in fields {
        write_string(bytes, strings, &field.name, "field")?;
        write_uleb(field.ty, bytes).map_err(io_error)?;
        if field.bit_offset.is_some() != field.bit_width.is_some() {
            return Err(malformed("field bit offset and width must appear together"));
        }
        if field.bit_offset.is_some() && field.offset.is_none() {
            return Err(malformed("bitfield is missing its byte offset"));
        }
        let flags = u8::from(field.offset.is_some()) | (u8::from(field.bit_offset.is_some()) << 1);
        bytes.push(flags);
        if let Some(offset) = field.offset {
            write_uleb(offset, bytes).map_err(io_error)?;
        }
        if let (Some(bit_offset), Some(bit_width)) = (field.bit_offset, field.bit_width) {
            write_uleb(bit_offset, bytes).map_err(io_error)?;
            write_uleb(bit_width, bytes).map_err(io_error)?;
        }
        write_count(bytes, field.annotations.len(), "field annotation")?;
        for annotation in &field.annotations {
            write_string(bytes, strings, &annotation.name, "field annotation")?;
            write_count(
                bytes,
                annotation.arguments.len(),
                "field annotation argument",
            )?;
            for argument in &annotation.arguments {
                write_string(bytes, strings, argument, "field annotation argument")?;
            }
        }
    }
    Ok(())
}

fn write_variants(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    variants: &[crate::types::BinaryVariant],
) -> Result<(), FormatError> {
    write_count(bytes, variants.len(), "enum variant")?;
    for variant in variants {
        write_string(bytes, strings, &variant.name, "enum variant")?;
        write_count(bytes, variant.payload.len(), "enum payload")?;
        write_indices(bytes, &variant.payload)?;
    }
    Ok(())
}

fn write_policy(bytes: &mut Vec<u8>, policy: &BinaryPolicy) -> Result<(), FormatError> {
    match policy.copy {
        BinaryCopyPolicy::Trivial => bytes.push(0),
        BinaryCopyPolicy::Implicit(function) => {
            bytes.push(1);
            write_uleb(function, bytes).map_err(io_error)?;
        }
        BinaryCopyPolicy::Explicit(function) => {
            bytes.push(2);
            write_uleb(function, bytes).map_err(io_error)?;
        }
        BinaryCopyPolicy::None => bytes.push(3),
    }
    match policy.destroy {
        BinaryDestroyPolicy::Trivial => bytes.push(0),
        BinaryDestroyPolicy::Function(function) => {
            bytes.push(1);
            write_uleb(function, bytes).map_err(io_error)?;
        }
    }
    match policy.relocate {
        BinaryRelocatePolicy::Bitwise => bytes.push(0),
        BinaryRelocatePolicy::Function(function) => {
            bytes.push(1);
            write_uleb(function, bytes).map_err(io_error)?;
        }
    }
    bytes.extend_from_slice(&[policy.exclusivity, policy.pairing, policy.lifetime]);
    Ok(())
}

fn write_string(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
    label: &str,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(format!("{label} name is empty")));
    }
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed(format!("{label} string is missing from STRINGS")))?;
    write_uleb(index, bytes).map_err(io_error)
}

fn intern_nonempty(
    strings: &mut StringTableBuilder,
    value: &str,
    label: &str,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(format!("{label} name is empty")));
    }
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| malformed(error.message))
}

fn write_count(bytes: &mut Vec<u8>, count: usize, label: &str) -> Result<(), FormatError> {
    let count =
        u64::try_from(count).map_err(|_| malformed(format!("{label} count overflows ULEB128")))?;
    write_uleb(count, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(error.to_string())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
