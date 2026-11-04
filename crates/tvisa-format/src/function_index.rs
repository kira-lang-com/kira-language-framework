use crate::binary::FormatError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionIndexRecord {
    pub offset: u64,
    pub size: u32,
}

/// Encode FUNCINDEX ranges after checking that they partition CODE.
pub fn encode_function_index(
    records: &[FunctionIndexRecord],
    code_size: u64,
    canonical: bool,
) -> Result<Vec<u8>, FormatError> {
    if records.is_empty() {
        if code_size != 0 {
            return Err(FormatError::malformed(
                None,
                "CODE must be empty when FUNCINDEX has no records",
            ));
        }
        return Ok(Vec::new());
    }
    let expected_count = u32::try_from(records.len())
        .map_err(|_| FormatError::malformed(None, "function count overflows"))?;
    let byte_len = records
        .len()
        .checked_mul(16)
        .ok_or_else(|| FormatError::malformed(None, "FUNCINDEX size overflows"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(byte_len)
        .map_err(|_| FormatError::malformed(None, "FUNCINDEX allocation failed"))?;
    for record in records {
        bytes.extend_from_slice(&record.offset.to_le_bytes());
        bytes.extend_from_slice(&record.size.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    let mut validator = FunctionIndexValidator::new(expected_count, code_size, canonical)?;
    validator.push(&bytes)?;
    validator.finish()?;
    Ok(bytes)
}

pub(crate) struct FunctionIndexValidator {
    expected_count: u32,
    code_size: u64,
    records: u32,
    pending: [u8; 16],
    pending_len: usize,
    canonical: bool,
    previous_offset: Option<u64>,
    ranges: Vec<(u64, u64)>,
}

impl FunctionIndexValidator {
    pub(crate) fn new(
        expected_count: u32,
        code_size: u64,
        canonical: bool,
    ) -> Result<Self, FormatError> {
        let capacity = usize::try_from(expected_count)
            .map_err(|_| FormatError::malformed(None, "function count does not fit in memory"))?;
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(capacity)
            .map_err(|_| FormatError::malformed(None, "function index allocation failed"))?;
        Ok(Self {
            expected_count,
            code_size,
            records: 0,
            pending: [0; 16],
            pending_len: 0,
            canonical,
            previous_offset: None,
            ranges,
        })
    }

    pub(crate) fn push(&mut self, mut bytes: &[u8]) -> Result<(), FormatError> {
        while !bytes.is_empty() {
            let count = (self.pending.len() - self.pending_len).min(bytes.len());
            self.pending[self.pending_len..self.pending_len + count]
                .copy_from_slice(&bytes[..count]);
            self.pending_len += count;
            bytes = &bytes[count..];
            if self.pending_len == self.pending.len() {
                let mut offset_bytes = [0; 8];
                offset_bytes.copy_from_slice(&self.pending[..8]);
                let offset = u64::from_le_bytes(offset_bytes);
                let mut size_bytes = [0; 4];
                size_bytes.copy_from_slice(&self.pending[8..12]);
                let size = u64::from(u32::from_le_bytes(size_bytes));
                let mut reserved_bytes = [0; 4];
                reserved_bytes.copy_from_slice(&self.pending[12..16]);
                let reserved = u32::from_le_bytes(reserved_bytes);
                if size < 3 {
                    return Err(FormatError::malformed(
                        None,
                        "function body is too short to contain its header",
                    ));
                }
                let end = offset
                    .checked_add(size)
                    .ok_or_else(|| FormatError::malformed(None, "function body range overflows"))?;
                if reserved != 0 {
                    return Err(FormatError::malformed(
                        None,
                        "FUNCINDEX reserved field is nonzero",
                    ));
                }
                if end > self.code_size {
                    return Err(FormatError::malformed(
                        None,
                        "function body extends beyond CODE",
                    ));
                }
                if self.canonical
                    && self
                        .previous_offset
                        .is_some_and(|previous| offset < previous)
                {
                    return Err(FormatError::noncanonical(
                        None,
                        "FUNCINDEX bodies are not in function order",
                    ));
                }
                self.previous_offset = Some(offset);
                self.ranges.push((offset, end));
                self.records += 1;
                self.pending_len = 0;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), FormatError> {
        if self.pending_len != 0 || self.records != self.expected_count {
            return Err(FormatError::malformed(None, "truncated FUNCINDEX"));
        }
        self.ranges.sort_unstable_by_key(|range| range.0);
        let mut next_offset = 0;
        for (start, end) in &self.ranges {
            if *start < next_offset {
                return Err(FormatError::malformed(
                    None,
                    "function bodies overlap in CODE",
                ));
            }
            if *start != next_offset {
                return Err(FormatError::malformed(
                    None,
                    "CODE contains bytes outside FUNCINDEX body ranges",
                ));
            }
            next_offset = *end;
        }
        if next_offset != self.code_size {
            return Err(FormatError::malformed(
                None,
                "CODE contains bytes outside FUNCINDEX body ranges",
            ));
        }
        Ok(())
    }
}
