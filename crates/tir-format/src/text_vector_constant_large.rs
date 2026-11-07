//! CONSTANTS-backed `const.vec` values whose vector layout exceeds 16 bytes.
use std::collections::HashMap;

use tir_types::Type;

use crate::text_signature_assembly::ParsedFunction;
use crate::text_vector_constant::VectorConstant;
use crate::types::{BinaryConstant, BinaryConstantTable};
use crate::{TextItem, TextModule, TextParseError};

pub(crate) type LargeVectors = Vec<Vec<(usize, VectorConstant)>>;

pub(crate) fn collect(
    module: &TextModule,
    functions: &[ParsedFunction],
) -> Result<LargeVectors, TextParseError> {
    functions
        .iter()
        .map(|function| {
            let mut vectors = Vec::new();
            if let Some(body) = &function.body {
                for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                    if instruction.opcode == "const.vec" {
                        let parsed = crate::text_vector_constant::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if !parsed.is_inline() {
                            vectors.push((instruction.span.start, parsed));
                        }
                    }
                }
            }
            Ok(vectors)
        })
        .collect()
}

pub(crate) fn append_roots(
    functions: &[ParsedFunction],
    vectors: &LargeVectors,
    roots: &mut Vec<(Type, TextItem)>,
) {
    for (function, vectors) in functions.iter().zip(vectors) {
        roots.extend(
            vectors
                .iter()
                .map(|(_, vector)| (vector.ty.clone(), function.item)),
        );
    }
}

pub(crate) fn intern(
    functions: &[ParsedFunction],
    vectors: &LargeVectors,
    types: &mut impl Iterator<Item = u64>,
    constants: &mut BinaryConstantTable,
    maps: &mut [HashMap<usize, (u64, u64)>],
) -> Result<(), TextParseError> {
    for ((function, vectors), map) in functions.iter().zip(vectors).zip(maps) {
        for (span, vector) in vectors {
            let ty = types.next().ok_or_else(|| {
                error(function.item, "large const.vec TYPES entry was not lowered")
            })?;
            let index = u64::try_from(constants.records.len())
                .map_err(|_| error(function.item, "large const.vec count overflows ULEB"))?;
            constants.records.push(BinaryConstant {
                ty,
                value: vector.constant_value(),
            });
            map.insert(*span, (ty, index));
        }
    }
    Ok(())
}

fn error(item: TextItem, message: &str) -> TextParseError {
    crate::text_straight_line_encoding_helpers::error(item, tir_diag::Span::new(0, 0), message)
}
