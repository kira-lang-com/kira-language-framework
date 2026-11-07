use std::collections::HashMap;

use tir_types::Type;

use crate::text_aggregate_constant::AggregateConstant;
use crate::text_signature_assembly::ParsedFunction;
use crate::types::{BinaryConstant, BinaryConstantTable};
use crate::{TextItem, TextModule, TextParseError};

pub(crate) type Aggregates = Vec<Vec<(usize, AggregateConstant)>>;

pub(crate) fn collect(
    module: &TextModule,
    functions: &[ParsedFunction],
    symbols: &crate::text_direct_call::Callees,
) -> Result<Aggregates, TextParseError> {
    functions
        .iter()
        .map(|function| {
            let mut aggregates = Vec::new();
            if let Some(body) = &function.body {
                for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                    if instruction.opcode == "const.aggregate" {
                        let value = crate::text_aggregate_constant::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                            Some(symbols),
                        )?;
                        aggregates.push((instruction.span.start, value));
                    }
                }
            }
            Ok(aggregates)
        })
        .collect()
}

pub(crate) fn append_roots(
    functions: &[ParsedFunction],
    aggregates: &Aggregates,
    roots: &mut Vec<(Type, TextItem)>,
) {
    for (function, values) in functions.iter().zip(aggregates) {
        roots.extend(
            values
                .iter()
                .map(|(_, constant)| (constant.ty.clone(), function.item)),
        );
    }
}

pub(crate) fn intern(
    functions: &[ParsedFunction],
    aggregates: Aggregates,
    types: &mut impl Iterator<Item = u64>,
    constants: &mut BinaryConstantTable,
    maps: &mut [HashMap<usize, (u64, u64)>],
) -> Result<(), TextParseError> {
    for ((function, values), map) in functions.iter().zip(aggregates).zip(maps) {
        for (span, value) in values {
            let ty = types
                .next()
                .ok_or_else(|| error(function.item, "const.aggregate type was not interned"))?;
            let index = u64::try_from(constants.records.len())
                .map_err(|_| error(function.item, "CONSTANTS count overflows ULEB"))?;
            constants.records.push(BinaryConstant {
                ty,
                value: value.value,
            });
            map.insert(span, (ty, index));
        }
    }
    Ok(())
}

fn error(item: TextItem, message: &str) -> TextParseError {
    crate::text_straight_line_encoding_helpers::error(item, tir_diag::Span::new(0, 0), message)
}
