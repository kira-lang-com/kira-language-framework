use super::{DecodedAddLoad, DecodedInstr};

pub(super) fn decode_add_loads(code: &mut [DecodedInstr]) -> Vec<DecodedAddLoad> {
    let mut operations = Vec::new();
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::Add64 { a, b, c } = code[pc] else {
            pc += 1;
            continue;
        };
        let (load_a, load_b, load_width) = match code[pc + 1] {
            DecodedInstr::Load8 { a, b } => (a, b, 1),
            DecodedInstr::Load16 { a, b } => (a, b, 2),
            DecodedInstr::Load32 { a, b } => (a, b, 4),
            DecodedInstr::Load64 { a, b } => (a, b, 8),
            _ => {
                pc += 1;
                continue;
            }
        };
        if load_b != a {
            pc += 1;
            continue;
        }
        let Ok(index) = u32::try_from(operations.len()) else {
            break;
        };
        operations.push(DecodedAddLoad {
            add_a: a,
            add_b: b,
            add_c: c,
            load_a,
            load_width,
        });
        code[pc] = DecodedInstr::AddLoad(index);
        pc += 2;
    }
    operations
}
