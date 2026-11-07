use tir_format::{BinaryNominalKind, BinaryType, BinaryTypeTable};

pub(super) fn weak_load_result(weak: Option<u64>, types: &BinaryTypeTable) -> Option<u64> {
    let BinaryType::Weak { pointee } = entry(types, weak?)? else {
        return None;
    };
    let reference = types.entries.iter().position(
        |ty| matches!(ty, BinaryType::Reference { pointee: actual } if actual == pointee),
    )?;
    let reference = u64::try_from(reference).ok()?;
    let optional = types
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, declaration)| {
            declaration.kind == BinaryNominalKind::Enum
                && declaration.generic_parameters.len() == 1
                && declaration.name.trim_start_matches('%').rsplit('.').next() == Some("Optional")
        })
        .map(|(index, _)| u64::try_from(index).ok())
        .collect::<Option<Vec<_>>>()?;
    if optional.len() != 1 {
        return None;
    }
    let declaration = optional[0];
    let lifecycle = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == declaration)?;
    if !matches!(
        lifecycle.policy.copy,
        tir_format::BinaryCopyPolicy::Implicit(_)
    ) || !matches!(
        lifecycle.policy.destroy,
        tir_format::BinaryDestroyPolicy::Function(_)
    ) || lifecycle.policy.pairing != 0
    {
        return None;
    }
    types
        .entries
        .iter()
        .position(|ty| {
            matches!(
                ty,
                BinaryType::Nominal { declaration: actual, arguments }
                    if *actual == declaration && arguments == &[reference]
            )
        })
        .and_then(|index| u64::try_from(index).ok())
}

fn entry(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}
