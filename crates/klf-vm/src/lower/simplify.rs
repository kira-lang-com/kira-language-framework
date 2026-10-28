use crate::bytecode::{Instr, Opcode};
use crate::program::BcFunction;

pub(super) fn fallthroughs(function: &mut BcFunction) {
    let mut offsets = Vec::with_capacity(function.code.len() + 1);
    let mut code = Vec::with_capacity(function.code.len());
    for (pc, word) in function.code.iter().enumerate() {
        offsets.push(code.len());
        let instruction = Instr::decode(*word).expect("lowered opcode");
        let redundant = instruction.op == Opcode::Br
            && function
                .branches
                .get(instruction.bx() as usize)
                .is_some_and(|branch| {
                    branch.condition.is_none()
                        && branch.else_edge.is_none()
                        && branch.then_edge.moves.is_empty()
                        && branch.then_edge.target == pc + 1
                });
        if !redundant && !matches!(instruction.op, Opcode::Nop | Opcode::Fence) {
            code.push(*word);
        }
    }
    offsets.push(code.len());
    for branch in &mut function.branches {
        branch.then_edge.target = offsets[branch.then_edge.target];
        if let Some(edge) = &mut branch.else_edge {
            edge.target = offsets[edge.target];
        }
    }
    function.code = code;
}

/// Whether `function` runs `setjmp`. A `longjmp` comes back to it with its
/// registers as they are then, so the bytecode passes, which rename,
/// share and drop registers, leave such a function alone.
pub(super) fn returns_twice(function: &BcFunction) -> bool {
    function.code.iter().any(|word| {
        Instr::decode(*word).is_some_and(|instruction| instruction.op == Opcode::SetJmp)
    })
}

pub(super) fn local_registers(function: &mut BcFunction) {
    if returns_twice(function)
        || function.code.iter().any(|word| {
            matches!(
                Instr::decode(*word).unwrap().op,
                Opcode::Jmp | Opcode::Jif | Opcode::Jifn
            )
        })
    {
        return;
    }
    let branching = function.code.iter().any(|word| {
        matches!(
            Instr::decode(*word).unwrap().op,
            Opcode::Br | Opcode::BrIf | Opcode::BrCmp | Opcode::Jmp | Opcode::Jif | Opcode::Jifn
        )
    });
    let starts = super::blocks::starts(function);
    let mut count = function.register_count as usize;
    let mut copy_register = None;
    if !branching {
        function.branches.clear();
    }
    let mut versions = vec![0u32; count];
    let mut aliases = (0..count).map(|reg| (reg, 0u32)).collect::<Vec<_>>();
    let mut constants: Vec<Option<u64>> = vec![None; count];
    let mut code = Vec::with_capacity(function.code.len());
    let mut offsets = Vec::with_capacity(function.code.len() + 1);
    let root = |reg: u16, aliases: &[(usize, u32)], versions: &[u32]| -> u16 {
        let (source, version) = aliases[reg as usize];
        if versions[source] == version {
            source as u16
        } else {
            reg
        }
    };
    for (pc, word) in function.code.iter().enumerate() {
        offsets.push(code.len());
        if starts[pc] {
            for reg in 0..count {
                aliases[reg] = (reg, versions[reg]);
            }
            constants.fill(None);
        }
        let mut instruction = Instr::decode(*word).unwrap();
        let (mask, writes, _) = effect(instruction, function);
        for (bit, reg) in [
            (1, &mut instruction.a),
            (2, &mut instruction.b),
            (4, &mut instruction.c),
        ] {
            if mask & bit != 0 {
                *reg = root(*reg, &aliases, &versions);
            }
        }
        if matches!(instruction.op, Opcode::Br | Opcode::BrIf | Opcode::BrCmp) {
            let branch = &mut function.branches[instruction.bx() as usize];
            match &mut branch.condition {
                Some(crate::program::BranchCondition::Register(reg)) => {
                    *reg = root(*reg, &aliases, &versions);
                }
                Some(crate::program::BranchCondition::IntegerCompare {
                    result, lhs, rhs, ..
                }) => {
                    *lhs = root(*lhs, &aliases, &versions);
                    *rhs = root(*rhs, &aliases, &versions);
                    let result = *result as usize;
                    versions[result] = versions[result].wrapping_add(1);
                    aliases[result] = (result, versions[result]);
                    constants[result] = None;
                }
                None => {}
            }
            for edge in std::iter::once(&mut branch.then_edge).chain(&mut branch.else_edge) {
                for movement in &mut edge.moves {
                    movement.source = root(movement.source, &aliases, &versions);
                }
            }
        }
        if matches!(instruction.op, Opcode::MemCpy | Opcode::MemMove)
            && let Some(length @ (1 | 2 | 4 | 8)) = constants[instruction.c as usize]
            && u16::try_from(count).is_ok()
        {
            let register = *copy_register.get_or_insert_with(|| {
                let register = count as u16;
                versions.push(0);
                aliases.push((count, 0));
                constants.push(None);
                count += 1;
                function.register_count += 1;
                register
            });
            let width = length.trailing_zeros() as u8;
            code.push(Instr::abc(Opcode::Load, register, instruction.b, 0).with_flags(width));
            code.push(Instr::abc(Opcode::Store, instruction.a, register, 0).with_flags(width));
            continue;
        }
        if matches!(instruction.op, Opcode::Trunc | Opcode::ZExt | Opcode::SExt)
            && instruction.flags & 7 <= 3
            && (instruction.flags >> 3) & 7 <= 3
            && let Some(value) = constants[instruction.b as usize]
            && let Some(value) =
                crate::numeric::convert_integer(instruction.op, value, instruction.flags)
            && let Ok(value) = i32::try_from(value as i64)
        {
            instruction = Instr::from_bx(Opcode::LoadI, instruction.a, value);
        }
        if instruction.flags == 3
            && matches!(instruction.op, Opcode::Add | Opcode::Sub | Opcode::Mul)
        {
            let left = constants[instruction.b as usize];
            let right = constants[instruction.c as usize];
            let source = match (instruction.op, left, right) {
                (Opcode::Add, Some(0), _) | (Opcode::Mul, Some(1), _) => Some(instruction.c),
                (Opcode::Add | Opcode::Sub, _, Some(0)) | (Opcode::Mul, _, Some(1)) => {
                    Some(instruction.b)
                }
                _ => None,
            };
            if let Some(source) = source {
                instruction = Instr::abc(Opcode::Mov, instruction.a, source, 0);
            } else if let (Some(left), Some(right)) = (left, right) {
                let value = match instruction.op {
                    Opcode::Add => left.wrapping_add(right),
                    Opcode::Sub => left.wrapping_sub(right),
                    _ => left.wrapping_mul(right),
                };
                if let Ok(value) = i32::try_from(value as i64) {
                    instruction = Instr::from_bx(Opcode::LoadI, instruction.a, value);
                }
            }
        }
        let constant = match instruction.op {
            Opcode::Mov => constants[instruction.b as usize],
            Opcode::LoadI => Some(i64::from(instruction.bx()) as u64),
            Opcode::LoadNull => Some(0),
            _ => None,
        };
        for reg in writes {
            versions[reg] = versions[reg].wrapping_add(1);
            aliases[reg] = (reg, versions[reg]);
            constants[reg] = None;
        }
        if instruction.op == Opcode::Mov {
            aliases[instruction.a as usize] =
                (instruction.b as usize, versions[instruction.b as usize]);
        }
        if matches!(
            instruction.op,
            Opcode::Mov | Opcode::LoadI | Opcode::LoadNull
        ) {
            constants[instruction.a as usize] = constant;
        }
        code.push(instruction);
    }
    offsets.push(code.len());
    for branch in &mut function.branches {
        branch.then_edge.target = offsets[branch.then_edge.target];
        if let Some(edge) = &mut branch.else_edge {
            edge.target = offsets[edge.target];
        }
    }
    if branching {
        function.code = code.into_iter().map(Instr::encode).collect();
        super::blocks::eliminate_dead(function);
        return;
    }
    let mut live = vec![false; count];
    let mut retained = Vec::with_capacity(code.len());
    for instruction in code.into_iter().rev() {
        let (mask, writes, pure) = effect(instruction, function);
        if pure && writes.iter().all(|reg| !live[*reg]) {
            continue;
        }
        for reg in writes {
            live[reg] = false;
        }
        if instruction.op == Opcode::Call {
            let site = &function.call_sites[instruction.bx() as usize];
            for reg in site.first_arg..site.first_arg + site.arg_count {
                live[reg as usize] = true;
            }
            if let Some(target) = site.target {
                live[target as usize] = true;
            }
        }
        for (bit, reg) in [(1, instruction.a), (2, instruction.b), (4, instruction.c)] {
            if mask & bit != 0 {
                live[reg as usize] = true;
            }
        }
        if mask == 8 {
            live.fill(true);
        }
        retained.push(instruction.encode());
    }
    retained.reverse();
    function.code = retained;
}

pub(super) fn effect(instruction: Instr, function: &BcFunction) -> (u8, Vec<usize>, bool) {
    let dest = vec![instruction.a as usize];
    match instruction.op {
        Opcode::Br | Opcode::BrIf | Opcode::BrCmp => (0, vec![], false),
        Opcode::Mov => (2, dest, true),
        Opcode::LoadI | Opcode::LoadK | Opcode::LoadNull | Opcode::LeaFn | Opcode::GlobalA => {
            (0, dest, true)
        }
        Opcode::Load if instruction.flags & 7 <= 3 => (2, dest, false),
        Opcode::Store if instruction.flags & 7 <= 3 => (3, vec![], false),
        Opcode::ALoad if instruction.flags & 7 <= 3 => (2, dest, false),
        Opcode::AStore if instruction.flags & 7 <= 3 => (3, vec![], false),
        Opcode::Add | Opcode::Sub | Opcode::Mul if instruction.flags & 7 <= 3 => {
            (6, dest, instruction.flags & 0x30 == 0)
        }
        Opcode::SDiv | Opcode::UDiv | Opcode::SRem | Opcode::URem if instruction.flags & 7 <= 3 => {
            (6, dest, false)
        }
        Opcode::And
        | Opcode::Or
        | Opcode::Xor
        | Opcode::Shl
        | Opcode::LShr
        | Opcode::AShr
        | Opcode::RotL
        | Opcode::RotR
            if instruction.flags & 7 <= 3 =>
        {
            (6, dest, true)
        }
        Opcode::Not
        | Opcode::Neg
        | Opcode::Ctlz
        | Opcode::Cttz
        | Opcode::Ctpop
        | Opcode::Bswap
        | Opcode::Bitrev
            if instruction.flags & 7 <= 3 =>
        {
            (2, dest, instruction.flags & 0x30 == 0)
        }
        Opcode::Bitcast => (2, dest, true),
        Opcode::PtrAdd | Opcode::PtrDiff => (6, dest, true),
        Opcode::HallocR => (6, dest, false),
        Opcode::ARmw if instruction.flags & 7 <= 3 => (6, dest, false),
        Opcode::Icmp if instruction.flags & 7 <= 3 => (6, dest, true),
        Opcode::FAdd
        | Opcode::FSub
        | Opcode::FMul
        | Opcode::FDiv
        | Opcode::FRem
        | Opcode::FMin
        | Opcode::FMax
        | Opcode::FCmp => (6, dest, true),
        Opcode::Trunc | Opcode::ZExt | Opcode::SExt
            if instruction.flags & 7 <= 3 && (instruction.flags >> 3) & 7 <= 3 =>
        {
            (2, dest, true)
        }
        Opcode::FNeg
        | Opcode::FAbs
        | Opcode::FSqrt
        | Opcode::FFloor
        | Opcode::FCeil
        | Opcode::FTrunc
        | Opcode::FRound => (2, dest, true),
        Opcode::MemCpy | Opcode::MemMove | Opcode::MemSet => (7, vec![], false),
        Opcode::RetV | Opcode::Fail => (1, vec![], false),
        Opcode::Ret | Opcode::Trap | Opcode::Unreach => (0, vec![], false),
        Opcode::SetJmp => (2, dest, false),
        Opcode::LongJmp => (3, vec![], false),
        Opcode::Alloca | Opcode::HallocC | Opcode::HallocU => (0, dest, false),
        Opcode::HfreeR | Opcode::Retain | Opcode::Release => (1, vec![], false),
        // An access carries its size and mode in `a` and `c`; only `b` names a
        // register, and neither claim nor release writes one.
        Opcode::AccBegin | Opcode::AccEnd => (2, vec![], false),
        Opcode::Call => {
            let site = &function.call_sites[instruction.bx() as usize];
            let mut writes = Vec::new();
            if site.has_result {
                writes.push(site.result as usize);
            }
            writes.extend(site.fallible.map(|fallible| fallible.failed as usize));
            (0, writes, false)
        }
        _ => (8, (0..function.register_count as usize).collect(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Outcome, Program, run};

    #[test]
    fn overwritten_alias_sources_keep_the_old_value() {
        let mut function = BcFunction {
            name: "alias".into(),
            register_count: 3,
            param_count: 1,
            returns_value: true,
            code: vec![
                Instr::abc(Opcode::Mov, 1, 0, 0).encode(),
                Instr::from_bx(Opcode::LoadI, 0, 42).encode(),
                Instr::abc(Opcode::Add, 2, 1, 0).with_flags(3).encode(),
                Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
            ],
            call_sites: vec![],
            allocations: vec![],
            branches: vec![],
        };
        local_registers(&mut function);
        let program = Program {
            functions: vec![function],
            consts: vec![],
            imports: vec![],
            globals: vec![],
            initializers: vec![],
        };
        assert_eq!(run(&program, 0, &[7]), Outcome::Return(Some(49)));
    }
    #[test]
    fn small_moves_keep_overlap_semantics_and_remap_branch_targets() {
        use crate::program::{AllocShape, BranchSite, BranchTarget};
        for (length, width) in [(1, 0), (2, 1), (4, 2), (8, 3)] {
            let mut function = BcFunction {
                name: "small_move".into(),
                register_count: 6,
                param_count: 1,
                returns_value: true,
                allocations: vec![AllocShape { size: 16, align: 8 }],
                call_sites: vec![],
                code: vec![
                    Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                    Instr::abc(Opcode::Store, 1, 0, 0)
                        .with_flags(width)
                        .encode(),
                    Instr::from_bx(Opcode::LoadI, 3, length).encode(),
                    Instr::from_bx(Opcode::LoadI, 4, 1).encode(),
                    Instr::abc(Opcode::Add, 5, 1, 4).with_flags(3).encode(),
                    Instr::abc(Opcode::MemMove, 5, 1, 3).encode(),
                    Instr::from_bx(Opcode::Br, 0, 0).encode(),
                    Instr::abc(Opcode::Load, 0, 5, 0).with_flags(width).encode(),
                    Instr::abc(Opcode::RetV, 0, 0, 0).encode(),
                ],
                branches: vec![BranchSite {
                    condition: None,
                    then_edge: BranchTarget {
                        target: 7,
                        moves: vec![],
                    },
                    else_edge: None,
                }],
            };
            let original = function.clone();
            local_registers(&mut function);
            assert!(
                function
                    .code
                    .iter()
                    .all(|word| Instr::decode(*word).unwrap().op != Opcode::MemMove)
            );
            let program = |function| Program {
                functions: vec![function],
                consts: vec![],
                imports: vec![],
                globals: vec![],
                initializers: vec![],
            };
            let value = 0xfedc_ba98_7654_3210;
            assert_eq!(
                run(&program(function), 0, &[value]),
                run(&program(original), 0, &[value])
            );
        }
    }
    #[test]
    fn constant_casts_preserve_signed_and_unsigned_narrowing() {
        for (value, width, extension, expected) in [
            (-1, 1, Opcode::ZExt, 65535),
            (-1, 1, Opcode::SExt, u64::MAX),
            (384, 0, Opcode::SExt, (-128i64) as u64),
        ] {
            let mut function = BcFunction {
                name: "constant_cast".into(),
                register_count: 3,
                param_count: 0,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::LoadI, 0, value).encode(),
                    Instr::abc(Opcode::Trunc, 1, 0, 0)
                        .with_flags(3 | (width << 3))
                        .encode(),
                    Instr::abc(extension, 2, 1, 0)
                        .with_flags(width | (3 << 3))
                        .encode(),
                    Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
                ],
                call_sites: vec![],
                allocations: vec![],
                branches: vec![],
            };
            local_registers(&mut function);
            assert!(function.code.iter().all(|word| !matches!(
                Instr::decode(*word).unwrap().op,
                Opcode::Trunc | Opcode::ZExt | Opcode::SExt
            )));
            let program = Program {
                functions: vec![function],
                consts: vec![],
                imports: vec![],
                globals: vec![],
                initializers: vec![],
            };
            assert_eq!(run(&program, 0, &[]), Outcome::Return(Some(expected)));
        }
    }
}
