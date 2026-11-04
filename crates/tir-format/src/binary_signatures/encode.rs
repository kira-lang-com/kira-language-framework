use std::collections::HashSet;

use crate::leb::write_uleb;
use crate::types::{
    BinaryEngine, BinaryEntryKind, BinaryFpMode, BinaryGenericStrategy, BinaryLinkage,
    BinarySignature, BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, SemanticsMode, StringTable};

pub(super) fn encode_signature_table(
    table: &BinarySignatureTable,
    strings: &StringTable,
    types: &BinaryTypeTable,
) -> Result<Vec<u8>, FormatError> {
    let count = u64::try_from(table.records.len())
        .map_err(|_| FormatError::malformed(None, "SIGNATURES count overflows"))?;
    if count >= super::MAX_SIGNATURE_COUNT {
        return Err(FormatError::malformed(
            None,
            "SIGNATURES count must be less than 2^32",
        ));
    }
    let mut bytes = Vec::new();
    write_index(count, &mut bytes)?;
    let mut names = HashSet::new();
    for signature in &table.records {
        encode_signature(signature, strings, types, &mut names, &mut bytes)?;
    }
    super::validate_frame_references(types, table.records.len())?;
    Ok(bytes)
}

fn encode_signature(
    signature: &BinarySignature,
    strings: &StringTable,
    types: &BinaryTypeTable,
    names: &mut HashSet<String>,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if signature.name.is_empty() || !names.insert(signature.name.clone()) {
        return Err(FormatError::malformed(
            None,
            "function name is empty or duplicated",
        ));
    }
    if signature.modifiers & !super::SIGNATURE_MODIFIER_MASK != 0 {
        return Err(FormatError::malformed(
            None,
            "SIGNATURES has unsupported function modifier bits",
        ));
    }
    if signature.modifiers & (1 << 44) != 0 && signature.modifiers & (1 << 45) != 0 {
        return Err(FormatError::malformed(
            None,
            "function is both inline and noinline",
        ));
    }
    let function = match super::get_type(types, signature.function_type, 0)? {
        BinaryType::Function(function) => function,
        _ => {
            return Err(FormatError::malformed(
                None,
                "signature type is not a function type",
            ));
        }
    };
    let mut parameter_names = HashSet::new();
    for parameter in &signature.generic_parameters {
        if parameter.name.is_empty() || !parameter_names.insert(parameter.name.as_str()) {
            return Err(FormatError::malformed(
                None,
                "generic parameter name is empty or duplicated",
            ));
        }
        let mut bounds = HashSet::new();
        for bound in &parameter.bounds {
            if bound.is_empty() || !bounds.insert(bound.as_str()) {
                return Err(FormatError::malformed(
                    None,
                    "generic bound is empty or duplicated",
                ));
            }
        }
    }
    let mut cursor = 0;
    super::validate_callable_attributes(
        &mut cursor,
        function,
        signature.engine,
        signature.target,
        signature.entry_kind,
        signature.variant.as_ref(),
        signature.platform_export.as_deref(),
    )?;
    super::validate_function_generics(
        &mut cursor,
        types,
        function,
        signature.generic_parameters.len(),
    )?;
    for type_index in signature
        .coroutine
        .iter()
        .flat_map(|(yield_type, resume_type)| [yield_type, resume_type])
        .chain(signature.instance_types.iter())
    {
        super::validate_type_generics(
            &mut cursor,
            types,
            *type_index,
            signature.generic_parameters.len(),
            &mut HashSet::new(),
        )?;
    }
    validate_variant(signature.variant.as_ref(), strings)?;

    write_string(&signature.name, strings, bytes)?;
    bytes.push(match signature.linkage {
        BinaryLinkage::Private => 0,
        BinaryLinkage::Module => 1,
        BinaryLinkage::Package => 2,
        BinaryLinkage::Exported => 3,
    });
    write_index(signature.function_type, bytes)?;
    write_count(signature.generic_parameters.len(), bytes)?;
    for parameter in &signature.generic_parameters {
        write_string(&parameter.name, strings, bytes)?;
        write_count(parameter.bounds.len(), bytes)?;
        for bound in &parameter.bounds {
            write_string(bound, strings, bytes)?;
        }
    }
    write_index(signature.modifiers, bytes)?;
    bytes.push(
        u8::from(signature.structured)
            | (u8::from(signature.kernel_fp) << 1)
            | (u8::from(signature.returns_twice) << 2)
            | (u8::from(signature.inlinable) << 3),
    );
    bytes.push(match signature.engine {
        Some(BinaryEngine::Native) => 0,
        Some(BinaryEngine::Runtime) => 1,
        Some(BinaryEngine::Any) => 2,
        None => 3,
    });
    bytes.push(match signature.semantics {
        None => 0,
        Some(SemanticsMode::Defined) => 1,
        Some(SemanticsMode::Permissive) => 2,
    });
    bytes.push(match signature.target {
        BinaryTarget::Cpu => 0,
        BinaryTarget::Gpu => 1,
        BinaryTarget::Both => 2,
    });
    bytes.push(match signature.fp_mode {
        None => 0,
        Some(BinaryFpMode::Precise) => 1,
        Some(BinaryFpMode::Fast) => 2,
        Some(BinaryFpMode::IeeeDenormals) => 3,
    });
    bytes.push(match signature.generic_strategy {
        None => 0,
        Some(BinaryGenericStrategy::Specialize) => 1,
        Some(BinaryGenericStrategy::Polymorphic) => 2,
        Some(BinaryGenericStrategy::Both) => 3,
    });
    if let Some((yield_type, resume_type)) = signature.coroutine {
        bytes.push(1);
        write_index(yield_type, bytes)?;
        write_index(resume_type, bytes)?;
    } else {
        bytes.push(0);
    }
    write_count(signature.instance_types.len(), bytes)?;
    for type_index in &signature.instance_types {
        write_index(*type_index, bytes)?;
    }
    if let Some(variant) = &signature.variant {
        bytes.push(1);
        bytes.push(variant.architecture);
        write_count(variant.features.len(), bytes)?;
        for feature in &variant.features {
            write_string(feature, strings, bytes)?;
        }
    } else {
        bytes.push(0);
    }
    bytes.push(entry_kind_code(signature.entry_kind));
    write_optional_string(signature.platform_export.as_deref(), strings, bytes)?;
    Ok(())
}

fn validate_variant(
    variant: Option<&crate::types::BinaryVariantAttribute>,
    strings: &StringTable,
) -> Result<(), FormatError> {
    let Some(variant) = variant else {
        return Ok(());
    };
    if !(1..=4).contains(&variant.architecture) {
        return Err(FormatError::malformed(None, "invalid variant architecture"));
    }
    if variant.features.iter().any(String::is_empty)
        || variant.features.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(FormatError::malformed(
            None,
            "variant feature names must be nonempty and strictly ordered",
        ));
    }
    if variant
        .features
        .iter()
        .any(|feature| strings.index_of(feature).is_none())
    {
        return Err(FormatError::malformed(
            None,
            "variant feature is absent from STRINGS",
        ));
    }
    Ok(())
}

fn entry_kind_code(kind: Option<BinaryEntryKind>) -> u8 {
    match kind {
        None => 0,
        Some(BinaryEntryKind::Compute) => 1,
        Some(BinaryEntryKind::Object) => 2,
        Some(BinaryEntryKind::Mesh) => 3,
        Some(BinaryEntryKind::Vertex) => 4,
        Some(BinaryEntryKind::Fragment) => 5,
        Some(BinaryEntryKind::Tile) => 6,
        Some(BinaryEntryKind::Raygen) => 7,
        Some(BinaryEntryKind::Intersection) => 8,
        Some(BinaryEntryKind::Anyhit) => 9,
        Some(BinaryEntryKind::Closesthit) => 10,
        Some(BinaryEntryKind::Miss) => 11,
        Some(BinaryEntryKind::Callable) => 12,
        Some(BinaryEntryKind::Node) => 13,
    }
}

fn write_optional_string(
    value: Option<&str>,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    match value {
        None => write_index(0, bytes),
        Some(value) if !value.is_empty() => write_string(value, strings, bytes),
        Some(_) => Err(FormatError::malformed(
            None,
            "platform export name is empty",
        )),
    }
}

fn write_string(
    value: &str,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| FormatError::malformed(None, "SIGNATURES string is absent from STRINGS"))?;
    let index = u64::try_from(index)
        .map_err(|_| FormatError::malformed(None, "SIGNATURES string index overflows"))?;
    if index == 0 {
        return Err(FormatError::malformed(
            None,
            "SIGNATURES string must be nonempty",
        ));
    }
    write_index(index, bytes)
}

fn write_count(count: usize, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .map_err(|_| FormatError::malformed(None, "SIGNATURES count overflows"))?;
    if count >= 1 << 32 {
        return Err(FormatError::malformed(
            None,
            "SIGNATURES count must be less than 2^32",
        ));
    }
    write_index(count, bytes)
}

fn write_index(index: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes).map_err(|error| {
        FormatError::malformed(None, format!("SIGNATURES encoding failed: {error}"))
    })
}
