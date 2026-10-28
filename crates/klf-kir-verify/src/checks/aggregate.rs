//! V2 rules for the aggregate instructions of section 12.6.

use super::{Context, enum_variant, require};
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Instruction, Operation, Type};

pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_structs(context, instruction, diagnostics)
        || verify_arrays(context, instruction, diagnostics)
}

/// Struct and tuple construction and projection.
fn verify_structs(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::StructNew { ty, fields } => {
            let Some(declaration) = context.declaration(Some(ty)) else {
                type_error(
                    instruction,
                    "`struct.new` requires a declared nominal type",
                    diagnostics,
                );
                return true;
            };
            if !require(
                declaration.fields.len() == fields.len(),
                "`struct.new` field count does not match the declaration",
                instruction,
                diagnostics,
            ) {
                return true;
            }
            for (declared, value) in declaration.fields.iter().zip(fields) {
                require(
                    context.ty(value) == Some(&declared.ty),
                    format!("`struct.new` field `{}` has the wrong type", declared.name),
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::StructExtract { aggregate, field } => {
            let declared = context
                .declaration(context.ty(aggregate))
                .and_then(|declaration| declaration.fields.get(*field as usize));
            require(
                declared.is_some(),
                "`struct.extract` requires a declared nominal struct and valid field index",
                instruction,
                diagnostics,
            );
        }
        Operation::StructDestructure(value) => {
            let Some(declaration) = context.declaration(context.ty(value)) else {
                type_error(
                    instruction,
                    "`struct.destructure` requires a declared nominal struct",
                    diagnostics,
                );
                return true;
            };
            require(
                instruction.results.len() == declaration.fields.len(),
                "`struct.destructure` must bind one result per field",
                instruction,
                diagnostics,
            );
        }
        Operation::TupleNew(values) => {
            for value in values {
                require(
                    context.ty(value).is_some(),
                    "`tuple.new` operand has no known type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::TupleExtract { tuple, index } => {
            require(
                tuple_elements(context, tuple)
                    .is_some_and(|elements| (*index as usize) < elements.len()),
                "`tuple.extract` requires a tuple and a valid index",
                instruction,
                diagnostics,
            );
        }
        Operation::TupleDestructure(value) => {
            let Some(elements) = tuple_elements(context, value) else {
                type_error(
                    instruction,
                    "`tuple.destructure` requires a tuple operand",
                    diagnostics,
                );
                return true;
            };
            require(
                instruction.results.len() == elements.len(),
                "`tuple.destructure` must bind one result per element",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

/// Array and enum construction, projection and injection.
fn verify_arrays(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::ArrayNew { element, elements } => {
            for value in elements {
                require(
                    context.ty(value) == Some(element),
                    "`array.new` operand does not match the element type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::EnumNew {
            ty,
            variant,
            payload,
        } => {
            let case = context
                .declaration(Some(ty))
                .and_then(|declaration| declaration.variants.get(*variant as usize));
            let Some(case) = case else {
                type_error(
                    instruction,
                    "`enum.new` requires a declared enum and a valid variant",
                    diagnostics,
                );
                return true;
            };
            if !require(
                case.payload.len() == payload.len(),
                "`enum.new` payload count does not match the variant",
                instruction,
                diagnostics,
            ) {
                return true;
            }
            for (declared, value) in case.payload.iter().zip(payload) {
                require(
                    context.ty(value) == Some(declared),
                    "`enum.new` payload has the wrong type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::EnumTag { value } => {
            require(
                context.enum_declaration(context.ty(value)).is_some(),
                "`enum.tag` requires an address of a declared enum",
                instruction,
                diagnostics,
            );
        }
        Operation::EnumTake { value, variant } | Operation::EnumProject { value, variant } => {
            let case = enum_variant(
                context,
                value,
                *variant,
                "enum payload access requires a declared enum and a valid variant",
                instruction,
                diagnostics,
            );
            if let Some(case) = case {
                require(
                    instruction.results.len() == case.payload.len(),
                    "enum payload access must bind one result per payload element",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::EnumPayloadAddr { base, variant } => {
            enum_variant(
                context,
                base,
                *variant,
                "`enum.payload.addr` requires a declared enum and valid variant",
                instruction,
                diagnostics,
            );
        }
        Operation::EnumInject { address, variant } => {
            enum_variant(
                context,
                address,
                *variant,
                "`enum.inject` requires a declared enum address and valid variant",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

fn tuple_elements<'a>(context: &Context<'a>, value: &Name) -> Option<&'a [Type]> {
    match context.ty(value) {
        Some(Type::Tuple(elements)) => Some(elements),
        _ => None,
    }
}
