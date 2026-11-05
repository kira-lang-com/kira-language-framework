use crate::types::{
    BinaryFunctionParameter, BinaryFunctionType, BinaryGenericParameter, BinaryLayout,
    BinaryNominalDeclaration, BinaryNominalDefinition, BinaryNominalKind, BinaryType,
    BinaryTypeTable,
};
use crate::{BinaryModule, FormatError, StringTable};
mod encode;
mod records;
use records::{
    malformed, read_bool, read_byte, read_count, read_enum_byte, read_fields, read_index,
    read_layout, read_nonempty_string, read_policy, read_variants, reserve,
};
use tir_types::FloatType;

const MAX_TYPE_COUNT: u64 = 1 << 32;
const MAX_NESTING_DEPTH: usize = 256;

impl BinaryModule {
    /// Decodes and structurally validates the TIR TYPES section.
    pub fn type_table(&self, memory_limit: u64) -> Result<BinaryTypeTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 4) else {
            return Ok(BinaryTypeTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        decode_type_table(&bytes, &strings)
    }
}

fn decode_type_table(bytes: &[u8], strings: &StringTable) -> Result<BinaryTypeTable, FormatError> {
    let mut cursor = 0;
    let declaration_count = read_count(bytes, &mut cursor, 3, "declaration")?;
    let mut declarations = reserve(declaration_count, bytes, cursor)?;
    let mut declaration_names = std::collections::HashSet::new();
    for _ in 0..declaration_count {
        let name = read_nonempty_string(bytes, &mut cursor, strings, "nominal declaration")?;
        if !declaration_names.insert(name.clone()) {
            return Err(malformed(cursor, "duplicate nominal declaration name"));
        }
        let parameter_count = read_count(bytes, &mut cursor, 2, "generic parameter")?;
        if parameter_count >= 1 << 16 {
            return Err(malformed(
                cursor,
                "nominal type has too many generic parameters",
            ));
        }
        let mut generic_parameters = reserve(parameter_count, bytes, cursor)?;
        for _ in 0..parameter_count {
            let parameter_name =
                read_nonempty_string(bytes, &mut cursor, strings, "generic parameter")?;
            let bound_count = read_count(bytes, &mut cursor, 1, "generic bound")?;
            let mut bounds = reserve(bound_count, bytes, cursor)?;
            for _ in 0..bound_count {
                let bound = read_nonempty_string(bytes, &mut cursor, strings, "generic bound")?;
                if bounds.contains(&bound) {
                    return Err(malformed(cursor, "duplicate generic bound"));
                }
                bounds.push(bound);
            }
            generic_parameters.push(BinaryGenericParameter {
                name: parameter_name,
                bounds,
            });
        }
        let kind = match read_byte(bytes, &mut cursor)? {
            0 => BinaryNominalKind::Struct,
            1 => BinaryNominalKind::Enum,
            2 => BinaryNominalKind::Explicit,
            _ => return Err(malformed(cursor - 1, "invalid nominal declaration kind")),
        };
        let mut parameter_names = std::collections::HashSet::new();
        for parameter in &generic_parameters {
            if !parameter_names.insert(parameter.name.as_str()) {
                return Err(malformed(
                    cursor,
                    "duplicate nominal generic parameter name",
                ));
            }
        }
        declarations.push(BinaryNominalDeclaration {
            name,
            generic_parameters,
            kind,
        });
    }

    let type_count = read_count(bytes, &mut cursor, 1, "type entry")?;
    if type_count as u64 >= MAX_TYPE_COUNT {
        return Err(malformed(
            cursor,
            "TYPES entry count must be less than 2^32",
        ));
    }
    let type_entries_offset = cursor;
    let mut decoder = TypeDecoder {
        bytes,
        cursor: &mut cursor,
        declarations: &declarations,
        entries: Vec::new(),
        depths: Vec::new(),
    };
    decoder
        .depths
        .try_reserve_exact(type_count)
        .map_err(|_| malformed(type_entries_offset, "TYPES depth allocation failed"))?;
    decoder
        .entries
        .try_reserve_exact(type_count)
        .map_err(|_| malformed(type_entries_offset, "TYPES entry allocation failed"))?;
    for _ in 0..type_count {
        let (entry, depth) = decoder.read_entry()?;
        if depth > MAX_NESTING_DEPTH {
            return Err(malformed(
                *decoder.cursor,
                "TYPES nesting depth exceeds 255",
            ));
        }
        decoder.depths.push(depth);
        decoder.entries.push(entry);
    }
    let entries = std::mem::take(&mut decoder.entries);
    drop(decoder);

    let definition_count = read_count(bytes, &mut cursor, 2, "nominal definition")?;
    let mut definitions = reserve(definition_count, bytes, cursor)?;
    let mut previous_declaration = None;
    for _ in 0..definition_count {
        let record_offset = cursor;
        let declaration = read_index(bytes, &mut cursor)?;
        let declaration_index = usize::try_from(declaration)
            .map_err(|_| malformed(record_offset, "nominal declaration index overflows memory"))?;
        let declaration_record = declarations
            .get(declaration_index)
            .ok_or_else(|| malformed(record_offset, "nominal declaration index is out of range"))?;
        if previous_declaration.is_some_and(|previous| declaration <= previous) {
            return Err(malformed(
                record_offset,
                "nominal definitions are not in increasing declaration order",
            ));
        }
        previous_declaration = Some(declaration);

        let layout = read_layout(bytes, &mut cursor)?;
        if declaration_record.kind == BinaryNominalKind::Enum
            && matches!(layout, BinaryLayout::Explicit { .. })
        {
            return Err(malformed(
                record_offset,
                "enum declaration cannot use explicit layout",
            ));
        }
        let (fields, variants) = match declaration_record.kind {
            BinaryNominalKind::Struct | BinaryNominalKind::Explicit => (
                read_fields(bytes, &mut cursor, strings, entries.len(), layout)?,
                Vec::new(),
            ),
            BinaryNominalKind::Enum => (
                Vec::new(),
                read_variants(bytes, &mut cursor, strings, entries.len())?,
            ),
        };
        let policy = read_policy(bytes, &mut cursor)?;
        definitions.push(BinaryNominalDefinition {
            declaration,
            layout,
            fields,
            variants,
            policy,
        });
    }

    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in TYPES section"));
    }
    Ok(BinaryTypeTable {
        declarations,
        entries,
        definitions,
    })
}

struct TypeDecoder<'a> {
    bytes: &'a [u8],
    cursor: &'a mut usize,
    declarations: &'a [BinaryNominalDeclaration],
    entries: Vec<BinaryType>,
    depths: Vec<usize>,
}

impl TypeDecoder<'_> {
    fn read_entry(&mut self) -> Result<(BinaryType, usize), FormatError> {
        let tag_offset = *self.cursor;
        let tag = read_byte(self.bytes, self.cursor)?;
        let mut child_depth = 0;
        let ty = match tag {
            0x01 => {
                let bits = self.read_uleb()?;
                if !(1..=16_384).contains(&bits) {
                    return Err(malformed(
                        *self.cursor,
                        "integer width is outside 1..=16384",
                    ));
                }
                BinaryType::Integer(bits)
            }
            0x02 => BinaryType::Float(FloatType::F16),
            0x03 => BinaryType::Float(FloatType::Bf16),
            0x04 => BinaryType::Float(FloatType::F32),
            0x05 => BinaryType::Float(FloatType::F64),
            0x06 => BinaryType::Float(FloatType::F128),
            0x07 => BinaryType::Float(FloatType::F8E4M3),
            0x08 => BinaryType::Float(FloatType::F8E5M2),
            0x09 => BinaryType::Float(FloatType::F6E2M3),
            0x0a => BinaryType::Float(FloatType::F6E3M2),
            0x0b => BinaryType::Float(FloatType::F4E2M1),
            0x0c => BinaryType::Float(FloatType::E8M0),
            0x10 => {
                BinaryType::Pointer(read_enum_byte(self.bytes, self.cursor, 9, "address space")?)
            }
            0x11 => {
                let pointee = self.read_type_ref(&mut child_depth)?;
                let space = read_enum_byte(self.bytes, self.cursor, 9, "address space")?;
                BinaryType::Address { pointee, space }
            }
            0x12 => BinaryType::Unit,
            0x13 => BinaryType::Never,
            0x20 => {
                let lanes = self.read_uleb()?;
                if !matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64) {
                    return Err(malformed(*self.cursor, "invalid fixed-vector lane count"));
                }
                let element = self.read_type_ref(&mut child_depth)?;
                if !self.is_fixed_vector_element(element) {
                    return Err(malformed(*self.cursor, "invalid fixed-vector element type"));
                }
                BinaryType::FixedVector { lanes, element }
            }
            0x21 => {
                let element = self.read_type_ref(&mut child_depth)?;
                let Some(bits) = self.element_bits(element) else {
                    return Err(malformed(
                        *self.cursor,
                        "invalid scalable-vector element type",
                    ));
                };
                if bits == 0 || 128 % bits != 0 {
                    return Err(malformed(
                        *self.cursor,
                        "scalable-vector lane count is not integral",
                    ));
                }
                BinaryType::ScalableVector { element }
            }
            0x22 => {
                let count = self.read_count(1, "tuple element")?;
                let mut elements = reserve(count, self.bytes, *self.cursor)?;
                for _ in 0..count {
                    elements.push(self.read_type_ref(&mut child_depth)?);
                }
                BinaryType::Tuple(elements)
            }
            0x23 => {
                let count = self.read_uleb()?;
                if count >= 1 << 48 {
                    return Err(malformed(
                        *self.cursor,
                        "array count must be less than 2^48",
                    ));
                }
                let element = self.read_type_ref(&mut child_depth)?;
                BinaryType::Array { count, element }
            }
            0x24 => {
                let declaration = self.read_uleb()?;
                let declaration_index = usize::try_from(declaration)
                    .map_err(|_| malformed(*self.cursor, "nominal type index overflows memory"))?;
                let generic_count = self.read_count(1, "nominal type argument")?;
                let declaration_record =
                    self.declarations.get(declaration_index).ok_or_else(|| {
                        malformed(
                            *self.cursor,
                            "nominal type declaration index is out of range",
                        )
                    })?;
                if generic_count != declaration_record.generic_parameters.len() {
                    return Err(malformed(
                        *self.cursor,
                        "nominal type argument count does not match its declaration",
                    ));
                }
                let mut arguments = reserve(generic_count, self.bytes, *self.cursor)?;
                for _ in 0..generic_count {
                    arguments.push(self.read_type_ref(&mut child_depth)?);
                }
                BinaryType::Nominal {
                    declaration,
                    arguments,
                }
            }
            0x25 => BinaryType::Generic {
                parameter: self.read_uleb()?,
            },
            0x26 => BinaryType::Reference {
                pointee: self.read_type_ref(&mut child_depth)?,
            },
            0x27 => BinaryType::Unique {
                pointee: self.read_type_ref(&mut child_depth)?,
            },
            0x28 => BinaryType::Weak {
                pointee: self.read_type_ref(&mut child_depth)?,
            },
            0x30 => BinaryType::Function(self.read_function_type(&mut child_depth)?),
            0x31 => {
                let function_type = self.read_type_ref(&mut child_depth)?;
                if !matches!(self.entry(function_type), Some(BinaryType::Function(_))) {
                    return Err(malformed(
                        *self.cursor,
                        "closure signature is not a function type",
                    ));
                }
                let borrowed = read_bool(self.bytes, self.cursor, "closure borrowed flag")?;
                BinaryType::Closure {
                    function_type,
                    borrowed,
                }
            }
            0x32 => {
                let yield_type = self.read_type_ref(&mut child_depth)?;
                let resume_type = self.read_type_ref(&mut child_depth)?;
                let result_type = self.read_type_ref(&mut child_depth)?;
                BinaryType::Coroutine {
                    yield_type,
                    resume_type,
                    result_type,
                }
            }
            0x33 => BinaryType::Frame {
                function_index: self.read_uleb()?,
            },
            0x40 => {
                let dimension = read_enum_byte(self.bytes, self.cursor, 6, "texture dimension")?;
                let component = self.read_type_ref(&mut child_depth)?;
                let access = read_enum_byte(self.bytes, self.cursor, 3, "texture access")?;
                let multisampled = read_bool(self.bytes, self.cursor, "multisampled flag")?;
                if !self.is_texture_component(component) {
                    return Err(malformed(*self.cursor, "invalid texture component type"));
                }
                BinaryType::Texture {
                    dimension,
                    component,
                    access,
                    multisampled,
                }
            }
            0x41 => BinaryType::Sampler,
            0x42 => BinaryType::AccelerationStructure,
            0x43 => {
                let function_type = self.read_type_ref(&mut child_depth)?;
                if !matches!(self.entry(function_type), Some(BinaryType::Function(_))) {
                    return Err(malformed(
                        *self.cursor,
                        "vft signature is not a function type",
                    ));
                }
                BinaryType::VisibleFunctionTable { function_type }
            }
            0x44 => {
                let rank = self.read_uleb()?;
                if !(1..=8).contains(&rank) {
                    return Err(malformed(*self.cursor, "tensor rank is outside 1..=8"));
                }
                let element = self.read_type_ref(&mut child_depth)?;
                BinaryType::Tensor { rank, element }
            }
            0x45 => BinaryType::RayQuery,
            0x46 => BinaryType::HitObject,
            0x47 => {
                let scope = read_enum_byte(self.bytes, self.cursor, 1, "cooperative-matrix scope")?;
                let rows = self.read_uleb()?;
                let columns = self.read_uleb()?;
                if rows == 0 || columns == 0 {
                    return Err(malformed(
                        *self.cursor,
                        "cooperative-matrix dimensions must be nonzero",
                    ));
                }
                let element = self.read_type_ref(&mut child_depth)?;
                let use_kind =
                    read_enum_byte(self.bytes, self.cursor, 2, "cooperative-matrix use")?;
                BinaryType::CooperativeMatrix {
                    scope,
                    rows,
                    columns,
                    element,
                    use_kind,
                }
            }
            0x48 => {
                let kind = read_enum_byte(self.bytes, self.cursor, 12, "entry kind")?;
                let root = self.read_type_ref(&mut child_depth)?;
                let immediate = self.read_type_ref(&mut child_depth)?;
                BinaryType::Entry {
                    kind,
                    root,
                    immediate,
                }
            }
            _ => return Err(malformed(tag_offset, "unknown TIR type tag")),
        };
        Ok((ty, child_depth + 1))
    }

    fn read_function_type(&mut self, depth: &mut usize) -> Result<BinaryFunctionType, FormatError> {
        let count = self.read_count(2, "function parameter")?;
        if count >= 1 << 16 {
            return Err(malformed(
                *self.cursor,
                "function type has too many parameters",
            ));
        }
        let mut parameters = reserve(count, self.bytes, *self.cursor)?;
        let mut context_count = 0;
        for _ in 0..count {
            let encoded = read_byte(self.bytes, self.cursor)?;
            let convention = encoded & 0x7f;
            if convention > 3 {
                return Err(malformed(
                    *self.cursor - 1,
                    "invalid function parameter convention",
                ));
            }
            let context = encoded & 0x80 != 0;
            context_count += usize::from(context);
            let ty = self.read_type_ref(depth)?;
            parameters.push(BinaryFunctionParameter {
                convention,
                context,
                ty,
            });
        }
        if context_count > 1 {
            return Err(malformed(
                *self.cursor,
                "function type has multiple context parameters",
            ));
        }
        let result = self.read_type_ref(depth)?;
        let encoded_error = self.read_uleb()?;
        let error = if encoded_error == 0 {
            None
        } else {
            let index = encoded_error - 1;
            self.check_type_ref(index)?;
            let type_index = usize::try_from(index).map_err(|_| {
                malformed(*self.cursor, "function error type index overflows memory")
            })?;
            *depth = (*depth).max(self.depths[type_index]);
            Some(index)
        };
        let flags = read_byte(self.bytes, self.cursor)?;
        if flags & !0x07 != 0 {
            return Err(malformed(
                *self.cursor - 1,
                "reserved function type flags are set",
            ));
        }
        let c_abi = flags & 2 != 0;
        let foreign = flags & 4 != 0;
        if foreign && !c_abi {
            return Err(malformed(
                *self.cursor - 1,
                "foreign function type must use the C ABI",
            ));
        }
        Ok(BinaryFunctionType {
            parameters,
            result,
            error,
            asynchronous: flags & 1 != 0,
            c_abi,
            foreign,
        })
    }

    fn read_type_ref(&mut self, depth: &mut usize) -> Result<u64, FormatError> {
        let index = self.read_uleb()?;
        self.check_type_ref(index)?;
        let type_index = usize::try_from(index)
            .map_err(|_| malformed(*self.cursor, "type reference overflows memory"))?;
        *depth = (*depth).max(self.depths[type_index]);
        Ok(index)
    }

    fn check_type_ref(&self, index: u64) -> Result<(), FormatError> {
        let index = usize::try_from(index)
            .map_err(|_| malformed(*self.cursor, "type reference overflows memory"))?;
        if index >= self.depths.len() {
            return Err(malformed(
                *self.cursor,
                "type reference is not topologically earlier",
            ));
        }
        Ok(())
    }

    fn read_uleb(&mut self) -> Result<u64, FormatError> {
        read_index(self.bytes, self.cursor)
    }

    fn read_count(&mut self, min_record_bytes: usize, label: &str) -> Result<usize, FormatError> {
        read_count(self.bytes, self.cursor, min_record_bytes, label)
    }

    fn entry(&self, index: u64) -> Option<&BinaryType> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.entries.get(index))
    }

    fn is_fixed_vector_element(&self, index: u64) -> bool {
        match self.entry(index) {
            Some(BinaryType::Integer(bits)) => (1..=64).contains(bits),
            Some(BinaryType::Float(_)) => true,
            Some(BinaryType::Pointer(0)) => true,
            _ => false,
        }
    }

    fn element_bits(&self, index: u64) -> Option<u64> {
        match self.entry(index) {
            Some(BinaryType::Integer(bits)) if (1..=128).contains(bits) => Some(*bits),
            Some(BinaryType::Float(kind)) => Some(u64::from(kind.bits())),
            Some(BinaryType::Pointer(0)) => Some(64),
            _ => None,
        }
    }

    fn is_texture_component(&self, index: u64) -> bool {
        match self.entry(index) {
            Some(BinaryType::Integer(32))
            | Some(BinaryType::Float(FloatType::F16 | FloatType::F32)) => true,
            Some(BinaryType::FixedVector { lanes: 4, element }) => {
                matches!(
                    self.entry(*element),
                    Some(BinaryType::Integer(32))
                        | Some(BinaryType::Float(FloatType::F16 | FloatType::F32))
                )
            }
            _ => false,
        }
    }
}
