//! Sections 12.7 to 12.10: storage, memory, heap and ownership instructions.

use super::common::{Parsed, parse_index, parse_type, split_top_level_once};
use crate::parse::{
    ParseError, error, parse_two_values, parse_value, split_top_level, strip_optional_mode,
};
use crate::{AccessEnforcement, AccessMode, LoadMode, Operation, RawAccess, StoreMode};
use klf_core::Name;

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_storage(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_access(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_raw(text, offset)? {
        return Ok(Some(operation));
    }
    parse_ownership(text, offset)
}

fn parse_storage(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(ty) = text.strip_prefix("stack.alloc ") {
        Operation::StackAlloc(parse_type(ty, offset)?)
    } else if let Some(value) = text.strip_prefix("stack.dealloc ") {
        Operation::StackDealloc(parse_value(value, offset)?)
    } else if let Some(rest) = text.strip_prefix("field.addr ") {
        let (base, field) = required_split(rest, offset)?;
        Operation::FieldAddr {
            base: parse_value(base, offset)?,
            field: parse_index(field, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("elem.addr ") {
        let (base, index) = required_split(rest, offset)?;
        Operation::ElemAddr {
            base: parse_value(base, offset)?,
            index: parse_value(index, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("addr.cast ") {
        let (address, ty) = rest
            .split_once(" to ")
            .ok_or_else(|| error(offset, "`addr.cast` requires `%address to <type>`"))?;
        Operation::AddrCast {
            address: parse_value(address, offset)?,
            ty: parse_type(ty, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("addr.toptr ") {
        Operation::AddrToPointer(parse_value(rest, offset)?)
    } else if let Some(ty) = text.strip_prefix("heap.alloc.counted ") {
        Operation::HeapAllocCounted(parse_type(ty, offset)?)
    } else if let Some(ty) = text.strip_prefix("heap.alloc.unique ") {
        Operation::HeapAllocUnique(parse_type(ty, offset)?)
    } else if let Some(rest) = text.strip_prefix("heap.alloc.array ") {
        let (element, count) = required_split(rest, offset)?;
        Operation::HeapAllocArray {
            element: parse_type(element, offset)?,
            count: parse_value(count, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("heap.alloc.raw ") {
        let (size, align) = parse_two_values(rest, offset)?;
        Operation::HeapAllocRaw { size, align }
    } else if let Some(rest) = text.strip_prefix("heap.free.raw ") {
        Operation::HeapFreeRaw(parse_value(rest, offset)?)
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_access(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("load.borrow ") {
        Operation::LoadBorrow(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("load ") {
        let mode = match bracketed(rest) {
            Some("copy") => LoadMode::Copy,
            Some("take") => LoadMode::Take,
            _ => LoadMode::Trivial,
        };
        Operation::Load {
            mode,
            address: parse_value(strip_optional_mode(rest), offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("store ") {
        let mode = match bracketed(rest) {
            Some("init") => StoreMode::Init,
            Some("assign") => StoreMode::Assign,
            _ => StoreMode::Trivial,
        };
        let (value, address) = strip_optional_mode(rest)
            .split_once(" to ")
            .ok_or_else(|| error(offset, "`store` requires `%value to %address`"))?;
        Operation::Store {
            mode,
            value: parse_value(value, offset)?,
            address: parse_value(address, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("copy.addr ") {
        let (source, dest, mode) = parse_addr_transfer(rest, offset)?;
        Operation::CopyAddr { source, dest, mode }
    } else if let Some(rest) = text.strip_prefix("move.addr ") {
        let (source, dest, mode) = parse_addr_transfer(rest, offset)?;
        Operation::MoveAddr { source, dest, mode }
    } else if let Some(rest) = text.strip_prefix("destroy.addr ") {
        Operation::DestroyAddr(parse_value(rest, offset)?)
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_raw(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("raw.load ") {
        let (ty, rest) = required_split(rest, offset)?;
        let (pointer, access) = parse_raw_access(rest, offset)?;
        Operation::RawLoad {
            ty: parse_type(ty, offset)?,
            pointer,
            access,
        }
    } else if let Some(rest) = text.strip_prefix("raw.store ") {
        let (value, rest) = required_split(rest, offset)?;
        let (pointer, access) = parse_raw_access(rest, offset)?;
        Operation::RawStore {
            value: parse_value(value, offset)?,
            pointer,
            access,
        }
    } else if let Some(rest) = text.strip_prefix("ptr.offset ") {
        let (pointer, bytes) = parse_two_values(rest, offset)?;
        Operation::PointerOffset { pointer, bytes }
    } else if let Some(rest) = text.strip_prefix("ptr.diff ") {
        let (lhs, rhs) = parse_two_values(rest, offset)?;
        Operation::PointerDiff { lhs, rhs }
    } else if let Some(rest) = text.strip_prefix("mem.copy ") {
        let [dest, source, length] = three_values(rest, offset, "mem.copy")?;
        Operation::MemCopy {
            dest,
            source,
            length,
        }
    } else if let Some(rest) = text.strip_prefix("mem.move ") {
        let [dest, source, length] = three_values(rest, offset, "mem.move")?;
        Operation::MemMove {
            dest,
            source,
            length,
        }
    } else if let Some(rest) = text.strip_prefix("mem.set ") {
        let [dest, byte, length] = three_values(rest, offset, "mem.set")?;
        Operation::MemSet { dest, byte, length }
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_ownership(text: &str, offset: usize) -> Parsed {
    for (prefix, constructor) in [
        ("copy ", Operation::Copy as fn(Name) -> Operation),
        ("move ", Operation::Move),
        ("destroy ", Operation::Destroy),
        ("borrow.begin ", Operation::BorrowBegin),
        ("borrow.end ", Operation::BorrowEnd),
        ("into.raw ", Operation::IntoRaw),
        ("ref.addr ", Operation::RefAddr),
        ("uniq.addr ", Operation::UniqAddr),
        ("weak.new ", Operation::WeakNew),
        ("weak.load ", Operation::WeakLoad),
        ("ref.unowned ", Operation::RefUnowned),
        ("ref.isunique ", Operation::RefIsUnique),
        ("access.end ", Operation::AccessEnd),
    ] {
        if let Some(value) = text.strip_prefix(prefix) {
            return Ok(Some(constructor(parse_value(value, offset)?)));
        }
    }
    if let Some(rest) = text.strip_prefix("from.raw ") {
        let (ty, pointer) = required_split(rest, offset)?;
        return Ok(Some(Operation::FromRaw {
            ty: parse_type(ty, offset)?,
            pointer: parse_value(pointer, offset)?,
        }));
    }
    if let Some(rest) = text.strip_prefix("access.begin ") {
        return parse_access_begin(rest, offset).map(Some);
    }
    Ok(None)
}

fn parse_access_begin(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let mut parts = text.splitn(3, ' ');
    let mode = match parts.next() {
        Some("read") => AccessMode::Read,
        Some("modify") => AccessMode::Modify,
        _ => return Err(error(offset, "`access.begin` needs `read` or `modify`")),
    };
    let enforcement = match parts.next() {
        Some("static") => AccessEnforcement::Static,
        Some("dynamic") => AccessEnforcement::Dynamic,
        Some("unchecked") => AccessEnforcement::Unchecked,
        _ => {
            return Err(error(
                offset,
                "`access.begin` needs `static`, `dynamic` or `unchecked`",
            ));
        }
    };
    let address = parts
        .next()
        .ok_or_else(|| error(offset, "`access.begin` needs an address"))?;
    Ok(Operation::AccessBegin {
        mode,
        enforcement,
        address: parse_value(address, offset)?,
    })
}

fn parse_addr_transfer(text: &str, offset: usize) -> Result<(Name, Name, StoreMode), ParseError> {
    let (source, rest) = text
        .split_once(" to ")
        .ok_or_else(|| error(offset, "`copy.addr`/`move.addr` need `%src to [mode] %dst`"))?;
    let mode = match bracketed(rest) {
        Some("assign") => StoreMode::Assign,
        _ => StoreMode::Init,
    };
    Ok((
        parse_value(source, offset)?,
        parse_value(strip_optional_mode(rest), offset)?,
        mode,
    ))
}

/// Splits `%pointer [align N] [volatile]`.
fn parse_raw_access(text: &str, offset: usize) -> Result<(Name, RawAccess), ParseError> {
    let mut access = RawAccess::default();
    let mut rest = text.trim();
    if let Some(head) = rest.strip_suffix(" volatile") {
        access.volatile = true;
        rest = head.trim_end();
    }
    if let Some((head, align)) = rest.rsplit_once(" align ") {
        access.align = Some(parse_index(align, offset)?);
        rest = head.trim_end();
    }
    Ok((parse_value(rest, offset)?, access))
}

fn three_values(text: &str, offset: usize, what: &str) -> Result<[Name; 3], ParseError> {
    let parts = split_top_level(text, ',');
    if parts.len() != 3 {
        return Err(error(offset, format!("`{what}` requires three operands")));
    }
    Ok([
        parse_value(parts[0], offset)?,
        parse_value(parts[1], offset)?,
        parse_value(parts[2], offset)?,
    ])
}

fn bracketed(text: &str) -> Option<&str> {
    text.trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(mode, _)| mode.trim())
}

fn required_split(text: &str, offset: usize) -> Result<(&str, &str), ParseError> {
    split_top_level_once(text, ',').ok_or_else(|| error(offset, "expected `,`"))
}
