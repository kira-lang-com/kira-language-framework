//! The `klf.definition.*` service wrappers: the phase boundary a definition
//! crosses to hand the kernel a unit, an artifact or the payload it parked in
//! one. Kept apart from the KIR builder wrappers because these are the only
//! services whose arguments are Session-owned identities rather than values.

#[path = "builtins_low_level.rs"]
mod low_level;

use crate::builtins::{fe_handle, signed_i64};
use crate::ty::{DefinitionHandle, FeType};

pub(crate) fn wrapper_types(service_name: &str) -> Option<(Vec<FeType>, FeType)> {
    let types = match service_name {
        "klf.definition.unit" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::SourceFile),
            ],
            fe_handle(DefinitionHandle::Unit),
        ),
        "klf.definition.tree" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::Unit),
                signed_i64(),
            ],
            fe_handle(DefinitionHandle::Tree),
        ),
        "klf.definition.program" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::Unit),
                signed_i64(),
            ],
            fe_handle(DefinitionHandle::Program),
        ),
        "klf.definition.input" => (
            vec![fe_handle(DefinitionHandle::Session), signed_i64()],
            fe_handle(DefinitionHandle::SourceFile),
        ),
        "klf.definition.tree_payload" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::Tree),
            ],
            signed_i64(),
        ),
        "klf.definition.program_payload" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::Program),
            ],
            signed_i64(),
        ),
        "klf.session.flag" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                FeType::Pointer,
                signed_i64(),
                FeType::Pointer,
            ],
            signed_i64(),
        ),
        "klf.session.flag_string_copy" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                FeType::Pointer,
                signed_i64(),
                FeType::Pointer,
                signed_i64(),
            ],
            signed_i64(),
        ),
        "klf.session.file_mode" | "klf.session.file_memory" | "klf.session.file_convention" => (
            vec![
                fe_handle(DefinitionHandle::Session),
                fe_handle(DefinitionHandle::SourceFile),
            ],
            signed_i64(),
        ),
        _ => return low_level::wrapper_types(service_name),
    };
    Some(types)
}
