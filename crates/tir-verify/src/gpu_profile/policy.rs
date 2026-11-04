use std::collections::{HashMap, HashSet};

use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextItemKind, TextModule, parse_type};
use tir_layout::LayoutEngine;
use tir_types::Type;

enum Policy {
    Explicit {
        trivial: bool,
        copyable: bool,
        bitwise_relocate: bool,
    },
    Unmanaged,
    Counted,
    Affine,
}

struct TypeDefinition {
    name: String,
    parameters: Vec<String>,
    fields: Vec<Type>,
    variants: Vec<String>,
    field_annotations: HashMap<String, HashSet<String>>,
    policy: Policy,
    is_struct: bool,
    is_enum: bool,
}

pub(crate) struct TypePolicies {
    definitions: HashMap<String, TypeDefinition>,
}

impl TypePolicies {
    pub(crate) fn from_module(module: &TextModule) -> Self {
        let mut definitions = HashMap::new();
        for item in module
            .items
            .iter()
            .filter(|item| item.kind == TextItemKind::Type)
        {
            let source = module.item_text(*item);
            let Ok(tokens) = lex(source) else {
                continue;
            };
            if let Some(definition) = parse_definition(source, &tokens) {
                definitions.insert(definition.name.clone(), definition);
            }
        }
        Self { definitions }
    }

    pub(crate) fn is_trivial(&self, ty: &Type) -> bool {
        self.is_trivial_inner(ty, &mut HashSet::new())
    }

    pub(crate) fn is_copyable(&self, ty: &Type) -> bool {
        self.is_copyable_inner(ty, &mut HashSet::new())
    }

    pub(crate) fn optional_type(&self, value: Type) -> Type {
        let mut candidates = self.definitions.values().filter(|definition| {
            definition.is_enum
                && definition.parameters.len() == 1
                && definition.name.rsplit('.').next() == Some("Optional")
        });
        let path = match (candidates.next(), candidates.next()) {
            (Some(definition), None) => definition.name.clone(),
            _ => "Optional".to_owned(),
        };
        Type::Nominal {
            path,
            arguments: vec![value],
        }
    }

    pub(crate) fn is_struct(&self, ty: &Type) -> bool {
        let Type::Nominal { path, arguments } = ty else {
            return false;
        };
        self.definition(path).is_some_and(|definition| {
            definition.is_struct && definition.parameters.len() == arguments.len()
        })
    }

    pub(crate) fn variant_index(&self, ty: &Type, name: &str) -> Option<usize> {
        let Type::Nominal { path, arguments } = ty else {
            return None;
        };
        self.definition(path).and_then(|definition| {
            (definition.is_enum && definition.parameters.len() == arguments.len())
                .then(|| {
                    definition
                        .variants
                        .iter()
                        .position(|variant| variant == name)
                })
                .flatten()
        })
    }

    pub(crate) fn is_loadable(&self, ty: &Type, layouts: &mut LayoutEngine) -> bool {
        layouts.layout(ty).is_ok() && self.is_bitwise_relocatable_inner(ty, &mut HashSet::new())
    }

    pub(crate) fn has_field_annotation(&self, ty: &Type, annotation: &str) -> bool {
        let Type::Nominal { path, arguments } = ty else {
            return false;
        };
        self.definition(path).is_some_and(|definition| {
            definition.parameters.len() == arguments.len()
                && definition
                    .field_annotations
                    .values()
                    .any(|annotations| annotations.contains(annotation))
        })
    }

    pub(crate) fn any_nested_type(&self, ty: &Type, predicate: impl Fn(&Type) -> bool) -> bool {
        self.any_nested_type_inner(ty, &predicate, &mut HashSet::new())
    }

    fn any_nested_type_inner(
        &self,
        ty: &Type,
        predicate: &impl Fn(&Type) -> bool,
        visiting: &mut HashSet<Type>,
    ) -> bool {
        if predicate(ty) {
            return true;
        }
        match ty {
            Type::FixedVector { element, .. }
            | Type::ScalableVector(element)
            | Type::Array { element, .. }
            | Type::Reference(element)
            | Type::Unique(element)
            | Type::Weak(element)
            | Type::Address {
                pointee: element, ..
            }
            | Type::Texture {
                component: element, ..
            }
            | Type::Tensor { element, .. }
            | Type::CooperativeMatrix { element, .. } => {
                self.any_nested_type_inner(element, predicate, visiting)
            }
            Type::Tuple(elements) => elements
                .iter()
                .any(|element| self.any_nested_type_inner(element, predicate, visiting)),
            Type::Nominal { path, arguments } => {
                if arguments
                    .iter()
                    .any(|argument| self.any_nested_type_inner(argument, predicate, visiting))
                {
                    return true;
                }
                if !visiting.insert(ty.clone()) {
                    return false;
                }
                let found = self.definition(path).is_some_and(|definition| {
                    if definition.parameters.len() != arguments.len() {
                        return false;
                    }
                    let bindings = definition
                        .parameters
                        .iter()
                        .cloned()
                        .zip(arguments.iter().cloned())
                        .collect::<HashMap<_, _>>();
                    definition.fields.iter().any(|field| {
                        self.any_nested_type_inner(
                            &field.substitute(&bindings),
                            predicate,
                            visiting,
                        )
                    })
                });
                visiting.remove(ty);
                found
            }
            Type::Function(signature) => {
                signature
                    .parameters
                    .iter()
                    .any(|parameter| self.any_nested_type_inner(&parameter.ty, predicate, visiting))
                    || self.any_nested_type_inner(&signature.result, predicate, visiting)
                    || signature.failure.as_ref().is_some_and(|failure| {
                        self.any_nested_type_inner(failure, predicate, visiting)
                    })
            }
            Type::VisibleFunctionTable(signature) => {
                signature
                    .parameters
                    .iter()
                    .any(|parameter| self.any_nested_type_inner(&parameter.ty, predicate, visiting))
                    || self.any_nested_type_inner(&signature.result, predicate, visiting)
                    || signature.failure.as_ref().is_some_and(|failure| {
                        self.any_nested_type_inner(failure, predicate, visiting)
                    })
            }
            Type::Closure(closure) => self.any_nested_type_inner(
                &Type::Function(closure.signature.clone()),
                predicate,
                visiting,
            ),
            Type::Coroutine {
                yield_type,
                resume_type,
                result_type,
            } => {
                self.any_nested_type_inner(yield_type, predicate, visiting)
                    || self.any_nested_type_inner(resume_type, predicate, visiting)
                    || self.any_nested_type_inner(result_type, predicate, visiting)
            }
            Type::Entry {
                root, immediate, ..
            } => {
                self.any_nested_type_inner(root, predicate, visiting)
                    || self.any_nested_type_inner(immediate, predicate, visiting)
            }
            Type::Integer(_)
            | Type::Float(_)
            | Type::Pointer(_)
            | Type::Code
            | Type::Unit
            | Type::Never
            | Type::Generic(_)
            | Type::Frame(_)
            | Type::Sampler
            | Type::AccelerationStructure
            | Type::RayQuery
            | Type::HitObject => false,
        }
    }

    fn is_trivial_inner(&self, ty: &Type, visiting: &mut HashSet<Type>) -> bool {
        match ty {
            Type::Reference(_) | Type::Unique(_) | Type::Weak(_) | Type::Closure(_) => false,
            Type::Generic(_) | Type::Frame(_) | Type::Coroutine { .. } => false,
            Type::Tuple(elements) => elements
                .iter()
                .all(|element| self.is_trivial_inner(element, visiting)),
            Type::Array { element, .. } => self.is_trivial_inner(element, visiting),
            Type::Nominal { path, arguments } => {
                if !arguments
                    .iter()
                    .all(|argument| self.is_trivial_inner(argument, visiting))
                {
                    return false;
                }
                let Some(definition) = self.definition(path) else {
                    return false;
                };
                if !visiting.insert(ty.clone()) {
                    return false;
                }
                let trivial = match &definition.policy {
                    Policy::Explicit { trivial, .. } => *trivial,
                    Policy::Unmanaged => true,
                    Policy::Affine => false,
                    Policy::Counted => {
                        if definition.parameters.len() != arguments.len() {
                            false
                        } else {
                            let bindings = definition
                                .parameters
                                .iter()
                                .cloned()
                                .zip(arguments.iter().cloned())
                                .collect::<HashMap<_, _>>();
                            definition.fields.iter().all(|field| {
                                self.is_trivial_inner(&field.substitute(&bindings), visiting)
                            })
                        }
                    }
                };
                visiting.remove(ty);
                trivial
            }
            _ => true,
        }
    }

    fn is_copyable_inner(&self, ty: &Type, visiting: &mut HashSet<Type>) -> bool {
        match ty {
            Type::Unique(_) | Type::Closure(_) | Type::Generic(_) | Type::Frame(_) => false,
            Type::Tuple(elements) => elements
                .iter()
                .all(|element| self.is_copyable_inner(element, visiting)),
            Type::Array { element, .. } => self.is_copyable_inner(element, visiting),
            Type::Nominal { path, arguments } => {
                let Some(definition) = self.definition(path) else {
                    return false;
                };
                if definition.parameters.len() != arguments.len() || !visiting.insert(ty.clone()) {
                    return false;
                }
                let bindings = definition
                    .parameters
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect::<HashMap<_, _>>();
                let copyable = match &definition.policy {
                    Policy::Explicit { copyable, .. } => *copyable,
                    Policy::Unmanaged => true,
                    Policy::Counted => definition.fields.iter().all(|field| {
                        self.is_copyable_inner(&field.substitute(&bindings), visiting)
                    }),
                    Policy::Affine => false,
                };
                visiting.remove(ty);
                copyable
            }
            Type::Reference(_) | Type::Weak(_) => true,
            Type::Coroutine { .. } => false,
            _ => true,
        }
    }

    fn is_bitwise_relocatable_inner(&self, ty: &Type, visiting: &mut HashSet<Type>) -> bool {
        match ty {
            Type::Generic(_) | Type::Frame(_) | Type::ScalableVector(_) | Type::Address { .. } => {
                false
            }
            Type::Tuple(elements) => elements
                .iter()
                .all(|element| self.is_bitwise_relocatable_inner(element, visiting)),
            Type::Array { element, .. } => self.is_bitwise_relocatable_inner(element, visiting),
            Type::Nominal { path, arguments } => {
                let Some(definition) = self.definition(path) else {
                    return false;
                };
                if definition.parameters.len() != arguments.len() || !visiting.insert(ty.clone()) {
                    return false;
                }
                let bindings = definition
                    .parameters
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect::<HashMap<_, _>>();
                let relocation_allowed = match &definition.policy {
                    Policy::Explicit {
                        bitwise_relocate, ..
                    } => *bitwise_relocate,
                    Policy::Unmanaged | Policy::Counted | Policy::Affine => true,
                };
                let bitwise = relocation_allowed
                    && definition.fields.iter().all(|field| {
                        self.is_bitwise_relocatable_inner(&field.substitute(&bindings), visiting)
                    });
                visiting.remove(ty);
                bitwise
            }
            _ => true,
        }
    }

    fn definition(&self, path: &str) -> Option<&TypeDefinition> {
        if let Some(definition) = self.definitions.get(path) {
            return Some(definition);
        }
        if path.contains('.') {
            return None;
        }
        let short_name = path.rsplit('.').next()?;
        let mut candidates = self
            .definitions
            .values()
            .filter(|definition| definition.name.rsplit('.').next() == Some(short_name));
        let definition = candidates.next()?;
        candidates.next().is_none().then_some(definition)
    }
}

fn parse_definition(source: &str, tokens: &[Token]) -> Option<TypeDefinition> {
    let percent = tokens.iter().position(|token| punctuation(token, '%'))?;
    let name_start = percent + 1;
    let mut name_end = name_start;
    while name_end < tokens.len()
        && (matches!(tokens[name_end].kind, TokenKind::Atom(_))
            || punctuation(&tokens[name_end], '.'))
    {
        name_end += 1;
    }
    let name = source
        .get(tokens.get(name_start)?.span.start..tokens.get(name_end - 1)?.span.end)?
        .to_owned();
    let parameters = generic_parameters(tokens, name_end);
    let body_start = tokens
        .iter()
        .enumerate()
        .skip(name_end)
        .find(|(_, token)| punctuation(token, '{'))
        .map(|(index, _)| index)?;
    let body_end = matching_brace(tokens, body_start)?;
    let policy_start = tokens
        .iter()
        .enumerate()
        .skip(body_end + 1)
        .find(|(_, token)| atom(token) == Some("policy"))
        .map(|(index, _)| index)?;
    let fields = parse_fields(source, tokens, body_start + 1, body_end)?;
    let is_enum = (name_end..body_start).any(|index| atom(&tokens[index]) == Some("enum"));
    let variants = if is_enum {
        parse_variant_names(tokens, body_start + 1, body_end)
    } else {
        Vec::new()
    };
    let field_annotations = parse_field_annotations(tokens, body_start + 1, body_end);
    let policy = parse_policy(tokens, policy_start)?;
    let is_struct = (name_end..body_start).any(|index| atom(&tokens[index]) == Some("struct"));
    Some(TypeDefinition {
        name,
        parameters,
        fields,
        variants,
        field_annotations,
        policy,
        is_struct,
        is_enum,
    })
}

fn parse_variant_names(tokens: &[Token], start: usize, end: usize) -> Vec<String> {
    split_ranges(tokens, start, end)
        .into_iter()
        .filter_map(|(start, end)| {
            (start..end)
                .find(|index| !matches!(tokens[*index].kind, TokenKind::Newline | TokenKind::End))
                .and_then(|index| atom(&tokens[index]).map(str::to_owned))
        })
        .collect()
}

fn parse_field_annotations(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> HashMap<String, HashSet<String>> {
    let mut fields = HashMap::new();
    for (range_start, range_end) in split_ranges(tokens, start, end) {
        let field_start = (range_start..range_end)
            .find(|index| !matches!(tokens[*index].kind, TokenKind::Newline | TokenKind::End));
        let field_end = (range_start..range_end)
            .rfind(|index| !matches!(tokens[*index].kind, TokenKind::Newline | TokenKind::End))
            .map(|index| index + 1);
        let (Some(field_start), Some(field_end)) = (field_start, field_end) else {
            continue;
        };
        let Some(field) = tokens.get(field_start) else {
            continue;
        };
        let Some(name) = atom(field) else {
            continue;
        };
        let annotations = (field_start..field_end)
            .filter_map(|index| {
                (punctuation(&tokens[index], '@'))
                    .then(|| atom(tokens.get(index + 1)?).map(str::to_owned))
                    .flatten()
            })
            .collect::<HashSet<_>>();
        if !annotations.is_empty() {
            fields.insert(name.to_owned(), annotations);
        }
    }
    fields
}

fn generic_parameters(tokens: &[Token], name_end: usize) -> Vec<String> {
    if !tokens
        .get(name_end)
        .is_some_and(|token| punctuation(token, '<'))
    {
        return Vec::new();
    }
    let mut parameters = Vec::new();
    let mut cursor = name_end + 1;
    while cursor + 1 < tokens.len() && !punctuation(&tokens[cursor], '>') {
        if punctuation(&tokens[cursor], '$')
            && let Some(name) = atom(&tokens[cursor + 1])
        {
            parameters.push(name.to_owned());
        }
        cursor += 1;
    }
    parameters
}

fn parse_policy(tokens: &[Token], start: usize) -> Option<Policy> {
    if atom(tokens.get(start + 1)?) == Some("default") {
        return Some(match atom(tokens.get(start + 2)?)? {
            "unmanaged" => Policy::Unmanaged,
            "counted" => Policy::Counted,
            "affine" => Policy::Affine,
            _ => return None,
        });
    }
    let copy_trivial = policy_value(tokens, start, "copy") == Some("trivial");
    let destroy_trivial = policy_value(tokens, start, "destroy") == Some("trivial");
    Some(Policy::Explicit {
        trivial: copy_trivial && destroy_trivial,
        copyable: policy_value(tokens, start, "copy") != Some("none"),
        bitwise_relocate: policy_value(tokens, start, "relocate") == Some("bitwise"),
    })
}

fn policy_value<'a>(tokens: &'a [Token], start: usize, key: &str) -> Option<&'a str> {
    tokens[start..].windows(3).find_map(|window| {
        (atom(&window[0]) == Some(key) && punctuation(&window[1], ':'))
            .then(|| atom(&window[2]))
            .flatten()
    })
}

fn parse_fields(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Vec<Type>> {
    let ranges = split_ranges(tokens, start, end);
    let mut fields = Vec::new();
    for (part_start, part_end) in ranges {
        if part_start == part_end {
            continue;
        }
        let colon = (part_start..part_end).find(|index| punctuation(&tokens[*index], ':'));
        if let Some(colon) = colon {
            let ty_end = (colon + 1..part_end)
                .find(|index| punctuation(&tokens[*index], '@'))
                .unwrap_or(part_end);
            if let Some(ty) = parse_type_range(source, tokens, colon + 1, ty_end) {
                fields.push(ty);
            }
            continue;
        }
        if let Some(open) = (part_start..part_end).find(|index| punctuation(&tokens[*index], '('))
            && let Some(close) = matching_pair(tokens, open, '(', ')')
        {
            for (argument_start, argument_end) in split_ranges(tokens, open + 1, close) {
                if let Some(ty) = parse_type_range(source, tokens, argument_start, argument_end) {
                    fields.push(ty);
                }
            }
        }
    }
    Some(fields)
}

fn parse_type_range(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    parse_type(source.get(first.span.start..last.span.end)?).ok()
}

fn split_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut part_start = start;
    let mut stack = Vec::new();
    for index in start..end {
        match &tokens[index].kind {
            TokenKind::Punctuation('(') => stack.push('('),
            TokenKind::Punctuation('[') => stack.push('['),
            TokenKind::Punctuation('{') => stack.push('{'),
            TokenKind::Punctuation('<') => stack.push('<'),
            TokenKind::Punctuation(')') => pop_if(&mut stack, '('),
            TokenKind::Punctuation(']') => pop_if(&mut stack, '['),
            TokenKind::Punctuation('}') => pop_if(&mut stack, '{'),
            TokenKind::Punctuation('>') => pop_if(&mut stack, '<'),
            TokenKind::Punctuation(',') if stack.is_empty() => {
                ranges.push((part_start, index));
                part_start = index + 1;
            }
            _ => {}
        }
    }
    ranges.push((part_start, end));
    ranges
}

fn matching_brace(tokens: &[Token], open: usize) -> Option<usize> {
    matching_pair(tokens, open, '{', '}')
}

fn matching_pair(tokens: &[Token], open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if punctuation(token, left) {
            depth += 1;
        } else if punctuation(token, right) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn pop_if(stack: &mut Vec<char>, expected: char) {
    if stack.last() == Some(&expected) {
        stack.pop();
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}
