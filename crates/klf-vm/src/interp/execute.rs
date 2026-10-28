#![allow(unsafe_code)]

mod helpers;
mod immediate;
mod memory;
mod numeric;
mod raw;
mod superinstructions;
use super::decoded::{DecodedCondition, DecodedFunction, DecodedInstr, join_u32};
use super::frames::{
    callee_frame, can_tail_call, finish_return, make_entry_frame, restore_pool, tail_call_frame,
    unwind,
};
use super::step::memory_step;
use super::{
    CANCELLED, Capabilities, EvaluationBudget, Flow, MAX_DEPTH, Machine, Outcome,
    ResolvedCapabilities,
};
use super::{foreign_call, native_import};
use crate::program::{ImportKind, Program};
use helpers::{
    allocation_fits, apply_branch_edge, branch_at, enum_switch_equal, join_i32, read_register,
    spend_branch_moves, spend_step, write_register,
};
use memory::{execute_alloca_group, execute_allocation};
use numeric::{
    execute_float_instruction, execute_integer_cast, execute_numeric_loop, execute_select,
    float_raw_index, integer_compare_fast, integer_compare_operands,
};
use raw::{Raw, execute_raw};
use superinstructions::{
    execute_add_load, execute_alloca_pair, execute_load_move_instruction, execute_move_pair,
};

pub(super) fn execute<const METERED: bool>(
    machine: &mut Machine,
    entry: usize,
    arguments: &[u64],
    capabilities: &Capabilities,
    resolved_capabilities: &ResolvedCapabilities<'_>,
    mut budget: Option<&mut EvaluationBudget>,
) -> Outcome {
    let version = machine.version;
    if entry >= machine.version_code(version).0.functions.len() {
        return Outcome::Trap("KLF-VM-0001".to_owned());
    }
    // The code outlives this call: version 0 is the machine's own borrow, and
    // a pushed version stays in its slot until `reap`, which runs only once
    // no frame has entered it.
    let (program, decoded_all) = {
        let (program, decoded) = machine.version_code(version);
        let program: *const Program = std::ptr::from_ref(program);
        let decoded: *const [DecodedFunction] = std::ptr::from_ref(decoded);
        // SAFETY: both point into data the machine owns for the whole run, and
        // `reap` cannot drop this version while `running` counts this call.
        unsafe { (&*program, &*decoded) }
    };
    #[cfg(feature = "execution-profile")]
    let profile_start = (machine.profile.total, std::time::Instant::now());
    let mut frame_pool = std::mem::take(&mut machine.frame_pool);
    machine.enter(version);
    let mut active = make_entry_frame(
        program,
        decoded_all,
        version,
        entry,
        arguments,
        frame_pool.pop(),
        &machine.heap,
    );
    let mut callers = Vec::new();
    let cancellation = machine.cancellation.clone();
    let cancelled = || {
        cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
    };
    let outcome = (|| {
        let decoded_functions = decoded_all.as_ptr();
        let mut function = &program.functions[entry];
        // SAFETY: decoding remains immutable, including across native callbacks;
        // entry, callees and restored caller indices are validated before use.
        let mut decoded = unsafe { &*decoded_functions.add(entry) };
        let mut pc = 0usize;
        // The polls of a cancellable run (`KLF-EDITOR.md` section 8.6): at
        // every call, and at every jump that does not move forward, which is
        // what closes a loop.
        macro_rules! trap {
            ($code:expr) => {{
                let outcome = Outcome::Trap($code.to_owned());
                return unwind(&mut active, &mut callers, &mut machine.heap, outcome);
            }};
        }
        macro_rules! poll {
            () => {
                if cancelled() {
                    trap!(CANCELLED);
                }
            };
        }
        macro_rules! jump {
            ($target:expr) => {{
                let target = $target;
                if target <= pc {
                    poll!();
                }
                target
            }};
        }
        macro_rules! scalar_memory {
            ($bytes:literal, $load:literal, $a:ident, $b:ident) => {
                if let Some(outcome) = memory::execute_decoded_memory::<$bytes, $load>(
                    $a,
                    $b,
                    &mut active,
                    &mut callers,
                    &mut machine.heap,
                    &mut pc,
                ) {
                    return outcome;
                }
            };
        }
        macro_rules! float64 {
            ($a:ident, $b:ident, $c:ident, $operator:tt) => {{
                let lhs = f64::from_bits(read_register(&active.registers, $b));
                let rhs = f64::from_bits(read_register(&active.registers, $c));
                write_register(&mut active.registers, $a, (lhs $operator rhs).to_bits());
                pc += 1;
            }};
        }
        loop {
            if METERED
                && let Some(code) = budget
                    .as_deref_mut()
                    .and_then(|budget| spend_step(&machine.heap, budget))
            {
                trap!(code);
            }
            // SAFETY: each decoded stream ends in End and every control-flow
            // target is clamped to that sentinel; sequential advances stop there.
            let instruction = unsafe { *decoded.code.get_unchecked(pc) };
            #[cfg(feature = "execution-profile")]
            machine.profile.step(active.function, pc);
            match instruction {
                DecodedInstr::LoadConstant { a, index } => {
                    write_register(&mut active.registers, a, decoded.constants[index as usize]);
                    pc += 1;
                }
                DecodedInstr::FloatMultiplyAdd(index) => numeric::execute_float_pair::<METERED>(
                    decoded.float_pairs[index as usize],
                    &mut active.registers,
                    &mut pc,
                ),
                DecodedInstr::FAdd64 { a, b, c } => float64!(a, b, c, +),
                DecodedInstr::FSub64 { a, b, c } => float64!(a, b, c, -),
                DecodedInstr::FMul64 { a, b, c } => float64!(a, b, c, *),
                DecodedInstr::FDiv64 { a, b, c } => float64!(a, b, c, /),
                DecodedInstr::Mov { a, b } => {
                    let value = read_register(&active.registers, b);
                    write_register(&mut active.registers, a, value);
                    pc += 1;
                }
                DecodedInstr::MovePair(index) => {
                    if !execute_move_pair::<METERED>(
                        decoded.move_pairs.get(index as usize).copied(),
                        &mut active.registers,
                        &mut pc,
                    ) {
                        trap!("KLF-VM-0001");
                    }
                }
                DecodedInstr::LoadI { a, low, high } => {
                    write_register(&mut active.registers, a, join_i32(low, high) as u64);
                    pc += 1;
                }
                DecodedInstr::LoadIBinary64(index) => {
                    if let Some(outcome) = immediate::execute::<METERED, false>(
                        decoded.immediate_binaries.get(index as usize).copied(),
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::LoadIBinaryMemory64(index) => {
                    if let Some(outcome) = immediate::execute::<METERED, true>(
                        decoded.immediate_binaries.get(index as usize).copied(),
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::LoadNull(a) => {
                    write_register(&mut active.registers, a, 0);
                    pc += 1;
                }
                DecodedInstr::IntegerCast { a, b, control } => {
                    execute_integer_cast(a, b, control, &mut active.registers);
                    pc += 1;
                }
                DecodedInstr::Select {
                    a,
                    condition,
                    if_true,
                } => {
                    execute_select(a, condition, if_true, &mut active.registers);
                    pc += 1;
                }
                instruction @ (DecodedInstr::Return | DecodedInstr::ReturnValue(_)) => {
                    let value = match instruction {
                        DecodedInstr::ReturnValue(source) => {
                            Some(read_register(&active.registers, source))
                        }
                        _ => None,
                    };
                    if let Some(outcome) = finish_return(
                        &mut active,
                        &mut callers,
                        &mut frame_pool,
                        &mut machine.heap,
                        &mut pc,
                        value,
                    ) {
                        return outcome;
                    }
                    function = &program.functions[active.function];
                    decoded = unsafe { &*decoded_functions.add(active.function) };
                }
                DecodedInstr::Nop => pc += 1,
                DecodedInstr::EnumSwitch(index) => {
                    let Some(switch) = decoded.enum_switches.get(index as usize) else {
                        trap!("KLF-VM-0001");
                    };
                    if METERED {
                        let first = &switch.cases[0];
                        write_register(&mut active.registers, first.constant_register, first.value);
                        pc += 1;
                        continue;
                    }
                    let tag = read_register(&active.registers, switch.tag);
                    let mut matched = false;
                    for case in &switch.cases {
                        write_register(&mut active.registers, case.constant_register, case.value);
                        let equal = enum_switch_equal(tag, case.value, switch.width);
                        write_register(
                            &mut active.registers,
                            case.result_register,
                            u64::from(equal),
                        );
                        if equal {
                            if let Some(code) = spend_branch_moves::<METERED>(
                                &machine.heap,
                                &mut budget,
                                case.edge.moves.len(),
                            ) {
                                trap!(code);
                            }
                            apply_branch_edge(&mut active, &case.edge);
                            pc = jump!(case.edge.target.min(decoded.code.len() - 1));
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        pc = jump!(switch.default_pc.min(decoded.code.len() - 1));
                    }
                }
                DecodedInstr::Add64 { a, b, c } => {
                    let value = read_register(&active.registers, b)
                        .wrapping_add(read_register(&active.registers, c));
                    write_register(&mut active.registers, a, value);
                    pc += 1;
                }
                DecodedInstr::AddLoad(index) => {
                    if let Some(outcome) = execute_add_load::<METERED>(
                        decoded.add_loads.get(index as usize).copied(),
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                instruction @ (DecodedInstr::Load8Move { .. }
                | DecodedInstr::Load16Move { .. }
                | DecodedInstr::Load32Move { .. }
                | DecodedInstr::Load64Move { .. }) => {
                    if let Some(outcome) = execute_load_move_instruction::<METERED>(
                        instruction,
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::Sub64 { a, b, c } => {
                    let value = read_register(&active.registers, b)
                        .wrapping_sub(read_register(&active.registers, c));
                    write_register(&mut active.registers, a, value);
                    pc += 1;
                }
                DecodedInstr::Mul64 { a, b, c } => {
                    let value = read_register(&active.registers, b)
                        .wrapping_mul(read_register(&active.registers, c));
                    write_register(&mut active.registers, a, value);
                    pc += 1;
                }
                instruction @ (DecodedInstr::IcmpEq64 { .. }
                | DecodedInstr::IcmpNe64 { .. }
                | DecodedInstr::IcmpSlt64 { .. }
                | DecodedInstr::IcmpSle64 { .. }
                | DecodedInstr::IcmpSgt64 { .. }
                | DecodedInstr::IcmpSge64 { .. }
                | DecodedInstr::IcmpUlt64 { .. }
                | DecodedInstr::IcmpUle64 { .. }
                | DecodedInstr::IcmpUgt64 { .. }
                | DecodedInstr::IcmpUge64 { .. }) => {
                    let (a, b, c, predicate) = integer_compare_operands(instruction)
                        .expect("matched an integer comparison instruction");
                    let left = read_register(&active.registers, b);
                    let right = read_register(&active.registers, c);
                    let matched = integer_compare_fast(
                        predicate,
                        left,
                        right,
                        crate::bytecode::WidthCode::W64,
                    );
                    write_register(&mut active.registers, a, u64::from(matched));
                    pc += 1;
                }
                instruction @ (DecodedInstr::FloatBinary(_)
                | DecodedInstr::FloatUnary(_)
                | DecodedInstr::FloatCompare(_)
                | DecodedInstr::FloatFma(_)) => {
                    let Some(raw_index) = float_raw_index(instruction) else {
                        unreachable!("matched a predecoded float instruction");
                    };
                    let Some(raw) = decoded.raw_instructions.get(raw_index as usize).copied()
                    else {
                        trap!("KLF-VM-0001");
                    };
                    execute_float_instruction(raw, &mut active.registers[..active.register_count]);
                    pc += 1;
                }
                DecodedInstr::Jmp { low, high } => {
                    pc = jump!((join_i32(low, high) as usize).min(decoded.code.len() - 1));
                }
                DecodedInstr::Jif {
                    condition,
                    low,
                    high,
                } => {
                    if read_register(&active.registers, condition) & 1 == 1 {
                        pc = jump!((join_i32(low, high) as usize).min(decoded.code.len() - 1));
                    } else {
                        pc += 1;
                    }
                }
                DecodedInstr::Jifn {
                    condition,
                    low,
                    high,
                } => {
                    if read_register(&active.registers, condition) & 1 == 0 {
                        pc = jump!((join_i32(low, high) as usize).min(decoded.code.len() - 1));
                    } else {
                        pc += 1;
                    }
                }
                DecodedInstr::Branch { low, high } => {
                    let site = join_u32(low, high);
                    let branch = branch_at(decoded, site);
                    let edge = &branch.then_edge;
                    if let Some(code) =
                        spend_branch_moves::<METERED>(&machine.heap, &mut budget, edge.moves.len())
                    {
                        trap!(code);
                    }
                    apply_branch_edge(&mut active, edge);
                    pc = jump!(edge.target.min(decoded.code.len() - 1));
                }
                DecodedInstr::BranchIf { low, high } => {
                    let site = join_u32(low, high);
                    let branch = branch_at(decoded, site);
                    let Some(DecodedCondition::Register(condition)) = branch.condition.as_ref()
                    else {
                        trap!("KLF-VM-0001");
                    };
                    let edge = if read_register(&active.registers, *condition) & 1 == 1 {
                        Some(&branch.then_edge)
                    } else {
                        branch.else_edge.as_ref()
                    };
                    if let Some(edge) = edge {
                        if let Some(code) = spend_branch_moves::<METERED>(
                            &machine.heap,
                            &mut budget,
                            edge.moves.len(),
                        ) {
                            trap!(code);
                        }
                        apply_branch_edge(&mut active, edge);
                        pc = jump!(edge.target.min(decoded.code.len() - 1));
                    } else {
                        pc += 1;
                    }
                }
                DecodedInstr::BranchCompare { low, high } => {
                    let site = join_u32(low, high);
                    let branch = branch_at(decoded, site);
                    if !METERED
                        && cancellation.is_none()
                        && let Some(plan_index) = branch.loop_plan
                    {
                        let plan = &decoded.loop_plans[plan_index as usize];
                        pc = execute_numeric_loop(&mut active, plan).min(decoded.code.len() - 1);
                        continue;
                    }
                    let Some(DecodedCondition::IntegerCompare {
                        result,
                        lhs,
                        rhs,
                        width,
                        predicate,
                    }) = branch.condition.as_ref()
                    else {
                        trap!("KLF-VM-0001");
                    };
                    let matched = integer_compare_fast(
                        *predicate,
                        read_register(&active.registers, *lhs),
                        read_register(&active.registers, *rhs),
                        *width,
                    );
                    write_register(&mut active.registers, *result, u64::from(matched));
                    let edge = if matched {
                        &branch.then_edge
                    } else if let Some(edge) = branch.else_edge.as_ref() {
                        edge
                    } else {
                        pc += 1;
                        continue;
                    };
                    if let Some(code) =
                        spend_branch_moves::<METERED>(&machine.heap, &mut budget, edge.moves.len())
                    {
                        trap!(code);
                    }
                    apply_branch_edge(&mut active, edge);
                    pc = jump!(edge.target.min(decoded.code.len() - 1));
                }
                call
                @ (DecodedInstr::Call { low, high } | DecodedInstr::TailCall { low, high }) => {
                    let site_index = join_u32(low, high);
                    // SAFETY: decoding validates each call-site index.
                    let site = unsafe { *function.call_sites.get_unchecked(site_index as usize) };
                    if let Some(import) = native_import(program, site, &active.registers) {
                        if budget.is_some() {
                            // A budgeted evaluation reaches no native code, so an
                            // address taken here can never be called: it reads as
                            // 0 rather than resolving a symbol. A module's
                            // initializer takes addresses for C to call later (the
                            // array buffer ops), and evaluating a macro runs it.
                            let address = program
                                .imports
                                .get(import as usize)
                                .is_some_and(|import| matches!(import.kind, ImportKind::Address));
                            if !address {
                                trap!("KLF-VM-0024");
                            }
                            if site.has_result {
                                active.registers[site.result as usize] = 0;
                            }
                            pc += 1;
                            continue;
                        }
                        let registers = &mut active.registers[..active.register_count];
                        if let Err(outcome) = foreign_call(
                            machine,
                            site,
                            registers,
                            capabilities,
                            resolved_capabilities,
                        ) {
                            return unwind(&mut active, &mut callers, &mut machine.heap, outcome);
                        }
                        pc += 1;
                        continue;
                    }
                    poll!();
                    if matches!(call, DecodedInstr::TailCall { .. })
                        && !METERED
                        && can_tail_call(&active, &machine.heap)
                    {
                        if active.depth >= MAX_DEPTH {
                            trap!("KLF-RT-0021");
                        }
                        let Some(callee) = tail_call_frame(program, decoded_all, site, &mut active)
                        else {
                            trap!("KLF-VM-0001");
                        };
                        function = &program.functions[callee];
                        decoded = unsafe { &*decoded_functions.add(callee) };
                        pc = 0;
                        continue;
                    }
                    if active.depth >= MAX_DEPTH {
                        trap!("KLF-RT-0021");
                    }
                    let Some(frame) = callee_frame(
                        program,
                        decoded_all,
                        site,
                        &active,
                        &mut frame_pool,
                        &machine.heap,
                    ) else {
                        trap!("KLF-VM-0001");
                    };
                    active.pc = pc + 1;
                    callers.push(std::mem::replace(&mut active, frame));
                    function = &program.functions[active.function];
                    decoded = unsafe { &*decoded_functions.add(active.function) };
                    pc = 0;
                }
                DecodedInstr::GlobalA {
                    a,
                    index_low,
                    index_high,
                } => {
                    let index = join_u32(index_low, index_high);
                    // SAFETY: decoding validates each global index against the program.
                    let address = unsafe { *machine.globals.get_unchecked(index as usize) };
                    write_register(&mut active.registers, a, address);
                    pc += 1;
                }
                DecodedInstr::AllocaGroup(index) => {
                    if let Some(outcome) = execute_alloca_group::<METERED>(
                        &decoded.alloca_groups[index as usize],
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut budget,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::AllocaPair(index) => {
                    if let Some(outcome) = execute_alloca_pair::<METERED>(
                        decoded.alloca_pairs.get(index as usize).copied(),
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut budget,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::Alloca {
                    a,
                    index_low,
                    index_high,
                } => {
                    let shape_index = join_u32(index_low, index_high) as usize;
                    // SAFETY: decoding validates each allocation-shape index.
                    let shape = unsafe { function.allocations.get_unchecked(shape_index) };
                    if let Some(outcome) = execute_allocation::<METERED>(
                        a,
                        *shape,
                        &mut active,
                        &mut callers,
                        &mut machine.heap,
                        &mut budget,
                        &mut pc,
                    ) {
                        return outcome;
                    }
                }
                DecodedInstr::Load8 { a, b } => scalar_memory!(1, true, a, b),
                DecodedInstr::Load16 { a, b } => scalar_memory!(2, true, a, b),
                DecodedInstr::Load32 { a, b } => scalar_memory!(4, true, a, b),
                DecodedInstr::Load64 { a, b } => scalar_memory!(8, true, a, b),
                DecodedInstr::Store8 { a, b } => scalar_memory!(1, false, a, b),
                DecodedInstr::Store16 { a, b } => scalar_memory!(2, false, a, b),
                DecodedInstr::Store32 { a, b } => scalar_memory!(4, false, a, b),
                DecodedInstr::Store64 { a, b } => scalar_memory!(8, false, a, b),
                DecodedInstr::Memory { low, high } => {
                    let raw_index = join_u32(low, high) as usize;
                    // SAFETY: decoding assigns a valid raw-instruction index.
                    let instruction = unsafe { *decoded.raw_instructions.get_unchecked(raw_index) };
                    if budget.as_deref().is_some_and(|budget| {
                        !allocation_fits(
                            function,
                            instruction,
                            &active.registers[..active.register_count],
                            &machine.heap,
                            budget,
                        )
                    }) {
                        trap!("KLF-VM-0023");
                    }
                    match memory_step(
                        function,
                        instruction,
                        &mut active.registers[..active.register_count],
                        &mut machine.heap,
                        &mut active.stack,
                        &mut active.accesses,
                    ) {
                        Flow::Next => {
                            super::cycles::trace(machine, &active, pc, instruction);
                            pc += 1;
                        }
                        Flow::Jump(_) => unreachable!("memory ops never jump"),
                        Flow::Done(outcome) => {
                            return unwind(&mut active, &mut callers, &mut machine.heap, outcome);
                        }
                    }
                }
                DecodedInstr::Other { low, high } => {
                    let raw_index = join_u32(low, high) as usize;
                    // SAFETY: decoding assigns a valid raw-instruction index.
                    let instruction = unsafe { *decoded.raw_instructions.get_unchecked(raw_index) };
                    let ran = execute_raw(
                        program,
                        instruction,
                        &mut active,
                        &mut callers,
                        &mut frame_pool,
                        &mut machine.heap,
                        &mut pc,
                    );
                    match ran {
                        Raw::Next => pc += 1,
                        Raw::Jump(target) => pc = jump!(target.min(decoded.code.len() - 1)),
                        Raw::End(outcome) => return outcome,
                        Raw::Switched | Raw::LongJumped => {
                            if matches!(ran, Raw::LongJumped) {
                                poll!();
                            }
                            function = &program.functions[active.function];
                            decoded = unsafe { &*decoded_functions.add(active.function) };
                        }
                    }
                }
                instruction @ (DecodedInstr::End | DecodedInstr::Invalid) => {
                    let code = if let DecodedInstr::End = instruction {
                        "KLF-VM-0002"
                    } else {
                        "KLF-VM-0001"
                    };
                    trap!(code);
                }
            }
        }
    })();
    #[cfg(feature = "execution-profile")]
    machine
        .profile
        .finish(entry, profile_start.0, profile_start.1.elapsed().as_nanos());
    restore_pool(&mut machine.frame_pool, frame_pool, active);
    machine.leave(version);
    machine.reap();
    outcome
}
