//! Verifier rule V12 (section 11.7): an exported function's signature uses only
//! RIDL-representable types. Raw pointers, addresses, bare generics, function
//! values and vectors are not part of the RIDL type system (section 20) and may
//! not cross an exported boundary.

use crate::Definition;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Engine, Function, Linkage, Operation, RelocatePolicy, Type, TypeDeclaration};
use rustc_hash::FxHashMap as HashMap;
use std::collections::BTreeSet;

const V8: &str = "KLF-KIR-1008";
const V9: &str = "KLF-KIR-1009";

/// Verifier rule V8 (section 11.7): an address-only type (one whose `relocate`
/// policy is a function) never appears as a loadable SSA value. This checks
/// instruction results, which are always loadable.
pub(crate) fn verify_address_only(
    function: &Function,
    types: &[TypeDeclaration],
    definitions: &HashMap<Name, Definition>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let address_only: BTreeSet<&Name> = types
        .iter()
        .filter(|declaration| matches!(declaration.policy.relocate, RelocatePolicy::Function(_)))
        .map(|declaration| &declaration.name)
        .collect();
    if address_only.is_empty() {
        return;
    }
    for (name, definition) in definitions {
        // Only instruction results are loadable SSA values here; block
        // parameters may be borrowed references to address-only storage.
        if definition.position.is_none() {
            continue;
        }
        if let Some(Type::Nominal(nominal, _)) = &definition.ty
            && address_only.contains(nominal)
        {
            diagnostics.push(
                Diagnostic::error(
                    V8,
                    format!("address-only type `%{nominal}` held by value in `%{name}`"),
                )
                .with_label(
                    function.span,
                    "address-only value must live at an address (V8)",
                ),
            );
        }
    }
}
const V10: &str = "KLF-KIR-1010";
const V11: &str = "KLF-KIR-1011";
const V12: &str = "KLF-KIR-1012";

/// Verifier rule V10 (section 11.7): `try.call` is used if and only if the
/// callee can fail. A `try.call` to an infallible callee, or a plain `call` to a
/// fallible one, is rejected.
pub(crate) fn verify_calls(
    function: &Function,
    index: &klf_kir::ModuleIndex<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let fallible = |callee: &Name| {
        index
            .functions
            .get(callee.as_str())
            .map(|f| f.failure.is_some())
    };
    for block in &function.blocks {
        for instruction in &block.instructions {
            match &instruction.operation {
                Operation::Call { callee, .. } if fallible(callee) == Some(true) => {
                    diagnostics.push(
                        Diagnostic::error(
                            V10,
                            format!("call to fallible `@{callee}` must use `try.call`"),
                        )
                        .with_label(instruction.span, "plain call to a fallible function (V10)"),
                    );
                }
                Operation::TryCall(call) if fallible(&call.callee) == Some(false) => {
                    let callee = &call.callee;
                    diagnostics.push(
                        Diagnostic::error(V10, format!("`try.call` to infallible `@{callee}`"))
                            .with_label(
                                instruction.span,
                                "try.call of a non-fallible function (V10)",
                            ),
                    );
                }
                _ => {}
            }
        }
    }
}

/// Verifier rule V9 (section 11.7): a function with engine `runtime` or `any`
/// contains no inline assembly.
pub(crate) fn verify_asm(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    if matches!(function.engine, Engine::Native) {
        return;
    }
    for block in &function.blocks {
        for instruction in &block.instructions {
            if matches!(instruction.operation, Operation::Asm(_)) {
                diagnostics.push(
                    Diagnostic::error(
                        V9,
                        format!(
                            "function `@{}` has engine `runtime`/`any` but contains `asm`",
                            function.name
                        ),
                    )
                    .with_label(instruction.span, "asm forces engine native (V9)"),
                );
            }
        }
    }
}

fn representable(ty: &Type) -> bool {
    match ty {
        Type::Integer(_) | Type::Float(_) | Type::Unit | Type::Nominal(_, _) => true,
        Type::Reference(inner)
        | Type::Unique(inner)
        | Type::Weak(inner)
        | Type::Array(_, inner) => representable(inner),
        Type::Tuple(elements) => elements.iter().all(representable),
        Type::Pointer
        | Type::Never
        | Type::Address(_)
        | Type::Generic(_)
        | Type::Function(_)
        | Type::Vector(_, _) => false,
    }
}

/// Verifier rule V11 (section 11.7, first clause): a generic parameter `$T` may
/// appear only in a function that declares it.
pub(crate) fn verify_generics(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    let declared: BTreeSet<&Name> = function
        .generics
        .iter()
        .map(|generic| &generic.name)
        .collect();
    let mut used = BTreeSet::new();
    for parameter in &function.parameters {
        collect_generics(&parameter.ty, &mut used);
    }
    collect_generics(&function.result, &mut used);
    if let Some(failure) = &function.failure {
        collect_generics(failure, &mut used);
    }
    for block in &function.blocks {
        for parameter in &block.parameters {
            collect_generics(&parameter.ty, &mut used);
        }
        for instruction in &block.instructions {
            for ty in operation_types(&instruction.operation) {
                collect_generics(ty, &mut used);
            }
        }
    }
    for name in used {
        if !declared.contains(name) {
            diagnostics.push(
                Diagnostic::error(
                    V11,
                    format!(
                        "generic parameter `${name}` is used in `@{}`, which does not declare it",
                        function.name
                    ),
                )
                .with_label(function.span, "undeclared generic parameter (V11)"),
            );
        }
    }
}

fn collect_generics<'a>(ty: &'a Type, into: &mut BTreeSet<&'a Name>) {
    match ty {
        Type::Generic(name) => {
            into.insert(name);
        }
        Type::Reference(inner)
        | Type::Unique(inner)
        | Type::Weak(inner)
        | Type::Address(inner)
        | Type::Vector(_, inner)
        | Type::Array(_, inner) => collect_generics(inner, into),
        Type::Tuple(elements) => elements.iter().for_each(|e| collect_generics(e, into)),
        Type::Nominal(_, arguments) => arguments.iter().for_each(|a| collect_generics(a, into)),
        _ => {}
    }
}

fn operation_types(operation: &Operation) -> Vec<&Type> {
    match operation {
        Operation::ConstInt { ty, .. }
        | Operation::ConstFloat { ty, .. }
        | Operation::ConstZero(ty)
        | Operation::Convert { ty, .. }
        | Operation::StackAlloc(ty)
        | Operation::HeapAllocCounted(ty)
        | Operation::HeapAllocUnique(ty)
        | Operation::DescriptorReference(ty)
        | Operation::WitnessReference { ty, .. } => vec![ty],
        Operation::HeapAllocArray { element, .. } => vec![element],
        Operation::Call { generic_args, .. } => generic_args.iter().collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn verify_signature(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    if !matches!(function.linkage, Linkage::Exported) {
        return;
    }
    let mut check = |ty: &Type, what: &str| {
        if !representable(ty) {
            diagnostics.push(
                Diagnostic::error(
                    V12,
                    format!(
                        "exported function `@{}` has a non-RIDL-representable {what} type `{ty}`",
                        function.name
                    ),
                )
                .with_label(function.span, "not RIDL-representable (V12)"),
            );
        }
    };
    for parameter in &function.parameters {
        check(&parameter.ty, "parameter");
    }
    check(&function.result, "result");
    if let Some(failure) = &function.failure {
        check(failure, "failure");
    }
}
