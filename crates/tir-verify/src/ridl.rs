use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    BinaryFunctionType, BinaryImport, BinaryImportEntityKind, BinaryImportKind, BinaryLayout,
    BinaryLinkage, BinarySignatureTable, BinaryType, BinaryTypeTable, TextItemKind, TextModule,
    function_has_header_attribute, function_uses_c_abi, parse_function_signature,
};
use tir_layout::{DefinitionKind, LayoutEngine, LayoutRule, NominalDefinition};
use tir_types::{AddressSpace, FloatType, FunctionType, Type};

use crate::gpu_profile::policy::TypePolicies;

mod binary;
use binary::*;

const V12: Code = Code::new(Dialect::Tir, Area::Verifier, 12);

pub(super) fn verify_text_exports(
    module: &TextModule,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Function)
    {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        if !has_exported_linkage(&tokens) {
            continue;
        }
        if function_uses_c_abi(source) {
            diagnostics.push(
                Diagnostic::error(
                    V12,
                    "an exported function cannot use abi(c); exported functions use the RIDL ABI",
                )
                .with_primary_span(item.span),
            );
        }
        if !function_has_header_attribute(source, "nounwind") {
            diagnostics.push(
                Diagnostic::error(V12, "an exported RIDL function must be nounwind")
                    .with_primary_span(item.span),
            );
        }
        let asynchronous = function_has_header_attribute(source, "async");

        let Ok(Some(signature)) = parse_function_signature(source) else {
            continue;
        };
        let mut nonrepresentable = Vec::new();
        for parameter in &signature.parameters {
            if asynchronous
                && matches!(
                    parameter.convention,
                    Some(tir_types::Convention::Borrow | tir_types::Convention::BorrowMut)
                )
                && !policies.is_trivial(&parameter.ty)
            {
                diagnostics.push(
                    Diagnostic::error(
                        V12,
                        format!(
                            "exported async function parameter %{} cannot be borrowed because its future may outlive the borrow",
                            parameter.name
                        ),
                    )
                    .with_primary_span(item.span),
                );
            }
            if !text_type_is_ridl(&parameter.ty, layouts, &mut HashSet::new()) {
                nonrepresentable.push(format!("parameter %{}: {}", parameter.name, parameter.ty));
            }
        }
        if !text_type_is_ridl_result(&signature.result, layouts, &mut HashSet::new()) {
            nonrepresentable.push(format!("result {}", signature.result));
        }
        if let Some(error) = &signature.failure
            && !text_type_is_ridl(error, layouts, &mut HashSet::new())
        {
            nonrepresentable.push(format!("failure result {error}"));
        }
        if !nonrepresentable.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    V12,
                    format!(
                        "exported function has non-RIDL-representable {}",
                        nonrepresentable.join(", ")
                    ),
                )
                .with_primary_span(item.span),
            );
        }
    }
    diagnostics
}

pub(super) fn verify_binary_exports(
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let imported_nominals = imported_nominal_declarations(imports, types);
    for signature in &signatures.records {
        if signature.linkage != BinaryLinkage::Exported {
            continue;
        }
        let Ok(index) = usize::try_from(signature.function_type) else {
            continue;
        };
        let Some(BinaryType::Function(function)) = types.entries.get(index) else {
            continue;
        };
        if function.c_abi {
            diagnostics.push(Diagnostic::error(
                V12,
                format!(
                    "exported function @{} uses the C ABI; exported functions use the RIDL ABI",
                    signature.name
                ),
            ));
        }
        if signature.modifiers & (1 << 42) == 0 {
            diagnostics.push(Diagnostic::error(
                V12,
                format!(
                    "exported RIDL function @{} must be nounwind",
                    signature.name
                ),
            ));
        }
        if function.asynchronous {
            for (index, parameter) in function.parameters.iter().enumerate() {
                if matches!(parameter.convention, 1 | 2)
                    && !crate::binary_code::is_type_trivial(types, parameter.ty)
                {
                    diagnostics.push(Diagnostic::error(
                        V12,
                        format!(
                            "exported async function @{} parameter {index} is borrowed and may outlive its borrow",
                            signature.name
                        ),
                    ));
                }
            }
        }
        let mut nonrepresentable = Vec::new();
        for (index, parameter) in function.parameters.iter().enumerate() {
            if !binary_type_is_ridl(
                parameter.ty,
                types,
                &imported_nominals,
                &mut Vec::new(),
                &mut HashSet::new(),
            ) {
                nonrepresentable.push(format!("parameter {index}"));
            }
        }
        if !binary_type_is_ridl_result(
            function.result,
            types,
            &imported_nominals,
            &mut Vec::new(),
            &mut HashSet::new(),
        ) {
            nonrepresentable.push("result".to_owned());
        }
        if let Some(error) = function.error
            && !binary_type_is_ridl(
                error,
                types,
                &imported_nominals,
                &mut Vec::new(),
                &mut HashSet::new(),
            )
        {
            nonrepresentable.push("failure result".to_owned());
        }
        if !nonrepresentable.is_empty() {
            diagnostics.push(Diagnostic::error(
                V12,
                format!(
                    "exported function @{} has non-RIDL-representable {}",
                    signature.name,
                    nonrepresentable.join(", ")
                ),
            ));
        }
    }
    diagnostics
}

fn imported_nominal_declarations(
    imports: Option<&[BinaryImport]>,
    types: &BinaryTypeTable,
) -> HashSet<u64> {
    imports
        .into_iter()
        .flatten()
        .filter(|import| {
            matches!(
                import.kind,
                BinaryImportKind::Ridl | BinaryImportKind::Package
            )
        })
        .flat_map(|import| &import.entities)
        .filter(|entity| entity.kind == BinaryImportEntityKind::Type)
        .filter_map(|entity| {
            match types
                .entries
                .get(usize::try_from(entity.type_index).ok()?)?
            {
                BinaryType::Nominal { declaration, .. } => Some(*declaration),
                _ => None,
            }
        })
        .collect()
}

fn text_type_is_ridl(ty: &Type, layouts: &LayoutEngine, active: &mut HashSet<String>) -> bool {
    match ty {
        Type::Integer(bits) => ridl_integer(*bits),
        Type::Float(float) => ridl_float(*float),
        Type::Pointer(space) => matches!(
            space,
            AddressSpace::Generic | AddressSpace::Device | AddressSpace::Constant
        ),
        Type::Unit | Type::Never | Type::Code | Type::Address { .. } => false,
        Type::FixedVector { lanes, element } => {
            matches!(*lanes, 2 | 3 | 4 | 8 | 16) && text_vector_element_is_ridl(element)
        }
        Type::ScalableVector(_) | Type::Array { .. } => false,
        Type::Tuple(elements) => elements
            .iter()
            .all(|element| text_type_is_ridl(element, layouts, active)),
        Type::Nominal { path, arguments } => {
            if !arguments
                .iter()
                .all(|argument| text_type_is_ridl(argument, layouts, active))
            {
                return false;
            }
            let Some(definition) = layouts.definition(path) else {
                return layouts.is_imported_nominal(path);
            };
            text_nominal_is_ridl(definition, arguments, layouts, active)
        }
        Type::Generic(_) => true,
        Type::Reference(_) | Type::Unique(_) | Type::Weak(_) => false,
        Type::Function(function) => text_function_is_ridl(function, layouts, active),
        Type::Closure(closure) => text_function_is_ridl(&closure.signature, layouts, active),
        Type::Coroutine { .. } | Type::Frame(_) => false,
        Type::Texture { component, .. } => text_texture_component_is_ridl(component),
        Type::Sampler | Type::AccelerationStructure => true,
        Type::VisibleFunctionTable(function) => text_function_is_ridl(function, layouts, active),
        Type::Tensor { count, element } => (1..=8).contains(count) && text_scalar_is_ridl(element),
        Type::RayQuery | Type::HitObject | Type::CooperativeMatrix { .. } => false,
        Type::Entry {
            root, immediate, ..
        } => {
            text_type_is_ridl_entry_component(root, layouts, active)
                && text_type_is_ridl_entry_component(immediate, layouts, active)
        }
    }
}

fn text_type_is_ridl_result(
    ty: &Type,
    layouts: &LayoutEngine,
    active: &mut HashSet<String>,
) -> bool {
    matches!(ty, Type::Unit) || text_type_is_ridl(ty, layouts, active)
}

fn text_type_is_ridl_entry_component(
    ty: &Type,
    layouts: &LayoutEngine,
    active: &mut HashSet<String>,
) -> bool {
    matches!(ty, Type::Unit) || text_type_is_ridl(ty, layouts, active)
}

fn text_scalar_is_ridl(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => ridl_integer(*bits),
        Type::Float(float) => ridl_float(*float),
        _ => false,
    }
}

fn text_vector_element_is_ridl(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(*bits, 8 | 16 | 32 | 64 | 128),
        Type::Float(float) => ridl_float(*float),
        _ => false,
    }
}

fn text_texture_component_is_ridl(ty: &Type) -> bool {
    text_scalar_is_ridl(ty)
        || matches!(
            ty,
            Type::FixedVector { lanes, element }
                if *lanes == 4 && text_vector_element_is_ridl(element)
        )
}

fn text_function_is_ridl(
    function: &FunctionType,
    layouts: &LayoutEngine,
    active: &mut HashSet<String>,
) -> bool {
    !function.asynchronous
        && function.abi == tir_types::Abi::Ridl
        && !function.foreign
        && function
            .parameters
            .iter()
            .all(|parameter| text_type_is_ridl(&parameter.ty, layouts, active))
        && text_type_is_ridl_result(&function.result, layouts, active)
        && function
            .failure
            .as_deref()
            .is_none_or(|error| text_type_is_ridl(error, layouts, active))
}

fn text_nominal_is_ridl(
    definition: &NominalDefinition,
    arguments: &[Type],
    layouts: &LayoutEngine,
    active: &mut HashSet<String>,
) -> bool {
    if definition.rule != LayoutRule::Ridl
        || definition.opaque
        || definition.parameters.len() != arguments.len()
        || definition.kind == DefinitionKind::Union
        || !active.insert(definition.path.clone())
    {
        return false;
    }
    let bindings: HashMap<_, _> = definition
        .parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect();
    let fields_are_ridl = definition
        .fields
        .iter()
        .chain(
            definition
                .variants
                .iter()
                .flat_map(|variant| &variant.fields),
        )
        .all(|field| {
            field.explicit_offset.is_none()
                && field.explicit_bit_offset.is_none()
                && field.bit_width.is_none()
                && !field.flexible
                && text_type_is_ridl(&field.ty.substitute(&bindings), layouts, active)
        });
    active.remove(&definition.path);
    fields_are_ridl
}

fn ridl_integer(bits: u16) -> bool {
    matches!(bits, 1 | 8 | 16 | 32 | 64 | 128)
}

fn ridl_float(float: FloatType) -> bool {
    matches!(
        float,
        FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64
    )
}
