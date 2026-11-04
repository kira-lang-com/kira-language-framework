use std::fmt;

use tir_diag::Span;

use crate::text::{Token, TokenKind};
use crate::{BinaryInstruction, BinaryOperand, TextInstruction};

const SCOPE_SHIFT: u32 = 26;
const SCOPE_MASK: u64 = 0x7 << SCOPE_SHIFT;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
const MEMORY_MASK: u8 = 0x1f;

/// Which synchronization instruction is being represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuSynchronizationKind {
    Barrier,
    MemoryFence,
}

/// The effective execution or memory scope of a GPU synchronization instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuSynchronizationScope {
    Thread,
    Subgroup,
    Threadgroup,
    Device,
    System,
}

impl GpuSynchronizationScope {
    const fn binary_code(self) -> u8 {
        match self {
            Self::Thread => 1,
            Self::Subgroup => 2,
            Self::Threadgroup => 3,
            Self::Device => 4,
            Self::System => 5,
        }
    }

    const fn from_binary_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Thread),
            2 => Some(Self::Subgroup),
            3 => Some(Self::Threadgroup),
            4 => Some(Self::Device),
            5 => Some(Self::System),
            _ => None,
        }
    }
}

/// One of the five GPU memory domains named by TIR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum GpuMemoryDomain {
    Device = 0,
    Threadgroup = 1,
    Texture = 2,
    Tile = 3,
    ObjectPayload = 4,
}

impl GpuMemoryDomain {
    const fn bit(self) -> u8 {
        1 << self as u8
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "device" => Some(Self::Device),
            "threadgroup" => Some(Self::Threadgroup),
            "texture" => Some(Self::Texture),
            "tile" => Some(Self::Tile),
            "object_payload" => Some(Self::ObjectPayload),
            _ => None,
        }
    }
}

/// A bit set of GPU memory domains, using the TIR binary mask assignments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuMemoryDomains(u8);

impl GpuMemoryDomains {
    /// All domains selected when a text instruction omits its memory clause.
    pub const fn all() -> Self {
        Self(MEMORY_MASK)
    }

    /// Construct a set from a TIR binary memory-domain mask.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !MEMORY_MASK == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Return the TIR binary memory-domain mask.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Check whether this set contains a memory domain.
    pub const fn contains(self, domain: GpuMemoryDomain) -> bool {
        self.0 & domain.bit() != 0
    }

    fn insert(&mut self, domain: GpuMemoryDomain) {
        self.0 |= domain.bit();
    }
}

/// Resolved semantics for a TIR or TVISA GPU synchronization instruction.
///
/// Text parsing resolves omitted clauses to the defaults in TIR section 17.9.
/// `scope_is_default` preserves the distinction needed by the binary scope
/// field, where zero means to use the instruction-specific default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuSynchronization {
    pub kind: GpuSynchronizationKind,
    pub scope: GpuSynchronizationScope,
    pub scope_is_default: bool,
    pub memory: GpuMemoryDomains,
    pub unchecked_uniformity: bool,
}

impl GpuSynchronization {
    /// TIR CODE modifier bits for this instruction's scope and uniformity flag.
    pub const fn binary_modifier_bits(self) -> u64 {
        let scope_code = if self.scope_is_default {
            0
        } else {
            self.scope.binary_code()
        };
        ((scope_code as u64) << SCOPE_SHIFT)
            | if self.unchecked_uniformity {
                UNCHECKED_UNIFORMITY
            } else {
                0
            }
    }

    /// TIR CODE memory-domain mask for this instruction.
    pub const fn binary_memory_mask(self) -> u8 {
        self.memory.bits()
    }
}

/// A malformed text synchronization instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuSynchronizationParseError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for GpuSynchronizationParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GpuSynchronizationParseError {}

/// A malformed decoded binary synchronization instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuSynchronizationBinaryError {
    pub message: &'static str,
}

impl fmt::Display for GpuSynchronizationBinaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for GpuSynchronizationBinaryError {}

/// Parse and resolve one text `gpu.barrier` or `gpu.memfence` instruction.
///
/// Other opcodes return `Ok(None)`. An omitted barrier scope resolves to
/// threadgroup; an omitted fence scope resolves to device. An omitted memory
/// clause resolves to all five TIR memory domains.
pub fn parse_gpu_synchronization(
    instruction: &TextInstruction,
) -> Result<Option<GpuSynchronization>, GpuSynchronizationParseError> {
    let kind = match instruction.opcode.as_str() {
        "gpu.barrier" => GpuSynchronizationKind::Barrier,
        "gpu.memfence" => GpuSynchronizationKind::MemoryFence,
        _ => return Ok(None),
    };

    let mut index = instruction_operands_start(instruction).ok_or_else(|| {
        parse_error(
            instruction.span,
            "malformed GPU synchronization instruction opcode",
        )
    })?;
    let mut scope = default_scope(kind);
    let mut scope_is_default = true;
    let mut scope_seen = false;
    let mut memory = GpuMemoryDomains::all();
    let mut memory_seen = false;
    let mut unchecked_uniformity = false;
    let mut unchecked_uniformity_seen = false;

    while index < instruction.tokens.len() {
        let token = &instruction.tokens[index];
        if punctuation(token, ']') {
            return Err(parse_error(
                token.span,
                "unmatched closing GPU synchronization clause",
            ));
        }
        if !punctuation(token, '[') {
            return Err(parse_error(
                token.span,
                "GPU synchronization instructions accept only bracketed clauses",
            ));
        }
        let Some(close) = matching_bracket(&instruction.tokens, index) else {
            return Err(parse_error(
                token.span,
                "unterminated GPU synchronization clause",
            ));
        };
        let clause = &instruction.tokens[index + 1..close];
        let clause_span = Span::new(token.span.start, instruction.tokens[close].span.end);

        if clause.len() == 1 && atom_at(clause, 0) == Some("unchecked_uniformity") {
            if kind != GpuSynchronizationKind::Barrier {
                return Err(parse_error(
                    clause_span,
                    "unchecked_uniformity is valid only on gpu.barrier",
                ));
            }
            if unchecked_uniformity_seen {
                return Err(parse_error(
                    clause_span,
                    "GPU barrier has multiple unchecked_uniformity clauses",
                ));
            }
            unchecked_uniformity = true;
            unchecked_uniformity_seen = true;
        } else if atom_at(clause, 0) == Some("memory") {
            if memory_seen {
                return Err(parse_error(
                    clause_span,
                    "GPU synchronization instruction has multiple memory clauses",
                ));
            }
            memory_seen = true;
            memory = parse_memory_clause(clause, kind, clause_span)?;
        } else {
            if scope_seen {
                return Err(parse_error(
                    clause_span,
                    "GPU synchronization instruction has multiple scope clauses",
                ));
            }
            scope_seen = true;
            scope = parse_scope_clause(clause, kind, clause_span)?;
            scope_is_default = false;
        }
        index = close + 1;
    }

    Ok(Some(GpuSynchronization {
        kind,
        scope,
        scope_is_default,
        memory,
        unchecked_uniformity,
    }))
}

/// Resolve the semantics from a decoded TIR binary CODE instruction.
pub fn parse_binary_gpu_synchronization(
    instruction: &BinaryInstruction,
) -> Result<Option<GpuSynchronization>, GpuSynchronizationBinaryError> {
    let kind = match instruction.opcode {
        0x402 => GpuSynchronizationKind::Barrier,
        0x403 => GpuSynchronizationKind::MemoryFence,
        _ => return Ok(None),
    };
    let scope_code = ((instruction.modifiers & SCOPE_MASK) >> SCOPE_SHIFT) as u8;
    let unchecked_uniformity = instruction.modifiers & UNCHECKED_UNIFORMITY != 0;
    let allowed_modifiers = SCOPE_MASK
        | if kind == GpuSynchronizationKind::Barrier {
            UNCHECKED_UNIFORMITY
        } else {
            0
        };
    if instruction.modifiers & !allowed_modifiers != 0
        || (kind == GpuSynchronizationKind::MemoryFence && unchecked_uniformity)
    {
        return Err(binary_error("invalid GPU synchronization modifiers"));
    }

    let scope_is_default = scope_code == 0;
    let scope = if scope_is_default {
        default_scope(kind)
    } else {
        GpuSynchronizationScope::from_binary_code(scope_code)
            .filter(|scope| scope_is_valid(*scope, kind))
            .ok_or_else(|| binary_error("invalid GPU synchronization scope"))?
    };
    let [BinaryOperand::Index(mask)] = instruction.operands.as_slice() else {
        return Err(binary_error(
            "GPU synchronization instruction must contain one memory-domain mask",
        ));
    };
    let mask = u8::try_from(*mask)
        .map_err(|_| binary_error("GPU memory-domain mask does not fit in one byte"))?;
    let memory = GpuMemoryDomains::from_bits(mask)
        .ok_or_else(|| binary_error("reserved GPU memory-domain bits are set"))?;

    Ok(Some(GpuSynchronization {
        kind,
        scope,
        scope_is_default,
        memory,
        unchecked_uniformity,
    }))
}

fn parse_scope_clause(
    clause: &[Token],
    kind: GpuSynchronizationKind,
    span: Span,
) -> Result<GpuSynchronizationScope, GpuSynchronizationParseError> {
    let scope = if clause.len() == 1 {
        atom_at(clause, 0).and_then(parse_scope_name)
    } else {
        None
    };
    if let Some(scope) = scope.filter(|scope| scope_is_valid(*scope, kind)) {
        Ok(scope)
    } else {
        Err(parse_error(
            span,
            if kind == GpuSynchronizationKind::Barrier {
                "gpu.barrier scope must be subgroup or threadgroup"
            } else {
                "gpu.memfence scope must be thread, subgroup, threadgroup, device, or system"
            },
        ))
    }
}

fn parse_memory_clause(
    clause: &[Token],
    kind: GpuSynchronizationKind,
    span: Span,
) -> Result<GpuMemoryDomains, GpuSynchronizationParseError> {
    if !punctuation_at(clause, 1, ':') {
        return Err(parse_error(span, "memory clause must use 'memory: ...'"));
    }
    let values = &clause[2..];
    let mut domains = GpuMemoryDomains(0);
    let mut expect_value = true;
    for token in values {
        if expect_value {
            let Some(name) = atom(token) else {
                return Err(parse_error(
                    token.span,
                    "memory clause expects a memory class",
                ));
            };
            let Some(domain) = GpuMemoryDomain::from_name(name) else {
                return Err(parse_error(
                    token.span,
                    format!("{name} is not valid in a {} memory clause", opcode(kind)),
                ));
            };
            if domains.contains(domain) {
                return Err(parse_error(
                    token.span,
                    format!("memory class '{name}' appears more than once"),
                ));
            }
            domains.insert(domain);
        } else if !punctuation(token, ',') {
            return Err(parse_error(
                token.span,
                "memory classes must be comma-separated",
            ));
        }
        expect_value = !expect_value;
    }
    if values.is_empty() || expect_value {
        return Err(parse_error(
            span,
            "memory clause requires at least one memory class",
        ));
    }
    Ok(domains)
}

fn instruction_operands_start(instruction: &TextInstruction) -> Option<usize> {
    let mut index = 0;
    if !instruction.results.is_empty() {
        index = instruction
            .tokens
            .iter()
            .position(|token| punctuation(token, '='))?
            + 1;
    }
    let parts = instruction.opcode.split('.').collect::<Vec<_>>();
    for (part_index, part) in parts.iter().enumerate() {
        if atom_at(&instruction.tokens, index) != Some(part) {
            return None;
        }
        index += 1;
        if part_index + 1 < parts.len() {
            if !punctuation_at(&instruction.tokens, index, '.') {
                return None;
            }
            index += 1;
        }
    }
    Some(index)
}

fn matching_bracket(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if punctuation(token, '[') {
            depth += 1;
        } else if punctuation(token, ']') {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn parse_scope_name(name: &str) -> Option<GpuSynchronizationScope> {
    match name {
        "thread" => Some(GpuSynchronizationScope::Thread),
        "subgroup" => Some(GpuSynchronizationScope::Subgroup),
        "threadgroup" => Some(GpuSynchronizationScope::Threadgroup),
        "device" => Some(GpuSynchronizationScope::Device),
        "system" => Some(GpuSynchronizationScope::System),
        _ => None,
    }
}

const fn default_scope(kind: GpuSynchronizationKind) -> GpuSynchronizationScope {
    match kind {
        GpuSynchronizationKind::Barrier => GpuSynchronizationScope::Threadgroup,
        GpuSynchronizationKind::MemoryFence => GpuSynchronizationScope::Device,
    }
}

const fn scope_is_valid(scope: GpuSynchronizationScope, kind: GpuSynchronizationKind) -> bool {
    match kind {
        GpuSynchronizationKind::Barrier => matches!(
            scope,
            GpuSynchronizationScope::Subgroup | GpuSynchronizationScope::Threadgroup
        ),
        GpuSynchronizationKind::MemoryFence => true,
    }
}

const fn opcode(kind: GpuSynchronizationKind) -> &'static str {
    match kind {
        GpuSynchronizationKind::Barrier => "gpu.barrier",
        GpuSynchronizationKind::MemoryFence => "gpu.memfence",
    }
}

const fn binary_error(message: &'static str) -> GpuSynchronizationBinaryError {
    GpuSynchronizationBinaryError { message }
}

fn parse_error(span: Span, message: impl Into<String>) -> GpuSynchronizationParseError {
    GpuSynchronizationParseError {
        span,
        message: message.into(),
    }
}

fn punctuation_at(tokens: &[Token], index: usize, character: char) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| punctuation(token, character))
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}
