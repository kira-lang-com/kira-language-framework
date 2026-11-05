use tir_format::leb::read_uleb;

use crate::binary::FormatError;
mod value_types;
pub use value_types::TvisaValueTypeRecord;
use value_types::{ValueTypeInfo, decode_value_type};

const MAX_SIGNATURE_ITEMS: u64 = 1 << 16;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeSection {
    pub value_type_count: u64,
    pub value_types: Vec<TvisaValueTypeRecord>,
    pub signatures: Vec<SignatureSummary>,
    pub c_aggregates: Vec<CAggregateSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureSummary {
    pub convention: u8,
    pub flags: u64,
    pub fixed_parameter_count: Option<u64>,
    pub parameter_count: u64,
    pub parameters: Vec<ParameterItemSummary>,
    pub result_count: u64,
    pub results: Vec<ResultItemSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterItemSummary {
    Leaf {
        type_index: u64,
    },
    Group(GroupSummary),
    ByRef {
        size: u64,
        alignment: u64,
        flags: u8,
    },
    SRet {
        size: u64,
        alignment: u64,
    },
    Context,
    CAggregate {
        index: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultItemSummary {
    Leaf { type_index: u64 },
    Group(GroupSummary),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupSummary {
    pub size: u64,
    pub alignment: u64,
    pub members: Vec<GroupMemberSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupMemberSummary {
    pub offset: u64,
    pub type_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CAggregateSummary {
    pub size: u64,
    pub alignment: u64,
    pub flags: u8,
    pub member_count: u64,
    pub members: Vec<CAggregateMemberSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CAggregateMemberSummary {
    pub offset: u64,
    pub type_index: u64,
    pub bit_width: u64,
}

impl TypeSection {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut cursor = 0;
        let value_type_count = read_index(bytes, &mut cursor, "TYPES type count")?;
        check_count(
            value_type_count,
            bytes.len().saturating_sub(cursor),
            cursor,
            "type",
        )?;
        let type_capacity = usize::try_from(value_type_count)
            .map_err(|_| malformed(cursor, "TYPES type count does not fit in memory"))?;
        let mut value_types = Vec::new();
        value_types
            .try_reserve_exact(type_capacity)
            .map_err(|_| malformed(cursor, "TYPES type allocation failed"))?;
        let mut value_type_records = Vec::new();
        value_type_records
            .try_reserve_exact(type_capacity)
            .map_err(|_| malformed(cursor, "TYPES type record allocation failed"))?;
        let mut max_signature_reference = None;
        for type_index in 0..value_type_count {
            let tag = read_byte(bytes, &mut cursor, "TYPES value type")?;
            let (value_type, record) = decode_value_type(
                bytes,
                &mut cursor,
                type_index,
                tag,
                &value_types,
                &mut max_signature_reference,
            )?;
            value_types.push(value_type);
            value_type_records.push(record);
        }

        let signature_count = read_index(bytes, &mut cursor, "TYPES signature count")?;
        check_count(
            signature_count,
            bytes.len().saturating_sub(cursor),
            cursor,
            "signature",
        )?;
        let signature_capacity = usize::try_from(signature_count)
            .map_err(|_| malformed(cursor, "TYPES signature count does not fit in memory"))?;
        let mut signatures = Vec::new();
        signatures
            .try_reserve_exact(signature_capacity)
            .map_err(|_| malformed(cursor, "TYPES signature allocation failed"))?;
        let mut max_aggregate_reference = None;
        for _ in 0..signature_count {
            signatures.push(decode_signature(
                bytes,
                &mut cursor,
                &value_types,
                &mut max_aggregate_reference,
            )?);
        }

        let aggregate_count = read_index(bytes, &mut cursor, "TYPES C aggregate count")?;
        check_count(
            aggregate_count,
            bytes.len().saturating_sub(cursor) / 4,
            cursor,
            "C aggregate",
        )?;
        let aggregate_capacity = usize::try_from(aggregate_count)
            .map_err(|_| malformed(cursor, "TYPES C aggregate count does not fit in memory"))?;
        let mut c_aggregates = Vec::new();
        c_aggregates
            .try_reserve_exact(aggregate_capacity)
            .map_err(|_| malformed(cursor, "TYPES C aggregate allocation failed"))?;
        for _ in 0..aggregate_count {
            c_aggregates.push(decode_c_aggregate(bytes, &mut cursor, &value_types)?);
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in TYPES section"));
        }
        if max_signature_reference.is_some_and(|index| index >= signature_count) {
            return Err(malformed(
                cursor,
                "value type references an invalid signature index",
            ));
        }
        if max_aggregate_reference.is_some_and(|index| index >= aggregate_count) {
            return Err(malformed(
                cursor,
                "signature references an invalid C aggregate index",
            ));
        }
        Ok(Self {
            value_type_count,
            value_types: value_type_records,
            signatures,
            c_aggregates,
        })
    }
}

fn decode_signature(
    bytes: &[u8],
    cursor: &mut usize,
    value_types: &[ValueTypeInfo],
    max_aggregate_reference: &mut Option<u64>,
) -> Result<SignatureSummary, FormatError> {
    let convention_offset = *cursor;
    let convention = read_byte(bytes, cursor, "signature convention")?;
    if convention > 4 {
        return Err(malformed(convention_offset, "invalid signature convention"));
    }
    let flags_offset = *cursor;
    let flags = read_index(bytes, cursor, "signature flags")?;
    if flags & !7 != 0 {
        return Err(malformed(flags_offset, "reserved signature flags are set"));
    }
    let fixed_parameter_count = if flags & 2 != 0 {
        Some(read_index(bytes, cursor, "fixed parameter count")?)
    } else {
        None
    };
    let parameter_count = read_index(bytes, cursor, "signature parameter count")?;
    if parameter_count >= MAX_SIGNATURE_ITEMS {
        return Err(malformed(
            *cursor,
            "signature parameter count exceeds the TVISA limit",
        ));
    }
    if fixed_parameter_count.is_some_and(|count| count > parameter_count) {
        return Err(malformed(
            *cursor,
            "variadic fixed parameter count exceeds the signature",
        ));
    }
    if fixed_parameter_count.is_some() && convention != 1 {
        return Err(malformed(
            *cursor,
            "TVISA variadic signatures must use the c convention",
        ));
    }
    let parameter_capacity = usize::try_from(parameter_count)
        .map_err(|_| malformed(*cursor, "signature parameter count does not fit in memory"))?;
    let mut parameters = Vec::new();
    parameters
        .try_reserve_exact(parameter_capacity)
        .map_err(|_| malformed(*cursor, "signature parameter allocation failed"))?;
    let mut context_count = 0;
    let mut sret_count = 0;
    let mut cagg_parameter = false;
    let mut c_has_forbidden_item = false;
    for _ in 0..parameter_count {
        let item = decode_parameter_item(bytes, cursor, value_types, max_aggregate_reference)?;
        match &item {
            ParameterItemSummary::Context => context_count += 1,
            ParameterItemSummary::SRet { .. } => sret_count += 1,
            ParameterItemSummary::CAggregate { .. } => cagg_parameter = true,
            ParameterItemSummary::Group(_) => c_has_forbidden_item = true,
            _ => {}
        }
        parameters.push(item);
    }
    let result_count = read_index(bytes, cursor, "signature result count")?;
    let result_capacity = usize::try_from(result_count)
        .map_err(|_| malformed(*cursor, "signature result count does not fit in memory"))?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(result_capacity)
        .map_err(|_| malformed(*cursor, "signature result allocation failed"))?;
    let mut result_groups = 0;
    for _ in 0..result_count {
        let item = decode_result_item(bytes, cursor, value_types)?;
        if matches!(&item, ResultItemSummary::Group(_)) {
            result_groups += 1;
        }
        results.push(item);
    }
    if context_count > 1 || sret_count > 1 {
        return Err(malformed(
            *cursor,
            "signature has duplicate context or sret items",
        ));
    }
    if context_count != 0 && convention != 0 {
        return Err(malformed(
            *cursor,
            "context items require the ridl convention",
        ));
    }
    if sret_count != 0 && result_count != 0 {
        return Err(malformed(
            *cursor,
            "sret signature cannot have result items",
        ));
    }
    if cagg_parameter && convention != 1 {
        return Err(malformed(*cursor, "cagg items require the c convention"));
    }
    if convention == 1 && c_has_forbidden_item {
        return Err(malformed(
            *cursor,
            "c signatures cannot use group or context parameter items",
        ));
    }
    if result_groups > 1 {
        return Err(malformed(
            *cursor,
            "signature has more than one result group",
        ));
    }
    Ok(SignatureSummary {
        convention,
        flags,
        fixed_parameter_count,
        parameter_count,
        parameters,
        result_count,
        results,
    })
}

fn decode_parameter_item(
    bytes: &[u8],
    cursor: &mut usize,
    value_types: &[ValueTypeInfo],
    max_aggregate_reference: &mut Option<u64>,
) -> Result<ParameterItemSummary, FormatError> {
    let kind_offset = *cursor;
    let kind = read_byte(bytes, cursor, "signature parameter item")?;
    match kind {
        0 => {
            let type_offset = *cursor;
            let type_index = read_index(bytes, cursor, "leaf type index")?;
            check_type_index(type_index, value_types.len() as u64, type_offset)?;
            Ok(ParameterItemSummary::Leaf { type_index })
        }
        1 => Ok(ParameterItemSummary::Group(decode_group(
            bytes,
            cursor,
            value_types,
        )?)),
        2 => {
            let (size, alignment) = decode_sized_item(bytes, cursor)?;
            let flags_offset = *cursor;
            let flags = read_byte(bytes, cursor, "byref flags")?;
            if flags & !3 != 0 {
                return Err(malformed(flags_offset, "reserved byref flags are set"));
            }
            Ok(ParameterItemSummary::ByRef {
                size,
                alignment,
                flags,
            })
        }
        3 => {
            let (size, alignment) = decode_sized_item(bytes, cursor)?;
            Ok(ParameterItemSummary::SRet { size, alignment })
        }
        4 => Ok(ParameterItemSummary::Context),
        5 => {
            let index = read_index(bytes, cursor, "C aggregate index")?;
            note_max(max_aggregate_reference, index);
            Ok(ParameterItemSummary::CAggregate { index })
        }
        _ => return Err(malformed(kind_offset, "invalid signature parameter item")),
    }
}

fn decode_result_item(
    bytes: &[u8],
    cursor: &mut usize,
    value_types: &[ValueTypeInfo],
) -> Result<ResultItemSummary, FormatError> {
    let kind_offset = *cursor;
    let kind = read_byte(bytes, cursor, "signature result item")?;
    match kind {
        0 => {
            let type_offset = *cursor;
            let type_index = read_index(bytes, cursor, "result type index")?;
            check_type_index(type_index, value_types.len() as u64, type_offset)?;
            Ok(ResultItemSummary::Leaf { type_index })
        }
        1 => Ok(ResultItemSummary::Group(decode_group(
            bytes,
            cursor,
            value_types,
        )?)),
        _ => return Err(malformed(kind_offset, "invalid signature result item")),
    }
}

fn decode_group(
    bytes: &[u8],
    cursor: &mut usize,
    value_types: &[ValueTypeInfo],
) -> Result<GroupSummary, FormatError> {
    let size_offset = *cursor;
    let size = read_index(bytes, cursor, "signature group size")?;
    if size == 0 {
        return Err(malformed(
            size_offset,
            "signature group size must be nonzero",
        ));
    }
    let alignment = read_alignment(bytes, cursor, "signature group alignment")?;
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, "signature group member count")?;
    if !(2..=4).contains(&count) {
        return Err(malformed(
            count_offset,
            "signature group requires 2 to 4 members",
        ));
    }
    let member_capacity = usize::try_from(count).map_err(|_| {
        malformed(
            *cursor,
            "signature group member count does not fit in memory",
        )
    })?;
    let mut members = Vec::new();
    members
        .try_reserve_exact(member_capacity)
        .map_err(|_| malformed(*cursor, "signature group member allocation failed"))?;
    let mut previous_end = None;
    for _ in 0..count {
        let member_offset_at = *cursor;
        let member_offset = read_index(bytes, cursor, "signature group member offset")?;
        let type_offset = *cursor;
        let type_index = read_index(bytes, cursor, "signature group member type")?;
        check_type_index(type_index, value_types.len() as u64, type_offset)?;
        let Some(member_size) = value_types[type_index as usize].group_member_size() else {
            return Err(malformed(
                type_offset,
                "signature group member must be a scalar or a fixed vector of at most 16 bytes",
            ));
        };
        let member_end = member_offset
            .checked_add(member_size)
            .ok_or_else(|| malformed(member_offset_at, "signature group member range overflows"))?;
        if member_end > size || previous_end.is_some_and(|end| member_offset < end) {
            return Err(malformed(
                member_offset_at,
                "signature group members overlap or lie outside the group",
            ));
        }
        previous_end = Some(member_end);
        members.push(GroupMemberSummary {
            offset: member_offset,
            type_index,
        });
    }
    Ok(GroupSummary {
        size,
        alignment,
        members,
    })
}

fn decode_sized_item(bytes: &[u8], cursor: &mut usize) -> Result<(u64, u64), FormatError> {
    let size = read_index(bytes, cursor, "signature item size")?;
    let alignment = read_alignment(bytes, cursor, "signature item alignment")?;
    Ok((size, alignment))
}

fn decode_c_aggregate(
    bytes: &[u8],
    cursor: &mut usize,
    value_types: &[ValueTypeInfo],
) -> Result<CAggregateSummary, FormatError> {
    let size = read_index(bytes, cursor, "C aggregate size")?;
    let alignment = read_alignment(bytes, cursor, "C aggregate alignment")?;
    let flags_offset = *cursor;
    let flags = read_byte(bytes, cursor, "C aggregate flags")?;
    if flags & !0x1f != 0 {
        return Err(malformed(
            flags_offset,
            "reserved C aggregate flags are set",
        ));
    }
    let count_offset = *cursor;
    let member_count = read_index(bytes, cursor, "C aggregate member count")?;
    check_count(
        member_count,
        bytes.len().saturating_sub(*cursor) / 3,
        count_offset,
        "C aggregate member",
    )?;
    let member_capacity = usize::try_from(member_count)
        .map_err(|_| malformed(*cursor, "C aggregate member count does not fit in memory"))?;
    let mut members = Vec::new();
    members
        .try_reserve_exact(member_capacity)
        .map_err(|_| malformed(*cursor, "C aggregate member allocation failed"))?;
    let mut previous_offset = None;
    let mut previous_non_bitfield_end = 0;
    let mut previous_bitfield_end = 0;
    for member_index in 0..member_count {
        let member_offset_at = *cursor;
        let member_offset = read_index(bytes, cursor, "C aggregate member offset")?;
        if member_offset > size
            || (member_offset == size && flags & 8 == 0)
            || previous_offset.is_some_and(|previous| member_offset < previous)
            || member_offset > 64
        {
            return Err(malformed(
                member_offset_at,
                "C aggregate member offsets are invalid",
            ));
        }
        previous_offset = Some(member_offset);
        let type_offset = *cursor;
        let type_index = read_index(bytes, cursor, "C aggregate member type")?;
        check_type_index(type_index, value_types.len() as u64, type_offset)?;
        let bit_width = read_index(bytes, cursor, "C aggregate bit-field width")?;
        if bit_width > u64::from(u32::MAX) {
            return Err(malformed(
                *cursor,
                "C aggregate bit-field width exceeds 32 bits",
            ));
        }
        let member_type = &value_types[type_index as usize];
        if bit_width != 0
            && (member_type.tag != 0x01
                || member_type.bit_width.is_none_or(|width| bit_width > width))
        {
            return Err(malformed(
                type_offset,
                "C aggregate bit-field must fit its integer storage type",
            ));
        }
        let flexible_tail = flags & 8 != 0
            && member_index + 1 == member_count
            && member_offset == size
            && bit_width == 0;
        let member_size = member_type
            .c_aggregate_member_size()
            .ok_or_else(|| malformed(type_offset, "C aggregate member has no fixed byte size"))?;
        let member_end = if flexible_tail {
            member_offset
        } else {
            member_offset
                .checked_add(member_size)
                .ok_or_else(|| malformed(member_offset_at, "C aggregate member range overflows"))?
        };
        if member_end > size {
            return Err(malformed(
                member_offset_at,
                "C aggregate member lies outside the aggregate",
            ));
        }
        if flags & 1 == 0 {
            let overlapped = if bit_width == 0 {
                member_offset < previous_non_bitfield_end || member_offset < previous_bitfield_end
            } else {
                member_offset < previous_non_bitfield_end
            };
            if overlapped {
                return Err(malformed(
                    member_offset_at,
                    "non-union C aggregate members overlap",
                ));
            }
        }
        if bit_width == 0 {
            previous_non_bitfield_end = previous_non_bitfield_end.max(member_end);
        } else {
            previous_bitfield_end = previous_bitfield_end.max(member_end);
        }
        members.push(CAggregateMemberSummary {
            offset: member_offset,
            type_index,
            bit_width,
        });
    }
    Ok(CAggregateSummary {
        size,
        alignment,
        flags,
        member_count,
        members,
    })
}

fn check_type_index(index: u64, count: u64, offset: usize) -> Result<(), FormatError> {
    if index >= count {
        Err(malformed(
            offset,
            "signature references an invalid value type index",
        ))
    } else {
        Ok(())
    }
}

fn note_max(maximum: &mut Option<u64>, value: u64) {
    if maximum.is_none_or(|current| current < value) {
        *maximum = Some(value);
    }
}

fn read_alignment(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    let alignment = read_index(bytes, cursor, field)?;
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(malformed(
            offset,
            format!("{field} must be a positive power of two"),
        ));
    }
    Ok(alignment)
}

fn check_count(
    count: u64,
    maximum: usize,
    offset: usize,
    description: &str,
) -> Result<(), FormatError> {
    if count > maximum as u64 {
        Err(malformed(
            offset,
            format!("TYPES {description} count exceeds the section size"),
        ))
    } else {
        Ok(())
    }
}

fn read_index(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid {field} ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
