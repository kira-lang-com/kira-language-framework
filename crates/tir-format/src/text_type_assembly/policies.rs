use super::declarations::ParsedDeclaration;
use super::syntax::{
    atom_at, find_punctuation, item_error, matching, path_end, span_at, split_ranges, token_text,
    token_text_for_path, trim_range,
};
use crate::TextParseError;
use crate::types::{BinaryCopyPolicy, BinaryDestroyPolicy, BinaryPolicy, BinaryRelocatePolicy};
use std::collections::HashMap;

pub(super) fn parse_policy(
    parsed: &ParsedDeclaration,
    functions: &HashMap<String, u64>,
) -> Result<BinaryPolicy, TextParseError> {
    let tokens = &parsed.tokens;
    if atom_at(tokens, parsed.policy_start + 1) == Some("default") {
        return match atom_at(tokens, parsed.policy_start + 2) {
            Some("unmanaged") => Ok(default_policy()),
            Some("counted" | "affine") => Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start + 2),
                "default counted and affine policies require synthesized lifecycle functions",
            )),
            _ => Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start + 2),
                "unknown default memory model",
            )),
        };
    }
    let open = find_punctuation(tokens, parsed.policy_start + 1, '{').ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(tokens, parsed.policy_start),
            "policy record has no '{'",
        )
    })?;
    let close = matching(tokens, open, '{', '}').ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(tokens, open),
            "unterminated policy record",
        )
    })?;
    let mut values = HashMap::new();
    for (start, end) in split_ranges(tokens, open + 1, close) {
        let (start, end) = trim_range(tokens, start, end);
        if start == end {
            continue;
        }
        let colon = find_punctuation(tokens, start, ':')
            .filter(|index| *index < end)
            .ok_or_else(|| {
                item_error(
                    parsed.item,
                    span_at(tokens, start),
                    "policy field is missing ':'",
                )
            })?;
        let key = atom_at(tokens, start).ok_or_else(|| {
            item_error(parsed.item, span_at(tokens, start), "invalid policy field")
        })?;
        if !matches!(
            key,
            "copy" | "destroy" | "relocate" | "exclusivity" | "pairing" | "lifetime"
        ) {
            return Err(item_error(
                parsed.item,
                span_at(tokens, start),
                "unknown policy field",
            ));
        }
        if values.insert(key.to_owned(), (colon + 1, end)).is_some() {
            return Err(item_error(
                parsed.item,
                span_at(tokens, start),
                "duplicate policy field",
            ));
        }
    }
    let copy = match policy_atom(parsed, &values, "copy")? {
        "trivial" => BinaryCopyPolicy::Trivial,
        "none" => BinaryCopyPolicy::None,
        "implicit" | "explicit" => {
            let index = policy_function(parsed, &values, "copy", functions)?;
            if policy_atom(parsed, &values, "copy")? == "implicit" {
                BinaryCopyPolicy::Implicit(index)
            } else {
                BinaryCopyPolicy::Explicit(index)
            }
        }
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid copy policy",
            ));
        }
    };
    let destroy = match policy_atom(parsed, &values, "destroy")? {
        "trivial" => BinaryDestroyPolicy::Trivial,
        "fn" => {
            BinaryDestroyPolicy::Function(policy_function(parsed, &values, "destroy", functions)?)
        }
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid destroy policy",
            ));
        }
    };
    let relocate = match policy_atom(parsed, &values, "relocate")? {
        "bitwise" => BinaryRelocatePolicy::Bitwise,
        "fn" => {
            BinaryRelocatePolicy::Function(policy_function(parsed, &values, "relocate", functions)?)
        }
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid relocate policy",
            ));
        }
    };
    let exclusivity = match policy_atom(parsed, &values, "exclusivity")? {
        "static" => 0,
        "dynamic" => 1,
        "none" => 2,
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid exclusivity policy",
            ));
        }
    };
    let pairing = match policy_atom(parsed, &values, "pairing")? {
        "counted" => 0,
        "none" => 1,
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid pairing policy",
            ));
        }
    };
    let lifetime = match policy_atom(parsed, &values, "lifetime")? {
        "eager" => 0,
        "scoped" => 1,
        _ => {
            return Err(item_error(
                parsed.item,
                span_at(tokens, parsed.policy_start),
                "invalid lifetime policy",
            ));
        }
    };
    Ok(BinaryPolicy {
        copy,
        destroy,
        relocate,
        exclusivity,
        pairing,
        lifetime,
    })
}

fn policy_atom<'a>(
    parsed: &'a ParsedDeclaration,
    values: &HashMap<String, (usize, usize)>,
    key: &str,
) -> Result<&'a str, TextParseError> {
    let (start, _) = values.get(key).copied().ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(&parsed.tokens, parsed.policy_start),
            "policy record is missing a required field",
        )
    })?;
    atom_at(&parsed.tokens, start).ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(&parsed.tokens, start),
            "invalid policy value",
        )
    })
}

fn policy_function(
    parsed: &ParsedDeclaration,
    values: &HashMap<String, (usize, usize)>,
    key: &str,
    functions: &HashMap<String, u64>,
) -> Result<u64, TextParseError> {
    let (start, end) = values.get(key).copied().ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(&parsed.tokens, parsed.policy_start),
            "policy record is missing a required field",
        )
    })?;
    let at = find_punctuation(&parsed.tokens, start, '@')
        .filter(|index| *index < end)
        .ok_or_else(|| {
            item_error(
                parsed.item,
                span_at(&parsed.tokens, start),
                "policy function requires '@path'",
            )
        })?;
    let path_start = at + 1;
    let path_end = path_end(&parsed.tokens, path_start);
    let path = token_text("", &parsed.tokens, path_start, path_end)
        .unwrap_or_else(|| token_text_for_path(&parsed.tokens, path_start, path_end));
    functions.get(&path).copied().ok_or_else(|| {
        item_error(
            parsed.item,
            span_at(&parsed.tokens, at),
            "policy function is not declared in this module",
        )
    })
}

fn default_policy() -> BinaryPolicy {
    BinaryPolicy {
        copy: BinaryCopyPolicy::Trivial,
        destroy: BinaryDestroyPolicy::Trivial,
        relocate: BinaryRelocatePolicy::Bitwise,
        exclusivity: 0,
        pairing: 1,
        lifetime: 0,
    }
}
