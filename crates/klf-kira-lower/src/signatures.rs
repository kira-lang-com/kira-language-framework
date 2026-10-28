//! Function signatures: every top-level function, definition phase and method,
//! resolved against the declared types, plus the built-in service wrappers.

use crate::methods::Method;
use crate::structs::resolve_type_with_context;
use crate::ty::FeType;
use crate::{builtins, trait_id_for};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{CMember, DeclKind, File, FunctionDecl, MemberKind, Mode, Param};
use klf_services::{Session, TraitId};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) struct Signature {
    pub generics: Vec<Name>,
    pub bounds: HashMap<Name, Vec<TraitId>>,
    /// The parameter types, the receiver first when `receiver` is set.
    pub params: Vec<FeType>,
    /// Which parameters are declared `move`: the callee owns what it is
    /// handed there and releases it, so the caller gives its counts up.
    pub moves: Vec<bool>,
    pub result: FeType,
    pub receiver: bool,
}

impl Signature {
    /// A non-generic free function.
    pub(crate) fn free(params: Vec<FeType>, result: FeType) -> Self {
        Self {
            generics: Vec::new(),
            bounds: HashMap::new(),
            moves: vec![false; params.len()],
            params,
            result,
            receiver: false,
        }
    }

    /// The parameters a call site supplies arguments for.
    /// Which of `arguments` are declared `move`.
    pub(crate) fn argument_moves(&self) -> &[bool] {
        if self.receiver {
            &self.moves[1..]
        } else {
            &self.moves
        }
    }

    pub(crate) fn arguments(&self) -> &[FeType] {
        if self.receiver {
            &self.params[1..]
        } else {
            &self.params
        }
    }
}

/// The names a signature's types resolve against.
pub(crate) struct TypeNames<'a> {
    pub structs: &'a HashSet<Name>,
    pub enums: &'a HashSet<Name>,
    pub traits: &'a HashMap<Name, TraitId>,
}

pub(crate) fn collect(
    session: &mut Session,
    file: &File,
    methods: &[Method],
    scope: &TypeNames<'_>,
) -> HashMap<Name, Signature> {
    let mut signatures = HashMap::new();
    for function in free_functions(file) {
        let signature = signature(session, function, None, scope);
        signatures.insert(function.name.name.clone(), signature);
    }
    for method in methods {
        let signature = signature(session, &method.decl, Some(&method.owner), scope);
        if signatures
            .insert(method.decl.name.name.clone(), signature)
            .is_some()
        {
            session.report(
                Diagnostic::error(
                    "KLFK-E0200",
                    format!("duplicate method `{}`", method.decl.name.name),
                )
                .with_label(method.decl.name.span, "already declared"),
            );
        }
    }
    for (name, signature) in builtins::signatures(session) {
        signatures.entry(name).or_insert(signature);
    }
    signatures
}

/// Top-level functions and the phases a `Language` construct declares.
fn free_functions(file: &File) -> Vec<&FunctionDecl> {
    let mut functions = Vec::new();
    for decl in &file.decls {
        match &decl.kind {
            DeclKind::Function(function) => functions.push(function),
            DeclKind::Construct(construct) => {
                functions.extend(construct.members.iter().filter_map(|member| {
                    let CMember::Member(member) = member else {
                        return None;
                    };
                    let MemberKind::Method(function) = &member.kind else {
                        return None;
                    };
                    is_definition_phase(function).then_some(function)
                }));
            }
            _ => {}
        }
    }
    functions
}

fn signature(
    session: &mut Session,
    function: &FunctionDecl,
    owner: Option<&FeType>,
    scope: &TypeNames<'_>,
) -> Signature {
    let generics: Vec<Name> = function
        .generics
        .iter()
        .map(|generic| generic.name.name.clone())
        .collect();
    let generic_names: HashSet<Name> = generics.iter().cloned().collect();
    let bounds = function
        .generics
        .iter()
        .map(|generic| {
            let bounds = generic
                .bounds
                .iter()
                .filter_map(|bound| trait_id_for(session, bound, scope.traits))
                .collect();
            (generic.name.name.clone(), bounds)
        })
        .collect();
    let resolve = |session: &mut Session, ty| {
        resolve_type_with_context(session, ty, scope.structs, scope.enums, &generic_names)
            .unwrap_or(FeType::Void)
    };
    let mut params = Vec::new();
    let mut moves = Vec::new();
    let mut receiver = false;
    for (index, param) in function.params.iter().enumerate() {
        match (param, owner) {
            (Param::Named(named), _) => {
                params.push(resolve(session, &named.ty));
                moves.push(named.mode == Some(Mode::Move));
            }
            (Param::SelfParam { .. }, Some(owner)) if index == 0 => {
                params.push(owner.clone());
                moves.push(false);
                receiver = true;
            }
            (Param::SelfParam { span, .. }, _) => session.report(
                Diagnostic::error("KLFK-E0206", "`self` is only a method's first parameter")
                    .with_label(*span, "not a receiver position"),
            ),
        }
    }
    let result = function
        .ret
        .as_ref()
        .map_or(FeType::Void, |ty| resolve(session, ty));
    Signature {
        generics,
        bounds,
        params,
        moves,
        result,
        receiver,
    }
}

pub(crate) fn is_definition_phase(function: &FunctionDecl) -> bool {
    matches!(
        function.name.name.as_str(),
        "units"
            | "parse"
            | "elaborate"
            | "lower"
            | "compatDriver"
            | "runProgram"
            | "emit"
            | "format"
            | "project"
    )
}
