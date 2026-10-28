use crate::bytecode::{Instr, Opcode};
use crate::program::{BcFunction, BranchCondition};
use std::collections::VecDeque;

fn terminator(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::Br
            | Opcode::BrIf
            | Opcode::BrCmp
            | Opcode::Ret
            | Opcode::RetV
            | Opcode::Fail
            | Opcode::Trap
            | Opcode::Unreach
    )
}

pub(super) fn starts(function: &BcFunction) -> Vec<bool> {
    let mut starts = vec![false; function.code.len() + 1];
    starts[0] = true;
    for branch in &function.branches {
        starts[branch.then_edge.target] = true;
        if let Some(edge) = &branch.else_edge {
            starts[edge.target] = true;
        }
    }
    for (pc, word) in function.code.iter().enumerate() {
        if terminator(Instr::decode(*word).unwrap().op) {
            starts[pc + 1] = true;
        }
    }
    starts[function.code.len()] = true;
    starts
}

fn effects(instruction: Instr, function: &BcFunction) -> (Vec<usize>, Vec<usize>, bool) {
    let (mask, writes, pure) = super::simplify::effect(instruction, function);
    let mut reads = Vec::new();
    if mask == 8 {
        reads.extend(0..function.register_count as usize);
    } else {
        for (bit, register) in [(1, instruction.a), (2, instruction.b), (4, instruction.c)] {
            if mask & bit != 0 {
                reads.push(register as usize);
            }
        }
    }
    if instruction.op == Opcode::Call {
        let site = &function.call_sites[instruction.bx() as usize];
        reads.extend((site.first_arg..site.first_arg + site.arg_count).map(usize::from));
        reads.extend(site.target.map(usize::from));
    }
    if matches!(instruction.op, Opcode::Br | Opcode::BrIf | Opcode::BrCmp) {
        let branch = &function.branches[instruction.bx() as usize];
        match branch.condition {
            Some(BranchCondition::Register(register)) => reads.push(register as usize),
            Some(BranchCondition::IntegerCompare { lhs, rhs, .. }) => {
                reads.extend([lhs as usize, rhs as usize]);
            }
            None => {}
        }
        // Keep edge sources live even when their destination is dead. Parallel
        // edge moves remain intact, including comparison result registers.
        for edge in std::iter::once(&branch.then_edge).chain(&branch.else_edge) {
            reads.extend(edge.moves.iter().map(|movement| movement.source as usize));
        }
    }
    (reads, writes, pure)
}

pub(super) fn eliminate_dead(function: &mut BcFunction) {
    if function.code.is_empty()
        || function.code.iter().any(|word| {
            matches!(
                Instr::decode(*word).unwrap().op,
                Opcode::Jmp | Opcode::Jif | Opcode::Jifn
            )
        })
    {
        return;
    }
    let boundaries = starts(function)
        .into_iter()
        .enumerate()
        .filter_map(|(pc, start)| start.then_some(pc))
        .collect::<Vec<_>>();
    let blocks = boundaries.len() - 1;
    let count = function.register_count as usize;
    let mut owners = vec![0; function.code.len() + 1];
    for (block, range) in boundaries.windows(2).enumerate() {
        owners[range[0]..range[1]].fill(block);
    }
    let instructions = function
        .code
        .iter()
        .map(|word| Instr::decode(*word).unwrap())
        .collect::<Vec<_>>();
    let effects = instructions
        .iter()
        .map(|instruction| effects(*instruction, function))
        .collect::<Vec<_>>();
    let mut successors = vec![Vec::new(); blocks];
    let mut predecessors = vec![Vec::new(); blocks];
    // Per-block register sets as rows of 64-bit words, so the dataflow
    // below joins and compares 64 registers at a time.
    let stride = count.div_ceil(64);
    let bit = |register: usize| (register / 64, 1u64 << (register % 64));
    let mut uses = vec![0u64; blocks * stride];
    let mut definitions = uses.clone();
    for block in 0..blocks {
        let range = boundaries[block]..boundaries[block + 1];
        let row = block * stride;
        for pc in range.clone() {
            let (reads, writes, _) = &effects[pc];
            for &register in reads {
                let (word, mask) = bit(register);
                if definitions[row + word] & mask == 0 {
                    uses[row + word] |= mask;
                }
            }
            for &register in writes {
                let (word, mask) = bit(register);
                definitions[row + word] |= mask;
            }
        }
        let last = instructions[range.end - 1];
        if matches!(last.op, Opcode::Br | Opcode::BrIf | Opcode::BrCmp) {
            let branch = &function.branches[last.bx() as usize];
            for edge in std::iter::once(&branch.then_edge).chain(&branch.else_edge) {
                if edge.target < function.code.len() {
                    successors[block].push(owners[edge.target]);
                }
            }
            if branch.condition.is_some() && branch.else_edge.is_none() && block + 1 < blocks {
                successors[block].push(block + 1);
            }
        } else if !terminator(last.op) && block + 1 < blocks {
            successors[block].push(block + 1);
        }
        for &successor in &successors[block] {
            predecessors[successor].push(block);
        }
    }
    let mut incoming = uses.clone();
    let mut outgoing = vec![0u64; blocks * stride];
    let mut queue = (0..blocks).rev().collect::<VecDeque<_>>();
    let mut queued = vec![true; blocks];
    while let Some(block) = queue.pop_front() {
        queued[block] = false;
        let row = block * stride;
        for &successor in &successors[block] {
            let from = successor * stride;
            for word in 0..stride {
                outgoing[row + word] |= incoming[from + word];
            }
        }
        let mut changed = false;
        for word in 0..stride {
            let index = row + word;
            let live = uses[index] | (outgoing[index] & !definitions[index]);
            changed |= live != incoming[index];
            incoming[index] = live;
        }
        if changed {
            for &predecessor in &predecessors[block] {
                if !queued[predecessor] {
                    queued[predecessor] = true;
                    queue.push_back(predecessor);
                }
            }
        }
    }
    let mut retained = vec![true; instructions.len()];
    for block in 0..blocks {
        let mut live = outgoing[block * stride..(block + 1) * stride].to_vec();
        let alive = |live: &[u64], register: usize| {
            let (word, mask) = bit(register);
            live[word] & mask != 0
        };
        for pc in (boundaries[block]..boundaries[block + 1]).rev() {
            let (reads, writes, pure) = &effects[pc];
            if *pure && writes.iter().all(|register| !alive(&live, *register)) {
                retained[pc] = false;
                continue;
            }
            for &register in writes {
                let (word, mask) = bit(register);
                live[word] &= !mask;
            }
            for &register in reads {
                let (word, mask) = bit(register);
                live[word] |= mask;
            }
        }
    }
    let mut offsets = Vec::with_capacity(retained.len() + 1);
    let mut code = Vec::with_capacity(retained.len());
    for (word, keep) in function.code.iter().zip(retained) {
        offsets.push(code.len());
        if keep {
            code.push(*word);
        }
    }
    offsets.push(code.len());
    for branch in &mut function.branches {
        branch.then_edge.target = offsets[branch.then_edge.target];
        if let Some(edge) = &mut branch.else_edge {
            edge.target = offsets[edge.target];
        }
    }
    function.code = code;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{BranchSite, BranchTarget, Program, RegisterMove};
    use crate::{Outcome, run};

    #[test]
    fn comparison_edge_reads_the_new_result_instead_of_its_old_alias() {
        let mut function = BcFunction {
            name: "comparison".into(),
            register_count: 4,
            param_count: 1,
            returns_value: true,
            allocations: vec![],
            call_sites: vec![],
            code: vec![
                Instr::from_bx(Opcode::LoadI, 1, 0).encode(),
                Instr::abc(Opcode::Mov, 2, 0, 0).encode(),
                Instr::from_bx(Opcode::BrCmp, 0, 0).encode(),
                Instr::abc(Opcode::RetV, 3, 0, 0).encode(),
            ],
            branches: vec![BranchSite {
                condition: Some(BranchCondition::IntegerCompare {
                    result: 2,
                    lhs: 0,
                    rhs: 1,
                    width: crate::bytecode::WidthCode::W64,
                    predicate: 1,
                }),
                then_edge: BranchTarget {
                    target: 3,
                    moves: vec![RegisterMove {
                        source: 2,
                        destination: 3,
                    }],
                },
                else_edge: Some(BranchTarget {
                    target: 3,
                    moves: vec![RegisterMove {
                        source: 2,
                        destination: 3,
                    }],
                }),
            }],
        };
        super::super::simplify::local_registers(&mut function);
        let program = Program {
            functions: vec![function],
            consts: vec![],
            imports: vec![],
            globals: vec![],
            initializers: vec![],
        };
        for argument in [0, 1, 2, 17] {
            assert_eq!(
                run(&program, 0, &[argument]),
                Outcome::Return(Some(u64::from(argument != 0)))
            );
        }
    }

    #[test]
    fn copy_elimination_preserves_loop_edges_and_overwritten_sources() {
        let mut function = BcFunction {
            name: "loop".into(),
            register_count: 7,
            param_count: 1,
            returns_value: true,
            allocations: vec![],
            call_sites: vec![],
            code: vec![
                Instr::from_bx(Opcode::LoadI, 1, 1).encode(),
                Instr::from_bx(Opcode::LoadI, 2, 0).encode(),
                Instr::from_bx(Opcode::Br, 0, 0).encode(),
                Instr::abc(Opcode::Mov, 3, 0, 0).encode(),
                Instr::abc(Opcode::Mov, 4, 3, 0).encode(),
                Instr::from_bx(Opcode::BrCmp, 0, 1).encode(),
                Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
                Instr::abc(Opcode::Mov, 5, 2, 0).encode(),
                Instr::abc(Opcode::Add, 2, 2, 1).with_flags(3).encode(),
                Instr::abc(Opcode::Add, 2, 2, 5).with_flags(3).encode(),
                Instr::abc(Opcode::Sub, 0, 0, 1).with_flags(3).encode(),
                Instr::from_bx(Opcode::Br, 0, 2).encode(),
            ],
            branches: vec![
                BranchSite {
                    condition: None,
                    then_edge: BranchTarget {
                        target: 3,
                        moves: vec![],
                    },
                    else_edge: None,
                },
                BranchSite {
                    condition: Some(BranchCondition::IntegerCompare {
                        result: 4,
                        lhs: 3,
                        rhs: 6,
                        width: crate::bytecode::WidthCode::W64,
                        predicate: 1,
                    }),
                    then_edge: BranchTarget {
                        target: 7,
                        moves: vec![RegisterMove {
                            source: 3,
                            destination: 0,
                        }],
                    },
                    else_edge: None,
                },
                BranchSite {
                    condition: None,
                    then_edge: BranchTarget {
                        target: 3,
                        moves: vec![],
                    },
                    else_edge: None,
                },
            ],
        };
        let original = function.clone();
        super::super::simplify::local_registers(&mut function);
        assert!(function.code.len() < original.code.len());
        for argument in 0..8 {
            let program = |function| Program {
                functions: vec![function],
                consts: vec![],
                imports: vec![],
                globals: vec![],
                initializers: vec![],
            };
            let expected = Outcome::Return(Some((1 << argument) - 1));
            assert_eq!(run(&program(original.clone()), 0, &[argument]), expected);
            assert_eq!(run(&program(function.clone()), 0, &[argument]), expected);
        }
    }
}
