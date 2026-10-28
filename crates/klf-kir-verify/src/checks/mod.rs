//! The V2 type-checking pass: result shapes and per-operation type rules.

mod aggregate;
mod arith;
mod calls;
mod control;
mod memory;
mod system;

use crate::{Definition, V2, pointee, type_error, value_type};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Instruction, RelocatePolicy, ResultArity, Type, TypeDeclaration};
use rustc_hash::FxHashMap as HashMap;

/// Everything a V2 rule may consult about the module around an instruction.
pub(crate) struct Context<'a> {
    pub function: &'a Function,
    /// The module's functions and types by name.
    pub index: &'a klf_kir::ModuleIndex<'a>,
    pub blocks: &'a HashMap<Name, usize>,
    pub definitions: &'a HashMap<Name, Definition>,
}

impl<'a> Context<'a> {
    pub(crate) fn ty(&self, name: &Name) -> Option<&'a Type> {
        value_type(name, self.definitions)
    }

    pub(crate) fn pointee(&self, name: &Name) -> Option<&'a Type> {
        self.ty(name).and_then(pointee)
    }

    /// What `name` addresses when it is used as an address.
    ///
    /// Section 11.6: a value of an address-only type exists only in memory and
    /// is manipulated through `*T`, and section 24.2 passes one by reference.
    /// After pass 5 such a value is its own address, so the address operations
    /// accept it directly.
    pub(crate) fn addressed(&self, name: &Name) -> Option<&'a Type> {
        let ty = self.ty(name)?;
        if self.is_address_only(ty) {
            return Some(ty);
        }
        pointee(ty)
    }

    fn is_address_only(&self, ty: &Type) -> bool {
        match ty {
            Type::Generic(_) => true,
            Type::Nominal(..) => self.declaration(Some(ty)).is_some_and(|declaration| {
                matches!(declaration.policy.relocate, RelocatePolicy::Function(_))
            }),
            _ => false,
        }
    }

    pub(crate) fn is_integer(&self, name: &Name) -> bool {
        self.ty(name).is_some_and(Type::is_integer)
    }

    pub(crate) fn is_pointer(&self, name: &Name) -> bool {
        matches!(self.ty(name), Some(Type::Pointer))
    }

    /// The declaration of the enum `name` addresses, if it is one.
    pub(crate) fn enum_declaration(&self, ty: Option<&Type>) -> Option<&'a TypeDeclaration> {
        let Type::Nominal(name, _) = pointee(ty?)? else {
            return None;
        };
        let declaration = self.index.types.get(name.as_str()).copied()?;
        (!declaration.variants.is_empty()).then_some(declaration)
    }

    /// The declaration of a nominal type named directly, without an address.
    pub(crate) fn declaration(&self, ty: Option<&Type>) -> Option<&'a TypeDeclaration> {
        let Type::Nominal(name, _) = ty? else {
            return None;
        };
        self.index.types.get(name.as_str()).copied()
    }

    /// The function `name` declares or defines.
    pub(crate) fn function_named(&self, name: &Name) -> Option<&'a Function> {
        self.index.functions.get(name.as_str()).copied()
    }
}

pub(crate) fn verify_types(
    function: &Function,
    index: &klf_kir::ModuleIndex<'_>,
    blocks: &HashMap<Name, usize>,
    definitions: &HashMap<Name, Definition>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let context = Context {
        function,
        index,
        blocks,
        definitions,
    };
    for block in &function.blocks {
        for instruction in &block.instructions {
            verify_result_shape(instruction, diagnostics);
            verify_operation(&context, instruction, diagnostics);
        }
    }
}

/// The first half of V2: an instruction's result count must match the arity its
/// operation declares. `ResultArity::Declared` covers the operations whose
/// result count follows from a callee signature or an aggregate's shape; those
/// are checked by the rule for the operation itself.
fn verify_result_shape(instruction: &Instruction, diagnostics: &mut Vec<Diagnostic>) {
    let ResultArity::Fixed(expected) = instruction.operation.result_arity() else {
        return;
    };
    if instruction.results.len() == expected {
        return;
    }
    diagnostics.push(
        Diagnostic::error(
            V2,
            format!(
                "instruction declares {} results but its operation yields {expected}",
                instruction.results.len()
            ),
        )
        .with_label(instruction.span, "instruction result shape is invalid"),
    );
}

fn verify_operation(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for family in [
        arith::verify,
        aggregate::verify,
        memory::verify,
        calls::verify,
        control::verify,
        system::verify,
    ] {
        if family(context, instruction, diagnostics) {
            return;
        }
    }
}

/// Checks that `%value` holds a declared enum reachable through an address and
/// that `variant` names one of its cases, reporting `message` otherwise.
pub(crate) fn enum_variant<'a>(
    context: &Context<'a>,
    value: &Name,
    variant: u32,
    message: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a klf_kir::EnumVariant> {
    let case = context
        .enum_declaration(context.ty(value))
        .and_then(|declaration| declaration.variants.get(variant as usize));
    if case.is_none() {
        type_error(instruction, message, diagnostics);
    }
    case
}

/// Reports `message` unless `condition` holds. Returns `condition`, so a rule
/// can stop after the first failure.
pub(crate) fn require(
    condition: bool,
    message: impl Into<String>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if !condition {
        type_error(instruction, message, diagnostics);
    }
    condition
}
