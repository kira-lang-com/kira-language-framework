use crate::bytecode::{Instr, Opcode};
use crate::program::{BcFunction, BranchCondition, BranchSite, BranchTarget, CallSite};
use klf_kir::{FunctionAttribute, Module};

struct Candidate {
    registers: u32,
    parameters: u32,
    code: Vec<Instr>,
    calls: Vec<CallSite>,
    branches: Vec<BranchSite>,
}

pub(super) fn leaves(functions: &mut [BcFunction], module: &Module) {
    let candidates = functions
        .iter()
        .zip(&module.functions)
        .enumerate()
        .map(|(index, (function, declaration))| {
            if declaration
                .attributes
                .contains(&FunctionAttribute::NoInline)
                || function.code.len() > 128
                || function.code.is_empty()
            {
                return None;
            }
            let code = function
                .code
                .iter()
                .map(|word| Instr::decode(*word))
                .collect::<Option<Vec<_>>>()?;
            if !code
                .iter()
                .any(|instruction| matches!(instruction.op, Opcode::Ret | Opcode::RetV))
            {
                return None;
            }
            for (pc, instruction) in code.iter().enumerate() {
                match instruction.op {
                    Opcode::Ret | Opcode::RetV => {}
                    Opcode::Call => {
                        let site = &function.call_sites[instruction.bx() as usize];
                        if site.fallible.is_some()
                            || (site.foreign.is_none()
                                && site.target.is_none()
                                && site.callee == index)
                        {
                            return None;
                        }
                    }
                    Opcode::Br | Opcode::BrIf | Opcode::BrCmp => {
                        let branch = &function.branches[instruction.bx() as usize];
                        if branch.then_edge.target <= pc
                            || branch.then_edge.target >= code.len()
                            || branch
                                .else_edge
                                .as_ref()
                                .is_some_and(|edge| edge.target <= pc || edge.target >= code.len())
                        {
                            return None;
                        }
                    }
                    op => {
                        operands(op)?;
                    }
                }
            }
            Some(Candidate {
                registers: function.register_count,
                parameters: function.param_count,
                code,
                calls: function.call_sites.clone(),
                branches: function.branches.clone(),
            })
        })
        .collect::<Vec<_>>();
    for function in functions {
        if super::simplify::returns_twice(function) {
            continue;
        }
        let base = function.register_count;
        let original_branches = function.branches.len();
        let mut code = Vec::with_capacity(function.code.len());
        let mut offsets = Vec::with_capacity(function.code.len() + 1);
        for word in &function.code {
            offsets.push(code.len());
            let instruction = Instr::decode(*word).expect("lowered opcode");
            let expansion = if instruction.op == Opcode::Call {
                function
                    .call_sites
                    .get(instruction.bx() as usize)
                    .and_then(|site| {
                        if site.foreign.is_some()
                            || site.target.is_some()
                            || site.fallible.is_some()
                        {
                            return None;
                        }
                        let candidate = candidates.get(site.callee)?.as_ref()?;
                        if candidate.parameters != u32::from(site.arg_count)
                            || base + candidate.registers > u32::from(u16::MAX)
                            || function.call_sites.len() + candidate.calls.len()
                                > usize::from(u16::MAX)
                            || function.branches.len()
                                + candidate.branches.len()
                                + candidate.code.len()
                                > usize::from(u16::MAX)
                        {
                            return None;
                        }
                        Some((*site, candidate))
                    })
            } else {
                None
            };
            if let Some((site, candidate)) = expansion {
                expand(
                    &mut code,
                    &site,
                    candidate,
                    base as u16,
                    &mut function.call_sites,
                    &mut function.branches,
                );
                function.register_count = function.register_count.max(base + candidate.registers);
            } else {
                code.push(*word);
            }
        }
        offsets.push(code.len());
        for branch in &mut function.branches[..original_branches] {
            branch.then_edge.target = offsets[branch.then_edge.target];
            if let Some(edge) = &mut branch.else_edge {
                edge.target = offsets[edge.target];
            }
        }
        function.code = code;
    }
}

fn expand(
    code: &mut Vec<u64>,
    site: &CallSite,
    candidate: &Candidate,
    base: u16,
    calls: &mut Vec<CallSite>,
    branches: &mut Vec<BranchSite>,
) {
    for argument in 0..site.arg_count {
        code.push(Instr::abc(Opcode::Mov, base + argument, site.first_arg + argument, 0).encode());
    }
    let controlled = !candidate.branches.is_empty()
        || candidate.code[..candidate.code.len() - 1]
            .iter()
            .any(|instruction| matches!(instruction.op, Opcode::Ret | Opcode::RetV));
    let mut offsets = Vec::with_capacity(candidate.code.len() + 1);
    let mut position = code.len();
    for instruction in &candidate.code {
        offsets.push(position);
        position += if matches!(instruction.op, Opcode::Ret | Opcode::RetV) {
            usize::from(instruction.op == Opcode::RetV && site.has_result) + usize::from(controlled)
        } else {
            1
        };
    }
    offsets.push(position);
    let end = position;
    let branch_base = branches.len();
    for branch in &candidate.branches {
        let mut branch = branch.clone();
        match &mut branch.condition {
            Some(BranchCondition::Register(register)) => *register += base,
            Some(BranchCondition::IntegerCompare {
                result, lhs, rhs, ..
            }) => {
                *result += base;
                *lhs += base;
                *rhs += base;
            }
            None => {}
        }
        for edge in std::iter::once(&mut branch.then_edge).chain(branch.else_edge.iter_mut()) {
            edge.target = offsets[edge.target];
            for movement in &mut edge.moves {
                movement.source += base;
                movement.destination += base;
            }
        }
        branches.push(branch);
    }
    for instruction in &candidate.code {
        let mut instruction = *instruction;
        if matches!(instruction.op, Opcode::Ret | Opcode::RetV) {
            if instruction.op == Opcode::RetV && site.has_result {
                code.push(Instr::abc(Opcode::Mov, site.result, base + instruction.a, 0).encode());
            }
            if controlled {
                let index = branches.len() as i32;
                branches.push(BranchSite {
                    condition: None,
                    then_edge: BranchTarget {
                        target: end,
                        moves: vec![],
                    },
                    else_edge: None,
                });
                code.push(Instr::from_bx(Opcode::Br, 0, index).encode());
            }
            continue;
        }
        if instruction.op == Opcode::Call {
            let mut nested = candidate.calls[instruction.bx() as usize];
            nested.first_arg += base;
            nested.result += base;
            nested.target = nested.target.map(|register| register + base);
            let index = calls.len() as i32;
            calls.push(nested);
            code.push(Instr::from_bx(Opcode::Call, nested.result, index).encode());
            continue;
        }
        if matches!(instruction.op, Opcode::Br | Opcode::BrIf | Opcode::BrCmp) {
            code.push(
                Instr::from_bx(instruction.op, 0, branch_base as i32 + instruction.bx()).encode(),
            );
            continue;
        }
        let mask = operands(instruction.op).expect("eligible helper opcode");
        if mask & 1 != 0 {
            instruction.a += base;
        }
        if mask & 2 != 0 {
            instruction.b += base;
        }
        if mask & 4 != 0 {
            instruction.c += base;
        }
        code.push(instruction.encode());
    }
    debug_assert_eq!(code.len(), end);
}

fn operands(op: Opcode) -> Option<u8> {
    match op {
        Opcode::Nop | Opcode::Fence | Opcode::Trap | Opcode::Unreach => Some(0),
        Opcode::HfreeR | Opcode::Fail | Opcode::AccEnd => Some(1),
        Opcode::AccBegin => Some(1),
        Opcode::HallocR => Some(7),
        Opcode::LoadK | Opcode::LoadI | Opcode::LoadNull | Opcode::LeaFn | Opcode::GlobalA => {
            Some(1)
        }
        Opcode::Mov
        | Opcode::Not
        | Opcode::Neg
        | Opcode::Ctlz
        | Opcode::Cttz
        | Opcode::Ctpop
        | Opcode::Bswap
        | Opcode::Bitrev
        | Opcode::FNeg
        | Opcode::FAbs
        | Opcode::FSqrt
        | Opcode::FFloor
        | Opcode::FCeil
        | Opcode::FTrunc
        | Opcode::FRound
        | Opcode::Trunc
        | Opcode::ZExt
        | Opcode::SExt
        | Opcode::FpTrunc
        | Opcode::FpExt
        | Opcode::SiToFp
        | Opcode::UiToFp
        | Opcode::FpToSi
        | Opcode::FpToUi
        | Opcode::Bitcast
        | Opcode::Load
        | Opcode::Store => Some(3),
        Opcode::Select
        | Opcode::Add
        | Opcode::Sub
        | Opcode::Mul
        | Opcode::SDiv
        | Opcode::UDiv
        | Opcode::SRem
        | Opcode::URem
        | Opcode::And
        | Opcode::Or
        | Opcode::Xor
        | Opcode::Shl
        | Opcode::LShr
        | Opcode::AShr
        | Opcode::RotL
        | Opcode::RotR
        | Opcode::Icmp
        | Opcode::FAdd
        | Opcode::FSub
        | Opcode::FMul
        | Opcode::FDiv
        | Opcode::FRem
        | Opcode::FMin
        | Opcode::FMax
        | Opcode::FFma
        | Opcode::FCmp
        | Opcode::PtrAdd
        | Opcode::PtrDiff => Some(7),
        _ => None,
    }
}
