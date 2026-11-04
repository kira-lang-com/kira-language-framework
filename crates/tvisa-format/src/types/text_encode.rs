use tir_types::{
    AddressSpace, CooperativeMatrixUse, FloatType, FunctionType, MatrixScope, TextureAccess,
    TextureDimension, Type,
};

use crate::binary::FormatError;
use crate::signature::{
    CAggregateDescriptor, ParameterItem, ResultItem, SignatureGroup, TextCallingConvention,
    TextSignature,
};

use super::{
    CAggregateMemberSummary, CAggregateSummary, GroupMemberSummary, GroupSummary,
    ParameterItemSummary, ResultItemSummary, SignatureSummary, TvisaValueTypeRecord, TypeSection,
};

/// Build TYPES from values in first-reference order and parsed text signatures.
/// `types_in_reference_order` includes signature types and types referenced by
/// other sections; nested value types are inserted before their parents. The
/// caller supplies signature indexes for `vft` types.
pub fn encode_text_type_section(
    types_in_reference_order: &[Type],
    signatures: &[TextSignature],
    mut resolve_signature: impl FnMut(&FunctionType) -> Option<u64>,
) -> Result<TypeSection, FormatError> {
    let mut interner = ValueTypeInterner::default();
    for ty in types_in_reference_order {
        ty.validate_tvisa()
            .map_err(|error| malformed(format!("invalid TVISA type: {error}")))?;
        interner.intern(ty, &mut resolve_signature)?;
    }

    let (signature_records, c_aggregates) = encode_text_signature_records(signatures, |ty| {
        interner
            .types
            .iter()
            .find_map(|(existing, index)| (existing == ty).then_some(*index))
    })?;
    let section = TypeSection {
        value_type_count: count(interner.records.len(), "value type count")?,
        value_types: interner.records,
        signatures: signature_records,
        c_aggregates,
    };
    section.encode()?;
    Ok(section)
}

#[derive(Default)]
struct ValueTypeInterner {
    types: Vec<(Type, u64)>,
    records: Vec<TvisaValueTypeRecord>,
}

impl ValueTypeInterner {
    fn intern(
        &mut self,
        ty: &Type,
        resolve_signature: &mut impl FnMut(&FunctionType) -> Option<u64>,
    ) -> Result<u64, FormatError> {
        if let Some((_, index)) = self.types.iter().find(|(existing, _)| existing == ty) {
            return Ok(*index);
        }
        let record = match ty {
            Type::Integer(bits) => TvisaValueTypeRecord::Integer {
                bits: u64::from(*bits),
            },
            Type::Float(kind) => TvisaValueTypeRecord::Float {
                tag: float_tag(*kind),
            },
            Type::Pointer(space) => TvisaValueTypeRecord::Pointer {
                address_space: address_space_code(*space),
            },
            Type::Code => TvisaValueTypeRecord::Code,
            Type::FixedVector { lanes, element } => TvisaValueTypeRecord::FixedVector {
                lanes: u64::from(*lanes),
                element_type: self.intern(element, resolve_signature)?,
            },
            Type::ScalableVector(element) => TvisaValueTypeRecord::ScalableVector {
                element_type: self.intern(element, resolve_signature)?,
            },
            Type::Texture {
                dimension,
                component,
                access,
                multisampled,
            } => TvisaValueTypeRecord::Texture {
                dimension: texture_dimension_code(*dimension),
                component_type: self.intern(component, resolve_signature)?,
                access: texture_access_code(*access),
                multisampled: *multisampled,
            },
            Type::Sampler => TvisaValueTypeRecord::Sampler,
            Type::AccelerationStructure => TvisaValueTypeRecord::AccelerationStructure,
            Type::VisibleFunctionTable(signature) => {
                let signature = resolve_signature(signature).ok_or_else(|| {
                    malformed("visible function table signature is absent from TYPES part B")
                })?;
                TvisaValueTypeRecord::VisibleFunctionTable { signature }
            }
            Type::Tensor {
                count: rank,
                element,
            } => TvisaValueTypeRecord::Tensor {
                rank: u64::from(*rank),
                element_type: self.intern(element, resolve_signature)?,
            },
            Type::RayQuery => TvisaValueTypeRecord::RayQuery,
            Type::HitObject => TvisaValueTypeRecord::HitObject,
            Type::CooperativeMatrix {
                scope,
                rows,
                columns,
                element,
                use_kind,
            } => TvisaValueTypeRecord::CooperativeMatrix {
                scope: matrix_scope_code(*scope),
                rows: u64::from(*rows),
                columns: u64::from(*columns),
                element_type: self.intern(element, resolve_signature)?,
                use_kind: matrix_use_code(*use_kind),
            },
            _ => return Err(malformed("type is not permitted as a TVISA value type")),
        };
        let index =
            if let Some(index) = self.records.iter().position(|existing| *existing == record) {
                count(index, "value type index")?
            } else {
                let index = count(self.records.len(), "value type index")?;
                self.records.push(record);
                index
            };
        self.types.push((ty.clone(), index));
        Ok(index)
    }
}

const fn float_tag(kind: FloatType) -> u8 {
    match kind {
        FloatType::F16 => 0x02,
        FloatType::Bf16 => 0x03,
        FloatType::F32 => 0x04,
        FloatType::F64 => 0x05,
        FloatType::F128 => 0x06,
        FloatType::F8E4M3 => 0x07,
        FloatType::F8E5M2 => 0x08,
        FloatType::F6E2M3 => 0x09,
        FloatType::F6E3M2 => 0x0a,
        FloatType::F4E2M1 => 0x0b,
        FloatType::E8M0 => 0x0c,
    }
}

const fn address_space_code(space: AddressSpace) -> u8 {
    match space {
        AddressSpace::Generic => 0,
        AddressSpace::Device => 1,
        AddressSpace::Constant => 2,
        AddressSpace::Threadgroup => 3,
        AddressSpace::Thread => 4,
        AddressSpace::ObjectPayload => 5,
        AddressSpace::RayPayload => 6,
        AddressSpace::HitAttribute => 7,
        AddressSpace::Tile => 8,
        AddressSpace::NodeRecord => 9,
    }
}

const fn texture_dimension_code(dimension: TextureDimension) -> u8 {
    match dimension {
        TextureDimension::D1 => 0,
        TextureDimension::D2 => 1,
        TextureDimension::D3 => 2,
        TextureDimension::Cube => 3,
        TextureDimension::D1Array => 4,
        TextureDimension::D2Array => 5,
        TextureDimension::CubeArray => 6,
    }
}

const fn texture_access_code(access: TextureAccess) -> u8 {
    match access {
        TextureAccess::Sample => 0,
        TextureAccess::Read => 1,
        TextureAccess::Write => 2,
        TextureAccess::ReadWrite => 3,
    }
}

const fn matrix_scope_code(scope: MatrixScope) -> u8 {
    match scope {
        MatrixScope::Subgroup => 0,
        MatrixScope::Threadgroup => 1,
    }
}

const fn matrix_use_code(use_kind: CooperativeMatrixUse) -> u8 {
    match use_kind {
        CooperativeMatrixUse::A => 0,
        CooperativeMatrixUse::B => 1,
        CooperativeMatrixUse::Acc => 2,
    }
}

/// Lower parsed TVISA signature declarations to TYPES signature and C aggregate
/// summaries. The caller resolves value types against the TYPES part-A table.
pub fn encode_text_signature_records(
    signatures: &[TextSignature],
    mut resolve_type: impl FnMut(&Type) -> Option<u64>,
) -> Result<(Vec<SignatureSummary>, Vec<CAggregateSummary>), FormatError> {
    let mut aggregates = Vec::<CAggregateSummary>::new();
    let mut records = Vec::new();
    records
        .try_reserve_exact(signatures.len())
        .map_err(|_| malformed("TYPES signature allocation failed"))?;

    for signature in signatures {
        let parameters = signature
            .parameters
            .iter()
            .map(|item| encode_parameter(item, &mut resolve_type, &mut aggregates))
            .collect::<Result<Vec<_>, _>>()?;
        let results = signature
            .results
            .iter()
            .map(|item| encode_result(item, &mut resolve_type))
            .collect::<Result<Vec<_>, _>>()?;
        let mut flags = u64::from(signature.fails);
        if signature.variadic.is_some() {
            flags |= 1 << 1;
        }
        if signature.noreturn {
            flags |= 1 << 2;
        }
        records.push(SignatureSummary {
            convention: convention_code(signature.convention),
            flags,
            fixed_parameter_count: signature.variadic.map(u64::from),
            parameter_count: count(parameters.len(), "signature parameter count")?,
            parameters,
            result_count: count(results.len(), "signature result count")?,
            results,
        });
    }

    Ok((records, aggregates))
}

fn encode_parameter(
    item: &ParameterItem,
    resolve_type: &mut impl FnMut(&Type) -> Option<u64>,
    aggregates: &mut Vec<CAggregateSummary>,
) -> Result<ParameterItemSummary, FormatError> {
    Ok(match item {
        ParameterItem::Leaf(ty) => ParameterItemSummary::Leaf {
            type_index: type_index(ty, resolve_type)?,
        },
        ParameterItem::Group(group) => {
            ParameterItemSummary::Group(encode_group(group, resolve_type)?)
        }
        ParameterItem::ByRef { size, align, flags } => ParameterItemSummary::ByRef {
            size: *size,
            alignment: *align,
            flags: encode_flags(flags, &["owned", "readonly"], "byref")?,
        },
        ParameterItem::Sret { size, align } => ParameterItemSummary::SRet {
            size: *size,
            alignment: *align,
        },
        ParameterItem::Context => ParameterItemSummary::Context,
        ParameterItem::CAggregate(descriptor) => {
            let record = encode_aggregate(descriptor, resolve_type)?;
            let index =
                if let Some(index) = aggregates.iter().position(|existing| *existing == record) {
                    count(index, "C aggregate index")?
                } else {
                    let index = count(aggregates.len(), "C aggregate index")?;
                    aggregates.push(record);
                    index
                };
            ParameterItemSummary::CAggregate { index }
        }
    })
}

fn encode_result(
    item: &ResultItem,
    resolve_type: &mut impl FnMut(&Type) -> Option<u64>,
) -> Result<ResultItemSummary, FormatError> {
    Ok(match item {
        ResultItem::Leaf(ty) => ResultItemSummary::Leaf {
            type_index: type_index(ty, resolve_type)?,
        },
        ResultItem::Group(group) => ResultItemSummary::Group(encode_group(group, resolve_type)?),
    })
}

fn encode_group(
    group: &SignatureGroup,
    resolve_type: &mut impl FnMut(&Type) -> Option<u64>,
) -> Result<GroupSummary, FormatError> {
    let members = group
        .members
        .iter()
        .map(|member| {
            Ok(GroupMemberSummary {
                offset: member.offset,
                type_index: type_index(&member.ty, resolve_type)?,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(GroupSummary {
        size: group.size,
        alignment: group.align,
        members,
    })
}

fn encode_aggregate(
    aggregate: &CAggregateDescriptor,
    resolve_type: &mut impl FnMut(&Type) -> Option<u64>,
) -> Result<CAggregateSummary, FormatError> {
    let flags = encode_flags(
        &aggregate.flags,
        &["union", "unaligned", "nontrivial", "flexible", "empty"],
        "C aggregate",
    )?;
    let members = aggregate
        .members
        .iter()
        .map(|member| {
            Ok(CAggregateMemberSummary {
                offset: member.offset,
                type_index: type_index(&member.ty, resolve_type)?,
                bit_width: member.bit_width.map_or(0, u64::from),
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(CAggregateSummary {
        size: aggregate.size,
        alignment: aggregate.align,
        flags,
        member_count: count(members.len(), "C aggregate member count")?,
        members,
    })
}

fn type_index(
    ty: &Type,
    resolve_type: &mut impl FnMut(&Type) -> Option<u64>,
) -> Result<u64, FormatError> {
    resolve_type(ty).ok_or_else(|| malformed("signature type is absent from TYPES part A"))
}

fn encode_flags(flags: &[String], names: &[&str], label: &str) -> Result<u8, FormatError> {
    let mut encoded = 0u8;
    for flag in flags {
        let Some(index) = names.iter().position(|name| *name == flag) else {
            return Err(malformed(format!("unknown {label} flag `{flag}`")));
        };
        let bit = 1u8 << index;
        if encoded & bit != 0 {
            return Err(malformed(format!("duplicate {label} flag `{flag}`")));
        }
        encoded |= bit;
    }
    Ok(encoded)
}

const fn convention_code(convention: TextCallingConvention) -> u8 {
    match convention {
        TextCallingConvention::Ridl => 0,
        TextCallingConvention::C => 1,
        TextCallingConvention::Rtcc => 2,
        TextCallingConvention::Gpu => 3,
        TextCallingConvention::Entry => 4,
    }
}

fn count(value: usize, label: &str) -> Result<u64, FormatError> {
    u64::try_from(value).map_err(|_| malformed(format!("{label} overflows")))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
