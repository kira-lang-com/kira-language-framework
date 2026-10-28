use super::{DecodedImmediateBinary, DecodedInstr, ImmediateBinaryKind};

pub(super) fn decode_immediate_binaries(code: &mut [DecodedInstr]) -> Vec<DecodedImmediateBinary> {
    let mut operations = Vec::new();
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::LoadI {
            a: constant_register,
            low,
            high,
        } = code[pc]
        else {
            pc += 1;
            continue;
        };
        let (kind, a, b, c) = match code[pc + 1] {
            DecodedInstr::Add64 { a, b, c } => (ImmediateBinaryKind::Add, a, b, c),
            DecodedInstr::Mul64 { a, b, c } => (ImmediateBinaryKind::Mul, a, b, c),
            _ => {
                pc += 1;
                continue;
            }
        };
        if constant_register != b && constant_register != c {
            pc += 1;
            continue;
        }
        let Ok(index) = u32::try_from(operations.len()) else {
            break;
        };
        let memory = code
            .get(pc + 2)
            .copied()
            .filter(|instruction| match instruction {
                DecodedInstr::Load8 { b, .. }
                | DecodedInstr::Load16 { b, .. }
                | DecodedInstr::Load32 { b, .. }
                | DecodedInstr::Load64 { b, .. } => *b == a,
                DecodedInstr::Store8 { a: address, .. }
                | DecodedInstr::Store16 { a: address, .. }
                | DecodedInstr::Store32 { a: address, .. }
                | DecodedInstr::Store64 { a: address, .. } => *address == a,
                _ => false,
            });
        operations.push(DecodedImmediateBinary {
            memory,
            constant_register,
            low,
            high,
            kind,
            a,
            b,
            c,
        });
        code[pc] = if memory.is_some() {
            DecodedInstr::LoadIBinaryMemory64(index)
        } else {
            DecodedInstr::LoadIBinary64(index)
        };
        pc += 2;
    }
    operations
}
