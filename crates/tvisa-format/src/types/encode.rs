use tir_format::leb::write_uleb;

use crate::binary::FormatError;

use super::{
    CAggregateSummary, GroupSummary, ParameterItemSummary, ResultItemSummary, SignatureSummary,
    TvisaValueTypeRecord, TypeSection,
};

impl TypeSection {
    /// Encode the TYPES section and reject summaries that are not valid TVISA records.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        if usize::try_from(self.value_type_count).ok() != Some(self.value_types.len()) {
            return Err(malformed(
                "TYPES value type count does not match its records",
            ));
        }

        let mut bytes = Vec::new();
        write_count(self.value_types.len(), &mut bytes)?;
        for value_type in &self.value_types {
            encode_value_type(value_type, &mut bytes)?;
        }

        write_count(self.signatures.len(), &mut bytes)?;
        for signature in &self.signatures {
            encode_signature(signature, &mut bytes)?;
        }

        write_count(self.c_aggregates.len(), &mut bytes)?;
        for aggregate in &self.c_aggregates {
            encode_c_aggregate(aggregate, &mut bytes)?;
        }

        let decoded = Self::decode(&bytes)?;
        if decoded != *self {
            return Err(malformed(
                "TYPES summaries are inconsistent with their binary encoding",
            ));
        }
        Ok(bytes)
    }
}

fn encode_value_type(
    value_type: &TvisaValueTypeRecord,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    match value_type {
        TvisaValueTypeRecord::Integer { bits } => {
            output.push(0x01);
            write_index(*bits, output)?;
        }
        TvisaValueTypeRecord::Float { tag } => output.push(*tag),
        TvisaValueTypeRecord::Pointer { address_space } => {
            output.extend([0x10, *address_space]);
        }
        TvisaValueTypeRecord::Code => output.push(0x14),
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => {
            output.push(0x20);
            write_index(*lanes, output)?;
            write_index(*element_type, output)?;
        }
        TvisaValueTypeRecord::ScalableVector { element_type } => {
            output.push(0x21);
            write_index(*element_type, output)?;
        }
        TvisaValueTypeRecord::Texture {
            dimension,
            component_type,
            access,
            multisampled,
        } => {
            output.extend([0x40, *dimension]);
            write_index(*component_type, output)?;
            output.extend([*access, u8::from(*multisampled)]);
        }
        TvisaValueTypeRecord::Sampler => output.push(0x41),
        TvisaValueTypeRecord::AccelerationStructure => output.push(0x42),
        TvisaValueTypeRecord::VisibleFunctionTable { signature } => {
            output.push(0x43);
            write_index(*signature, output)?;
        }
        TvisaValueTypeRecord::Tensor { rank, element_type } => {
            output.push(0x44);
            write_index(*rank, output)?;
            write_index(*element_type, output)?;
        }
        TvisaValueTypeRecord::RayQuery => output.push(0x45),
        TvisaValueTypeRecord::HitObject => output.push(0x46),
        TvisaValueTypeRecord::CooperativeMatrix {
            scope,
            rows,
            columns,
            element_type,
            use_kind,
        } => {
            output.extend([0x47, *scope]);
            write_index(*rows, output)?;
            write_index(*columns, output)?;
            write_index(*element_type, output)?;
            output.push(*use_kind);
        }
    }
    Ok(())
}

fn encode_signature(signature: &SignatureSummary, output: &mut Vec<u8>) -> Result<(), FormatError> {
    if usize::try_from(signature.parameter_count).ok() != Some(signature.parameters.len())
        || usize::try_from(signature.result_count).ok() != Some(signature.results.len())
        || (signature.flags & 2 != 0) != signature.fixed_parameter_count.is_some()
    {
        return Err(malformed(
            "TYPES signature counts or variadic flag do not match its records",
        ));
    }

    output.push(signature.convention);
    write_index(signature.flags, output)?;
    if let Some(fixed_parameter_count) = signature.fixed_parameter_count {
        write_index(fixed_parameter_count, output)?;
    }
    write_count(signature.parameters.len(), output)?;
    for parameter in &signature.parameters {
        match parameter {
            ParameterItemSummary::Leaf { type_index } => {
                output.push(0);
                write_index(*type_index, output)?;
            }
            ParameterItemSummary::Group(group) => {
                output.push(1);
                encode_group(group, output)?;
            }
            ParameterItemSummary::ByRef {
                size,
                alignment,
                flags,
            } => {
                output.push(2);
                write_index(*size, output)?;
                write_index(*alignment, output)?;
                output.push(*flags);
            }
            ParameterItemSummary::SRet { size, alignment } => {
                output.push(3);
                write_index(*size, output)?;
                write_index(*alignment, output)?;
            }
            ParameterItemSummary::Context => output.push(4),
            ParameterItemSummary::CAggregate { index } => {
                output.push(5);
                write_index(*index, output)?;
            }
        }
    }
    write_count(signature.results.len(), output)?;
    for result in &signature.results {
        match result {
            ResultItemSummary::Leaf { type_index } => {
                output.push(0);
                write_index(*type_index, output)?;
            }
            ResultItemSummary::Group(group) => {
                output.push(1);
                encode_group(group, output)?;
            }
        }
    }
    Ok(())
}

fn encode_group(group: &GroupSummary, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_index(group.size, output)?;
    write_index(group.alignment, output)?;
    write_count(group.members.len(), output)?;
    for member in &group.members {
        write_index(member.offset, output)?;
        write_index(member.type_index, output)?;
    }
    Ok(())
}

fn encode_c_aggregate(
    aggregate: &CAggregateSummary,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if usize::try_from(aggregate.member_count).ok() != Some(aggregate.members.len()) {
        return Err(malformed(
            "TYPES C aggregate member count does not match its records",
        ));
    }
    write_index(aggregate.size, output)?;
    write_index(aggregate.alignment, output)?;
    output.push(aggregate.flags);
    write_count(aggregate.members.len(), output)?;
    for member in &aggregate.members {
        write_index(member.offset, output)?;
        write_index(member.type_index, output)?;
        write_index(member.bit_width, output)?;
    }
    Ok(())
}

fn write_count(count: usize, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count).map_err(|_| malformed("TYPES record count overflows"))?;
    write_index(count, output)
}

fn write_index(value: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output)
        .map_err(|error| malformed(format!("failed to encode TYPES ULEB: {error}")))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
