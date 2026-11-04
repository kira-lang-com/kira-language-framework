use std::collections::{HashMap, HashSet, VecDeque};
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinarySignature};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_body(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    asynchronous: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !asynchronous && caller.coroutine.is_none() {
        return;
    }

    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            if matches!(instruction.opcode, 0x1e4 | 0x210..=0x215) {
                report(
                    diagnostics,
                    caller,
                    instruction,
                    "setjmp, longjmp, va.*, and br.indirect are forbidden in coroutine and async functions",
                );
            }
            if caller.coroutine.is_some() && instruction.opcode == 0x1c1 {
                verify_destroy_path(body, caller, block_index, instruction, diagnostics);
            }
        }
    }
    verify_static_access_scopes(body, caller, diagnostics);
    verify_borrows_across_suspension(body, caller, asynchronous, diagnostics);
}

fn verify_borrows_across_suspension(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    asynchronous: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if body.blocks.iter().any(|block| block.instructions.is_none()) {
        return;
    }
    let mut block_parameters = Vec::with_capacity(body.blocks.len());
    let mut instruction_results = Vec::with_capacity(body.blocks.len());
    let mut definitions = vec![HashSet::new(); body.blocks.len()];
    let mut uses = vec![HashSet::new(); body.blocks.len()];
    let mut borrowed_values = HashSet::new();
    let mut allowed_parameters = HashSet::new();
    let mut value_index = 0_u64;

    for (block_index, block) in body.blocks.iter().enumerate() {
        let start = value_index;
        value_index += block.parameters.len() as u64;
        let parameters = (start..value_index).collect::<Vec<_>>();
        for (parameter_index, parameter) in block.parameters.iter().enumerate() {
            let value = parameters[parameter_index];
            if matches!(parameter.convention, 1 | 2) {
                borrowed_values.insert(value);
            }
            if block_index == 0
                && caller.coroutine.is_some()
                && !asynchronous
                && parameter.convention == 1
            {
                allowed_parameters.insert(value);
            }
        }
        block_parameters.push(parameters);

        let mut result_starts = Vec::new();
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            for value in value_ids(instruction) {
                if !definitions[block_index].contains(&value) {
                    uses[block_index].insert(value);
                }
            }
            let result_start = value_index;
            result_starts.push(result_start);
            if matches!(instruction.opcode, 0x141 | 0x183) {
                borrowed_values.insert(result_start);
            }
            for offset in 0..instruction.result_count {
                definitions[block_index].insert(result_start + offset);
            }
            value_index += instruction.result_count;
        }
        instruction_results.push(result_starts);
    }

    let mut borrowed_flow = HashMap::<u64, Vec<u64>>::new();
    let mut edge_arguments = vec![Vec::<(usize, Vec<u64>)>::new(); body.blocks.len()];
    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for (instruction_index, instruction) in instructions.iter().enumerate() {
            let result_start = instruction_results[block_index][instruction_index];
            if instruction.result_count > 0 && preserves_borrow(instruction.opcode) {
                for value in value_ids(instruction) {
                    borrowed_flow.entry(value).or_default().push(result_start);
                }
            }
        }
        if let Some(terminator) = instructions.last() {
            super::super::structure::visit_edges(terminator, |target, arguments| {
                let Ok(target_index) = usize::try_from(target) else {
                    return;
                };
                let Some(parameters) = block_parameters.get(target_index) else {
                    return;
                };
                for (&argument, &parameter) in arguments.iter().zip(parameters) {
                    borrowed_flow.entry(argument).or_default().push(parameter);
                }
                edge_arguments[block_index].push((target_index, arguments.to_vec()));
            });
        }
    }
    let mut pending: VecDeque<_> = borrowed_values.iter().copied().collect();
    while let Some(value) = pending.pop_front() {
        if let Some(next_values) = borrowed_flow.get(&value) {
            for &next in next_values {
                if borrowed_values.insert(next) {
                    pending.push_back(next);
                }
            }
        }
    }

    let live_out = compute_liveness(
        body,
        &block_parameters,
        &definitions,
        &uses,
        &edge_arguments,
    );
    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        if let Some(terminator) = instructions.last()
            && terminator.opcode == 0x1c1
            && let Some(value) = live_out[block_index]
                .iter()
                .filter(|value| {
                    borrowed_values.contains(value) && !allowed_parameters.contains(value)
                })
                .min()
        {
            report(
                diagnostics,
                caller,
                terminator,
                format!(
                    "borrowed value {value} is live across suspension but is not a borrow parameter of the coroutine"
                ),
            );
        }

        let mut live = live_out[block_index].clone();
        for (instruction_index, instruction) in instructions.iter().enumerate().rev() {
            if instruction.opcode == 0x1c8
                && let Some(value) = live
                    .iter()
                    .filter(|value| {
                        borrowed_values.contains(value) && !allowed_parameters.contains(value)
                    })
                    .min()
            {
                report(
                    diagnostics,
                    caller,
                    instruction,
                    format!(
                        "borrowed value {value} is live across suspension but is not a borrow parameter of the coroutine"
                    ),
                );
            }
            let result_start = instruction_results[block_index][instruction_index];
            for offset in 0..instruction.result_count {
                live.remove(&(result_start + offset));
            }
            live.extend(value_ids(instruction));
        }
    }
}

fn compute_liveness(
    body: &BinaryFunctionBody,
    block_parameters: &[Vec<u64>],
    definitions: &[HashSet<u64>],
    uses: &[HashSet<u64>],
    edges: &[Vec<(usize, Vec<u64>)>],
) -> Vec<HashSet<u64>> {
    let mut live_in = vec![HashSet::new(); body.blocks.len()];
    let mut live_out = vec![HashSet::new(); body.blocks.len()];
    loop {
        let mut changed = false;
        for block_index in (0..body.blocks.len()).rev() {
            let mut outgoing = HashSet::new();
            for (target, arguments) in &edges[block_index] {
                let Some(target_live) = live_in.get(*target) else {
                    continue;
                };
                let Some(parameters) = block_parameters.get(*target) else {
                    continue;
                };
                for value in target_live {
                    if !parameters.contains(value) {
                        outgoing.insert(*value);
                    }
                }
                for (&argument, &parameter) in arguments.iter().zip(parameters) {
                    if target_live.contains(&parameter) {
                        outgoing.insert(argument);
                    }
                }
            }
            let mut incoming = uses[block_index].clone();
            incoming.extend(
                outgoing
                    .iter()
                    .filter(|value| !definitions[block_index].contains(value))
                    .copied(),
            );
            if outgoing != live_out[block_index] || incoming != live_in[block_index] {
                live_out[block_index] = outgoing;
                live_in[block_index] = incoming;
                changed = true;
            }
        }
        if !changed {
            return live_out;
        }
    }
}

fn preserves_borrow(opcode: u64) -> bool {
    matches!(opcode, 0x03d | 0x045 | 0x122..=0x128 | 0x148)
}

fn verify_static_access_scopes(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if body.blocks.iter().any(|block| block.instructions.is_none()) {
        return;
    }
    let mut block_parameters = Vec::with_capacity(body.blocks.len());
    let mut value_index = 0_u64;
    for block in &body.blocks {
        let start = value_index;
        value_index += block.parameters.len() as u64;
        block_parameters.push((start..value_index).collect::<Vec<_>>());
        for instruction in block.instructions.as_deref().unwrap_or_default() {
            value_index += instruction.result_count;
        }
    }
    let mut argument_flow = HashMap::<u64, Vec<u64>>::new();
    let mut successors = vec![Vec::new(); body.blocks.len()];
    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            for operand in &instruction.operands {
                match operand {
                    BinaryOperand::BlockTarget { block, arguments } => {
                        add_argument_edges(*block, arguments, &block_parameters, &mut argument_flow)
                    }
                    BinaryOperand::SwitchCases(cases) => {
                        for (_, block, arguments) in cases {
                            add_argument_edges(
                                *block,
                                arguments,
                                &block_parameters,
                                &mut argument_flow,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(terminator) = instructions.last() {
            super::super::structure::visit_edges(terminator, |target, _| {
                if let Ok(target) = usize::try_from(target)
                    && target < successors.len()
                {
                    successors[block_index].push(target);
                }
            });
        }
    }

    value_index = 0;
    for (block_index, block) in body.blocks.iter().enumerate() {
        value_index += block.parameters.len() as u64;
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for (instruction_index, instruction) in instructions.iter().enumerate() {
            if instruction.opcode == 0x187 && ((instruction.modifiers >> 48) & 0xf) <= 1 {
                let aliases = value_aliases(value_index, &argument_flow);
                verify_static_access_scope(
                    body,
                    caller,
                    &successors,
                    block_index,
                    instruction_index + 1,
                    &aliases,
                    diagnostics,
                );
            }
            value_index += instruction.result_count;
        }
    }
}

fn verify_static_access_scope(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    successors: &[Vec<usize>],
    start_block: usize,
    start_instruction: usize,
    aliases: &HashSet<u64>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut pending = VecDeque::from([(start_block, start_instruction)]);
    let mut seen = HashSet::new();
    while let Some((block_index, first_instruction)) = pending.pop_front() {
        if !seen.insert((block_index, first_instruction)) {
            continue;
        }
        let Some(instructions) = body
            .blocks
            .get(block_index)
            .and_then(|block| block.instructions.as_ref())
        else {
            continue;
        };
        let mut ended = false;
        for instruction in instructions.iter().skip(first_instruction) {
            if instruction.opcode == 0x188
                && value_ids(instruction).any(|value| aliases.contains(&value))
            {
                ended = true;
                break;
            }
            if matches!(instruction.opcode, 0x1c1 | 0x1c8) {
                report(
                    diagnostics,
                    caller,
                    instruction,
                    "static access scope crosses a coroutine or async suspension point",
                );
                return;
            }
            if instruction.terminator {
                break;
            }
        }
        if !ended {
            pending.extend(
                successors
                    .get(block_index)
                    .into_iter()
                    .flatten()
                    .map(|target| (*target, 0)),
            );
        }
    }
}

fn add_argument_edges(
    block: u64,
    arguments: &[u64],
    block_parameters: &[Vec<u64>],
    flow: &mut HashMap<u64, Vec<u64>>,
) {
    let Some(parameters) = usize::try_from(block)
        .ok()
        .and_then(|block| block_parameters.get(block))
    else {
        return;
    };
    for (&argument, &parameter) in arguments.iter().zip(parameters) {
        flow.entry(argument).or_default().push(parameter);
    }
}

fn value_aliases(value: u64, flow: &HashMap<u64, Vec<u64>>) -> HashSet<u64> {
    let mut aliases = HashSet::from([value]);
    let mut pending = VecDeque::from([value]);
    while let Some(value) = pending.pop_front() {
        if let Some(next_values) = flow.get(&value) {
            for &next in next_values {
                if aliases.insert(next) {
                    pending.push_back(next);
                }
            }
        }
    }
    aliases
}

fn value_ids(instruction: &BinaryInstruction) -> impl Iterator<Item = u64> + '_ {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
}

fn verify_destroy_path(
    body: &BinaryFunctionBody,
    caller: &BinarySignature,
    yield_block: usize,
    yield_instruction: &BinaryInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(destroy_target) = yield_instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .nth(1)
    else {
        return;
    };

    let mut pending = vec![destroy_target];
    let mut reachable = vec![false; body.blocks.len()];
    let mut reverse_edges = vec![Vec::new(); body.blocks.len()];
    let mut forward_edges = vec![Vec::new(); body.blocks.len()];
    let mut finishes = Vec::new();
    let mut finish_block = vec![false; body.blocks.len()];
    while let Some(block_index) = pending.pop() {
        let Ok(index) = usize::try_from(block_index) else {
            continue;
        };
        let Some(seen) = reachable.get_mut(index) else {
            continue;
        };
        if *seen {
            continue;
        }
        *seen = true;

        let Some(block) = body.blocks.get(index) else {
            continue;
        };
        let Some(terminator) = block.instructions.as_ref().and_then(|items| items.last()) else {
            continue;
        };
        if terminator.opcode == 0x1c3 {
            finishes.push(index);
            finish_block[index] = true;
            continue;
        }

        let mut successors = Vec::new();
        super::super::structure::visit_edges(terminator, |target, _| successors.push(target));
        for target in successors {
            let Ok(target_index) = usize::try_from(target) else {
                continue;
            };
            if let Some(predecessors) = reverse_edges.get_mut(target_index) {
                predecessors.push(index);
                forward_edges[index].push(target_index);
                pending.push(target);
            }
        }
    }

    let mut reaches_finish = vec![false; body.blocks.len()];
    let mut pending = finishes;
    while let Some(index) = pending.pop() {
        let Some(reaches) = reaches_finish.get_mut(index) else {
            continue;
        };
        if *reaches {
            continue;
        }
        *reaches = true;
        pending.extend(reverse_edges[index].iter().copied());
    }

    if let Some(index) = reachable
        .iter()
        .zip(&reaches_finish)
        .position(|(seen, finishes)| *seen && !finishes)
        && let Some(terminator) = body.blocks[index]
            .instructions
            .as_ref()
            .and_then(|items| items.last())
    {
        report(
            diagnostics,
            caller,
            terminator,
            format!(
                "destroy path from coro.yield in block {yield_block} reaches block {index}, which cannot reach coro.finish"
            ),
        );
        return;
    }

    if let Some(index) = nonterminating_cycle(&reachable, &finish_block, &forward_edges)
        && let Some(terminator) = body.blocks[index]
            .instructions
            .as_ref()
            .and_then(|items| items.last())
    {
        report(
            diagnostics,
            caller,
            terminator,
            format!(
                "destroy path from coro.yield in block {yield_block} can loop through block {index} without reaching coro.finish"
            ),
        );
    }
}

fn nonterminating_cycle(
    reachable: &[bool],
    finish_block: &[bool],
    edges: &[Vec<usize>],
) -> Option<usize> {
    let mut colors = vec![0_u8; reachable.len()];
    for start in 0..reachable.len() {
        if !reachable[start] || finish_block[start] || colors[start] != 0 {
            continue;
        }
        colors[start] = 1;
        let mut stack = vec![(start, 0_usize)];
        while let Some((block, edge_index)) = stack.last_mut() {
            let Some(&target) = edges[*block].get(*edge_index) else {
                colors[*block] = 2;
                stack.pop();
                continue;
            };
            *edge_index += 1;
            if finish_block[target] {
                continue;
            }
            match colors[target] {
                0 => {
                    colors[target] = 1;
                    stack.push((target, 0));
                }
                1 => return Some(target),
                _ => {}
            }
        }
    }
    None
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    caller: &BinarySignature,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{} CODE offset {}: {}",
            caller.name,
            instruction.offset,
            message.into()
        ),
    ));
}
