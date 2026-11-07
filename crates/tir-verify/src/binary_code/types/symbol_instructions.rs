//! V2 cross-section verification for direct GLOBALS and TYPES reference CODE.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryGlobalTable, BinaryInstruction, BinaryOperand, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    types: &BinaryTypeTable,
    globals: Option<&BinaryGlobalTable>,
    function: &str,
) -> Vec<Diagnostic> {
    body.blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter_map(|instruction| {
            check(instruction, types, globals).map(|issue| {
                Diagnostic::error(
                    V2,
                    format!(
                        "function @{function} CODE offset {}: {issue}",
                        instruction.offset
                    ),
                )
            })
        })
        .collect()
}

fn check(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    globals: Option<&BinaryGlobalTable>,
) -> Option<&'static str> {
    if !matches!(instruction.opcode, 0x00b..=0x00d) {
        return None;
    }
    if instruction.modifiers != 0 || instruction.result_count != 1 || instruction.terminator {
        return Some("symbol reference must define exactly one SSA value with no modifiers");
    }
    match (instruction.opcode, instruction.operands.as_slice()) {
        (0x00b | 0x00c, [BinaryOperand::Symbol { kind: 1, index }]) => {
            let global = globals.and_then(|globals| {
                usize::try_from(*index)
                    .ok()
                    .and_then(|index| globals.records.get(index))
            });
            match global {
                Some(global) if instruction.opcode != 0x00c || global.thread_local => None,
                Some(_) => Some("tls.addr symbol does not name a thread-local global"),
                None => Some("global.addr/tls.addr symbol index is outside GLOBALS"),
            }
        }
        (0x00d, [BinaryOperand::Type(ty)]) => {
            if usize::try_from(*ty)
                .ok()
                .and_then(|index| types.entries.get(index))
                .is_some()
            {
                None
            } else {
                Some("descriptor.ref type index is outside TYPES")
            }
        }
        _ => Some("symbol reference has invalid operand kinds or arity"),
    }
}

#[cfg(test)]
mod tests {
    use super::check;
    use tir_format::{
        BinaryAddressSpace, BinaryGlobal, BinaryGlobalTable, BinaryInstruction, BinaryLinkage,
        BinaryOperand, BinaryType, BinaryTypeTable,
    };

    fn instruction(opcode: u64, operands: Vec<BinaryOperand>) -> BinaryInstruction {
        BinaryInstruction {
            offset: 0,
            opcode,
            modifiers: 0,
            result_count: 1,
            terminator: false,
            operands,
        }
    }

    #[test]
    fn v2_checks_cross_section_global_and_descriptor_references() {
        let types = BinaryTypeTable {
            declarations: Vec::new(),
            entries: vec![BinaryType::Integer(32)],
            definitions: Vec::new(),
        };
        let global = |thread_local| BinaryGlobal {
            name: "Storage".into(),
            linkage: BinaryLinkage::Module,
            ty: 0,
            address_space: BinaryAddressSpace::Generic,
            thread_local,
            constant: false,
            per_cpu: false,
            alignment: None,
            initializer: None,
        };
        let globals = BinaryGlobalTable {
            records: vec![global(false), global(true)],
        };
        let symbol = |index| BinaryOperand::Symbol { kind: 1, index };
        assert!(check(&instruction(0x00b, vec![symbol(0)]), &types, Some(&globals)).is_none());
        assert!(check(&instruction(0x00c, vec![symbol(1)]), &types, Some(&globals)).is_none());
        assert!(
            check(
                &instruction(0x00d, vec![BinaryOperand::Type(0)]),
                &types,
                Some(&globals)
            )
            .is_none()
        );
        assert!(check(&instruction(0x00c, vec![symbol(0)]), &types, Some(&globals)).is_some());
        assert!(check(&instruction(0x00b, vec![symbol(9)]), &types, Some(&globals)).is_some());
        assert!(check(&instruction(0x00b, vec![symbol(0)]), &types, None).is_some());
        assert!(
            check(
                &instruction(0x00d, vec![BinaryOperand::Type(3)]),
                &types,
                Some(&globals)
            )
            .is_some()
        );
        assert!(
            check(
                &instruction(0x00b, vec![BinaryOperand::Type(0)]),
                &types,
                Some(&globals)
            )
            .is_some()
        );
        let mut malformed = instruction(0x00c, vec![symbol(1)]);
        malformed.modifiers = 1 << 7;
        assert!(check(&malformed, &types, Some(&globals)).is_some());
        malformed.modifiers = 0;
        malformed.result_count = 0;
        assert!(check(&malformed, &types, Some(&globals)).is_some());
    }
}
