use crate::leb::write_uleb;
use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryNominalKind, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable};

const MAX_VALUE_DEPTH: usize = 256;

#[derive(Clone)]
struct TypeUse {
    index: u64,
    bindings: Vec<TypeUse>,
}

pub(super) fn encode_constant_table(
    table: &BinaryConstantTable,
    strings: &StringTable,
    types: &BinaryTypeTable,
) -> Result<Vec<u8>, FormatError> {
    let count = u64::try_from(table.records.len())
        .map_err(|_| FormatError::malformed(None, "CONSTANTS count overflows"))?;
    if count >= 1 << 32 {
        return Err(FormatError::malformed(
            None,
            "CONSTANTS count must be less than 2^32",
        ));
    }
    let mut bytes = Vec::new();
    write_index(count, &mut bytes)?;
    {
        let mut encoder = ConstantEncoder {
            bytes: &mut bytes,
            strings,
            types,
        };
        for constant in &table.records {
            encoder.write_index(constant.ty)?;
            encoder.write_value(
                TypeUse {
                    index: constant.ty,
                    bindings: Vec::new(),
                },
                &constant.value,
                0,
            )?;
        }
    }
    validate_encoded(&bytes, strings, types)?;
    Ok(bytes)
}

struct ConstantEncoder<'a> {
    bytes: &'a mut Vec<u8>,
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
            return Err(FormatError::malformed(None, "constant nesting exceeds 255"));
        }
        let ty = self.resolve(ty, 0)?;
        let entry = self.type_entry(ty.index)?.clone();
        match (entry, value) {
            (BinaryType::Integer(bits), BinaryConstantValue::Integer(raw)) => {
                self.bytes.push(0);
                write_integer(raw, bits, self.bytes)?;
            }
            (BinaryType::Float(float), BinaryConstantValue::Float(raw)) => {
                let expected = usize::try_from(float.bits().div_ceil(8))
                    .map_err(|_| FormatError::malformed(None, "float width overflows memory"))?;
                if raw.len() != expected {
                    return Err(FormatError::malformed(
                        None,
                        "floating constant has the wrong byte width",
                    ));
                }
                self.bytes.push(0);
                self.bytes.extend_from_slice(raw);
            }
            (_, BinaryConstantValue::Zero) => self.bytes.push(1),
            (_, BinaryConstantValue::Bytes(raw)) => {
                self.bytes.push(2);
                self.write_count(raw.len())?;
                self.bytes.extend_from_slice(raw);
            }
            (entry, BinaryConstantValue::String(value)) if self.is_string_tuple(&ty)? => {
                let _ = entry;
                self.bytes.push(3);
                let index = self.strings.index_of(value).ok_or_else(|| {
                    FormatError::malformed(None, "string constant is absent from STRINGS")
                })?;
                self.write_index(u64::try_from(index).map_err(|_| {
                    FormatError::malformed(None, "string constant index overflows")
                })?)?;
            }
            (BinaryType::Tuple(elements), BinaryConstantValue::Tuple(values)) => {
                self.bytes.push(4);
                self.write_children(&elements, values, &ty.bindings, depth + 1)?;
            }
            (BinaryType::Array { count, element }, BinaryConstantValue::Array(values)) => {
                let expected = usize::try_from(count)
                    .map_err(|_| FormatError::malformed(None, "array constant count overflows"))?;
                if expected != values.len() {
                    return Err(FormatError::malformed(
                        None,
                        "array constant element count does not match its type",
                    ));
                }
                self.bytes.push(4);
                for value in values {
                    self.write_value(self.child_use(element, &ty.bindings), value, depth + 1)?;
                }
            }
            (BinaryType::FixedVector { lanes, element }, BinaryConstantValue::Vector(values)) => {
                let expected = usize::try_from(lanes)
                    .map_err(|_| FormatError::malformed(None, "vector lane count overflows"))?;
                if expected != values.len() {
                    return Err(FormatError::malformed(
                        None,
                        "vector constant lane count does not match its type",
                    ));
                }
                self.bytes.push(4);
                for value in values {
                    self.write_value(self.child_use(element, &ty.bindings), value, depth + 1)?;
                }
            }
            (
                BinaryType::Nominal {
                    declaration,
                    arguments,
                },
                BinaryConstantValue::Struct(values),
            ) => {
                if self.declaration_kind(declaration)? != BinaryNominalKind::Struct {
                    return Err(FormatError::malformed(
                        None,
                        "struct constant requires a struct nominal type",
                    ));
                }
                let definition = self.definition(declaration)?.clone();
                if definition.fields.len() != values.len() {
                    return Err(FormatError::malformed(
                        None,
                        "struct constant field count does not match its type",
                    ));
                }
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.bytes.push(4);
                for (field, value) in definition.fields.iter().zip(values) {
                    self.write_value(self.child_use(field.ty, &bindings), value, depth + 1)?;
                }
            }
            (
                BinaryType::Nominal {
                    declaration,
                    arguments,
                },
                BinaryConstantValue::Enum { variant, payload },
            ) => {
                if self.declaration_kind(declaration)? != BinaryNominalKind::Enum {
                    return Err(FormatError::malformed(
                        None,
                        "enum constant requires an enum nominal type",
                    ));
                }
                let definition = self.definition(declaration)?.clone();
                let variant_index = usize::try_from(*variant)
                    .map_err(|_| FormatError::malformed(None, "enum variant index overflows"))?;
                let record = definition.variants.get(variant_index).ok_or_else(|| {
                    FormatError::malformed(None, "enum variant index is out of range")
                })?;
                if record.payload.len() != payload.len() {
                    return Err(FormatError::malformed(
                        None,
                        "enum payload count does not match its variant",
                    ));
                }
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.bytes.push(5);
                self.write_index(*variant)?;
                for (payload_type, value) in record.payload.iter().zip(payload) {
                    self.write_value(self.child_use(*payload_type, &bindings), value, depth + 1)?;
                }
            }
            (
                BinaryType::Nominal {
                    declaration,
                    arguments,
                },
                BinaryConstantValue::Union { member, value },
            ) => {
                if self.declaration_kind(declaration)? != BinaryNominalKind::Explicit {
                    return Err(FormatError::malformed(
                        None,
                        "union constant requires an explicit nominal type",
                    ));
                }
                let definition = self.definition(declaration)?.clone();
                let member_index = usize::try_from(*member)
                    .map_err(|_| FormatError::malformed(None, "union member index overflows"))?;
                let field = definition.fields.get(member_index).ok_or_else(|| {
                    FormatError::malformed(None, "union member index is out of range")
                })?;
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                self.bytes.push(8);
                self.write_index(*member)?;
                self.write_value(self.child_use(field.ty, &bindings), value, depth + 1)?;
            }
            (BinaryType::Pointer(_), BinaryConstantValue::Null) => self.bytes.push(6),
            (BinaryType::Sampler, BinaryConstantValue::Sampler(descriptor)) => {
                self.bytes.push(7);
                self.bytes.extend_from_slice(descriptor);
            }
            _ => {
                return Err(FormatError::malformed(
                    None,
                    "constant value form does not match its type",
                ));
            }
        }
        Ok(())
    }

    fn write_children(
        &mut self,
        types: &[u64],
        values: &[BinaryConstantValue],
        bindings: &[TypeUse],
        depth: usize,
    ) -> Result<(), FormatError> {
        if types.len() != values.len() {
            return Err(FormatError::malformed(
                None,
                "aggregate constant element count does not match its type",
            ));
        }
        for (ty, value) in types.iter().zip(values) {
            self.write_value(self.child_use(*ty, bindings), value, depth)?;
        }
        Ok(())
    }

    fn resolve(&self, ty: TypeUse, depth: usize) -> Result<TypeUse, FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(FormatError::malformed(
                None,
                "generic constant type substitution is recursive",
            ));
        }
        match self.type_entry(ty.index)? {
            BinaryType::Generic { parameter } => {
                let parameter = usize::try_from(*parameter).map_err(|_| {
                    FormatError::malformed(None, "generic parameter index overflows")
                })?;
                let binding = ty.bindings.get(parameter).cloned().ok_or_else(|| {
                    FormatError::malformed(None, "constant contains an unsubstituted generic type")
                })?;
                self.resolve(binding, depth + 1)
            }
            _ => Ok(ty),
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

    fn is_string_tuple(&self, ty: &TypeUse) -> Result<bool, FormatError> {
        let ty = self.resolve(ty.clone(), 0)?;
        let BinaryType::Tuple(elements) = self.type_entry(ty.index)? else {
            return Ok(false);
        };
        if elements.len() != 2 {
            return Ok(false);
        }
        let pointer = self.resolve(self.child_use(elements[0], &ty.bindings), 0)?;
        let length = self.resolve(self.child_use(elements[1], &ty.bindings), 0)?;
        Ok(
            matches!(self.type_entry(pointer.index)?, BinaryType::Pointer(0))
                && matches!(self.type_entry(length.index)?, BinaryType::Integer(64)),
        )
    }

    fn declaration_kind(&self, declaration: u64) -> Result<BinaryNominalKind, FormatError> {
        let index = usize::try_from(declaration)
            .map_err(|_| FormatError::malformed(None, "nominal declaration index overflows"))?;
        self.types
            .declarations
            .get(index)
            .map(|declaration| declaration.kind)
            .ok_or_else(|| FormatError::malformed(None, "nominal declaration index is invalid"))
    }

    fn definition(
        &self,
        declaration: u64,
    ) -> Result<&crate::types::BinaryNominalDefinition, FormatError> {
        self.types
            .definitions
            .iter()
            .find(|definition| definition.declaration == declaration)
            .ok_or_else(|| FormatError::malformed(None, "constant uses an incomplete nominal type"))
    }

    fn type_entry(&self, index: u64) -> Result<&BinaryType, FormatError> {
        let index = usize::try_from(index)
            .map_err(|_| FormatError::malformed(None, "constant type index overflows memory"))?;
        self.types
            .entries
            .get(index)
            .ok_or_else(|| FormatError::malformed(None, "constant type index is out of range"))
    }

    fn child_use(&self, index: u64, bindings: &[TypeUse]) -> TypeUse {
        TypeUse {
            index,
            bindings: bindings.to_vec(),
        }
    }

    fn write_count(&mut self, count: usize) -> Result<(), FormatError> {
        let count = u64::try_from(count)
            .map_err(|_| FormatError::malformed(None, "CONSTANTS count overflows"))?;
        self.write_index(count)
    }

    fn write_index(&mut self, index: u64) -> Result<(), FormatError> {
        write_index(index, self.bytes)
    }
}

fn write_integer(raw: &[u8], bits: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let bit_width = usize::try_from(bits)
        .map_err(|_| FormatError::malformed(None, "integer width overflows memory"))?;
    let byte_width = bit_width.div_ceil(8);
    if raw.len() != byte_width {
        return Err(FormatError::malformed(
            None,
            "integer constant has the wrong byte width",
        ));
    }
    if bit_width % 8 != 0 && raw.last().is_some_and(|byte| byte >> (bit_width % 8) != 0) {
        return Err(FormatError::malformed(
            None,
            "integer constant exceeds its type width",
        ));
    }
    let significant_bits = raw.iter().rposition(|byte| *byte != 0).map_or(0, |index| {
        index * 8 + (8 - raw[index].leading_zeros() as usize)
    });
    let groups = significant_bits.div_ceil(7).max(1);
    for group in 0..groups {
        let mut payload = 0u8;
        for bit in 0..7 {
            let position = group * 7 + bit;
            if position < bit_width && raw[position / 8] & (1 << (position % 8)) != 0 {
                payload |= 1 << bit;
            }
        }
        output.push(payload | if group + 1 < groups { 0x80 } else { 0 });
    }
    Ok(())
}

fn validate_encoded(
    bytes: &[u8],
    strings: &StringTable,
    types: &BinaryTypeTable,
) -> Result<(), FormatError> {
    let mut decoder = super::ConstantDecoder::new(bytes, strings, types);
    let count = super::read_count(bytes, &mut decoder.cursor, 2, "constant")?;
    for _ in 0..count {
        let ty = super::read_index(bytes, &mut decoder.cursor)?;
        if usize::try_from(ty).map_or(true, |index| index >= types.entries.len()) {
            return Err(FormatError::malformed(
                None,
                "constant type index is out of range",
            ));
        }
        decoder.read_value(ty, 0)?;
    }
    if decoder.cursor != bytes.len() {
        return Err(FormatError::malformed(
            None,
            "trailing bytes in CONSTANTS section",
        ));
    }
    Ok(())
}

fn write_index(index: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes).map_err(|error| {
        FormatError::malformed(None, format!("CONSTANTS encoding failed: {error}"))
    })
}
