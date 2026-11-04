use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryFunctionType, BinaryGlobalTable, BinaryImport, BinaryImportEntityKind,
    BinaryOperand, BinarySignature, BinarySignatureTable, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) struct ResolvedFunctionSymbol<'a> {
    pub(super) local_index: Option<u64>,
    pub(super) function_type_index: u64,
    pub(super) function_type: &'a BinaryFunctionType,
    pub(super) generic_parameter_count: usize,
    pub(super) signature: Option<&'a BinarySignature>,
    pub(super) import_reference: Option<(u64, u64)>,
    pub(super) name: String,
}

pub(super) fn resolve_function_symbol<'a>(
    symbol_index: u64,
    signatures: &'a BinarySignatureTable,
    types: &'a BinaryTypeTable,
    imports: &'a [BinaryImport],
) -> Option<ResolvedFunctionSymbol<'a>> {
    let imported_count = imported_entity_count(imports, BinaryImportEntityKind::Function);
    if symbol_index < imported_count {
        let mut current = 0_u64;
        for (import_index, import) in imports.iter().enumerate() {
            for (entity_index, entity) in import.entities.iter().enumerate() {
                if entity.kind != BinaryImportEntityKind::Function {
                    continue;
                }
                if current == symbol_index {
                    let function_type = function_type(types, entity.type_index)?;
                    return Some(ResolvedFunctionSymbol {
                        local_index: None,
                        function_type_index: entity.type_index,
                        function_type,
                        generic_parameter_count: 0,
                        signature: None,
                        import_reference: Some((import_index as u64, entity_index as u64)),
                        name: format!("{}.{}", import.name, entity.name),
                    });
                }
                current += 1;
            }
        }
        return None;
    }

    let local_index = symbol_index.checked_sub(imported_count)?;
    let signature = signatures.records.get(usize::try_from(local_index).ok()?)?;
    Some(ResolvedFunctionSymbol {
        local_index: Some(local_index),
        function_type_index: signature.function_type,
        function_type: function_type(types, signature.function_type)?,
        generic_parameter_count: signature.generic_parameters.len(),
        signature: Some(signature),
        import_reference: None,
        name: signature.name.clone(),
    })
}

pub(super) fn local_function_index(
    symbol_index: u64,
    imports: Option<&[BinaryImport]>,
) -> Option<u64> {
    symbol_index.checked_sub(imported_entity_count(
        imports.unwrap_or_default(),
        BinaryImportEntityKind::Function,
    ))
}

pub(super) fn imported_data_reference(
    symbol_index: u64,
    imports: &[BinaryImport],
) -> Option<(u64, u64)> {
    let imported_count = imports
        .iter()
        .flat_map(|import| &import.entities)
        .filter(|entity| {
            matches!(
                entity.kind,
                BinaryImportEntityKind::Global | BinaryImportEntityKind::PlatformData
            )
        })
        .count() as u64;
    if symbol_index >= imported_count {
        return None;
    }
    let mut current = 0_u64;
    for (import_index, import) in imports.iter().enumerate() {
        for (entity_index, entity) in import.entities.iter().enumerate() {
            if !matches!(
                entity.kind,
                BinaryImportEntityKind::Global | BinaryImportEntityKind::PlatformData
            ) {
                continue;
            }
            if current == symbol_index {
                return Some((import_index as u64, entity_index as u64));
            }
            current += 1;
        }
    }
    None
}

pub(super) fn verify_code_symbol_references(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    imports: &[BinaryImport],
    globals: Option<&BinaryGlobalTable>,
) -> Vec<Diagnostic> {
    let imported_functions = imported_entity_count(imports, BinaryImportEntityKind::Function);
    let imported_data = imports
        .iter()
        .flat_map(|import| &import.entities)
        .filter(|entity| {
            matches!(
                entity.kind,
                BinaryImportEntityKind::Global | BinaryImportEntityKind::PlatformData
            )
        })
        .count() as u64;
    let function_count = imported_functions.saturating_add(signatures.records.len() as u64);
    let data_count =
        imported_data.saturating_add(globals.map_or(0, |table| table.records.len() as u64));
    let mut diagnostics = Vec::new();

    for body in &code.functions {
        let Some(function) = usize::try_from(body.function_index)
            .ok()
            .and_then(|index| signatures.records.get(index))
        else {
            continue;
        };
        for block in &body.blocks {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for instruction in instructions {
                for operand in &instruction.operands {
                    let BinaryOperand::Symbol { kind, index } = operand else {
                        continue;
                    };
                    let (label, count) = match kind {
                        0 => ("function", function_count),
                        1 => ("data", data_count),
                        _ => continue,
                    };
                    if *index >= count {
                        diagnostics.push(Diagnostic::error(
                            V2,
                            format!(
                                "function @{} CODE offset {}: {label} symbol index {index} is out of range",
                                function.name, instruction.offset
                            ),
                        ));
                    }
                }
            }
        }
    }

    diagnostics
}

fn imported_entity_count(imports: &[BinaryImport], kind: BinaryImportEntityKind) -> u64 {
    imports
        .iter()
        .flat_map(|import| &import.entities)
        .filter(|entity| entity.kind == kind)
        .count() as u64
}

fn function_type(types: &BinaryTypeTable, index: u64) -> Option<&BinaryFunctionType> {
    match types.entries.get(usize::try_from(index).ok()?)? {
        BinaryType::Function(function) => Some(function),
        _ => None,
    }
}
