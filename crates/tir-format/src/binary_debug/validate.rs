use crate::FormatError;

use super::{BinaryDebugTable, DebugScopeKind, DebugSourceLocation, DebugSourceType};

fn fail(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}

fn within(index: u64, count: usize, what: &str) -> Result<(), FormatError> {
    if index >= count as u64 {
        return Err(fail(format!("DEBUG {what} index is out of range")));
    }
    Ok(())
}

fn source(source: &DebugSourceLocation, files: usize) -> Result<(), FormatError> {
    within(source.file_index, files, "file")?;
    if source.line == 0 || source.column == 0 {
        return Err(fail("DEBUG source line/column must be one-based"));
    }
    Ok(())
}

pub(super) fn validate(table: &BinaryDebugTable) -> Result<(), FormatError> {
    for ty in &table.source_types {
        let count = table.source_types.len();
        match ty {
            DebugSourceType::Base {
                encoding,
                bit_width,
                ..
            } => {
                if *encoding == 0 || *bit_width == 0 {
                    return Err(fail("DEBUG base encoding and bit width must be nonzero"));
                }
            }
            DebugSourceType::Composite {
                kind,
                size,
                alignment,
                members,
                ..
            } => {
                if *kind > 1 || !alignment.is_power_of_two() {
                    return Err(fail("DEBUG composite kind/alignment is invalid"));
                }
                for member in members {
                    within(member.type_index, count, "member source type")?;
                    if member.byte_offset > *size {
                        return Err(fail("DEBUG composite member lies beyond its size"));
                    }
                }
            }
            DebugSourceType::Enum {
                underlying_type, ..
            } => within(*underlying_type, count, "enum underlying type")?,
            DebugSourceType::Pointer { pointee_type }
            | DebugSourceType::Reference { pointee_type, .. } => {
                within(*pointee_type, count, "pointee source type")?
            }
            DebugSourceType::Typedef { type_index, .. }
            | DebugSourceType::Qualified { type_index, .. } => {
                within(*type_index, count, "referenced source type")?
            }
            DebugSourceType::TemplateParameter {
                kind, value_type, ..
            } => {
                if *kind > 1
                    || (*kind == 0 && value_type.is_some())
                    || (*kind == 1 && value_type.is_none())
                {
                    return Err(fail(
                        "DEBUG template parameter kind and value type disagree",
                    ));
                }
                if let Some(index) = value_type {
                    within(*index, count, "template value type")?;
                }
            }
            DebugSourceType::Array { element_type, .. } => {
                within(*element_type, count, "array element type")?
            }
        }
        if let DebugSourceType::Qualified { qualifiers, .. } = ty
            && *qualifiers & !7 != 0
        {
            return Err(fail("DEBUG type has invalid qualifier bits"));
        }
    }
    for (index, scope) in table.scopes.iter().enumerate() {
        within(
            scope.compile_unit,
            table.compile_units.len(),
            "compile unit",
        )?;
        match &scope.kind {
            DebugScopeKind::Function if scope.parent.is_none() => {}
            DebugScopeKind::Lexical | DebugScopeKind::Inlined { .. } => {
                let parent = scope
                    .parent
                    .ok_or_else(|| fail("DEBUG nested scope has no parent"))?;
                if parent >= index as u64
                    || table.scopes[parent as usize].function_index != scope.function_index
                {
                    return Err(fail(
                        "DEBUG nested scope parent is not earlier in the same function",
                    ));
                }
            }
            _ => return Err(fail("DEBUG function scope must have no parent")),
        }
        if let DebugScopeKind::Inlined { call_site } = &scope.kind {
            source(call_site, table.files.len())?;
        }
    }
    let mut previous = None;
    for location in &table.locations {
        let key = (location.function_index, location.instruction_ordinal);
        if previous.is_some_and(|old| old >= key) {
            return Err(fail(
                "DEBUG locations must be strictly ordered by function and ordinal",
            ));
        }
        previous = Some(key);
        source(&location.source, table.files.len())?;
        if let Some(scope) = location.scope {
            within(scope, table.scopes.len(), "location scope")?;
            if table.scopes[scope as usize].function_index != location.function_index {
                return Err(fail("DEBUG location scope belongs to another function"));
            }
        }
    }
    for variable in &table.variables {
        within(
            variable.source_type,
            table.source_types.len(),
            "variable source type",
        )?;
        within(variable.scope, table.scopes.len(), "variable scope")?;
    }
    for label in &table.labels {
        within(label.scope, table.scopes.len(), "label scope")?;
        source(&label.source, table.files.len())?;
    }
    let mut previous_data = None;
    for data in &table.data {
        if previous_data.is_some_and(|old| old >= data.data_index) {
            return Err(fail("DEBUG DATA records must be strictly ordered"));
        }
        previous_data = Some(data.data_index);
        within(
            data.source_type,
            table.source_types.len(),
            "DATA source type",
        )?;
        source(&data.source, table.files.len())?;
    }
    Ok(())
}
