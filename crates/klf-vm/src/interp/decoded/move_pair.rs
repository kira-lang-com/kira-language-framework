use super::{DecodedInstr, DecodedMovePair};

pub(super) fn decode_move_pairs(code: &mut [DecodedInstr]) -> Vec<DecodedMovePair> {
    let mut pairs = Vec::new();
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::Mov {
            a: first_a,
            b: first_b,
        } = code[pc]
        else {
            pc += 1;
            continue;
        };
        let DecodedInstr::Mov {
            a: second_a,
            b: second_b,
        } = code[pc + 1]
        else {
            pc += 1;
            continue;
        };
        let Ok(index) = u32::try_from(pairs.len()) else {
            break;
        };
        pairs.push(DecodedMovePair {
            first_a,
            first_b,
            second_a,
            second_b,
        });
        code[pc] = DecodedInstr::MovePair(index);
        pc += 2;
    }
    pairs
}
