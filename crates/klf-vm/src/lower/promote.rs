use super::{LowerError, Lowerer, err};
use crate::bytecode::{Instr, Opcode, WidthCode};
use klf_core::Name;
use klf_kir::{Function, Operation, Type};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
pub(super) struct Slot {
    register: u16,
    width: WidthCode,
    read: bool,
    initialize: bool,
}

pub(super) fn assign(
    function: &Function,
    mut register_count: u32,
) -> Result<(HashMap<Name, Slot>, u32), LowerError> {
    let instructions = || function.blocks.iter().flat_map(|block| &block.instructions);
    let candidates = instructions()
        .filter_map(|instruction| {
            let Operation::StackAlloc(ty) = &instruction.operation else {
                return None;
            };
            let width = match ty {
                Type::Integer(1 | 8) => WidthCode::W8,
                Type::Integer(16) => WidthCode::W16,
                Type::Integer(32) | Type::Float(32) => WidthCode::W32,
                Type::Integer(64) | Type::Float(64) | Type::Pointer => WidthCode::W64,
                _ => return None,
            };
            let [name] = instruction.results.as_slice() else {
                return None;
            };
            Some((name.clone(), width))
        })
        .collect::<Vec<_>>();
    let names = candidates
        .iter()
        .map(|(name, _)| name)
        .collect::<HashSet<_>>();
    let mut escaped = HashSet::new();
    let mut reads = HashSet::new();
    for instruction in instructions() {
        for used in instruction.operation.used_values() {
            if !names.contains(used) {
                continue;
            }
            let local = match &instruction.operation {
                Operation::Load { address, .. } => {
                    reads.insert(used.clone());
                    address == used
                }
                Operation::Store { address, value, .. } => address == used && value != used,
                Operation::StackDealloc(address) => address == used,
                _ => false,
            };
            if !local {
                escaped.insert(used.clone());
            }
        }
    }
    let mut slots = HashMap::new();
    for (name, width) in candidates {
        if escaped.contains(&name) {
            continue;
        }
        let register = u16::try_from(register_count)
            .map_err(|_| err("function needs more than 65535 registers"))?;
        register_count += 1;
        let read = reads.contains(&name);
        let initialize = read && !stored_before_load(function, &name);
        slots.insert(
            name,
            Slot {
                register,
                width,
                read,
                initialize,
            },
        );
    }
    Ok((slots, register_count))
}

fn stored_before_load(function: &Function, name: &Name) -> bool {
    for block in &function.blocks {
        let Some(start) = block.instructions.iter().position(|instruction| {
            matches!(instruction.operation, Operation::StackAlloc(_))
                && instruction.results.contains(name)
        }) else {
            continue;
        };
        for instruction in &block.instructions[start + 1..] {
            if instruction.operation.used_values().contains(&name) {
                return matches!(&instruction.operation, Operation::Store { address, .. } if address == name);
            }
            if !instruction.operation.successors().is_empty() {
                break;
            }
        }
        return false;
    }
    false
}

impl Lowerer<'_> {
    pub(super) fn lower_promoted_memory(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::StackAlloc(_) => {
                let slot = *self.promoted_slots.get(results.first()?)?;
                if slot.initialize {
                    self.emit(Instr::abc(Opcode::LoadNull, slot.register, 0, 0));
                }
                Ok(())
            }
            Operation::Load { address, .. } => {
                let slot = *self.promoted_slots.get(address)?;
                self.result(results).map(|dest| {
                    self.emit(Instr::abc(Opcode::Mov, dest, slot.register, 0));
                })
            }
            Operation::Store { value, address, .. } => {
                let slot = *self.promoted_slots.get(address)?;
                if !slot.read {
                    return Some(Ok(()));
                }
                self.register(value).map(|source| {
                    if slot.width == WidthCode::W64 {
                        self.emit(Instr::abc(Opcode::Mov, slot.register, source, 0));
                    } else {
                        let flags = WidthCode::W64.code() | (slot.width.code() << 3);
                        self.emit(
                            Instr::abc(Opcode::Trunc, slot.register, source, 0).with_flags(flags),
                        );
                    }
                })
            }
            Operation::StackDealloc(address) => {
                self.promoted_slots.get(address)?;
                Ok(())
            }
            _ => return None,
        };
        Some(result)
    }
}
