use crate::program::CallSite;

use super::{DecodedInstr, join_u32};

pub(super) fn decode_tail_calls(code: &mut [DecodedInstr], call_sites: &[CallSite]) {
    if code
        .iter()
        .any(|instruction| matches!(instruction, DecodedInstr::Alloca { .. }))
    {
        return;
    }
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::Call { low, high } = code[pc] else {
            pc += 1;
            continue;
        };
        let Some(site) = call_sites.get(join_u32(low, high) as usize) else {
            pc += 1;
            continue;
        };
        let tail_return = match code[pc + 1] {
            DecodedInstr::ReturnValue(register) => site.has_result && site.result == register,
            DecodedInstr::Return => !site.has_result,
            _ => false,
        };
        if site.foreign.is_some() || !tail_return {
            pc += 1;
            continue;
        }
        code[pc] = DecodedInstr::TailCall { low, high };
        pc += 2;
    }
}
