//! Shared printing helpers for the section-12 instruction set.

use crate::{DebugVariable, RawAccess, Type};
use klf_core::Name;
use std::fmt;

/// Writes `(%a, %b, ...)`.
pub(super) fn arguments(formatter: &mut fmt::Formatter<'_>, values: &[Name]) -> fmt::Result {
    formatter.write_str("(")?;
    values_list(formatter, values)?;
    formatter.write_str(")")
}

pub(super) fn values_list(formatter: &mut fmt::Formatter<'_>, values: &[Name]) -> fmt::Result {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "%{value}")?;
    }
    Ok(())
}

pub(super) fn type_list(formatter: &mut fmt::Formatter<'_>, types: &[Type]) -> fmt::Result {
    for (index, ty) in types.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{ty}")?;
    }
    Ok(())
}

pub(super) fn generic_arguments(formatter: &mut fmt::Formatter<'_>, types: &[Type]) -> fmt::Result {
    if types.is_empty() {
        return Ok(());
    }
    formatter.write_str("<")?;
    type_list(formatter, types)?;
    formatter.write_str(">")
}

pub(super) fn raw_access(formatter: &mut fmt::Formatter<'_>, access: RawAccess) -> fmt::Result {
    if let Some(align) = access.align {
        write!(formatter, " align {align}")?;
    }
    if access.volatile {
        formatter.write_str(" volatile")?;
    }
    Ok(())
}

pub(super) fn debug_variable(
    formatter: &mut fmt::Formatter<'_>,
    variable: &DebugVariable,
) -> fmt::Result {
    write!(
        formatter,
        "!var({:?}, {}, {:?})",
        variable.name, variable.ty, variable.scope
    )
}
