//! Print a `.klffi` module back to text (grammar A.3). The output reparses to
//! the same module.

use crate::model::{Annotation, FfiFunction, FfiItem, FfiModule, FfiStruct};
use std::fmt::Write;

#[must_use]
pub fn emit(module: &FfiModule) -> String {
    let mut out = String::new();
    for (index, item) in module.items.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match item {
            FfiItem::Function(function) => emit_function(&mut out, function),
            FfiItem::Struct(structure) => emit_struct(&mut out, structure),
            FfiItem::Constant(constant) => {
                let _ = writeln!(
                    out,
                    "let {}: {} = {}",
                    constant.name, constant.ty, constant.value
                );
            }
        }
    }
    out
}

fn emit_annotations(out: &mut String, annotations: &[Annotation]) {
    for annotation in annotations {
        let _ = write!(out, "@FFI.{}", annotation.kind.spelling());
        if !annotation.pairs.is_empty() {
            let pairs: Vec<String> = annotation
                .pairs
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect();
            let _ = write!(out, " {{ {} }}", pairs.join(", "));
        }
        out.push('\n');
    }
}

fn emit_function(out: &mut String, function: &FfiFunction) {
    emit_annotations(out, &function.annotations);
    let params: Vec<String> = function
        .parameters
        .iter()
        .map(|(name, ty)| format!("{name}: {ty}"))
        .collect();
    let _ = write!(out, "function {}({})", function.name, params.join(", "));
    if function.result != "Void" {
        let _ = write!(out, " -> {}", function.result);
    }
    out.push('\n');
}

fn emit_struct(out: &mut String, structure: &FfiStruct) {
    emit_annotations(out, &structure.annotations);
    if structure.fields.is_empty() {
        let _ = writeln!(out, "struct {}", structure.name);
        return;
    }
    let _ = writeln!(out, "struct {} {{", structure.name);
    for field in &structure.fields {
        let _ = write!(out, "    let {}: {}", field.name, field.ty);
        if let Some(bits) = field.bits {
            let _ = write!(out, " @Bits({bits})");
        }
        out.push('\n');
    }
    out.push_str("}\n");
}
