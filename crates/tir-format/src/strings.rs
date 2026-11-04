use std::io::{self, Write};

use crate::leb::{LebError, read_uleb, write_uleb};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringTable {
    values: Vec<String>,
}

impl StringTable {
    pub fn from_values(values: Vec<String>) -> Result<Self, StringTableError> {
        if values.is_empty() {
            return Err(error(0, "invalid string table count"));
        }
        if !values[0].is_empty() {
            return Err(error(0, "string table entry zero is not empty"));
        }
        Ok(Self { values })
    }

    pub fn encode(&self, output: &mut impl Write) -> io::Result<()> {
        let count = u64::try_from(self.values.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many strings"))?;
        if count == 0 || !self.values[0].is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "string table entry zero must be empty",
            ));
        }
        write_uleb(count, output)?;
        for value in &self.values {
            let length = u64::try_from(value.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "string is too long"))?;
            write_uleb(length, output)?;
            output.write_all(value.as_bytes())?;
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StringTableError> {
        let mut cursor = 0;
        let count = read_uleb(bytes, &mut cursor).map_err(|issue| leb_error(issue, 0))?;
        if count == 0 || count > bytes.len() as u64 {
            return Err(error(cursor as u64, "invalid string table count"));
        }
        let capacity = usize::try_from(count)
            .map_err(|_| error(cursor as u64, "string table count does not fit in memory"))?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(capacity)
            .map_err(|_| error(cursor as u64, "string table allocation failed"))?;
        for index in 0..count {
            let length_offset = cursor as u64;
            let length =
                read_uleb(bytes, &mut cursor).map_err(|issue| leb_error(issue, length_offset))?;
            let length = usize::try_from(length)
                .map_err(|_| error(length_offset, "string length does not fit in memory"))?;
            let end = cursor
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| error(cursor as u64, "string extends beyond the table"))?;
            let value = std::str::from_utf8(&bytes[cursor..end])
                .map_err(|_| error(cursor as u64, "string is not valid UTF-8"))?;
            if index == 0 && !value.is_empty() {
                return Err(error(length_offset, "string table entry zero is not empty"));
            }
            values.push(value.to_owned());
            cursor = end;
        }
        if cursor != bytes.len() {
            return Err(error(cursor as u64, "trailing bytes in string table"));
        }
        Ok(Self { values })
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        self.values.get(index).map(String::as_str)
    }

    #[must_use]
    pub fn index_of(&self, value: &str) -> Option<usize> {
        self.values.iter().position(|candidate| candidate == value)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringTableError {
    pub offset: u64,
    pub message: &'static str,
}

#[derive(Clone, Copy)]
enum State {
    Count,
    Length {
        index: u64,
        count: u64,
    },
    Data {
        index: u64,
        count: u64,
        remaining: u64,
    },
    Done,
}

pub struct StringTableValidator {
    state: State,
    leb: [u8; 10],
    leb_len: usize,
    offset: u64,
    total_size: u64,
    utf8_pending: Vec<u8>,
}

impl StringTableValidator {
    #[must_use]
    pub fn new(total_size: u64) -> Self {
        Self {
            state: State::Count,
            leb: [0; 10],
            leb_len: 0,
            offset: 0,
            total_size,
            utf8_pending: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<(), StringTableError> {
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            match self.state {
                State::Count | State::Length { .. } => {
                    let byte = bytes[cursor];
                    cursor += 1;
                    self.offset += 1;
                    if self.leb_len == self.leb.len() {
                        return Err(error(self.offset - 1, "LEB128 value overflows its type"));
                    }
                    self.leb[self.leb_len] = byte;
                    self.leb_len += 1;
                    if byte & 0x80 != 0 {
                        if self.leb_len == self.leb.len() {
                            return Err(error(self.offset - 1, "LEB128 value overflows its type"));
                        }
                        continue;
                    }

                    let state = self.state;
                    let mut leb_cursor = 0;
                    let value = read_uleb(&self.leb[..self.leb_len], &mut leb_cursor)
                        .map_err(|issue| leb_error(issue, self.offset - self.leb_len as u64))?;
                    self.leb_len = 0;
                    match state {
                        State::Count => {
                            if value == 0 || value > self.total_size {
                                return Err(error(self.offset - 1, "invalid string table count"));
                            }
                            self.state = State::Length {
                                index: 0,
                                count: value,
                            };
                        }
                        State::Length { index, count } => {
                            if index == 0 && value != 0 {
                                return Err(error(
                                    self.offset - 1,
                                    "string table entry zero is not empty",
                                ));
                            }
                            if value > self.total_size.saturating_sub(self.offset) {
                                return Err(error(self.offset, "string extends beyond the table"));
                            }
                            if value == 0 {
                                self.next_string(index, count);
                            } else {
                                self.state = State::Data {
                                    index,
                                    count,
                                    remaining: value,
                                };
                            }
                        }
                        State::Data { .. } | State::Done => unreachable!(),
                    }
                }
                State::Data {
                    index,
                    count,
                    remaining,
                } => {
                    let available = (bytes.len() - cursor) as u64;
                    let length = usize::try_from(remaining.min(available)).unwrap_or(bytes.len());
                    self.push_utf8(&bytes[cursor..cursor + length])?;
                    cursor += length;
                    self.offset += length as u64;
                    let remaining = remaining - length as u64;
                    if remaining == 0 {
                        if !self.utf8_pending.is_empty() {
                            return Err(error(
                                self.offset - self.utf8_pending.len() as u64,
                                "string is not valid UTF-8",
                            ));
                        }
                        self.next_string(index, count);
                    } else {
                        self.state = State::Data {
                            index,
                            count,
                            remaining,
                        };
                    }
                }
                State::Done => return Err(error(self.offset, "trailing bytes in string table")),
            }
        }
        Ok(())
    }

    fn push_utf8(&mut self, bytes: &[u8]) -> Result<(), StringTableError> {
        if self.utf8_pending.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(_) => return Ok(()),
                Err(issue) if issue.error_len().is_none() => {
                    self.utf8_pending
                        .extend_from_slice(&bytes[issue.valid_up_to()..]);
                    return Ok(());
                }
                Err(_) => return Err(error(self.offset, "string is not valid UTF-8")),
            }
        }

        let mut joined = Vec::with_capacity(self.utf8_pending.len() + bytes.len());
        joined.extend_from_slice(&self.utf8_pending);
        joined.extend_from_slice(bytes);
        self.utf8_pending.clear();
        match std::str::from_utf8(&joined) {
            Ok(_) => Ok(()),
            Err(issue) if issue.error_len().is_none() => {
                self.utf8_pending
                    .extend_from_slice(&joined[issue.valid_up_to()..]);
                if self.utf8_pending.len() > 3 {
                    return Err(error(self.offset, "string is not valid UTF-8"));
                }
                Ok(())
            }
            Err(_) => Err(error(self.offset, "string is not valid UTF-8")),
        }
    }

    fn next_string(&mut self, index: u64, count: u64) {
        self.utf8_pending.clear();
        if index + 1 == count {
            self.state = State::Done;
        } else {
            self.state = State::Length {
                index: index + 1,
                count,
            };
        }
    }

    pub fn finish(self) -> Result<(), StringTableError> {
        match self.state {
            State::Done if self.offset == self.total_size => Ok(()),
            _ => Err(error(self.offset, "truncated string table")),
        }
    }
}

fn error(offset: u64, message: &'static str) -> StringTableError {
    StringTableError { offset, message }
}

fn leb_error(issue: LebError, offset: u64) -> StringTableError {
    let message = match issue {
        LebError::Truncated => "truncated LEB128",
        LebError::Overflow => "LEB128 value overflows its type",
        LebError::NonMinimal => "LEB128 value is not minimally encoded",
        LebError::InvalidSign => "integer immediate has an invalid sign encoding",
    };
    error(offset, message)
}
