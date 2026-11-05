use std::fmt::Write as _;

use crate::StringTable;
use crate::types::{
    BinaryCodeTable, BinaryConstantTable, BinaryEngine, BinaryFunctionBody, BinaryFunctionType,
    BinarySignature, BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable,
};

pub(crate) fn format_function_declarations(
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    code: &BinaryCodeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
) -> Result<String, crate::FormatError> {
    let mut text = String::new();
    for (function_index, record) in signatures.records.iter().enumerate() {
        let function = match types.entries.get(index(record.function_type)?) {
            Some(BinaryType::Function(function)) => function,
            _ => return Err(malformed("signature type is not a function type")),
        };
        let body = code
            .functions
            .iter()
            .find(|body| body.function_index == function_index as u64);
        format_function(
            &mut text, record, function, types, signatures, body, constants, strings,
        )?;
    }
    Ok(text)
}

fn format_function(
    text: &mut String,
    record: &BinarySignature,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    body: Option<&BinaryFunctionBody>,
    constants: &BinaryConstantTable,
    strings: &StringTable,
) -> Result<(), crate::FormatError> {
    if !record.generic_parameters.is_empty()
        || record.coroutine.is_some()
        || !record.instance_types.is_empty()
        || record.variant.is_some()
        || record.entry_kind.is_some()
        || record.platform_export.is_some()
        || function.foreign
    {
        return Err(malformed(
            "tirc dis does not yet support this function signature attribute",
        ));
    }
    write!(
        text,
        "\nfunc {} @{} (",
        linkage_name(record.linkage),
        record.name
    )
    .map_err(formatting_error)?;
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index != 0 {
            text.push_str(", ");
        }
        write!(text, "%arg{index}: ").map_err(formatting_error)?;
        if parameter.context {
            text.push_str("context ");
        }
        match parameter.convention {
            0 => text.push_str("owned "),
            1 => text.push_str("borrow "),
            2 => text.push_str("borrowmut "),
            3 => {}
            _ => return Err(malformed("unknown function parameter convention")),
        }
        text.push_str(&crate::text_type_disassembly::format_type_reference(
            types,
            parameter.ty,
            signatures,
        )?);
    }
    text.push_str(") -> ");
    text.push_str(&crate::text_type_disassembly::format_type_reference(
        types,
        function.result,
        signatures,
    )?);
    if let Some(error) = function.error {
        text.push_str(" fails ");
        text.push_str(&crate::text_type_disassembly::format_type_reference(
            types, error, signatures,
        )?);
    }
    if let Some(engine) = record.engine {
        write!(text, " engine({})", engine_name(engine)).map_err(formatting_error)?;
    } else if record.target != BinaryTarget::Gpu {
        return Err(malformed("CPU function is missing its engine"));
    }
    if let Some(semantics) = record.semantics {
        write!(text, " semantics({})", semantics_name(semantics)).map_err(formatting_error)?;
    }
    match record.target {
        BinaryTarget::Cpu => {}
        BinaryTarget::Gpu => text.push_str(" target(gpu)"),
        BinaryTarget::Both => text.push_str(" target(cpu, gpu)"),
    }
    if function.asynchronous {
        text.push_str(" async");
    }
    if function.c_abi {
        text.push_str(" abi(c)");
    }
    if let Some(mode) = record.fp_mode {
        write!(text, " fp({})", fp_mode_name(mode)).map_err(formatting_error)?;
    }
    if let Some(strategy) = record.generic_strategy {
        write!(text, " strategy({})", strategy_name(strategy)).map_err(formatting_error)?;
    }
    for (bit, name) in [
        (41, "cold"),
        (42, "nounwind"),
        (43, "noreturn"),
        (44, "inline"),
        (45, "noinline"),
    ] {
        if record.modifiers & (1 << bit) != 0 {
            write!(text, " {name}").map_err(formatting_error)?;
        }
    }
    if record.structured {
        text.push_str(" structured");
    }
    if record.kernel_fp {
        text.push_str(" kernel.fp");
    }
    if record.returns_twice {
        text.push_str(" returns_twice");
    }
    if record.inlinable {
        text.push_str(" inlinable");
    }
    if let Some(body) = body {
        crate::text_function_body_disassembly::format_return_body(
            text, body, function, types, signatures, constants, strings,
        )?;
    }
    Ok(())
}

fn linkage_name(linkage: crate::types::BinaryLinkage) -> &'static str {
    match linkage {
        crate::types::BinaryLinkage::Private => "private",
        crate::types::BinaryLinkage::Module => "module",
        crate::types::BinaryLinkage::Package => "package",
        crate::types::BinaryLinkage::Exported => "exported",
    }
}

fn engine_name(engine: BinaryEngine) -> &'static str {
    match engine {
        BinaryEngine::Native => "native",
        BinaryEngine::Runtime => "runtime",
        BinaryEngine::Any => "any",
    }
}

fn semantics_name(semantics: crate::SemanticsMode) -> &'static str {
    match semantics {
        crate::SemanticsMode::Defined => "defined",
        crate::SemanticsMode::Permissive => "permissive",
    }
}

fn fp_mode_name(mode: crate::types::BinaryFpMode) -> &'static str {
    match mode {
        crate::types::BinaryFpMode::Precise => "precise",
        crate::types::BinaryFpMode::Fast => "fast",
        crate::types::BinaryFpMode::IeeeDenormals => "ieee_denormals",
    }
}

fn strategy_name(strategy: crate::types::BinaryGenericStrategy) -> &'static str {
    match strategy {
        crate::types::BinaryGenericStrategy::Specialize => "specialize",
        crate::types::BinaryGenericStrategy::Polymorphic => "polymorphic",
        crate::types::BinaryGenericStrategy::Both => "both",
    }
}

fn index(value: u64) -> Result<usize, crate::FormatError> {
    usize::try_from(value).map_err(|_| malformed("function type index overflows memory"))
}

fn formatting_error(_: std::fmt::Error) -> crate::FormatError {
    malformed("TIR text formatting failed")
}

fn malformed(message: impl Into<String>) -> crate::FormatError {
    crate::FormatError::malformed(None, message)
}
