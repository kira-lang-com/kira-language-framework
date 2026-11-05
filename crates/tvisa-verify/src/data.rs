mod model;
mod parser;
mod verify;

use model::{DataKind, DataObject, Relocation, RelocationKind, TargetKind};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_target::Profile;
use tvisa_format::{DataRelocationKind, TextImage, TvisaImage};

pub(super) const VFY9: Code = Code::new(Dialect::Tvisa, Area::Verifier, 9);
pub(super) const VFY10: Code = Code::new(Dialect::Tvisa, Area::Verifier, 10);
pub(super) const VFY11: Code = Code::new(Dialect::Tvisa, Area::Verifier, 11);

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    verify::verify_image(image)
}

pub(super) fn verify_binary(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let section = match image.data_section(memory_limit) {
        Ok(section) => section,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(counts) => counts,
        Err(error) => return vec![error.diagnostic],
    };
    let variants = match image.variant_index(memory_limit) {
        Ok(index) => index,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(records) => records,
        Err(error) => return vec![error.diagnostic],
    };
    let defined_data_count = section.objects.len() as u64;
    let data_limit = imports.data.saturating_add(defined_data_count);
    let mut diagnostics = Vec::new();
    for object in &section.objects {
        if object.per_cpu
            && (image.header.profile != Profile::Kernel
                || matches!(
                    object.kind,
                    tvisa_format::DataKind::Tls
                        | tvisa_format::DataKind::TlsZero
                        | tvisa_format::DataKind::GpuConstant
                ))
        {
            diagnostics.push(Diagnostic::error(
                VFY10,
                "percpu data is only valid for non-TLS CPU data in the kernel profile",
            ));
        }
        for relocation in &object.relocations {
            let space = relocation.symbol_space();
            let index = relocation.symbol_index();
            let valid_space = match relocation.kind {
                DataRelocationKind::Abs64
                | DataRelocationKind::Tpoff64
                | DataRelocationKind::Import64 => relocation.symbol_space() == 1,
                DataRelocationKind::Code64
                | DataRelocationKind::GpuEntry64
                | DataRelocationKind::ImportCode64 => relocation.symbol_space() == 0,
                DataRelocationKind::Rel32 => matches!(relocation.symbol_space(), 0 | 1),
                DataRelocationKind::Dispatch64 => relocation.symbol_space() == 2,
            };
            if !valid_space {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    format!(
                        "{:?} relocation uses symbol space {}",
                        relocation.kind,
                        relocation.symbol_space()
                    ),
                ));
                continue;
            }
            let valid_target = match relocation.kind {
                DataRelocationKind::Abs64 => {
                    space == 1 && index >= imports.data && index < data_limit
                }
                DataRelocationKind::Code64 => {
                    defined_function(&functions, imports.functions, index)
                        .is_some_and(|function| function.target == 0 && function.address_taken())
                }
                DataRelocationKind::GpuEntry64 => {
                    defined_function(&functions, imports.functions, index)
                        .is_some_and(|function| function.target == 2)
                }
                DataRelocationKind::Rel32 if space == 1 => {
                    index >= imports.data && index < data_limit
                }
                DataRelocationKind::Rel32 => defined_function(&functions, imports.functions, index)
                    .is_some_and(|function| function.target == 0),
                DataRelocationKind::Tpoff64 => {
                    index >= imports.data
                        && index < data_limit
                        && usize::try_from(index - imports.data)
                            .ok()
                            .and_then(|object| section.objects.get(object))
                            .is_some_and(|object| {
                                matches!(
                                    object.kind,
                                    tvisa_format::DataKind::Tls | tvisa_format::DataKind::TlsZero
                                )
                            })
                }
                DataRelocationKind::Import64 => index < imports.data,
                DataRelocationKind::ImportCode64 => index < imports.functions,
                DataRelocationKind::Dispatch64 => usize::try_from(index)
                    .ok()
                    .and_then(|index| variants.dispatch.get(index))
                    .is_some_and(|is_dispatch| *is_dispatch),
            };
            if !valid_target {
                diagnostics.push(Diagnostic::error(
                    VFY10,
                    format!(
                        "{:?} relocation references invalid symbol index {index} in space {space}",
                        relocation.kind
                    ),
                ));
            }
            if matches!(
                relocation.kind,
                DataRelocationKind::Code64 | DataRelocationKind::ImportCode64
            ) {
                let target_tag = if relocation.kind == DataRelocationKind::ImportCode64 {
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| imports.function_tags.get(index))
                        .copied()
                } else {
                    defined_function(&functions, imports.functions, index)
                        .and_then(|function| function.type_tag)
                };
                if target_tag.is_none() || relocation.tag.is_some_and(|tag| Some(tag) != target_tag)
                {
                    diagnostics.push(Diagnostic::error(
                        VFY11,
                        format!(
                            "{:?} relocation has a missing or mismatched function tag",
                            relocation.kind
                        ),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn defined_function(
    functions: &[tvisa_format::FunctionRecordSummary],
    import_count: u64,
    index: u64,
) -> Option<&tvisa_format::FunctionRecordSummary> {
    let defined_index = index.checked_sub(import_count)?;
    usize::try_from(defined_index)
        .ok()
        .and_then(|index| functions.get(index))
}
