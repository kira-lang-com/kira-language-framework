mod atomics;
#[path = "binary_structured.rs"]
mod binary_structured;
mod calls;
mod gpu_collectives;
mod gpu_control;
mod infer;
mod instruction_types;
mod memory;
mod semantics;
mod slots;
mod symbol_operands;
mod tv_only;
mod unwind;
mod vectors;

use infer::infer_value_types;
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_verify::DominatorTree;
use tvisa_format::{
    AssemblyRecord, BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary,
    ImportCounts, ParameterItemSummary, TvisaImage, TvisaValueTypeRecord, TypeSection,
    VariantIndex,
};

const VFY1: Code = Code::new(Dialect::Tvisa, Area::Verifier, 1);
const VFY4: Code = Code::new(Dialect::Tvisa, Area::Verifier, 4);
const VFY50: Code = Code::new(Dialect::Tvisa, Area::Verifier, 50);
const ARCH3: Code = Code::new(Dialect::Tvisa, Area::Architecture, 3);
const HAS_ASSEMBLY: u32 = 1 << 1;
const FUNCTION_HAS_ASSEMBLY: u64 = 1 << 8;

pub(super) fn gpu_value_types(
    body: &BinaryFunctionBody,
    function: &FunctionRecordSummary,
    types: &TypeSection,
    functions: &[FunctionRecordSummary],
    imports: &ImportCounts,
    variants: &VariantIndex,
) -> Vec<Option<u64>> {
    infer_value_types(body, function, types, functions, imports, variants)
}

pub(super) fn gpu_atomic_capabilities(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> std::collections::HashSet<(u8, &'static str)> {
    atomics::gpu_capability_requirements(body, value_types, types)
}

pub(super) fn verify(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let bodies = match image.function_bodies(memory_limit) {
        Ok(bodies) => bodies,
        Err(error) => return vec![error.diagnostic],
    };
    let types = match image.type_section(memory_limit) {
        Ok(types) => types,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    let variants = match image.variant_index(memory_limit) {
        Ok(variants) => variants,
        Err(error) => return vec![error.diagnostic],
    };
    let unwind_section = match image.unwind_section(memory_limit) {
        Ok(section) => section,
        Err(error) => return vec![error.diagnostic],
    };
    let constants = match image.constants(memory_limit) {
        Ok(constants) => constants,
        Err(error) => return vec![error.diagnostic],
    };
    let strings = match image.string_table(memory_limit) {
        Ok(strings) => strings,
        Err(error) => return vec![error.diagnostic],
    };
    let assembly_records = match image.assembly_records(memory_limit) {
        Ok(records) => records,
        Err(error) => return vec![error.diagnostic],
    };
    let analysis = match image.analysis_summary(memory_limit) {
        Ok(analysis) => analysis,
        Err(error) => return vec![error.diagnostic],
    };
    let data = image.data_section(memory_limit).ok();
    let gpu_entries = image
        .sections
        .iter()
        .any(|section| section.kind == 11)
        .then(|| image.gpu_entry_records(memory_limit).ok())
        .flatten();
    let mut diagnostics = unwind::verify_section(&unwind_section, &functions, &imports);
    diagnostics.extend(crate::binary_analysis::verify(
        image,
        &bodies,
        &functions,
        &imports,
        &types,
        &analysis,
        data.as_ref(),
    ));
    verify_assembly_inventory(
        image,
        &bodies,
        &functions,
        &imports,
        &assembly_records,
        &strings,
        &mut diagnostics,
    );
    for body in &bodies {
        let Some(function_index) = body.header.function_index.checked_sub(imports.functions) else {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!(
                    "CODE function index {} names an import",
                    body.header.function_index
                ),
            ));
            diagnostics.extend(verify_body(body, &[], false));
            continue;
        };
        let Some(function) = usize::try_from(function_index)
            .ok()
            .and_then(|index| functions.get(index))
        else {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!(
                    "CODE function index {} has no SIGNATURES record",
                    body.header.function_index
                ),
            ));
            diagnostics.extend(verify_body(body, &[], false));
            continue;
        };
        let value_types =
            infer_value_types(body, function, &types, &functions, &imports, &variants);
        diagnostics.extend(verify_body(body, &value_types, function.target != 0));
        diagnostics.extend(instruction_types::verify_function(
            body,
            &value_types,
            &types,
        ));
        diagnostics.extend(tv_only::verify_function(body, &value_types, &types));
        diagnostics.extend(crate::type_tags::verify_binary_function(body, &types));
        diagnostics.extend(vectors::verify_function(body, &value_types, &types));
        diagnostics.extend(atomics::verify_function(
            body,
            &value_types,
            &types,
            function.target != 0,
        ));
        diagnostics.extend(memory::verify_function(body, &value_types, &types));
        let Some(signature) = usize::try_from(function.signature_index)
            .ok()
            .and_then(|index| types.signatures.get(index))
        else {
            continue;
        };
        verify_entry_parameters(
            body,
            signature.parameters.as_slice(),
            &types,
            &mut diagnostics,
        );
        diagnostics.extend(calls::verify_function(
            body,
            function.signature_index,
            &value_types,
            &types,
            &functions,
            &imports,
            &variants,
        ));
        diagnostics.extend(slots::verify_function(
            body,
            function,
            function.semantics == 0,
            &value_types,
            &types,
        ));
        diagnostics.extend(semantics::verify_function(
            body,
            function,
            &types,
            &value_types,
            image.header.profile,
        ));
        diagnostics.extend(symbol_operands::verify_function(
            body,
            function,
            image.header.profile,
            &types,
            &functions,
            &imports,
            &variants,
            data.as_ref(),
            gpu_entries.as_deref(),
        ));
        diagnostics.extend(gpu_control::verify_function(
            body,
            &types,
            &value_types,
            &strings,
            &constants,
        ));
        diagnostics.extend(gpu_collectives::verify_function(body, &types, &value_types));
        let unwind_record = unwind_section
            .functions
            .iter()
            .find(|record| record.function_index == body.header.function_index);
        diagnostics.extend(unwind::verify_function(
            body,
            function,
            unwind_record,
            &types,
            &functions,
            &imports,
        ));
    }
    diagnostics
}

fn verify_assembly_inventory(
    image: &TvisaImage,
    bodies: &[BinaryFunctionBody],
    functions: &[tvisa_format::FunctionRecordSummary],
    imports: &tvisa_format::ImportCounts,
    assembly_records: &[AssemblyRecord],
    strings: &tir_format::StringTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut image_has_assembly = false;
    for body in bodies {
        let has_assembly = body.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|instruction| matches!(instruction.opcode, 0x220 | 0x221))
        });
        image_has_assembly |= has_assembly;
        let Some(function_index) = body.header.function_index.checked_sub(imports.functions) else {
            continue;
        };
        let Some(function) = usize::try_from(function_index)
            .ok()
            .and_then(|index| functions.get(index))
        else {
            continue;
        };
        if (function.attributes & FUNCTION_HAS_ASSEMBLY != 0) != has_assembly {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "function @{} assembly attribute disagrees with CODE",
                    function.name
                ),
            ));
        }
        if has_assembly
            && (function.target != 0
                || !function
                    .variant
                    .as_ref()
                    .is_some_and(|variant| (1..=3).contains(&variant.architecture)))
        {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "assembly in @{} requires an aarch64, x64 or rv64 variant body",
                    function.name
                ),
            ));
        }
        if let Some(architecture) = function.variant.as_ref().and_then(|variant| {
            crate::assembly_constraints::AssemblyArchitecture::from_code(variant.architecture)
        }) {
            for instruction in body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter(|instruction| matches!(instruction.opcode, 0x220 | 0x221))
            {
                if let Err(message) = crate::assembly_constraints::verify_binary_instruction(
                    instruction,
                    architecture,
                    assembly_records,
                    strings,
                ) {
                    diagnostics.push(Diagnostic::error(
                        ARCH3,
                        format!(
                            "invalid assembly constraints in @{}: {message}",
                            function.name
                        ),
                    ));
                }
            }
        }
        if image.header.profile != tir_target::Profile::Kernel
            && body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| {
                    instruction.opcode == 0x220 && instruction.modifiers & (1 << 40) != 0
                })
        {
            diagnostics.push(Diagnostic::error(
                VFY50,
                format!(
                    "privileged assembly is only valid in kernel function @{}",
                    function.name
                ),
            ));
        }
    }
    if (image.header.flags & HAS_ASSEMBLY != 0) != image_has_assembly {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "TVISA header assembly flag disagrees with CODE",
        ));
    }
}

fn verify_body(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    gpu: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let successors = body
        .blocks
        .iter()
        .map(|block| {
            let mut targets = Vec::new();
            if let Some(instruction) = block.instructions.last() {
                visit_edges(instruction, |target, _, _| {
                    if let Ok(target) = usize::try_from(target) {
                        targets.push(target);
                    }
                });
            }
            targets
        })
        .collect::<Vec<_>>();
    let dominators = DominatorTree::compute(&successors);
    verify_reverse_postorder(body, &successors, &dominators, &mut diagnostics);
    if gpu {
        diagnostics.extend(binary_structured::verify(body, &successors));
    }

    for block in &body.blocks {
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        visit_edges(terminator, |target, arguments, implicit_results| {
            let Some(target_index) = usize::try_from(target)
                .ok()
                .and_then(|index| body.blocks.get(index))
            else {
                return;
            };
            let actual_count = arguments.len().saturating_add(implicit_results as usize);
            if actual_count != target_index.parameter_types.len() {
                diagnostics.push(Diagnostic::error(
                    VFY1,
                    format!(
                        "edge to block {target} passes {actual_count} values, expected {}",
                        target_index.parameter_types.len()
                    ),
                ));
                return;
            }
            for (argument, expected) in arguments.iter().zip(
                target_index
                    .parameter_types
                    .iter()
                    .skip(implicit_results as usize),
            ) {
                if let Some(actual) = usize::try_from(*argument)
                    .ok()
                    .and_then(|index| value_types.get(index))
                    .copied()
                    .flatten()
                    && actual != *expected
                {
                    diagnostics.push(Diagnostic::error(
                        VFY1,
                        format!("edge to block {target} passes type {actual}, expected {expected}"),
                    ));
                }
            }
        });
    }

    verify_value_dominance(body, &dominators, &mut diagnostics);
    diagnostics
}

fn verify_entry_parameters(
    body: &BinaryFunctionBody,
    parameters: &[ParameterItemSummary],
    types: &TypeSection,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(entry) = body.blocks.first() else {
        return;
    };
    let mut expected = Vec::new();
    for parameter in parameters {
        match parameter {
            ParameterItemSummary::Leaf { type_index } => expected.push(Some(*type_index)),
            ParameterItemSummary::Group(group) => {
                expected.extend(group.members.iter().map(|member| Some(member.type_index)))
            }
            ParameterItemSummary::ByRef { .. }
            | ParameterItemSummary::SRet { .. }
            | ParameterItemSummary::Context
            | ParameterItemSummary::CAggregate { .. } => expected.push(None),
        }
    }
    if expected.len() != entry.parameter_types.len() {
        diagnostics.push(Diagnostic::error(
            VFY1,
            format!(
                "entry block has {} parameters, signature has {} leaves",
                entry.parameter_types.len(),
                expected.len()
            ),
        ));
        return;
    }
    for (index, (actual, expected)) in entry.parameter_types.iter().zip(expected).enumerate() {
        let matches = expected.map_or_else(
            || {
                matches!(
                    types.value_types.get(*actual as usize),
                    Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
                )
            },
            |expected| *actual == expected,
        );
        if !matches {
            diagnostics.push(Diagnostic::error(
                VFY1,
                format!(
                    "entry parameter {index} has type {actual}, expected {}",
                    expected.map_or_else(|| "pointer".to_owned(), |ty| ty.to_string())
                ),
            ));
        }
    }
}

fn verify_reverse_postorder(
    body: &BinaryFunctionBody,
    successors: &[Vec<usize>],
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let ordered_successors = successors
        .iter()
        .map(|targets| {
            let mut targets = targets.clone();
            targets.sort_unstable();
            targets.dedup();
            targets
        })
        .collect::<Vec<_>>();
    let mut visited = vec![false; body.blocks.len()];
    let mut order = Vec::with_capacity(body.blocks.len());
    for &root in dominators.roots() {
        let mut postorder = Vec::new();
        visit_postorder(
            root,
            &ordered_successors,
            dominators.components(),
            &mut visited,
            &mut postorder,
        );
        order.extend(postorder.into_iter().rev());
    }
    if let Some((expected, actual)) = order
        .iter()
        .copied()
        .enumerate()
        .find(|(expected, actual)| expected != actual)
    {
        diagnostics.push(Diagnostic::error(
            VFY1,
            format!(
                "blocks are not in reverse post-order: block {actual} appears at position {expected}, expected block {expected}"
            ),
        ));
    }
}

fn visit_postorder(
    root: usize,
    successors: &[Vec<usize>],
    components: &[usize],
    visited: &mut [bool],
    postorder: &mut Vec<usize>,
) {
    visited[root] = true;
    let mut stack = vec![(root, 0usize)];
    while let Some((block, next_successor)) = stack.last().copied() {
        let next = successors[block]
            .get(next_successor)
            .copied()
            .filter(|target| components[*target] == components[block]);
        if let Some(target) = next {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            if !visited[target] {
                visited[target] = true;
                stack.push((target, 0));
            }
        } else if next_successor < successors[block].len() {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
        } else {
            postorder.push(block);
            stack.pop();
        }
    }
}

fn verify_value_dominance(
    body: &BinaryFunctionBody,
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut definitions = Vec::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        definitions.extend(std::iter::repeat_n(
            (block_index, 0usize),
            block.parameter_types.len(),
        ));
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            definitions.extend(std::iter::repeat_n(
                (block_index, instruction_index + 1),
                usize::from(instruction.result_count),
            ));
        }
    }
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            verify_instruction_uses(
                instruction,
                block_index,
                instruction_index + 1,
                &definitions,
                dominators,
                diagnostics,
            );
        }
    }
}

fn verify_instruction_uses(
    instruction: &BinaryInstruction,
    block_index: usize,
    use_position: usize,
    definitions: &[(usize, usize)],
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    visit_values(&instruction.operands, &mut |value| {
        let Some((definition_block, definition_position)) = usize::try_from(value)
            .ok()
            .and_then(|index| definitions.get(index))
            .copied()
        else {
            return;
        };
        let dominates = if definition_block == block_index {
            definition_position < use_position
        } else {
            dominators.dominates(definition_block, block_index)
        };
        if !dominates {
            diagnostics.push(Diagnostic::error(
                VFY4,
                format!(
                    "definition of value {value} does not dominate its use in block {block_index}"
                ),
            ));
        }
    });
}

fn visit_values(operands: &[BinaryOperand], visit: &mut impl FnMut(u64)) {
    for operand in operands {
        match operand {
            BinaryOperand::Value(value) => visit(*value),
            BinaryOperand::Values(values) => {
                for value in values {
                    visit(*value);
                }
            }
            BinaryOperand::BlockTarget { arguments, .. } => {
                for value in arguments {
                    visit(*value);
                }
            }
            BinaryOperand::BlockTargets(targets) => {
                for (_, arguments) in targets {
                    for value in arguments {
                        visit(*value);
                    }
                }
            }
            BinaryOperand::SwitchCases(cases) => {
                for (_, _, arguments) in cases {
                    for value in arguments {
                        visit(*value);
                    }
                }
            }
            BinaryOperand::Type(_)
            | BinaryOperand::Index(_)
            | BinaryOperand::Integer(_)
            | BinaryOperand::IntegerBits(_)
            | BinaryOperand::TypeTag(_)
            | BinaryOperand::Symbol(_)
            | BinaryOperand::Bytes(_)
            | BinaryOperand::Integers(_)
            | BinaryOperand::Block(_)
            | BinaryOperand::Blocks(_)
            | BinaryOperand::Clauses(_) => {}
        }
    }
}

fn visit_edges(instruction: &BinaryInstruction, mut visit: impl FnMut(u64, &[u64], u8)) {
    match instruction.opcode {
        0x1e0 => {
            for operand in &instruction.operands {
                if let BinaryOperand::BlockTarget { block, arguments } = operand {
                    visit(*block, arguments, 0);
                }
            }
        }
        0x1e1 => {
            let mut targets = instruction
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::BlockTarget { block, arguments } => {
                        Some((*block, arguments.as_slice()))
                    }
                    _ => None,
                });
            for (block, arguments) in targets.by_ref().take(2) {
                visit(block, arguments, 0);
            }
        }
        0x1e2 => {
            for operand in &instruction.operands {
                match operand {
                    BinaryOperand::SwitchCases(cases) => {
                        for (_, block, arguments) in cases {
                            visit(*block, arguments, 0);
                        }
                    }
                    BinaryOperand::BlockTarget { block, arguments } => {
                        visit(*block, arguments, 0);
                        break;
                    }
                    _ => {}
                }
            }
        }
        0x1e4 | 0x221 => {
            for operand in &instruction.operands {
                if let BinaryOperand::Blocks(blocks) = operand {
                    for block in blocks {
                        visit(*block, &[], 0);
                    }
                }
            }
        }
        0x1aa => {
            let mut targets = instruction
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::BlockTarget { block, arguments } => {
                        Some((*block, arguments.as_slice()))
                    }
                    _ => None,
                });
            if let Some((block, arguments)) = targets.next() {
                visit(block, arguments, instruction.normal_edge_result_count);
            }
            if let Some((block, arguments)) = targets.next() {
                visit(block, arguments, 0);
            }
        }
        _ => {}
    }
}
