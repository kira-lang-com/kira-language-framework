use super::Machine;
use super::decoded::{DecodedInstr, join_u32};
use crate::program::Program;

struct FrameSample {
    ordinal: u64,
    dispatches: u64,
    nanoseconds: u128,
    functions: Vec<(usize, u64)>,
}

pub(super) struct Profile {
    frame_begin: Option<usize>,
    frame_end: Option<usize>,
    frame_start: Option<(u64, std::time::Instant)>,
    frame_ordinal: u64,
    frame_functions: Vec<u64>,
    frames: Vec<FrameSample>,
    pub(super) total: u64,
    entries: Vec<u64>,
    invocations: Vec<(usize, u64, u64, u128)>,
    functions: Vec<u64>,
    instructions: Vec<Vec<u64>>,
}

impl Profile {
    pub(super) fn new(program: &Program) -> Self {
        Self {
            frame_begin: std::env::var("KLF_VM_PROFILE_FRAME_BEGIN")
                .ok()
                .and_then(|name| program.function_index(&name)),
            frame_end: std::env::var("KLF_VM_PROFILE_FRAME_END")
                .ok()
                .and_then(|name| program.function_index(&name)),
            frame_start: None,
            frame_ordinal: 0,
            frame_functions: Vec::new(),
            frames: Vec::new(),
            total: 0,
            entries: vec![0; program.functions.len()],
            invocations: Vec::new(),
            functions: vec![0; program.functions.len()],
            instructions: program
                .functions
                .iter()
                .map(|function| vec![0; function.code.len() + 1])
                .collect(),
        }
    }

    pub(super) fn finish(&mut self, entry: usize, start: u64, elapsed: u128) {
        self.entries[entry] += 1;
        self.invocations
            .push((entry, self.entries[entry], self.total - start, elapsed));
    }

    fn frame_boundary(&mut self, function: usize) {
        if self.frame_begin == Some(function) {
            self.frame_functions.clone_from(&self.functions);
            self.frame_ordinal += 1;
            self.frame_start = Some((self.total, std::time::Instant::now()));
        } else if self.frame_end == Some(function)
            && let Some((start, clock)) = self.frame_start.take()
        {
            let nanoseconds = clock.elapsed().as_nanos();
            let mut functions = self
                .functions
                .iter()
                .zip(&self.frame_functions)
                .enumerate()
                .filter_map(|(index, (end, begin))| (end > begin).then_some((index, end - begin)))
                .collect::<Vec<_>>();
            functions.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
            functions.truncate(10);
            self.frames.push(FrameSample {
                ordinal: self.frame_ordinal,
                dispatches: self.total - start,
                nanoseconds,
                functions,
            });
        }
    }

    #[inline(always)]
    pub(super) fn step(&mut self, function: usize, pc: usize) {
        if pc == 0 {
            self.frame_boundary(function);
        }
        self.total += 1;
        self.functions[function] += 1;
        self.instructions[function][pc] += 1;
    }
}

impl Drop for Machine<'_> {
    fn drop(&mut self) {
        let total: u64 = self.profile.functions.iter().sum();
        let mut ranked = self
            .profile
            .functions
            .iter()
            .enumerate()
            .collect::<Vec<_>>();
        ranked.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(**count));
        eprintln!("vm-profile total-dispatches {total}");
        for index in [self.profile.frame_begin, self.profile.frame_end]
            .into_iter()
            .flatten()
        {
            if self.profile.instructions[index][0] == 0 {
                eprintln!(
                    "vm-profile frame-marker unavailable {} (possibly inlined)",
                    self.program.functions[index].name
                );
            }
        }
        if let Ok(names) = std::env::var("KLF_VM_PROFILE_FUNCTIONS") {
            for name in names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if let Some(index) = self.program.function_index(name) {
                    eprintln!(
                        "vm-profile selected-function dispatches {} entry-pc-hits {} {}",
                        self.profile.functions[index], self.profile.instructions[index][0], name
                    );
                } else {
                    eprintln!("vm-profile selected-function unavailable {name}");
                }
            }
        }
        self.profile
            .frames
            .sort_unstable_by_key(|frame| std::cmp::Reverse(frame.nanoseconds));
        for frame in self.profile.frames.iter().take(8) {
            eprintln!(
                "vm-profile frame {} dispatches {} elapsed-ms {:.3}",
                frame.ordinal,
                frame.dispatches,
                frame.nanoseconds as f64 / 1_000_000.0
            );
            for (function, count) in &frame.functions {
                eprintln!(
                    "vm-profile frame-function {count} {}",
                    self.program.functions[*function].name
                );
            }
        }
        self.profile
            .invocations
            .sort_unstable_by_key(|entry| std::cmp::Reverse(entry.3));
        for (entry, ordinal, count, nanoseconds) in self.profile.invocations.iter().take(20) {
            eprintln!(
                "vm-profile invocation {ordinal} dispatches {count} elapsed-ms {:.3} {}",
                *nanoseconds as f64 / 1_000_000.0,
                self.program.functions[*entry].name
            );
        }
        let mut operations = std::collections::HashMap::<String, u64>::new();
        let mut pairs = std::collections::HashMap::<(String, String), u64>::new();
        for (function, counts) in self.profile.instructions.iter().enumerate() {
            for (pc, count) in counts.iter().enumerate().filter(|(_, count)| **count != 0) {
                let name = |pc| {
                    let decoded = &self.decoded[function];
                    let instruction = decoded.code[pc];
                    let raw = match instruction {
                        DecodedInstr::Other { low, high } | DecodedInstr::Memory { low, high } => {
                            Some(join_u32(low, high) as usize)
                        }
                        DecodedInstr::FloatBinary(index)
                        | DecodedInstr::FloatUnary(index)
                        | DecodedInstr::FloatCompare(index)
                        | DecodedInstr::FloatFma(index) => Some(index as usize),
                        _ => None,
                    };
                    let label = match raw {
                        Some(index) => format!("{:?}", decoded.raw_instructions[index].op),
                        None => format!("{instruction:?}"),
                    };
                    label.split([' ', '(']).next().unwrap().to_owned()
                };
                let op = name(pc);
                *operations.entry(op.clone()).or_default() += count;
                if pc + 1 < counts.len() && counts[pc + 1] != 0 {
                    *pairs.entry((op, name(pc + 1))).or_default() += (*count).min(counts[pc + 1]);
                }
            }
        }
        let mut operations = operations.into_iter().collect::<Vec<_>>();
        operations.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
        for (name, count) in operations.into_iter().take(30) {
            eprintln!("vm-profile opcode {count} {name}");
        }
        let mut pairs = pairs.into_iter().collect::<Vec<_>>();
        pairs.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(*count));
        for ((first, second), count) in pairs.into_iter().take(20) {
            eprintln!("vm-profile adjacent-count {count} {first} {second}");
        }
        for (index, count) in ranked.into_iter().take(25) {
            if *count == 0 {
                break;
            }
            eprintln!(
                "vm-profile function {count} {}",
                self.program.functions[index].name
            );
            let mut instructions = self.profile.instructions[index]
                .iter()
                .enumerate()
                .collect::<Vec<_>>();
            instructions.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(**count));
            for (pc, count) in instructions.into_iter().take(3) {
                eprintln!(
                    "vm-profile pc {pc} count {count} {:?}",
                    self.decoded[index].code[pc]
                );
            }
        }
    }
}
