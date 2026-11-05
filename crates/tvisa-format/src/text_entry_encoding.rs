use tir_diag::Span;
use tir_format::StringTable;
use tir_format::numeric_literals::parse_unsigned_integer;
use tir_types::Type;

use crate::gpu_entries::{
    GpuEntryKind, GpuEntryRecord, GpuNodeOutput, GpuNodeRecord, GpuRootRecord,
    GpuSpecializationConstant, GpuStageField, encode_gpu_entry_records,
};
use crate::text::{TextParseError, error};
use crate::text_entries::{TextEntryField, TextEntryValue, TextEntryValueKind, TextGpuEntry};
use crate::text_entry_validation::CAPABILITIES;

/// Resolves textual GPU entry references against the containing image's tables.
pub trait TextGpuEntryResolver {
    fn function_index(&self, symbol: &str, kind: GpuEntryKind) -> Option<u64>;
    fn value_type_index(&self, ty: &Type) -> Option<u64>;
    fn specialization_constant_index(&self, ty: &Type, default: &TextEntryValue) -> Option<u64>;
}

/// Encodes parsed entries after their function, type, constant, and string references are resolved.
pub fn encode_text_gpu_entry_records(
    entries: &[TextGpuEntry],
    resolver: &impl TextGpuEntryResolver,
    strings: &StringTable,
    value_type_count: u64,
) -> Result<Vec<u8>, TextParseError> {
    let records = entries
        .iter()
        .map(|entry| resolve_entry(entry, resolver, strings))
        .collect::<Result<Vec<_>, _>>()?;
    encode_gpu_entry_records(&records, strings, value_type_count).map_err(|failure| {
        error(
            entries.first().map_or(Span::new(0, 0), |entry| entry.span),
            &failure.to_string(),
        )
    })
}

fn resolve_entry(
    entry: &TextGpuEntry,
    resolver: &impl TextGpuEntryResolver,
    strings: &StringTable,
) -> Result<GpuEntryRecord, TextParseError> {
    let function_index = resolver
        .function_index(&entry.function, entry.kind)
        .ok_or_else(|| {
            error(
                entry.span,
                "GPU entry function is not in the function table",
            )
        })?;
    if entry.kind == GpuEntryKind::Visible {
        return Ok(empty_record(
            function_index,
            entry.kind,
            Some(hex(field(entry, "typeid")?)?),
        ));
    }

    let threads = three_unsigned(field(entry, "threads")?)?;
    let root = optional_object(entry, "root")?
        .map(|fields| {
            let alignment_value = member(fields, "align_log2")?;
            let alignment_log2 = u8::try_from(unsigned(alignment_value)?)
                .map_err(|_| error(alignment_value.span, "root alignment is out of range"))?;
            Ok::<_, TextParseError>(GpuRootRecord {
                size: unsigned(member(fields, "size")?)?,
                alignment_log2,
                type_id: hex(member(fields, "typeid")?)?,
                address_space: match atom(member(fields, "space")?)? {
                    "device" => 1,
                    "constant" => 2,
                    _ => {
                        return Err(error(
                            member(fields, "space")?.span,
                            "unknown root address space",
                        ));
                    }
                },
            })
        })
        .transpose()?;
    let immediate = optional_object(entry, "immediate")?;
    let (immediate_size, immediate_type_id) = if let Some(fields) = immediate {
        (
            unsigned(member(fields, "size")?)?,
            hex(member(fields, "typeid")?)?,
        )
    } else {
        (0, 0)
    };
    let stage_input = resolve_stage_fields(entry, "stage_input", resolver)?;
    let stage_output = resolve_stage_fields(entry, "stage_output", resolver)?;
    let payload_sizes = optional_object(entry, "payload")?
        .map(|fields| {
            Ok::<_, TextParseError>([
                unsigned(member(fields, "object")?)?,
                unsigned(member(fields, "ray")?)?,
                unsigned(member(fields, "hit_attribute")?)?,
                unsigned(member(fields, "callable")?)?,
            ])
        })
        .transpose()?
        .unwrap_or([0; 4]);
    let (mesh_maximum, mesh_topology) = if let Some(fields) = optional_object(entry, "mesh")? {
        let topology = match atom(member(fields, "topology")?)? {
            "points" => 0,
            "lines" => 1,
            "triangles" => 2,
            _ => {
                return Err(error(
                    member(fields, "topology")?.span,
                    "unknown mesh topology",
                ));
            }
        };
        (
            [
                unsigned(member(fields, "max_vertices")?)?,
                unsigned(member(fields, "max_primitives")?)?,
            ],
            topology,
        )
    } else {
        ([0; 2], 0)
    };
    let threadgroup_memory = unsigned(field(entry, "threadgroup_memory")?)?;
    let capabilities = capability_bits(field(entry, "capabilities")?)?;
    let subgroup_size_log2 = subgroup_log2(field(entry, "subgroup_size")?)?;
    let floating_point_flags = floating_point_bits(field(entry, "floating_point")?)?;
    let specialization_constants = resolve_specialization_constants(entry, resolver)?;
    let node = optional_object(entry, "node")?
        .map(|fields| resolve_node(fields, strings))
        .transpose()?;
    let ray_recursion_depth = optional_object(entry, "ray_tracing")?
        .map(|fields| {
            let depth_value = member(fields, "max_recursion_depth")?;
            u8::try_from(unsigned(depth_value)?)
                .map_err(|_| error(depth_value.span, "ray recursion depth is out of range"))
        })
        .transpose()?;
    let fragment_flags = if entry.kind == GpuEntryKind::Fragment {
        Some(resolve_fragment(entry)?)
    } else {
        None
    };

    Ok(GpuEntryRecord {
        function_index,
        kind: entry.kind,
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

fn empty_record(
    function_index: u64,
    kind: GpuEntryKind,
    visible_type_id: Option<u64>,
) -> GpuEntryRecord {
    GpuEntryRecord {
        function_index,
        kind,
        visible_type_id,
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

fn resolve_stage_fields(
    entry: &TextGpuEntry,
    name: &str,
    resolver: &impl TextGpuEntryResolver,
) -> Result<Vec<GpuStageField>, TextParseError> {
    array(field(entry, name)?)?
        .iter()
        .map(|value| {
            let fields = object(value)?;
            let semantic_value = member(fields, "semantic")?;
            let (semantic, semantic_index) = resolve_semantic(semantic_value)?;
            let location = unsigned(member(fields, "location")?)?;
            if semantic_index.is_some_and(|index| index != location) {
                return Err(error(
                    semantic_value.span,
                    "parameterized stage semantic index must equal its location",
                ));
            }
            let type_value = member(fields, "type")?;
            let TextEntryValueKind::Type(ty) = &type_value.kind else {
                return Err(error(type_value.span, "expected TVISA type"));
            };
            let type_index = resolver.value_type_index(ty).ok_or_else(|| {
                error(type_value.span, "stage value type is not in the type table")
            })?;
            let modes = array(member(fields, "interpolation")?)?;
            if modes.len() > 1 {
                return Err(error(
                    member(fields, "interpolation")?.span,
                    "binary TVISA stores one interpolation code per stage field",
                ));
            }
            let interpolation = modes.first().map_or(Ok(0), |mode| {
                Ok(match atom(mode)? {
                    "flat" => 0,
                    "perspective" => 1,
                    "noperspective" => 2,
                    "centroid" => 3,
                    "sample" => 4,
                    "invariant" => 5,
                    "per_primitive" => 6,
                    _ => return Err(error(mode.span, "unknown interpolation mode")),
                })
            })?;
            Ok(GpuStageField {
                semantic,
                location,
                type_index,
                interpolation,
                byte_offset: unsigned(member(fields, "offset")?)?,
            })
        })
        .collect()
}

fn resolve_semantic(value: &TextEntryValue) -> Result<(u8, Option<u64>), TextParseError> {
    let (name, index) = match &value.kind {
        TextEntryValueKind::Atom(name) => (name.as_str(), None),
        TextEntryValueKind::Call(name, args) if name == "depth" && args.len() == 1 => {
            (name.as_str(), None)
        }
        TextEntryValueKind::Call(name, args) if args.len() == 1 => {
            (name.as_str(), Some(unsigned(&args[0])?))
        }
        _ => return Err(error(value.span, "invalid stage semantic")),
    };
    let code = match name {
        "user" if index.is_none() => 0,
        "position" if index.is_none() => 1,
        "color" => 2,
        "color_index1" => 3,
        "depth" => match value_call_atom(value)? {
            "any" => 4,
            "greater" => 5,
            "less" => 6,
            _ => return Err(error(value.span, "unknown depth semantic")),
        },
        "stencil" if index.is_none() => 7,
        "sample_mask" if index.is_none() => 8,
        "clip_distance" => 9,
        "cull_distance" => 10,
        "point_size" if index.is_none() => 11,
        "layer" if index.is_none() => 12,
        "viewport" if index.is_none() => 13,
        "primitive_id" if index.is_none() => 14,
        "view_index" if index.is_none() => 15,
        "cull" if index.is_none() => 16,
        "shading_rate" if index.is_none() => 17,
        _ => return Err(error(value.span, "unknown GPU stage semantic")),
    };
    Ok((code, index))
}

fn value_call_atom(value: &TextEntryValue) -> Result<&str, TextParseError> {
    let TextEntryValueKind::Call(_, args) = &value.kind else {
        return Err(error(value.span, "expected semantic argument"));
    };
    args.first()
        .map(atom)
        .transpose()?
        .ok_or_else(|| error(value.span, "expected semantic argument"))
}

fn resolve_specialization_constants(
    entry: &TextGpuEntry,
    resolver: &impl TextGpuEntryResolver,
) -> Result<Vec<GpuSpecializationConstant>, TextParseError> {
    array(field(entry, "specialization_constants")?)?
        .iter()
        .map(|value| {
            let fields = object(value)?;
            let id_value = member(fields, "id")?;
            let id = u32::try_from(unsigned(id_value)?)
                .map_err(|_| error(id_value.span, "specialization id is out of range"))?;
            let type_value = member(fields, "type")?;
            let TextEntryValueKind::Type(ty) = &type_value.kind else {
                return Err(error(type_value.span, "expected TVISA type"));
            };
            let type_index = resolver.value_type_index(ty).ok_or_else(|| {
                error(
                    type_value.span,
                    "specialization type is not in the type table",
                )
            })?;
            let default = member(fields, "default")?;
            let default_constant = resolver
                .specialization_constant_index(ty, default)
                .ok_or_else(|| {
                    error(
                        default.span,
                        "specialization default is not in the constant table",
                    )
                })?;
            Ok(GpuSpecializationConstant {
                id,
                type_index,
                default_constant,
            })
        })
        .collect()
}

fn resolve_node(
    fields: &[TextEntryField],
    strings: &StringTable,
) -> Result<GpuNodeRecord, TextParseError> {
    let launch_mode = match atom(member(fields, "launch")?)? {
        "broadcast" => 0,
        "coalescing" => 1,
        "thread" => 2,
        _ => {
            return Err(error(
                member(fields, "launch")?.span,
                "unknown node launch mode",
            ));
        }
    };
    let maximum_dispatch_grid = three_unsigned(member(fields, "max_dispatch_grid")?)?;
    let input_record_size = unsigned(member(fields, "input_record_size")?)?;
    let outputs = array(member(fields, "outputs")?)?
        .iter()
        .map(|value| {
            let members = object(value)?;
            let name_value = member(members, "name")?;
            let name = string(name_value)?;
            if strings.index_of(name).is_none() {
                return Err(error(
                    name_value.span,
                    "node output name is missing from the string table",
                ));
            }
            Ok(GpuNodeOutput {
                name: name.to_owned(),
                record_size: unsigned(member(members, "record_size")?)?,
                maximum_records: unsigned(member(members, "max_records")?)?,
            })
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    Ok(GpuNodeRecord {
        launch_mode,
        maximum_dispatch_grid,
        input_record_size,
        outputs,
    })
}

fn resolve_fragment(entry: &TextGpuEntry) -> Result<u8, TextParseError> {
    let Some(fields) = optional_object(entry, "fragment")? else {
        return Ok(0);
    };
    let mut flags = 0;
    if atom(member(fields, "early_fragment_tests")?)? == "true" {
        flags |= 1;
    }
    match atom(member(fields, "writes_depth")?)? {
        "none" => {}
        "any" => flags |= 2 | (1 << 2),
        "greater" => flags |= 2 | (2 << 2),
        "less" => flags |= 2 | (3 << 2),
        _ => {
            return Err(error(
                member(fields, "writes_depth")?.span,
                "unknown depth-write mode",
            ));
        }
    }
    if atom(member(fields, "discards_or_demotes")?)? == "true" {
        flags |= 1 << 4;
    }
    if atom(member(fields, "reads_attachments")?)? == "true" {
        flags |= 1 << 5;
    }
    Ok(flags)
}

fn capability_bits(value: &TextEntryValue) -> Result<u64, TextParseError> {
    let mut bits = 0_u64;
    for value in array(value)? {
        let name = atom(value)?;
        let index = CAPABILITIES
            .iter()
            .position(|candidate| *candidate == name)
            .ok_or_else(|| error(value.span, "unknown optional GPU capability"))?;
        bits |= 1 << index;
    }
    Ok(bits)
}

fn subgroup_log2(value: &TextEntryValue) -> Result<u8, TextParseError> {
    let size = match atom(value)? {
        "any" => return Ok(0),
        literal => parse_unsigned_integer(literal)
            .and_then(|number| u64::try_from(number).ok())
            .ok_or_else(|| error(value.span, "invalid subgroup size"))?,
    };
    u8::try_from(size.trailing_zeros())
        .map_err(|_| error(value.span, "subgroup size is out of range"))
}

fn floating_point_bits(value: &TextEntryValue) -> Result<u8, TextParseError> {
    let mut bits = 0;
    for value in array(value)? {
        match atom(value)? {
            "fast" => bits |= 1,
            "ieee_denormals" => bits |= 2,
            _ => return Err(error(value.span, "unknown floating-point mode")),
        }
    }
    Ok(bits)
}

fn three_unsigned(value: &TextEntryValue) -> Result<[u64; 3], TextParseError> {
    let values = array(value)?;
    if values.len() != 3 {
        return Err(error(value.span, "expected three unsigned values"));
    }
    Ok([
        unsigned(&values[0])?,
        unsigned(&values[1])?,
        unsigned(&values[2])?,
    ])
}

fn optional_object<'a>(
    entry: &'a TextGpuEntry,
    name: &str,
) -> Result<Option<&'a [TextEntryField]>, TextParseError> {
    let value = field(entry, name)?;
    if atom(value).is_ok_and(|name| name == "none") {
        return Ok(None);
    }
    Ok(Some(object(value)?))
}

fn field<'a>(entry: &'a TextGpuEntry, name: &str) -> Result<&'a TextEntryValue, TextParseError> {
    entry
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| error(entry.span, "missing GPU entry field"))
}

fn member<'a>(
    fields: &'a [TextEntryField],
    name: &str,
) -> Result<&'a TextEntryValue, TextParseError> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
        .ok_or_else(|| error(Span::new(0, 0), "missing nested GPU entry field"))
}

fn object(value: &TextEntryValue) -> Result<&[TextEntryField], TextParseError> {
    if let TextEntryValueKind::Object(fields) = &value.kind {
        Ok(fields)
    } else {
        Err(error(value.span, "expected GPU entry object"))
    }
}

fn array(value: &TextEntryValue) -> Result<&[TextEntryValue], TextParseError> {
    if let TextEntryValueKind::Array(values) = &value.kind {
        Ok(values)
    } else {
        Err(error(value.span, "expected GPU entry array"))
    }
}

fn atom(value: &TextEntryValue) -> Result<&str, TextParseError> {
    if let TextEntryValueKind::Atom(atom) = &value.kind {
        Ok(atom)
    } else {
        Err(error(value.span, "expected scalar literal"))
    }
}

fn string(value: &TextEntryValue) -> Result<&str, TextParseError> {
    if let TextEntryValueKind::String(string) = &value.kind {
        Ok(string)
    } else {
        Err(error(value.span, "expected string literal"))
    }
}

fn unsigned(value: &TextEntryValue) -> Result<u64, TextParseError> {
    parse_unsigned_integer(atom(value)?)
        .and_then(|number| u64::try_from(number).ok())
        .ok_or_else(|| error(value.span, "invalid unsigned integer"))
}

fn hex(value: &TextEntryValue) -> Result<u64, TextParseError> {
    let literal = atom(value)?;
    if !(literal.starts_with("0x") || literal.starts_with("0X")) {
        return Err(error(value.span, "expected hexadecimal integer"));
    }
    unsigned(value)
}
