use super::DecodedInstr;

pub(super) fn decode_load_moves(code: &mut [DecodedInstr]) {
    let mut pc = 0;
    while pc + 1 < code.len() {
        let (load_a, load_b, load_width) = match code[pc] {
            DecodedInstr::Load8 { a, b } => (a, b, 1),
            DecodedInstr::Load16 { a, b } => (a, b, 2),
            DecodedInstr::Load32 { a, b } => (a, b, 4),
            DecodedInstr::Load64 { a, b } => (a, b, 8),
            _ => {
                pc += 1;
                continue;
            }
        };
        let DecodedInstr::Mov {
            a: move_a,
            b: move_b,
        } = code[pc + 1]
        else {
            pc += 1;
            continue;
        };
        if move_b != load_a {
            pc += 1;
            continue;
        }
        code[pc] = match load_width {
            1 => DecodedInstr::Load8Move {
                load_a,
                load_b,
                move_a,
            },
            2 => DecodedInstr::Load16Move {
                load_a,
                load_b,
                move_a,
            },
            4 => DecodedInstr::Load32Move {
                load_a,
                load_b,
                move_a,
            },
            8 => DecodedInstr::Load64Move {
                load_a,
                load_b,
                move_a,
            },
            _ => unreachable!(),
        };
        pc += 2;
    }
}
