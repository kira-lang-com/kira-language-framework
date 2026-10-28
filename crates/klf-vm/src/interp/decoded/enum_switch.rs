use super::{
    DecodedBranchSite, DecodedCondition, DecodedEnumSwitch, DecodedEnumSwitchCase, DecodedInstr,
    join_u32,
};
use crate::bytecode::{Instr, Opcode, WidthCode, flags_width};

pub(super) fn decode_enum_switches(
    code: &mut [DecodedInstr],
    raw_instructions: &[Instr],
    branches: &[Option<DecodedBranchSite>],
) -> Vec<DecodedEnumSwitch> {
    let mut switches = Vec::new();
    let mut start = 0;
    while start + 2 < code.len() {
        if !matches!(
            code[start],
            DecodedInstr::LoadI {
                low: 0,
                high: 0,
                ..
            }
        ) {
            start += 1;
            continue;
        }
        let mut cursor = start;
        let mut tag = None;
        let mut width = None;
        let mut cases = Vec::new();
        while let Some(DecodedInstr::LoadI {
            a: constant_register,
            low,
            high,
        }) = code.get(cursor).copied()
        {
            let value = i64::from(join_u32(low, high) as i32);
            if value != cases.len() as i64 {
                break;
            }
            let Some((result_register, lhs, rhs, compare_width)) =
                equality_operands(code.get(cursor + 1).copied(), raw_instructions)
            else {
                break;
            };
            let tag_register = if lhs == constant_register {
                rhs
            } else if rhs == constant_register {
                lhs
            } else {
                break;
            };
            if tag_register == constant_register || tag_register == result_register {
                break;
            }
            if tag.is_some_and(|known| known != tag_register)
                || width.is_some_and(|known| known != compare_width)
            {
                break;
            }
            let Some(DecodedInstr::BranchIf { low, high }) = code.get(cursor + 2).copied() else {
                break;
            };
            let site = join_u32(low, high) as usize;
            let Some(branch) = branches.get(site).and_then(Option::as_ref) else {
                break;
            };
            if !matches!(branch.condition, Some(DecodedCondition::Register(register)) if register == result_register)
                || branch.else_edge.is_some()
            {
                break;
            }
            cases.push(DecodedEnumSwitchCase {
                constant_register,
                result_register,
                value: value as u64,
                edge: branch.then_edge.clone(),
            });
            tag = Some(tag_register);
            width = Some(compare_width);
            cursor += 3;
        }
        if cases.len() < 8 {
            start += 1;
            continue;
        }
        let end = cursor;
        if cases
            .iter()
            .any(|case| (start..end).contains(&case.edge.target))
        {
            start += 1;
            continue;
        }
        let Ok(switch_index) = u32::try_from(switches.len()) else {
            break;
        };
        switches.push(DecodedEnumSwitch {
            tag: tag.expect("a matched switch has a tag register"),
            width: width.expect("a matched switch has a comparison width"),
            cases,
            default_pc: end,
        });
        code[start] = DecodedInstr::EnumSwitch(switch_index);
        start = end;
    }
    switches
}

fn equality_operands(
    instruction: Option<DecodedInstr>,
    raw_instructions: &[Instr],
) -> Option<(u16, u16, u16, WidthCode)> {
    match instruction? {
        DecodedInstr::IcmpEq64 { a, b, c } => Some((a, b, c, WidthCode::W64)),
        DecodedInstr::Other { low, high } => {
            let raw = raw_instructions
                .get(join_u32(low, high) as usize)
                .copied()?;
            if raw.op != Opcode::Icmp || raw.flags >> 3 != 0 {
                return None;
            }
            let width = flags_width(raw.flags)?;
            (width != WidthCode::W128).then_some((raw.a, raw.b, raw.c, width))
        }
        _ => None,
    }
}
