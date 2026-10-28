//! The KIR signature of a KLF Kira function and the driver that runs its body.
//!
//! Two shapes need translating before the body can be lowered: aggregate
//! results, which become a hidden out-pointer parameter, and the definition
//! `units` phase, whose `[SourceFile]` argument and `[Unit]` result cross the
//! phase boundary as integer handles rather than as lists.

use super::{Access, Local, Lowerer};
use crate::ty::{DefinitionHandle, FeType};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Linkage};
use klf_kira_syntax::ast::{FunctionDecl, Mode, Param};
use klf_services::{FunctionBuilder, Session};
use std::collections::{HashMap, HashSet};

/// The names in scope while a signature is resolved.
struct Names {
    structs: HashSet<Name>,
    enums: HashSet<Name>,
    generics: HashSet<Name>,
    /// In an instance, the type each generic parameter stands for.
    substitutions: HashMap<Name, FeType>,
}

impl Names {
    fn resolve(&self, session: &mut Session, ty: &klf_kira_syntax::ast::Type) -> Option<FeType> {
        crate::structs::resolve_type_with_context(
            session,
            ty,
            &self.structs,
            &self.enums,
            &self.generics,
        )
        .map(|resolved| resolved.substitute(&self.substitutions))
    }
}

/// The KIR parameter list and what the body has to bind from it.
struct Parameters {
    kir: Vec<(Name, klf_kir::Type)>,
    /// Parameters the body binds directly, as (source name, SSA name, type).
    bindings: Vec<(Name, Name, FeType)>,
    /// The `move` parameters, whose storage the body owns.
    owned: Vec<(Name, FeType)>,
    /// The parameters passed without `move`, `copy` or `mut`.
    read_only: Vec<Name>,
    /// The types of the parameters the body may write through.
    writable: Vec<FeType>,
    /// For `units`: the declared list parameter and the count it arrives as.
    definition_inputs: Option<(Name, Name)>,
}

/// The lowered signature: what KIR sees, plus what the body needs to bind.
struct Abi {
    params: Vec<(Name, klf_kir::Type)>,
    bindings: Vec<(Name, Name, FeType)>,
    owned: Vec<(Name, FeType)>,
    read_only: Vec<Name>,
    writable: Vec<FeType>,
    definition_inputs: Option<(Name, Name)>,
    result: FeType,
    kir_result: klf_kir::Type,
    ret_slot: Option<Name>,
    units_result: bool,
}

pub(crate) fn lower_function(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    func: &FunctionDecl,
    owner: Option<&FeType>,
    instance: Option<&super::Instance>,
) -> Option<Function> {
    let body = check_lowerable(session, &elaborated.types, func)?;
    let names = Names {
        structs: elaborated.structs.keys().cloned().collect(),
        enums: elaborated.enums.keys().cloned().collect(),
        generics: func
            .generics
            .iter()
            .map(|generic| generic.name.name.clone())
            .collect(),
        substitutions: instance
            .map(|instance| instance.substitutions.clone())
            .unwrap_or_default(),
    };
    let abi = build_abi(session, func, owner, &names);

    let name = instance.map_or_else(|| func.name.name.clone(), |instance| instance.name.clone());
    let mut fb = FunctionBuilder::new(name, &abi.params, abi.kir_result.clone(), func.span);
    fb.set_linkage(Linkage::Module);

    // `units` returns its `[Unit]` list one handle at a time, so the session it
    // was called with has to stay reachable from the return.
    let definition_session = (abi.units_result || abi.definition_inputs.is_some())
        .then(|| session_parameter(func))
        .flatten();
    let mut lowerer = Lowerer::new(
        session,
        elaborated,
        counts,
        fb,
        abi.result.clone(),
        abi.ret_slot,
    );
    lowerer.generic_names = names.generics;
    lowerer.substitutions = names.substitutions;
    lowerer.definition_session = abi
        .units_result
        .then(|| definition_session.clone())
        .flatten();
    lowerer.push_scope();
    for (name, access_name, ty) in &abi.bindings {
        // A parameter no writable one can hold is shared: nothing the body
        // does reaches its storage, so a `let` may read inside it in place.
        let shared = (abi.read_only.contains(name)
            && !abi
                .writable
                .iter()
                .any(|writable| lowerer.reaches(writable, ty)))
        .then_some(super::Shared::Parameter);
        lowerer.bind(
            name.clone(),
            Local {
                access: Access::Ssa(access_name.clone()),
                ty: ty.clone(),
                mutable: false,
                shared,
            },
        );
    }
    for (storage, ty) in abi.owned {
        lowerer.own_in_scope(storage, ty);
    }
    if let Some((name, count)) = abi.definition_inputs {
        bind_input_list(&mut lowerer, name, &count, definition_session);
    }
    lowerer.lower_block(body);
    lowerer.pop_scope();
    close_body(&mut lowerer, &abi.result, body.span);
    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    Some(function)
}

/// Rejects the two shapes lowering cannot start from, and yields the body.
fn check_lowerable<'a>(
    session: &mut Session,
    types: &crate::session_types::TypeHandles,
    func: &'a FunctionDecl,
) -> Option<&'a klf_kira_syntax::ast::Block> {
    let Some(body) = &func.body else {
        session.report(
            Diagnostic::error(
                "KLFK-E0206",
                "a function without a body is outside the M4 subset",
            )
            .with_label(func.span, "expected a block"),
        );
        return None;
    };
    if types.function_type(&func.name.name).is_none() {
        session.report(
            Diagnostic::error("KLFK-E0206", "function has no session type handle")
                .with_label(func.name.span, "elaboration did not register this function"),
        );
        return None;
    }
    Some(body)
}

fn build_abi(
    session: &mut Session,
    func: &FunctionDecl,
    owner: Option<&FeType>,
    names: &Names,
) -> Abi {
    let parameters = build_parameters(session, func, owner, names);
    let result = func
        .ret
        .as_ref()
        .and_then(|ty| names.resolve(session, ty))
        .unwrap_or(FeType::Void);
    let units_result = is_definition_units_result(func, &result);
    let mut abi = Abi {
        params: parameters.kir,
        bindings: parameters.bindings,
        owned: parameters.owned,
        read_only: parameters.read_only,
        writable: parameters.writable,
        definition_inputs: parameters.definition_inputs,
        kir_result: klf_kir::Type::Unit,
        ret_slot: None,
        units_result,
        result,
    };
    build_result(session, func, &mut abi);
    abi
}

fn build_parameters(
    session: &mut Session,
    func: &FunctionDecl,
    owner: Option<&FeType>,
    names: &Names,
) -> Parameters {
    let mut kir = Vec::new();
    let mut bindings = Vec::new();
    let mut moved = Vec::new();
    let mut read_only = Vec::new();
    let mut writable = Vec::new();
    // The `units` phase is handed the number of inputs and rebuilds the
    // `[SourceFile]` list it declares from the input service.
    let mut definition_inputs = None;
    for parameter in &func.params {
        let declared = match (parameter, owner) {
            (Param::Named(declared), _) => declared,
            (Param::SelfParam { span, mode }, Some(owner)) => {
                if *mode != Some(Mode::Borrow) {
                    writable.push(owner.clone());
                }
                let receiver = Name::new("self");
                kir.push((receiver.clone(), parameter_kir_type(session, owner, *span)));
                bindings.push((receiver.clone(), receiver, owner.clone()));
                continue;
            }
            (Param::SelfParam { .. }, None) => continue,
        };
        let ty = names.resolve(session, &declared.ty).unwrap_or(FeType::Void);
        if is_definition_input_list(func, &ty) {
            let count = Name::new(format!("{}_count", declared.name.name));
            kir.push((count.clone(), klf_kir::Type::Integer(64)));
            definition_inputs = Some((declared.name.name.clone(), count));
            continue;
        }
        let kir_ty = parameter_kir_type(session, &ty, declared.ty.span);
        kir.push((declared.name.name.clone(), kir_ty));
        match declared.mode {
            Some(Mode::Move) => moved.push((declared.name.name.clone(), ty.clone())),
            None | Some(Mode::Borrow) => read_only.push(declared.name.name.clone()),
            Some(Mode::BorrowMut) => writable.push(ty.clone()),
            Some(Mode::Copy) => {}
        }
        bindings.push((declared.name.name.clone(), declared.name.name.clone(), ty));
    }
    Parameters {
        kir,
        bindings,
        owned: moved,
        read_only,
        writable,
        definition_inputs,
    }
}

/// An aggregate is passed by reference: the KIR parameter is a pointer to the
/// caller's storage, and the callee treats the parameter name as that address.
fn parameter_kir_type(session: &mut Session, ty: &FeType, span: klf_source::Span) -> klf_kir::Type {
    match ty {
        FeType::Struct(name) | FeType::Enum(name) => {
            klf_kir::Type::Address(Box::new(klf_kir::Type::Nominal(name.clone(), Vec::new())))
        }
        FeType::List(_) | FeType::Function { .. } => klf_kir::Type::Address(Box::new(ty.to_kir())),
        FeType::Array { .. } => {
            session.report(
                Diagnostic::error("KLFK-E0206", "array parameters are not lowered yet")
                    .with_label(span, "not supported yet"),
            );
            klf_kir::Type::Unit
        }
        other => other.to_kir(),
    }
}

/// Fills in `abi_result` and, for an aggregate result, the out-pointer slot.
fn build_result(session: &mut Session, func: &FunctionDecl, abi: &mut Abi) {
    // The definition `units` phase has a special integer-handle ABI, so its
    // `[Unit]` result is not lowered through the aggregate out-pointer path.
    if abi.units_result {
        abi.kir_result = klf_kir::Type::Integer(64);
        return;
    }
    let aggregate = match &abi.result {
        FeType::Struct(name) | FeType::Enum(name) => Some(name.clone()),
        FeType::List(_) => Some(Name::new("List")),
        FeType::Function { .. } => Some(Name::new(crate::ty::CLOSURE_RECORD)),
        _ => None,
    };
    let Some(name) = aggregate else {
        if matches!(abi.result, FeType::Array { .. }) {
            session.report(
                Diagnostic::error("KLFK-E0206", "array return types are not lowered yet")
                    .with_label(func.span, "not supported yet"),
            );
        }
        abi.kir_result = abi.result.to_kir();
        return;
    };
    // Aggregate results use an out-pointer (sret): the caller allocates storage
    // and passes its address as a hidden leading parameter the callee fills.
    let ret_name = Name::new("__ret");
    abi.params.insert(
        0,
        (
            ret_name.clone(),
            klf_kir::Type::Address(Box::new(klf_kir::Type::Nominal(name, Vec::new()))),
        ),
    );
    abi.ret_slot = Some(ret_name);
}

fn bind_input_list(
    lowerer: &mut Lowerer<'_>,
    name: Name,
    count: &Name,
    definition_session: Option<Name>,
) {
    let element = FeType::DefinitionHandle(DefinitionHandle::SourceFile);
    let session_value = definition_session.unwrap_or_else(|| count.clone());
    let header = lowerer.build_input_list(&session_value, count, &element);
    lowerer.bind(
        name,
        Local {
            access: Access::Ssa(header),
            ty: FeType::List(Box::new(element)),
            mutable: false,
            shared: None,
        },
    );
}

pub(super) fn close_body(lowerer: &mut Lowerer<'_>, result: &FeType, span: klf_source::Span) {
    if !lowerer.reachable {
        return;
    }
    if *result == FeType::Void {
        lowerer.fb.ret(None);
        return;
    }
    lowerer.session.report(
        Diagnostic::error(
            "KLFK-E0205",
            "control reaches the end of a non-Void function",
        )
        .with_label(span, "missing a return"),
    );
    lowerer.trap_reported(
        span,
        &super::trap_report::TrapText {
            code: "KLF-RT-0001",
            headline: "control reached the end of a function that returns a value",
            label: "missing a return",
            note: "every path through a non-Void function must end in `return`",
            help: "add a `return` on the path that falls through",
        },
    );
}

/// The value of the leading `session: Session` parameter every phase declares.
fn session_parameter(func: &FunctionDecl) -> Option<Name> {
    let Param::Named(named) = func.params.first()? else {
        return None;
    };
    Some(named.name.name.clone())
}

fn is_definition_input_list(func: &FunctionDecl, ty: &FeType) -> bool {
    // A parse takes the program's files as its `program` list, so its loader
    // reads the other packages of the program (`docs/conformers.md`).
    matches!(func.name.name.as_str(), "units" | "parse")
        && matches!(
            ty,
            FeType::List(element)
                if **element == FeType::DefinitionHandle(DefinitionHandle::SourceFile)
        )
}

fn is_definition_units_result(func: &FunctionDecl, ty: &FeType) -> bool {
    func.name.name.as_str() == "units"
        && matches!(
            ty,
            FeType::List(element)
                if **element == FeType::DefinitionHandle(DefinitionHandle::Unit)
        )
}
