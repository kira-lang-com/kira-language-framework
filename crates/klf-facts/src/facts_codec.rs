//! A text form of [`Facts`] (in the format of [`crate::codec`]), which an
//! analysis worker writes and the language server reads back.

use crate::codec::{Records, boolean, flag, optional, present, unescape};
use crate::store::{
    CallEdge, Entity, Facts, FileFacts, Hint, Link, LinkTarget, Reference, Runnable, SpanText,
    Symbol, Token,
};
use crate::{FactPhase, FoldKind, HintKind, ReferenceRole, RunnableKind, SymbolKind, TokenClass};
use klf_core::FileId;

impl Facts {
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = Records::default();
        out.line(&["sequence", &self.sequence.to_string(), self.phase.name()]);
        for (file, facts) in &self.files {
            encode_file(&mut out, *file, facts);
        }
        for key in &self.order {
            if let Some(symbol) = self.symbols.get(key) {
                encode_symbol(&mut out, *key, symbol);
            }
        }
        for edge in &self.calls {
            out.line(&[
                "call",
                &edge.caller.0.to_string(),
                &edge.callee.0.to_string(),
                &edge.file.as_u32().to_string(),
                &edge.start.to_string(),
                &edge.end.to_string(),
            ]);
        }
        for (subtype, supertype) in &self.supertypes {
            out.line(&["super", &subtype.0.to_string(), &supertype.0.to_string()]);
        }
        out.text
    }

    /// Reads back what [`Self::encode`] wrote.
    ///
    /// # Errors
    /// Returns the first malformed line.
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut facts = Self::new();
        let mut file = None;
        for line in text.lines() {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            decode_line(&mut facts, &mut file, &fields)
                .ok_or_else(|| format!("malformed fact line `{line}`"))?;
        }
        Ok(facts)
    }
}

fn encode_file(out: &mut Records, file: FileId, facts: &FileFacts) {
    out.line(&["file", &file.as_u32().to_string()]);
    for token in &facts.tokens {
        out.line(&[
            "t",
            &token.start.to_string(),
            &token.end.to_string(),
            token.class.name(),
            &token.modifiers.to_string(),
            token.phase.name(),
            &token.sequence.to_string(),
        ]);
    }
    for reference in &facts.references {
        out.line(&[
            "r",
            &reference.start.to_string(),
            &reference.end.to_string(),
            &reference.entity.0.to_string(),
            reference.role.name(),
        ]);
    }
    for (key, texts) in [("ty", &facts.types), ("v", &facts.values)] {
        for text in texts {
            out.line(&[
                key,
                &text.start.to_string(),
                &text.end.to_string(),
                &entity(text.entity),
                &text.text,
            ]);
        }
    }
    for hint in &facts.hints {
        out.line(&[
            "h",
            &hint.at.to_string(),
            hint.kind.name(),
            flag(hint.padding_left),
            flag(hint.padding_right),
            &entity(hint.target),
            &hint.label,
            &optional(hint.insert.as_deref()),
        ]);
    }
    for (start, end, kind) in &facts.folds {
        out.line(&["f", &start.to_string(), &end.to_string(), kind.name()]);
    }
    for (key, spans) in [
        ("n", &facts.nodes),
        ("i", &facts.inactive),
        ("u", &facts.unresolved),
    ] {
        for (start, end) in spans {
            out.line(&[key, &start.to_string(), &end.to_string()]);
        }
    }
    for link in &facts.links {
        let (kind, value) = match &link.target {
            LinkTarget::File(path) => ("file", path.clone()),
            LinkTarget::Url(url) => ("url", url.clone()),
            LinkTarget::Entity(target) => ("entity", target.0.to_string()),
        };
        out.line(&[
            "l",
            &link.start.to_string(),
            &link.end.to_string(),
            kind,
            &value,
        ]);
    }
    for runnable in &facts.runnables {
        let start = runnable.start.to_string();
        let end = runnable.end.to_string();
        let mut fields = vec!["x", &start, &end, runnable.kind.name(), &runnable.label];
        fields.extend(runnable.arguments.iter().map(String::as_str));
        out.line(&fields);
    }
    for (target, start, end, selection_start, selection_end) in &facts.declarations {
        out.line(&[
            "d",
            &target.0.to_string(),
            &start.to_string(),
            &end.to_string(),
            &selection_start.to_string(),
            &selection_end.to_string(),
        ]);
    }
    for (start, end, scope) in &facts.scopes {
        out.line(&[
            "s",
            &start.to_string(),
            &end.to_string(),
            &scope.to_string(),
        ]);
    }
}

fn encode_symbol(out: &mut Records, key: Entity, symbol: &Symbol) {
    let bits =
        u8::from(symbol.deprecated) | u8::from(symbol.exported) << 1 | u8::from(symbol.local) << 2;
    let numbers = [
        key.0.to_string(),
        symbol
            .file
            .map_or_else(String::new, |file| file.as_u32().to_string()),
        symbol.start.to_string(),
        symbol.end.to_string(),
        symbol.selection_start.to_string(),
        symbol.selection_end.to_string(),
        symbol.kind.map_or("", SymbolKind::name).to_owned(),
        entity(symbol.container),
        bits.to_string(),
    ];
    let mut fields = vec!["sym"];
    fields.extend(numbers.iter().map(String::as_str));
    fields.extend([
        symbol.name.as_str(),
        &symbol.signature,
        &symbol.documentation,
        &symbol.detail,
    ]);
    fields.extend(symbol.parameters.iter().map(String::as_str));
    out.line(&fields);
}

fn entity(value: Option<Entity>) -> String {
    value.map_or_else(String::new, |entity| entity.0.to_string())
}

#[allow(clippy::option_option)]
fn optional_entity(field: &str) -> Option<Option<Entity>> {
    if field.is_empty() {
        return Some(None);
    }
    field.parse().ok().map(|key| Some(Entity(key)))
}

fn number<T: std::str::FromStr>(field: &str) -> Option<T> {
    field.parse().ok()
}

fn decode_line(facts: &mut Facts, file: &mut Option<FileId>, fields: &[String]) -> Option<()> {
    let (key, rest) = fields.split_first()?;
    match key.as_str() {
        "sequence" => {
            let [sequence, phase] = rest else { return None };
            facts.sequence = number(sequence)?;
            facts.phase = FactPhase::from_name(phase)?;
        }
        "file" => *file = Some(FileId::from_u32(number(rest.first()?)?)),
        "sym" => decode_symbol(facts, rest)?,
        "call" => {
            let [from, to, at, start, end] = rest else {
                return None;
            };
            facts.calls.push(CallEdge {
                caller: Entity(number(from)?),
                callee: Entity(number(to)?),
                file: FileId::from_u32(number(at)?),
                start: number(start)?,
                end: number(end)?,
            });
        }
        "super" => {
            let [subtype, supertype] = rest else {
                return None;
            };
            facts
                .supertypes
                .push((Entity(number(subtype)?), Entity(number(supertype)?)));
        }
        _ => decode_file_line(facts, (*file)?, key, rest)?,
    }
    Some(())
}

fn decode_file_line(facts: &mut Facts, file: FileId, key: &str, rest: &[String]) -> Option<()> {
    let target = facts.files.entry(file).or_default();
    match (key, rest) {
        ("t", [start, end, class, modifiers, phase, sequence]) => target.tokens.push(Token {
            start: number(start)?,
            end: number(end)?,
            class: TokenClass::from_name(class)?,
            modifiers: number(modifiers)?,
            phase: FactPhase::from_name(phase)?,
            sequence: number(sequence)?,
        }),
        ("r", [start, end, owner, role]) => target.references.push(Reference {
            start: number(start)?,
            end: number(end)?,
            entity: Entity(number(owner)?),
            role: ReferenceRole::from_name(role)?,
        }),
        ("ty" | "v", [start, end, owner, text]) => {
            let text = SpanText {
                start: number(start)?,
                end: number(end)?,
                text: text.clone(),
                entity: optional_entity(owner)?,
            };
            if key == "ty" {
                target.types.push(text);
            } else {
                target.values.push(text);
            }
        }
        ("h", [at, kind, left, right, owner, label, insert]) => target.hints.push(Hint {
            at: number(at)?,
            label: label.clone(),
            kind: HintKind::from_name(kind)?,
            padding_left: boolean(left)?,
            padding_right: boolean(right)?,
            target: optional_entity(owner)?,
            insert: present(insert)?,
        }),
        ("f", [start, end, kind]) => {
            target
                .folds
                .push((number(start)?, number(end)?, FoldKind::from_name(kind)?));
        }
        ("n", [start, end]) => target.nodes.push((number(start)?, number(end)?)),
        ("i", [start, end]) => target.inactive.push((number(start)?, number(end)?)),
        ("u", [start, end]) => target.unresolved.push((number(start)?, number(end)?)),
        ("l", [start, end, kind, value]) => target.links.push(Link {
            start: number(start)?,
            end: number(end)?,
            target: match kind.as_str() {
                "file" => LinkTarget::File(value.clone()),
                "url" => LinkTarget::Url(value.clone()),
                "entity" => LinkTarget::Entity(Entity(number(value)?)),
                _ => return None,
            },
        }),
        ("x", [start, end, kind, label, arguments @ ..]) => target.runnables.push(Runnable {
            label: label.clone(),
            kind: RunnableKind::from_name(kind)?,
            start: number(start)?,
            end: number(end)?,
            arguments: arguments.to_vec(),
        }),
        ("d", [owner, start, end, selection_start, selection_end]) => target.declarations.push((
            Entity(number(owner)?),
            number(start)?,
            number(end)?,
            number(selection_start)?,
            number(selection_end)?,
        )),
        ("s", [start, end, scope]) => {
            target
                .scopes
                .push((number(start)?, number(end)?, number(scope)?));
        }
        _ => return None,
    }
    Some(())
}

fn decode_symbol(facts: &mut Facts, rest: &[String]) -> Option<()> {
    let [
        key,
        file,
        start,
        end,
        selection_start,
        selection_end,
        kind,
        container,
        bits,
        name,
        signature,
        documentation,
        detail,
        parameters @ ..,
    ] = rest
    else {
        return None;
    };
    let bits: u8 = number(bits)?;
    let symbol = Symbol {
        name: name.clone(),
        kind: if kind.is_empty() {
            None
        } else {
            Some(SymbolKind::from_name(kind)?)
        },
        file: if file.is_empty() {
            None
        } else {
            Some(FileId::from_u32(number(file)?))
        },
        start: number(start)?,
        end: number(end)?,
        selection_start: number(selection_start)?,
        selection_end: number(selection_end)?,
        container: optional_entity(container)?,
        signature: signature.clone(),
        documentation: documentation.clone(),
        detail: detail.clone(),
        parameters: parameters.to_vec(),
        deprecated: bits & 1 != 0,
        exported: bits & 2 != 0,
        local: bits & 4 != 0,
    };
    facts.declare_symbol(Entity(number(key)?), symbol);
    Some(())
}
