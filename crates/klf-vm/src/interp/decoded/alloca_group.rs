use crate::program::AllocShape;

use super::{DecodedInstr, join_u32};

pub(crate) struct DecodedAllocaGroup {
    pub(in crate::interp) slots: Vec<DecodedAllocaSlot>,
    pub(in crate::interp) size: u32,
    pub(in crate::interp) align: u32,
}

pub(in crate::interp) struct DecodedAllocaSlot {
    pub(in crate::interp) register: u16,
    pub(in crate::interp) shape: AllocShape,
    pub(in crate::interp) offset: u32,
}

pub(super) fn decode_alloca_groups(
    code: &mut [DecodedInstr],
    allocations: &[AllocShape],
) -> Vec<DecodedAllocaGroup> {
    let mut groups = Vec::new();
    let mut pc = 0;
    while pc < code.len() {
        let mut slots = Vec::new();
        let mut size = 0u32;
        let mut align = 16;
        for instruction in &code[pc..] {
            let DecodedInstr::Alloca {
                a,
                index_low,
                index_high,
            } = *instruction
            else {
                break;
            };
            let Some(shape) = allocations.get(join_u32(index_low, index_high) as usize) else {
                break;
            };
            let slot_align = shape.align.max(16);
            if !slot_align.is_power_of_two() || slot_align > 4096 {
                break;
            }
            let Some(offset) = size
                .checked_add(slot_align - 1)
                .map(|n| n & !(slot_align - 1))
            else {
                break;
            };
            let Some(end) = offset.checked_add(shape.size).filter(|end| *end <= 65536) else {
                break;
            };
            slots.push(DecodedAllocaSlot {
                register: a,
                shape: *shape,
                offset,
            });
            size = end;
            align = align.max(slot_align);
        }
        if slots.len() >= 3 && size != 0 {
            let Ok(index) = u32::try_from(groups.len()) else {
                break;
            };
            let count = slots.len();
            groups.push(DecodedAllocaGroup { slots, size, align });
            code[pc] = DecodedInstr::AllocaGroup(index);
            pc += count;
        } else {
            pc += 1;
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{Instr, Opcode};
    use crate::interp::decoded::decode_program;
    use crate::interp::{Capabilities, EvaluationBudget, Machine, Outcome};
    use crate::program::{BcFunction, BranchSite, BranchTarget, Program};

    fn program() -> Program {
        Program {
            functions: vec![BcFunction {
                name: "allocation_group".into(),
                register_count: 4,
                param_count: 0,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::Alloca, 0, 0).encode(),
                    Instr::from_bx(Opcode::Alloca, 1, 1).encode(),
                    Instr::from_bx(Opcode::Alloca, 2, 2).encode(),
                    Instr::from_bx(Opcode::LoadI, 3, 73).encode(),
                    Instr::abc(Opcode::Store, 0, 3, 0).with_flags(3).encode(),
                    Instr::from_bx(Opcode::LoadI, 3, 19).encode(),
                    Instr::abc(Opcode::Store, 1, 3, 0).with_flags(3).encode(),
                    Instr::abc(Opcode::Load, 3, 0, 0).with_flags(3).encode(),
                    Instr::abc(Opcode::RetV, 3, 0, 0).encode(),
                ],
                allocations: vec![
                    AllocShape { size: 8, align: 8 },
                    AllocShape { size: 8, align: 64 },
                    AllocShape { size: 0, align: 32 },
                ],
                branches: vec![],
                call_sites: vec![],
            }],
            consts: vec![],
            globals: vec![],
            imports: vec![],
            initializers: vec![],
        }
    }

    #[test]
    fn allocation_groups_preserve_values_alignment_reuse_and_metered_steps() {
        let program = program();
        let decoded = decode_program(&program);
        assert!(matches!(decoded[0].code[0], DecodedInstr::AllocaGroup(0)));
        assert_eq!(decoded[0].alloca_groups[0].slots[1].offset, 64);
        assert_eq!(decoded[0].alloca_groups[0].slots[2].offset, 96);
        let mut machine = Machine::new(&program);
        for _ in 0..2 {
            assert_eq!(
                machine.call(0, &[], &Capabilities::new()),
                Outcome::Return(Some(73))
            );
        }
        let mut budget = EvaluationBudget {
            steps_remaining: 9,
            memory_bytes: 1 << 20,
        };
        let mut machine = Machine::for_evaluation(&program, &budget).unwrap();
        assert_eq!(
            machine.evaluate(0, &[], &mut budget),
            Outcome::Return(Some(73))
        );
        assert_eq!(budget.steps_remaining, 0);
        let mut budget = EvaluationBudget {
            steps_remaining: 2,
            memory_bytes: 1 << 20,
        };
        let mut machine = Machine::for_evaluation(&program, &budget).unwrap();
        assert_eq!(
            machine.evaluate(0, &[], &mut budget),
            Outcome::Trap("KLF-VM-0022".to_owned())
        );
    }

    #[test]
    fn allocation_groups_keep_interior_entry_and_reject_large_or_irregular_alignments() {
        let mut program = program();
        program.functions[0].code[0] = Instr::from_bx(Opcode::Alloca, 0, 0).encode();
        let decoded = decode_program(&program);
        assert!(matches!(decoded[0].code[2], DecodedInstr::Alloca { .. }));
        let mut interior = program.clone();
        let function = &mut interior.functions[0];
        function.allocations[2].size = 8;
        function.code[4] = Instr::abc(Opcode::Store, 1, 3, 0).with_flags(3).encode();
        function.code[6] = Instr::abc(Opcode::Store, 2, 3, 0).with_flags(3).encode();
        function.code[7] = Instr::abc(Opcode::Load, 3, 1, 0).with_flags(3).encode();
        function
            .code
            .insert(0, Instr::from_bx(Opcode::Br, 0, 0).encode());
        function.branches.push(BranchSite {
            condition: None,
            then_edge: BranchTarget {
                target: 2,
                moves: vec![],
            },
            else_edge: None,
        });
        assert!(matches!(
            decode_program(&interior)[0].code[1],
            DecodedInstr::AllocaGroup(0)
        ));
        assert_eq!(
            Machine::new(&interior).call(0, &[], &Capabilities::new()),
            Outcome::Return(Some(73))
        );
        let mut budget = EvaluationBudget {
            steps_remaining: 9,
            memory_bytes: 1 << 20,
        };
        let mut machine = Machine::for_evaluation(&interior, &budget).unwrap();
        assert_eq!(
            machine.evaluate(0, &[], &mut budget),
            Outcome::Return(Some(73))
        );
        assert_eq!(budget.steps_remaining, 0);
        for shape in [
            AllocShape {
                size: 1 << 20,
                align: 16,
            },
            AllocShape { size: 8, align: 48 },
        ] {
            program.functions[0].allocations[1] = shape;
            assert!(decode_program(&program)[0].alloca_groups.is_empty());
        }
    }
}
