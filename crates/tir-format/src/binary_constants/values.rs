use super::ConstantDecoder;
use crate::types::{
    BinaryConstantValue, BinaryCopyPolicy, BinaryDestroyPolicy, BinaryNominalKind,
    BinaryRelocatePolicy, BinaryType,
};
use crate::{FormatError, leb};

const MAX_VALUE_DEPTH: usize = 256;

#[derive(Clone)]
struct TypeUse {
    index: u64,
    bindings: Vec<TypeUse>,
}

impl ConstantDecoder<'_> {
    pub(super) fn read_value(
        &mut self,
        type_index: u64,
        depth: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        self.read_value_in(
            TypeUse {
                index: type_index,
                bindings: Vec::new(),
            },
            depth,
        )
    }

    fn read_value_in(
        &mut self,
        ty: TypeUse,
        depth: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(malformed(self.cursor, "constant nesting exceeds 255"));
        }
        let ty = self.resolve(ty, 0)?;
        let form_offset = self.cursor;
        let form = read_byte(self.bytes, &mut self.cursor)?;
        let entry = self.type_entry(ty.index)?.clone();
        match form {
            0 => self.read_scalar(&entry),
            1 => {
                if !self.is_trivial(&ty, 0)? {
                    return Err(malformed(
                        form_offset,
                        "zero constant requires a trivial type",
                    ));
                }
                Ok(BinaryConstantValue::Zero)
            }
            2 => {
                if !self.is_trivial(&ty, 0)? {
                    return Err(malformed(
                        form_offset,
                        "byte constant requires a trivial type",
                    ));
                }
                let length = read_count(self.bytes, &mut self.cursor, 1, "byte constant")?;
                Ok(BinaryConstantValue::Bytes(read_bytes(
                    self.bytes,
                    &mut self.cursor,
                    length,
                    "byte constant",
                )?))
            }
            3 => {
                if !self.is_string_tuple(&ty)? {
                    return Err(malformed(
                        form_offset,
                        "string constant type must be (ptr, i64)",
                    ));
                }
                let index_offset = self.cursor;
                let index = usize::try_from(read_index(self.bytes, &mut self.cursor)?)
                    .map_err(|_| malformed(index_offset, "string index overflows memory"))?;
                let value = self.strings.get(index).ok_or_else(|| {
                    malformed(index_offset, "string constant index is out of range")
                })?;
                Ok(BinaryConstantValue::String(value.to_owned()))
            }
            4 => self.read_aggregate(entry, &ty, depth + 1, form_offset),
            5 => self.read_enum(entry, &ty, depth + 1, form_offset),
            8 => self.read_union(entry, &ty, depth + 1, form_offset),
            6 => {
                if !matches!(entry, BinaryType::Pointer(_)) {
                    return Err(malformed(
                        form_offset,
                        "null constant requires a pointer type",
                    ));
                }
                Ok(BinaryConstantValue::Null)
            }
            7 => self.read_sampler(&entry, form_offset),
            _ => Err(malformed(form_offset, "invalid CONSTANTS value-form code")),
        }
    }

    fn read_scalar(&mut self, ty: &BinaryType) -> Result<BinaryConstantValue, FormatError> {
        match ty {
            BinaryType::Integer(bits) => Ok(BinaryConstantValue::Integer(read_integer_bits(
                self.bytes,
                &mut self.cursor,
                *bits,
            )?)),
            BinaryType::Float(float) => {
                let value = read_bytes(
                    self.bytes,
                    &mut self.cursor,
                    usize::from(float.bits().div_ceil(8)),
                    "floating constant",
                )?;
                let padding = float.bits() % 8;
                if padding != 0 && value.last().is_some_and(|byte| byte >> padding != 0) {
                    return Err(malformed(
                        self.cursor - 1,
                        "floating constant has nonzero padding bits",
                    ));
                }
                Ok(BinaryConstantValue::Float(value))
            }
            _ => Err(malformed(
                self.cursor,
                "scalar constant requires integer or float type",
            )),
        }
    }

    fn read_aggregate(
        &mut self,
        entry: BinaryType,
        ty: &TypeUse,
        depth: usize,
        offset: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        if !self.is_trivial(ty, 0)? {
            return Err(malformed(
                offset,
                "aggregate constant requires a trivial type",
            ));
        }
        match entry {
            BinaryType::Tuple(elements) => {
                let mut values = reserve(elements.len(), self.bytes, self.cursor)?;
                for element in elements {
                    values.push(self.read_child(element, &ty.bindings, depth)?);
                }
                Ok(BinaryConstantValue::Tuple(values))
            }
            BinaryType::Array { count, element } => {
                let count = self.checked_element_count(count, "array constant")?;
                let mut values = reserve(count, self.bytes, self.cursor)?;
                for _ in 0..count {
                    values.push(self.read_child(element, &ty.bindings, depth)?);
                }
                Ok(BinaryConstantValue::Array(values))
            }
            BinaryType::FixedVector { lanes, element } => {
                let count = self.checked_element_count(lanes, "vector constant")?;
                let mut values = reserve(count, self.bytes, self.cursor)?;
                for _ in 0..count {
                    values.push(self.read_child(element, &ty.bindings, depth)?);
                }
                Ok(BinaryConstantValue::Vector(values))
            }
            BinaryType::Nominal {
                declaration,
                arguments,
            } => {
                let definition = self.definition(declaration)?.clone();
                if self.types.declarations[declaration as usize].kind != BinaryNominalKind::Struct {
                    return Err(malformed(
                        offset,
                        "aggregate nominal constants require a struct type",
                    ));
                }
                let bindings = self.make_bindings(&arguments, &ty.bindings)?;
                let mut values = reserve(definition.fields.len(), self.bytes, self.cursor)?;
                for field in definition.fields {
                    values.push(self.read_child(field.ty, &bindings, depth)?);
                }
                Ok(BinaryConstantValue::Struct(values))
            }
            _ => Err(malformed(
                offset,
                "aggregate form does not match the constant type",
            )),
        }
    }

    fn read_union(
        &mut self,
        entry: BinaryType,
        ty: &TypeUse,
        depth: usize,
        offset: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        if !self.is_trivial(ty, 0)? {
            return Err(malformed(offset, "union constant requires a trivial type"));
        }
        let BinaryType::Nominal {
            declaration,
            arguments,
        } = entry
        else {
            return Err(malformed(
                offset,
                "union form requires a nominal union type",
            ));
        };
        if self.types.declarations[declaration as usize].kind != BinaryNominalKind::Explicit {
            return Err(malformed(
                offset,
                "union form requires a nominal union type",
            ));
        }
        let definition = self.definition(declaration)?.clone();
        let member_offset = self.cursor;
        let member = read_index(self.bytes, &mut self.cursor)?;
        let member_index = usize::try_from(member)
            .map_err(|_| malformed(member_offset, "union member index overflows memory"))?;
        let field = definition
            .fields
            .get(member_index)
            .ok_or_else(|| malformed(member_offset, "union member index is out of range"))?;
        let bindings = self.make_bindings(&arguments, &ty.bindings)?;
        let value = self.read_child(field.ty, &bindings, depth)?;
        Ok(BinaryConstantValue::Union {
            member,
            value: Box::new(value),
        })
    }

    fn read_enum(
        &mut self,
        entry: BinaryType,
        ty: &TypeUse,
        depth: usize,
        offset: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        if !self.is_trivial(ty, 0)? {
            return Err(malformed(offset, "enum constant requires a trivial type"));
        }
        let BinaryType::Nominal {
            declaration,
            arguments,
        } = entry
        else {
            return Err(malformed(offset, "enum form requires a nominal enum type"));
        };
        if self.types.declarations[declaration as usize].kind != BinaryNominalKind::Enum {
            return Err(malformed(offset, "enum form requires a nominal enum type"));
        }
        let definition = self.definition(declaration)?.clone();
        let variant_offset = self.cursor;
        let variant = read_index(self.bytes, &mut self.cursor)?;
        let variant_index = usize::try_from(variant)
            .map_err(|_| malformed(variant_offset, "enum variant index overflows memory"))?;
        let record = definition
            .variants
            .get(variant_index)
            .ok_or_else(|| malformed(variant_offset, "enum variant index is out of range"))?;
        let bindings = self.make_bindings(&arguments, &ty.bindings)?;
        let mut payload = reserve(record.payload.len(), self.bytes, self.cursor)?;
        for payload_type in &record.payload {
            payload.push(self.read_child(*payload_type, &bindings, depth)?);
        }
        Ok(BinaryConstantValue::Enum { variant, payload })
    }

    fn read_sampler(
        &mut self,
        ty: &BinaryType,
        offset: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        if !matches!(ty, BinaryType::Sampler) {
            return Err(malformed(offset, "sampler form requires sampler type"));
        }
        let bytes = read_bytes(self.bytes, &mut self.cursor, 23, "sampler descriptor")?;
        if bytes[0..3].iter().any(|value| *value > 1)
            || bytes[3..6].iter().any(|value| *value > 4)
            || bytes[6] > 2
            || !(1..=16).contains(&bytes[7])
            || (bytes[8] > 7 && bytes[8] != 255)
            || bytes[9] > 2
            || bytes[10] > 1
        {
            return Err(malformed(offset, "invalid sampler descriptor field"));
        }
        let descriptor: [u8; 23] = bytes
            .try_into()
            .map_err(|_| malformed(offset, "invalid sampler descriptor size"))?;
        Ok(BinaryConstantValue::Sampler(descriptor))
    }

    fn read_child(
        &mut self,
        index: u64,
        bindings: &[TypeUse],
        depth: usize,
    ) -> Result<BinaryConstantValue, FormatError> {
        self.read_value_in(
            TypeUse {
                index,
                bindings: bindings.to_vec(),
            },
            depth,
        )
    }

    fn resolve(&self, ty: TypeUse, depth: usize) -> Result<TypeUse, FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(malformed(
                self.cursor,
                "generic constant type substitution is recursive",
            ));
        }
        match self.type_entry(ty.index)? {
            BinaryType::Generic { parameter } => {
                let parameter = usize::try_from(*parameter).map_err(|_| {
                    malformed(self.cursor, "generic parameter index overflows memory")
                })?;
                ty.bindings
                    .get(parameter)
                    .cloned()
                    .ok_or_else(|| {
                        malformed(
                            self.cursor,
                            "constant contains an unsubstituted generic type",
                        )
                    })
                    .and_then(|binding| self.resolve(binding, depth + 1))
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

    fn checked_element_count(&self, count: u64, label: &str) -> Result<usize, FormatError> {
        let count = usize::try_from(count)
            .map_err(|_| malformed(self.cursor, format!("{label} count overflows memory")))?;
        if count > self.bytes.len().saturating_sub(self.cursor) {
            return Err(malformed(
                self.cursor,
                format!("{label} count exceeds remaining data"),
            ));
        }
        Ok(count)
    }

    fn is_trivial(&self, ty: &TypeUse, depth: usize) -> Result<bool, FormatError> {
        if depth >= MAX_VALUE_DEPTH {
            return Ok(false);
        }
        let ty = self.resolve(ty.clone(), 0)?;
        match self.type_entry(ty.index)? {
            BinaryType::Integer(_)
            | BinaryType::Float(_)
            | BinaryType::Pointer(_)
            | BinaryType::Unit
            | BinaryType::Sampler
            | BinaryType::AccelerationStructure
            | BinaryType::Texture { .. }
            | BinaryType::VisibleFunctionTable { .. }
            | BinaryType::Tensor { .. }
            | BinaryType::RayQuery
            | BinaryType::HitObject
            | BinaryType::CooperativeMatrix { .. }
            | BinaryType::Entry { .. } => Ok(true),
            BinaryType::FixedVector { element, .. } => {
                self.is_trivial(&self.child_use(*element, &ty.bindings), depth + 1)
            }
            BinaryType::Tuple(elements) => {
                for element in elements {
                    if !self.is_trivial(&self.child_use(*element, &ty.bindings), depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            BinaryType::Array { element, .. } => {
                self.is_trivial(&self.child_use(*element, &ty.bindings), depth + 1)
            }
            BinaryType::Nominal { declaration, .. } => {
                let definition = self.definition(*declaration)?;
                Ok(matches!(definition.policy.copy, BinaryCopyPolicy::Trivial)
                    && matches!(definition.policy.destroy, BinaryDestroyPolicy::Trivial)
                    && matches!(definition.policy.relocate, BinaryRelocatePolicy::Bitwise))
            }
            _ => Ok(false),
        }
    }

    fn child_use(&self, index: u64, bindings: &[TypeUse]) -> TypeUse {
        TypeUse {
            index,
            bindings: bindings.to_vec(),
        }
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

    fn type_entry(&self, index: u64) -> Result<&BinaryType, FormatError> {
        let index = usize::try_from(index)
            .map_err(|_| malformed(self.cursor, "constant type index overflows memory"))?;
        self.types
            .entries
            .get(index)
            .ok_or_else(|| malformed(self.cursor, "constant type index is out of range"))
    }

    fn definition(
        &self,
        declaration: u64,
    ) -> Result<&crate::types::BinaryNominalDefinition, FormatError> {
        self.types
            .definitions
            .iter()
            .find(|definition| definition.declaration == declaration)
            .ok_or_else(|| malformed(self.cursor, "constant uses an incomplete nominal type"))
    }
}

fn read_integer_bits(bytes: &[u8], cursor: &mut usize, bits: u64) -> Result<Vec<u8>, FormatError> {
    let bit_width =
        usize::try_from(bits).map_err(|_| malformed(*cursor, "integer width overflows memory"))?;
    if !(1..=16384).contains(&bit_width) {
        return Err(malformed(
            *cursor,
            "integer width is outside the TIR type range",
        ));
    }
    let byte_width = bit_width.div_ceil(8);
    // A minimal ULEB can be shorter than the zero-extended integer byte width.
    // The encoded section size therefore cannot bound this allocation.
    let mut result = Vec::new();
    result
        .try_reserve_exact(byte_width)
        .map_err(|_| malformed(*cursor, "integer constant allocation failed"))?;
    result.resize(byte_width, 0);
    let maximum_bytes = bit_width.div_ceil(7);
    for group in 0..maximum_bytes {
        let byte = read_byte(bytes, cursor)?;
        let payload = byte & 0x7f;
        for bit in 0..7 {
            let position = group * 7 + bit;
            if payload & (1 << bit) == 0 {
                continue;
            }
            if position >= bit_width {
                return Err(malformed(
                    *cursor - 1,
                    "integer constant exceeds its type width",
                ));
            }
            result[position / 8] |= 1 << (position % 8);
        }
        if byte & 0x80 == 0 {
            if group > 0 && payload == 0 {
                return Err(malformed(*cursor - 1, "non-minimal integer constant ULEB"));
            }
            return Ok(result);
        }
    }
    Err(malformed(
        *cursor,
        "integer constant ULEB overflows its type width",
    ))
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes {
        return Err(malformed(
            offset,
            format!("{label} count exceeds remaining CONSTANTS data"),
        ));
    }
    Ok(count)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    if count > bytes.len() {
        return Err(malformed(offset, "CONSTANTS count exceeds section size"));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "CONSTANTS allocation failed"))?;
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    leb::read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid CONSTANTS ULEB: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, "truncated CONSTANTS section"))?;
    *cursor += 1;
    Ok(value)
}

fn read_bytes(
    bytes: &[u8],
    cursor: &mut usize,
    length: usize,
    label: &str,
) -> Result<Vec<u8>, FormatError> {
    let end = cursor
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(*cursor, format!("truncated {label}")))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| malformed(*cursor, format!("{label} allocation failed")))?;
    output.extend_from_slice(&bytes[*cursor..end]);
    *cursor = end;
    Ok(output)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
