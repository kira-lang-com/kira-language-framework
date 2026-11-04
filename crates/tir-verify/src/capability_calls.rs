use std::collections::{BTreeSet, HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCapabilities, BinaryCodeTable, BinaryGenericTable, BinaryGlobalTable, BinaryImport,
    BinaryImportKind, BinaryLinkage, BinaryModuleRecord, BinaryOperand, BinarySignatureTable,
    BinaryTarget, BinaryType, BinaryTypeTable,
};
use tir_target::Platform;

use crate::binary_symbols::{imported_data_reference, resolve_function_symbol};

const V20: Code = Code::new(Dialect::Tir, Area::Verifier, 20);

pub(crate) type CallGraphRecords<'a> = (
    Option<&'a BinaryModuleRecord>,
    Option<&'a BinaryCodeTable>,
    Option<&'a BinarySignatureTable>,
    Option<&'a BinaryTypeTable>,
    Option<&'a BinaryGlobalTable>,
    Option<&'a BinaryGenericTable>,
    Option<&'a tir_format::StringTable>,
);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReachedImports {
    pub(crate) libtessera_calls: Vec<tir_format::BinaryImportReference>,
    pub(crate) requires_functions: Vec<tir_format::BinaryImportReference>,
    pub(crate) platform_imports: Vec<tir_format::BinaryImportReference>,
    pub(crate) implied_entitlements: Vec<String>,
}

#[derive(Default)]
struct ReachedImportSets {
    libtessera_calls: BTreeSet<tir_format::BinaryImportReference>,
    requires_functions: BTreeSet<tir_format::BinaryImportReference>,
    platform_imports: BTreeSet<tir_format::BinaryImportReference>,
    implied_entitlements: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FunctionTarget {
    function_type: u64,
    local_index: Option<u64>,
    import_reference: Option<(u64, u64)>,
}

pub(crate) fn verify_reached_imports(
    records: CallGraphRecords<'_>,
    imports: Option<&[BinaryImport]>,
    capabilities: &BinaryCapabilities,
    platform: Platform,
) -> Vec<Diagnostic> {
    let Some(imports) = imports else {
        return Vec::new();
    };
    let expected = compute_reached_imports(records, imports, platform);
    let mut diagnostics = Vec::new();
    if expected.libtessera_calls != capabilities.reached_libtessera_calls {
        diagnostics.push(Diagnostic::error(
            V20,
            "CAPS LibTessera calls do not match the conservative CODE call graph",
        ));
    }
    if expected.requires_functions != capabilities.reached_requires_functions {
        diagnostics.push(Diagnostic::error(
            V20,
            "CAPS @Requires functions do not match the conservative CODE call graph",
        ));
    }
    if expected.platform_imports != capabilities.reached_platform_imports {
        diagnostics.push(Diagnostic::error(
            V20,
            "CAPS platform imports do not match the conservative CODE call graph",
        ));
    }
    if expected.implied_entitlements != capabilities.implied_entitlements {
        diagnostics.push(Diagnostic::error(
            V20,
            "CAPS implied entitlements do not match the conservative CODE call graph",
        ));
    }
    diagnostics
}

pub(crate) fn compute_reached_imports(
    records: CallGraphRecords<'_>,
    imports: &[BinaryImport],
    platform: Platform,
) -> ReachedImports {
    let (Some(module), Some(code), Some(signatures), Some(types), globals, generics, strings) =
        records
    else {
        return ReachedImports::default();
    };
    let reached = reachable_imports(
        module, code, signatures, types, imports, globals, generics, strings, platform,
    );
    ReachedImports {
        libtessera_calls: reached.libtessera_calls.into_iter().collect(),
        requires_functions: reached.requires_functions.into_iter().collect(),
        platform_imports: reached.platform_imports.into_iter().collect(),
        implied_entitlements: reached.implied_entitlements.into_iter().collect(),
    }
}

fn reachable_imports(
    module: &BinaryModuleRecord,
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: &[BinaryImport],
    globals: Option<&BinaryGlobalTable>,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&tir_format::StringTable>,
    platform: Platform,
) -> ReachedImportSets {
    let bodies = code
        .functions
        .iter()
        .filter_map(|body| {
            usize::try_from(body.function_index)
                .ok()
                .map(|index| (index, body))
        })
        .collect::<HashMap<_, _>>();
    let mut reachable = signatures
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, signature)| {
            (signature.linkage != tir_format::BinaryLinkage::Private
                || signature.entry_kind.is_some()
                || signature.platform_export.is_some()
                || signature.inlinable)
                .then_some(index)
        })
        .collect::<HashSet<_>>();
    for initializer in module.initializers.iter().chain(&module.finalizers) {
        if let Ok(index) = usize::try_from(initializer.function_index) {
            reachable.insert(index);
        }
    }
    let mut processed = HashSet::new();
    let mut address_taken = HashSet::new();
    let mut indirect_sites = Vec::new();
    let mut visible_function_sites = Vec::new();
    let mut reached = ReachedImportSets::default();

    if let Some(generics) = generics {
        for function_index in generics
            .vtables
            .iter()
            .flat_map(|vtable| vtable.slots.iter().map(|slot| slot.function_index))
            .chain(generics.conformances.iter().flat_map(|conformance| {
                conformance
                    .witnesses
                    .iter()
                    .map(|witness| witness.function_index)
            }))
        {
            if let Some(signature) = usize::try_from(function_index)
                .ok()
                .and_then(|index| signatures.records.get(index))
            {
                address_taken.insert(FunctionTarget {
                    function_type: signature.function_type,
                    local_index: Some(function_index),
                    import_reference: None,
                });
            }
        }
    }

    loop {
        let pending = reachable
            .difference(&processed)
            .copied()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        for function_index in pending {
            processed.insert(function_index);
            let Some(body) = bodies.get(&function_index).copied() else {
                continue;
            };
            let value_types =
                crate::binary_code::infer_value_types(body, signatures, types, Some(imports));
            for block in &body.blocks {
                let Some(instructions) = &block.instructions else {
                    continue;
                };
                for instruction in instructions {
                    for operand in &instruction.operands {
                        match operand {
                            BinaryOperand::Symbol { kind: 0, index } => {
                                let Some(target) =
                                    resolve_function_symbol(*index, signatures, types, imports)
                                else {
                                    continue;
                                };
                                if is_function_address(instruction.opcode) {
                                    address_taken.insert(FunctionTarget {
                                        function_type: target.function_type_index,
                                        local_index: target.local_index,
                                        import_reference: target.import_reference,
                                    });
                                    record_platform_import(
                                        target.import_reference,
                                        imports,
                                        &mut reached,
                                    );
                                } else if instruction.opcode == 0x282 {
                                    if let Some(index) = target
                                        .local_index
                                        .and_then(|index| usize::try_from(index).ok())
                                    {
                                        reachable.insert(index);
                                    }
                                } else if is_direct_call(instruction.opcode)
                                    || is_fallback_call(instruction.opcode)
                                {
                                    follow_target(
                                        target.local_index,
                                        target.import_reference,
                                        imports,
                                        &mut reachable,
                                        &mut reached,
                                    );
                                }
                            }
                            BinaryOperand::Symbol { kind: 1, index } => {
                                if let Some(reference) = imported_data_reference(*index, imports) {
                                    record_platform_import(Some(reference), imports, &mut reached);
                                }
                            }
                            BinaryOperand::Clauses(clauses) => {
                                for (kind, function_index) in clauses {
                                    if *kind != 1 {
                                        continue;
                                    }
                                    if let Some(target) = resolve_function_symbol(
                                        *function_index,
                                        signatures,
                                        types,
                                        imports,
                                    ) {
                                        follow_target(
                                            target.local_index,
                                            target.import_reference,
                                            imports,
                                            &mut reachable,
                                            &mut reached,
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(function_type) = indirect_function_type(
                        instruction,
                        &value_types,
                        types,
                        signatures,
                        generics,
                        strings,
                        imports,
                        globals,
                    ) {
                        if instruction.opcode == 0x404 {
                            visible_function_sites.push(function_type);
                        } else {
                            indirect_sites.push(function_type);
                        }
                    }
                }
            }
        }

        let previous_count = reachable.len();
        for function_type in &indirect_sites {
            for target in &address_taken {
                if target.function_type == *function_type {
                    follow_target(
                        target.local_index,
                        target.import_reference,
                        imports,
                        &mut reachable,
                        &mut reached,
                    );
                }
            }
        }
        for function_type in &visible_function_sites {
            for (index, signature) in signatures.records.iter().enumerate() {
                if signature.function_type == *function_type
                    && signature.linkage == BinaryLinkage::Exported
                    && signature.target == BinaryTarget::Gpu
                    && signature.entry_kind.is_none()
                {
                    follow_target(
                        Some(index as u64),
                        None,
                        imports,
                        &mut reachable,
                        &mut reached,
                    );
                }
            }
        }
        if reachable.len() == previous_count && reachable.difference(&processed).next().is_none() {
            break;
        }
    }

    if platform == Platform::Tessera {
        reached.platform_imports.clear();
    } else {
        reached.implied_entitlements.clear();
    }
    reached
}

fn follow_target(
    local_index: Option<u64>,
    import_reference: Option<(u64, u64)>,
    imports: &[BinaryImport],
    reachable: &mut HashSet<usize>,
    reached: &mut ReachedImportSets,
) {
    if let Some(index) = local_index.and_then(|index| usize::try_from(index).ok()) {
        reachable.insert(index);
    }
    if let Some(reference) = import_reference {
        let reference = tir_format::BinaryImportReference {
            import_index: reference.0,
            entity_index: reference.1,
        };
        let Some(import) = usize::try_from(reference.import_index)
            .ok()
            .and_then(|index| imports.get(index))
        else {
            return;
        };
        let Some(entity) = usize::try_from(reference.entity_index)
            .ok()
            .and_then(|index| import.entities.get(index))
        else {
            return;
        };
        if import.kind == BinaryImportKind::Platform {
            reached.platform_imports.insert(reference);
        } else if import.kind == BinaryImportKind::Ridl {
            if !entity.required_entitlements.is_empty() {
                reached.requires_functions.insert(reference);
                reached
                    .implied_entitlements
                    .extend(entity.required_entitlements.iter().cloned());
            }
            if crate::platform::is_libtessera_module(&import.name) {
                reached.libtessera_calls.insert(reference);
                reached
                    .implied_entitlements
                    .extend(entity.implied_entitlements.iter().cloned());
            }
        }
    }
}

fn record_platform_import(
    reference: Option<(u64, u64)>,
    imports: &[BinaryImport],
    reached: &mut ReachedImportSets,
) {
    let Some((import_index, entity_index)) = reference else {
        return;
    };
    if usize::try_from(import_index)
        .ok()
        .and_then(|index| imports.get(index))
        .is_some_and(|import| import.kind == BinaryImportKind::Platform)
    {
        reached
            .platform_imports
            .insert(tir_format::BinaryImportReference {
                import_index,
                entity_index,
            });
    }
}

fn is_function_address(opcode: u64) -> bool {
    matches!(opcode, 0x00a | 0x280)
}

fn is_direct_call(opcode: u64) -> bool {
    matches!(opcode, 0x1a0 | 0x1a7..=0x1aa | 0x1e7 | 0x1c0)
}

fn is_fallback_call(opcode: u64) -> bool {
    matches!(opcode, 0x260 | 0x261)
}

fn indirect_function_type(
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&tir_format::StringTable>,
    imports: &[BinaryImport],
    globals: Option<&BinaryGlobalTable>,
) -> Option<u64> {
    match instruction.opcode {
        0x1a1 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => Some(*index),
                _ => None,
            }),
        0x1a3 => first_value(instruction)
            .and_then(|value| {
                value_types
                    .get(usize::try_from(value).ok()?)
                    .copied()
                    .flatten()
            })
            .and_then(|index| types.entries.get(usize::try_from(index).ok()?))
            .and_then(|ty| match ty {
                BinaryType::Closure { function_type, .. } => Some(*function_type),
                _ => None,
            }),
        0x1a5 | 0x1a6 => crate::binary_code::witness_or_vtable_function_type(
            instruction,
            signatures,
            generics,
            strings,
            Some(imports),
            globals,
        ),
        0x404 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => Some(*index),
                _ => None,
            }),
        _ => None,
    }
}

fn first_value(instruction: &tir_format::BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(*value),
            BinaryOperand::Values(values) => values.first().copied(),
            _ => None,
        })
}
