use std::collections::{HashMap, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, FunctionRecordSummary,
    TvisaValueTypeRecord, TypeSection,
};

const VFY7: Code = Code::new(Dialect::Tvisa, Area::Verifier, 7);

#[derive(Clone, Copy)]
enum Operation {
    Address,
    Start,
    End,
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    function: &FunctionRecordSummary,
    is_defined: bool,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut operations = HashMap::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            let Some(operation) = operation(instruction.opcode) else {
                continue;
            };
            let Some(slot_index) = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(index) => usize::try_from(*index).ok(),
                    _ => None,
                })
            else {
                diagnostics.push(Diagnostic::error(
                    VFY7,
                    format!(
                        "{} is missing its frame slot index",
                        opcode_name(instruction.opcode)
                    ),
                ));
                continue;
            };
            let Some(slot) = function.slots.get(slot_index) else {
                diagnostics.push(Diagnostic::error(
                    VFY7,
                    format!("slot operand {slot_index} names no frame slot"),
                ));
                continue;
            };
            if !matches!(operation, Operation::Address) && slot.flags & (1 << 4) == 0 {
                diagnostics.push(Diagnostic::error(
                    VFY7,
                    format!(
                        "{} requires scoped frame slot {slot_index}",
                        opcode_name(instruction.opcode)
                    ),
                ));
            }
            operations.insert((slot_index, instruction.offset), operation);
        }
    }

    let successors = body
        .blocks
        .iter()
        .map(|block| {
            let mut targets = Vec::new();
            if let Some(instruction) = block.instructions.last() {
                visit_edges(instruction, |target| {
                    if let Ok(target) = usize::try_from(target) {
                        targets.push(target);
                    }
                });
            }
            targets
        })
        .collect::<Vec<_>>();
    let pointer_flow = PointerFlow::new(body, function, value_types, types);
    for (slot_index, slot) in function.slots.iter().enumerate() {
        if slot.flags & (1 << 4) != 0 {
            verify_lifetime(
                slot_index,
                is_defined,
                body,
                &successors,
                &operations,
                &pointer_flow,
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

fn verify_lifetime(
    slot_index: usize,
    is_defined: bool,
    body: &BinaryFunctionBody,
    successors: &[Vec<usize>],
    operations: &HashMap<(usize, u64), Operation>,
    pointer_flow: &PointerFlow,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = vec![[false; 2]; body.blocks.len()];
    let mut queue = VecDeque::new();
    let mut reported = HashSet::new();
    let mut reported_pointer_uses = HashSet::new();
    for root in 0..body.blocks.len() {
        if seen[root][0] || seen[root][1] {
            continue;
        }
        seen[root][0] = true;
        queue.push_back((root, false));
        while let Some((block_index, mut active)) = queue.pop_front() {
            for instruction in &body.blocks[block_index].instructions {
                let operation = operations.get(&(slot_index, instruction.offset));
                match operation {
                    Some(Operation::Start) => {
                        if active && reported.insert((block_index, instruction.offset, 0)) {
                            diagnostics.push(Diagnostic::error(
                                VFY7,
                                format!("slot.start {slot_index} occurs while active"),
                            ));
                        }
                        active = true;
                    }
                    Some(Operation::End) => {
                        if !active && reported.insert((block_index, instruction.offset, 1)) {
                            diagnostics.push(Diagnostic::error(
                                VFY7,
                                format!("slot.end {slot_index} occurs while inactive"),
                            ));
                        }
                        active = false;
                    }
                    Some(Operation::Address)
                        if is_defined
                            && !active
                            && reported.insert((block_index, instruction.offset, 2)) =>
                    {
                        diagnostics.push(Diagnostic::error(
                            VFY7,
                            format!("slot.addr {slot_index} occurs outside its scoped lifetime"),
                        ));
                    }
                    _ => {}
                }
            }
            let targets = successors.get(block_index).map_or(&[][..], Vec::as_slice);
            if targets.is_empty() && active && reported.insert((block_index, 0, 3)) {
                diagnostics.push(Diagnostic::error(
                    VFY7,
                    format!("scoped slot {slot_index} remains active at function exit"),
                ));
            }
            let state = usize::from(active);
            for &target in targets {
                if target < seen.len() && !seen[target][state] {
                    seen[target][state] = true;
                    queue.push_back((target, active));
                }
            }
        }
    }
    if is_defined {
        for address in pointer_flow
            .slot_addresses
            .get(slot_index)
            .into_iter()
            .flatten()
        {
            let pointers = pointer_flow.values_from_address(address.value);
            verify_pointer_lifetime(
                slot_index,
                address,
                &pointers,
                body,
                successors,
                operations,
                &mut reported_pointer_uses,
                diagnostics,
            );
        }
    }
}

fn verify_pointer_lifetime(
    slot_index: usize,
    address: &AddressRoot,
    pointers: &HashSet<usize>,
    body: &BinaryFunctionBody,
    successors: &[Vec<usize>],
    operations: &HashMap<(usize, u64), Operation>,
    reported: &mut HashSet<(usize, u64)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = vec![[[false; 2]; 2]; body.blocks.len()];
    let mut queue = VecDeque::new();
    for root in 0..body.blocks.len() {
        if seen[root].iter().flatten().any(|visited| *visited) {
            continue;
        }
        seen[root][0][0] = true;
        queue.push_back((root, false, false));
        while let Some((block_index, mut active, mut address_valid)) = queue.pop_front() {
            for instruction in &body.blocks[block_index].instructions {
                let operation = operations.get(&(slot_index, instruction.offset));
                let uses_address = value_operands(instruction).any(|value| {
                    usize::try_from(value).is_ok_and(|value| pointers.contains(&value))
                });
                if uses_address
                    && (!active || !address_valid)
                    && reported.insert((block_index, instruction.offset))
                {
                    diagnostics.push(Diagnostic::error(
                        VFY7,
                        format!(
                            "an address derived from scoped slot {slot_index} is used outside its lifetime"
                        ),
                    ));
                }
                match operation {
                    Some(Operation::Start) => {
                        active = true;
                        address_valid = false;
                    }
                    Some(Operation::End) => {
                        active = false;
                        address_valid = false;
                    }
                    Some(Operation::Address)
                        if block_index == address.block && instruction.offset == address.offset =>
                    {
                        address_valid = active;
                    }
                    _ => {}
                }
            }
            let targets = successors.get(block_index).map_or(&[][..], Vec::as_slice);
            for &target in targets {
                if target < seen.len() {
                    let active_index = usize::from(active);
                    let valid_index = usize::from(address_valid);
                    if !seen[target][active_index][valid_index] {
                        seen[target][active_index][valid_index] = true;
                        queue.push_back((target, active, address_valid));
                    }
                }
            }
        }
    }
}

struct PointerFlow {
    edges: Vec<Vec<usize>>,
    slot_addresses: Vec<Vec<AddressRoot>>,
}

#[derive(Clone, Copy)]
struct AddressRoot {
    value: usize,
    block: usize,
    offset: u64,
}

impl PointerFlow {
    fn new(
        body: &BinaryFunctionBody,
        function: &FunctionRecordSummary,
        value_types: &[Option<u64>],
        types: &TypeSection,
    ) -> Self {
        let mut edges = vec![Vec::new(); value_types.len()];
        let mut slot_addresses = vec![Vec::new(); function.slots.len()];
        let mut block_parameters = Vec::with_capacity(body.blocks.len());
        let mut value_index = 0usize;
        for block in &body.blocks {
            let start = value_index;
            value_index += block.parameter_types.len();
            block_parameters.push((start..value_index).collect::<Vec<_>>());
            for instruction in &block.instructions {
                value_index += usize::from(instruction.result_count);
            }
        }

        value_index = 0;
        for (block_index, block) in body.blocks.iter().enumerate() {
            value_index += block.parameter_types.len();
            for instruction in &block.instructions {
                let result = value_index;
                if instruction.opcode == 0x700
                    && let Some(slot) = index_operand(instruction)
                    && let Some(addresses) = slot_addresses.get_mut(slot)
                    && result < value_types.len()
                {
                    addresses.push(AddressRoot {
                        value: result,
                        block: block_index,
                        offset: instruction.offset,
                    });
                }
                if preserves_pointer(instruction.opcode)
                    && value_has_pointer_type(result, value_types, types)
                    && result < value_types.len()
                {
                    for operand in value_operands(instruction) {
                        if let Ok(operand) = usize::try_from(operand)
                            && operand < edges.len()
                            && value_has_pointer_type(operand, value_types, types)
                        {
                            edges[operand].push(result);
                        }
                    }
                }
                value_index += usize::from(instruction.result_count);

                for operand in &instruction.operands {
                    match operand {
                        BinaryOperand::BlockTarget { block, arguments } => add_edge_arguments(
                            *block,
                            arguments,
                            &block_parameters,
                            value_types,
                            types,
                            &mut edges,
                        ),
                        BinaryOperand::SwitchCases(cases) => {
                            for (_, block, arguments) in cases {
                                add_edge_arguments(
                                    *block,
                                    arguments,
                                    &block_parameters,
                                    value_types,
                                    types,
                                    &mut edges,
                                );
                            }
                        }
                        BinaryOperand::BlockTargets(targets) => {
                            for (block, arguments) in targets {
                                add_edge_arguments(
                                    *block,
                                    arguments,
                                    &block_parameters,
                                    value_types,
                                    types,
                                    &mut edges,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Self {
            edges,
            slot_addresses,
        }
    }

    fn values_from_address(&self, address: usize) -> HashSet<usize> {
        let mut values = HashSet::new();
        let mut queue = VecDeque::new();
        if values.insert(address) {
            queue.push_back(address);
        }
        while let Some(value) = queue.pop_front() {
            if let Some(next_values) = self.edges.get(value) {
                for &next in next_values {
                    if values.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        values
    }
}

fn add_edge_arguments(
    block: u64,
    arguments: &[u64],
    block_parameters: &[Vec<usize>],
    value_types: &[Option<u64>],
    types: &TypeSection,
    edges: &mut [Vec<usize>],
) {
    let Some(parameters) = usize::try_from(block)
        .ok()
        .and_then(|block| block_parameters.get(block))
    else {
        return;
    };
    for (&argument, &parameter) in arguments.iter().zip(parameters) {
        if let Ok(argument) = usize::try_from(argument)
            && argument < edges.len()
            && value_has_pointer_type(argument, value_types, types)
            && value_has_pointer_type(parameter, value_types, types)
        {
            edges[argument].push(parameter);
        }
    }
}

fn value_has_pointer_type(value: usize, value_types: &[Option<u64>], types: &TypeSection) -> bool {
    value_types
        .get(value)
        .copied()
        .flatten()
        .and_then(|type_index| usize::try_from(type_index).ok())
        .and_then(|type_index| types.value_types.get(type_index))
        .is_some_and(|ty| matches!(ty, TvisaValueTypeRecord::Pointer { .. }))
}

fn preserves_pointer(opcode: u64) -> bool {
    matches!(opcode, 0x03d | 0x045 | 0x148)
}

fn index_operand(instruction: &BinaryInstruction) -> Option<usize> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => usize::try_from(*index).ok(),
            _ => None,
        })
}

fn value_operands(instruction: &BinaryInstruction) -> impl Iterator<Item = u64> + '_ {
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

fn operation(opcode: u64) -> Option<Operation> {
    match opcode {
        0x700 => Some(Operation::Address),
        0x701 => Some(Operation::Start),
        0x702 => Some(Operation::End),
        _ => None,
    }
}

fn opcode_name(opcode: u64) -> &'static str {
    match opcode {
        0x700 => "slot.addr",
        0x701 => "slot.start",
        0x702 => "slot.end",
        _ => "slot instruction",
    }
}

fn visit_edges(instruction: &BinaryInstruction, mut visit: impl FnMut(u64)) {
    match instruction.opcode {
        0x1e0 => {
            for operand in &instruction.operands {
                if let BinaryOperand::BlockTarget { block, .. } = operand {
                    visit(*block);
                }
            }
        }
        0x1e1 => {
            for block in instruction
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::BlockTarget { block, .. } => Some(*block),
                    _ => None,
                })
                .take(2)
            {
                visit(block);
            }
        }
        0x1e2 => {
            for operand in &instruction.operands {
                match operand {
                    BinaryOperand::SwitchCases(cases) => {
                        for (_, block, _) in cases {
                            visit(*block);
                        }
                    }
                    BinaryOperand::BlockTarget { block, .. } => {
                        visit(*block);
                        break;
                    }
                    _ => {}
                }
            }
        }
        0x1e4 => {
            for block in instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Blocks(blocks) => Some(blocks),
                    _ => None,
                })
                .into_iter()
                .flatten()
            {
                visit(*block);
            }
        }
        0x1aa => {
            for block in instruction
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::BlockTarget { block, .. } => Some(*block),
                    _ => None,
                })
                .take(2)
            {
                visit(block);
            }
        }
        _ => {}
    }
}
