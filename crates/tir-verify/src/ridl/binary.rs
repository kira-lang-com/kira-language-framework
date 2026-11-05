use super::*;

pub(super) fn binary_type_is_ridl(
    index: u64,
    types: &BinaryTypeTable,
    imported_nominals: &HashSet<u64>,
    substitutions: &mut Vec<HashMap<u64, u64>>,
    active_nominals: &mut HashSet<u64>,
) -> bool {
    match types
        .entries
        .get(usize::try_from(index).unwrap_or(usize::MAX))
    {
        Some(BinaryType::Integer(bits)) => ridl_binary_integer(*bits),
        Some(BinaryType::Float(float)) => ridl_float(*float),
        Some(BinaryType::Pointer(space)) => matches!(*space, 0..=2),
        Some(BinaryType::Unit | BinaryType::Never | BinaryType::Address { .. }) | None => false,
        Some(BinaryType::FixedVector { lanes, element }) => {
            matches!(*lanes, 2 | 3 | 4 | 8 | 16)
                && binary_vector_element_is_ridl(*element, types, substitutions)
        }
        Some(BinaryType::ScalableVector { .. } | BinaryType::Array { .. }) => false,
        Some(BinaryType::Tuple(elements)) => elements.iter().all(|element| {
            binary_type_is_ridl(
                *element,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            )
        }),
        Some(BinaryType::Nominal {
            declaration,
            arguments,
        }) => {
            if !arguments.iter().all(|argument| {
                binary_type_is_ridl(
                    *argument,
                    types,
                    imported_nominals,
                    substitutions,
                    active_nominals,
                )
            }) {
                return false;
            }
            let Some(definition) = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
            else {
                // Imported nominal layouts are checked once the referenced RIDL
                // modules are available to the linker.
                return imported_nominals.contains(declaration);
            };
            let Some(declaration_record) = types
                .declarations
                .get(usize::try_from(*declaration).unwrap_or(usize::MAX))
            else {
                return false;
            };
            if definition.layout != BinaryLayout::Ridl
                || declaration_record.kind == tir_format::BinaryNominalKind::Explicit
                || declaration_record.generic_parameters.len() != arguments.len()
                || !active_nominals.insert(*declaration)
            {
                return false;
            }
            let local_substitutions: HashMap<_, _> = arguments
                .iter()
                .enumerate()
                .map(|(parameter, argument)| (parameter as u64, *argument))
                .collect();
            substitutions.push(local_substitutions);
            let fields_are_ridl = definition.fields.iter().all(|field| {
                field.offset.is_none()
                    && field.bit_offset.is_none()
                    && field.bit_width.is_none()
                    && !field.annotations.iter().any(|annotation| {
                        annotation.name == "flexible" && annotation.arguments.is_empty()
                    })
                    && binary_type_is_ridl(
                        field.ty,
                        types,
                        imported_nominals,
                        substitutions,
                        active_nominals,
                    )
            }) && definition
                .variants
                .iter()
                .flat_map(|variant| &variant.payload)
                .all(|field| {
                    binary_type_is_ridl(
                        *field,
                        types,
                        imported_nominals,
                        substitutions,
                        active_nominals,
                    )
                });
            substitutions.pop();
            active_nominals.remove(declaration);
            fields_are_ridl
        }
        Some(BinaryType::Generic { parameter }) => {
            let Some(replacement) = substitutions
                .last()
                .and_then(|local| local.get(parameter))
                .copied()
            else {
                return true;
            };
            let Some(local) = substitutions.pop() else {
                return false;
            };
            let result = binary_type_is_ridl(
                replacement,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            );
            substitutions.push(local);
            result
        }
        Some(
            BinaryType::Reference { .. } | BinaryType::Unique { .. } | BinaryType::Weak { .. },
        ) => false,
        Some(BinaryType::Function(function)) => binary_function_is_ridl(
            function,
            types,
            imported_nominals,
            substitutions,
            active_nominals,
        ),
        Some(BinaryType::Closure { function_type, .. })
        | Some(BinaryType::VisibleFunctionTable { function_type }) => {
            let Some(BinaryType::Function(function)) = types
                .entries
                .get(usize::try_from(*function_type).unwrap_or(usize::MAX))
            else {
                return false;
            };
            binary_function_is_ridl(
                function,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            )
        }
        Some(BinaryType::Coroutine { .. } | BinaryType::Frame { .. }) => false,
        Some(BinaryType::Texture { component, .. }) => {
            binary_texture_component_is_ridl(*component, types, substitutions)
        }
        Some(BinaryType::Sampler | BinaryType::AccelerationStructure) => true,
        Some(BinaryType::Tensor { rank, element }) => {
            (1..=8).contains(rank) && binary_scalar_is_ridl(*element, types, substitutions)
        }
        Some(BinaryType::Entry {
            root, immediate, ..
        }) => {
            binary_type_is_ridl_entry_component(
                *root,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            ) && binary_type_is_ridl_entry_component(
                *immediate,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            )
        }
        Some(
            BinaryType::RayQuery | BinaryType::HitObject | BinaryType::CooperativeMatrix { .. },
        ) => false,
    }
}

pub(super) fn binary_type_is_ridl_result(
    index: u64,
    types: &BinaryTypeTable,
    imported_nominals: &HashSet<u64>,
    substitutions: &mut Vec<HashMap<u64, u64>>,
    active_nominals: &mut HashSet<u64>,
) -> bool {
    matches!(
        types
            .entries
            .get(usize::try_from(index).unwrap_or(usize::MAX)),
        Some(BinaryType::Unit)
    ) || binary_type_is_ridl(
        index,
        types,
        imported_nominals,
        substitutions,
        active_nominals,
    )
}

pub(super) fn binary_type_is_ridl_entry_component(
    index: u64,
    types: &BinaryTypeTable,
    imported_nominals: &HashSet<u64>,
    substitutions: &mut Vec<HashMap<u64, u64>>,
    active_nominals: &mut HashSet<u64>,
) -> bool {
    binary_type_is_ridl_result(
        index,
        types,
        imported_nominals,
        substitutions,
        active_nominals,
    )
}

pub(super) fn binary_scalar_is_ridl(
    index: u64,
    types: &BinaryTypeTable,
    substitutions: &mut Vec<HashMap<u64, u64>>,
) -> bool {
    match types
        .entries
        .get(usize::try_from(index).unwrap_or(usize::MAX))
    {
        Some(BinaryType::Integer(bits)) => ridl_binary_integer(*bits),
        Some(BinaryType::Float(float)) => ridl_float(*float),
        Some(BinaryType::Generic { parameter }) => {
            let Some(replacement) = substitutions
                .last()
                .and_then(|local| local.get(parameter))
                .copied()
            else {
                return true;
            };
            let Some(local) = substitutions.pop() else {
                return false;
            };
            let result = binary_scalar_is_ridl(replacement, types, substitutions);
            substitutions.push(local);
            result
        }
        _ => false,
    }
}

pub(super) fn binary_vector_element_is_ridl(
    index: u64,
    types: &BinaryTypeTable,
    substitutions: &mut Vec<HashMap<u64, u64>>,
) -> bool {
    match types
        .entries
        .get(usize::try_from(index).unwrap_or(usize::MAX))
    {
        Some(BinaryType::Integer(bits)) => matches!(*bits, 8 | 16 | 32 | 64 | 128),
        Some(BinaryType::Float(float)) => ridl_float(*float),
        Some(BinaryType::Generic { parameter }) => {
            let Some(replacement) = substitutions
                .last()
                .and_then(|local| local.get(parameter))
                .copied()
            else {
                return true;
            };
            let Some(local) = substitutions.pop() else {
                return false;
            };
            let result = binary_vector_element_is_ridl(replacement, types, substitutions);
            substitutions.push(local);
            result
        }
        _ => false,
    }
}

pub(super) fn binary_texture_component_is_ridl(
    index: u64,
    types: &BinaryTypeTable,
    substitutions: &mut Vec<HashMap<u64, u64>>,
) -> bool {
    binary_scalar_is_ridl(index, types, substitutions)
        || match types
            .entries
            .get(usize::try_from(index).unwrap_or(usize::MAX))
        {
            Some(BinaryType::FixedVector { lanes, element }) => {
                *lanes == 4 && binary_vector_element_is_ridl(*element, types, substitutions)
            }
            _ => false,
        }
}

pub(super) fn binary_function_is_ridl(
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    imported_nominals: &HashSet<u64>,
    substitutions: &mut Vec<HashMap<u64, u64>>,
    active_nominals: &mut HashSet<u64>,
) -> bool {
    !function.asynchronous
        && !function.c_abi
        && !function.foreign
        && function.parameters.iter().all(|parameter| {
            binary_type_is_ridl(
                parameter.ty,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            )
        })
        && binary_type_is_ridl_result(
            function.result,
            types,
            imported_nominals,
            substitutions,
            active_nominals,
        )
        && function.error.is_none_or(|error| {
            binary_type_is_ridl(
                error,
                types,
                imported_nominals,
                substitutions,
                active_nominals,
            )
        })
}

pub(super) fn ridl_binary_integer(bits: u64) -> bool {
    matches!(bits, 1 | 8 | 16 | 32 | 64 | 128)
}

pub(super) fn has_exported_linkage(tokens: &[Token]) -> bool {
    tokens
        .iter()
        .take_while(|token| !matches!(token.kind, TokenKind::Punctuation('@')))
        .any(|token| atom(token) == Some("exported"))
}

pub(super) fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}
