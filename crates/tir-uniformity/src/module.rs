use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextItem, TextItemKind, TextModule, parse_function_body};

use super::{FunctionAnalysis, Uniformity, analyze_gpu_function};

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

struct GpuFunction {
    item: TextItem,
    symbol: String,
    analysis: FunctionAnalysis,
    calls: Vec<CallSite>,
}

struct CallSite {
    block: usize,
    span: Span,
    target: Option<String>,
    indirect: bool,
}

pub fn verify_gpu_module(module: &TextModule) -> Vec<Diagnostic> {
    let mut functions = Vec::new();
    let mut diagnostics = Vec::new();
    for item in &module.items {
        if !matches!(item.kind, TextItemKind::Function | TextItemKind::Entry) {
            continue;
        }
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        if !is_gpu_callable(item.kind, &tokens) {
            continue;
        }
        let Some(symbol) = symbol_at(source, &tokens, 0) else {
            continue;
        };
        let Ok(Some(body)) = parse_function_body(source) else {
            continue;
        };
        let analysis = analyze_gpu_function(source, &body, item.kind == TextItemKind::Entry);
        diagnostics.extend(analysis.diagnostics.iter().cloned().map(|mut diagnostic| {
            shift(&mut diagnostic, item.span.start);
            diagnostic
        }));
        functions.push(GpuFunction {
            item: *item,
            symbol,
            calls: collect_calls(source, &body),
            analysis,
        });
    }

    let symbols = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.symbol.as_str(), index))
        .collect::<HashMap<_, _>>();
    let summaries = call_summaries(&functions, &symbols);
    diagnostics.extend(verify_call_sites(&functions, &symbols, &summaries));
    diagnostics
}

fn is_gpu_callable(kind: TextItemKind, tokens: &[Token]) -> bool {
    if kind == TextItemKind::Entry {
        return true;
    }
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let mut in_target = false;
    for token in &tokens[..header_end] {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => in_target = true,
            TokenKind::Punctuation(')') => in_target = false,
            TokenKind::Atom(name) if in_target && name == "gpu" => return true,
            _ => {}
        }
    }
    false
}

fn symbol_at(source: &str, tokens: &[Token], start: usize) -> Option<String> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| {
            if !punctuation(token, '@') {
                return None;
            }
            let first = tokens.get(index + 1)?;
            if atom(first).is_none() {
                return None;
            }
            let mut end = index + 2;
            while punctuation_at(tokens, end, '.') && atom_at(tokens, end + 1).is_some() {
                end += 2;
            }
            source
                .get(first.span.start..tokens.get(end - 1)?.span.end)
                .map(str::to_owned)
        })
}

fn collect_calls(source: &str, body: &FunctionBody) -> Vec<CallSite> {
    body.blocks
        .iter()
        .enumerate()
        .flat_map(|(block, basic_block)| {
            basic_block
                .instructions
                .iter()
                .filter_map(move |instruction| {
                    let opcode = instruction.opcode.as_str();
                    let indirect = opcode == "gpu.call.vft";
                    if !indirect && !matches!(opcode, "call" | "tail.call") {
                        return None;
                    }
                    Some(CallSite {
                        block,
                        span: instruction.span,
                        target: (!indirect)
                            .then(|| symbol_at(source, &instruction.tokens, 0))
                            .flatten(),
                        indirect,
                    })
                })
        })
        .collect()
}

fn call_summaries(
    functions: &[GpuFunction],
    symbols: &HashMap<&str, usize>,
) -> Vec<Option<Uniformity>> {
    let mut summaries = functions
        .iter()
        .map(|function| function.analysis.required_on_entry)
        .collect::<Vec<_>>();
    loop {
        let mut changed = false;
        let previous = summaries.clone();
        for (index, function) in functions.iter().enumerate() {
            for call in &function.calls {
                let requirement = call_requirement(call, symbols, &previous);
                let Some(requirement) = requirement else {
                    continue;
                };
                let raised =
                    previous[index].map_or(requirement, |current| current.min(requirement));
                if summaries[index] != Some(raised) {
                    summaries[index] = Some(raised);
                    changed = true;
                }
            }
        }
        if !changed {
            return summaries;
        }
    }
}

fn call_requirement(
    call: &CallSite,
    symbols: &HashMap<&str, usize>,
    summaries: &[Option<Uniformity>],
) -> Option<Uniformity> {
    if call.indirect {
        summaries.iter().flatten().copied().min()
    } else {
        call.target
            .as_deref()
            .and_then(|symbol| symbols.get(symbol))
            .and_then(|index| summaries[*index])
    }
}

fn verify_call_sites(
    functions: &[GpuFunction],
    symbols: &HashMap<&str, usize>,
    summaries: &[Option<Uniformity>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for function in functions {
        for call in &function.calls {
            let Some(required) = call_requirement(call, symbols, summaries) else {
                continue;
            };
            if function.analysis.control[call.block].satisfies(required) {
                continue;
            }
            let target = call.target.as_deref().unwrap_or("visible function table");
            let mut diagnostic = Diagnostic::error(
                V15,
                format!(
                    "call to GPU function {target} requires uniform control flow at scope {required:?}"
                ),
            )
            .with_primary_span(call.span);
            shift(&mut diagnostic, function.item.span.start);
            diagnostics.push(diagnostic);
        }
    }
    diagnostics
}

fn punctuation_at(tokens: &[Token], index: usize, character: char) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| punctuation(token, character))
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

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn shift(diagnostic: &mut Diagnostic, offset: usize) {
    if let Some(span) = &mut diagnostic.primary_span {
        span.start = span.start.saturating_add(offset);
        span.end = span.end.saturating_add(offset);
    }
    for label in &mut diagnostic.labels {
        label.span.start = label.span.start.saturating_add(offset);
        label.span.end = label.span.end.saturating_add(offset);
    }
}
