use std::collections::HashSet;

use crate::FormatError;
use crate::StringTable;
use crate::leb::read_uleb;
use crate::types::{BinaryGpuNodeOutput, BinaryGpuNodeOutputKind, BinaryGpuNodeRecord};

const MAX_RECORDS: u64 = 1 << 32;

pub(crate) fn read(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<BinaryGpuNodeRecord, FormatError> {
    let launch_offset = *cursor;
    let launch_mode = read_byte(bytes, cursor, "node launch mode")?;
    if launch_mode > 2 {
        return Err(malformed(launch_offset, "invalid GPU node launch mode"));
    }
    let maximum_dispatch_grid = [
        read_index(bytes, cursor, "node dispatch-grid dimension")?,
        read_index(bytes, cursor, "node dispatch-grid dimension")?,
        read_index(bytes, cursor, "node dispatch-grid dimension")?,
    ];
    if maximum_dispatch_grid.contains(&0) {
        return Err(malformed(
            launch_offset,
            "GPU node dispatch-grid dimensions must be positive",
        ));
    }
    let input_record_size = read_index(bytes, cursor, "node input record size")?;
    let output_count_offset = *cursor;
    let output_count = read_index(bytes, cursor, "node output count")?;
    if output_count >= MAX_RECORDS || output_count > bytes.len().saturating_sub(*cursor) as u64 / 4
    {
        return Err(malformed(
            output_count_offset,
            "GPU node output count exceeds limits",
        ));
    }
    let capacity = usize::try_from(output_count).map_err(|_| {
        malformed(
            output_count_offset,
            "GPU node output count overflows memory",
        )
    })?;
    let mut outputs = Vec::new();
    outputs
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(*cursor, "GPU node output allocation failed"))?;
    let mut names = HashSet::new();
    for _ in 0..capacity {
        let name_offset = *cursor;
        let name_index = usize::try_from(read_index(bytes, cursor, "node output name")?)
            .map_err(|_| malformed(name_offset, "GPU node output name index overflows memory"))?;
        let name = strings
            .get(name_index)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| malformed(name_offset, "GPU node output name is invalid"))?
            .to_owned();
        if !names.insert(name.clone()) {
            return Err(malformed(name_offset, "duplicate GPU node output name"));
        }
        let kind_offset = *cursor;
        let kind = match read_byte(bytes, cursor, "node output kind")? {
            0 => BinaryGpuNodeOutputKind::Node,
            1 => BinaryGpuNodeOutputKind::Mesh,
            _ => return Err(malformed(kind_offset, "invalid GPU node output kind")),
        };
        let record_size = read_index(bytes, cursor, "node output record size")?;
        let maximum_records = read_index(bytes, cursor, "node output maximum records")?;
        if record_size == 0 || maximum_records == 0 {
            return Err(malformed(
                name_offset,
                "GPU node output sizes and limits must be positive",
            ));
        }
        outputs.push(BinaryGpuNodeOutput {
            name,
            kind,
            record_size,
            maximum_records,
        });
    }
    Ok(BinaryGpuNodeRecord {
        launch_mode,
        maximum_dispatch_grid,
        input_record_size,
        outputs,
    })
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
