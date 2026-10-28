//! Parsing of a single instruction's operation text (section 12).
//!
//! The families are tried in a fixed order so that longer opcodes are matched
//! before the shorter opcodes they start with.

mod aggregate;
mod arithmetic;
mod calls;
mod common;
mod constants;
mod control;
mod memory;
mod system;

use super::{ParseError, error};
use crate::Operation;

pub(super) fn parse_operation(text: &str, offset: usize) -> Result<Operation, ParseError> {
    for family in [
        constants::parse,
        control::parse,
        calls::parse,
        system::parse,
        memory::parse,
        aggregate::parse,
        arithmetic::parse,
    ] {
        if let Some(operation) = family(text, offset)? {
            return Ok(operation);
        }
    }
    Err(error(offset, format!("unknown instruction `{text}`")))
}
