use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_target::Profile;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, DataKind, DataSection,
    FunctionRecordSummary, GpuEntryRecord, ImportCounts, TypeSection, VariantIndex,
};

const VFY10: Code = Code::new(Dialect::Tvisa, Area::Verifier, 10);
const VFY11: Code = Code::new(Dialect::Tvisa, Area::Verifier, 11);
const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);
const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    caller: &FunctionRecordSummary,
    profile: Profile,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
    data: Option<&DataSection>,
    gpu_entries: Option<&[GpuEntryRecord]>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        match instruction.opcode {
            0x00a | 0x00b | 0x00c | 0x010 => {
                if let Some(symbol) = symbol_operand(instruction) {
                    verify_constant_symbol(
                        instruction,
                        symbol,
                        caller,
                        types,
                        functions,
                        imports,
                        data,
                        gpu_entries,
                        &mut diagnostics,
                    );
                }
            }
            0x1a0 | 0x1a9 => verify_direct_call(
                instruction,
                caller,
                functions,
                imports,
                variants,
                types,
                &mut diagnostics,
            ),
            0x1aa | 0x1e7 if (instruction.modifiers >> 48) & 0xf == 0 => {
                verify_direct_call(
                    instruction,
                    caller,
                    functions,
                    imports,
                    variants,
                    types,
                    &mut diagnostics,
                );
            }
            0x273 | 0x280..=0x282 => {
                if let Some(symbol) = symbol_operand(instruction) {
                    verify_platform_symbol(
                        instruction,
                        symbol,
                        caller,
                        profile,
                        types,
                        functions,
                        imports,
                        data,
                        &mut diagnostics,
                    );
                }
            }
            _ => {}
        }
    }
    diagnostics
}

fn verify_platform_symbol(
    instruction: &BinaryInstruction,
    symbol: u64,
    caller: &FunctionRecordSummary,
    profile: Profile,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    data: Option<&DataSection>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let space = symbol & 3;
    let index = symbol >> 2;
    match instruction.opcode {
        0x273 => {
            let defined = index.checked_sub(imports.data).and_then(|index| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| data?.objects.get(index))
            });
            let valid = space == 1
                && profile == Profile::Kernel
                && caller.target == 0
                && defined.is_some_and(|object| {
                    object.per_cpu
                        && !matches!(
                            object.kind,
                            DataKind::Tls | DataKind::TlsZero | DataKind::GpuConstant
                        )
                });
            if !valid {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "kernel.percpu.addr requires a kernel CPU function and a defined per-CPU data symbol",
                ));
            }
        }
        0x280 => {
            let module_kind = usize::try_from(index)
                .ok()
                .and_then(|index| imports.function_module_kinds.get(index));
            let convention = function_signature(index, functions, imports, types)
                .map(|signature| signature.convention);
            let valid = space == 0
                && caller.target == 0
                && module_kind == Some(&2)
                && convention == Some(1);
            if !valid {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "foreign.addr requires an imported platform function with the C ABI in CPU code",
                ));
            }
        }
        0x281 => {
            let platform_data = usize::try_from(index)
                .ok()
                .and_then(|index| imports.data_module_kinds.get(index))
                .is_some_and(|module_kind| *module_kind == 2);
            if space != 1 || caller.target != 0 || index >= imports.data || !platform_data {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "foreign.data requires an imported platform data symbol in CPU code",
                ));
            }
        }
        0x282 => {
            let convention = function_signature(index, functions, imports, types)
                .map(|signature| signature.convention);
            let function = function_record(index, functions, imports);
            let valid = space == 0
                && caller.target == 0
                && convention == Some(1)
                && function.is_some_and(|function| function.target == 0);
            if !valid {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "foreign.entry requires a defined CPU function with the C ABI",
                ));
            }
        }
        _ => {}
    }
}

fn verify_constant_symbol(
    instruction: &BinaryInstruction,
    symbol: u64,
    caller: &FunctionRecordSummary,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    data: Option<&DataSection>,
    gpu_entries: Option<&[GpuEntryRecord]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let space = symbol & 3;
    let index = symbol >> 2;
    match instruction.opcode {
        0x00a => {
            if space != 0 {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "func.ref must use the function symbol space",
                ));
                return;
            }
            let address_taken = if index < imports.functions {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| imports.function_tags.get(index))
                    .is_some()
                    && function_signature(index, functions, imports, types)
                        .is_some_and(|signature| signature.convention <= 2)
            } else {
                function_record(index, functions, imports)
                    .is_some_and(|function| function.target == 0 && function.address_taken())
            };
            if caller.target != 0 || !address_taken {
                diagnostics.push(Diagnostic::error(
                    VFY11,
                    "func.ref requires an address_taken CPU function and is unavailable in GPU code",
                ));
            }
        }
        0x00b | 0x00c => {
            if space != 1 {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    format!(
                        "{} must use the data-object symbol space",
                        if instruction.opcode == 0x00b {
                            "global.addr"
                        } else {
                            "tls.addr"
                        }
                    ),
                ));
                return;
            }
            let data = data.map_or(&[][..], |data| data.objects.as_slice());
            let data_limit = imports.data.saturating_add(data.len() as u64);
            if index >= data_limit {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    format!("data symbol index {index} is out of range"),
                ));
                return;
            }
            if instruction.opcode == 0x00c {
                let tls = caller.target == 0
                    && index >= imports.data
                    && usize::try_from(index - imports.data)
                        .ok()
                        .and_then(|index| data.get(index))
                        .is_some_and(|object| {
                            matches!(object.kind, DataKind::Tls | DataKind::TlsZero)
                        });
                if !tls {
                    diagnostics.push(Diagnostic::error(
                        VFY10,
                        "tls.addr must name a defined TLS object in CPU code",
                    ));
                }
            } else if caller.target != 0 {
                let gpu_constant = index >= imports.data
                    && usize::try_from(index - imports.data)
                        .ok()
                        .and_then(|index| data.get(index))
                        .is_some_and(|object| object.kind == DataKind::GpuConstant);
                if !gpu_constant {
                    diagnostics.push(Diagnostic::error(
                        VFY21,
                        "GPU global.addr must name an initialized GPU constant object",
                    ));
                }
            }
        }
        0x010 => {
            if space != 0 {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    "gpu.entry.ref must use the function symbol space",
                ));
                return;
            }
            let is_entry = index >= imports.functions
                && function_record(index, functions, imports)
                    .is_some_and(|function| matches!(function.target, 1 | 2))
                && gpu_entries.is_some_and(|entries| {
                    entries.iter().any(|entry| entry.function_index == index)
                });
            if caller.target != 0 || !is_entry {
                diagnostics.push(Diagnostic::error(
                    VFY22,
                    "gpu.entry.ref must name an image GPU entry and is only valid in CPU code",
                ));
            }
        }
        _ => {}
    }
}

fn verify_direct_call(
    instruction: &BinaryInstruction,
    caller: &FunctionRecordSummary,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(symbol) = symbol_operand(instruction) else {
        return;
    };
    let index = symbol >> 2;
    match symbol & 3 {
        0 => {
            let convention = function_signature(index, functions, imports, types)
                .map(|signature| signature.convention);
            let target = if index < imports.functions {
                convention.map(|convention| match convention {
                    0..=2 => 0,
                    3 => 1,
                    4 => 2,
                    _ => u8::MAX,
                })
            } else {
                function_record(index, functions, imports).map(|function| function.target)
            };
            let valid = match caller.target {
                0 => target == Some(0),
                1 | 2 => target == Some(1) && convention == Some(3),
                _ => false,
            };
            if !valid {
                diagnostics.push(Diagnostic::error(
                    VFY22,
                    "direct call target is not callable from this CPU/GPU function",
                ));
            }
        }
        2 => {
            let valid = caller.target == 0
                && variants
                    .sets
                    .get(usize::try_from(index).unwrap_or(usize::MAX))
                    .is_some_and(|set| {
                        set.members.iter().all(|member| {
                            function_record(member.function_index, functions, imports)
                                .is_some_and(|function| function.target == 0)
                        })
                    });
            if !valid {
                diagnostics.push(Diagnostic::error(
                    VFY22,
                    "variant and dispatch calls are only valid from CPU functions",
                ));
            }
        }
        _ => diagnostics.push(Diagnostic::error(
            VFY10,
            "direct call uses a non-callable symbol space",
        )),
    }
}

fn symbol_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol(symbol) => Some(*symbol),
            _ => None,
        })
}

fn function_record<'a>(
    index: u64,
    functions: &'a [FunctionRecordSummary],
    imports: &ImportCounts,
) -> Option<&'a FunctionRecordSummary> {
    let index = index.checked_sub(imports.functions)?;
    usize::try_from(index)
        .ok()
        .and_then(|index| functions.get(index))
}

fn function_signature<'a>(
    index: u64,
    functions: &'a [FunctionRecordSummary],
    imports: &'a ImportCounts,
    types: &'a TypeSection,
) -> Option<&'a tvisa_format::SignatureSummary> {
    let signature_index = if index < imports.functions {
        imports
            .function_signatures
            .get(usize::try_from(index).ok()?)
            .copied()?
    } else {
        function_record(index, functions, imports)?.signature_index
    };
    types.signatures.get(usize::try_from(signature_index).ok()?)
}
