//! Intern GPU specialization defaults in TYPES and CONSTANTS first-use order.
use std::collections::HashMap;

use tir_types::Type;

use crate::text_gpu_spec_const::GpuSpecialization;
use crate::text_signature_assembly::ParsedFunction;
use crate::types::{BinaryConstant, BinaryConstantTable};
use crate::{TextItem, TextModule, TextParseError};

pub(crate) type Specializations = Vec<Vec<(usize, GpuSpecialization)>>;

pub(crate) fn collect(
    module: &TextModule,
    functions: &[ParsedFunction],
) -> Result<Specializations, TextParseError> {
    functions
        .iter()
        .map(|function| {
            let mut results = Vec::new();
            if let Some(body) = &function.body {
                for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                    if instruction.opcode == "gpu.spec_const" {
                        let op = crate::text_gpu_spec_const::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        results.push((instruction.span.start, op));
                    }
                }
            }
            Ok(results)
        })
        .collect()
}

pub(crate) fn append_roots(
    functions: &[ParsedFunction],
    specializations: &Specializations,
    roots: &mut Vec<(Type, TextItem)>,
) {
    for (function, values) in functions.iter().zip(specializations) {
        roots.extend(
            values
                .iter()
                .map(|(_, constant)| (constant.ty.clone(), function.item)),
        );
    }
}

pub(crate) fn intern(
    functions: &[ParsedFunction],
    specializations: Specializations,
    types: &mut impl Iterator<Item = u64>,
    constants: &mut BinaryConstantTable,
    maps: &mut [HashMap<usize, (u64, u64)>],
) -> Result<(), TextParseError> {
    for ((function, values), map) in functions.iter().zip(specializations).zip(maps) {
        for (span, value) in values {
            let ty = types.next().ok_or_else(|| {
                error(function.item, "gpu.spec_const result type was not interned")
            })?;
            let index = u64::try_from(constants.records.len())
                .map_err(|_| error(function.item, "CONSTANTS count overflows ULEB"))?;
            constants.records.push(BinaryConstant {
                ty,
                value: value.default,
            });
            map.insert(span, (ty, index));
        }
    }
    Ok(())
}

fn error(item: TextItem, message: &str) -> TextParseError {
    crate::text_straight_line_encoding_helpers::error(item, tir_diag::Span::new(0, 0), message)
}
