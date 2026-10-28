//! `.klbc` bytecode module file (section 15.2).
//!
//! This encodes the program shape the VM currently produces: the string pool,
//! the constant pool, and one entry per function with its code words, call-site
//! table and allocation table. Sections the model does not yet populate (types,
//! globals, descriptors, witnesses, imports, exports, debug, metadata) are
//! omitted until their data exists, per decision D-109.

use crate::bytecode::WidthCode;
use crate::program::{
    AllocShape, BcFunction, BranchCondition, BranchSite, BranchTarget, CallSite, Const,
    ForeignClass, ForeignSignature, Import, ImportKind, Program, RegisterMove,
};
use klf_core::Name;

const MAGIC: [u8; 4] = [0x4B, 0x4C, 0x42, 0x43];
const KIND_STRINGS: u32 = 1;
const KIND_CONSTS: u32 = 3;
const KIND_FUNCS: u32 = 5;
const KIND_CODE: u32 = 6;
const KIND_GLOBALS: u32 = 7;
const KIND_INITIALIZERS: u32 = 8;
const KIND_IMPORTS: u32 = 9;
const KIND_BRANCHES: u32 = 13;

/// Errors from decoding a malformed `.klbc` image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KlbcError(pub String);

impl std::fmt::Display for KlbcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn i128(&mut self, value: i128) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn string(&mut self, value: &str) {
        self.u32(u32::try_from(value.len()).expect("string fits in u32"));
        self.bytes.extend_from_slice(value.as_bytes());
    }
}

/// Encode a program as a `.klbc` image.
///
/// # Panics
/// Panics if the program exceeds the `u32` field limits of the format (more
/// than about four billion functions, constants, code words or sections).
#[must_use]
pub fn encode(program: &Program) -> Vec<u8> {
    let strings = encode_strings(program);
    let consts = encode_consts(program);
    let (funcs, code) = encode_functions(program);
    let globals = encode_globals(program);
    let branches = encode_branches(program);
    let mut initializers = Writer::default();
    for index in &program.initializers {
        initializers.u32(u32::try_from(*index).expect("function index fits"));
    }
    let sections = [
        (KIND_STRINGS, strings, program.functions.len()),
        (KIND_CONSTS, consts, program.consts.len()),
        (KIND_FUNCS, funcs, program.functions.len()),
        (KIND_CODE, code, program.functions.len()),
        (KIND_GLOBALS, globals, program.globals.len()),
        (
            KIND_INITIALIZERS,
            initializers.bytes,
            program.initializers.len(),
        ),
        (KIND_IMPORTS, encode_imports(program), program.imports.len()),
        (KIND_BRANCHES, branches, program.functions.len()),
    ];

    let mut header = Writer::default();
    header.bytes.extend_from_slice(&MAGIC);
    header.bytes.extend_from_slice(&1u16.to_le_bytes());
    header.bytes.extend_from_slice(&1u16.to_le_bytes());
    header.u8(8);
    header.bytes.extend_from_slice(&[0, 0, 0]);
    header.u32(0);
    header.u32(u32::try_from(sections.len()).expect("section count fits"));

    let table_len = sections.len() * 16;
    let mut offset = header.bytes.len() + table_len;
    let mut table = Writer::default();
    for (kind, payload, count) in &sections {
        table.u32(*kind);
        table.u32(u32::try_from(offset).expect("offset fits"));
        table.u32(u32::try_from(payload.len()).expect("size fits"));
        table.u32(u32::try_from(*count).expect("count fits"));
        offset += payload.len();
    }

    let mut image = header.bytes;
    image.extend_from_slice(&table.bytes);
    for (_, payload, _) in &sections {
        image.extend_from_slice(payload);
    }
    image
}

fn encode_strings(program: &Program) -> Vec<u8> {
    let mut writer = Writer::default();
    for function in &program.functions {
        writer.string(function.name.as_str());
    }
    writer.bytes
}

fn encode_consts(program: &Program) -> Vec<u8> {
    let mut writer = Writer::default();
    for constant in &program.consts {
        match constant {
            Const::Int(value) => {
                writer.u8(0);
                writer.i128(*value);
            }
            Const::Float(bits) => {
                writer.u8(1);
                writer.u64(*bits);
            }
            Const::String(value) => {
                writer.u8(2);
                writer.string(value);
            }
        }
    }
    writer.bytes
}

fn encode_imports(program: &Program) -> Vec<u8> {
    let mut writer = Writer::default();
    for import in &program.imports {
        writer.string(&import.symbol);
        match &import.kind {
            ImportKind::Call(signature) => {
                writer.u8(0);
                encode_signature(&mut writer, signature);
            }
            ImportKind::Pointer(signature) => {
                writer.u8(4);
                encode_signature(&mut writer, signature);
            }
            ImportKind::Variadic { fixed, signature } => {
                writer.u8(5);
                writer.u32(*fixed);
                encode_signature(&mut writer, signature);
            }
            ImportKind::Address => writer.u8(1),
            ImportKind::Library { first: 0 } => writer.u8(2),
            ImportKind::Library { first } => {
                writer.u8(3);
                writer.u32(*first);
            }
        }
    }
    writer.bytes
}

fn encode_signature(writer: &mut Writer, signature: &ForeignSignature) {
    writer.u8(signature.result.map_or(0xff, class_code));
    writer.u32(u32::try_from(signature.parameters.len()).expect("parameter count fits"));
    for class in &signature.parameters {
        writer.u8(class_code(*class));
    }
}

fn decode_signature(reader: &mut Reader<'_>) -> Result<ForeignSignature, KlbcError> {
    let result = match reader.u8()? {
        0xff => None,
        code => Some(class_of(code)?),
    };
    let count = reader.u32()? as usize;
    let parameters = (0..count)
        .map(|_| class_of(reader.u8()?))
        .collect::<Result<_, _>>()?;
    Ok(ForeignSignature { parameters, result })
}

/// One byte per class: the low bits are the integer width's log2 (0 for 8
/// bits through 3 for 64), bit 4 marks a signed integer, and 0x20 and 0x21
/// are `f32` and `f64`.
fn class_code(class: ForeignClass) -> u8 {
    let log2 = |bits: u16| match bits {
        0..=8 => 0,
        9..=16 => 1,
        17..=32 => 2,
        _ => 3,
    };
    match class {
        ForeignClass::Unsigned(1) => 0x40,
        ForeignClass::Unsigned(bits) => log2(bits),
        ForeignClass::Signed(bits) => 0x10 | log2(bits),
        ForeignClass::F32 => 0x20,
        ForeignClass::F64 => 0x21,
    }
}

fn class_of(code: u8) -> Result<ForeignClass, KlbcError> {
    let bits = |log2: u8| 8u16 << log2;
    Ok(match code {
        0x40 => ForeignClass::Unsigned(1),
        0..=3 => ForeignClass::Unsigned(bits(code)),
        0x10..=0x13 => ForeignClass::Signed(bits(code & 3)),
        0x20 => ForeignClass::F32,
        0x21 => ForeignClass::F64,
        _ => return Err(KlbcError(format!("unknown foreign class {code:#x}"))),
    })
}

fn decode_imports(bytes: &[u8], count: usize) -> Result<Vec<Import>, KlbcError> {
    let mut reader = Reader::new(bytes);
    (0..count)
        .map(|_| {
            let symbol = reader.string()?;
            let kind = match reader.u8()? {
                0 => ImportKind::Call(decode_signature(&mut reader)?),
                4 => ImportKind::Pointer(decode_signature(&mut reader)?),
                5 => ImportKind::Variadic {
                    fixed: reader.u32()?,
                    signature: decode_signature(&mut reader)?,
                },
                1 => ImportKind::Address,
                2 => ImportKind::Library { first: 0 },
                3 => ImportKind::Library {
                    first: reader.u32()?,
                },
                kind => return Err(KlbcError(format!("unknown import kind {kind}"))),
            };
            Ok(Import { symbol, kind })
        })
        .collect()
}

fn encode_globals(program: &Program) -> Vec<u8> {
    let mut writer = Writer::default();
    for global in &program.globals {
        writer.u32(global.size);
        writer.u32(global.align);
        writer.u32(u32::try_from(global.init.len()).expect("init length fits"));
        for byte in &global.init {
            writer.u8(*byte);
        }
    }
    writer.bytes
}

fn encode_functions(program: &Program) -> (Vec<u8>, Vec<u8>) {
    let mut funcs = Writer::default();
    let mut code = Writer::default();
    for function in &program.functions {
        let code_offset = code.bytes.len() / 8;
        for word in &function.code {
            code.u64(*word);
        }
        funcs.u32(function.register_count);
        funcs.u32(function.param_count);
        funcs.u8(u8::from(function.returns_value));
        funcs.u32(u32::try_from(code_offset).expect("code offset fits"));
        funcs.u32(u32::try_from(function.code.len()).expect("code length fits"));
        funcs.u32(u32::try_from(function.call_sites.len()).expect("call count fits"));
        for site in &function.call_sites {
            funcs.u32(u32::try_from(site.callee).expect("callee index fits"));
            funcs.u32(u32::from(site.first_arg));
            funcs.u32(u32::from(site.arg_count));
            funcs.u32(u32::from(site.result));
            funcs.u8(u8::from(site.has_result));
            funcs.u32(site.foreign.unwrap_or(u32::MAX));
            funcs.u32(site.target.map_or(u32::MAX, u32::from));
            funcs.u32(
                site.fallible
                    .map_or(u32::MAX, |site| u32::from(site.failed)),
            );
            funcs.u32(site.fallible.map_or(u32::MAX, |site| site.branch));
        }
        funcs.u32(u32::try_from(function.allocations.len()).expect("alloc count fits"));
        for shape in &function.allocations {
            funcs.u32(shape.size);
            funcs.u32(shape.align);
        }
    }
    (funcs.bytes, code.bytes)
}

fn encode_branches(program: &Program) -> Vec<u8> {
    let mut writer = Writer::default();
    for function in &program.functions {
        writer.u32(u32::try_from(function.branches.len()).expect("branch count fits"));
        for site in &function.branches {
            match &site.condition {
                None => writer.u8(0),
                Some(BranchCondition::Register(register)) => {
                    writer.u8(1);
                    writer.u32(u32::from(*register));
                }
                Some(BranchCondition::IntegerCompare {
                    result,
                    lhs,
                    rhs,
                    width,
                    predicate,
                }) => {
                    writer.u8(2);
                    writer.u32(u32::from(*result));
                    writer.u32(u32::from(*lhs));
                    writer.u32(u32::from(*rhs));
                    writer.u8(width.bits() as u8);
                    writer.u8(*predicate);
                }
            }
            encode_branch_target(&mut writer, &site.then_edge);
            if let Some(edge) = &site.else_edge {
                writer.u8(1);
                encode_branch_target(&mut writer, edge);
            } else {
                writer.u8(0);
            }
        }
    }
    writer.bytes
}

fn encode_branch_target(writer: &mut Writer, edge: &BranchTarget) {
    writer.u32(u32::try_from(edge.target).expect("branch target fits"));
    writer.u32(u32::try_from(edge.moves.len()).expect("branch move count fits"));
    for movement in &edge.moves {
        writer.u32(u32::from(movement.source));
        writer.u32(u32::from(movement.destination));
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], KlbcError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| KlbcError("unexpected end of image".to_owned()))?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], KlbcError> {
        self.take(N)?
            .try_into()
            .map_err(|_| KlbcError("truncated image".to_owned()))
    }
    fn u8(&mut self) -> Result<u8, KlbcError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, KlbcError> {
        Ok(u32::from_le_bytes(self.bytes::<4>()?))
    }
    fn u64(&mut self) -> Result<u64, KlbcError> {
        Ok(u64::from_le_bytes(self.bytes::<8>()?))
    }
    fn i128(&mut self) -> Result<i128, KlbcError> {
        Ok(i128::from_le_bytes(self.bytes::<16>()?))
    }
    fn string(&mut self) -> Result<String, KlbcError> {
        let length = self.u32()? as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| KlbcError("invalid UTF-8 string".to_owned()))
    }
}

/// Decode a `.klbc` image back into a program.
///
/// # Errors
/// Returns [`KlbcError`] when the image is truncated or malformed.
pub fn decode(image: &[u8]) -> Result<Program, KlbcError> {
    let mut reader = Reader::new(image);
    if reader.take(4)? != MAGIC {
        return Err(KlbcError("bad magic".to_owned()));
    }
    let major = u16::from_le_bytes(reader.bytes::<2>()?);
    let minor = u16::from_le_bytes(reader.bytes::<2>()?);
    if major != 1 || minor > 1 {
        return Err(KlbcError(format!(
            "unsupported bytecode version {major}.{minor}"
        )));
    }
    let _ptrwidth = reader.u8()?;
    let _reserved = reader.take(3)?;
    let _flags = reader.u32()?;
    let section_count = reader.u32()? as usize;
    let mut table = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let kind = reader.u32()?;
        let offset = reader.u32()? as usize;
        let size = reader.u32()? as usize;
        let count = reader.u32()? as usize;
        table.push((kind, offset, size, count));
    }
    let section = |kind: u32| table.iter().find(|entry| entry.0 == kind).copied();
    let slice = |offset: usize, size: usize| {
        image
            .get(offset..offset + size)
            .ok_or_else(|| KlbcError("section out of range".to_owned()))
    };

    let (_, offset, size, string_count) =
        section(KIND_STRINGS).ok_or_else(|| KlbcError("missing STRINGS".to_owned()))?;
    let names = decode_strings(slice(offset, size)?, string_count)?;

    let (_, offset, size, const_count) =
        section(KIND_CONSTS).ok_or_else(|| KlbcError("missing CONSTS".to_owned()))?;
    let consts = decode_consts(slice(offset, size)?, const_count)?;

    let (_, offset, size, _) =
        section(KIND_CODE).ok_or_else(|| KlbcError("missing CODE".to_owned()))?;
    let code = decode_code(slice(offset, size)?)?;

    let (_, offset, size, func_count) =
        section(KIND_FUNCS).ok_or_else(|| KlbcError("missing FUNCS".to_owned()))?;
    let mut functions = decode_functions(slice(offset, size)?, func_count, &names, &code)?;
    if let Some((_, offset, size, count)) = section(KIND_BRANCHES) {
        decode_branches(slice(offset, size)?, count, &mut functions)?;
    } else if minor >= 1 {
        return Err(KlbcError("missing BRANCHES".to_owned()));
    }

    let globals = match section(KIND_GLOBALS) {
        Some((_, offset, size, global_count)) => {
            decode_globals(slice(offset, size)?, global_count)?
        }
        None => Vec::new(),
    };
    let mut initializers = Vec::new();
    if let Some((_, offset, size, count)) = section(KIND_INITIALIZERS) {
        let mut reader = Reader::new(slice(offset, size)?);
        for _ in 0..count {
            let index = reader.u32()? as usize;
            if index >= functions.len() {
                return Err(KlbcError("initializer out of range".to_owned()));
            }
            initializers.push(index);
        }
    }

    let imports = match section(KIND_IMPORTS) {
        Some((_, offset, size, count)) => decode_imports(slice(offset, size)?, count)?,
        None => Vec::new(),
    };
    for site in functions.iter().flat_map(|function| &function.call_sites) {
        if site
            .foreign
            .is_some_and(|import| import as usize >= imports.len())
        {
            return Err(KlbcError("call site names a missing import".to_owned()));
        }
    }

    Ok(Program {
        functions,
        consts,
        imports,
        globals,
        initializers,
    })
}

fn decode_globals(
    bytes: &[u8],
    count: usize,
) -> Result<Vec<crate::program::GlobalSpec>, KlbcError> {
    let mut reader = Reader::new(bytes);
    (0..count)
        .map(|_| {
            let size = reader.u32()?;
            let align = reader.u32()?;
            let init_len = reader.u32()? as usize;
            let mut init = Vec::with_capacity(init_len);
            for _ in 0..init_len {
                init.push(reader.u8()?);
            }
            Ok(crate::program::GlobalSpec { size, align, init })
        })
        .collect()
}

fn decode_strings(bytes: &[u8], count: usize) -> Result<Vec<String>, KlbcError> {
    let mut reader = Reader::new(bytes);
    (0..count).map(|_| reader.string()).collect()
}

fn decode_consts(bytes: &[u8], count: usize) -> Result<Vec<Const>, KlbcError> {
    let mut reader = Reader::new(bytes);
    (0..count)
        .map(|_| match reader.u8()? {
            0 => Ok(Const::Int(reader.i128()?)),
            1 => Ok(Const::Float(reader.u64()?)),
            2 => Ok(Const::String(reader.string()?)),
            _ => Err(KlbcError("unknown constant tag".to_owned())),
        })
        .collect()
}

fn decode_branches(
    bytes: &[u8],
    count: usize,
    functions: &mut [BcFunction],
) -> Result<(), KlbcError> {
    if count != functions.len() {
        return Err(KlbcError("BRANCHES function count mismatch".to_owned()));
    }
    let mut reader = Reader::new(bytes);
    for function in functions {
        let branch_count = reader.u32()? as usize;
        let mut branches = Vec::with_capacity(branch_count);
        for _ in 0..branch_count {
            let condition = match reader.u8()? {
                0 => None,
                1 => Some(BranchCondition::Register(branch_register(&mut reader)?)),
                2 => {
                    let result = branch_register(&mut reader)?;
                    let lhs = branch_register(&mut reader)?;
                    let rhs = branch_register(&mut reader)?;
                    let width = WidthCode::from_bits(u16::from(reader.u8()?))
                        .ok_or_else(|| KlbcError("unknown branch compare width".to_owned()))?;
                    let predicate = reader.u8()?;
                    Some(BranchCondition::IntegerCompare {
                        result,
                        lhs,
                        rhs,
                        width,
                        predicate,
                    })
                }
                tag => return Err(KlbcError(format!("unknown branch condition {tag}"))),
            };
            let then_edge = decode_branch_target(&mut reader)?;
            let else_edge = match reader.u8()? {
                0 => None,
                1 => Some(decode_branch_target(&mut reader)?),
                tag => return Err(KlbcError(format!("unknown else edge tag {tag}"))),
            };
            branches.push(BranchSite {
                condition,
                then_edge,
                else_edge,
            });
        }
        function.branches = branches;
    }
    Ok(())
}

fn branch_register(reader: &mut Reader<'_>) -> Result<u16, KlbcError> {
    u16::try_from(reader.u32()?).map_err(|_| oversized())
}

fn decode_branch_target(reader: &mut Reader<'_>) -> Result<BranchTarget, KlbcError> {
    let target = reader.u32()? as usize;
    let count = reader.u32()? as usize;
    let mut moves = Vec::with_capacity(count);
    for _ in 0..count {
        moves.push(RegisterMove {
            source: branch_register(reader)?,
            destination: branch_register(reader)?,
        });
    }
    Ok(BranchTarget { target, moves })
}

fn decode_code(bytes: &[u8]) -> Result<Vec<u64>, KlbcError> {
    if !bytes.len().is_multiple_of(8) {
        return Err(KlbcError("CODE section is not word-aligned".to_owned()));
    }
    let mut reader = Reader::new(bytes);
    (0..bytes.len() / 8).map(|_| reader.u64()).collect()
}

fn decode_functions(
    bytes: &[u8],
    count: usize,
    names: &[String],
    code: &[u64],
) -> Result<Vec<BcFunction>, KlbcError> {
    let mut reader = Reader::new(bytes);
    let mut functions = Vec::with_capacity(count);
    for index in 0..count {
        let name = names
            .get(index)
            .ok_or_else(|| KlbcError("missing function name".to_owned()))?;
        let register_count = reader.u32()?;
        let param_count = reader.u32()?;
        let returns_value = reader.u8()? != 0;
        let code_offset = reader.u32()? as usize;
        let code_length = reader.u32()? as usize;
        let function_code = code
            .get(code_offset..code_offset + code_length)
            .ok_or_else(|| KlbcError("code range out of bounds".to_owned()))?
            .to_vec();
        let call_count = reader.u32()? as usize;
        let mut call_sites = Vec::with_capacity(call_count);
        for _ in 0..call_count {
            let callee = reader.u32()? as usize;
            let first_arg = u16::try_from(reader.u32()?).map_err(|_| oversized())?;
            let arg_count = u16::try_from(reader.u32()?).map_err(|_| oversized())?;
            let result = u16::try_from(reader.u32()?).map_err(|_| oversized())?;
            let has_result = reader.u8()? != 0;
            let foreign_raw = reader.u32()?;
            let foreign = if foreign_raw == u32::MAX {
                None
            } else {
                Some(foreign_raw)
            };
            let target_raw = reader.u32()?;
            let target = if target_raw == u32::MAX {
                None
            } else {
                Some(u16::try_from(target_raw).map_err(|_| oversized())?)
            };
            let failed_raw = reader.u32()?;
            let branch = reader.u32()?;
            let fallible = if failed_raw == u32::MAX {
                None
            } else {
                Some(crate::program::FallibleSite {
                    failed: u16::try_from(failed_raw).map_err(|_| oversized())?,
                    branch,
                })
            };
            call_sites.push(CallSite {
                callee,
                first_arg,
                arg_count,
                result,
                has_result,
                foreign,
                target,
                fallible,
            });
        }
        let alloc_count = reader.u32()? as usize;
        let mut allocations = Vec::with_capacity(alloc_count);
        for _ in 0..alloc_count {
            allocations.push(AllocShape {
                size: reader.u32()?,
                align: reader.u32()?,
            });
        }
        functions.push(BcFunction {
            name: Name::new(name),
            register_count,
            param_count,
            returns_value,
            code: function_code,
            call_sites,
            allocations,
            branches: Vec::new(),
        });
    }
    Ok(functions)
}

fn oversized() -> KlbcError {
    KlbcError("register index does not fit in u16".to_owned())
}
