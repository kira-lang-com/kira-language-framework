//! CODE-local names and text literals interned into STRINGS by first use.
use crate::text_signature_assembly::ParsedFunction;
use crate::{TextModule, TextParseError};

pub(crate) fn collect(
    module: &TextModule,
    functions: &[ParsedFunction],
) -> Result<Vec<Vec<(usize, String)>>, TextParseError> {
    let mut result = Vec::with_capacity(functions.len());
    for function in functions {
        let mut strings = Vec::new();
        if let Some(body) = &function.body {
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                match instruction.opcode.as_str() {
                    "const.string" => {
                        let constant =
                            crate::text_function_constant_assembly::parse_entry_constant(
                                instruction,
                                module.item_text(function.item),
                                function.item,
                            )?;
                        let crate::text_function_constant_assembly::ParsedEntryConstant::String(
                            value,
                        ) = constant
                        else {
                            unreachable!("const.string parser returned another constant kind")
                        };
                        strings.push((instruction.span.start, value.value));
                    }
                    "gpu.builtin" => {
                        let builtin = crate::text_gpu_builtin::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        strings.push((instruction.span.start, builtin.name));
                    }
                    "gpu.printf" => {
                        let parsed = crate::text_gpu_printf::parse(instruction, function.item)?;
                        strings.push((instruction.span.start, parsed.format));
                    }
                    "target.has" => {
                        let parsed = crate::text_simple_instructions::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_simple_instructions::SimpleOperation::Feature {
                            name,
                            ..
                        } = parsed
                        {
                            strings.push((instruction.span.start, name));
                        }
                    }
                    _ => {}
                }
            }
        }
        result.push(strings);
    }
    Ok(result)
}
