use crate::binary::{FormatError, TvisaImage};
use tir_format::{BinaryDebugTable, DebugScopeKind};

impl TvisaImage {
    /// Decode the optional DEBUG section using the shared TIR table grammar.
    pub fn debug_table(&self, memory_limit: u64) -> Result<Option<BinaryDebugTable>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 15) else {
            return Ok(None);
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        BinaryDebugTable::decode(&bytes, &strings)
            .map(Some)
            .map_err(|error| FormatError::malformed(error.offset, error.diagnostic.message))
    }

    /// Check DEBUG references that depend on TVISA function, CODE and DATA tables.
    pub fn validate_debug_references(&self, memory_limit: u64) -> Result<(), FormatError> {
        let debug = self.debug_table(memory_limit)?.unwrap_or_default();
        let imports = self.import_counts(memory_limit)?;
        let functions = self.function_records(memory_limit)?;
        let data = self.data_section(memory_limit)?;
        let bodies = self.function_bodies(memory_limit)?;
        let first_defined_function = imports.functions;
        let total_functions = first_defined_function
            .checked_add(u64::from(self.header.function_count))
            .ok_or_else(|| FormatError::malformed(None, "DEBUG function range overflows"))?;

        for scope in &debug.scopes {
            if scope.function_index < first_defined_function
                || scope.function_index >= total_functions
            {
                return Err(FormatError::malformed(
                    None,
                    "DEBUG scope refers to an imported or out-of-range function",
                ));
            }
        }
        for (ordinal, (function, body)) in functions.iter().zip(&bodies).enumerate() {
            let function_index = first_defined_function + ordinal as u64;
            if function.debug_scope != 0 {
                let scope_index = function.debug_scope - 1;
                let scope = usize::try_from(scope_index)
                    .ok()
                    .and_then(|index| debug.scopes.get(index))
                    .ok_or_else(|| {
                        FormatError::malformed(None, "SIGNATURES debug scope is out of range")
                    })?;
                if scope.function_index != function_index
                    || !matches!(scope.kind, DebugScopeKind::Function)
                {
                    return Err(FormatError::malformed(
                        None,
                        "SIGNATURES debug scope is not this function's function scope",
                    ));
                }
            }

            for slot in &function.slots {
                if slot.debug_variable == 0 {
                    continue;
                }
                let variable_index = slot.debug_variable - 1;
                let variable = usize::try_from(variable_index)
                    .ok()
                    .and_then(|index| debug.variables.get(index))
                    .ok_or_else(|| {
                        FormatError::malformed(None, "frame slot debug variable is out of range")
                    })?;
                if debug
                    .scopes
                    .get(variable.scope as usize)
                    .is_none_or(|scope| scope.function_index != function_index)
                {
                    return Err(FormatError::malformed(
                        None,
                        "frame slot debug variable belongs to a different function",
                    ));
                }
            }

            let instruction_count = body
                .blocks
                .iter()
                .try_fold(0u64, |count, block| {
                    count.checked_add(block.instructions.len() as u64)
                })
                .ok_or_else(|| FormatError::malformed(None, "DEBUG instruction count overflows"))?;
            if debug.locations.iter().any(|location| {
                location.function_index == function_index
                    && location.instruction_ordinal >= instruction_count
            }) {
                return Err(FormatError::malformed(
                    None,
                    "DEBUG location ordinal is outside its function body",
                ));
            }
        }

        for location in &debug.locations {
            if location.function_index < first_defined_function
                || location.function_index >= total_functions
            {
                return Err(FormatError::malformed(
                    None,
                    "DEBUG location refers to an imported or out-of-range function",
                ));
            }
        }

        let mut data_record_ordinal = 0usize;
        for (data_index, object) in data.objects.iter().enumerate() {
            let record = debug.data.get(data_record_ordinal);
            let expected_debug_index =
                if record.is_some_and(|record| record.data_index == data_index as u64) {
                    data_record_ordinal += 1;
                    data_record_ordinal as u64
                } else {
                    0
                };
            if object.debug_index != expected_debug_index {
                return Err(FormatError::malformed(
                    None,
                    "DATA debug index disagrees with DEBUG record order",
                ));
            }
        }
        if data_record_ordinal != debug.data.len() {
            return Err(FormatError::malformed(
                None,
                "DEBUG DATA record references an invalid DATA object",
            ));
        }

        for (ordinal, body) in bodies.iter().enumerate() {
            let function_index = first_defined_function + ordinal as u64;
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                let record_index = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        crate::BinaryOperand::Index(index) => Some(*index),
                        _ => None,
                    });
                match instruction.opcode {
                    0x240 | 0x241 => {
                        let variable = record_index
                            .and_then(|index| usize::try_from(index).ok())
                            .and_then(|index| debug.variables.get(index))
                            .ok_or_else(|| {
                                FormatError::malformed(
                                    None,
                                    "debug.value or debug.addr references an invalid DEBUG variable",
                                )
                            })?;
                        if debug
                            .scopes
                            .get(variable.scope as usize)
                            .is_none_or(|scope| scope.function_index != function_index)
                        {
                            return Err(FormatError::malformed(
                                None,
                                "debug.value or debug.addr uses a variable from another function",
                            ));
                        }
                    }
                    0x242 => {
                        let label = record_index
                            .and_then(|index| usize::try_from(index).ok())
                            .and_then(|index| debug.labels.get(index))
                            .ok_or_else(|| {
                                FormatError::malformed(
                                    None,
                                    "debug.label references an invalid DEBUG label",
                                )
                            })?;
                        if debug
                            .scopes
                            .get(label.scope as usize)
                            .is_none_or(|scope| scope.function_index != function_index)
                        {
                            return Err(FormatError::malformed(
                                None,
                                "debug.label uses a label from another function",
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
