//! Printing for function declarations and definitions (section 10.3).

use super::operation::write_operation;
use super::spelling::{engine, linkage};
use crate::{BlockTarget, CType, Function, FunctionAbi, Parameter, Type};
use std::fmt;

pub(crate) fn write_function(
    formatter: &mut fmt::Formatter<'_>,
    function: &Function,
) -> fmt::Result {
    let foreign = if function.foreign { "foreign " } else { "" };
    write!(
        formatter,
        "func {} {}@{}",
        linkage(function.linkage),
        foreign,
        crate::text::spelling::Symbol(function.name.as_str())
    )?;
    write_generics(formatter, function)?;
    formatter.write_str("(")?;
    write_parameters(formatter, &function.parameters)?;
    write!(formatter, ") -> {}", function.result)?;
    if let Some(failure) = &function.failure {
        write!(formatter, " fails {failure}")?;
    }
    write!(formatter, " engine({})", engine(function.engine))?;
    if function.abi == FunctionAbi::Ridl {
        formatter.write_str(" abi(ridl)")?;
    }
    if function.nounwind {
        formatter.write_str(" nounwind")?;
    }
    for attribute in &function.attributes {
        write!(formatter, " {}", attribute.spelling())?;
    }
    if let Some(prototype) = &function.c_prototype {
        formatter.write_str(" c(")?;
        if let Some(name) = &prototype.syscall {
            write!(formatter, "syscall {}: ", name.as_str())?;
        }
        for (index, parameter) in prototype.parameters.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            if prototype.variadic == Some(index) {
                formatter.write_str("..., ")?;
            }
            write_c_type(formatter, parameter)?;
        }
        if prototype.variadic == Some(prototype.parameters.len()) {
            if !prototype.parameters.is_empty() {
                formatter.write_str(", ")?;
            }
            formatter.write_str("...")?;
        }
        formatter.write_str(" -> ")?;
        write_c_type(formatter, &prototype.result)?;
        formatter.write_str(")")?;
    }
    if function.foreign {
        return writeln!(formatter);
    }
    formatter.write_str(" {\n")?;
    for block in &function.blocks {
        write_block(formatter, block)?;
    }
    writeln!(formatter, "}}")
}

fn write_c_type(formatter: &mut fmt::Formatter<'_>, ty: &CType) -> fmt::Result {
    match ty {
        CType::Void => formatter.write_str("void"),
        CType::Bool => formatter.write_str("bool"),
        CType::Signed(bits) => write!(formatter, "s{bits}"),
        CType::Unsigned(bits) => write!(formatter, "u{bits}"),
        CType::Float(bits) => write!(formatter, "f{bits}"),
        CType::Pointer => formatter.write_str("ptr"),
        CType::Value(name) => write!(formatter, "{}", Type::Nominal(name.clone(), Vec::new())),
    }
}

fn write_generics(formatter: &mut fmt::Formatter<'_>, function: &Function) -> fmt::Result {
    if function.generics.is_empty() {
        return Ok(());
    }
    formatter.write_str("<")?;
    for (index, generic) in function.generics.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "${}", generic.name)?;
        for (bound_index, bound) in generic.bounds.iter().enumerate() {
            formatter.write_str(if bound_index == 0 { ": " } else { " + " })?;
            write!(formatter, "%{bound}")?;
        }
    }
    formatter.write_str(">")
}

fn write_block(formatter: &mut fmt::Formatter<'_>, block: &crate::Block) -> fmt::Result {
    write!(formatter, "{}(", block.name)?;
    write_parameters(formatter, &block.parameters)?;
    writeln!(formatter, "):")?;
    for instruction in &block.instructions {
        formatter.write_str("  ")?;
        write_results(formatter, &instruction.results)?;
        write_operation(formatter, &instruction.operation)?;
        if let Some(location) = &instruction.debug_location {
            write!(
                formatter,
                " !loc({:?}:{}:{})",
                location.file, location.line, location.column
            )?;
        }
        writeln!(formatter)?;
    }
    Ok(())
}

fn write_results(formatter: &mut fmt::Formatter<'_>, results: &[klf_core::Name]) -> fmt::Result {
    match results {
        [] => Ok(()),
        [result] => write!(formatter, "%{result} = "),
        results => {
            formatter.write_str("(")?;
            for (index, result) in results.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "%{result}")?;
            }
            formatter.write_str(") = ")
        }
    }
}

pub(crate) fn write_parameters(
    formatter: &mut fmt::Formatter<'_>,
    parameters: &[Parameter],
) -> fmt::Result {
    for (index, parameter) in parameters.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "%{}: ", parameter.name)?;
        if parameter.context {
            formatter.write_str("context ")?;
        }
        if let Some(convention) = parameter.convention {
            write!(formatter, "{convention} ")?;
        }
        write!(formatter, "{}", parameter.ty)?;
    }
    Ok(())
}

pub(crate) fn write_target(
    formatter: &mut fmt::Formatter<'_>,
    target: &BlockTarget,
) -> fmt::Result {
    write!(formatter, "{}(", target.block)?;
    for (index, argument) in target.arguments.iter().enumerate() {
        if index > 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "%{argument}")?;
    }
    formatter.write_str(")")
}
