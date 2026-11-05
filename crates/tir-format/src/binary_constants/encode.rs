use crate::leb::write_uleb;
use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryNominalKind, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable, StringTableBuilder};

const MAX_VALUE_DEPTH: usize = 256;

impl BinaryConstantTable {
    /// Adds constant strings to STRINGS in their first-reference order.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        for record in &self.records {
            intern_value_strings(&record.value, strings)?;
        }
        Ok(())
    }

    /// Encodes CONSTANTS and validates the result against its type table.
    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
    ) -> Result<Vec<u8>, FormatError> {
        if self.records.len() as u64 >= 1 << 32 {
            return Err(malformed("CONSTANTS count must be less than 2^32"));
        }
        let mut encoder = ConstantEncoder {
            bytes: Vec::new(),
            strings,
            types,
        };
        write_count(&mut encoder.bytes, self.records.len())?;
        for record in &self.records {
            write_uleb(record.ty, &mut encoder.bytes).map_err(io_error)?;
            encoder.write_value(
                TypeUse {
                    index: record.ty,
                    bindings: Vec::new(),
                },
                &record.value,
                0,
            )?;
        }
        validate_encoded(&encoder.bytes, strings, types)?;
        Ok(encoder.bytes)
    }
}

fn intern_value_strings(
    value: &BinaryConstantValue,
    strings: &mut StringTableBuilder,
) -> Result<(), FormatError> {
    match value {
        BinaryConstantValue::String(value) => {
            strings
                .intern(value)
                .map_err(|error| malformed(error.message))?;
        }
        BinaryConstantValue::Tuple(values)
        | BinaryConstantValue::Array(values)
        | BinaryConstantValue::Vector(values)
        | BinaryConstantValue::Struct(values)
        | BinaryConstantValue::Enum {
            payload: values, ..
        } => {
            for value in values {
                intern_value_strings(value, strings)?;
            }
        }
        BinaryConstantValue::Union { value, .. } => intern_value_strings(value, strings)?,
        BinaryConstantValue::Integer(_)
        | BinaryConstantValue::Float(_)
        | BinaryConstantValue::Zero
        | BinaryConstantValue::Bytes(_)
        | BinaryConstantValue::Null
        | BinaryConstantValue::Sampler(_) => {}
    }
    Ok(())
}

#[derive(Clone)]
struct TypeUse {
    index: u64,
    bindings: Vec<TypeUse>,
}

struct ConstantEncoder<'a> {
    bytes: Vec<u8>,
    strings: &'a StringTable,
    types: &'a BinaryTypeTable,
}

impl ConstantEncoder<'_> {
    fn write_value(
        &mut self,
        ty: TypeUse,
        value: &BinaryConstantValue,
        depth: usize,
    ) -> Result<(), FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(malformed("constant nesting exceeds 255"));
        }
        let ty = self.resolve(ty, 0)?;
        let entry = self.type_entry(ty.index)?.clone();
        match value {
            BinaryConstantValue::Integer(bytes) => {
                let BinaryType::Integer(bits) = entry else {
                    return Err(malformed("integer constant requires an integer type"));
                };
                self.bytes.push(0);
                write_integer_bits(bytes, bits, &mut self.bytes)?;
            }
            BinaryConstantValue::Float(bytes) => {
                let BinaryType::Float(float) = entry else {
                    return Err(malformed("floating constant requires a floating type"));
                };
                let expected = usize::from(float.bits().div_ceil(8));
                if bytes.len() != expected {
                    return Err(malformed("floating constant has the wrong byte width"));
                }
                if float.bits() % 8 != 0
                    && bytes
                        .last()
                        .is_some_and(|byte| byte >> (float.bits() % 8) != 0)
                {
                    return Err(malformed("floating constant has nonzero padding bits"));
                }
                self.bytes.push(0);
                self.bytes.extend_from_slice(bytes);
            }
            BinaryConstantValue::Zero => self.bytes.push(1),
            BinaryConstantValue::Bytes(bytes) => {
                self.bytes.push(2);
                write_count(&mut self.bytes, bytes.len())?;
                self.bytes.extend_from_slice(bytes);
            }
            BinaryConstantValue::String(value) => {
                self.bytes.push(3);
                let index = self
                    .strings
                    .index_of(value)
                    .ok_or_else(|| malformed("string constant is missing from STRINGS"))?;
                write_uleb(index, &mut self.bytes).map_err(io_error)?;
            }
            BinaryConstantValue::Tuple(values) => {
                let BinaryType::Tuple(elements) = entry else {
                    return Err(malformed("tuple constant does not have tuple type"));
                };
                self.write_aggregate(values, &elements, &ty.bindings, depth)?;
            }
            BinaryConstantValue::Array(values) => {
                let BinaryType::Array { count, element } = entry else {
                    return Err(malformed("array constant does not have array type"));
                };
                self.write_repeated_aggregate(values, element, count, &ty.bindings, depth)?;
            }
            BinaryConstantValue::Vector(values) => {
                let BinaryType::FixedVector { lanes, element } = entry else {
                    return Err(malformed("vector constant does not have fixed-vector type"));
                };
                self.write_repeated_aggregate(values, element, lanes, &ty.bindings, depth)?;
            }
            BinaryConstantValue::Struct(values) => {
                let BinaryType::Nominal {
                    declaration,
                    arguments,
                } = entry
                else {
                    return Err(malformed("struct constant requires a nominal struct type"));
                };
                if self.nominal_kind(declaration)? != BinaryNominalKind::Struct {
                    return Err(malformed("struct constant requires a nominal struct type"));
                }
                let definition = self.definition(declaration)?;
                let field_types = definition
                    .fields
                    .iter()
                    .map(|field| field.ty)
                    .collect::<Vec<_>>();
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.write_aggregate(values, &field_types, &bindings, depth)?;
            }
            BinaryConstantValue::Union { member, value } => {
                let BinaryType::Nominal {
                    declaration,
                    arguments,
                } = entry
                else {
                    return Err(malformed("union constant requires a nominal explicit type"));
                };
                if self.nominal_kind(declaration)? != BinaryNominalKind::Explicit {
                    return Err(malformed("union constant requires a nominal explicit type"));
                }
                let field_type = self
                    .definition(declaration)?
                    .fields
                    .get(to_usize(*member, "union member index")?)
                    .map(|field| field.ty)
                    .ok_or_else(|| malformed("union member index is out of range"))?;
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.bytes.push(8);
                write_uleb(*member, &mut self.bytes).map_err(io_error)?;
                self.write_value(
                    TypeUse {
                        index: field_type,
                        bindings,
                    },
                    value,
                    depth + 1,
                )?;
            }
            BinaryConstantValue::Enum { variant, payload } => {
                let BinaryType::Nominal {
                    declaration,
                    arguments,
                } = entry
                else {
                    return Err(malformed("enum constant requires a nominal enum type"));
                };
                if self.nominal_kind(declaration)? != BinaryNominalKind::Enum {
                    return Err(malformed("enum constant requires a nominal enum type"));
                }
                let variant_record = self
                    .definition(declaration)?
                    .variants
                    .get(to_usize(*variant, "enum variant index")?)
                    .ok_or_else(|| malformed("enum variant index is out of range"))?;
                let payload_types = variant_record.payload.clone();
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.bytes.push(5);
                write_uleb(*variant, &mut self.bytes).map_err(io_error)?;
                if payload.len() != payload_types.len() {
                    return Err(malformed(
                        "enum constant payload count does not match its variant",
                    ));
                }
                for (type_index, value) in payload_types.iter().zip(payload) {
                    self.write_value(
                        TypeUse {
                            index: *type_index,
                            bindings: bindings.clone(),
                        },
                        value,
                        depth + 1,
                    )?;
                }
            }
            BinaryConstantValue::Null => {
                if !matches!(entry, BinaryType::Pointer(_)) {
                    return Err(malformed("null constant requires a pointer type"));
                }
                self.bytes.push(6);
            }
            BinaryConstantValue::Sampler(descriptor) => {
                if !matches!(entry, BinaryType::Sampler) {
                    return Err(malformed("sampler constant requires sampler type"));
                }
                self.bytes.push(7);
                self.bytes.extend_from_slice(descriptor);
            }
        }
        Ok(())
    }

    fn write_aggregate(
        &mut self,
        values: &[BinaryConstantValue],
        types: &[u64],
        bindings: &[TypeUse],
        depth: usize,
    ) -> Result<(), FormatError> {
        if values.len() != types.len() {
            return Err(malformed(
                "aggregate constant element count does not match its type",
            ));
        }
        self.bytes.push(4);
        for (type_index, value) in types.iter().zip(values) {
            self.write_value(
                TypeUse {
                    index: *type_index,
                    bindings: bindings.to_vec(),
                },
                value,
                depth + 1,
            )?;
        }
        Ok(())
    }

    fn write_repeated_aggregate(
        &mut self,
        values: &[BinaryConstantValue],
        element: u64,
        count: u64,
        bindings: &[TypeUse],
        depth: usize,
    ) -> Result<(), FormatError> {
        if to_usize(count, "aggregate element count")? != values.len() {
            return Err(malformed(
                "aggregate constant element count does not match its type",
            ));
        }
        self.bytes.push(4);
        for value in values {
            self.write_value(
                TypeUse {
                    index: element,
                    bindings: bindings.to_vec(),
                },
                value,
                depth + 1,
            )?;
        }
        Ok(())
    }

    fn resolve(&self, ty: TypeUse, depth: usize) -> Result<TypeUse, FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(malformed("generic constant type substitution is recursive"));
        }
        if let BinaryType::Generic { parameter } = self.type_entry(ty.index)? {
            let binding = ty
                .bindings
                .get(to_usize(*parameter, "generic parameter index")?)
                .cloned()
                .ok_or_else(|| malformed("constant contains an unsubstituted generic type"))?;
            self.resolve(binding, depth + 1)
        } else {
            Ok(ty)
        }
    }

    fn make_bindings(
        &self,
        arguments: &[u64],
        outer: &[TypeUse],
    ) -> Result<Vec<TypeUse>, FormatError> {
        arguments
            .iter()
            .map(|index| {
                self.resolve(
                    TypeUse {
                        index: *index,
                        bindings: outer.to_vec(),
                    },
                    0,
                )
            })
            .collect()
    }

    fn type_entry(&self, index: u64) -> Result<&BinaryType, FormatError> {
        self.types
            .entries
            .get(to_usize(index, "type index")?)
            .ok_or_else(|| malformed("constant type index is out of range"))
    }

    fn nominal_kind(&self, index: u64) -> Result<BinaryNominalKind, FormatError> {
        self.types
            .declarations
            .get(to_usize(index, "nominal declaration index")?)
            .map(|declaration| declaration.kind)
            .ok_or_else(|| malformed("nominal declaration index is out of range"))
    }

    fn definition(
        &self,
        index: u64,
    ) -> Result<&crate::types::BinaryNominalDefinition, FormatError> {
        self.types
            .definitions
            .iter()
            .find(|definition| definition.declaration == index)
            .ok_or_else(|| malformed("constant uses an incomplete nominal type"))
    }
}

fn validate_encoded(
    bytes: &[u8],
    strings: &StringTable,
    types: &BinaryTypeTable,
) -> Result<(), FormatError> {
    let mut decoder = super::ConstantDecoder::new(bytes, strings, types);
    let count = usize::try_from(
        crate::leb::read_uleb(bytes, &mut decoder.cursor)
            .map_err(|error| malformed(format!("invalid CONSTANTS ULEB: {error:?}")))?,
    )
    .map_err(|_| malformed("constant count overflows memory"))?;
    if count > bytes.len().saturating_sub(decoder.cursor) / 2 {
        return Err(malformed("constant count exceeds encoded data"));
    }
    for _ in 0..count {
        let ty = crate::leb::read_uleb(bytes, &mut decoder.cursor)
            .map_err(|error| malformed(format!("invalid CONSTANTS ULEB: {error:?}")))?;
        decoder.read_value(ty, 0)?;
    }
    if decoder.cursor != bytes.len() {
        return Err(malformed("trailing bytes in CONSTANTS encoding"));
    }
    Ok(())
}

fn write_integer_bits(bytes: &[u8], width: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let width = usize::try_from(width).map_err(|_| malformed("integer width overflows memory"))?;
    if bytes.len() != width.div_ceil(8) {
        return Err(malformed("integer constant has the wrong byte width"));
    }
    if width % 8 != 0 && bytes.last().is_some_and(|byte| byte >> (width % 8) != 0) {
        return Err(malformed("integer constant exceeds its type width"));
    }
    let groups = width.div_ceil(7);
    let group_value = |group: usize| {
        (0..7).fold(0_u8, |value, bit| {
            let position = group * 7 + bit;
            if position < width && bytes[position / 8] & (1 << (position % 8)) != 0 {
                value | (1 << bit)
            } else {
                value
            }
        })
    };
    let mut last = groups.saturating_sub(1);
    while last > 0 && group_value(last) == 0 {
        last -= 1;
    }
    for group in 0..=last {
        let mut value = group_value(group);
        if group != last {
            value |= 0x80;
        }
        output.push(value);
    }
    Ok(())
}

fn write_count(output: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    write_uleb(
        u64::try_from(count).map_err(|_| malformed("constant count overflows ULEB128"))?,
        output,
    )
    .map_err(io_error)
}

fn to_usize(value: u64, label: &str) -> Result<usize, FormatError> {
    usize::try_from(value).map_err(|_| malformed(format!("{label} overflows memory")))
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("CONSTANTS encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
