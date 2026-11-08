//! Conservative multi-block direct stack initialization (TIR 10.5, T1).
//! Independent entry stack allocations, no aliases, no consuming reads, and
//! reverse-order deallocation immediately before every exit. The verifier
//! decides whether a path may read each slot before initialization.
use tir_format::text::TokenKind;
use tir_format::{
    FunctionBody, TextInstruction, TextItemKind, parse_function_body, parse_text_module,
};
use tir_types::{FloatType, Type};

use crate::{CanonicalizeError, error};

pub(super) fn validate(body: &FunctionBody) -> Result<(), CanonicalizeError> {
    let Some(entry) = body.blocks.first() else {
        return Err(error("CFG scalar memory has no entry block"));
    };
    if body.blocks.len() < 2 {
        return Err(error("CFG scalar memory requires multiple blocks"));
    }
    let allocations = entry
        .instructions
        .iter()
        .filter(|op| op.opcode == "stack.alloc")
        .collect::<Vec<_>>();
    if allocations.is_empty() {
        return Err(error("CFG scalar memory requires entry allocations"));
    }
    let mut slots = Vec::with_capacity(allocations.len());
    for allocation in &allocations {
        let [slot] = allocation.results.as_slice() else {
            return Err(error("CFG stack.alloc must define one address"));
        };
        if slots.contains(&slot) {
            return Err(error("CFG stack.alloc defines a duplicate address"));
        }
        slots.push(slot);
    }
    let mut exits = 0;
    for (block_index, block) in body.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            if instruction.opcode == "stack.alloc" && block_index != 0 {
                return Err(error("CFG scalar memory requires entry-only allocations"));
            }
            let refs = references(instruction);
            for slot in &slots {
                if !refs.contains(slot) {
                    continue;
                }
                let allowed = match instruction.opcode.as_str() {
                    "load" => {
                        refs.len() == 1
                            && refs.first() == Some(slot)
                            && matches!(mode(instruction).as_deref(), Some("trivial" | "copy"))
                    }
                    "store" => {
                        refs.len() == 2
                            && refs.last() == Some(slot)
                            && refs.first() != Some(slot)
                            && matches!(mode(instruction).as_deref(), Some("init" | "assign"))
                    }
                    "copy.addr" | "move.addr" => {
                        refs.len() == 2
                            && refs[0] != refs[1]
                            && refs
                                .iter()
                                .all(|address| slots.iter().any(|slot| *slot == address))
                            && matches!(mode(instruction).as_deref(), Some("init" | "assign"))
                    }
                    "stack.dealloc" => refs.len() == 1 && refs.first() == Some(slot),
                    _ => false,
                };
                if !allowed {
                    return Err(error(format!(
                        "CFG stack slot %{slot} escapes or has an unsupported memory use"
                    )));
                }
            }
        }
        if block
            .instructions
            .last()
            .is_some_and(|op| op.opcode == "return")
        {
            exits += 1;
            let deallocations = block
                .instructions
                .iter()
                .filter(|instruction| instruction.opcode == "stack.dealloc")
                .collect::<Vec<_>>();
            if deallocations.len() != slots.len()
                || block.instructions.len() < slots.len() + 1
                || !block.instructions
                    [block.instructions.len() - slots.len() - 1..block.instructions.len() - 1]
                    .iter()
                    .zip(slots.iter().rev())
                    .all(|(instruction, slot)| {
                        instruction.opcode == "stack.dealloc"
                            && references(instruction).first() == Some(slot)
                    })
            {
                return Err(error(
                    "every CFG return must immediately follow reverse-order stack.dealloc for all entry slots",
                ));
            }
        } else if block
            .instructions
            .iter()
            .any(|op| op.opcode == "stack.dealloc")
        {
            return Err(error("CFG stack.dealloc may only precede a return"));
        }
    }
    if exits == 0 {
        return Err(error("CFG scalar memory has no return exit"));
    }
    Ok(())
}

pub(super) fn zero_initialize(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|e| error(format!("TIR text: {e}")))?;
    let diagnostics = tir_verify::verify_text_module(&module, Default::default());
    let mut edits = Vec::new();
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let Some(body) = parse_function_body(module.item_text(*item))
            .map_err(|e| error(format!("function body: {e}")))?
        else {
            continue;
        };
        if body.blocks.len() < 2 {
            continue;
        }
        if validate(&body).is_err() {
            continue;
        }
        let mut names = body
            .blocks
            .iter()
            .flat_map(|block| {
                block.parameters.iter().map(|p| p.name.clone()).chain(
                    block
                        .instructions
                        .iter()
                        .flat_map(|i| i.results.iter().cloned()),
                )
            })
            .collect::<std::collections::HashSet<_>>();
        let mut suffix = 0;
        for allocation in body.blocks[0]
            .instructions
            .iter()
            .filter(|operation| operation.opcode == "stack.alloc")
        {
            let [slot] = allocation.results.as_slice() else {
                continue;
            };
            let has_uninitialized_read = diagnostics.iter().any(|diagnostic| {
                diagnostic.code.to_string() == "TIR-OWN-0014"
                    && diagnostic.message.contains(&format!("`%{slot}`"))
                    && diagnostic.primary_span.is_some_and(|span| {
                        span.start >= item.span.start && span.start < item.span.end
                    })
            });
            if !has_uninitialized_read {
                continue;
            }
            // Inserting zero must not hide an independent double-initialization error.
            if diagnostics.iter().any(|diagnostic| {
                diagnostic.code.to_string() == "TIR-OWN-0015"
                    && diagnostic.message.contains(&format!("`%{slot}`"))
                    && diagnostic.primary_span.is_some_and(|span| {
                        span.start >= item.span.start && span.start < item.span.end
                    })
            }) {
                continue;
            }
            let instruction_source = module
                .item_text(*item)
                .get(allocation.span.start..allocation.span.end)
                .unwrap_or("");
            let Some(ty_text) = instruction_source
                .split_once("stack.alloc ")
                .map(|(_, ty)| ty.trim())
            else {
                continue;
            };
            let zero = match tir_format::parse_type(ty_text) {
                Ok(Type::Integer(bits)) if bits <= 128 => format!("const.int i{bits}, 0"),
                Ok(Type::Integer(bits)) => format!("const.zero i{bits}"),
                Ok(Type::Float(float))
                    if !matches!(
                        float,
                        FloatType::F4E2M1 | FloatType::F6E2M3 | FloatType::F6E3M2
                    ) =>
                {
                    format!("const.zero {float}")
                }
                Ok(ty @ Type::FixedVector { .. }) if crate::is_pure_value_type(&ty) => {
                    format!("const.zero {ty}")
                }
                _ => continue,
            };
            let source_offset = item.span.start.saturating_add(allocation.span.start);
            let before = source
                .get(..source_offset)
                .ok_or_else(|| error("invalid CFG allocation span"))?;
            let line_start = before.rfind('\n').map_or(0, |index| index + 1);
            let indentation = &source[line_start..source_offset];
            if !indentation.chars().all(char::is_whitespace) {
                return Err(error("CFG allocation has malformed indentation"));
            }
            let after = source
                .get(source_offset..)
                .ok_or_else(|| error("invalid CFG allocation span"))?;
            let line_end = after
                .find('\n')
                .ok_or_else(|| error("CFG allocation is not line-terminated"))?;
            let insertion = source_offset + line_end + 1;
            let zero_name = loop {
                let candidate = format!("tir_cfg_zero_{suffix}");
                suffix += 1;
                if names.insert(candidate.clone()) {
                    break candidate;
                }
            };
            edits.push((insertion, insertion, format!(
                "{indentation}%{zero_name} = {zero}\n{indentation}store [init] %{zero_name} to %{slot}\n"
            )));
            for instruction in body.blocks.iter().flat_map(|b| &b.instructions) {
                if !matches!(
                    instruction.opcode.as_str(),
                    "store" | "copy.addr" | "move.addr"
                ) || references(instruction).last() != Some(slot)
                {
                    continue;
                }
                let absolute = item.span.start.saturating_add(instruction.span.start);
                let line = source
                    .get(absolute..)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                let Some(mode_index) = line.find("[init]") else {
                    continue;
                };
                edits.push((
                    absolute + mode_index,
                    absolute + mode_index + 6,
                    "[assign]".to_owned(),
                ));
            }
        }
    }
    edits.sort_by(|left, right| right.0.cmp(&left.0));
    let mut transformed = source.to_owned();
    for (start, end, replacement) in edits {
        transformed.replace_range(start..end, &replacement);
    }
    Ok(transformed)
}

fn mode(instruction: &TextInstruction) -> Option<String> {
    instruction.tokens.windows(3).find_map(|group| {
        match (&group[0].kind, &group[1].kind, &group[2].kind) {
            (TokenKind::Punctuation('['), TokenKind::Atom(value), TokenKind::Punctuation(']')) => {
                Some(value.clone())
            }
            _ => None,
        }
    })
}

fn references(instruction: &TextInstruction) -> Vec<String> {
    let mut names = instruction
        .tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(result) = instruction.results.first()
        && let Some(position) = names.iter().position(|name| name == result)
    {
        names.remove(position);
    }
    names
}

#[cfg(test)]
mod tests {
    use crate::canonicalize_text;

    fn module(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.branch(%v: i32, %condition: i1) -> i32 engine(native) {{\n{body}\n}}\n"
        )
    }

    #[test]
    fn partially_initialized_branch_is_zeroed_at_entry() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  store [init] %v to %slot\n  br bb2()\nbb2:\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&raw).expect("branch definite initialization");
        assert!(canonical.contains("const.int i32, 0"));
        assert_eq!(canonical.matches("store [init]").count(), 1);
        assert_eq!(canonical.matches("store [assign]").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("branch canonicalization is stable")
        );
    }

    #[test]
    fn branching_with_proven_initialization_needs_no_rewrite() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  cond.br %condition, bb1(), bb2()\nbb1:\n  br bb2()\nbb2:\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&raw).expect("definitely initialized CFG");
        assert!(!canonical.contains("tir_cfg_zero"));
        assert!(!canonical.contains("const.int i32, 0"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable CFG")
        );
    }

    #[test]
    fn invalid_cfg_memory_is_rejected() {
        for body in [
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  cond.br %condition, bb1(), bb2()\nbb1:\n  store [init] %v to %slot\n  br bb2()\nbb2:\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  %alias = copy %slot\n  br bb2()\nbb2:\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  stack.dealloc %slot\n  br bb2()\nbb2:\n  %loaded = load [trivial] %slot\n  return %loaded",
        ] {
            assert!(
                canonicalize_text(&module(body)).is_err(),
                "invalid CFG memory canonicalized: {body}"
            );
        }
    }

    #[test]
    fn scalar_slot_is_zeroed_across_loop_back_edge() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  br bb1()\nbb1:\n  %loaded = load [trivial] %slot\n  cond.br %condition, bb1(), bb2()\nbb2:\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&raw).expect("entry zero dominates loop reads");
        assert!(canonical.contains("const.int i32, 0"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("loop canonicalization is stable")
        );
    }

    #[test]
    fn scalar_slot_is_zeroed_before_multiple_exits() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb2:\n  store [init] %v to %slot\n  stack.dealloc %slot\n  return %v\nbb1:\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&raw).expect("zeroing across exits");
        assert!(canonical.contains("const.int i32, 0"));
        assert!(canonical.contains("store [assign]"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("multiple exits canonicalization remains stable")
        );
    }

    #[test]
    fn independent_cfg_slots_get_distinct_zero_initializers() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  store [init] %v to %first\n  br bb2()\nbb2:\n  %left = load [trivial] %first\n  %right = load [trivial] %second\n  %sum = add %left, %right\n  stack.dealloc %second\n  stack.dealloc %first\n  return %sum",
        );
        let canonical = canonicalize_text(&raw).expect("independent stack slots zero successfully");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 2);
        assert_eq!(canonical.matches("store [init]").count(), 2);
        assert_eq!(canonical.matches("store [assign]").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("multi-slot CFG remains canonical")
        );
    }

    #[test]
    fn only_uninitialized_cfg_slot_needs_zeroing() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  store [init] %v to %first\n  cond.br %condition, bb1(), bb2()\nbb1:\n  br bb2()\nbb2:\n  %left = load [trivial] %first\n  %right = load [trivial] %second\n  %sum = add %left, %right\n  stack.dealloc %second\n  stack.dealloc %first\n  return %sum",
        );
        let canonical = canonicalize_text(&raw).expect("one stack slot is definitely initialized");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 1);
        assert_eq!(canonical.matches("store [init]").count(), 2);
        assert_eq!(canonical.matches("store [assign]").count(), 0);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("partial zeroing stays canonical")
        );
    }

    #[test]
    fn independent_cfg_slots_require_lifo_deallocation() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  br bb2()\nbb2:\n  %left = load [trivial] %first\n  stack.dealloc %first\n  stack.dealloc %second\n  return %left",
        );
        assert!(
            canonicalize_text(&raw).is_err(),
            "non-LIFO multi-slot CFG is not canonical"
        );
    }

    #[test]
    fn mixed_scalar_and_vector_cfg_slots_are_initialized() {
        let raw = "module @Mixed tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Mixed.compute(%v: f32, %c: i1) -> f32 engine(native) {\nbb0(%v: f32, %c: i1):\n  %scalar = stack.alloc f32\n  %vector = stack.alloc vec<4 x f32>\n  cond.br %c, bb1(), bb2()\nbb1:\n  store [init] %v to %scalar\n  br bb2()\nbb2:\n  %loaded = load [trivial] %scalar\n  %other = load [trivial] %vector\n  stack.dealloc %vector\n  stack.dealloc %scalar\n  return %loaded\n}\n";
        let canonical = canonicalize_text(raw).expect("mixed CFG scalar and vector initialization");
        assert!(canonical.contains("const.zero f32"));
        assert!(canonical.contains("const.zero vec<4 x f32>"));
        assert_eq!(canonical.matches("store [assign]").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("mixed CFG is canonical")
        );
    }

    #[test]
    fn branch_local_address_copy_initializes_source_and_destination() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %source = stack.alloc i32\n  %destination = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  copy.addr %source to [init] %destination\n  br bb2()\nbb2:\n  %left = load [trivial] %source\n  %right = load [trivial] %destination\n  %sum = add %left, %right\n  stack.dealloc %destination\n  stack.dealloc %source\n  return %sum",
        );
        let canonical =
            canonicalize_text(&raw).expect("CFG copy source and destination are initialized");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 2);
        assert!(canonical.contains("copy.addr %") && canonical.contains("to [assign] %"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("address copy CFG canonicalization is stable")
        );
    }

    #[test]
    fn self_copy_or_unknown_address_is_rejected_in_cfg() {
        for copy in [
            "copy.addr %source to [init] %source",
            "copy.addr %source to [init] %not_allocated",
        ] {
            let raw = module(&format!(
                "bb0(%v: i32, %condition: i1):\n  %source = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  {copy}\n  br bb2()\nbb2:\n  %loaded = load [trivial] %source\n  stack.dealloc %source\n  return %loaded"
            ));
            assert!(
                canonicalize_text(&raw).is_err(),
                "invalid CFG address copy accepted: {copy}"
            );
        }
    }

    #[test]
    fn branch_local_move_address_initializes_and_consumes_source() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %source = stack.alloc i32\n  %destination = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  move.addr %source to [init] %destination\n  br bb2()\nbb2:\n  %value = load [trivial] %destination\n  stack.dealloc %destination\n  stack.dealloc %source\n  return %value",
        );
        let canonical =
            canonicalize_text(&raw).expect("CFG move source and destination get zero initializers");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 2);
        assert!(canonical.contains("move.addr %") && canonical.contains("to [assign] %"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("CFG move is canonical")
        );
    }

    #[test]
    fn use_after_branch_local_move_is_rejected() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %source = stack.alloc i32\n  %destination = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  move.addr %source to [init] %destination\n  br bb2()\nbb2:\n  %invalid = load [trivial] %source\n  stack.dealloc %destination\n  stack.dealloc %source\n  return %invalid",
        );
        assert!(
            canonicalize_text(&raw).is_err(),
            "CFG must reject read after consuming move on one path"
        );
    }

    #[test]
    fn self_move_address_is_rejected_in_cfg() {
        let raw = module(
            "bb0(%v: i32, %condition: i1):\n  %slot = stack.alloc i32\n  cond.br %condition, bb1(), bb2()\nbb1:\n  move.addr %slot to [init] %slot\n  br bb2()\nbb2:\n  %value = load [trivial] %slot\n  stack.dealloc %slot\n  return %value",
        );
        assert!(
            canonicalize_text(&raw).is_err(),
            "self move must be rejected"
        );
    }
}
