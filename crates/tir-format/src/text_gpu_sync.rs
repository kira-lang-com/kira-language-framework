//! GPU barriers and memory fences (0x402, 0x403) with section 22.8 modifiers.
use std::fmt::Write as _;

use tir_diag::Span;

use crate::types::{BinaryInstruction, BinaryOperand};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const SCOPE_SHIFT: u64 = 26;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
const MEMORY_CLASSES: [&str; 5] = ["device", "threadgroup", "texture", "tile", "object_payload"];

pub(crate) struct GpuSync {
    opcode: u64,
    modifiers: u64,
    mask: u8,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "gpu.barrier" | "gpu.memfence")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<GpuSync, TextParseError> {
    let barrier = match instruction.opcode.as_str() {
        "gpu.barrier" => true,
        "gpu.memfence" => false,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "expected GPU synchronization opcode",
            ));
        }
    };
    if !instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "GPU synchronization cannot define SSA results",
        ));
    }
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .and_then(|line| line.trim().strip_prefix(&instruction.opcode))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "GPU synchronization source is malformed",
            )
        })?;
    let mut rest = text.trim();
    let mut scope = None;
    let mut memory = None;
    let mut unchecked_uniformity = false;
    while !rest.is_empty() {
        let content = rest
            .strip_prefix('[')
            .and_then(|tail| tail.split_once(']'))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "GPU synchronization requires bracketed clauses",
                )
            })?;
        let (clause, remainder) = content;
        let clause = clause.trim();
        if clause == "unchecked_uniformity" {
            if !barrier || unchecked_uniformity {
                return Err(error(
                    item,
                    instruction.span,
                    "invalid or repeated unchecked_uniformity clause",
                ));
            }
            unchecked_uniformity = true;
        } else if let Some(domains) = clause.strip_prefix("memory:") {
            if memory.is_some() {
                return Err(error(item, instruction.span, "duplicate GPU memory clause"));
            }
            let mut mask = 0u8;
            let domains = domains.trim();
            if domains.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "GPU memory clause cannot be empty",
                ));
            }
            for domain in domains.split(',') {
                let name = domain.trim();
                let index = MEMORY_CLASSES
                    .iter()
                    .position(|candidate| *candidate == name)
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "GPU memory domain is not encodable in the section 22.7 five-bit mask",
                        )
                    })?;
                let bit = 1 << index;
                if mask & bit != 0 {
                    return Err(error(item, instruction.span, "duplicate GPU memory domain"));
                }
                mask |= bit;
            }
            memory = Some(mask);
        } else {
            if scope.is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate GPU synchronization scope",
                ));
            }
            let code = match clause {
                "thread" if !barrier => 1,
                "subgroup" => 2,
                "threadgroup" => 3,
                "device" if !barrier => 4,
                "system" if !barrier => 5,
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "invalid GPU synchronization scope",
                    ));
                }
            };
            scope = Some(code);
        }
        rest = remainder.trim();
    }
    Ok(GpuSync {
        opcode: if barrier { 0x402 } else { 0x403 },
        modifiers: (scope.unwrap_or(0) << SCOPE_SHIFT)
            | if unchecked_uniformity {
                UNCHECKED_UNIFORMITY
            } else {
                0
            },
        mask: memory.unwrap_or(0x1f),
    })
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let operation = parse(instruction, source, item)?;
    for value in [operation.opcode, operation.modifiers, 0] {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    bytes.push(operation.mask);
    Ok(())
}

pub(crate) fn format(text: &mut String, operation: &BinaryInstruction) -> Result<(), FormatError> {
    let barrier = match operation.opcode {
        0x402 => true,
        0x403 => false,
        _ => return Err(malformed("not a GPU synchronization instruction")),
    };
    let scope_code = (operation.modifiers >> SCOPE_SHIFT) & 7;
    let allowed = (7 << SCOPE_SHIFT) | if barrier { UNCHECKED_UNIFORMITY } else { 0 };
    if operation.modifiers & !allowed != 0 || operation.result_count != 0 {
        return Err(malformed(
            "invalid GPU synchronization modifier word or result count",
        ));
    }
    let scope = match scope_code {
        0 => None,
        1 if !barrier => Some("thread"),
        2 => Some("subgroup"),
        3 => Some("threadgroup"),
        4 if !barrier => Some("device"),
        5 if !barrier => Some("system"),
        _ => return Err(malformed("invalid GPU synchronization scope code")),
    };
    let [BinaryOperand::Index(mask)] = operation.operands.as_slice() else {
        return Err(malformed("GPU synchronization needs one memory mask"));
    };
    if *mask == 0 || *mask > 0x1f {
        return Err(malformed(
            "GPU synchronization memory mask is not text-representable",
        ));
    }
    text.push_str(if barrier {
        "    gpu.barrier"
    } else {
        "    gpu.memfence"
    });
    if let Some(scope) = scope {
        write!(text, " [{scope}]").map_err(|issue| malformed(issue.to_string()))?;
    }
    if *mask != 0x1f {
        let domains = MEMORY_CLASSES
            .iter()
            .enumerate()
            .filter_map(|(index, name)| (mask & (1 << index) != 0).then_some(*name))
            .collect::<Vec<_>>();
        write!(text, " [memory: {}]", domains.join(", "))
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if operation.modifiers & UNCHECKED_UNIFORMITY != 0 {
        text.push_str(" [unchecked_uniformity]");
    }
    text.push('\n');
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
