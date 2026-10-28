//! `klf.editor.*` (`KLF-EDITOR.md` section 6): the services a definition
//! records editor facts through.
//!
//! A span is three words: the file, or -1 for the phase's own, then the start
//! and end byte offsets. An optional entity is -1 when absent. Strings are
//! buffers; where a service takes several, they are separated by NUL bytes.
//! In `Build` mode the phase has no fact store and every recording service
//! returns 0 without reading its arguments.

use crate::{DefinitionServiceState, ServiceSink};
use klf_core::FileId;
use klf_facts::{
    AnalysisMode, CallEdge, Entity, FactPhase, Facts, FoldKind, Hint, HintKind, LinkTarget,
    ReferenceRole, Runnable, RunnableKind, Symbol, SymbolKind, TokenClass, TokenModifier,
};
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory};
use klf_services::{DefinitionPhase, Session};
use std::sync::{Arc, Mutex};

const NONE: u64 = u64::MAX;
const TOKEN_WORDS: usize = 4;

/// The fact store a phase records into, when the session serves an editor.
pub(crate) fn open_facts(session: &Session, phase: DefinitionPhase) -> Option<Facts> {
    (session.mode() == AnalysisMode::Editor).then(|| {
        let mut facts = Facts::new();
        facts.set_phase(match phase {
            DefinitionPhase::Units => FactPhase::Units,
            DefinitionPhase::Parse => FactPhase::Parse,
            DefinitionPhase::Elaborate => FactPhase::Elaborate,
            DefinitionPhase::Lower => FactPhase::Lower,
        });
        facts
    })
}

type Recorder = fn(&Context<'_>, &mut Facts, &[u64], &mut dyn ServiceMemory) -> Option<u64>;

/// What a recorder may read of the phase besides its arguments.
struct Context<'a> {
    file: Option<FileId>,
    sources: &'a [(FileId, String, Arc<str>)],
}

impl Context<'_> {
    fn file(&self, word: u64) -> Option<FileId> {
        let file = if word == NONE {
            self.file?
        } else {
            FileId::from_u32(u32::try_from(word).ok()?)
        };
        self.text(file).map(|_| file)
    }

    fn text(&self, file: FileId) -> Option<&str> {
        self.sources
            .iter()
            .find(|(known, _, _)| *known == file)
            .map(|(_, _, text)| &**text)
    }

    /// The span `[file, start, end]`, inside the file's text.
    fn span(&self, words: &[u64]) -> Option<(FileId, u32, u32)> {
        let [file, start, end] = words else {
            return None;
        };
        let file = self.file(*file)?;
        let (start, end) = (u32::try_from(*start).ok()?, u32::try_from(*end).ok()?);
        let length = self.text(file)?.len();
        (start <= end && usize::try_from(end).ok()? <= length).then_some((file, start, end))
    }
}

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state("klfsvc_editor_mode", state, |state, _| {
        u64::from(state.facts.is_some())
    });
    let recorders: [(&str, Recorder); 19] = [
        ("klfsvc_editor_classify", classify),
        ("klfsvc_editor_classify_many", classify_many),
        ("klfsvc_editor_symbol", symbol),
        ("klfsvc_editor_symbol_text", symbol_text),
        ("klfsvc_editor_symbol_flags", symbol_flags),
        ("klfsvc_editor_declaration", declaration),
        ("klfsvc_editor_reference", reference),
        ("klfsvc_editor_unresolved", unresolved),
        ("klfsvc_editor_type_at", type_at),
        ("klfsvc_editor_value_at", value_at),
        ("klfsvc_editor_hint", hint),
        ("klfsvc_editor_fold", fold),
        ("klfsvc_editor_node", node),
        ("klfsvc_editor_link", link),
        ("klfsvc_editor_runnable", runnable),
        ("klfsvc_editor_call_edge", call_edge),
        ("klfsvc_editor_type_edge", type_edge),
        ("klfsvc_editor_inactive", inactive),
        ("klfsvc_editor_scope", scope),
    ];
    for (name, recorder) in recorders {
        services.register_state_with_memory(name, state, move |state, arguments, memory| {
            let Some(mut facts) = state.facts.take() else {
                return 0;
            };
            let context = Context {
                file: state.file,
                sources: &state.sources,
            };
            let recorded = recorder(&context, &mut facts, arguments, memory);
            state.facts = Some(facts);
            recorded.unwrap_or(0)
        });
    }
}

fn entity(word: u64) -> Option<Entity> {
    (word != NONE).then_some(Entity(word))
}

fn modifiers(word: u64) -> u32 {
    let all = TokenModifier::ALL
        .iter()
        .fold(0, |bits, modifier| bits | modifier.bit());
    u32::try_from(word & u64::from(all)).unwrap_or(0)
}

fn read_bytes(memory: &mut dyn ServiceMemory, address: u64, length: u64) -> Option<Vec<u8>> {
    let length = usize::try_from(length).ok()?;
    if length > MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    (length == 0 || memory.read_buffer(address, &mut bytes)).then_some(bytes)
}

fn read_text(memory: &mut dyn ServiceMemory, address: u64, length: u64) -> Option<String> {
    String::from_utf8(read_bytes(memory, address, length)?).ok()
}

/// `[file, start, end, class, modifiers]`.
fn classify(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, class, bits] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    facts.classify(
        file,
        start,
        end,
        TokenClass::from_code(*class)?,
        modifiers(*bits),
    );
    Some(1)
}

/// `[file, tokens, count]`: `count` tokens of four words each, laid out as
/// `classify` takes them after the file. Returns how many were recorded.
fn classify_many(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, address, count] = words else {
        return None;
    };
    let file = context.file(*file)?;
    let bytes = usize::try_from(*count).ok()?.checked_mul(TOKEN_WORDS * 8)?;
    let buffer = read_bytes(memory, *address, u64::try_from(bytes).ok()?)?;
    let mut recorded = 0;
    for token in buffer.as_chunks::<{ TOKEN_WORDS * 8 }>().0 {
        let word = |index: usize| {
            let mut bytes = [0; 8];
            bytes.copy_from_slice(&token[index * 8..index * 8 + 8]);
            u64::from_le_bytes(bytes)
        };
        let Some((_, start, end)) = context.span(&[u64::from(file.as_u32()), word(0), word(1)])
        else {
            continue;
        };
        let Some(class) = TokenClass::from_code(word(2)) else {
            continue;
        };
        facts.classify(file, start, end, class, modifiers(word(3)));
        recorded += 1;
    }
    Some(recorded)
}

/// `[entity, file, start, end, selection start, selection end, kind,
/// container]`. The name is the selection's text until `symbol_text` sets
/// another.
fn symbol(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [
        entity,
        file,
        start,
        end,
        selection_start,
        selection_end,
        kind,
        container,
    ] = words
    else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let (_, selection_start, selection_end) =
        context.span(&[u64::from(file.as_u32()), *selection_start, *selection_end])?;
    let text = context.text(file)?;
    let name = text
        .get(selection_start as usize..selection_end as usize)
        .unwrap_or_default()
        .to_owned();
    let symbol = Symbol {
        name,
        kind: SymbolKind::from_code(*kind),
        file: Some(file),
        start,
        end,
        selection_start,
        selection_end,
        container: self::entity(*container),
        ..Symbol::default()
    };
    facts.declare_symbol(Entity(*entity), symbol);
    Some(1)
}

/// `[entity, field, text, length]`: field 0 is the name, 1 the signature,
/// 2 the documentation, 3 the detail, and 4 appends a parameter.
fn symbol_text(
    _: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [entity, field, address, length] = words else {
        return None;
    };
    let text = read_text(memory, *address, *length)?;
    let symbol = facts.symbol_mut(Entity(*entity))?;
    match field {
        0 => symbol.name = text,
        1 => symbol.signature = text,
        2 => symbol.documentation = text,
        3 => symbol.detail = text,
        4 => symbol.parameters.push(text),
        _ => return None,
    }
    Some(1)
}

/// `[entity, flags]`: 1 deprecated, 2 exported, 4 local.
fn symbol_flags(
    _: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [entity, flags] = words else {
        return None;
    };
    let symbol = facts.symbol_mut(Entity(*entity))?;
    symbol.deprecated = flags & 1 != 0;
    symbol.exported = flags & 2 != 0;
    symbol.local = flags & 4 != 0;
    Some(1)
}

/// `[entity, file, start, end, selection start, selection end]`.
fn declaration(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [entity, file, start, end, selection_start, selection_end] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let (_, selection_start, selection_end) =
        context.span(&[u64::from(file.as_u32()), *selection_start, *selection_end])?;
    facts.declaration(
        Entity(*entity),
        file,
        (start, end),
        (selection_start, selection_end),
    );
    Some(1)
}

/// `[file, start, end, entity, role]`.
fn reference(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, entity, role] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    facts.reference(
        file,
        start,
        end,
        Entity(*entity),
        ReferenceRole::from_code(*role)?,
    );
    Some(1)
}

/// `[file, start, end]`.
fn unresolved(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let (file, start, end) = context.span(words)?;
    facts.unresolved(file, start, end);
    Some(1)
}

/// `[file, start, end]`.
fn inactive(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let (file, start, end) = context.span(words)?;
    facts.inactive(file, start, end);
    Some(1)
}

/// `[file, start, end]`.
fn node(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let (file, start, end) = context.span(words)?;
    facts.syntax_node(file, start, end);
    Some(1)
}

/// `[file, start, end, scope]`: the lexical scope of the span.
fn scope(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, scope] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    facts.scope(file, start, end, *scope);
    Some(1)
}

/// `[file, start, end, entity, text, length]`.
fn type_at(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, entity, address, length] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let text = read_text(memory, *address, *length)?;
    facts.type_at(file, start, end, text, self::entity(*entity));
    Some(1)
}

/// `[file, start, end, text, length]`.
fn value_at(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, address, length] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let text = read_text(memory, *address, *length)?;
    facts.value_at(file, start, end, text);
    Some(1)
}

/// `[file, at, kind, flags, target, text, length]`: flags 1 pads the left,
/// 2 the right. The text is the label, then optionally a NUL and the text
/// accepting the hint inserts.
fn hint(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, at, kind, flags, target, address, length] = words else {
        return None;
    };
    let (file, at, _) = context.span(&[*file, *at, *at])?;
    let text = read_text(memory, *address, *length)?;
    let (label, insert) = match text.split_once('\0') {
        Some((label, insert)) => (label.to_owned(), Some(insert.to_owned())),
        None => (text, None),
    };
    facts.hint(
        file,
        Hint {
            at,
            label,
            kind: HintKind::from_code(*kind)?,
            padding_left: flags & 1 != 0,
            padding_right: flags & 2 != 0,
            target: entity(*target),
            insert,
        },
    );
    Some(1)
}

/// `[file, start, end, kind]`.
fn fold(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, kind] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    facts.fold(file, start, end, FoldKind::from_code(*kind)?);
    Some(1)
}

/// `[file, start, end, kind, entity, text, length]`: kind 0 links to the file
/// the text names, 1 to the URL it holds, 2 to the entity.
fn link(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, kind, entity, address, length] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let target = match kind {
        0 => LinkTarget::File(read_text(memory, *address, *length)?),
        1 => LinkTarget::Url(read_text(memory, *address, *length)?),
        2 => LinkTarget::Entity(self::entity(*entity)?),
        _ => return None,
    };
    facts.link(file, start, end, target);
    Some(1)
}

/// `[file, start, end, kind, text, length]`: the text is the label, then
/// each driver argument, NUL-separated.
fn runnable(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    memory: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [file, start, end, kind, address, length] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    let text = read_text(memory, *address, *length)?;
    let mut parts = text.split('\0').map(str::to_owned);
    let label = parts.next().unwrap_or_default();
    facts.runnable(
        file,
        Runnable {
            label,
            kind: RunnableKind::from_code(*kind)?,
            start,
            end,
            arguments: parts.collect(),
        },
    );
    Some(1)
}

/// `[caller, callee, file, start, end]`.
fn call_edge(
    context: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [from, to, file, start, end] = words else {
        return None;
    };
    let (file, start, end) = context.span(&[*file, *start, *end])?;
    facts.call_edge(CallEdge {
        caller: Entity(*from),
        callee: Entity(*to),
        file,
        start,
        end,
    });
    Some(1)
}

/// `[subtype, supertype]`.
fn type_edge(
    _: &Context<'_>,
    facts: &mut Facts,
    words: &[u64],
    _: &mut dyn ServiceMemory,
) -> Option<u64> {
    let [subtype, supertype] = words else {
        return None;
    };
    facts.type_edge(Entity(*subtype), Entity(*supertype));
    Some(1)
}
