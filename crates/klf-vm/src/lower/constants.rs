use crate::bytecode::{Instr, Opcode, WidthCode, flags_width};
use crate::program::{BcFunction, BranchCondition, BranchTarget};
use std::collections::VecDeque;

/// The registers known to hold a constant, sorted by register. A register
/// absent from it is not known. Only a register some fold or branch reads is
/// ever known (`tracked`), so a state holds a few constants, and copying or
/// merging one costs those rather than the function's register count.
#[derive(Clone)]
struct State<'a> {
    known: Vec<(u32, u64)>,
    tracked: &'a [bool],
}

impl<'a> State<'a> {
    fn new(tracked: &'a [bool]) -> Self {
        Self {
            known: Vec::new(),
            tracked,
        }
    }

    fn get(&self, register: usize) -> Option<u64> {
        let register = register as u32;
        self.known
            .binary_search_by_key(&register, |&(known, _)| known)
            .ok()
            .map(|index| self.known[index].1)
    }

    fn set(&mut self, register: usize, constant: Option<u64>) {
        let constant = constant.filter(|_| self.tracked[register]);
        let register = register as u32;
        match (
            self.known
                .binary_search_by_key(&register, |&(known, _)| known),
            constant,
        ) {
            (Ok(index), Some(constant)) => self.known[index].1 = constant,
            (Ok(index), None) => {
                self.known.remove(index);
            }
            (Err(index), Some(constant)) => self.known.insert(index, (register, constant)),
            (Err(_), None) => {}
        }
    }
}

/// The registers whose constant can change what `propagate` writes: the
/// operands `value` folds, the registers a branch tests, and, through the
/// moves on an edge, the registers those are moved from.
fn tracked(function: &BcFunction, instructions: &[Instr]) -> Vec<bool> {
    let mut tracked = vec![false; function.register_count as usize];
    for instruction in instructions {
        match instruction.op {
            Opcode::Mov | Opcode::Bitcast | Opcode::Trunc | Opcode::ZExt | Opcode::SExt => {
                tracked[instruction.b as usize] = true;
            }
            Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Icmp => {
                tracked[instruction.b as usize] = true;
                tracked[instruction.c as usize] = true;
            }
            _ => {}
        }
    }
    for branch in &function.branches {
        match &branch.condition {
            Some(BranchCondition::Register(register)) => tracked[*register as usize] = true,
            Some(BranchCondition::IntegerCompare { lhs, rhs, .. }) => {
                tracked[*lhs as usize] = true;
                tracked[*rhs as usize] = true;
            }
            None => {}
        }
    }
    let moves = function
        .branches
        .iter()
        .flat_map(|branch| std::iter::once(&branch.then_edge).chain(&branch.else_edge))
        .flat_map(|edge| &edge.moves)
        .collect::<Vec<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        for movement in &moves {
            if tracked[movement.destination as usize] && !tracked[movement.source as usize] {
                tracked[movement.source as usize] = true;
                changed = true;
            }
        }
    }
    tracked
}

fn value(instruction: Instr, state: &State) -> Option<u64> {
    let a = || state.get(instruction.b as usize);
    let b = || state.get(instruction.c as usize);
    match instruction.op {
        Opcode::LoadI => Some(i64::from(instruction.bx()) as u64),
        Opcode::LoadNull => Some(0),
        Opcode::Mov | Opcode::Bitcast => a(),
        Opcode::Trunc | Opcode::ZExt | Opcode::SExt
            if instruction.flags & 7 <= 3 && (instruction.flags >> 3) & 7 <= 3 =>
        {
            crate::numeric::convert_integer(instruction.op, a()?, instruction.flags)
        }
        Opcode::Add | Opcode::Sub | Opcode::Mul if instruction.flags == 3 => {
            Some(match instruction.op {
                Opcode::Add => a()?.wrapping_add(b()?),
                Opcode::Sub => a()?.wrapping_sub(b()?),
                _ => a()?.wrapping_mul(b()?),
            })
        }
        Opcode::Icmp if instruction.flags & 7 <= 3 => {
            Some(u64::from(crate::numeric::integer_compare(
                instruction.flags >> 3,
                a()?,
                b()?,
                flags_width(instruction.flags)?,
            )))
        }
        _ => None,
    }
}

fn transfer(instruction: Instr, function: &BcFunction, state: &mut State) -> Option<u64> {
    let constant = value(instruction, state);
    let (_, writes, _) = super::simplify::effect(instruction, function);
    for register in writes {
        state.set(register, None);
    }
    if matches!(
        instruction.op,
        Opcode::LoadI
            | Opcode::LoadNull
            | Opcode::Mov
            | Opcode::Bitcast
            | Opcode::Trunc
            | Opcode::ZExt
            | Opcode::SExt
            | Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Icmp
    ) {
        state.set(instruction.a as usize, constant);
    }
    constant
}

fn condition(condition: Option<&BranchCondition>, state: &mut State) -> Option<bool> {
    match condition {
        None => Some(true),
        Some(BranchCondition::Register(register)) => {
            state.get(*register as usize).map(|v| v & 1 == 1)
        }
        Some(BranchCondition::IntegerCompare {
            result,
            lhs,
            rhs,
            width,
            predicate,
        }) => {
            let matched = (*width != WidthCode::W128).then_some(()).and_then(|()| {
                state
                    .get(*lhs as usize)
                    .zip(state.get(*rhs as usize))
                    .map(|(lhs, rhs)| crate::numeric::integer_compare(*predicate, lhs, rhs, *width))
            });
            state.set(*result as usize, matched.map(u64::from));
            matched
        }
    }
}

fn edge_state<'a>(edge: &BranchTarget, state: &State<'a>) -> State<'a> {
    let mut next = state.clone();
    // The moves are parallel: every source is read before any is written.
    let sources = edge
        .moves
        .iter()
        .map(|movement| state.get(movement.source as usize))
        .collect::<Vec<_>>();
    for (movement, constant) in edge.moves.iter().zip(sources) {
        next.set(movement.destination as usize, constant);
    }
    next
}

/// Joins `next` into what a block has seen: a register stays known only
/// while every way in agrees on its constant.
fn merge<'a>(slot: &mut Option<State<'a>>, next: State<'a>) -> bool {
    let Some(current) = slot else {
        *slot = Some(next);
        return true;
    };
    let before = current.known.len();
    let mut others = next.known.iter().peekable();
    current.known.retain(|&(register, constant)| {
        while others.next_if(|&&(other, _)| other < register).is_some() {}
        others.next_if(|&&(other, value)| other == register && value == constant).is_some()
    });
    current.known.len() != before
}

pub(super) fn propagate(function: &mut BcFunction) {
    if function.code.is_empty()
        || super::simplify::returns_twice(function)
        || function.branches.is_empty()
        || function.code.iter().any(|word| {
            matches!(
                Instr::decode(*word).unwrap().op,
                Opcode::Jmp | Opcode::Jif | Opcode::Jifn
            )
        })
    {
        return;
    }
    let boundaries = super::blocks::starts(function)
        .into_iter()
        .enumerate()
        .filter_map(|(pc, start)| start.then_some(pc))
        .collect::<Vec<_>>();
    let blocks = boundaries.len() - 1;
    let mut owners = vec![blocks; function.code.len() + 1];
    for (block, range) in boundaries.windows(2).enumerate() {
        owners[range[0]..range[1]].fill(block);
    }
    let instructions = function
        .code
        .iter()
        .map(|word| Instr::decode(*word).unwrap())
        .collect::<Vec<_>>();
    let tracked = tracked(function, &instructions);
    let mut incoming = vec![None; blocks];
    incoming[0] = Some(State::new(&tracked));
    let mut queue = VecDeque::from([0]);
    let mut queued = vec![false; blocks];
    queued[0] = true;
    while let Some(block) = queue.pop_front() {
        queued[block] = false;
        let mut state = incoming[block].as_ref().unwrap().clone();
        let range = boundaries[block]..boundaries[block + 1];
        for &instruction in &instructions[range.clone()] {
            transfer(instruction, function, &mut state);
        }
        let last = instructions[range.end - 1];
        let mut successors = Vec::new();
        if matches!(last.op, Opcode::Br | Opcode::BrIf | Opcode::BrCmp) {
            let branch = &function.branches[last.bx() as usize];
            let matched = condition(branch.condition.as_ref(), &mut state);
            if matched != Some(false) {
                successors.push((
                    branch.then_edge.target,
                    edge_state(&branch.then_edge, &state),
                ));
            }
            if branch.condition.is_some() && matched != Some(true) {
                if let Some(edge) = &branch.else_edge {
                    successors.push((edge.target, edge_state(edge, &state)));
                } else {
                    successors.push((range.end, state));
                }
            }
        } else if !matches!(
            last.op,
            Opcode::Ret | Opcode::RetV | Opcode::Fail | Opcode::Trap | Opcode::Unreach
        ) {
            successors.push((range.end, state));
        }
        for (target, state) in successors {
            let successor = owners[target];
            if successor < blocks && merge(&mut incoming[successor], state) && !queued[successor] {
                queue.push_back(successor);
                queued[successor] = true;
            }
        }
    }
    let mut offsets = vec![0; instructions.len() + 1];
    let mut code = Vec::with_capacity(instructions.len());
    for block in 0..blocks {
        let range = boundaries[block]..boundaries[block + 1];
        let Some(mut state) = incoming[block].clone() else {
            offsets[range].fill(code.len());
            continue;
        };
        for pc in range {
            offsets[pc] = code.len();
            let mut instruction = instructions[pc];
            let constant = transfer(instruction, function, &mut state);
            if matches!(instruction.op, Opcode::BrIf | Opcode::BrCmp) {
                let mut branch = function.branches[instruction.bx() as usize].clone();
                if let Some(matched) = condition(branch.condition.as_ref(), &mut state) {
                    if let Some(BranchCondition::IntegerCompare { result, .. }) = branch.condition {
                        code.push(
                            Instr::from_bx(Opcode::LoadI, result, i32::from(matched)).encode(),
                        );
                    }
                    if !matched {
                        let Some(edge) = branch.else_edge.take() else {
                            continue;
                        };
                        branch.then_edge = edge;
                    }
                    branch.condition = None;
                    branch.else_edge = None;
                    let index = function.branches.len();
                    function.branches.push(branch);
                    instruction = Instr::from_bx(Opcode::Br, 0, index as i32);
                }
            } else if !matches!(instruction.op, Opcode::LoadI | Opcode::LoadNull)
                && let Some(constant) = constant
                && let Ok(constant) = i32::try_from(constant as i64)
            {
                instruction = Instr::from_bx(Opcode::LoadI, instruction.a, constant);
            }
            code.push(instruction.encode());
        }
    }
    offsets[instructions.len()] = code.len();
    for branch in &mut function.branches {
        branch.then_edge.target = offsets[branch.then_edge.target];
        if let Some(edge) = &mut branch.else_edge {
            edge.target = offsets[edge.target];
        }
    }
    function.code = code;
    super::simplify::fallthroughs(function);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{BranchSite, RegisterMove};
    use crate::{Program, run};

    fn function(code: Vec<Instr>, branches: Vec<BranchSite>) -> BcFunction {
        BcFunction {
            name: "constants".into(),
            register_count: 5,
            param_count: 1,
            returns_value: true,
            code: code.into_iter().map(Instr::encode).collect(),
            branches,
            call_sites: vec![],
            allocations: vec![],
        }
    }
    fn edge(target: usize) -> BranchTarget {
        BranchTarget {
            target,
            moves: vec![],
        }
    }
    fn program(function: BcFunction) -> Program {
        Program {
            functions: vec![function],
            consts: vec![],
            imports: vec![],
            globals: vec![],
            initializers: vec![],
        }
    }
    fn equivalent(mut function: BcFunction, args: &[u64]) -> BcFunction {
        let before = program(function.clone());
        propagate(&mut function);
        let after = program(function.clone());
        for &arg in args {
            assert_eq!(run(&before, 0, &[arg]), run(&after, 0, &[arg]));
        }
        function
    }

    #[test]
    fn constant_compare_preserves_result_and_parallel_edge_moves() {
        for predicate in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
            let mut then_edge = edge(4);
            then_edge.moves = vec![
                RegisterMove {
                    source: 1,
                    destination: 2,
                },
                RegisterMove {
                    source: 2,
                    destination: 1,
                },
            ];
            let function = function(
                vec![
                    Instr::from_bx(Opcode::LoadI, 1, 7),
                    Instr::from_bx(Opcode::LoadI, 2, 9),
                    Instr::from_bx(Opcode::BrCmp, 0, 0),
                    Instr::abc(Opcode::Trap, 0, 0, 0),
                    Instr::abc(Opcode::Add, 3, 1, 2).with_flags(3),
                    Instr::abc(Opcode::Add, 3, 3, 4).with_flags(3),
                    Instr::abc(Opcode::RetV, 3, 0, 0),
                ],
                vec![BranchSite {
                    condition: Some(BranchCondition::IntegerCompare {
                        result: 4,
                        lhs: 1,
                        rhs: 2,
                        width: WidthCode::W64,
                        predicate,
                    }),
                    then_edge,
                    else_edge: Some(edge(4)),
                }],
            );
            let after = equivalent(function, &[0]);
            assert!(
                after
                    .code
                    .iter()
                    .all(|word| Instr::decode(*word).unwrap().op != Opcode::BrCmp)
            );
        }
    }

    #[test]
    fn loop_backedges_widen_constants_and_keep_parameter_branches() {
        let function = function(
            vec![
                Instr::from_bx(Opcode::LoadI, 1, 0),
                Instr::from_bx(Opcode::LoadI, 2, 1),
                Instr::from_bx(Opcode::BrCmp, 0, 0),
                Instr::abc(Opcode::Add, 1, 1, 2).with_flags(3),
                Instr::from_bx(Opcode::Br, 0, 1),
                Instr::abc(Opcode::RetV, 1, 0, 0),
            ],
            vec![
                BranchSite {
                    condition: Some(BranchCondition::IntegerCompare {
                        result: 3,
                        lhs: 1,
                        rhs: 0,
                        width: WidthCode::W64,
                        predicate: 6,
                    }),
                    then_edge: edge(3),
                    else_edge: Some(edge(5)),
                },
                BranchSite {
                    condition: None,
                    then_edge: edge(2),
                    else_edge: None,
                },
            ],
        );
        let after = equivalent(function, &[0, 1, 2, 7, 29]);
        assert!(
            after
                .code
                .iter()
                .any(|word| Instr::decode(*word).unwrap().op == Opcode::BrCmp)
        );
    }

    #[test]
    fn differing_predecessors_do_not_fold_a_joined_value() {
        let function = function(
            vec![
                Instr::from_bx(Opcode::BrIf, 0, 0),
                Instr::from_bx(Opcode::LoadI, 1, 10),
                Instr::from_bx(Opcode::Br, 0, 1),
                Instr::from_bx(Opcode::LoadI, 1, 20),
                Instr::from_bx(Opcode::Br, 0, 2),
                Instr::from_bx(Opcode::LoadI, 2, 1),
                Instr::abc(Opcode::Add, 3, 1, 2).with_flags(3),
                Instr::abc(Opcode::RetV, 3, 0, 0),
            ],
            vec![
                BranchSite {
                    condition: Some(BranchCondition::Register(0)),
                    then_edge: edge(1),
                    else_edge: Some(edge(3)),
                },
                BranchSite {
                    condition: None,
                    then_edge: edge(5),
                    else_edge: None,
                },
                BranchSite {
                    condition: None,
                    then_edge: edge(5),
                    else_edge: None,
                },
            ],
        );
        equivalent(function, &[0, 1, 2, 3]);
    }
}
