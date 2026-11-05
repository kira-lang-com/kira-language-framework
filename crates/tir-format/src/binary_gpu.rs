use crate::leb::read_uleb;
use crate::types::{
    BinaryConstantTable, BinaryEntryKind, BinaryFpMode, BinaryGpuEntry, BinaryGpuInterface,
    BinaryGpuInterfaceRole, BinaryGpuSpecializationConstant, BinaryGpuTable, BinarySignatureTable,
    BinaryType, BinaryTypeTable,
};
use crate::{BinaryModule, FormatError};

mod encode;

const MAX_RECORDS: u64 = 1 << 32;
const CAPABILITY_MASK: u64 = (1 << 19) - 1;

impl BinaryModule {
    /// Decodes GPU entry metadata and cross-checks its references against the
    /// function types, constants, and entry attributes already decoded.
    pub fn gpu_table(
        &self,
        memory_limit: u64,
        signatures: &BinarySignatureTable,
        types: &BinaryTypeTable,
        constants: &BinaryConstantTable,
    ) -> Result<BinaryGpuTable, FormatError> {
        let entry_count = signatures
            .records
            .iter()
            .filter(|signature| signature.entry_kind.is_some())
            .count();
        let mut entry_indexes = Vec::new();
        entry_indexes
            .try_reserve_exact(entry_count)
            .map_err(|_| FormatError::malformed(None, "GPU entry-index allocation failed"))?;
        for (index, signature) in signatures.records.iter().enumerate() {
            if signature.entry_kind.is_some() {
                entry_indexes.push(
                    u64::try_from(index).map_err(|_| {
                        FormatError::malformed(None, "GPU entry index overflows u64")
                    })?,
                );
            }
        }
        let section = self.sections.iter().find(|section| section.kind == 10);
        if entry_indexes.is_empty() {
            if section.is_some() {
                return Err(FormatError::malformed(
                    None,
                    "GPU section is present without entry functions",
                ));
            }
            return Ok(BinaryGpuTable::default());
        }
        let Some(section) = section else {
            return Err(FormatError::malformed(
                None,
                "GPU section is missing for entry functions",
            ));
        };

        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count_offset = cursor;
        let count = read_count(&bytes, &mut cursor, 1, "GPU entry")?;
        if count != entry_indexes.len() {
            return Err(malformed(
                count_offset,
                "GPU entry count does not match SIGNATURES entries",
            ));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| malformed(cursor, "GPU entry allocation failed"))?;
        let mut previous_function = None;
        for expected_index in entry_indexes {
            let record_offset = cursor;
            let function_index = read_index(&bytes, &mut cursor, "entry function index")?;
            if function_index != expected_index
                || previous_function.is_some_and(|previous| function_index <= previous)
            {
                return Err(malformed(
                    record_offset,
                    "GPU entries are not in SIGNATURES declaration order",
                ));
            }
            previous_function = Some(function_index);
            let signature = &signatures.records[expected_index as usize];
            let kind_offset = cursor;
            let kind = read_entry_kind(read_byte(&bytes, &mut cursor, "entry kind")?, kind_offset)?;
            if Some(kind) != signature.entry_kind {
                return Err(malformed(
                    kind_offset,
                    "GPU entry kind disagrees with SIGNATURES",
                ));
            }
            let function_type = function_type(signature.function_type, types, record_offset)?;
            validate_entry_result(kind, function_type, types, record_offset)?;
            let threads = [
                read_index(&bytes, &mut cursor, "thread dimension")?,
                read_index(&bytes, &mut cursor, "thread dimension")?,
                read_index(&bytes, &mut cursor, "thread dimension")?,
            ];
            validate_threads(kind, threads, cursor)?;
            let interfaces = read_interfaces(&bytes, &mut cursor, kind, function_type, types)?;
            let payload_offset = cursor;
            let payload_reference = read_index(&bytes, &mut cursor, "payload size")?;
            let payload_size = payload_reference.checked_sub(1);
            validate_payload(kind, payload_size, payload_offset)?;
            let threadgroup_memory = read_index(&bytes, &mut cursor, "threadgroup memory size")?;
            let mesh_maximum_vertices = read_index(&bytes, &mut cursor, "mesh vertex limit")?;
            let mesh_maximum_primitives = read_index(&bytes, &mut cursor, "mesh primitive limit")?;
            let topology_offset = cursor;
            let mesh_topology = read_byte(&bytes, &mut cursor, "mesh topology")?;
            validate_mesh(
                kind,
                mesh_maximum_vertices,
                mesh_maximum_primitives,
                mesh_topology,
                topology_offset,
            )?;
            let capability_offset = cursor;
            let capabilities = read_index(&bytes, &mut cursor, "capability mask")?;
            if capabilities & !CAPABILITY_MASK != 0 {
                return Err(malformed(
                    capability_offset,
                    "GPU capability mask has unknown bits",
                ));
            }
            let subgroup_offset = cursor;
            let subgroup_size = read_index(&bytes, &mut cursor, "subgroup size")?;
            if subgroup_size != 0
                && (!(4..=128).contains(&subgroup_size) || !subgroup_size.is_power_of_two())
            {
                return Err(malformed(subgroup_offset, "invalid GPU subgroup size"));
            }
            let fp_offset = cursor;
            let floating_point_mode =
                read_fp_mode(read_byte(&bytes, &mut cursor, "FP mode")?, fp_offset)?;
            if floating_point_mode != signature.fp_mode {
                return Err(malformed(
                    fp_offset,
                    "GPU FP mode disagrees with SIGNATURES",
                ));
            }
            let specialization_constants =
                read_specialization_constants(&bytes, &mut cursor, types, constants)?;
            entries.push(BinaryGpuEntry {
                function_index,
                kind,
                threads,
                interfaces,
                payload_size,
                threadgroup_memory,
                mesh_maximum_vertices,
                mesh_maximum_primitives,
                mesh_topology,
                capabilities,
                subgroup_size,
                floating_point_mode,
                specialization_constants,
            });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in GPU section"));
        }
        Ok(BinaryGpuTable { entries })
    }
}

fn read_interfaces(
    bytes: &[u8],
    cursor: &mut usize,
    kind: BinaryEntryKind,
    function: &crate::types::BinaryFunctionType,
    types: &BinaryTypeTable,
) -> Result<Vec<BinaryGpuInterface>, FormatError> {
    let count_offset = *cursor;
    let count = read_count(bytes, cursor, 2, "GPU interface")?;
    if count > function.parameters.len().saturating_add(1) {
        return Err(malformed(
            count_offset,
            "GPU interface count exceeds entry signature",
        ));
    }
    let mut interfaces = Vec::new();
    interfaces
        .try_reserve_exact(count)
        .map_err(|_| malformed(*cursor, "GPU interface allocation failed"))?;
    let mut seen_parameters = Vec::new();
    seen_parameters
        .try_reserve_exact(function.parameters.len())
        .map_err(|_| malformed(*cursor, "GPU parameter bitmap allocation failed"))?;
    seen_parameters.resize(function.parameters.len(), false);
    let mut previous_parameter = None;
    let mut root_seen = false;
    let mut immediate_seen = false;
    let mut stage_parameters = Vec::new();
    stage_parameters
        .try_reserve_exact(count)
        .map_err(|_| malformed(*cursor, "GPU stage-parameter allocation failed"))?;
    let mut attributes_seen = false;
    for _ in 0..count {
        let role_offset = *cursor;
        let role = match read_byte(bytes, cursor, "interface role")? {
            0 => BinaryGpuInterfaceRole::Root,
            1 => BinaryGpuInterfaceRole::Immediate,
            2 => BinaryGpuInterfaceRole::StageParameter,
            3 => BinaryGpuInterfaceRole::IntersectionAttributes,
            _ => return Err(malformed(role_offset, "invalid GPU interface role")),
        };
        let parameter_index = if role == BinaryGpuInterfaceRole::IntersectionAttributes {
            if kind != BinaryEntryKind::Intersection || attributes_seen {
                return Err(malformed(
                    role_offset,
                    "invalid intersection-attributes interface",
                ));
            }
            attributes_seen = true;
            None
        } else {
            if attributes_seen {
                return Err(malformed(
                    role_offset,
                    "intersection attributes must be the final interface",
                ));
            }
            let parameter_offset = *cursor;
            let index = read_index(bytes, cursor, "interface parameter index")?;
            let index_usize = usize::try_from(index).map_err(|_| {
                malformed(
                    parameter_offset,
                    "interface parameter index overflows memory",
                )
            })?;
            if index_usize >= function.parameters.len()
                || seen_parameters[index_usize]
                || previous_parameter.is_some_and(|previous| index <= previous)
            {
                return Err(malformed(
                    parameter_offset,
                    "GPU interface parameter indexes are invalid",
                ));
            }
            seen_parameters[index_usize] = true;
            previous_parameter = Some(index);
            if role == BinaryGpuInterfaceRole::Root {
                if root_seen {
                    return Err(malformed(role_offset, "duplicate GPU root interface"));
                }
                root_seen = true;
            }
            if role == BinaryGpuInterfaceRole::Immediate {
                if immediate_seen {
                    return Err(malformed(role_offset, "duplicate GPU immediate interface"));
                }
                immediate_seen = true;
            }
            if role == BinaryGpuInterfaceRole::StageParameter {
                stage_parameters.push(index_usize);
            }
            Some(index)
        };
        let type_offset = *cursor;
        let ty = read_index(bytes, cursor, "interface type")?;
        type_index(ty, types, type_offset)?;
        if let Some(index) = parameter_index {
            if function.parameters[index as usize].ty != ty {
                return Err(malformed(
                    type_offset,
                    "GPU interface type disagrees with SIGNATURES",
                ));
            }
        }
        match role {
            BinaryGpuInterfaceRole::Root => verify_root_type(ty, types, type_offset)?,
            BinaryGpuInterfaceRole::Immediate | BinaryGpuInterfaceRole::IntersectionAttributes => {
                require_struct(ty, types, type_offset)?;
            }
            BinaryGpuInterfaceRole::StageParameter => {}
        }
        interfaces.push(BinaryGpuInterface {
            role,
            parameter_index,
            ty,
        });
    }
    if seen_parameters.iter().any(|seen| !seen) {
        return Err(malformed(
            count_offset,
            "GPU interface does not cover every entry parameter",
        ));
    }
    let attribute_required = kind == BinaryEntryKind::Intersection;
    if attributes_seen != attribute_required {
        return Err(malformed(
            count_offset,
            "intersection attributes type is missing or unexpected",
        ));
    }
    verify_stage_parameters(kind, &stage_parameters, function, types, count_offset)?;
    Ok(interfaces)
}

fn verify_stage_parameters(
    kind: BinaryEntryKind,
    stage_parameters: &[usize],
    function: &crate::types::BinaryFunctionType,
    types: &BinaryTypeTable,
    offset: usize,
) -> Result<(), FormatError> {
    let (minimum, maximum) = match kind {
        BinaryEntryKind::Object
        | BinaryEntryKind::Fragment
        | BinaryEntryKind::Miss
        | BinaryEntryKind::Callable
        | BinaryEntryKind::Node => (1, 1),
        BinaryEntryKind::Mesh => (0, 1),
        BinaryEntryKind::Anyhit | BinaryEntryKind::Closesthit => (2, 2),
        _ => (0, 0),
    };
    if !(minimum..=maximum).contains(&stage_parameters.len()) {
        return Err(malformed(
            offset,
            "GPU stage-parameter count disagrees with entry kind",
        ));
    }
    for (stage_index, parameter_index) in stage_parameters.iter().enumerate() {
        let ty = function.parameters[*parameter_index].ty;
        let valid = match (kind, stage_index) {
            (BinaryEntryKind::Fragment, 0) => is_struct(ty, types),
            (BinaryEntryKind::Object | BinaryEntryKind::Mesh, 0) => is_pointer_struct(ty, 5, types),
            (BinaryEntryKind::Anyhit | BinaryEntryKind::Closesthit, 0)
            | (BinaryEntryKind::Miss | BinaryEntryKind::Callable, 0) => {
                is_pointer_struct(ty, 6, types)
            }
            (BinaryEntryKind::Anyhit | BinaryEntryKind::Closesthit, 1) => {
                is_pointer_struct(ty, 7, types)
            }
            (BinaryEntryKind::Node, 0) => is_pointer_struct(ty, 9, types),
            _ => false,
        };
        if !valid {
            return Err(malformed(
                offset,
                "GPU stage-parameter type disagrees with entry kind",
            ));
        }
    }
    Ok(())
}

fn validate_threads(
    kind: BinaryEntryKind,
    threads: [u64; 3],
    offset: usize,
) -> Result<(), FormatError> {
    let any = threads.iter().any(|dimension| *dimension != 0);
    let all_positive = threads.iter().all(|dimension| *dimension > 0);
    match kind {
        BinaryEntryKind::Compute if !any || all_positive => Ok(()),
        BinaryEntryKind::Compute => Err(malformed(
            offset,
            "GPU thread dimensions must all be zero or positive",
        )),
        BinaryEntryKind::Object | BinaryEntryKind::Mesh if all_positive => Ok(()),
        BinaryEntryKind::Tile if all_positive && threads[2] == 1 => Ok(()),
        BinaryEntryKind::Object | BinaryEntryKind::Mesh | BinaryEntryKind::Tile => Err(malformed(
            offset,
            "GPU entry requires three positive thread dimensions",
        )),
        _ if !any => Ok(()),
        _ => Err(malformed(
            offset,
            "GPU entry kind does not accept thread dimensions",
        )),
    }
}

fn validate_payload(
    kind: BinaryEntryKind,
    payload_size: Option<u64>,
    offset: usize,
) -> Result<(), FormatError> {
    match (kind, payload_size) {
        (BinaryEntryKind::Object, Some(size)) if size <= 16_384 => Ok(()),
        (BinaryEntryKind::Object, _) => Err(malformed(
            offset,
            "object GPU entry needs a valid payload size",
        )),
        (_, None) => Ok(()),
        _ => Err(malformed(
            offset,
            "payload size is only valid for object GPU entries",
        )),
    }
}

fn validate_mesh(
    kind: BinaryEntryKind,
    vertices: u64,
    primitives: u64,
    topology: u8,
    offset: usize,
) -> Result<(), FormatError> {
    if kind == BinaryEntryKind::Mesh {
        if vertices > 256 || primitives > 512 || !(1..=3).contains(&topology) {
            return Err(malformed(offset, "invalid GPU mesh limits or topology"));
        }
    } else if vertices != 0 || primitives != 0 || topology != 0 {
        return Err(malformed(
            offset,
            "mesh fields are only valid for mesh entries",
        ));
    }
    Ok(())
}

fn read_specialization_constants(
    bytes: &[u8],
    cursor: &mut usize,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
) -> Result<Vec<BinaryGpuSpecializationConstant>, FormatError> {
    let count_offset = *cursor;
    let count = read_count(bytes, cursor, 3, "GPU specialization constant")?;
    let mut records = Vec::new();
    records
        .try_reserve_exact(count)
        .map_err(|_| malformed(*cursor, "GPU specialization-constant allocation failed"))?;
    let mut previous_id = None;
    for _ in 0..count {
        let id_offset = *cursor;
        let id = read_index(bytes, cursor, "specialization constant id")?;
        let id = u32::try_from(id)
            .map_err(|_| malformed(id_offset, "GPU specialization constant id exceeds u32"))?;
        if previous_id.is_some_and(|previous| id <= previous) {
            return Err(malformed(
                id_offset,
                "GPU specialization constants are not ordered by id",
            ));
        }
        previous_id = Some(id);
        let ty_offset = *cursor;
        let ty = read_index(bytes, cursor, "specialization constant type")?;
        type_index(ty, types, ty_offset)?;
        let constant_offset = *cursor;
        let default_constant = read_index(bytes, cursor, "specialization constant default")?;
        let default = constants
            .records
            .get(usize::try_from(default_constant).map_err(|_| {
                malformed(
                    constant_offset,
                    "specialization constant default index overflows memory",
                )
            })?)
            .ok_or_else(|| {
                malformed(
                    constant_offset,
                    "specialization constant default index is out of range",
                )
            })?;
        if default.ty != ty {
            return Err(malformed(
                constant_offset,
                "specialization constant default has the wrong type",
            ));
        }
        records.push(BinaryGpuSpecializationConstant {
            id,
            ty,
            default_constant,
        });
    }
    if records.len() != count {
        return Err(malformed(
            count_offset,
            "GPU specialization-constant count mismatch",
        ));
    }
    Ok(records)
}

fn verify_root_type(ty: u64, types: &BinaryTypeTable, offset: usize) -> Result<(), FormatError> {
    let index = type_index(ty, types, offset)?;
    match &types.entries[index] {
        BinaryType::Address { pointee, space } if matches!(*space, 1 | 2) => {
            require_struct(*pointee, types, offset)
        }
        _ => Err(malformed(
            offset,
            "GPU root type must point to a struct in constant or device space",
        )),
    }
}

fn is_pointer_struct(ty: u64, space: u8, types: &BinaryTypeTable) -> bool {
    let Ok(index) = usize::try_from(ty) else {
        return false;
    };
    matches!(types.entries.get(index), Some(BinaryType::Address { pointee, space: actual }) if *actual == space && is_struct(*pointee, types))
}

fn is_struct(ty: u64, types: &BinaryTypeTable) -> bool {
    let Ok(index) = usize::try_from(ty) else {
        return false;
    };
    let Some(BinaryType::Nominal { declaration, .. }) = types.entries.get(index) else {
        return false;
    };
    let Ok(declaration_index) = usize::try_from(*declaration) else {
        return false;
    };
    types
        .declarations
        .get(declaration_index)
        .is_some_and(|record| {
            record.kind == crate::types::BinaryNominalKind::Struct
                && types
                    .definitions
                    .iter()
                    .any(|definition| definition.declaration == *declaration)
        })
}

fn require_struct(ty: u64, types: &BinaryTypeTable, offset: usize) -> Result<(), FormatError> {
    if is_struct(ty, types) {
        Ok(())
    } else {
        Err(malformed(
            offset,
            "GPU interface type must be a defined struct",
        ))
    }
}

fn function_type<'a>(
    ty: u64,
    types: &'a BinaryTypeTable,
    offset: usize,
) -> Result<&'a crate::types::BinaryFunctionType, FormatError> {
    let ty = type_index(ty, types, offset)?;
    match types.entries.get(ty) {
        Some(BinaryType::Function(function)) => Ok(function),
        _ => Err(malformed(
            offset,
            "GPU entry signature type is not a function",
        )),
    }
}

fn validate_entry_result(
    kind: BinaryEntryKind,
    function: &crate::types::BinaryFunctionType,
    types: &BinaryTypeTable,
    offset: usize,
) -> Result<(), FormatError> {
    if function.error.is_some() || function.asynchronous || function.c_abi || function.foreign {
        return Err(malformed(
            offset,
            "GPU entries cannot fail, be async, or use a foreign ABI",
        ));
    }
    let result = type_index(function.result, types, offset)?;
    let valid = if matches!(kind, BinaryEntryKind::Vertex | BinaryEntryKind::Fragment) {
        is_struct(function.result, types)
    } else {
        matches!(types.entries.get(result), Some(BinaryType::Unit))
    };
    if valid {
        Ok(())
    } else {
        Err(malformed(
            offset,
            "GPU entry result type disagrees with its kind",
        ))
    }
}

fn type_index(ty: u64, types: &BinaryTypeTable, offset: usize) -> Result<usize, FormatError> {
    let index =
        usize::try_from(ty).map_err(|_| malformed(offset, "GPU type index overflows memory"))?;
    if index >= types.entries.len() {
        return Err(malformed(offset, "GPU type index is out of range"));
    }
    Ok(index)
}

fn read_entry_kind(value: u8, offset: usize) -> Result<BinaryEntryKind, FormatError> {
    match value {
        1 => Ok(BinaryEntryKind::Compute),
        2 => Ok(BinaryEntryKind::Object),
        3 => Ok(BinaryEntryKind::Mesh),
        4 => Ok(BinaryEntryKind::Vertex),
        5 => Ok(BinaryEntryKind::Fragment),
        6 => Ok(BinaryEntryKind::Tile),
        7 => Ok(BinaryEntryKind::Raygen),
        8 => Ok(BinaryEntryKind::Intersection),
        9 => Ok(BinaryEntryKind::Anyhit),
        10 => Ok(BinaryEntryKind::Closesthit),
        11 => Ok(BinaryEntryKind::Miss),
        12 => Ok(BinaryEntryKind::Callable),
        13 => Ok(BinaryEntryKind::Node),
        _ => Err(malformed(offset, "invalid GPU entry kind")),
    }
}

fn read_fp_mode(value: u8, offset: usize) -> Result<Option<BinaryFpMode>, FormatError> {
    match value {
        0 => Ok(None),
        1 => Ok(Some(BinaryFpMode::Precise)),
        2 => Ok(Some(BinaryFpMode::Fast)),
        3 => Ok(Some(BinaryFpMode::IeeeDenormals)),
        _ => Err(malformed(offset, "invalid GPU floating-point mode")),
    }
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor, label)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count as u64 >= MAX_RECORDS {
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

fn read_index(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid GPU {field} ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, format!("truncated GPU {field}")))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
