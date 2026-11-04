use std::collections::HashSet;

use crate::leb::{LebError, read_uleb};
use crate::types::{
    BinaryEngine, BinaryEntryKind, BinaryFpMode, BinaryFunctionType, BinaryGenericParameter,
    BinaryGenericStrategy, BinaryLinkage, BinarySignature, BinarySignatureTable, BinaryTarget,
    BinaryType, BinaryTypeTable, BinaryVariantAttribute,
};
use crate::{BinaryModule, FormatError, SemanticsMode, StringTable};

const MAX_SIGNATURE_COUNT: u64 = 1 << 32;
const SIGNATURE_MODIFIER_MASK: u64 = 0b1_1111 << 41;

mod encode;

impl BinarySignatureTable {
    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
    ) -> Result<Vec<u8>, FormatError> {
        encode::encode_signature_table(self, strings, types)
    }
}

impl BinaryModule {
    /// Decodes SIGNATURES and validates its type and frame references.
    pub fn signature_table(
        &self,
        memory_limit: u64,
        types: &BinaryTypeTable,
    ) -> Result<BinarySignatureTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 7) else {
            return Ok(BinarySignatureTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count_offset = cursor;
        let count = read_count(&bytes, &mut cursor, 16, "signature")?;
        if count as u64 >= MAX_SIGNATURE_COUNT {
            return Err(malformed(
                count_offset,
                "SIGNATURES count must be less than 2^32",
            ));
        }
        let mut records = reserve(count, &bytes, cursor)?;
        let mut names = HashSet::new();
        names
            .try_reserve(count)
            .map_err(|_| malformed(cursor, "SIGNATURES name allocation failed"))?;

        for _ in 0..count {
            let name_offset = cursor;
            let name = read_string(&bytes, &mut cursor, &strings)?;
            if name.is_empty() {
                return Err(malformed(name_offset, "function name is empty"));
            }
            if !names.insert(name.clone()) {
                return Err(malformed(name_offset, "duplicate function name"));
            }
            let linkage = match read_byte(&bytes, &mut cursor)? {
                0 => BinaryLinkage::Private,
                1 => BinaryLinkage::Module,
                2 => BinaryLinkage::Package,
                3 => BinaryLinkage::Exported,
                _ => return Err(malformed(cursor - 1, "invalid function linkage")),
            };
            let function_type_offset = cursor;
            let function_type = read_index(&bytes, &mut cursor)?;
            let function = match get_type(types, function_type, function_type_offset)? {
                BinaryType::Function(function) => function,
                _ => {
                    return Err(malformed(
                        function_type_offset,
                        "signature type is not a function type",
                    ));
                }
            };
            let generic_parameters = read_generic_parameters(&bytes, &mut cursor, &strings)?;

            let modifiers_offset = cursor;
            let modifiers = read_index(&bytes, &mut cursor)?;
            if modifiers & !SIGNATURE_MODIFIER_MASK != 0 {
                return Err(malformed(
                    modifiers_offset,
                    "SIGNATURES has unsupported function modifier bits",
                ));
            }
            let flags = read_byte(&bytes, &mut cursor)?;
            if flags & !0x0f != 0 {
                return Err(malformed(cursor - 1, "invalid function flags"));
            }
            if modifiers & (1 << 44) != 0 && modifiers & (1 << 45) != 0 {
                return Err(malformed(
                    modifiers_offset,
                    "function is both inline and noinline",
                ));
            }

            let engine = match read_byte(&bytes, &mut cursor)? {
                0 => Some(BinaryEngine::Native),
                1 => Some(BinaryEngine::Runtime),
                2 => Some(BinaryEngine::Any),
                3 => None,
                _ => return Err(malformed(cursor - 1, "invalid function engine")),
            };
            let semantics = match read_byte(&bytes, &mut cursor)? {
                0 => None,
                1 => Some(SemanticsMode::Defined),
                2 => Some(SemanticsMode::Permissive),
                _ => return Err(malformed(cursor - 1, "invalid function semantics")),
            };
            let target = match read_byte(&bytes, &mut cursor)? {
                0 => BinaryTarget::Cpu,
                1 => BinaryTarget::Gpu,
                2 => BinaryTarget::Both,
                _ => return Err(malformed(cursor - 1, "invalid function target")),
            };
            let fp_mode = match read_byte(&bytes, &mut cursor)? {
                0 => None,
                1 => Some(BinaryFpMode::Precise),
                2 => Some(BinaryFpMode::Fast),
                3 => Some(BinaryFpMode::IeeeDenormals),
                _ => {
                    return Err(malformed(
                        cursor - 1,
                        "invalid function floating-point mode",
                    ));
                }
            };
            let generic_strategy = match read_byte(&bytes, &mut cursor)? {
                0 => None,
                1 => Some(BinaryGenericStrategy::Specialize),
                2 => Some(BinaryGenericStrategy::Polymorphic),
                3 => Some(BinaryGenericStrategy::Both),
                _ => return Err(malformed(cursor - 1, "invalid generic strategy")),
            };
            let coroutine = match read_byte(&bytes, &mut cursor)? {
                0 => None,
                1 => Some((
                    read_type_reference(&bytes, &mut cursor, types)?,
                    read_type_reference(&bytes, &mut cursor, types)?,
                )),
                _ => return Err(malformed(cursor - 1, "invalid coroutine-present flag")),
            };
            let instance_count = read_count(&bytes, &mut cursor, 1, "instance type")?;
            let mut instance_types = reserve(instance_count, &bytes, cursor)?;
            for _ in 0..instance_count {
                instance_types.push(read_type_reference(&bytes, &mut cursor, types)?);
            }
            let variant = match read_byte(&bytes, &mut cursor)? {
                0 => None,
                1 => Some(read_variant(&bytes, &mut cursor, &strings)?),
                _ => return Err(malformed(cursor - 1, "invalid variant-present flag")),
            };
            let entry_kind = read_entry_kind(&bytes, &mut cursor)?;
            let platform_export = read_optional_string(&bytes, &mut cursor, &strings)?;

            validate_callable_attributes(
                &mut cursor,
                function,
                engine,
                target,
                entry_kind,
                variant.as_ref(),
                platform_export.as_deref(),
            )?;
            validate_function_generics(&mut cursor, types, function, generic_parameters.len())?;
            for type_index in coroutine
                .iter()
                .flat_map(|(yield_type, resume_type)| [yield_type, resume_type])
                .chain(instance_types.iter())
            {
                validate_type_generics(
                    &mut cursor,
                    types,
                    *type_index,
                    generic_parameters.len(),
                    &mut HashSet::new(),
                )?;
            }

            records.push(BinarySignature {
                name,
                linkage,
                function_type,
                generic_parameters,
                modifiers,
                structured: flags & 1 != 0,
                kernel_fp: flags & 2 != 0,
                returns_twice: flags & 4 != 0,
                inlinable: flags & 8 != 0,
                engine,
                semantics,
                target,
                fp_mode,
                generic_strategy,
                coroutine,
                instance_types,
                variant,
                entry_kind,
                platform_export,
            });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in SIGNATURES section"));
        }
        validate_frame_references(types, records.len())?;
        Ok(BinarySignatureTable { records })
    }
}

fn read_generic_parameters(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<Vec<BinaryGenericParameter>, FormatError> {
    let count = read_count(bytes, cursor, 2, "generic parameter")?;
    let mut parameters = reserve(count, bytes, *cursor)?;
    let mut names = HashSet::new();
    names
        .try_reserve(count)
        .map_err(|_| malformed(*cursor, "generic parameter name allocation failed"))?;
    for _ in 0..count {
        let offset = *cursor;
        let name = read_nonempty_string(bytes, cursor, strings, "generic parameter")?;
        if !names.insert(name.clone()) {
            return Err(malformed(offset, "duplicate generic parameter name"));
        }
        let bound_count = read_count(bytes, cursor, 1, "generic bound")?;
        let mut bounds = reserve(bound_count, bytes, *cursor)?;
        let mut unique_bounds = HashSet::new();
        unique_bounds
            .try_reserve(bound_count)
            .map_err(|_| malformed(*cursor, "generic bound allocation failed"))?;
        for _ in 0..bound_count {
            let bound_offset = *cursor;
            let bound = read_nonempty_string(bytes, cursor, strings, "generic bound")?;
            if !unique_bounds.insert(bound.clone()) {
                return Err(malformed(bound_offset, "duplicate generic bound"));
            }
            bounds.push(bound);
        }
        parameters.push(BinaryGenericParameter { name, bounds });
    }
    Ok(parameters)
}

fn read_variant(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<BinaryVariantAttribute, FormatError> {
    let architecture = read_byte(bytes, cursor)?;
    if !(1..=4).contains(&architecture) {
        return Err(malformed(*cursor - 1, "invalid variant architecture"));
    }
    let count = read_count(bytes, cursor, 1, "variant feature")?;
    let mut features = reserve(count, bytes, *cursor)?;
    for _ in 0..count {
        let offset = *cursor;
        let feature = read_nonempty_string(bytes, cursor, strings, "variant feature")?;
        if features
            .last()
            .is_some_and(|previous: &String| feature.as_str() <= previous.as_str())
        {
            return Err(malformed(
                offset,
                "variant feature names are not strictly ordered",
            ));
        }
        features.push(feature);
    }
    Ok(BinaryVariantAttribute {
        architecture,
        features,
    })
}

fn read_entry_kind(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<Option<BinaryEntryKind>, FormatError> {
    let offset = *cursor;
    let kind = read_byte(bytes, cursor)?;
    let entry = match kind {
        0 => return Ok(None),
        1 => BinaryEntryKind::Compute,
        2 => BinaryEntryKind::Object,
        3 => BinaryEntryKind::Mesh,
        4 => BinaryEntryKind::Vertex,
        5 => BinaryEntryKind::Fragment,
        6 => BinaryEntryKind::Tile,
        7 => BinaryEntryKind::Raygen,
        8 => BinaryEntryKind::Intersection,
        9 => BinaryEntryKind::Anyhit,
        10 => BinaryEntryKind::Closesthit,
        11 => BinaryEntryKind::Miss,
        12 => BinaryEntryKind::Callable,
        13 => BinaryEntryKind::Node,
        _ => return Err(malformed(offset, "invalid entry kind")),
    };
    Ok(Some(entry))
}

fn validate_callable_attributes(
    cursor: &mut usize,
    function: &BinaryFunctionType,
    engine: Option<BinaryEngine>,
    target: BinaryTarget,
    entry_kind: Option<BinaryEntryKind>,
    variant: Option<&BinaryVariantAttribute>,
    platform_export: Option<&str>,
) -> Result<(), FormatError> {
    if (target == BinaryTarget::Gpu) != engine.is_none() {
        return Err(malformed(
            *cursor,
            "engine presence does not match the function target",
        ));
    }
    if entry_kind.is_some() && target != BinaryTarget::Gpu {
        return Err(malformed(*cursor, "entry target must be GPU"));
    }
    if variant.is_some() && target == BinaryTarget::Gpu {
        return Err(malformed(
            *cursor,
            "GPU-only function cannot have a CPU architecture variant",
        ));
    }
    if platform_export.is_some() && !function.c_abi {
        return Err(malformed(
            *cursor,
            "platform export requires a C-ABI function type",
        ));
    }
    if function.foreign && platform_export.is_some() {
        return Err(malformed(
            *cursor,
            "foreign function type cannot be a platform export definition",
        ));
    }
    Ok(())
}

fn validate_function_generics(
    cursor: &mut usize,
    types: &BinaryTypeTable,
    function: &BinaryFunctionType,
    generic_count: usize,
) -> Result<(), FormatError> {
    for index in function
        .parameters
        .iter()
        .map(|parameter| parameter.ty)
        .chain([function.result])
        .chain(function.error)
    {
        validate_type_generics(cursor, types, index, generic_count, &mut HashSet::new())?;
    }
    Ok(())
}

fn validate_type_generics(
    cursor: &mut usize,
    types: &BinaryTypeTable,
    index: u64,
    generic_count: usize,
    visited: &mut HashSet<u64>,
) -> Result<(), FormatError> {
    if !visited.insert(index) {
        return Ok(());
    }
    let ty = get_type(types, index, *cursor)?;
    match ty {
        BinaryType::Generic { parameter } => {
            if usize::try_from(*parameter).map_or(true, |parameter| parameter >= generic_count) {
                return Err(malformed(*cursor, "generic type parameter is out of range"));
            }
            Ok(())
        }
        BinaryType::Address { pointee, .. }
        | BinaryType::FixedVector {
            element: pointee, ..
        }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Tensor {
            element: pointee, ..
        } => validate_type_generic_children(
            cursor,
            types,
            std::iter::once(*pointee),
            generic_count,
            visited,
        ),
        BinaryType::Tuple(elements) => validate_type_generic_children(
            cursor,
            types,
            elements.iter().copied(),
            generic_count,
            visited,
        ),
        BinaryType::Nominal { arguments, .. } => validate_type_generic_children(
            cursor,
            types,
            arguments.iter().copied(),
            generic_count,
            visited,
        ),
        BinaryType::Function(function) => validate_type_generic_children(
            cursor,
            types,
            function
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .chain([function.result])
                .chain(function.error),
            generic_count,
            visited,
        ),
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => validate_type_generic_children(
            cursor,
            types,
            std::iter::once(*function_type),
            generic_count,
            visited,
        ),
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => validate_type_generic_children(
            cursor,
            types,
            [*yield_type, *resume_type, *result_type],
            generic_count,
            visited,
        ),
        BinaryType::Texture { component, .. } => validate_type_generic_children(
            cursor,
            types,
            std::iter::once(*component),
            generic_count,
            visited,
        ),
        BinaryType::CooperativeMatrix { element, .. } => validate_type_generic_children(
            cursor,
            types,
            std::iter::once(*element),
            generic_count,
            visited,
        ),
        BinaryType::Entry {
            root, immediate, ..
        } => validate_type_generic_children(
            cursor,
            types,
            [*root, *immediate],
            generic_count,
            visited,
        ),
        _ => Ok(()),
    }
}

fn validate_type_generic_children(
    cursor: &mut usize,
    types: &BinaryTypeTable,
    children: impl IntoIterator<Item = u64>,
    generic_count: usize,
    visited: &mut HashSet<u64>,
) -> Result<(), FormatError> {
    for child in children {
        validate_type_generics(cursor, types, child, generic_count, visited)?;
    }
    Ok(())
}

fn validate_frame_references(
    types: &BinaryTypeTable,
    signature_count: usize,
) -> Result<(), FormatError> {
    for (index, ty) in types.entries.iter().enumerate() {
        if let BinaryType::Frame { function_index } = ty
            && usize::try_from(*function_index).map_or(true, |function| function >= signature_count)
        {
            return Err(malformed(
                index,
                "frame type function index is out of range",
            ));
        }
    }
    Ok(())
}

fn get_type(
    types: &BinaryTypeTable,
    index: u64,
    offset: usize,
) -> Result<&BinaryType, FormatError> {
    let index =
        usize::try_from(index).map_err(|_| malformed(offset, "type index overflows memory"))?;
    types
        .entries
        .get(index)
        .ok_or_else(|| malformed(offset, "type index is out of range"))
}

fn read_type_reference(
    bytes: &[u8],
    cursor: &mut usize,
    types: &BinaryTypeTable,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor)?;
    get_type(types, index, offset)?;
    Ok(index)
}

fn read_optional_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<Option<String>, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, "platform export name index overflows memory"))?;
    let name = strings
        .get(index)
        .ok_or_else(|| malformed(offset, "platform export name index is out of range"))?;
    if index == 0 {
        return Ok(None);
    }
    if name.is_empty() {
        return Err(malformed(offset, "platform export name is empty"));
    }
    Ok(Some(name.to_owned()))
}

fn read_nonempty_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} name index overflows memory")))?;
    let value = strings
        .get(index)
        .ok_or_else(|| malformed(offset, format!("{label} name index is out of range")))?;
    if value.is_empty() {
        return Err(malformed(offset, format!("{label} name is empty")));
    }
    Ok(value.to_owned())
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<String, FormatError> {
    read_nonempty_string(bytes, cursor, strings, "function")
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
    if count as u64 >= MAX_SIGNATURE_COUNT {
        return Err(malformed(
            offset,
            format!("{label} count must be less than 2^32"),
        ));
    }
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes {
        return Err(malformed(
            *cursor,
            format!("{label} count exceeds section size"),
        ));
    }
    Ok(count)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "SIGNATURES allocation failed"))?;
    if count > bytes.len() {
        return Err(malformed(offset, "SIGNATURES count exceeds section size"));
    }
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| map_leb_error(error, *cursor))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, "truncated SIGNATURES section"))?;
    *cursor += 1;
    Ok(byte)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn map_leb_error(error: LebError, offset: usize) -> FormatError {
    malformed(offset, format!("invalid SIGNATURES ULEB: {error:?}"))
}
