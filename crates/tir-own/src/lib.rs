//! TIR ownership canonicalization, milestone T1.
//!
//! The pure-value sublanguage is supported first: ownership passes have no
//! work to perform on primitives, explicit trivial copies/moves/destroys and
//! SSA-only instructions. Other raw TIR is
//! deliberately refused until the full five-pass canonicalizer is implemented.
//! Never emit an apparently canonical module after skipping a required pass.
#![forbid(unsafe_code)]

mod unique_linear;

use std::error::Error;
use std::fmt;

use tir_diag::Severity;
use tir_format::{
    BinaryModule, TextItemKind, assemble_text_module, disassemble_text_module, parse_function_body,
    parse_function_signature, parse_text_module,
};
use tir_types::{Convention, Type};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CanonicalizeError(pub String);

impl fmt::Display for CanonicalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for CanonicalizeError {}

fn error(message: impl Into<String>) -> CanonicalizeError {
    CanonicalizeError(message.into())
}

/// Canonicalizes the proven-trivial subset of textual TIR. An unsupported
/// ownership pattern is a hard error, never a pass-through or silent rewrite.
/// Full T1 canonicalization remains a separate milestone.
pub fn canonicalize_text(source: &str) -> Result<String, CanonicalizeError> {
    let source = insert_unused_unique_destroys(source)?;
    let module = parse_text_module(&source).map_err(|e| error(format!("TIR text: {e}")))?;
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            return Err(error(
                "TIR ownership canonicalization does not yet handle non-function module records",
            ));
        }
        let function = module.item_text(*item);
        let signature = parse_function_signature(function)
            .map_err(|e| error(format!("function signature: {e}")))?
            .ok_or_else(|| error("function has no signature"))?;
        if signature.failure.is_some() || !is_pure_value_type(&signature.result) {
            return Err(error(
                "TIR ownership canonicalization requires a supported result and no failure type",
            ));
        }
        let unique_parameters = signature
            .parameters
            .iter()
            .filter(|p| matches!(p.ty, Type::Unique(_)))
            .count();
        if signature.parameters.iter().any(|p| {
            !is_pure_value_type(&p.ty)
                && !(matches!(p.ty, Type::Unique(_)) && p.convention == Some(Convention::Owned))
        }) || (unique_parameters != 0 && signature.result != Type::Unit)
        {
            return Err(error(
                "TIR ownership canonicalization requires scalar values or directly destroyed owned unique arguments",
            ));
        }
        let body = parse_function_body(function)
            .map_err(|e| error(format!("function body: {e}")))?
            .ok_or_else(|| {
                error("TIR ownership canonicalization requires a defined function body")
            })?;
        if unique_parameters != 0 && body.blocks.len() == 1 {
            let entry = &body.blocks[0];
            let required = entry
                .parameters
                .iter()
                .filter(|p| matches!(p.ty, Type::Unique(_)))
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>();
            if !unique_linear::remaining_owned_values(entry, &required)?.is_empty() {
                return Err(error(
                    "owned unique canonicalization did not destroy every live owned value",
                ));
            }
        } else if unique_parameters != 0 {
            let Some(entry) = body.blocks.first() else {
                return Err(error("owned unique function has no entry block"));
            };
            if entry.parameters.len() != signature.parameters.len() {
                return Err(error(
                    "owned unique block signature does not match the function signature",
                ));
            }
            let required = entry
                .parameters
                .iter()
                .filter(|p| matches!(p.ty, Type::Unique(_)))
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>();
            let mut exits = 0;
            for block in &body.blocks {
                let Some((terminator, operations)) = block.instructions.split_last() else {
                    return Err(error("owned unique control flow has an unterminated block"));
                };
                match terminator.opcode.as_str() {
                    "return" => {
                        exits += 1;
                        if terminator.tokens.len() != 1 {
                            return Err(error("owned unique function must return unit"));
                        }
                        let mut consumed = Vec::new();
                        for instruction in operations {
                            if instruction.opcode != "destroy" {
                                return Err(error(
                                    "owned unique canonicalization cannot safely resolve arbitrary uses",
                                ));
                            }
                            if let [_, _, token] = instruction.tokens.as_slice()
                                && let tir_format::text::TokenKind::Atom(value) = &token.kind
                            {
                                consumed.push(value.as_str());
                            }
                        }
                        if consumed.len() != required.len()
                            || required.iter().any(|name| {
                                consumed.iter().filter(|value| *value == name).count() != 1
                            })
                        {
                            return Err(error(
                                "each return path must destroy every owned unique argument exactly once",
                            ));
                        }
                    }
                    "br" | "cond.br" if operations.is_empty() => {}
                    _ => {
                        return Err(error(
                            "owned unique canonicalization supports only branch control flow and return-path destroys",
                        ));
                    }
                }
            }
            if exits == 0 {
                return Err(error(
                    "owned unique canonicalization requires at least one exit path",
                ));
            }
        }
        for (block_index, block) in body.blocks.iter().enumerate() {
            if block
                .parameters
                .iter()
                .any(|p| !is_pure_value_type(&p.ty) && block_index != 0)
                || block.parameters.iter().any(|p| {
                    !is_pure_value_type(&p.ty)
                        && !(unique_parameters != 0
                            && matches!(p.ty, Type::Unique(_))
                            && p.convention == Some(Convention::Owned))
                })
            {
                return Err(error(
                    "TIR ownership canonicalization cannot lower an owned or address-only block parameter",
                ));
            }
            for instruction in &block.instructions {
                if !is_pure_value_instruction(&instruction.opcode) {
                    return Err(error(format!(
                        "TIR ownership canonicalization has no safe five-pass lowering for `{}`",
                        instruction.opcode,
                    )));
                }
            }
        }
    }
    let diagnostics = tir_verify::verify_text_module(&module, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    if !diagnostics.is_empty() {
        return Err(error(format!(
            "TIR verifier rejected raw input: {diagnostics:?}"
        )));
    }
    let bytes = assemble_text_module(&module).map_err(|e| error(format!("TIR assembler: {e}")))?;
    let binary = BinaryModule::decode(&bytes).map_err(|e| error(format!("TIR binary: {e}")))?;
    let diagnostics = tir_verify::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    if !diagnostics.is_empty() {
        return Err(error(format!(
            "TIR binary verifier rejected output: {diagnostics:?}"
        )));
    }
    let canonical = disassemble_text_module(&binary, u64::MAX)
        .map_err(|e| error(format!("TIR disassembler: {e}")))?;
    let reparsed =
        parse_text_module(&canonical).map_err(|e| error(format!("TIR canonical text: {e}")))?;
    let reassembled = assemble_text_module(&reparsed)
        .map_err(|e| error(format!("TIR canonical reassembly: {e}")))?;
    if reassembled != bytes {
        return Err(error(
            "TIR canonicalization produced a non-canonical binary round trip",
        ));
    }
    Ok(canonical)
}

/// Inserts a destroy for an otherwise unused owned unique parameter in the
/// simplest provably safe case. More complicated functions are left unchanged
/// and rejected by the canonical subset validator below.
fn insert_unused_unique_destroys(source: &str) -> Result<String, CanonicalizeError> {
    let module = parse_text_module(source).map_err(|e| error(format!("TIR text: {e}")))?;
    let mut edits = Vec::new();
    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let function = module.item_text(*item);
        let signature = parse_function_signature(function)
            .map_err(|e| error(format!("function signature: {e}")))?
            .ok_or_else(|| error("function has no signature"))?;
        if signature.result != Type::Unit || signature.failure.is_some() {
            continue;
        }
        let body =
            parse_function_body(function).map_err(|e| error(format!("function body: {e}")))?;
        let Some(body) = body else {
            continue;
        };
        let Some(entry) = body.blocks.first() else {
            continue;
        };
        if signature.parameters.len() != entry.parameters.len() {
            continue;
        }
        let unique = entry
            .parameters
            .iter()
            .filter(|parameter| {
                matches!(parameter.ty, Type::Unique(_))
                    && parameter.convention == Some(Convention::Owned)
            })
            .collect::<Vec<_>>();
        if unique.is_empty() {
            continue;
        }
        // For one basic block, resolve explicit moves and destroy whichever
        // unique SSA values remain live on the unique return path.
        if let [block] = body.blocks.as_slice() {
            let required = unique.iter().map(|p| p.name.as_str()).collect::<Vec<_>>();
            if let Ok(remaining) = unique_linear::remaining_owned_values(block, &required)
                && !remaining.is_empty()
            {
                let terminator = block.instructions.last().expect("linear body has a return");
                let original_offset = item.span.start.saturating_add(terminator.span.start);
                let Some(return_line) = source.get(..original_offset) else {
                    return Err(error("malformed ownership source span"));
                };
                let line_start = return_line.rfind('\n').map_or(0, |offset| offset + 1);
                let indentation = &source[line_start..original_offset];
                if !indentation.chars().all(char::is_whitespace) {
                    return Err(error("unexpected ownership terminator indentation"));
                }
                let insertion = remaining
                    .iter()
                    .map(|value| format!("{indentation}destroy %{value}\n"))
                    .collect::<String>();
                edits.push((line_start, insertion));
            }
            continue;
        }
        // In multi-block control flow, only operate on blocks with a branch
        // or a return, and add destroys immediately before each return.
        if body.blocks.iter().any(|block| {
            block.instructions.len() != 1
                || !matches!(
                    block.instructions[0].opcode.as_str(),
                    "return" | "br" | "cond.br"
                )
                || (block.instructions[0].opcode == "return"
                    && block.instructions[0].tokens.len() != 1)
        }) {
            continue;
        }
        for block in &body.blocks {
            let terminator = &block.instructions[0];
            if terminator.opcode != "return" {
                continue;
            }
            let original_offset = item.span.start.saturating_add(terminator.span.start);
            let Some(return_line) = source.get(..original_offset) else {
                return Err(error("malformed ownership source span"));
            };
            let line_start = return_line.rfind('\n').map_or(0, |offset| offset + 1);
            let indentation = &source[line_start..original_offset];
            if !indentation.chars().all(char::is_whitespace) {
                return Err(error("unexpected ownership terminator indentation"));
            }
            let insertion = unique
                .iter()
                .map(|parameter| format!("{indentation}destroy %{}\n", parameter.name))
                .collect::<String>();
            edits.push((line_start, insertion));
        }
    }
    let mut transformed = source.to_owned();
    for (offset, insertion) in edits.into_iter().rev() {
        transformed.insert_str(offset, &insertion);
    }
    Ok(transformed)
}

fn is_pure_value_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Integer(_) | Type::Float(_) | Type::Pointer(_) | Type::Unit | Type::Never
    )
}

fn is_pure_value_instruction(op: &str) -> bool {
    matches!(
        op,
        "return"
            | "br"
            | "cond.br"
            | "const.int"
            | "const.bool"
            | "add"
            | "sub"
            | "mul"
            | "and"
            | "or"
            | "xor"
            | "not"
            | "neg"
            | "abs"
            | "icmp"
            | "select"
            | "copy"
            | "move"
            | "destroy"
            | "borrow.begin"
            | "borrow.end"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.calculate(%a: i32, %b: i32, %condition: i1) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32, %condition: i1):\n{body}\n}}\n"
        )
    }

    #[test]
    fn pure_value_canonicalization_is_idempotent() {
        let text = source(
            "  %sum = add %a, %b\n  %same = icmp eq %sum, %b\n  %result = select %same, %sum, %a\n  return %result",
        );
        let once = canonicalize_text(&text).expect("pure value module canonicalizes");
        let twice = canonicalize_text(&once).expect("canonical text remains canonical");
        assert_eq!(once, twice);
    }

    #[test]
    fn canonicalizes_multiblock_pure_control_flow() {
        let text = source(
            "  %sum = add %a, %b\n  cond.br %condition, bb1(%sum), bb2(%a)\nbb1(%left: i32):\n  br bb2(%left)\nbb2(%result: i32):\n  return %result",
        );
        let once = canonicalize_text(&text).expect("pure control flow canonicalizes");
        assert_eq!(
            once,
            canonicalize_text(&once).expect("stable canonical text")
        );
    }

    #[test]
    fn raw_ownership_operations_are_not_silently_accepted() {
        let text = source("  %borrowed = borrow.begin %a\n  return %a");
        let error = canonicalize_text(&text).expect_err("borrow pass is not yet supported");
        assert!(error.0.contains("TIR verifier rejected raw input"));
    }

    #[test]
    fn explicitly_consumed_trivial_values_are_canonical() {
        let text = source(
            "  %dup = copy %a\n  %taken = move %dup\n  destroy %a\n  %sum = add %taken, %b\n  return %sum",
        );
        let once = canonicalize_text(&text).expect("trivial explicit ownership canonicalizes");
        assert_eq!(
            once,
            canonicalize_text(&once).expect("canonicalization is stable")
        );
    }

    #[test]
    fn explicitly_balanced_trivial_borrow_is_canonical() {
        let text = source(
            "  %loan = borrow.begin %a\n  borrow.end %loan\n  %sum = add %a, %b\n  return %sum",
        );
        let once = canonicalize_text(&text).expect("balanced explicit borrow canonicalizes");
        assert!(once.contains("borrow.begin"));
        assert_eq!(
            once,
            canonicalize_text(&once).expect("canonical text is stable")
        );
    }

    #[test]
    fn unsupported_memory_operations_are_not_silently_accepted() {
        let text = source("  %slot = stack.alloc i32\n  return %a");
        let error = canonicalize_text(&text).expect_err("address-only pass is not yet supported");
        assert!(error.0.contains("no safe five-pass lowering"));
    }
}

#[cfg(test)]
mod unique_destroy_tests {
    use super::*;

    #[test]
    fn unused_owned_unique_parameter_gets_destroy_inserted() {
        let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  return\n}\n";
        let canonical = canonicalize_text(text).expect("destroy insertion succeeds");
        assert!(
            canonical.contains("destroy %arg0"),
            "destroy must be explicit: {canonical}"
        );
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("canonical text is stable")
        );
    }

    #[test]
    fn multiple_unused_owned_unique_parameters_are_destroyed() {
        let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%first: owned uniq i32, %second: owned uniq i64) -> unit engine(native) {\nbb0(%first: owned uniq i32, %second: owned uniq i64):\n  return\n}\n";
        let canonical = canonicalize_text(text).expect("two owned unique values get destroys");
        assert_eq!(canonical.matches("destroy %").count(), 2);
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("canonical text remains canonical")
        );
    }

    #[test]
    fn unsupported_unique_ownership_flow_remains_rejected() {
        let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %copy = copy %object\n  return\n}\n";
        assert!(
            canonicalize_text(text).is_err(),
            "ownership resolution is not implemented for this case"
        );
    }
}

#[cfg(test)]
mod branch_destroy_tests {
    use super::*;

    #[test]
    fn owned_unique_is_destroyed_on_each_return_path() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32, %condition: i1) -> unit engine(native) {\nbb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb2:\n  return\nbb1:\n  return\n}\n";
        let once = canonicalize_text(source).expect("owned unique is destroyed on both exits");
        assert_eq!(once.matches("destroy %arg0").count(), 2);
        assert_eq!(
            once,
            canonicalize_text(&once).expect("canonical text remains canonical")
        );
    }

    #[test]
    fn mixed_return_and_branch_paths_insert_destroy_at_exit_only() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32, %condition: i1) -> unit engine(native) {\nbb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb1:\n  br bb2()\nbb2:\n  return\n}\n";
        let once = canonicalize_text(source).expect("owned unique destroyed at merged exit");
        assert_eq!(once.matches("destroy %arg0").count(), 1);
        assert_eq!(
            once,
            canonicalize_text(&once).expect("canonical text remains canonical")
        );
    }
}

#[cfg(test)]
mod moved_unique_destroy_tests {
    use super::*;

    #[test]
    fn inserts_destroy_of_final_unique_move_result() {
        let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %first = move %object\n  %second = move %first\n  return\n}\n";
        let canonical = canonicalize_text(text).expect("moved unique receives a destroy");
        assert!(
            canonical.contains("destroy %v2"),
            "final owned value must be destroyed: {canonical}"
        );
        assert_eq!(
            canonical,
            canonicalize_text(&canonical).expect("canonical text is stable")
        );
    }

    #[test]
    fn cannot_move_owned_unique_twice() {
        let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %first = move %object\n  %second = move %object\n  return\n}\n";
        assert!(
            canonicalize_text(text).is_err(),
            "a second move from consumed storage must fail"
        );
    }
}
