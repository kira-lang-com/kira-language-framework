//! Printing for the module-level items of section 10.1.

use super::spelling::{
    Symbol, copy_policy, destroy_policy, exclusivity, layout_rule, linkage, pairing,
    relocate_policy, type_kind,
};
use crate::{
    Conformance, ExportRecord, GlobalDeclaration, GlobalInit, ImportRecord, ModuleAttribute,
    TraitDeclaration, TypeDeclaration, TypeKind, Vtable, WitnessDeclaration,
};
use std::fmt;
use std::fmt::Write as _;

pub(crate) fn write_attribute(
    formatter: &mut fmt::Formatter<'_>,
    attribute: &ModuleAttribute,
) -> fmt::Result {
    write!(formatter, "attribute {}", attribute.name)?;
    if let Some(value) = &attribute.value {
        write!(formatter, " = {value:?}")?;
    }
    writeln!(formatter)
}

pub(crate) fn write_type(
    formatter: &mut fmt::Formatter<'_>,
    declaration: &TypeDeclaration,
) -> fmt::Result {
    write!(formatter, "type %{}", declaration.name)?;
    if !declaration.generics.is_empty() {
        formatter.write_str("<")?;
        for (index, generic) in declaration.generics.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "${generic}")?;
        }
        formatter.write_str(">")?;
    }
    write!(
        formatter,
        " = {}{} {} {{ ",
        if declaration.open { "open " } else { "" },
        type_kind(declaration.kind),
        layout_rule(declaration.layout)
    )?;
    if declaration.kind == TypeKind::Enum {
        write_variants(formatter, declaration)?;
    } else {
        write_fields(formatter, declaration)?;
    }
    formatter.write_str(" }")?;
    if let Some(size) = declaration.size {
        write!(formatter, " size {size}")?;
    }
    if let Some(align) = declaration.align {
        write!(formatter, " align {align}")?;
    }
    writeln!(formatter)?;
    writeln!(
        formatter,
        "policy {{ copy: {}, destroy: {}, relocate: {}, exclusivity: {}, pairing: {} }}",
        copy_policy(&declaration.policy.copy),
        destroy_policy(&declaration.policy.destroy),
        relocate_policy(&declaration.policy.relocate),
        exclusivity(declaration.policy.exclusivity),
        pairing(declaration.policy.pairing)
    )
}

fn write_fields(formatter: &mut fmt::Formatter<'_>, declaration: &TypeDeclaration) -> fmt::Result {
    for (index, field) in declaration.fields.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{}: {}", field.name, field.ty)?;
        if let Some(offset) = field.offset {
            write!(formatter, " @{offset}")?;
        }
    }
    Ok(())
}

fn write_variants(
    formatter: &mut fmt::Formatter<'_>,
    declaration: &TypeDeclaration,
) -> fmt::Result {
    for (index, variant) in declaration.variants.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{}", variant.name)?;
        if !variant.payload.is_empty() {
            formatter.write_str("(")?;
            for (payload_index, payload) in variant.payload.iter().enumerate() {
                if payload_index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{payload}")?;
            }
            formatter.write_str(")")?;
        }
    }
    Ok(())
}

pub(crate) fn write_global(
    formatter: &mut fmt::Formatter<'_>,
    global: &GlobalDeclaration,
) -> fmt::Result {
    let qualifier = if global.mutable { "" } else { "const " };
    let init = match &global.init {
        GlobalInit::Zero => "zero".to_owned(),
        GlobalInit::Int(value) => value.to_string(),
        GlobalInit::Bytes(bytes) => bytes_literal(bytes),
    };
    write!(
        formatter,
        "global {} {qualifier}@{} : {} = {init}",
        linkage(global.linkage),
        Symbol(global.name.as_str()),
        global.ty
    )?;
    if global.thread_local {
        formatter.write_str(" tls")?;
    }
    writeln!(formatter)
}

fn bytes_literal(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7E => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "\\{byte:02x}");
            }
        }
    }
    out.push('"');
    out
}

pub(crate) fn write_vtable(formatter: &mut fmt::Formatter<'_>, vtable: &Vtable) -> fmt::Result {
    write!(
        formatter,
        "vtable @{} : {}",
        Symbol(vtable.name.as_str()),
        vtable.ty
    )?;
    if let Some(trait_name) = &vtable.trait_name {
        write!(formatter, " for %{trait_name}")?;
    }
    formatter.write_str(" { ")?;
    for (index, entry) in vtable.entries.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "@{}", Symbol(entry.as_str()))?;
    }
    formatter.write_str(" }\n")
}

pub(crate) fn write_trait(
    formatter: &mut fmt::Formatter<'_>,
    declaration: &TraitDeclaration,
) -> fmt::Result {
    write!(formatter, "trait %{}", declaration.name)?;
    if !declaration.generics.is_empty() {
        formatter.write_str("<")?;
        for (index, generic) in declaration.generics.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "${generic}")?;
        }
        formatter.write_str(">")?;
    }
    formatter.write_str(" { associated: [")?;
    for (index, associated) in declaration.associated.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "${associated}")?;
    }
    formatter.write_str("], methods: [")?;
    for (index, method) in declaration.methods.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{} : {}", method.name, method.signature)?;
    }
    formatter.write_str("] }\n")
}

pub(crate) fn write_conformance(
    formatter: &mut fmt::Formatter<'_>,
    conformance: &Conformance,
) -> fmt::Result {
    write!(
        formatter,
        "conformance {} : %{}",
        conformance.ty, conformance.trait_name
    )?;
    if let Some(witness) = &conformance.witness {
        write!(formatter, " witness @{}", Symbol(witness.as_str()))?;
    }
    for (index, (parameter, bound)) in conformance.conditions.iter().enumerate() {
        formatter.write_str(if index == 0 { " where " } else { ", " })?;
        write!(formatter, "${parameter}: %{bound}")?;
    }
    writeln!(formatter)
}

pub(crate) fn write_import(
    formatter: &mut fmt::Formatter<'_>,
    record: &ImportRecord,
) -> fmt::Result {
    writeln!(
        formatter,
        "import @{} : {} from {:?}",
        Symbol(record.symbol.as_str()),
        record.ty,
        record.module
    )
}

pub(crate) fn write_export(
    formatter: &mut fmt::Formatter<'_>,
    record: &ExportRecord,
) -> fmt::Result {
    writeln!(
        formatter,
        "export @{} as {:?} : {}",
        Symbol(record.symbol.as_str()),
        record.ridl_name,
        record.ty
    )
}

pub(crate) fn write_witness(
    formatter: &mut fmt::Formatter<'_>,
    witness: &WitnessDeclaration,
) -> fmt::Result {
    write!(
        formatter,
        "witness {} : %{} {{ witnesses: [",
        witness.ty, witness.trait_name
    )?;
    for (index, function) in witness.witnesses.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "@{}", Symbol(function.as_str()))?;
    }
    formatter.write_str("], associated: [")?;
    for (index, ty) in witness.associated.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{ty}")?;
    }
    formatter.write_str("] }\n")
}
