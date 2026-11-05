use std::collections::HashMap;

use tir_diag::{Diagnostic, Span};
use tir_target::Profile;
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind};

use super::parser::{function_address_info, parse_data};
use super::{DataKind, DataObject, Relocation, RelocationKind, TargetKind, VFY9, VFY10, VFY11};

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let targets = symbol_kinds(image);
    let mut diagnostics = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Data {
            continue;
        }
        let source = image.item_text(*item);
        let Some(data) = parse_data(source, &mut diagnostics) else {
            continue;
        };
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
                .with_primary_span(Span::new(item.span.start, item.span.end)),
            );
        }
        verify_relocations(&data, &targets, item.span.start, &mut diagnostics);
    }
    diagnostics
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
                if let Some(name) = crate::gpu_restrictions::imported_data_name(source) {
                    targets.insert(name, TargetKind::DataImport);
                } else if let Some(name) = crate::gpu_restrictions::item_symbol(source) {
                    targets.insert(
                        name,
                        TargetKind::ImportedFunction {
                            tag: function_address_info(source).1,
                        },
                    );
                }
            }
            TextItemKind::Data => {
                if let Some((name, kind, _per_cpu)) = crate::gpu_restrictions::data_record(source) {
                    targets.insert(name, TargetKind::Data(DataKind::parse(&kind)));
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
    data: &DataObject,
    targets: &HashMap<String, TargetKind>,
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut occupied = Vec::new();
    for relocation in &data.relocations {
        let width = relocation.kind.width();
        let Some(end) = relocation.offset.checked_add(width) else {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation field range overflows")
                    .with_primary_span(shift(relocation.span, base)),
            );
            continue;
        };
        if data.size.is_some_and(|size| end > size) {
            diagnostics.push(
                Diagnostic::error(VFY9, "relocation field extends beyond its data object")
                    .with_primary_span(shift(relocation.span, base)),
            );
        }
        if occupied.iter().any(|(start, previous_end): &(u64, u64)| {
            relocation.offset < *previous_end && *start < end
        }) {
            diagnostics.push(
                Diagnostic::error(VFY9, "data relocations overlap")
                    .with_primary_span(shift(relocation.span, base)),
            );
        }
        occupied.push((relocation.offset, end));

        if data.kind == DataKind::Readonly && !matches!(relocation.kind, RelocationKind::Rel32) {
            diagnostics.push(
                Diagnostic::error(VFY9, "read-only data permits only REL32 relocations")
                    .with_primary_span(shift(relocation.span, base)),
            );
        }
        let target = targets.get(&relocation.target).copied();
        match target {
            Some(target) if relocation_target_matches(relocation.kind, target) => {}
            Some(_) => diagnostics.push(
                Diagnostic::error(
                    VFY10,
                    format!(
                        "{} relocation target @{} is in the wrong symbol space",
                        relocation_name(relocation.kind),
                        relocation.target
                    ),
                )
                .with_primary_span(shift(relocation.target_span, base)),
            ),
            None => diagnostics.push(
                Diagnostic::error(
                    VFY10,
                    format!("relocation names unknown symbol @{}", relocation.target),
                )
                .with_primary_span(shift(relocation.target_span, base)),
            ),
        }
        match (relocation.kind, target) {
            (
                RelocationKind::Code64,
                Some(TargetKind::DefinedFunction {
                    gpu: false,
                    address_taken,
                    tag,
                    ..
                }),
            ) => {
                if !address_taken || tag.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY11,
                            format!(
                                "CODE64 relocation target @{} must be address_taken and tagged",
                                relocation.target
                            ),
                        )
                        .with_primary_span(shift(relocation.target_span, base)),
                    );
                }
                verify_relocation_tag(relocation, tag, base, diagnostics);
            }
            (RelocationKind::ImportCode64, Some(TargetKind::ImportedFunction { tag })) => {
                if tag.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY11,
                            format!(
                                "IMPORTCODE64 relocation target @{} has no function tag",
                                relocation.target
                            ),
                        )
                        .with_primary_span(shift(relocation.target_span, base)),
                    );
                }
                verify_relocation_tag(relocation, tag, base, diagnostics);
            }
            _ => {}
        }
    }
}

fn verify_relocation_tag(
    relocation: &Relocation,
    target_tag: Option<u32>,
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if relocation.tag.is_some_and(|tag| Some(tag) != target_tag) {
        diagnostics.push(
            Diagnostic::error(
                VFY11,
                format!(
                    "relocation tag does not match function @{}",
                    relocation.target
                ),
            )
            .with_primary_span(shift(relocation.span, base)),
        );
    }
}

fn relocation_target_matches(kind: RelocationKind, target: TargetKind) -> bool {
    match kind {
        RelocationKind::Abs64 => matches!(target, TargetKind::Data(_)),
        RelocationKind::Code64 => {
            matches!(target, TargetKind::DefinedFunction { gpu: false, .. })
        }
        RelocationKind::Rel32 => matches!(
            target,
            TargetKind::Data(_) | TargetKind::DefinedFunction { gpu: false, .. }
        ),
        RelocationKind::GpuEntry64 => matches!(target, TargetKind::GpuEntry),
        RelocationKind::Tpoff64 => {
            matches!(target, TargetKind::Data(DataKind::Tls | DataKind::TlsZero))
        }
        RelocationKind::Import64 => matches!(target, TargetKind::DataImport),
        RelocationKind::ImportCode64 => matches!(target, TargetKind::ImportedFunction { .. }),
        RelocationKind::Dispatch64 => matches!(target, TargetKind::DispatchSet),
    }
}

fn relocation_name(kind: RelocationKind) -> &'static str {
    match kind {
        RelocationKind::Abs64 => "ABS64",
        RelocationKind::Code64 => "CODE64",
        RelocationKind::Rel32 => "REL32",
        RelocationKind::GpuEntry64 => "GPUENTRY64",
        RelocationKind::Tpoff64 => "TPOFF64",
        RelocationKind::Import64 => "IMPORT64",
        RelocationKind::ImportCode64 => "IMPORTCODE64",
        RelocationKind::Dispatch64 => "DISPATCH64",
    }
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
