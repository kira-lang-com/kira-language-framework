use crate::{BinaryType, BinaryTypeTable, FormatError};

pub(crate) fn constant_type_name(
    types: &BinaryTypeTable,
    index: u64,
) -> Result<String, FormatError> {
    match types
        .entries
        .get(index as usize)
        .ok_or_else(|| FormatError::malformed(None, "type index is out of range"))?
    {
        BinaryType::Pointer(0) => Ok("ptr".to_owned()),
        BinaryType::Pointer(space) => {
            let name = match space {
                1 => "device",
                2 => "constant",
                3 => "threadgroup",
                4 => "thread",
                5 => "object_payload",
                6 => "ray_payload",
                7 => "hit_attribute",
                8 => "tile",
                9 => "node_record",
                _ => {
                    return Err(FormatError::malformed(
                        None,
                        "invalid pointer address space in constant",
                    ));
                }
            };
            Ok(format!("ptr<{name}>"))
        }
        _ => crate::text_memory_disassembly::format_storage_type(types, index),
    }
}
