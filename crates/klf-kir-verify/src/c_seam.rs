//! A function's C prototype (section 26.2) against its KIR signature.

use klf_diag::Diagnostic;
use klf_kir::{CType, Function, LayoutRule, Module, Type};

const CODE: &str = "KLF-KIR-1014";

/// Each C type matches the KIR parameter or result that carries it: a scalar
/// its own width, an aggregate by value the address of a C-layout record, and
/// an aggregate result an address taken first with a `unit` result.
pub(crate) fn verify_c_prototype(
    function: &Function,
    module: &Module,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(prototype) = &function.c_prototype else {
        return;
    };
    let mut report = |message: String| {
        diagnostics.push(Diagnostic::error(CODE, message).with_label(
            function.span,
            "the C prototype must describe the KIR signature",
        ));
    };
    let out = usize::from(matches!(prototype.result, CType::Value(_)));
    if function.parameters.len() != prototype.parameters.len() + out {
        report(format!(
            "function `@{}` has {} parameters, its C prototype {}",
            function.name,
            function.parameters.len(),
            prototype.parameters.len() + out
        ));
        return;
    }
    if out == 1 {
        if !is_address(&function.parameters[0].ty) || function.result != Type::Unit {
            report(format!(
                "function `@{}` returns a C aggregate, so it takes the result's address first and returns `unit`",
                function.name
            ));
        }
        check_value(&prototype.result, module, &mut report);
    } else if !carries(&prototype.result, &function.result) {
        report(format!(
            "the result of `@{}` is `{}`, not its C type",
            function.name, function.result
        ));
    }
    for (parameter, ty) in function.parameters[out..].iter().zip(&prototype.parameters) {
        if matches!(ty, CType::Void) || !carries(ty, &parameter.ty) {
            report(format!(
                "parameter `%{}` of `@{}` is `{}`, not its C type",
                parameter.name, function.name, parameter.ty
            ));
        }
        check_value(ty, module, &mut report);
    }
}

fn carries(c: &CType, ty: &Type) -> bool {
    match c {
        CType::Void => *ty == Type::Unit,
        CType::Bool => matches!(ty, Type::Integer(1 | 8)),
        CType::Signed(bits) | CType::Unsigned(bits) => {
            matches!(bits, 8 | 16 | 32 | 64) && *ty == Type::Integer(*bits)
        }
        CType::Float(bits) => matches!(bits, 32 | 64) && *ty == Type::Float(*bits),
        CType::Pointer | CType::Value(_) => is_address(ty),
    }
}

fn is_address(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Pointer | Type::Address(_) | Type::Reference(_) | Type::Function(_)
    )
}

fn check_value(c: &CType, module: &Module, report: &mut impl FnMut(String)) {
    let CType::Value(name) = c else {
        return;
    };
    match module
        .types
        .iter()
        .find(|declaration| declaration.name == *name)
    {
        Some(declaration)
            if declaration.layout == LayoutRule::C && declaration.generics.is_empty() => {}
        Some(_) => report(format!(
            "`%{name}` crosses the C seam by value without C layout"
        )),
        None => report(format!("`%{name}` is not a declared type")),
    }
}
