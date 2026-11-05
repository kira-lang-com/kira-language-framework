use std::collections::HashSet;

use crate::leb::write_uleb;
use crate::types::{
    BinaryEngine, BinaryEntryKind, BinaryFpMode, BinaryGenericStrategy, BinaryLinkage,
    BinarySignature, BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, SemanticsMode, StringTable, StringTableBuilder};

const MODIFIER_MASK: u64 = 0b1_1111 << 41;

impl BinarySignatureTable {
    /// Adds SIGNATURES strings to STRINGS in record order.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        for record in &self.records {
            intern_nonempty(strings, &record.name, "function name")?;
            for parameter in &record.generic_parameters {
                intern_nonempty(strings, &parameter.name, "generic parameter")?;
                for bound in &parameter.bounds {
                    intern_nonempty(strings, bound, "generic bound")?;
                }
            }
            if let Some(variant) = &record.variant {
                for feature in &variant.features {
                    intern_nonempty(strings, feature, "variant feature")?;
                }
            }
            if let Some(name) = &record.platform_export {
                intern_nonempty(strings, name, "platform export")?;
            }
        }
        Ok(())
    }

    /// Encodes SIGNATURES and validates each record's table-local invariants.
    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
    ) -> Result<Vec<u8>, FormatError> {
        if self.records.len() as u64 >= 1 << 32 {
            return Err(malformed("SIGNATURES count must be less than 2^32"));
        }
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.records.len())?;
        let mut names = HashSet::new();
        for record in &self.records {
            if record.name.is_empty() || !names.insert(&record.name) {
                return Err(malformed("function name is empty or duplicated"));
            }
            if record.modifiers & !MODIFIER_MASK != 0 {
                return Err(malformed(
                    "SIGNATURES has unsupported function modifier bits",
                ));
            }
            if record.modifiers & (1 << 44) != 0 && record.modifiers & (1 << 45) != 0 {
                return Err(malformed("function is both inline and noinline"));
            }
            let function = match types
                .entries
                .get(index(record.function_type, "function type index")?)
            {
                Some(BinaryType::Function(function)) => function,
                _ => return Err(malformed("signature type is not a function type")),
            };
            validate_callable(record, function)?;
            validate_generic_parameters(&record.generic_parameters)?;

            write_string_index(&mut bytes, strings, &record.name)?;
            bytes.push(linkage_code(record.linkage));
            write_uleb(record.function_type, &mut bytes).map_err(io_error)?;
            write_count(&mut bytes, record.generic_parameters.len())?;
            for parameter in &record.generic_parameters {
                write_string_index(&mut bytes, strings, &parameter.name)?;
                write_count(&mut bytes, parameter.bounds.len())?;
                for bound in &parameter.bounds {
                    write_string_index(&mut bytes, strings, bound)?;
                }
            }
            write_uleb(record.modifiers, &mut bytes).map_err(io_error)?;
            bytes.push(
                u8::from(record.structured)
                    | (u8::from(record.kernel_fp) << 1)
                    | (u8::from(record.returns_twice) << 2)
                    | (u8::from(record.inlinable) << 3),
            );
            bytes.push(engine_code(record.engine));
            bytes.push(semantics_code(record.semantics));
            bytes.push(target_code(record.target));
            bytes.push(fp_code(record.fp_mode));
            bytes.push(strategy_code(record.generic_strategy));
            if let Some((yield_type, resume_type)) = record.coroutine {
                bytes.push(1);
                write_uleb(yield_type, &mut bytes).map_err(io_error)?;
                write_uleb(resume_type, &mut bytes).map_err(io_error)?;
            } else {
                bytes.push(0);
            }
            write_count(&mut bytes, record.instance_types.len())?;
            for ty in &record.instance_types {
                write_uleb(*ty, &mut bytes).map_err(io_error)?;
            }
            if let Some(variant) = &record.variant {
                if !(1..=4).contains(&variant.architecture)
                    || variant.features.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return Err(malformed("invalid variant architecture or feature order"));
                }
                bytes.push(1);
                bytes.push(variant.architecture);
                write_count(&mut bytes, variant.features.len())?;
                for feature in &variant.features {
                    write_string_index(&mut bytes, strings, feature)?;
                }
            } else {
                bytes.push(0);
            }
            bytes.push(entry_code(record.entry_kind));
            if let Some(export) = &record.platform_export {
                write_string_index(&mut bytes, strings, export)?;
            } else {
                write_string_index(&mut bytes, strings, "")?;
            }
        }
        Ok(bytes)
    }
}

fn validate_callable(
    record: &BinarySignature,
    function: &crate::types::BinaryFunctionType,
) -> Result<(), FormatError> {
    if (record.target == BinaryTarget::Gpu) != record.engine.is_none() {
        return Err(malformed(
            "engine presence does not match the function target",
        ));
    }
    if record.entry_kind.is_some() && record.target != BinaryTarget::Gpu {
        return Err(malformed("entry target must be GPU"));
    }
    if record.variant.is_some() && record.target == BinaryTarget::Gpu {
        return Err(malformed(
            "GPU-only function cannot have a CPU architecture variant",
        ));
    }
    if record.platform_export.is_some() && !function.c_abi {
        return Err(malformed("platform export requires a C-ABI function type"));
    }
    if record
        .platform_export
        .as_ref()
        .is_some_and(|name| name.is_empty())
    {
        return Err(malformed("platform export name is empty"));
    }
    if function.foreign && record.platform_export.is_some() {
        return Err(malformed(
            "foreign function type cannot be a platform export definition",
        ));
    }
    Ok(())
}

fn validate_generic_parameters(
    parameters: &[crate::types::BinaryGenericParameter],
) -> Result<(), FormatError> {
    let mut names = HashSet::new();
    for parameter in parameters {
        if parameter.name.is_empty() || !names.insert(&parameter.name) {
            return Err(malformed("generic parameter name is empty or duplicated"));
        }
        let mut bounds = HashSet::new();
        for bound in &parameter.bounds {
            if bound.is_empty() || !bounds.insert(bound) {
                return Err(malformed("generic bound is empty or duplicated"));
            }
        }
    }
    Ok(())
}

fn intern_nonempty(
    strings: &mut StringTableBuilder,
    value: &str,
    label: &str,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(format!("{label} is empty")));
    }
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| malformed(error.message))
}

fn write_string_index(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed("SIGNATURES string is missing from STRINGS"))?;
    write_uleb(index, bytes).map_err(io_error)
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    write_uleb(
        u64::try_from(count).map_err(|_| malformed("SIGNATURES count overflows ULEB128"))?,
        bytes,
    )
    .map_err(io_error)
}

fn index(value: u64, label: &str) -> Result<usize, FormatError> {
    usize::try_from(value).map_err(|_| malformed(format!("{label} overflows memory")))
}

fn linkage_code(linkage: BinaryLinkage) -> u8 {
    match linkage {
        BinaryLinkage::Private => 0,
        BinaryLinkage::Module => 1,
        BinaryLinkage::Package => 2,
        BinaryLinkage::Exported => 3,
    }
}

fn engine_code(engine: Option<BinaryEngine>) -> u8 {
    match engine {
        Some(BinaryEngine::Native) => 0,
        Some(BinaryEngine::Runtime) => 1,
        Some(BinaryEngine::Any) => 2,
        None => 3,
    }
}

fn semantics_code(semantics: Option<SemanticsMode>) -> u8 {
    match semantics {
        None => 0,
        Some(SemanticsMode::Defined) => 1,
        Some(SemanticsMode::Permissive) => 2,
    }
}

fn target_code(target: BinaryTarget) -> u8 {
    match target {
        BinaryTarget::Cpu => 0,
        BinaryTarget::Gpu => 1,
        BinaryTarget::Both => 2,
    }
}

fn fp_code(mode: Option<BinaryFpMode>) -> u8 {
    match mode {
        None => 0,
        Some(BinaryFpMode::Precise) => 1,
        Some(BinaryFpMode::Fast) => 2,
        Some(BinaryFpMode::IeeeDenormals) => 3,
    }
}

fn strategy_code(strategy: Option<BinaryGenericStrategy>) -> u8 {
    match strategy {
        None => 0,
        Some(BinaryGenericStrategy::Specialize) => 1,
        Some(BinaryGenericStrategy::Polymorphic) => 2,
        Some(BinaryGenericStrategy::Both) => 3,
    }
}

fn entry_code(kind: Option<BinaryEntryKind>) -> u8 {
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

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("SIGNATURES encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
