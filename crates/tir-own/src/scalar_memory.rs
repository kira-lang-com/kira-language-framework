use std::collections::HashSet;

use tir_format::text::TokenKind;
use tir_format::{
    FunctionBody, TextInstruction, TextItemKind, parse_function_body, parse_text_module,
};
use tir_types::{FloatType, Type};

use crate::{CanonicalizeError, error};

pub(super) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "stack.alloc"
            | "stack.dealloc"
            | "load"
            | "load.borrow"
            | "store"
            | "copy.addr"
            | "move.addr"
            | "destroy.addr"
            | "access.begin"
            | "access.end"
    )
}

pub(super) fn zero_initialize_simple_slots(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|e| error(format!("TIR text: {e}")))?;
    let mut edits = Vec::new();
    let mut next_zero = 0usize;

    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let Some(body) = parse_function_body(module.item_text(*item))
            .map_err(|e| error(format!("function body: {e}")))?
        else {
            continue;
        };
        let [block] = body.blocks.as_slice() else {
            continue;
        };
        let mut names = block
            .parameters
            .iter()
            .map(|p| p.name.clone())
            .chain(
                block
                    .instructions
                    .iter()
                    .flat_map(|i| i.results.iter().cloned()),
            )
            .collect::<HashSet<_>>();

        for instruction in &block.instructions {
            if instruction.opcode != "stack.alloc" {
                continue;
            }
            let [slot] = instruction.results.as_slice() else {
                continue;
            };
            let mut needs_zero = false;
            let mut supported = true;
            let mut initializer = None;
            let mut consumed = false;
            for use_instruction in &block.instructions {
                let references = value_references(use_instruction);
                if !references.contains(slot) {
                    continue;
                }
                if consumed && use_instruction.opcode != "stack.dealloc" {
                    supported = false;
                    break;
                }
                match use_instruction.opcode.as_str() {
                    "load" => {
                        let text = module
                            .item_text(*item)
                            .get(use_instruction.span.start..use_instruction.span.end)
                            .unwrap_or("");
                        if text.contains("load [trivial]") || text.contains("load [copy]") {
                            needs_zero |= initializer.is_none();
                        } else if text.contains("load [take]") {
                            needs_zero |= initializer.is_none();
                            consumed = true;
                        } else {
                            supported = false;
                        }
                    }
                    "load.borrow" => needs_zero |= initializer.is_none(),
                    "copy.addr" if references.first() == Some(slot) && references.len() == 2 => {
                        needs_zero |= initializer.is_none();
                    }
                    "move.addr" if references.first() == Some(slot) && references.len() == 2 => {
                        needs_zero |= initializer.is_none();
                        consumed = true;
                    }
                    "destroy.addr" if references.len() == 1 && references.first() == Some(slot) => {
                        needs_zero |= initializer.is_none();
                        consumed = true;
                    }
                    "store" if references.len() == 2 && references.last() == Some(slot) => {
                        let absolute = item.span.start.saturating_add(use_instruction.span.start);
                        let text = source
                            .get(absolute..)
                            .unwrap_or("")
                            .lines()
                            .next()
                            .unwrap_or("");
                        if !text.trim_start().starts_with("store [init] ") || initializer.is_some()
                        {
                            supported = false;
                        } else {
                            initializer = Some(use_instruction);
                        }
                    }
                    "stack.dealloc" => {}
                    _ => supported = false,
                }
                if !supported {
                    break;
                }
            }
            if !supported || !needs_zero {
                continue;
            }
            let function_source = module.item_text(*item);
            let instruction_text = function_source
                .get(instruction.span.start..instruction.span.end)
                .unwrap_or("");
            let Some(ty_text) = instruction_text
                .split_once("stack.alloc ")
                .map(|(_, ty)| ty.trim())
            else {
                continue;
            };
            let zero_expression = match tir_format::parse_type(ty_text) {
                Ok(Type::Integer(width)) if width <= 128 => {
                    format!("const.int i{width}, 0")
                }
                Ok(Type::Integer(width)) => format!("const.zero i{width}"),
                Ok(Type::Float(float))
                    if matches!(
                        float,
                        FloatType::F16
                            | FloatType::Bf16
                            | FloatType::F32
                            | FloatType::F64
                            | FloatType::F128
                            | FloatType::F8E4M3
                            | FloatType::F8E5M2
                            | FloatType::E8M0
                    ) =>
                {
                    format!("const.zero {float}")
                }
                Ok(ty @ Type::FixedVector { .. }) if crate::is_pure_value_type(&ty) => {
                    format!("const.zero {ty}")
                }
                _ => continue,
            };

            let name = loop {
                let candidate = format!("tir_zero_{next_zero}");
                next_zero += 1;
                if names.insert(candidate.clone()) {
                    break candidate;
                }
            };
            let start = item.span.start.saturating_add(instruction.span.start);
            let Some(before) = source.get(..start) else {
                return Err(error("invalid stack allocation source span"));
            };
            let line_start = before.rfind('\n').map_or(0, |index| index + 1);
            let indentation = &source[line_start..start];
            if !indentation.chars().all(char::is_whitespace) {
                return Err(error("stack allocation has unexpected indentation"));
            }
            let Some(after) = source.get(start..) else {
                return Err(error("invalid stack allocation source span"));
            };
            let Some(line_end) = after.find('\n') else {
                continue;
            };
            let insertion_offset = start + line_end + 1;
            edits.push((
                insertion_offset,
                insertion_offset,
                format!(
                    "{indentation}%{name} = {zero_expression}\n{indentation}store [init] %{name} to %{slot}\n"
                ),
            ));
            if let Some(write) = initializer {
                let start = item.span.start.saturating_add(write.span.start);
                let line = source
                    .get(start..)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                let Some(mode) = line.find("[init]") else {
                    return Err(error("initializer store mode is missing"));
                };
                edits.push((
                    start + mode,
                    start + mode + "[init]".len(),
                    "[assign]".to_owned(),
                ));
            }
        }
    }

    let mut transformed = source.to_owned();
    edits.sort_by(|left, right| right.0.cmp(&left.0));
    for (start, end, replacement) in edits {
        transformed.replace_range(start..end, &replacement);
    }
    Ok(transformed)
}

pub(super) fn validate(body: &FunctionBody) -> Result<(), CanonicalizeError> {
    let [block] = body.blocks.as_slice() else {
        return Err(error(
            "scalar memory canonicalization requires one basic block",
        ));
    };
    let mut allocated = HashSet::new();
    let mut active: Vec<String> = Vec::new();
    let mut accesses: Vec<(String, String)> = Vec::new();

    for instruction in &block.instructions {
        let references = value_references(instruction);
        let address_uses = references
            .iter()
            .filter(|name| allocated.contains(*name))
            .collect::<Vec<_>>();

        if !address_uses.is_empty() && !handles(&instruction.opcode) {
            return Err(error(format!(
                "stack address escapes through unsupported `{}`",
                instruction.opcode
            )));
        }
        for name in &address_uses {
            if !active.contains(name) {
                return Err(error(format!(
                    "stack address `%{name}` is used after deallocation"
                )));
            }
        }

        match instruction.opcode.as_str() {
            "stack.alloc" => {
                let [name] = instruction.results.as_slice() else {
                    return Err(error("stack.alloc must define one address"));
                };
                if !allocated.insert(name.clone()) {
                    return Err(error("stack.alloc redefines an address"));
                }
                active.push(name.clone());
            }
            "stack.dealloc" => {
                let Some(name) = references.first() else {
                    return Err(error("stack.dealloc requires an address"));
                };
                if references.len() != 1 || active.last() != Some(name) {
                    return Err(error(format!(
                        "stack.dealloc `%{name}` does not match the most recent allocation"
                    )));
                }
                if accesses.iter().any(|(_, address)| address == name) {
                    return Err(error(
                        "stack.dealloc cannot run during an active static access",
                    ));
                }
                active.pop();
            }
            "access.begin" => {
                let [token] = instruction.results.as_slice() else {
                    return Err(error("access.begin must produce exactly one token"));
                };
                let [address] = references.as_slice() else {
                    return Err(error("access.begin requires one address"));
                };
                if !active.contains(address) {
                    return Err(error(
                        "local scalar canonicalization requires a live stack address for access.begin",
                    ));
                }
                if accesses.iter().any(|(name, _)| name == token) {
                    return Err(error("access token is redefined while active"));
                }
                accesses.push((token.clone(), address.clone()));
            }
            "access.end" => {
                let [token] = references.as_slice() else {
                    return Err(error("access.end requires one scope token"));
                };
                let Some((current, _)) = accesses.pop() else {
                    return Err(error("access.end has no active access"));
                };
                if &current != token {
                    return Err(error(
                        "access.end must close the innermost local static access",
                    ));
                }
            }
            _ => {}
        }
    }
    if !accesses.is_empty() {
        return Err(error("static access scope remains open at function exit"));
    }
    if !active.is_empty() {
        return Err(error(
            "scalar stack allocations must be deallocated on exit",
        ));
    }
    Ok(())
}

fn value_references(instruction: &TextInstruction) -> Vec<String> {
    let mut references = instruction
        .tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(result) = instruction.results.first()
        && let Some(position) = references.iter().position(|name| name == result)
    {
        references.remove(position);
    }
    references
}

#[cfg(test)]
mod tests {
    use super::super::canonicalize_text;

    fn module(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.memory(%v: i32) -> i32 engine(native) {{\nbb0(%v: i32):\n{body}\n}}\n"
        )
    }

    #[test]
    fn explicit_scalar_stack_lifetime_is_canonical() {
        let source = module(
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %loaded = load [take] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&source).expect("canonical scalar memory");
        assert!(canonical.contains("stack.alloc i32"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable canonical text")
        );
    }

    #[test]
    fn initialized_float_stack_memory_is_canonical() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.float(%v: f32) -> f32 engine(native) {\nbb0(%v: f32):\n  %slot = stack.alloc f32\n  store [init] %v to %slot\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded\n}\n";
        let canonical = canonicalize_text(source).expect("explicitly initialized float stack");
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable float memory")
        );
    }

    #[test]
    fn uninitialized_float_slot_uses_const_zero() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.float() -> f32 engine(native) {\nbb0:\n  %slot = stack.alloc f32\n  %value = load [trivial] %slot\n  stack.dealloc %slot\n  return %value\n}\n";
        let canonical = canonicalize_text(source).expect("uninitialized f32 is zero initialized");
        assert!(canonical.contains("const.zero f32"));
        assert_eq!(canonical.matches("store [init]").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("float initialization remains canonical")
        );
    }

    #[test]
    fn float_read_then_store_init_becomes_assign_after_zeroing() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.float(%v: f64) -> f64 engine(native) {\nbb0(%v: f64):\n  %slot = stack.alloc f64\n  %before = load [trivial] %slot\n  store [init] %v to %slot\n  stack.dealloc %slot\n  return %before\n}\n";
        let canonical = canonicalize_text(source).expect("f64 zero and reassignment");
        assert!(canonical.contains("const.zero f64"));
        assert_eq!(canonical.matches("store [assign]").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable f64 canonicalization")
        );
    }

    #[test]
    fn wide_integer_stack_slot_is_zero_initialized() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.wide() -> i257 engine(native) {\nbb0:\n  %slot = stack.alloc i257\n  %value = load [trivial] %slot\n  stack.dealloc %slot\n  return %value\n}\n";
        let canonical = canonicalize_text(source).expect("wide integer zero initialization");
        assert!(canonical.contains("const.zero i257"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("wide canonical text stays stable")
        );
    }

    #[test]
    fn vector_stack_zero_initialization_is_idempotent() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.vector() -> vec<4 x i32> engine(native) {\nbb0:\n  %slot = stack.alloc vec<4 x i32>\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded\n}\n";
        let canonical = canonicalize_text(source).expect("trivial vector zero initialization");
        assert!(canonical.contains("const.zero vec<4 x i32>"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("canonical vector is idempotent")
        );
    }

    #[test]
    fn explicit_address_copy_and_move_are_canonical() {
        for operation in ["copy.addr", "move.addr"] {
            let source = module(&format!(
                "  %from = stack.alloc i32\n  %to = stack.alloc i32\n  store [init] %v to %from\n  {operation} %from to [init] %to\n  %loaded = load [take] %to\n  stack.dealloc %to\n  stack.dealloc %from\n  return %loaded"
            ));
            let canonical = canonicalize_text(&source).expect("explicit address transfer");
            assert_eq!(
                canonical,
                canonicalize_text(&canonical).expect("stable canonical text")
            );
        }
    }

    #[test]
    fn balanced_static_accesses_canonicalize() {
        let source = module(
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %read = access.begin [read] [static] %slot\n  %modify = access.begin [read] [static] %slot\n  access.end %modify\n  access.end %read\n  %loaded = load [take] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&source).expect("nested nonconflicting static accesses");
        assert!(canonical.contains("access.begin [read] [static]"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable access scopes")
        );
    }

    #[test]
    fn local_access_scopes_must_be_closed_before_deallocation() {
        for body in [
            "  %slot = stack.alloc i32\n  %scope = access.begin [read] [static] %slot\n  stack.dealloc %slot\n  access.end %scope\n  return %v",
            "  %slot = stack.alloc i32\n  %scope = access.begin [read] [static] %slot\n  stack.dealloc %slot\n  return %v",
            "  %slot = stack.alloc i32\n  %first = access.begin [read] [static] %slot\n  %second = access.begin [read] [static] %slot\n  access.end %first\n  access.end %second\n  stack.dealloc %slot\n  return %v",
            "  %slot = stack.alloc i32\n  %first = access.begin [modify] [static] %slot\n  %second = access.begin [read] [static] %slot\n  access.end %second\n  access.end %first\n  stack.dealloc %slot\n  return %v",
        ] {
            assert!(
                canonicalize_text(&module(body)).is_err(),
                "invalid access scope was accepted: {body}"
            );
        }
    }

    #[test]
    fn readonly_scalar_stack_slot_is_zero_initialized_once() {
        let source = module(
            "  %slot = stack.alloc i32\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
        );
        let canonical = canonicalize_text(&source).expect("readonly stack slot is zeroed");
        assert!(canonical.contains("const.int i32, 0"));
        assert!(canonical.contains("store [init] %v2 to %v1"));
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("zero initialization is idempotent")
        );
    }

    #[test]
    fn readonly_copy_source_is_zero_initialized() {
        let source = module(
            "  %from = stack.alloc i32\n  %to = stack.alloc i32\n  copy.addr %from to [init] %to\n  %loaded = load [take] %to\n  stack.dealloc %to\n  stack.dealloc %from\n  return %loaded",
        );
        let canonical = canonicalize_text(&source).expect("copy source is zeroed");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("zero initialization is stable")
        );
    }

    #[test]
    fn two_readonly_slots_are_zero_initialized_independently() {
        let source = module(
            "  %first = stack.alloc i32\n  %second = stack.alloc i32\n  %left = load [trivial] %first\n  %right = load [trivial] %second\n  stack.dealloc %second\n  stack.dealloc %first\n  %sum = add %left, %right\n  return %sum",
        );
        let canonical = canonicalize_text(&source).expect("both slots are zeroed");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 2);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("both zero initializers are stable")
        );
    }

    #[test]
    fn read_before_later_init_uses_zero_then_assignment() {
        let source = module(
            "  %slot = stack.alloc i32\n  %before = load [trivial] %slot\n  store [init] %v to %slot\n  %after = load [trivial] %slot\n  stack.dealloc %slot\n  %result = add %before, %after\n  return %result",
        );
        let canonical = canonicalize_text(&source).expect("zeroing and assignment work together");
        assert_eq!(canonical.matches("store [init]").count(), 1);
        assert_eq!(canonical.matches("store [assign]").count(), 1);
        assert_eq!(canonical.matches("const.int i32, 0").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("idempotent rewrite")
        );
    }

    #[test]
    fn uninitialized_take_is_zeroed_before_consumption() {
        let source = module(
            "  %slot = stack.alloc i32\n  %taken = load [take] %slot\n  stack.dealloc %slot\n  return %taken",
        );
        let canonical = canonicalize_text(&source).expect("take source is initialized");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable canonical text")
        );
    }

    #[test]
    fn uninitialized_move_address_source_is_zeroed() {
        let source = module(
            "  %from = stack.alloc i32\n  %to = stack.alloc i32\n  move.addr %from to [init] %to\n  %loaded = load [take] %to\n  stack.dealloc %to\n  stack.dealloc %from\n  return %loaded",
        );
        let canonical = canonicalize_text(&source).expect("move source is initialized");
        assert_eq!(canonical.matches("const.int i32, 0").count(), 1);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("stable canonical text")
        );
    }

    #[test]
    fn unsafe_stack_lifetimes_are_not_canonicalized() {
        for body in [
            "  %slot = stack.alloc i32\n  %taken = load [take] %slot\n  %loaded = load [trivial] %slot\n  stack.dealloc %slot\n  return %loaded",
            "  %slot = stack.alloc i32\n  %before = load [trivial] %slot\n  store [init] %v to %slot\n  store [init] %v to %slot\n  stack.dealloc %slot\n  return %before",
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %loaded = load [take] %slot\n  %again = load [trivial] %slot\n  return %again",
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  stack.dealloc %slot\n  %loaded = load [trivial] %slot\n  return %loaded",
            "  %a = stack.alloc i32\n  %b = stack.alloc i32\n  store [init] %v to %a\n  stack.dealloc %a\n  stack.dealloc %b\n  return %v",
            "  %slot = stack.alloc i32\n  %alias = copy %slot\n  stack.dealloc %slot\n  return %v",
        ] {
            let result = canonicalize_text(&module(body));
            assert!(result.is_err(), "unsafe stack flow was accepted: {body}");
        }
    }
}
