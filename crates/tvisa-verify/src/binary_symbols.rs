use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::TvisaImage;

const VFY8: Code = Code::new(Dialect::Tvisa, Area::Verifier, 8);
const VFY11: Code = Code::new(Dialect::Tvisa, Area::Verifier, 11);
const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);
const HAS_GPU: u32 = 1;
const HAS_UNMANAGED: u32 = 1 << 4;
const HAS_SANDBOXED: u32 = 1 << 5;
const HAS_C_FUNCTIONS: u32 = 1 << 7;

pub(super) fn verify_binary_records(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    if let Err(error) = image.validate_signature_indices(memory_limit) {
        return vec![error.diagnostic];
    }
    if let Err(error) = image.function_body_headers(memory_limit) {
        return vec![error.diagnostic];
    }
    let types = match image.type_section(memory_limit) {
        Ok(types) => types,
        Err(error) => return vec![error.diagnostic],
    };
    let mut diagnostics = super::binary_signatures::verify_types(&types);
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    for function in &functions {
        let Some(signature) = types.signatures.get(function.signature_index as usize) else {
            continue;
        };
        let valid_convention = match function.target {
            0 => signature.convention <= 2,
            1 => signature.convention == 3,
            2 => signature.convention == 4,
            _ => false,
        };
        if !valid_convention {
            diagnostics.push(Diagnostic::error(
                VFY22,
                format!(
                    "function @{} target {} disagrees with signature convention {}",
                    function.name, function.target, signature.convention
                ),
            ));
        }
    }
    check_header_flag(
        &mut diagnostics,
        image,
        HAS_GPU,
        functions.iter().any(|function| function.target != 0),
        "GPU code",
    );
    check_header_flag(
        &mut diagnostics,
        image,
        HAS_UNMANAGED,
        functions.iter().any(|function| function.trust_class == 2),
        "unmanaged functions",
    );
    check_header_flag(
        &mut diagnostics,
        image,
        HAS_SANDBOXED,
        functions.iter().any(|function| function.trust_class == 1),
        "sandboxed functions",
    );
    let has_c_functions = imports
        .function_signatures
        .iter()
        .copied()
        .chain(functions.iter().map(|function| function.signature_index))
        .any(|index| {
            types
                .signatures
                .get(index as usize)
                .is_some_and(|signature| signature.convention == 1)
        });
    check_header_flag(
        &mut diagnostics,
        image,
        HAS_C_FUNCTIONS,
        has_c_functions,
        "C functions",
    );
    if functions.iter().any(|function| function.target == 2)
        && !image.sections.iter().any(|section| section.kind == 11)
    {
        diagnostics.push(Diagnostic::error(
            VFY22,
            "GPU entry function is missing the GPU section record",
        ));
    }

    for function in functions {
        verify_slots(&mut diagnostics, &function);
        verify_type_tag(&mut diagnostics, &function);
        verify_trust_attribute(&mut diagnostics, &function);
    }
    diagnostics
}

fn verify_trust_attribute(
    diagnostics: &mut Vec<Diagnostic>,
    function: &tvisa_format::FunctionRecordSummary,
) {
    let attribute_sandboxed = function.attributes & (1 << 7) != 0;
    let trust_sandboxed = function.trust_class == 1;
    if attribute_sandboxed != trust_sandboxed {
        diagnostics.push(Diagnostic::error(
            VFY22,
            format!(
                "function @{} sandboxed attribute disagrees with its trust class",
                function.name
            ),
        ));
    }
}

fn verify_slots(diagnostics: &mut Vec<Diagnostic>, function: &tvisa_format::FunctionRecordSummary) {
    for slot in &function.slots {
        let valid_space = match function.target {
            0 => slot.address_space == 0,
            1 => slot.address_space == 4,
            2 => matches!(slot.address_space, 3 | 4),
            _ => false,
        };
        if !valid_space {
            diagnostics.push(Diagnostic::error(
                VFY8,
                format!(
                    "frame slot address space {} is invalid for function @{}",
                    slot.address_space, function.name
                ),
            ));
        }
    }
}

fn verify_type_tag(
    diagnostics: &mut Vec<Diagnostic>,
    function: &tvisa_format::FunctionRecordSummary,
) {
    let address_taken = function.address_taken();
    if address_taken != function.type_tag.is_some() {
        diagnostics.push(Diagnostic::error(
            VFY11,
            format!(
                "function @{} must carry a type tag exactly when address_taken is set",
                function.name
            ),
        ));
    }
    if function.linkage == 2 && function.target == 0 && !address_taken {
        diagnostics.push(Diagnostic::error(
            VFY11,
            format!(
                "exported CPU function @{} must be address_taken",
                function.name
            ),
        ));
    }
    if function.target != 0 && address_taken {
        diagnostics.push(Diagnostic::error(
            VFY11,
            format!(
                "GPU function @{} cannot use a CPU address_taken type tag",
                function.name
            ),
        ));
    }
}

fn check_header_flag(
    diagnostics: &mut Vec<Diagnostic>,
    image: &TvisaImage,
    flag: u32,
    actual: bool,
    description: &str,
) {
    if (image.header.flags & flag != 0) != actual {
        diagnostics.push(Diagnostic::error(
            VFY11,
            format!("header {description} flag disagrees with image records"),
        ));
    }
}
