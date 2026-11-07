use std::fmt::Write as _;

use crate::{BinaryInstruction, BinaryOperand, BinaryTypeTable, FormatError};

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let op = crate::text_conversion_assembly::name(instruction.opcode)
        .ok_or_else(|| malformed("unsupported conversion opcode"))?;
    if instruction.result_count != 1 {
        return Err(malformed("conversion must define one SSA value"));
    }
    let [BinaryOperand::Value(source), BinaryOperand::Type(target)] =
        instruction.operands.as_slice()
    else {
        return Err(malformed(
            "conversion requires one value and destination type",
        ));
    };
    let (source_name, _) = usize::try_from(*source)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("conversion operand is not an earlier SSA value"))?;
    let source_name = source_name.clone();
    let target_name = crate::text_straight_line_disassembly::constant_type_name(types, *target)?;
    let mut modifiers = instruction.modifiers;
    let prefix = if matches!(instruction.opcode, 0x0a7 | 0x0a8) {
        let poison = 1u64 << 47;
        let overflow = modifiers & 3;
        let prefix = match (overflow, modifiers & poison != 0) {
            (2, false) => " [sat]",
            (1, false) => " [trap]",
            (3, true) => " [poison]",
            _ => return Err(malformed("invalid fptosi/fptoui overflow modifier")),
        };
        modifiers &= !(3 | poison);
        prefix
    } else {
        ""
    };
    let rounding = if matches!(instruction.opcode, 0x0a3 | 0x0a5 | 0x0a6 | 0x0ac) {
        let mode = (modifiers >> 13) & 7;
        modifiers &= !(7 << 13);
        match mode {
            0 => "",
            1 => " [rne]",
            2 => " [rtz]",
            3 => " [rup]",
            4 => " [rdn]",
            5 => " [rna]",
            _ => return Err(malformed("invalid conversion rounding mode")),
        }
    } else {
        ""
    };
    let saturation = if instruction.opcode == 0x0ac {
        let selector = (modifiers >> 48) & 0xf;
        modifiers &= !(0xf << 48);
        match selector {
            0 => "",
            1 => " [satfinite]",
            2 => " [nonsat]",
            _ => return Err(malformed("invalid fcvt saturation selector")),
        }
    } else {
        ""
    };
    if modifiers != 0 {
        return Err(malformed("conversion contains unsupported modifier bits"));
    }
    let result = format!("v{}", values.len());
    writeln!(
        text,
        "    %{result} = {op}{prefix} %{source_name} to {target_name}{rounding}{saturation}"
    )
    .map_err(|error| malformed(error.to_string()))?;
    values.push((result, *target));
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
