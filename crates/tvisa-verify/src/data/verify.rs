use std::collections::HashMap;

use tir_diag::Diagnostic;
use tir_target::Profile;
use tvisa_format::{
    DataKind, DataRelocationKind, TextCallingConvention, TextData, TextDataInitializer,
    TextDataRelocation, TextImage, TextImportPayload, TextItemKind,
};

use super::parser::function_address_info;
use super::{TargetKind, VFY9, VFY10, VFY11};

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let targets = symbol_kinds(image);
    let mut diagnostics = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Data {
            continue;
        }
        let Some(data) = image.data_for_item(*item) else {
            continue;
        };
        if data.alignment == 0 || !data.alignment.is_power_of_two() {
            diagnostics.push(
                Diagnostic::error(VFY9, "data alignment must be a positive power of two")
                    .with_primary_span(data.span),
            );
        }
        let zero_kind = matches!(data.kind, DataKind::Zero | DataKind::TlsZero);
        if zero_kind != data.is_zero_initialized() {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    if zero_kind {
                        "zero-initialized data kinds require a zero initializer"
                    } else {
                        "zero initializers require a zero-initialized data kind"
                    },
                )
                .with_primary_span(data.initializer_span()),
            );
        }
        verify_aggregate(data, &mut diagnostics);
        if data.per_cpu
            && (image.header.profile != Profile::Kernel
                || matches!(
                    data.kind,
                    DataKind::Tls | DataKind::TlsZero | DataKind::GpuConstant
                ))
        {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "percpu data is only valid for non-TLS CPU data in the kernel profile",
                )
                .with_primary_span(data.span),
            );
        }
        verify_relocations(data, &targets, &mut diagnostics);
    }
    diagnostics
}

fn verify_aggregate(data: &TextData, diagnostics: &mut Vec<Diagnostic>) {
    let TextDataInitializer::Aggregate { size, elements, .. } = &data.initializer else {
        return;
    };
    let mut previous_end = 0_u64;
    for element in elements {
        if element.bytes.is_empty() {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data elements must be nonempty")
                    .with_primary_span(element.span),
            );
        }
        let Some(end) = element.offset.checked_add(element.bytes.len() as u64) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "aggregate data element range overflows")
                    .with_primary_span(element.span),
            );
            continue;
        };
        if element.offset < previous_end {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "aggregate data elements must be ordered and must not overlap",
                )
                .with_primary_span(element.span),
            );
        }
        if end > *size {
            diagnostics.push(
                Diagnostic::error(
                    VFY9,
                    "aggregate data element extends beyond its declared size",
                )
                .with_primary_span(element.span),
            );
        }
        previous_end = previous_end.max(end);
    }
}

fn symbol_kinds(image: &TextImage) -> HashMap<String, TargetKind> {
    let mut targets = HashMap::new();
    for item in &image.items {
        let source = image.item_text(*item);
        match item.kind {
            TextItemKind::Function => {
                let (Some(name), Some(signature_name)) = (
                    crate::gpu_restrictions::item_symbol(source),
                    crate::gpu::function_signature_name(source),
                ) else {
                    continue;
                };
                let gpu = image
                    .signatures
                    .iter()
                    .find(|signature| signature.name == signature_name)
                    .is_some_and(|signature| {
                        matches!(
                            signature.convention,
                            TextCallingConvention::Gpu | TextCallingConvention::Entry
                        )
                    });
                let (address_taken, tag) = function_address_info(source);
                targets.insert(
                    name,
                    TargetKind::DefinedFunction {
                        gpu,
                        address_taken,
                        tag,
                    },
                );
            }
            TextItemKind::Import => {
                if let Some(import) = image.import_for_item(*item) {
                    match &import.payload {
                        TextImportPayload::Data { .. } => {
                            targets.insert(import.symbol.clone(), TargetKind::DataImport);
                        }
                        TextImportPayload::Function { tag, .. } => {
                            targets.insert(
                                import.symbol.clone(),
                                TargetKind::ImportedFunction { tag: Some(*tag) },
                            );
                        }
                    }
                }
            }
            TextItemKind::Data => {
                if let Some(data) = image.data_for_item(*item) {
                    targets.insert(data.symbol.clone(), TargetKind::Data(data.kind));
                }
            }
            TextItemKind::Variants => {
                if let Some(name) = crate::gpu_restrictions::item_symbol(source) {
                    targets.insert(name, TargetKind::VariantSet);
                }
            }
            TextItemKind::Dispatch => {
                if let Some(name) = crate::gpu_restrictions::item_symbol(source) {
                    targets.insert(name, TargetKind::DispatchSet);
                }
            }
            TextItemKind::Entry => {
                if let Some(name) = crate::gpu_restrictions::item_symbol(source) {
                    targets.insert(name, TargetKind::GpuEntry);
                }
            }
            TextItemKind::Signature | TextItemKind::Metadata => {}
        }
    }
    targets
}

fn verify_relocations(
    data: &TextData,
    targets: &HashMap<String, TargetKind>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut occupied = Vec::new();
    for relocation in &data.relocations {
        let width = relocation.kind.width();
        let Some(end) = relocation.offset.checked_add(width) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation field range overflows")
                    .with_primary_span(relocation.span),
            );
            continue;
        };
        if end > data.size() {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation field extends beyond its data object")
                    .with_primary_span(relocation.span),
            );
        }
        if occupied.iter().any(|(start, previous_end): &(u64, u64)| {
            relocation.offset < *previous_end && *start < end
        }) {
            diagnostics.push(
                Diagnostic::error(VFY9, "data relocations overlap")
                    .with_primary_span(relocation.span),
            );
        }
        occupied.push((relocation.offset, end));

        if data.kind == DataKind::Readonly && relocation.kind != DataRelocationKind::Rel32 {
            diagnostics.push(
                Diagnostic::error(VFY9, "read-only data permits only REL32 relocations")
                    .with_primary_span(relocation.span),
            );
        }
        if relocation.tag.is_some()
            && !matches!(
                relocation.kind,
                DataRelocationKind::Code64 | DataRelocationKind::ImportCode64
            )
        {
            diagnostics.push(
                Diagnostic::error(VFY9, "only code-pointer relocations may carry a tag")
                    .with_primary_span(relocation.span),
            );
        }
        let target = targets.get(&relocation.symbol).copied();
        match target {
            Some(target) if relocation_target_matches(relocation.kind, target) => {}
            Some(_) => diagnostics.push(
                Diagnostic::error(
                    VFY10,
                    format!(
                        "{} relocation target @{} is in the wrong symbol space",
                        relocation_name(relocation.kind),
                        relocation.symbol
                    ),
                )
                .with_primary_span(relocation.symbol_span),
            ),
            None => diagnostics.push(
                Diagnostic::error(
                    VFY10,
                    format!("relocation names unknown symbol @{}", relocation.symbol),
                )
                .with_primary_span(relocation.symbol_span),
            ),
        }
        match (relocation.kind, target) {
            (
                DataRelocationKind::Code64,
                Some(TargetKind::DefinedFunction {
                    gpu: false,
                    address_taken,
                    tag,
                }),
            ) => {
                if !address_taken || tag.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY11,
                            format!(
                                "CODE64 relocation target @{} must be address_taken and tagged",
                                relocation.symbol
                            ),
                        )
                        .with_primary_span(relocation.symbol_span),
                    );
                }
                verify_relocation_tag(relocation, tag, diagnostics);
            }
            (DataRelocationKind::ImportCode64, Some(TargetKind::ImportedFunction { tag })) => {
                if tag.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY11,
                            format!(
                                "IMPORTCODE64 relocation target @{} has no function tag",
                                relocation.symbol
                            ),
                        )
                        .with_primary_span(relocation.symbol_span),
                    );
                }
                verify_relocation_tag(relocation, tag, diagnostics);
            }
            _ => {}
        }
    }
}

fn verify_relocation_tag(
    relocation: &TextDataRelocation,
    target_tag: Option<u32>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if relocation.tag.is_some_and(|tag| Some(tag) != target_tag) {
        diagnostics.push(
            Diagnostic::error(
                VFY11,
                format!(
                    "relocation tag does not match function @{}",
                    relocation.symbol
                ),
            )
            .with_primary_span(relocation.span),
        );
    }
}

fn relocation_target_matches(kind: DataRelocationKind, target: TargetKind) -> bool {
    match kind {
        DataRelocationKind::Abs64 => matches!(target, TargetKind::Data(_)),
        DataRelocationKind::Code64 => {
            matches!(target, TargetKind::DefinedFunction { gpu: false, .. })
        }
        DataRelocationKind::Rel32 => matches!(
            target,
            TargetKind::Data(_) | TargetKind::DefinedFunction { gpu: false, .. }
        ),
        DataRelocationKind::GpuEntry64 => matches!(target, TargetKind::GpuEntry),
        DataRelocationKind::Tpoff64 => {
            matches!(target, TargetKind::Data(DataKind::Tls | DataKind::TlsZero))
        }
        DataRelocationKind::Import64 => matches!(target, TargetKind::DataImport),
        DataRelocationKind::ImportCode64 => {
            matches!(target, TargetKind::ImportedFunction { .. })
        }
        DataRelocationKind::Dispatch64 => matches!(target, TargetKind::DispatchSet),
    }
}

fn relocation_name(kind: DataRelocationKind) -> &'static str {
    match kind {
        DataRelocationKind::Abs64 => "ABS64",
        DataRelocationKind::Code64 => "CODE64",
        DataRelocationKind::Rel32 => "REL32",
        DataRelocationKind::GpuEntry64 => "GPUENTRY64",
        DataRelocationKind::Tpoff64 => "TPOFF64",
        DataRelocationKind::Import64 => "IMPORT64",
        DataRelocationKind::ImportCode64 => "IMPORTCODE64",
        DataRelocationKind::Dispatch64 => "DISPATCH64",
    }
}
