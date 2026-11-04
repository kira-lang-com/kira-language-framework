use tir_diag::Diagnostic;
use tir_format::BinarySignatureTable;
use tir_format::{BinaryGenericTable, BinaryInstruction, BinaryOperand, StringTable};

use super::V15;

pub(super) fn resolve_gpu_call(
    instruction: &BinaryInstruction,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    signatures: &BinarySignatureTable,
    caller: &str,
) -> Option<Result<usize, Diagnostic>> {
    if instruction.opcode != 0x1a5 {
        return None;
    }
    let resolved = resolve(instruction, generics, strings).and_then(|target| {
        let target = target.ok_or_else(|| "GPU call.witness has no target".to_owned())?;
        let callee = signatures
            .records
            .get(target)
            .ok_or_else(|| "GPU witness function index is out of range".to_owned())?;
        if !super::is_gpu_target(callee.target) {
            return Err(format!("GPU code calls CPU-only witness @{}", callee.name));
        }
        Ok(target)
    });
    Some(resolved.map_err(|message| {
        Diagnostic::error(
            V15,
            format!(
                "function @{caller} CODE offset {}: {message}",
                instruction.offset
            ),
        )
    }))
}

pub(super) fn resolve(
    instruction: &BinaryInstruction,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
) -> Result<Option<usize>, String> {
    if instruction.opcode != 0x1a5 {
        return Ok(None);
    }
    let type_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
        .ok_or_else(|| "GPU call.witness has no concrete type operand".to_owned())?;
    let method_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .ok_or_else(|| "GPU call.witness has no trait-method name".to_owned())?;
    let method_path = strings
        .and_then(|strings| {
            usize::try_from(method_index)
                .ok()
                .and_then(|index| strings.get(index))
        })
        .ok_or_else(|| "GPU call.witness trait-method name is unavailable".to_owned())?;
    let (trait_path, method) = method_path
        .rsplit_once('.')
        .filter(|(trait_path, method)| !trait_path.is_empty() && !method.is_empty())
        .ok_or_else(|| "GPU call.witness must name Trait.method".to_owned())?;
    let conformances = generics
        .into_iter()
        .flat_map(|generics| &generics.conformances)
        .filter(|conformance| {
            conformance.type_index == type_index && conformance.trait_path == trait_path
        })
        .collect::<Vec<_>>();
    let [conformance] = conformances.as_slice() else {
        return Err(if conformances.is_empty() {
            format!(
                "GPU call.witness {trait_path}.{method} has no local concrete conformance to resolve statically"
            )
        } else {
            format!("GPU call.witness {trait_path}.{method} has ambiguous concrete conformances")
        });
    };
    let targets = conformance
        .witnesses
        .iter()
        .filter(|witness| witness.method == method)
        .map(|witness| witness.function_index)
        .collect::<Vec<_>>();
    let [target] = targets.as_slice() else {
        return Err(if targets.is_empty() {
            format!(
                "GPU call.witness {trait_path}.{method} has no statically resolvable witness function"
            )
        } else {
            format!("GPU call.witness {trait_path}.{method} has ambiguous witness functions")
        });
    };
    usize::try_from(*target)
        .map(Some)
        .map_err(|_| "GPU witness function index overflows memory".to_owned())
}
