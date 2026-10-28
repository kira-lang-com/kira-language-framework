use crate::bytecode::{Instr, Opcode, WidthCode, flags_width};
use crate::program::BcFunction;

struct Slot {
    width: WidthCode,
    escaped: bool,
    register: Option<u16>,
}

#[derive(Clone, Copy)]
enum Use {
    Allocate(usize),
    Alias(usize),
    Load(usize),
    Store(usize),
    Free(usize),
}

pub(super) fn scalar_slots(function: &mut BcFunction) {
    if !function.branches.is_empty() || super::simplify::returns_twice(function) {
        return;
    }
    let mut tracked = vec![None; function.register_count as usize];
    let mut slots: Vec<Slot> = Vec::new();
    let mut uses = vec![None; function.code.len()];
    for (pc, word) in function.code.iter().enumerate() {
        let instruction = Instr::decode(*word).unwrap();
        match instruction.op {
            Opcode::Alloca => {
                let shape = function.allocations[instruction.bx() as usize];
                let width = shape
                    .size
                    .checked_mul(8)
                    .and_then(|bits| u16::try_from(bits).ok())
                    .and_then(WidthCode::from_bits);
                if let Some(width) = width.filter(|width| *width != WidthCode::W128) {
                    let slot = slots.len();
                    slots.push(Slot {
                        width,
                        escaped: false,
                        register: None,
                    });
                    tracked[instruction.a as usize] = Some(slot);
                    uses[pc] = Some(Use::Allocate(slot));
                } else {
                    tracked[instruction.a as usize] = None;
                }
                continue;
            }
            Opcode::Mov => {
                tracked[instruction.a as usize] = tracked[instruction.b as usize];
                uses[pc] = tracked[instruction.b as usize].map(Use::Alias);
                continue;
            }
            Opcode::Load => {
                if let Some(slot) = tracked[instruction.b as usize] {
                    if flags_width(instruction.flags) == Some(slots[slot].width) {
                        uses[pc] = Some(Use::Load(slot));
                    } else {
                        slots[slot].escaped = true;
                    }
                }
                tracked[instruction.a as usize] = None;
                continue;
            }
            Opcode::Store => {
                if let Some(slot) = tracked[instruction.a as usize] {
                    if flags_width(instruction.flags) == Some(slots[slot].width) {
                        uses[pc] = Some(Use::Store(slot));
                    } else {
                        slots[slot].escaped = true;
                    }
                }
                if let Some(slot) = tracked[instruction.b as usize] {
                    slots[slot].escaped = true;
                }
                continue;
            }
            Opcode::HfreeR => {
                if let Some(slot) = tracked[instruction.a as usize] {
                    uses[pc] = Some(Use::Free(slot));
                }
                continue;
            }
            _ => {}
        }
        let (mask, writes, _) = super::simplify::effect(instruction, function);
        if mask == 8 {
            for slot in tracked.iter().flatten() {
                slots[*slot].escaped = true;
            }
        }
        for (bit, reg) in [(1, instruction.a), (2, instruction.b), (4, instruction.c)] {
            if mask & bit != 0
                && let Some(slot) = tracked[reg as usize]
            {
                slots[slot].escaped = true;
            }
        }
        if instruction.op == Opcode::Call {
            let site = &function.call_sites[instruction.bx() as usize];
            for reg in site.first_arg..site.first_arg + site.arg_count {
                if let Some(slot) = tracked[reg as usize] {
                    slots[slot].escaped = true;
                }
            }
            if let Some(target) = site.target.and_then(|reg| tracked[reg as usize]) {
                slots[target].escaped = true;
            }
        }
        for reg in writes {
            tracked[reg] = None;
        }
    }
    for slot in &mut slots {
        if !slot.escaped
            && let Ok(register) = u16::try_from(function.register_count)
        {
            slot.register = Some(register);
            function.register_count += 1;
        }
    }
    let mut code = Vec::with_capacity(function.code.len());
    for (word, usage) in function.code.iter().zip(uses) {
        let Some(usage) = usage else {
            code.push(*word);
            continue;
        };
        let index = match usage {
            Use::Allocate(slot)
            | Use::Alias(slot)
            | Use::Load(slot)
            | Use::Store(slot)
            | Use::Free(slot) => slot,
        };
        let Some(register) = slots[index].register else {
            code.push(*word);
            continue;
        };
        let instruction = Instr::decode(*word).unwrap();
        match usage {
            Use::Allocate(_) => code.push(Instr::abc(Opcode::LoadNull, register, 0, 0).encode()),
            Use::Alias(_) | Use::Free(_) => {}
            Use::Load(_) => code.push(Instr::abc(Opcode::Mov, instruction.a, register, 0).encode()),
            Use::Store(_) => {
                let value = if slots[index].width == WidthCode::W64 {
                    Instr::abc(Opcode::Mov, register, instruction.b, 0)
                } else {
                    Instr::abc(Opcode::Trunc, register, instruction.b, 0)
                        .with_flags(WidthCode::W64.code() | (slots[index].width.code() << 3))
                };
                code.push(value.encode());
            }
        }
    }
    function.code = code;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{AllocShape, Program};
    use crate::{Outcome, run};

    #[test]
    fn scalar_promotion_handles_pointer_aliases_and_keeps_partial_stores_in_memory() {
        for (size, store_width, load_width, promoted, expected) in [
            (8, 3, 3, true, 0x1234),
            (1, 0, 0, true, 0x34),
            (8, 3, 0, false, 0x34),
        ] {
            let mut function = BcFunction {
                name: "slot".into(),
                register_count: 3,
                param_count: 1,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                    Instr::abc(Opcode::Mov, 2, 1, 0).encode(),
                    Instr::abc(Opcode::Store, 2, 0, 0)
                        .with_flags(store_width)
                        .encode(),
                    Instr::abc(Opcode::Load, 0, 2, 0)
                        .with_flags(load_width)
                        .encode(),
                    Instr::abc(Opcode::RetV, 0, 0, 0).encode(),
                ],
                call_sites: vec![],
                allocations: vec![AllocShape { size, align: 8 }],
                branches: vec![],
            };
            scalar_slots(&mut function);
            assert_eq!(
                function
                    .code
                    .iter()
                    .all(|word| Instr::decode(*word).unwrap().op != Opcode::Alloca),
                promoted
            );
            let program = Program {
                functions: vec![function],
                consts: vec![],
                imports: vec![],
                globals: vec![],
                initializers: vec![],
            };
            assert_eq!(run(&program, 0, &[0x1234]), Outcome::Return(Some(expected)));
        }
    }
}
