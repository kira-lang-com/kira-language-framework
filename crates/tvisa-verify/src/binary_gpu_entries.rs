use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    GpuEntryKind, ParameterItemSummary, ResultItemSummary, TvisaImage, TvisaValueTypeRecord,
    TypeSection,
};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub(super) fn verify(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let constants = match image.constants(memory_limit) {
        Ok(constants) => constants,
        Err(error) => return vec![error.diagnostic],
    };
    if !image.sections.iter().any(|section| section.kind == 11) {
        return Vec::new();
    }
    let entries = match image.gpu_entry_records(memory_limit) {
        Ok(entries) => entries,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let types = match image.type_section(memory_limit) {
        Ok(types) => types,
        Err(error) => return vec![error.diagnostic],
    };
    let mut diagnostics = Vec::new();
    let mut recorded = HashSet::new();
    for entry in &entries {
        recorded.insert(entry.function_index);
        let Some(local_index) = entry.function_index.checked_sub(imports.functions) else {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function index {} names an imported function",
                    entry.function_index
                ),
            ));
            continue;
        };
        let Some(function) = usize::try_from(local_index)
            .ok()
            .and_then(|index| functions.get(index))
        else {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function index {} is out of range",
                    entry.function_index
                ),
            ));
            continue;
        };
        let target_matches = match entry.kind {
            GpuEntryKind::Visible => function.target == 1 && function.linkage == 2,
            _ => function.target == 2,
        };
        if !target_matches {
            diagnostics.push(Diagnostic::error(
                VFY22,
                if entry.kind == GpuEntryKind::Visible {
                    format!(
                        "visible GPU record for @{} requires an exported target-1 function",
                        function.name
                    )
                } else {
                    format!(
                        "GPU {:?} record for @{} disagrees with its function target {}",
                        entry.kind, function.name, function.target
                    )
                },
            ));
        }
        verify_entry_signature(entry, function, &types, &constants, &mut diagnostics);
    }
    for (index, function) in functions.iter().enumerate() {
        if function.target == 2 {
            let function_index = imports.functions + index as u64;
            if !recorded.contains(&function_index) {
                diagnostics.push(Diagnostic::error(
                    VFY22,
                    format!(
                        "GPU entry function @{} has no GPU entry record",
                        function.name
                    ),
                ));
            }
        }
    }
    diagnostics
}

fn verify_entry_signature(
    entry: &tvisa_format::GpuEntryRecord,
    function: &tvisa_format::FunctionRecordSummary,
    types: &TypeSection,
    constants: &[tvisa_format::ConstantRecord],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(signature) = usize::try_from(function.signature_index)
        .ok()
        .and_then(|index| types.signatures.get(index))
    else {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU entry function @{} has an invalid signature index",
                function.name
            ),
        ));
        return;
    };

    let expected_convention = if entry.kind == GpuEntryKind::Visible {
        3 // gpu
    } else {
        4 // entry
    };
    if signature.convention != expected_convention {
        let convention = if expected_convention == 3 {
            "gpu"
        } else {
            "entry"
        };
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU {:?} record for @{} requires the {convention} signature convention",
                entry.kind, function.name
            ),
        ));
    }

    if entry.kind == GpuEntryKind::Visible {
        return;
    }

    if signature.flags & 7 != 0 {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU entry function @{} cannot have failing, variadic, or noreturn signature flags",
                function.name
            ),
        ));
    }

    let mut input_item = 0;
    if let Some(root) = &entry.root {
        let root_matches = matches!(
            signature.parameters.first(),
            Some(ParameterItemSummary::Leaf { type_index })
                if matches!(
                    usize::try_from(*type_index)
                        .ok()
                        .and_then(|index| types.value_types.get(index)),
                    Some(TvisaValueTypeRecord::Pointer { address_space })
                        if *address_space == root.address_space
                )
        );
        if !root_matches {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function @{} root must be its first leaf pointer parameter in the recorded address space",
                    function.name
                ),
            ));
        }
        input_item += 1;
    }
    if entry.immediate_size != 0 {
        match signature.parameters.get(input_item) {
            Some(
                ParameterItemSummary::Leaf { .. }
                | ParameterItemSummary::Group(_)
                | ParameterItemSummary::ByRef { .. },
            ) => {}
            _ => diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function @{} has immediate data but no valid immediate parameter item",
                    function.name
                ),
            )),
        }
    }

    for specialization in &entry.specialization_constants {
        if !valid_specialization_type(specialization.type_index, entry.capabilities, types) {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function @{} specialization constant {} has an unsupported type or lacks the float64 capability",
                    function.name, specialization.id
                ),
            ));
        }
        let Some(default) = usize::try_from(specialization.default_constant)
            .ok()
            .and_then(|index| constants.get(index))
        else {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function @{} specialization constant {} references an invalid default",
                    function.name, specialization.id
                ),
            ));
            continue;
        };
        if default.type_index != specialization.type_index {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU entry function @{} specialization constant {} default type disagrees with its declared type",
                    function.name, specialization.id
                ),
            ));
        }
    }

    let parameter_types = flatten_parameter_types(&signature.parameters);
    if !entry.stage_input.is_empty()
        && !contains_type_sequence(
            parameter_types
                .get(entry_input_value_count(entry, signature)..)
                .unwrap_or(&[]),
            &entry
                .stage_input
                .iter()
                .map(|field| field.type_index)
                .collect::<Vec<_>>(),
        )
    {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU {:?} entry function @{} signature does not contain its stage input fields in order",
                entry.kind, function.name
            ),
        ));
    }

    let result_types = flatten_result_types(&signature.results);
    if matches!(entry.kind, GpuEntryKind::Vertex | GpuEntryKind::Fragment) {
        let stage_output_types = entry
            .stage_output
            .iter()
            .map(|field| field.type_index)
            .collect::<Vec<_>>();
        if result_types != stage_output_types {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU {:?} entry function @{} result fields disagree with its stage outputs",
                    entry.kind, function.name
                ),
            ));
        } else if let [ResultItemSummary::Group(group)] = signature.results.as_slice()
            && (group.members.len() != entry.stage_output.len()
                || group
                    .members
                    .iter()
                    .zip(&entry.stage_output)
                    .any(|(member, field)| member.offset != field.byte_offset))
        {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "GPU {:?} entry function @{} result layout disagrees with its stage outputs",
                    entry.kind, function.name
                ),
            ));
        }
    } else if !result_types.is_empty() {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "GPU {:?} entry function @{} must return unit",
                entry.kind, function.name
            ),
        ));
    }
}

fn entry_input_value_count(
    entry: &tvisa_format::GpuEntryRecord,
    signature: &tvisa_format::SignatureSummary,
) -> usize {
    let mut item_index = 0;
    let mut value_count = 0;
    if entry.root.is_some() {
        item_index += 1;
        value_count += 1;
    }
    if entry.immediate_size != 0 {
        if let Some(item) = signature.parameters.get(item_index) {
            value_count += match item {
                ParameterItemSummary::Group(group) => group.members.len(),
                ParameterItemSummary::Leaf { .. } | ParameterItemSummary::ByRef { .. } => 1,
                _ => 0,
            };
        }
    }
    value_count
}

fn valid_specialization_type(type_index: u64, capabilities: u64, types: &TypeSection) -> bool {
    let Some(value_type) = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.value_types.get(index))
    else {
        return false;
    };
    match value_type {
        TvisaValueTypeRecord::Integer { bits } => *bits <= 64,
        TvisaValueTypeRecord::Float { tag } => {
            matches!(*tag, 0x02..=0x05) && (*tag != 0x05 || capabilities & 1 != 0)
        }
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } if matches!(*lanes, 2 | 3 | 4) => {
            let Some(element) = usize::try_from(*element_type)
                .ok()
                .and_then(|index| types.value_types.get(index))
            else {
                return false;
            };
            match element {
                TvisaValueTypeRecord::Integer { bits } => *bits <= 64,
                TvisaValueTypeRecord::Float { tag } => {
                    matches!(*tag, 0x02..=0x05 | 0x07..=0x0c)
                        && (*tag != 0x05 || capabilities & 1 != 0)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn flatten_parameter_types(parameters: &[ParameterItemSummary]) -> Vec<Option<u64>> {
    let mut types = Vec::new();
    for parameter in parameters {
        match parameter {
            ParameterItemSummary::Leaf { type_index } => types.push(Some(*type_index)),
            ParameterItemSummary::Group(group) => {
                types.extend(group.members.iter().map(|member| Some(member.type_index)));
            }
            ParameterItemSummary::ByRef { .. }
            | ParameterItemSummary::SRet { .. }
            | ParameterItemSummary::Context
            | ParameterItemSummary::CAggregate { .. } => types.push(None),
        }
    }
    types
}

fn flatten_result_types(results: &[ResultItemSummary]) -> Vec<u64> {
    let mut types = Vec::new();
    for result in results {
        match result {
            ResultItemSummary::Leaf { type_index } => types.push(*type_index),
            ResultItemSummary::Group(group) => {
                types.extend(group.members.iter().map(|member| member.type_index));
            }
        }
    }
    types
}

fn contains_type_sequence(parameters: &[Option<u64>], sequence: &[u64]) -> bool {
    !sequence.is_empty()
        && parameters.windows(sequence.len()).any(|window| {
            window
                .iter()
                .zip(sequence)
                .all(|(parameter, expected)| *parameter == Some(*expected))
        })
}
