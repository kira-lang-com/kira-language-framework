use super::{DecodedBranchSite, DecodedCondition, DecodedInstr, join_u32};

pub(super) fn decode_simple_branches(
    code: &mut [DecodedInstr],
    branches: &[Option<DecodedBranchSite>],
) {
    let end = code.len();
    for (pc, instruction) in code.iter_mut().enumerate() {
        let (site, conditional) = match *instruction {
            DecodedInstr::Branch { low, high } => (join_u32(low, high), false),
            DecodedInstr::BranchIf { low, high } => (join_u32(low, high), true),
            _ => continue,
        };
        let Some(branch) = branches.get(site as usize).and_then(Option::as_ref) else {
            continue;
        };
        if !branch.then_edge.moves.is_empty()
            || branch
                .else_edge
                .as_ref()
                .is_some_and(|edge| !edge.moves.is_empty())
        {
            continue;
        }
        let target = branch.then_edge.target.min(end);
        if !conditional {
            if let Some((low, high)) = immediate(target) {
                *instruction = DecodedInstr::Jmp { low, high };
            }
            continue;
        }
        let Some(DecodedCondition::Register(condition)) = branch.condition else {
            continue;
        };
        let other = branch
            .else_edge
            .as_ref()
            .map_or(pc + 1, |edge| edge.target.min(end));
        let (target, inverted) = if other == pc + 1 {
            (target, false)
        } else if target == pc + 1 {
            (other, true)
        } else {
            continue;
        };
        if let Some((low, high)) = immediate(target) {
            *instruction = if inverted {
                DecodedInstr::Jifn {
                    condition,
                    low,
                    high,
                }
            } else {
                DecodedInstr::Jif {
                    condition,
                    low,
                    high,
                }
            };
        }
    }
}

fn immediate(target: usize) -> Option<(u16, u16)> {
    let target = i32::try_from(target).ok()? as u32;
    Some((target as u16, (target >> 16) as u16))
}

#[cfg(test)]
mod tests {
    use super::super::super::{EvaluationBudget, Machine, Outcome};
    use super::super::decode_program;
    use super::*;
    use crate::program::{
        BcFunction, BranchCondition, BranchSite, BranchTarget, Program, RegisterMove,
    };
    use crate::{Capabilities, Instr, Opcode};

    #[test]
    fn direct_conditional_jumps_keep_targets_metering_and_edge_moves() {
        for (then_target, else_target) in [(1, 3), (3, 1)] {
            let mut program = Program {
                functions: vec![BcFunction {
                    name: "branch".into(),
                    register_count: 2,
                    param_count: 1,
                    returns_value: true,
                    code: vec![
                        Instr::from_bx(Opcode::BrIf, 0, 0).encode(),
                        Instr::from_bx(Opcode::LoadI, 1, 7).encode(),
                        Instr::from_bx(Opcode::Br, 0, 1).encode(),
                        Instr::from_bx(Opcode::LoadI, 1, 9).encode(),
                        Instr::abc(Opcode::RetV, 1, 0, 0).encode(),
                    ],
                    allocations: vec![],
                    call_sites: vec![],
                    branches: vec![
                        BranchSite {
                            condition: Some(BranchCondition::Register(0)),
                            then_edge: BranchTarget {
                                target: then_target,
                                moves: vec![],
                            },
                            else_edge: Some(BranchTarget {
                                target: else_target,
                                moves: vec![],
                            }),
                        },
                        BranchSite {
                            condition: None,
                            then_edge: BranchTarget {
                                target: 4,
                                moves: vec![],
                            },
                            else_edge: None,
                        },
                    ],
                }],
                consts: vec![],
                globals: vec![],
                imports: vec![],
                initializers: vec![],
            };
            let decoded = decode_program(&program);
            assert!(matches!(
                decoded[0].code[0],
                DecodedInstr::Jif { .. } | DecodedInstr::Jifn { .. }
            ));
            assert!(matches!(decoded[0].code[2], DecodedInstr::Jmp { .. }));
            for argument in [0, 1, 2, 3] {
                let target = if argument & 1 == 1 {
                    then_target
                } else {
                    else_target
                };
                let expected = Outcome::Return(Some(if target == 1 { 7 } else { 9 }));
                assert_eq!(
                    Machine::new(&program).call(0, &[argument], &Capabilities::new()),
                    expected
                );
                let mut budget = EvaluationBudget {
                    steps_remaining: 10,
                    memory_bytes: 4096,
                };
                let mut machine = Machine::for_evaluation(&program, &budget).unwrap();
                assert_eq!(machine.evaluate(0, &[argument], &mut budget), expected);
                assert_eq!(budget.steps_remaining, if target == 1 { 6 } else { 7 });
            }
            program.functions[0].branches[0]
                .then_edge
                .moves
                .push(RegisterMove {
                    source: 0,
                    destination: 1,
                });
            assert!(matches!(
                decode_program(&program)[0].code[0],
                DecodedInstr::BranchIf { .. }
            ));
        }
    }
}
