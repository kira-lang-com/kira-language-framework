//! Instruction execution: the pure register step and the memory step that
//! needs the heap.

use super::access::Accesses;
use super::{Flow, Outcome};
use crate::bytecode::{Instr, Opcode, OverflowMode, flags_overflow, flags_signed, flags_width};
use crate::numeric::{
    as_float, convert_integer, float_binary, float_compare, float_fma, float_to_int,
    float_to_int_saturating, float_unary, from_float, int_to_float, integer_compare, integer_op,
    integer_unary, load_const,
};
use crate::program::{BcFunction, Const, Program};
use klf_rt::Heap;

pub(super) fn is_memory(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::Alloca
            | Opcode::HallocC
            | Opcode::HallocU
            | Opcode::HallocR
            | Opcode::HfreeR
            | Opcode::Retain
            | Opcode::Release
            | Opcode::Load
            | Opcode::Store
            | Opcode::ALoad
            | Opcode::AStore
            | Opcode::MemCpy
            | Opcode::MemMove
            | Opcode::MemSet
            | Opcode::WeakLoad
            | Opcode::IsUnique
            | Opcode::ARmw
            | Opcode::ACas
            | Opcode::AccBegin
            | Opcode::AccEnd
    )
}

#[allow(clippy::cast_ptr_alignment)]
pub(super) fn memory_step(
    function: &BcFunction,
    instruction: Instr,
    registers: &mut [u64],
    heap: &mut Heap,
    stack: &mut Vec<*mut u8>,
    accesses: &mut Accesses,
) -> Flow {
    let shape = |index: i32| function.allocations.get(index as usize).copied();
    match instruction.op {
        Opcode::Alloca => match shape(instruction.bx()) {
            Some(shape) => {
                let pointer = heap.alloc_raw(shape.size as usize, shape.align.max(1) as usize);
                stack.push(pointer);
                registers[instruction.a as usize] = pointer as u64;
                Flow::Next
            }
            None => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
        },
        Opcode::HallocC | Opcode::HallocU => match shape(instruction.bx()) {
            Some(shape) => {
                let pointer = if matches!(instruction.op, Opcode::HallocC) {
                    heap.alloc_counted(shape.size as usize, shape.align.max(1) as usize, None)
                } else {
                    heap.alloc_unique(shape.size as usize, shape.align.max(1) as usize)
                };
                registers[instruction.a as usize] = pointer as u64;
                Flow::Next
            }
            None => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
        },
        Opcode::HallocR => {
            let size = registers[instruction.b as usize] as usize;
            let align = (registers[instruction.c as usize] as usize).max(1);
            let pointer = heap.alloc_untracked(size, align);
            registers[instruction.a as usize] = pointer as u64;
            Flow::Next
        }
        Opcode::HfreeR => {
            let pointer = registers[instruction.a as usize] as *mut u8;
            stack.retain(|slot| *slot != pointer);
            if heap.free_raw(pointer) {
                Flow::Next
            } else {
                Flow::Done(super::frames::double_free())
            }
        }
        Opcode::Retain => {
            heap.retain(registers[instruction.a as usize] as *mut u8);
            Flow::Next
        }
        Opcode::Release => {
            heap.release(registers[instruction.a as usize] as *mut u8);
            Flow::Next
        }
        Opcode::Load | Opcode::ALoad => {
            let Some(width) = flags_width(instruction.flags) else {
                return Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned()));
            };
            memory_access_step(
                usize::from(instruction.a),
                usize::from(instruction.b),
                width.bits() / 8,
                true,
                registers,
                heap,
            )
        }
        Opcode::Store | Opcode::AStore => {
            let Some(width) = flags_width(instruction.flags) else {
                return Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned()));
            };
            memory_access_step(
                usize::from(instruction.a),
                usize::from(instruction.b),
                width.bits() / 8,
                false,
                registers,
                heap,
            )
        }
        Opcode::MemCpy | Opcode::MemMove | Opcode::MemSet => {
            bulk_memory(instruction, registers, heap)
        }
        Opcode::WeakLoad => {
            let pointer = registers[instruction.b as usize] as *mut u8;
            registers[instruction.a as usize] = heap.weak_load(pointer) as u64;
            Flow::Next
        }
        Opcode::IsUnique => {
            let pointer = registers[instruction.b as usize] as *mut u8;
            registers[instruction.a as usize] = u64::from(heap.is_unique(pointer));
            Flow::Next
        }
        Opcode::AccBegin => {
            // `a` and `c` are immediates: the claimed byte count and the mode.
            let address = registers[instruction.b as usize];
            let size = u64::from(u32::from(instruction.a));
            let modify = instruction.c != 0;
            if accesses.begin(address, size, modify) {
                return Flow::Done(Outcome::Trap("KLF-RT-0003".to_owned()));
            }
            Flow::Next
        }
        Opcode::AccEnd => {
            accesses.end(registers[instruction.b as usize]);
            Flow::Next
        }
        Opcode::ARmw => atomic_rmw(instruction, registers, heap),
        Opcode::ACas => atomic_compare_exchange(instruction, registers, heap),
        _ => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
    }
}

fn memory_access_step(
    a: usize,
    b: usize,
    bytes: u32,
    is_load: bool,
    registers: &mut [u64],
    heap: &mut Heap,
) -> Flow {
    if is_load {
        let pointer = registers[b] as *const u8;
        match heap.read(pointer, bytes) {
            Some(value) => {
                registers[a] = value;
                Flow::Next
            }
            None => Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned())),
        }
    } else {
        let pointer = registers[a] as *mut u8;
        if heap.write(pointer, bytes, registers[b]) {
            Flow::Next
        } else {
            Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()))
        }
    }
}

/// `mem.copy`, `mem.move` and `mem.set` (section 12.8), each range checked
/// whole before any byte moves. A copy is done as a move, which a copy
/// between ranges that do not overlap cannot tell apart.
fn bulk_memory(instruction: Instr, registers: &mut [u64], heap: &mut Heap) -> Flow {
    let dest = registers[instruction.a as usize] as *mut u8;
    let source = registers[instruction.b as usize];
    let length = registers[instruction.c as usize] as usize;
    let done = if matches!(instruction.op, Opcode::MemSet) {
        heap.fill(dest, source as u8, length)
    } else {
        heap.copy(dest, source as *const u8, length)
    };
    if done {
        Flow::Next
    } else {
        Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()))
    }
}

/// `atomic.rmw` (section 12.14). The interpreter runs one thread, so the
/// read-modify-write is already indivisible and the memory order is inert.
fn atomic_rmw(instruction: Instr, registers: &mut [u64], heap: &mut Heap) -> Flow {
    let Some(width) = flags_width(instruction.flags) else {
        return Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned()));
    };
    let bytes = width.bits() / 8;
    let pointer = registers[instruction.b as usize] as usize;
    let operand = registers[instruction.c as usize];
    let Some(previous) = heap.read(pointer as *const u8, bytes) else {
        return Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()));
    };
    let updated = crate::numeric::atomic_apply(instruction.flags >> 3, previous, operand, width);
    if !heap.write(pointer as *mut u8, bytes, updated) {
        return Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()));
    }
    registers[instruction.a as usize] = previous;
    Flow::Next
}

/// `atomic.cmpxchg` (section 12.14), yielding the value read in `a` and whether
/// the exchange happened in `a + 1`.
fn atomic_compare_exchange(instruction: Instr, registers: &mut [u64], heap: &mut Heap) -> Flow {
    let Some(width) = flags_width(instruction.flags) else {
        return Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned()));
    };
    let bytes = width.bits() / 8;
    let pointer = registers[instruction.b as usize] as usize;
    let expected = registers[instruction.c as usize];
    let replacement = registers[instruction.c as usize + 1];
    let Some(previous) = heap.read(pointer as *const u8, bytes) else {
        return Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()));
    };
    let exchanged = previous == expected;
    if exchanged && !heap.write(pointer as *mut u8, bytes, replacement) {
        return Flow::Done(Outcome::Trap("KLF-RT-0005".to_owned()));
    }
    registers[instruction.a as usize] = previous;
    registers[instruction.a as usize + 1] = u64::from(exchanged);
    Flow::Next
}

pub(super) fn step(program: &Program, instruction: Instr, registers: &mut [u64]) -> Flow {
    match numeric_step(instruction, registers) {
        Some(flow) => flow,
        None => control_step(program, instruction, registers),
    }
}

/// The register-only instructions: arithmetic, comparison and conversion.
/// Returns `None` for an instruction this step does not handle.
fn numeric_step(instruction: Instr, registers: &mut [u64]) -> Option<Flow> {
    integer_and_float_arithmetic(instruction, registers)
        .or_else(|| conversion_step(instruction, registers))
        .or_else(|| bitwise_step(instruction, registers))
}

/// Moves, constants and the arithmetic of sections 12.5 and 12.6.
fn integer_and_float_arithmetic(instruction: Instr, registers: &mut [u64]) -> Option<Flow> {
    let read = |index: u16| registers[index as usize];
    let flow = match instruction.op {
        Opcode::Mov => {
            registers[instruction.a as usize] = read(instruction.b);
            Flow::Next
        }
        Opcode::LoadI => {
            registers[instruction.a as usize] = i64::from(instruction.bx()) as u64;
            Flow::Next
        }
        Opcode::LeaFn => {
            registers[instruction.a as usize] = instruction.bx() as u64;
            Flow::Next
        }
        Opcode::LoadNull => {
            registers[instruction.a as usize] = 0;
            Flow::Next
        }
        Opcode::Add
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
        | Opcode::RotR => {
            let Some(width) = flags_width(instruction.flags) else {
                return Some(Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())));
            };
            match integer_op(
                instruction.op,
                read(instruction.b),
                read(instruction.c),
                width,
                flags_signed(instruction.flags),
                flags_overflow(instruction.flags),
            ) {
                Ok(value) => {
                    registers[instruction.a as usize] = value;
                    Flow::Next
                }
                Err(trap) => Flow::Done(Outcome::Trap(trap.to_owned())),
            }
        }
        Opcode::Icmp => {
            let Some(width) = flags_width(instruction.flags & 0b111) else {
                return Some(Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())));
            };
            let predicate = instruction.flags >> 3;
            let result =
                integer_compare(predicate, read(instruction.b), read(instruction.c), width);
            registers[instruction.a as usize] = u64::from(result);
            Flow::Next
        }
        Opcode::FAdd
        | Opcode::FSub
        | Opcode::FMul
        | Opcode::FDiv
        | Opcode::FRem
        | Opcode::FMin
        | Opcode::FMax => {
            let value = float_binary(
                instruction.op,
                read(instruction.b),
                read(instruction.c),
                instruction.flags & 1 == 1,
            );
            registers[instruction.a as usize] = value;
            Flow::Next
        }
        Opcode::FNeg
        | Opcode::FAbs
        | Opcode::FSqrt
        | Opcode::FFloor
        | Opcode::FCeil
        | Opcode::FTrunc
        | Opcode::FRound => {
            let value = float_unary(
                instruction.op,
                read(instruction.b),
                instruction.flags & 1 == 1,
            );
            registers[instruction.a as usize] = value;
            Flow::Next
        }
        _ => return None,
    };
    Some(flow)
}

/// Width and representation conversions, float comparison and fused
/// multiply-add.
fn conversion_step(instruction: Instr, registers: &mut [u64]) -> Option<Flow> {
    let read = |index: u16| registers[index as usize];
    let flow = match instruction.op {
        Opcode::Trunc | Opcode::ZExt | Opcode::SExt => {
            match convert_integer(instruction.op, read(instruction.b), instruction.flags) {
                Some(value) => {
                    registers[instruction.a as usize] = value;
                    Flow::Next
                }
                None => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
            }
        }
        Opcode::SiToFp | Opcode::UiToFp => {
            match int_to_float(instruction.op, read(instruction.b), instruction.flags) {
                Some(value) => {
                    registers[instruction.a as usize] = value;
                    Flow::Next
                }
                None => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
            }
        }
        Opcode::FpToSi | Opcode::FpToUi
            if flags_overflow(instruction.flags) == OverflowMode::Saturate =>
        {
            let signed = instruction.op == Opcode::FpToSi;
            match float_to_int_saturating(signed, read(instruction.b), instruction.flags) {
                Some(value) => {
                    registers[instruction.a as usize] = value;
                    Flow::Next
                }
                None => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
            }
        }
        Opcode::FpToSi | Opcode::FpToUi => {
            match float_to_int(instruction.op, read(instruction.b), instruction.flags) {
                Ok(value) => {
                    registers[instruction.a as usize] = value;
                    Flow::Next
                }
                Err(trap) => Flow::Done(Outcome::Trap(trap.to_owned())),
            }
        }
        Opcode::FpTrunc => {
            registers[instruction.a as usize] =
                from_float(as_float(read(instruction.b), true), false);
            Flow::Next
        }
        Opcode::FpExt => {
            registers[instruction.a as usize] =
                from_float(as_float(read(instruction.b), false), true);
            Flow::Next
        }
        Opcode::FCmp => {
            let predicate = instruction.flags >> 1;
            let result = float_compare(
                predicate,
                read(instruction.b),
                read(instruction.c),
                instruction.flags & 1 == 1,
            );
            registers[instruction.a as usize] = u64::from(result);
            Flow::Next
        }
        Opcode::FFma => {
            registers[instruction.a as usize] = float_fma(
                read(instruction.b),
                read(instruction.c),
                read(instruction.c + 1),
                instruction.flags & 1 == 1,
            );
            Flow::Next
        }
        _ => return None,
    };
    Some(flow)
}

/// Bitwise operations, select, pointer arithmetic and the saturating
/// conversions.
fn bitwise_step(instruction: Instr, registers: &mut [u64]) -> Option<Flow> {
    let read = |index: u16| registers[index as usize];
    let flow = match instruction.op {
        Opcode::Not
        | Opcode::Neg
        | Opcode::Ctlz
        | Opcode::Cttz
        | Opcode::Ctpop
        | Opcode::Bswap
        | Opcode::Bitrev => {
            let Some(width) = flags_width(instruction.flags) else {
                return Some(Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())));
            };
            registers[instruction.a as usize] =
                integer_unary(instruction.op, read(instruction.b), width);
            Flow::Next
        }
        Opcode::Select => {
            registers[instruction.a as usize] = if read(instruction.b) & 1 == 1 {
                read(instruction.c)
            } else {
                read(instruction.c + 1)
            };
            Flow::Next
        }
        Opcode::Bitcast | Opcode::PtrAdd | Opcode::PtrDiff => {
            let value = match instruction.op {
                Opcode::Bitcast => read(instruction.b),
                Opcode::PtrAdd => read(instruction.b).wrapping_add(read(instruction.c)),
                _ => read(instruction.b).wrapping_sub(read(instruction.c)),
            };
            registers[instruction.a as usize] = value;
            Flow::Next
        }
        _ => return None,
    };
    Some(flow)
}

/// Control transfer, constants and the traps for anything left over. A memory
/// opcode never arrives here; `memory_step` claims it first.
fn control_step(program: &Program, instruction: Instr, registers: &mut [u64]) -> Flow {
    let read = |index: u16| registers[index as usize];
    match instruction.op {
        Opcode::LoadK => match load_const(program, instruction.bx()) {
            Ok(value) => {
                registers[instruction.a as usize] = value;
                Flow::Next
            }
            Err(trap) => Flow::Done(Outcome::Trap(trap.to_owned())),
        },
        Opcode::Jmp => Flow::Jump(instruction.bx() as usize),
        Opcode::Jif => {
            if read(instruction.a) & 1 == 1 {
                Flow::Jump(instruction.bx() as usize)
            } else {
                Flow::Next
            }
        }
        Opcode::Jifn => {
            if read(instruction.a) & 1 == 0 {
                Flow::Jump(instruction.bx() as usize)
            } else {
                Flow::Next
            }
        }
        Opcode::Ret => Flow::Done(Outcome::Return(None)),
        Opcode::RetV => Flow::Done(Outcome::Return(Some(read(instruction.a)))),
        Opcode::Fail => Flow::Done(Outcome::Fail(read(instruction.a))),
        Opcode::Unreach => Flow::Done(Outcome::Trap("KLF-RT-0001".to_owned())),
        Opcode::Trap => match &program.consts[instruction.bx() as usize] {
            Const::String(code) => Flow::Done(Outcome::Trap(crate::trap_operands::filled(code))),
            _ => Flow::Done(Outcome::Trap("KLF-VM-0001".to_owned())),
        },
        Opcode::Nop | Opcode::Fence => Flow::Next,
        _ => Flow::Done(Outcome::Trap("KLF-VM-0003".to_owned())),
    }
}
