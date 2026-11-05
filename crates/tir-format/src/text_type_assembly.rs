mod declarations;
mod lower;
mod members;
mod policies;
mod syntax;

use crate::types::{BinaryNominalDeclaration, BinaryTypeTable};
use crate::{TextItem, TextItemKind, TextModule, TextParseError};
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::Type;

pub(crate) fn parse_text_type_table_with_roots(
    module: &TextModule,
    leading_roots: &[(Type, TextItem)],
    middle_roots: &[(Type, TextItem)],
    trailing_roots: &[(Type, TextItem)],
) -> Result<(BinaryTypeTable, Vec<u64>, Vec<u64>, Vec<u64>), TextParseError> {
    let function_indices = lower::function_indices(module)?;
    let mut parsed = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Type)
    {
        parsed.push(declarations::parse_declaration(module, *item)?);
    }
    let mut names = HashMap::new();
    for (index, declaration) in parsed.iter().enumerate() {
        if names
            .insert(declaration.declaration.name.clone(), index as u64)
            .is_some()
        {
            return Err(syntax::item_error(
                declaration.item,
                Span::new(0, 0),
                "duplicate nominal declaration",
            ));
        }
    }
    let declarations = parsed
        .iter()
        .map(|declaration| declaration.declaration.clone())
        .collect::<Vec<BinaryNominalDeclaration>>();
    let mut builder = lower::TypeTableBuilder::new(&declarations, &names, &function_indices);
    let leading_indices = leading_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let mut definitions = Vec::new();
    for (index, declaration) in parsed.iter().enumerate() {
        definitions.push(members::parse_definition(
            module,
            declaration,
            index as u64,
            &mut builder,
        )?);
    }
    let middle_indices = middle_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let trailing_indices = trailing_roots
        .iter()
        .map(|(ty, item)| builder.intern_external_type(ty, *item))
        .collect::<Result<Vec<_>, _>>()?;
    let entries = builder.into_entries();
    Ok((
        BinaryTypeTable {
            declarations,
            entries,
            definitions,
        },
        leading_indices,
        middle_indices,
        trailing_indices,
    ))
}
