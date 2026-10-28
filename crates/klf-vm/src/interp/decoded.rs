mod add_load;
mod alloca_group;
mod alloca_pair;
mod constants;
mod enum_switch;
mod float_pair;
mod immediate_binary;
mod instructions;
mod load_move;
mod move_pair;
mod simple_branch;
mod tail_call;

use crate::bytecode::{Instr, WidthCode};
use crate::program::{BranchCondition, BranchSite, BranchTarget, Program, RegisterMove};
use add_load::decode_add_loads;
pub(super) use alloca_group::DecodedAllocaGroup;
use alloca_group::decode_alloca_groups;
use alloca_pair::decode_alloca_pairs;
use enum_switch::decode_enum_switches;
use float_pair::decode_float_pairs;
use immediate_binary::decode_immediate_binaries;
use instructions::decode_instruction;
use load_move::decode_load_moves;
use move_pair::decode_move_pairs;
use tail_call::decode_tail_calls;

pub(super) fn join_u32(low: u16, high: u16) -> u32 {
    u32::from(low) | (u32::from(high) << 16)
}

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum DecodedInstr {
    LoadConstant {
        a: u16,
        index: u32,
    },
    FloatMultiplyAdd(u32),
    FAdd64 {
        a: u16,
        b: u16,
        c: u16,
    },
    FSub64 {
        a: u16,
        b: u16,
        c: u16,
    },
    FMul64 {
        a: u16,
        b: u16,
        c: u16,
    },
    FDiv64 {
        a: u16,
        b: u16,
        c: u16,
    },
    Add64 {
        a: u16,
        b: u16,
        c: u16,
    },
    Sub64 {
        a: u16,
        b: u16,
        c: u16,
    },
    Mul64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpEq64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpNe64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpSlt64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpSle64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpSgt64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpSge64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpUlt64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpUle64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpUgt64 {
        a: u16,
        b: u16,
        c: u16,
    },
    IcmpUge64 {
        a: u16,
        b: u16,
        c: u16,
    },
    Mov {
        a: u16,
        b: u16,
    },
    LoadI {
        a: u16,
        low: u16,
        high: u16,
    },
    LoadNull(u16),
    IntegerCast {
        a: u16,
        b: u16,
        control: u16,
    },
    Select {
        a: u16,
        condition: u16,
        if_true: u16,
    },
    FloatBinary(u32),
    FloatUnary(u32),
    FloatCompare(u32),
    FloatFma(u32),
    Return,
    ReturnValue(u16),
    Nop,
    Load8 {
        a: u16,
        b: u16,
    },
    Load16 {
        a: u16,
        b: u16,
    },
    Load32 {
        a: u16,
        b: u16,
    },
    Load64 {
        a: u16,
        b: u16,
    },
    Store8 {
        a: u16,
        b: u16,
    },
    Store16 {
        a: u16,
        b: u16,
    },
    Store32 {
        a: u16,
        b: u16,
    },
    Store64 {
        a: u16,
        b: u16,
    },
    Alloca {
        a: u16,
        index_low: u16,
        index_high: u16,
    },
    Jmp {
        low: u16,
        high: u16,
    },
    Jif {
        condition: u16,
        low: u16,
        high: u16,
    },
    Jifn {
        condition: u16,
        low: u16,
        high: u16,
    },
    Branch {
        low: u16,
        high: u16,
    },
    BranchIf {
        low: u16,
        high: u16,
    },
    BranchCompare {
        low: u16,
        high: u16,
    },
    LoadIBinary64(u32),
    LoadIBinaryMemory64(u32),
    AllocaPair(u32),
    AllocaGroup(u32),
    MovePair(u32),
    AddLoad(u32),
    Load8Move {
        load_a: u16,
        load_b: u16,
        move_a: u16,
    },
    Load16Move {
        load_a: u16,
        load_b: u16,
        move_a: u16,
    },
    Load32Move {
        load_a: u16,
        load_b: u16,
        move_a: u16,
    },
    Load64Move {
        load_a: u16,
        load_b: u16,
        move_a: u16,
    },
    TailCall {
        low: u16,
        high: u16,
    },
    End,
    EnumSwitch(u32),
    Call {
        low: u16,
        high: u16,
    },
    GlobalA {
        a: u16,
        index_low: u16,
        index_high: u16,
    },
    Memory {
        low: u16,
        high: u16,
    },
    Other {
        low: u16,
        high: u16,
    },
    Invalid,
}

pub(crate) struct DecodedFunction {
    pub(crate) code: Vec<DecodedInstr>,
    pub(crate) constants: Vec<u64>,
    pub(crate) float_pairs: Vec<FloatMultiplyAdd>,
    pub(crate) raw_instructions: Vec<Instr>,
    pub(crate) branches: Vec<Option<DecodedBranchSite>>,
    pub(crate) max_branch_moves: usize,
    pub(crate) loop_plans: Vec<DecodedLoopPlan>,
    pub(crate) immediate_binaries: Vec<DecodedImmediateBinary>,
    pub(crate) move_pairs: Vec<DecodedMovePair>,
    pub(crate) add_loads: Vec<DecodedAddLoad>,
    pub(crate) enum_switches: Vec<DecodedEnumSwitch>,
    pub(crate) alloca_pairs: Vec<DecodedAllocaPair>,
    pub(crate) alloca_groups: Vec<DecodedAllocaGroup>,
}

#[derive(Clone, Copy)]
pub(crate) struct DecodedAllocaPair {
    pub(super) first_a: u16,
    pub(super) first_size: u32,
    pub(super) first_align: u32,
    pub(super) second_a: u16,
    pub(super) second_size: u32,
    pub(super) second_align: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct DecodedImmediateBinary {
    pub(super) memory: Option<DecodedInstr>,
    pub(super) constant_register: u16,
    pub(super) low: u16,
    pub(super) high: u16,
    pub(super) kind: ImmediateBinaryKind,
    pub(super) a: u16,
    pub(super) b: u16,
    pub(super) c: u16,
}

#[derive(Clone, Copy)]
pub(super) enum ImmediateBinaryKind {
    Add,
    Mul,
}

#[derive(Clone, Copy)]
pub(crate) struct FloatMultiplyAdd {
    pub(super) multiply_a: u16,
    pub(super) multiply_b: u16,
    pub(super) multiply_c: u16,
    pub(super) add_a: u16,
    pub(super) add_b: u16,
    pub(super) add_c: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct DecodedMovePair {
    pub(super) first_a: u16,
    pub(super) first_b: u16,
    pub(super) second_a: u16,
    pub(super) second_b: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct DecodedAddLoad {
    pub(super) add_a: u16,
    pub(super) add_b: u16,
    pub(super) add_c: u16,
    pub(super) load_a: u16,
    pub(super) load_width: u8,
}

pub(crate) struct DecodedEnumSwitch {
    pub(super) tag: u16,
    pub(super) width: WidthCode,
    pub(super) cases: Vec<DecodedEnumSwitchCase>,
    pub(super) default_pc: usize,
}

pub(super) struct DecodedEnumSwitchCase {
    pub(super) constant_register: u16,
    pub(super) result_register: u16,
    pub(super) value: u64,
    pub(super) edge: DecodedBranchTarget,
}

#[derive(Clone)]
pub(crate) struct DecodedBranchSite {
    pub(super) condition: Option<DecodedCondition>,
    pub(super) then_edge: DecodedBranchTarget,
    pub(super) else_edge: Option<DecodedBranchTarget>,
    pub(super) loop_plan: Option<u32>,
}

#[derive(Clone, Copy)]
pub(super) enum DecodedCondition {
    Register(u16),
    IntegerCompare {
        result: u16,
        lhs: u16,
        rhs: u16,
        width: WidthCode,
        predicate: u8,
    },
}

#[derive(Clone)]
pub(super) struct DecodedBranchTarget {
    pub(super) target: usize,
    pub(super) moves: Vec<RegisterMove>,
    pub(super) needs_scratch: bool,
}

pub(crate) struct DecodedLoopPlan {
    pub(super) condition: DecodedCondition,
    pub(super) body_when_matched: bool,
    pub(super) body_edge: DecodedBranchTarget,
    pub(super) exit_edge: DecodedBranchTarget,
    pub(super) body: Vec<DecodedInstr>,
    pub(super) back_edge: DecodedBranchTarget,
}

pub(crate) fn decode_program(program: &Program) -> Vec<DecodedFunction> {
    program
        .functions
        .iter()
        .map(|function| {
            let mut branches = function
                .branches
                .iter()
                .map(|site| decode_branch_site(site, function.register_count))
                .collect::<Vec<_>>();
            let max_branch_moves = branches
                .iter()
                .flatten()
                .flat_map(|site| {
                    std::iter::once(&site.then_edge)
                        .chain(site.else_edge.iter())
                        .map(|edge| edge.moves.len())
                })
                .max()
                .unwrap_or(0);
            let mut raw_instructions = Vec::new();
            let mut code: Vec<_> = function
                .code
                .iter()
                .map(|word| match Instr::decode(*word) {
                    Some(instruction) => decode_instruction(
                        instruction,
                        &branches,
                        function.register_count,
                        &function.allocations,
                        function.call_sites.len(),
                        program.globals.len(),
                        &mut raw_instructions,
                    ),
                    None => DecodedInstr::Invalid,
                })
                .collect();
            let constants = constants::decode_constants(
                &mut code,
                &raw_instructions,
                program,
                function.register_count,
            );
            let float_pairs = decode_float_pairs(&mut code);
            let enum_switches = decode_enum_switches(&mut code, &raw_instructions, &branches);
            let immediate_binaries = decode_immediate_binaries(&mut code);
            let add_loads = decode_add_loads(&mut code);
            decode_load_moves(&mut code);
            let move_pairs = decode_move_pairs(&mut code);
            decode_tail_calls(&mut code, &function.call_sites);
            let alloca_groups = decode_alloca_groups(&mut code, &function.allocations);
            let alloca_pairs = decode_alloca_pairs(&mut code, &function.allocations);
            let mut loop_plans = Vec::new();
            for head in 0..code.len() {
                let DecodedInstr::BranchCompare { low, high } = code[head] else {
                    continue;
                };
                let site = join_u32(low, high) as usize;
                let Some(plan) = find_loop_plan(head, site, &code, &branches) else {
                    continue;
                };
                let Ok(index) = u32::try_from(loop_plans.len()) else {
                    continue;
                };
                loop_plans.push(plan);
                if let Some(branch) = branches.get_mut(site).and_then(Option::as_mut) {
                    branch.loop_plan = Some(index);
                }
            }
            simple_branch::decode_simple_branches(&mut code, &branches);
            code.push(DecodedInstr::End);
            DecodedFunction {
                code,
                constants,
                float_pairs,
                raw_instructions,
                branches,
                max_branch_moves,
                loop_plans,
                immediate_binaries,
                move_pairs,
                add_loads,
                enum_switches,
                alloca_pairs,
                alloca_groups,
            }
        })
        .collect()
}

fn decode_branch_site(site: &BranchSite, register_count: u32) -> Option<DecodedBranchSite> {
    let condition = match &site.condition {
        None => None,
        Some(BranchCondition::Register(register)) if u32::from(*register) < register_count => {
            Some(DecodedCondition::Register(*register))
        }
        Some(BranchCondition::IntegerCompare {
            result,
            lhs,
            rhs,
            width,
            predicate,
        }) if [result, lhs, rhs]
            .iter()
            .all(|register| u32::from(**register) < register_count) =>
        {
            Some(DecodedCondition::IntegerCompare {
                result: *result,
                lhs: *lhs,
                rhs: *rhs,
                width: *width,
                predicate: *predicate,
            })
        }
        _ => return None,
    };
    let then_edge = decode_branch_target(&site.then_edge, register_count)?;
    let else_edge = match site.else_edge.as_ref() {
        Some(edge) => Some(decode_branch_target(edge, register_count)?),
        None => None,
    };
    Some(DecodedBranchSite {
        condition,
        then_edge,
        else_edge,
        loop_plan: None,
    })
}

fn decode_branch_target(edge: &BranchTarget, register_count: u32) -> Option<DecodedBranchTarget> {
    if edge.moves.iter().any(|movement| {
        u32::from(movement.source) >= register_count
            || u32::from(movement.destination) >= register_count
    }) {
        return None;
    }
    let needs_scratch = edge.moves.iter().enumerate().any(|(at, movement)| {
        edge.moves
            .iter()
            .enumerate()
            .any(|(other, source)| other != at && movement.destination == source.source)
    });
    Some(DecodedBranchTarget {
        target: edge.target,
        moves: edge.moves.clone(),
        needs_scratch,
    })
}

fn find_loop_plan(
    head: usize,
    site: usize,
    code: &[DecodedInstr],
    branches: &[Option<DecodedBranchSite>],
) -> Option<DecodedLoopPlan> {
    let branch = branches.get(site)?.as_ref()?;
    let condition = branch.condition.as_ref()?;
    if !matches!(condition, DecodedCondition::IntegerCompare { .. }) {
        return None;
    }
    let else_edge = branch.else_edge.as_ref()?;
    let then_body = find_numeric_loop_body(branch.then_edge.target, head, code, branches);
    let else_body = find_numeric_loop_body(else_edge.target, head, code, branches);
    match (then_body, else_body) {
        (Some((body, back_edge)), None) => Some(DecodedLoopPlan {
            condition: *condition,
            body_when_matched: true,
            body_edge: branch.then_edge.clone(),
            exit_edge: else_edge.clone(),
            body,
            back_edge,
        }),
        (None, Some((body, back_edge))) => Some(DecodedLoopPlan {
            condition: *condition,
            body_when_matched: false,
            body_edge: else_edge.clone(),
            exit_edge: branch.then_edge.clone(),
            body,
            back_edge,
        }),
        _ => None,
    }
}

fn find_numeric_loop_body(
    mut pc: usize,
    head: usize,
    code: &[DecodedInstr],
    branches: &[Option<DecodedBranchSite>],
) -> Option<(Vec<DecodedInstr>, DecodedBranchTarget)> {
    let mut body = Vec::new();
    loop {
        match *code.get(pc)? {
            instruction if is_numeric(instruction) => {
                body.push(instruction);
                pc += 1;
            }
            DecodedInstr::Branch { low, high } if !body.is_empty() => {
                let site = join_u32(low, high);
                let branch = branches.get(site as usize)?.as_ref()?;
                if branch.condition.is_none()
                    && branch.else_edge.is_none()
                    && branch.then_edge.target == head
                {
                    return Some((body, branch.then_edge.clone()));
                }
                return None;
            }
            _ => return None,
        }
    }
}

fn is_numeric(instruction: DecodedInstr) -> bool {
    matches!(
        instruction,
        DecodedInstr::Add64 { .. }
            | DecodedInstr::Sub64 { .. }
            | DecodedInstr::Mul64 { .. }
            | DecodedInstr::IcmpEq64 { .. }
            | DecodedInstr::IcmpNe64 { .. }
            | DecodedInstr::IcmpSlt64 { .. }
            | DecodedInstr::IcmpSle64 { .. }
            | DecodedInstr::IcmpSgt64 { .. }
            | DecodedInstr::IcmpSge64 { .. }
            | DecodedInstr::IcmpUlt64 { .. }
            | DecodedInstr::IcmpUle64 { .. }
            | DecodedInstr::IcmpUgt64 { .. }
            | DecodedInstr::IcmpUge64 { .. }
    )
}

#[cfg(test)]
mod tests;
