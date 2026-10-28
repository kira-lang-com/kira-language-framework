//! The cycle detector of a debug run (section 18.6): each counted allocation
//! with a trace map registers it, with the allocation site, and a run that
//! ends with blocks still live reports the cycles among them.

use super::frames::Frame;
use super::{Machine, Outcome};
use crate::capabilities::Capabilities;
use crate::program::{Program, Traces};
use crate::{Instr, Opcode};

/// What a run does with the cycles left at its end, from `KLF_CYCLES`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cycles {
    /// Collect nothing at exit.
    Off,
    /// Print each leaked cycle to standard error.
    Warn,
    /// Print each leaked cycle, and fail the run with `KLF-RT-0101`.
    Error,
}

impl Cycles {
    /// The mode `KLF_CYCLES` names (`off`, `warn` or `error`), or `default`
    /// when it is unset or names none of them.
    #[must_use]
    pub fn from_env(default: Self) -> Self {
        match std::env::var("KLF_CYCLES").as_deref() {
            Ok("off") => Self::Off,
            Ok("warn") => Self::Warn,
            Ok("error") => Self::Error,
            _ => default,
        }
    }
}

/// Registers the trace map of the counted allocation `instruction` just made
/// in `frame`. Only the program the traces were lowered with has them, so a
/// frame running pushed code registers nothing.
#[inline]
pub(super) fn trace(machine: &mut Machine<'_>, frame: &Frame, pc: usize, instruction: Instr) {
    let Some(traces) = machine.traces else {
        return;
    };
    if instruction.op != Opcode::HallocC || frame.version != 0 {
        return;
    }
    let Ok(shape) = u16::try_from(instruction.bx()) else {
        return;
    };
    if let Some(map) = traces.get(&(frame.function, shape)) {
        let pointer = frame.registers[instruction.a as usize] as *mut u8;
        machine.heap.trace(pointer, map.as_ptr());
        let name = &machine.program.functions[frame.function].name;
        machine.heap.trace_site(pointer, &format!("{name}+{pc}"));
    }
}

/// Runs `entry` as [`super::run_with`] does, with the trace maps of
/// [`crate::lower_module_traced`]. A clean return that leaves blocks live
/// first reports the leaked cycles as `cycles` says, then traps with
/// `KLF-RT-0101` under [`Cycles::Error`] when there are any and with
/// `KLF-RT-0100` otherwise.
#[must_use]
pub fn run_traced(
    program: &Program,
    traces: &Traces,
    entry: usize,
    arguments: &[u64],
    capabilities: &Capabilities,
    cycles: Cycles,
) -> Outcome {
    let mut machine = Machine::new(program);
    machine.traces = Some(traces);
    for initializer in &program.initializers {
        let outcome = machine.call(*initializer, &[], capabilities);
        if !matches!(outcome, Outcome::Return(_)) {
            return machine.end(outcome, capabilities);
        }
    }
    let mut outcome = machine.call(entry, arguments, capabilities);
    if matches!(outcome, Outcome::Return(_)) && !machine.balanced() {
        outcome = Outcome::Trap(leak_code(&machine, cycles).to_owned());
    }
    machine.end(outcome, capabilities)
}

fn leak_code(machine: &Machine<'_>, cycles: Cycles) -> &'static str {
    if cycles == Cycles::Off {
        return "KLF-RT-0100";
    }
    let leaked = machine.heap.leaked_cycles();
    for cycle in &leaked {
        eprintln!("KLF-RT-0101: leaked cycle {cycle}");
    }
    if cycles == Cycles::Error && !leaked.is_empty() {
        "KLF-RT-0101"
    } else {
        "KLF-RT-0100"
    }
}
