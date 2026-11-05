use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::read_uleb;

use crate::binary::{FormatError, TvisaImage};

mod encode;
pub use encode::encode_gpu_entry_records;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum GpuEntryKind {
    Compute = 0,
    Object = 1,
    Mesh = 2,
    Vertex = 3,
    Fragment = 4,
    Tile = 5,
    Raygen = 6,
    Intersection = 7,
    Anyhit = 8,
    Closesthit = 9,
    Miss = 10,
    Callable = 11,
    Node = 12,
    Visible = 13,
}

impl GpuEntryKind {
    fn decode(value: u8, offset: usize) -> Result<Self, FormatError> {
        match value {
            0 => Ok(Self::Compute),
            1 => Ok(Self::Object),
            2 => Ok(Self::Mesh),
            3 => Ok(Self::Vertex),
            4 => Ok(Self::Fragment),
            5 => Ok(Self::Tile),
            6 => Ok(Self::Raygen),
            7 => Ok(Self::Intersection),
            8 => Ok(Self::Anyhit),
            9 => Ok(Self::Closesthit),
            10 => Ok(Self::Miss),
            11 => Ok(Self::Callable),
            12 => Ok(Self::Node),
            13 => Ok(Self::Visible),
            _ => Err(malformed(offset, "invalid GPU entry kind")),
        }
    }

    #[must_use]
    pub const fn is_ray_tracing(self) -> bool {
        matches!(
            self,
            Self::Raygen
                | Self::Intersection
                | Self::Anyhit
                | Self::Closesthit
                | Self::Miss
                | Self::Callable
        )
    }

    #[must_use]
    pub const fn has_threads(self) -> bool {
        matches!(self, Self::Compute | Self::Object | Self::Mesh | Self::Tile)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuRootRecord {
    pub size: u64,
    pub alignment_log2: u8,
    pub type_id: u64,
    pub address_space: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuStageField {
    pub semantic: u8,
    pub location: u64,
    pub type_index: u64,
    pub interpolation: u8,
    pub byte_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuSpecializationConstant {
    pub id: u32,
    pub type_index: u64,
    pub default_constant: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuNodeOutput {
    pub name: String,
    pub record_size: u64,
    pub maximum_records: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuNodeRecord {
    pub launch_mode: u8,
    pub maximum_dispatch_grid: [u64; 3],
    pub input_record_size: u64,
    pub outputs: Vec<GpuNodeOutput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuEntryRecord {
    pub function_index: u64,
    pub kind: GpuEntryKind,
    pub visible_type_id: Option<u64>,
    pub threads: [u64; 3],
    pub root: Option<GpuRootRecord>,
    pub immediate_size: u64,
    pub immediate_type_id: u64,
    pub stage_input: Vec<GpuStageField>,
    pub stage_output: Vec<GpuStageField>,
    pub payload_sizes: [u64; 4],
    pub mesh_maximum: [u64; 2],
    pub mesh_topology: u8,
    pub threadgroup_memory: u64,
    pub capabilities: u64,
    pub subgroup_size_log2: u8,
    pub floating_point_flags: u8,
    pub specialization_constants: Vec<GpuSpecializationConstant>,
    pub node: Option<GpuNodeRecord>,
    pub ray_recursion_depth: Option<u8>,
    pub fragment_flags: Option<u8>,
}

impl TvisaImage {
    /// Decodes the binary GPU entry table and validates its record-local fields.
    pub fn gpu_entry_records(&self, memory_limit: u64) -> Result<Vec<GpuEntryRecord>, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 11)
            .ok_or_else(|| FormatError::malformed(None, "GPU section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let strings = self.string_table(memory_limit)?;
        let type_section = self.type_section(memory_limit)?;
        decode_gpu_entries(&bytes, &strings, type_section.value_type_count)
    }
}

fn decode_gpu_entries(
    bytes: &[u8],
    strings: &StringTable,
    type_count: u64,
) -> Result<Vec<GpuEntryRecord>, FormatError> {
    let mut cursor = 0;
    let mut entries = Vec::new();
    let mut functions = HashSet::new();
    while cursor < bytes.len() {
        let start = cursor;
        let entry = decode_entry(bytes, &mut cursor, strings, type_count)?;
        if !functions.insert(entry.function_index) {
            return Err(malformed(start, "duplicate GPU entry function index"));
        }
        entries
            .try_reserve(1)
            .map_err(|_| malformed(start, "GPU entry allocation failed"))?;
        entries.push(entry);
    }
    Ok(entries)
}

fn decode_entry(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    type_count: u64,
) -> Result<GpuEntryRecord, FormatError> {
    let function_index = read_index(bytes, cursor, "GPU entry function index")?;
    let kind_offset = *cursor;
    let kind = GpuEntryKind::decode(read_byte(bytes, cursor, "GPU entry kind")?, kind_offset)?;
    if kind == GpuEntryKind::Visible {
        let type_id = read_u64(bytes, cursor, "visible GPU entry type id")?;
        return Ok(empty_entry(function_index, kind, Some(type_id)));
    }

    let threads = [
        read_index(bytes, cursor, "GPU thread count")?,
        read_index(bytes, cursor, "GPU thread count")?,
        read_index(bytes, cursor, "GPU thread count")?,
    ];
    validate_threads(kind, threads, *cursor)?;

    let root_size = read_index(bytes, cursor, "GPU root size")?;
    let root_alignment_log2 = read_byte(bytes, cursor, "GPU root alignment")?;
    let root_type_id = read_u64(bytes, cursor, "GPU root type id")?;
    let root_space = read_byte(bytes, cursor, "GPU root address space")?;
    let root = decode_root(
        root_size,
        root_alignment_log2,
        root_type_id,
        root_space,
        *cursor,
    )?;

    let immediate_size = read_index(bytes, cursor, "GPU immediate size")?;
    let immediate_type_id = read_u64(bytes, cursor, "GPU immediate type id")?;
    if immediate_size > 256 || (immediate_size == 0 && immediate_type_id != 0) {
        return Err(malformed(*cursor, "invalid GPU immediate record"));
    }

    let stage_input = read_stage_fields(bytes, cursor, type_count, "GPU stage input")?;
    let stage_output = read_stage_fields(bytes, cursor, type_count, "GPU stage output")?;
    validate_stage_fields(kind, &stage_input, &stage_output, *cursor)?;

    let payload_sizes = [
        read_index(bytes, cursor, "GPU object payload size")?,
        read_index(bytes, cursor, "GPU ray payload size")?,
        read_index(bytes, cursor, "GPU hit attribute size")?,
        read_index(bytes, cursor, "GPU callable data size")?,
    ];
    let mesh_maximum = [
        read_index(bytes, cursor, "GPU mesh vertex limit")?,
        read_index(bytes, cursor, "GPU mesh primitive limit")?,
    ];
    let mesh_topology_offset = *cursor;
    let mesh_topology = read_byte(bytes, cursor, "GPU mesh topology")?;
    validate_mesh(kind, mesh_maximum, mesh_topology, mesh_topology_offset)?;

    let threadgroup_memory = read_index(bytes, cursor, "GPU threadgroup memory size")?;
    let capabilities = read_index(bytes, cursor, "GPU capabilities")?;
    if capabilities >> 19 != 0 {
        return Err(malformed(*cursor, "GPU capability set has unknown bits"));
    }
    let subgroup_offset = *cursor;
    let subgroup_size_log2 = read_byte(bytes, cursor, "GPU subgroup size")?;
    if subgroup_size_log2 != 0 && !(2..=7).contains(&subgroup_size_log2) {
        return Err(malformed(
            subgroup_offset,
            "invalid GPU subgroup size exponent",
        ));
    }
    let floating_point_offset = *cursor;
    let floating_point_flags = read_byte(bytes, cursor, "GPU floating-point flags")?;
    if floating_point_flags & !3 != 0 {
        return Err(malformed(
            floating_point_offset,
            "reserved GPU floating-point flags are set",
        ));
    }
    let specialization_constants = read_specialization_constants(bytes, cursor, type_count)?;

    let node = if kind == GpuEntryKind::Node {
        Some(read_node(bytes, cursor, strings)?)
    } else {
        None
    };
    let ray_recursion_depth = if kind.is_ray_tracing() {
        let offset = *cursor;
        let depth = read_byte(bytes, cursor, "GPU ray recursion depth")?;
        if depth > 31 {
            return Err(malformed(offset, "GPU ray recursion depth exceeds 31"));
        }
        Some(depth)
    } else {
        None
    };
    let fragment_flags = if kind == GpuEntryKind::Fragment {
        let offset = *cursor;
        let flags = read_byte(bytes, cursor, "GPU fragment flags")?;
        validate_fragment_flags(flags, offset)?;
        Some(flags)
    } else {
        None
    };

    Ok(GpuEntryRecord {
        function_index,
        kind,
        visible_type_id: None,
        threads,
        root,
        immediate_size,
        immediate_type_id,
        stage_input,
        stage_output,
        payload_sizes,
        mesh_maximum,
        mesh_topology,
        threadgroup_memory,
        capabilities,
        subgroup_size_log2,
        floating_point_flags,
        specialization_constants,
        node,
        ray_recursion_depth,
        fragment_flags,
    })
}

fn empty_entry(function_index: u64, kind: GpuEntryKind, type_id: Option<u64>) -> GpuEntryRecord {
    GpuEntryRecord {
        function_index,
        kind,
        visible_type_id: type_id,
        threads: [0; 3],
        root: None,
        immediate_size: 0,
        immediate_type_id: 0,
        stage_input: Vec::new(),
        stage_output: Vec::new(),
        payload_sizes: [0; 4],
        mesh_maximum: [0; 2],
        mesh_topology: 0,
        threadgroup_memory: 0,
        capabilities: 0,
        subgroup_size_log2: 0,
        floating_point_flags: 0,
        specialization_constants: Vec::new(),
        node: None,
        ray_recursion_depth: None,
        fragment_flags: None,
    }
}

fn decode_root(
    size: u64,
    alignment_log2: u8,
    type_id: u64,
    address_space: u8,
    offset: usize,
) -> Result<Option<GpuRootRecord>, FormatError> {
    if size == 0 {
        if alignment_log2 != 0 || type_id != 0 || address_space != 0 {
            return Err(malformed(offset, "absent GPU root record is not zeroed"));
        }
        return Ok(None);
    }
    if alignment_log2 > 63 || !matches!(address_space, 1 | 2) {
        return Err(malformed(offset, "invalid GPU root record"));
    }
    Ok(Some(GpuRootRecord {
        size,
        alignment_log2,
        type_id,
        address_space,
    }))
}

fn read_stage_fields(
    bytes: &[u8],
    cursor: &mut usize,
    type_count: u64,
    name: &str,
) -> Result<Vec<GpuStageField>, FormatError> {
    let count = read_index(bytes, cursor, name)?;
    if count > bytes.len().saturating_sub(*cursor) as u64 / 5 {
        return Err(malformed(
            *cursor,
            format!("{name} count exceeds the section size"),
        ));
    }
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(*cursor, format!("{name} count does not fit in memory")))?;
    let mut fields = Vec::new();
    fields
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(*cursor, format!("{name} allocation failed")))?;
    let mut seen = HashSet::new();
    for _ in 0..count {
        let start = *cursor;
        let semantic = read_byte(bytes, cursor, "GPU stage semantic")?;
        if semantic > 17 {
            return Err(malformed(start, "unknown GPU stage semantic"));
        }
        let location = read_index(bytes, cursor, "GPU stage location")?;
        let type_index = read_index(bytes, cursor, "GPU stage type index")?;
        if type_index >= type_count {
            return Err(malformed(*cursor, "GPU stage type index is out of range"));
        }
        let interpolation = read_byte(bytes, cursor, "GPU interpolation")?;
        if interpolation > 6 {
            return Err(malformed(*cursor - 1, "unknown GPU interpolation code"));
        }
        let byte_offset = read_index(bytes, cursor, "GPU stage byte offset")?;
        if !seen.insert((semantic, location)) {
            return Err(malformed(
                start,
                "duplicate GPU stage semantic and location",
            ));
        }
        fields.push(GpuStageField {
            semantic,
            location,
            type_index,
            interpolation,
            byte_offset,
        });
    }
    Ok(fields)
}

fn validate_stage_fields(
    kind: GpuEntryKind,
    inputs: &[GpuStageField],
    outputs: &[GpuStageField],
    offset: usize,
) -> Result<(), FormatError> {
    if kind != GpuEntryKind::Fragment && !inputs.is_empty() {
        return Err(malformed(
            offset,
            "only fragment entries can declare stage inputs",
        ));
    }
    if !matches!(
        kind,
        GpuEntryKind::Vertex | GpuEntryKind::Mesh | GpuEntryKind::Fragment
    ) && !outputs.is_empty()
    {
        return Err(malformed(
            offset,
            "this GPU entry kind cannot declare stage outputs",
        ));
    }
    if kind == GpuEntryKind::Vertex && !outputs.iter().any(|field| field.semantic == 1) {
        return Err(malformed(
            offset,
            "vertex entry has no position stage output",
        ));
    }
    if inputs.iter().any(|field| field.semantic == 17) {
        return Err(malformed(offset, "shading-rate semantic is output-only"));
    }
    Ok(())
}

fn validate_threads(
    kind: GpuEntryKind,
    threads: [u64; 3],
    offset: usize,
) -> Result<(), FormatError> {
    if !kind.has_threads() && threads != [0; 3] {
        return Err(malformed(
            offset,
            "non-threaded GPU entry has a threadgroup size",
        ));
    }
    if kind.has_threads() && threads == [0; 3] && kind != GpuEntryKind::Compute {
        return Err(malformed(offset, "GPU entry requires fixed thread counts"));
    }
    if kind.has_threads() && threads != [0; 3] && threads.contains(&0) {
        return Err(malformed(
            offset,
            "fixed GPU thread counts must all be positive",
        ));
    }
    if kind == GpuEntryKind::Tile && threads != [0; 3] && threads[2] != 1 {
        return Err(malformed(
            offset,
            "tile entry requires a z thread count of one",
        ));
    }
    Ok(())
}

fn validate_mesh(
    kind: GpuEntryKind,
    maximum: [u64; 2],
    topology: u8,
    offset: usize,
) -> Result<(), FormatError> {
    if topology > 2 {
        return Err(malformed(offset, "invalid GPU mesh topology"));
    }
    if kind == GpuEntryKind::Mesh {
        if !(1..=256).contains(&maximum[0]) || !(1..=512).contains(&maximum[1]) {
            return Err(malformed(offset, "GPU mesh limits are out of range"));
        }
    } else if maximum != [0; 2] || topology != 0 {
        return Err(malformed(offset, "non-mesh entry has mesh metadata"));
    }
    Ok(())
}

fn validate_fragment_flags(flags: u8, offset: usize) -> Result<(), FormatError> {
    if flags & !0x3f != 0 {
        return Err(malformed(offset, "reserved GPU fragment flags are set"));
    }
    let has_depth = flags & 2 != 0;
    let depth_mode = (flags >> 2) & 3;
    if has_depth != (depth_mode != 0) {
        return Err(malformed(offset, "GPU fragment depth flags disagree"));
    }
    Ok(())
}

fn read_specialization_constants(
    bytes: &[u8],
    cursor: &mut usize,
    type_count: u64,
) -> Result<Vec<GpuSpecializationConstant>, FormatError> {
    let count = read_index(bytes, cursor, "GPU specialization constant count")?;
    if count > bytes.len().saturating_sub(*cursor) as u64 / 3 {
        return Err(malformed(
            *cursor,
            "GPU specialization constant count exceeds the section size",
        ));
    }
    let capacity = usize::try_from(count).map_err(|_| {
        malformed(
            *cursor,
            "GPU specialization constant count does not fit in memory",
        )
    })?;
    let mut constants = Vec::new();
    constants
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(*cursor, "GPU specialization constant allocation failed"))?;
    let mut ids = HashSet::new();
    for _ in 0..count {
        let id_offset = *cursor;
        let id = read_index(bytes, cursor, "GPU specialization constant id")?;
        let id = u32::try_from(id)
            .map_err(|_| malformed(id_offset, "GPU specialization constant id exceeds 32 bits"))?;
        let type_index = read_index(bytes, cursor, "GPU specialization constant type index")?;
        if type_index >= type_count {
            return Err(malformed(
                *cursor,
                "GPU specialization constant type index is out of range",
            ));
        }
        let default_constant = read_index(bytes, cursor, "GPU specialization constant default")?;
        if !ids.insert(id) {
            return Err(malformed(
                id_offset,
                "duplicate GPU specialization constant id",
            ));
        }
        constants.push(GpuSpecializationConstant {
            id,
            type_index,
            default_constant,
        });
    }
    Ok(constants)
}

fn read_node(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<GpuNodeRecord, FormatError> {
    let launch_offset = *cursor;
    let launch_mode = read_byte(bytes, cursor, "GPU node launch mode")?;
    if launch_mode > 2 {
        return Err(malformed(launch_offset, "invalid GPU node launch mode"));
    }
    let maximum_dispatch_grid = [
        read_index(bytes, cursor, "GPU node dispatch grid")?,
        read_index(bytes, cursor, "GPU node dispatch grid")?,
        read_index(bytes, cursor, "GPU node dispatch grid")?,
    ];
    let input_record_size = read_index(bytes, cursor, "GPU node input record size")?;
    let output_count = read_index(bytes, cursor, "GPU node output count")?;
    if output_count > bytes.len().saturating_sub(*cursor) as u64 / 3 {
        return Err(malformed(
            *cursor,
            "GPU node output count exceeds the section size",
        ));
    }
    let capacity = usize::try_from(output_count)
        .map_err(|_| malformed(*cursor, "GPU node output count does not fit in memory"))?;
    let mut outputs = Vec::new();
    outputs
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(*cursor, "GPU node output allocation failed"))?;
    let mut names = HashSet::new();
    for _ in 0..output_count {
        let name_offset = *cursor;
        let name_index = read_index(bytes, cursor, "GPU node output name")?;
        let name = usize::try_from(name_index)
            .ok()
            .and_then(|index| strings.get(index))
            .ok_or_else(|| malformed(name_offset, "GPU node output name is out of range"))?
            .to_owned();
        let record_size = read_index(bytes, cursor, "GPU node output record size")?;
        let maximum_records = read_index(bytes, cursor, "GPU node output maximum records")?;
        if !names.insert(name.clone()) {
            return Err(malformed(name_offset, "duplicate GPU node output name"));
        }
        outputs.push(GpuNodeOutput {
            name,
            record_size,
            maximum_records,
        });
    }
    Ok(GpuNodeRecord {
        launch_mode,
        maximum_dispatch_grid,
        input_record_size,
        outputs,
    })
}

fn read_index(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid {field} ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u8, FormatError> {
    let offset = *cursor;
    let byte = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(byte)
}

fn read_u64(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let start = *cursor;
    let end = start
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(start, format!("truncated {field}")))?;
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[start..end]);
    *cursor = end;
    Ok(u64::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
