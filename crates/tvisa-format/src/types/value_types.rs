use crate::binary::FormatError;

use super::{malformed, note_max, read_byte, read_index};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TvisaValueTypeRecord {
    Integer {
        bits: u64,
    },
    Float {
        tag: u8,
    },
    Pointer {
        address_space: u8,
    },
    Code,
    FixedVector {
        lanes: u64,
        element_type: u64,
    },
    ScalableVector {
        element_type: u64,
    },
    Texture {
        dimension: u8,
        component_type: u64,
        access: u8,
        multisampled: bool,
    },
    Sampler,
    AccelerationStructure,
    VisibleFunctionTable {
        signature: u64,
    },
    Tensor {
        rank: u64,
        element_type: u64,
    },
    RayQuery,
    HitObject,
    CooperativeMatrix {
        scope: u8,
        rows: u64,
        columns: u64,
        element_type: u64,
        use_kind: u8,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ValueTypeInfo {
    pub(super) tag: u8,
    pub(super) bit_width: Option<u64>,
    pub(super) byte_size: Option<u64>,
    element: Option<u64>,
    lanes: Option<u64>,
    record: Option<TvisaValueTypeRecord>,
}

impl ValueTypeInfo {
    fn new(tag: u8) -> Self {
        Self {
            tag,
            bit_width: None,
            byte_size: None,
            element: None,
            lanes: None,
            record: None,
        }
    }

    pub(super) fn group_member_size(self) -> Option<u64> {
        if matches!(self.tag, 0x01..=0x08 | 0x0c | 0x10 | 0x14) {
            return self.byte_size;
        }
        if self.tag == 0x20 {
            let size = self.byte_size?;
            return (size <= 16).then_some(size);
        }
        None
    }

    fn vector_element(self) -> bool {
        matches!(self.tag, 0x01..=0x0c | 0x10)
            && (self.tag != 0x01 || self.bit_width.is_some_and(|width| width <= 64))
    }

    fn scalable_element(self) -> bool {
        self.vector_element()
            && self
                .bit_width
                .is_some_and(|width| width > 0 && width <= 128 && 128 % width == 0)
    }

    fn texture_component(self, types: &[Self]) -> bool {
        if matches!(self.tag, 0x02 | 0x04) || (self.tag == 0x01 && self.bit_width == Some(32)) {
            return true;
        }
        self.tag == 0x20
            && self.lanes == Some(4)
            && self
                .element
                .and_then(|index| types.get(index as usize))
                .is_some_and(|element| {
                    matches!(element.tag, 0x02 | 0x04)
                        || (element.tag == 0x01 && element.bit_width == Some(32))
                })
    }

    pub(super) fn c_aggregate_member_size(self) -> Option<u64> {
        if matches!(self.tag, 0x01..=0x08 | 0x0c | 0x10 | 0x20) {
            return self.byte_size;
        }
        None
    }
}

pub(super) fn decode_value_type(
    bytes: &[u8],
    cursor: &mut usize,
    type_index: u64,
    tag: u8,
    previous_types: &[ValueTypeInfo],
    max_signature_reference: &mut Option<u64>,
) -> Result<(ValueTypeInfo, TvisaValueTypeRecord), FormatError> {
    let mut value_type = ValueTypeInfo::new(tag);
    match tag {
        0x01 => {
            let offset = *cursor;
            let width = read_index(bytes, cursor, "integer width")?;
            if !matches!(width, 1 | 8 | 16 | 32 | 64 | 128) {
                return Err(malformed(offset, "invalid TVISA integer width"));
            }
            value_type.bit_width = Some(width);
            value_type.byte_size = Some(scalar_size(width));
            value_type.record = Some(TvisaValueTypeRecord::Integer { bits: width });
        }
        0x02..=0x0c => {
            let width = float_width(tag);
            value_type.bit_width = Some(width);
            value_type.byte_size = Some(scalar_size(width));
            value_type.record = Some(TvisaValueTypeRecord::Float { tag });
        }
        0x10 => {
            let offset = *cursor;
            let address_space = read_byte(bytes, cursor, "pointer address space")?;
            if address_space > 9 {
                return Err(malformed(offset, "invalid TVISA pointer address space"));
            }
            value_type.bit_width = Some(64);
            value_type.byte_size = Some(8);
            value_type.record = Some(TvisaValueTypeRecord::Pointer { address_space });
        }
        0x14 | 0x41..=0x44 => {
            value_type.byte_size = Some(8);
            match tag {
                0x14 => value_type.record = Some(TvisaValueTypeRecord::Code),
                0x41 => value_type.record = Some(TvisaValueTypeRecord::Sampler),
                0x42 => {
                    value_type.record = Some(TvisaValueTypeRecord::AccelerationStructure);
                }
                0x43 => {
                    let signature = read_index(bytes, cursor, "visible function signature")?;
                    note_max(max_signature_reference, signature);
                    value_type.record =
                        Some(TvisaValueTypeRecord::VisibleFunctionTable { signature });
                }
                0x44 => {
                    let rank_offset = *cursor;
                    let rank = read_index(bytes, cursor, "tensor rank")?;
                    if !(1..=8).contains(&rank) {
                        return Err(malformed(rank_offset, "invalid tensor rank"));
                    }
                    let element_offset = *cursor;
                    let element = read_index(bytes, cursor, "tensor element type")?;
                    validate_reference(element, type_index, element_offset)?;
                    value_type.element = Some(element);
                    value_type.record = Some(TvisaValueTypeRecord::Tensor {
                        rank,
                        element_type: element,
                    });
                }
                _ => {}
            }
        }
        0x20 => {
            let offset = *cursor;
            let lanes = read_index(bytes, cursor, "vector lane count")?;
            if !matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64) {
                return Err(malformed(offset, "invalid TVISA vector lane count"));
            }
            let element_offset = *cursor;
            let element_index = read_index(bytes, cursor, "vector element type")?;
            validate_reference(element_index, type_index, element_offset)?;
            let Some(element) = previous_types.get(element_index as usize).copied() else {
                return Err(malformed(
                    offset,
                    "vector references an invalid element type",
                ));
            };
            if !element.vector_element() {
                return Err(malformed(
                    offset,
                    "fixed vector element must be an integer, float, narrow float, or pointer",
                ));
            }
            let size = vector_size(lanes, element)
                .ok_or_else(|| malformed(offset, "fixed vector element has no supported size"))?;
            if size > 512 {
                return Err(malformed(offset, "fixed vector exceeds 512 bytes"));
            }
            value_type.element = Some(element_index);
            value_type.lanes = Some(lanes);
            value_type.byte_size = Some(size);
            value_type.record = Some(TvisaValueTypeRecord::FixedVector {
                lanes,
                element_type: element_index,
            });
        }
        0x21 => {
            let offset = *cursor;
            let element_index = read_index(bytes, cursor, "scalable vector element type")?;
            validate_reference(element_index, type_index, *cursor)?;
            let Some(element) = previous_types.get(element_index as usize).copied() else {
                return Err(malformed(
                    offset,
                    "scalable vector references an invalid type",
                ));
            };
            if !element.scalable_element() {
                return Err(malformed(
                    offset,
                    "scalable vector element width must divide 128 bits",
                ));
            }
            value_type.element = Some(element_index);
            value_type.record = Some(TvisaValueTypeRecord::ScalableVector {
                element_type: element_index,
            });
        }
        0x40 => {
            let dimension_offset = *cursor;
            let dimension = read_byte(bytes, cursor, "texture dimension")?;
            if dimension > 6 {
                return Err(malformed(dimension_offset, "invalid texture dimension"));
            }
            let component_offset = *cursor;
            let component_index = read_index(bytes, cursor, "texture component type")?;
            validate_reference(component_index, type_index, component_offset)?;
            let Some(component) = previous_types.get(component_index as usize).copied() else {
                return Err(malformed(
                    component_offset,
                    "texture references an invalid component type",
                ));
            };
            if !component.texture_component(previous_types) {
                return Err(malformed(
                    component_offset,
                    "texture component must be f16, f32, i32, or a four-lane vector of one",
                ));
            }
            let access_offset = *cursor;
            let access = read_byte(bytes, cursor, "texture access")?;
            if access > 3 {
                return Err(malformed(access_offset, "invalid texture access"));
            }
            let multisampled_offset = *cursor;
            let multisampled = read_byte(bytes, cursor, "texture multisample flag")?;
            if multisampled > 1 {
                return Err(malformed(
                    multisampled_offset,
                    "invalid texture multisample flag",
                ));
            }
            value_type.element = Some(component_index);
            value_type.byte_size = Some(8);
            value_type.record = Some(TvisaValueTypeRecord::Texture {
                dimension,
                component_type: component_index,
                access,
                multisampled: multisampled != 0,
            });
        }
        0x45 => value_type.record = Some(TvisaValueTypeRecord::RayQuery),
        0x46 => value_type.record = Some(TvisaValueTypeRecord::HitObject),
        0x47 => {
            let scope_offset = *cursor;
            let scope = read_byte(bytes, cursor, "cooperative matrix scope")?;
            if scope > 1 {
                return Err(malformed(scope_offset, "invalid cooperative matrix scope"));
            }
            let rows_offset = *cursor;
            let rows = read_index(bytes, cursor, "cooperative matrix rows")?;
            if rows == 0 {
                return Err(malformed(
                    rows_offset,
                    "cooperative matrix rows must be nonzero",
                ));
            }
            let columns_offset = *cursor;
            let columns = read_index(bytes, cursor, "cooperative matrix columns")?;
            if columns == 0 {
                return Err(malformed(
                    columns_offset,
                    "cooperative matrix columns must be nonzero",
                ));
            }
            let element_offset = *cursor;
            let element = read_index(bytes, cursor, "cooperative matrix element type")?;
            validate_reference(element, type_index, element_offset)?;
            let use_offset = *cursor;
            let use_kind = read_byte(bytes, cursor, "cooperative matrix use")?;
            if use_kind > 2 {
                return Err(malformed(use_offset, "invalid cooperative matrix use"));
            }
            value_type.element = Some(element);
            value_type.record = Some(TvisaValueTypeRecord::CooperativeMatrix {
                scope,
                rows,
                columns,
                element_type: element,
                use_kind,
            });
        }
        _ => {
            return Err(FormatError::unsupported_type(
                Some(cursor.saturating_sub(1)),
                format!("type tag 0x{tag:02X} is not permitted in TVISA"),
            ));
        }
    }
    let record = value_type
        .record
        .ok_or_else(|| malformed(*cursor, "TVISA value type record was not retained"))?;
    Ok((value_type, record))
}

fn float_width(tag: u8) -> u64 {
    match tag {
        0x02 | 0x03 => 16,
        0x04 => 32,
        0x05 => 64,
        0x06 => 128,
        0x07 | 0x08 | 0x0c => 8,
        0x09 | 0x0a => 6,
        0x0b => 4,
        _ => unreachable!(),
    }
}

fn validate_reference(index: u64, current_type: u64, offset: usize) -> Result<(), FormatError> {
    if index >= current_type {
        return Err(malformed(
            offset,
            "TYPES value types must reference earlier type indexes",
        ));
    }
    Ok(())
}

fn scalar_size(width: u64) -> u64 {
    let bytes = width.div_ceil(8);
    bytes.next_power_of_two().min(16)
}

fn vector_size(lanes: u64, element: ValueTypeInfo) -> Option<u64> {
    let width = element.bit_width?;
    if element.tag == 0x01 && width == 1 {
        return Some(lanes);
    }
    if matches!(width, 4 | 6) {
        return Some(lanes.checked_mul(width)?.div_ceil(8));
    }
    let element_size = element.byte_size?;
    let lane_alignment = lanes.checked_next_power_of_two()?;
    let alignment = element_size.checked_mul(lane_alignment)?.min(16);
    let size = lanes.checked_mul(element_size)?;
    size.checked_add(alignment - 1)
        .map(|rounded| rounded / alignment * alignment)
}
