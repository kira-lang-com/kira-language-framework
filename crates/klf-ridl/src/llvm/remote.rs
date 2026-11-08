//! Scalar RIDL adapters across an isolated Kira runtime boundary.

use super::{Ir, path, quote, scalar_type};
use crate::module::{Convention, ExtendBody, FunctionDecl, ItemKind, Module};
use crate::types::{Scalar, Type};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Scalar-only adapters for a Kira application whose RIDL provider runs in
/// a separate process. The emitted symbol has the same C view as the normal
/// local adapter; only its callee changes to the runtime's RIDL wire bridge.
/// The `.ridl` source is embedded unchanged and interpreted by that bridge.
///
/// # Errors
/// Refuses pointer-bearing, borrowed, generic or aggregate signatures rather
/// than silently changing their ownership semantics across processes.
pub fn remote_adapters(
    module: &Module,
    callers: &BTreeMap<String, String>,
    target: &str,
) -> Result<String, String> {
    if target.contains("windows") || target.starts_with("wasm32") {
        return Err(format!("remote native RIDL is not supported on {target}"));
    }
    let registry = module.registry();
    let defined = |name: &str| name.to_owned();
    let destroys = BTreeMap::new();
    let mut ir = Ir::new(&registry, target, &module.name, &destroys, &defined, false);
    ir.declare(
        "klfrt_ridl_remote_scalar",
        "declare i32 @klfrt_ridl_remote_scalar(ptr, i64, ptr, i64, ptr, i64, ptr)",
    );
    ir.declare("llvm.trap", "declare void @llvm.trap()");
    let text = crate::emit::emit(module);
    if text.len() > 64 * 1024 {
        return Err("remote RIDL contract exceeds the 64 KiB wire limit".into());
    }
    let contract = ir.private_bytes("ridl.remote.contract", text.as_bytes());
    for (index, item) in module.items.iter().enumerate() {
        match &item.kind {
            ItemKind::Function(function) => {
                let path = path(None, function);
                if let Some(caller) = callers.get(&path) {
                    ir.remote_scalar_adapter(
                        caller,
                        &path,
                        &contract,
                        text.len(),
                        function,
                        index,
                    )?;
                }
            }
            ItemKind::Extend(extend) if matches!(extend.body, ExtendBody::Methods(_)) => {
                if let ExtendBody::Methods(methods) = &extend.body {
                    for function in methods {
                        if callers.contains_key(&path(Some(&extend.target), function)) {
                            return Err(format!(
                                "remote RIDL method `{}` requires an object proxy adapter",
                                function.name
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(ir.finish())
}

impl Ir<'_> {
    fn remote_scalar_adapter(
        &mut self,
        caller: &str,
        path: &str,
        contract: &str,
        contract_len: usize,
        function: &FunctionDecl,
        index: usize,
    ) -> Result<(), String> {
        if !function.generics.is_empty() || function.self_param.is_some() {
            return Err(format!(
                "remote RIDL `{path}` requires a non-scalar adapter"
            ));
        }
        let mut parameters = Vec::new();
        for param in &function.params {
            let Type::Scalar(scalar) = &param.ty else {
                return Err(format!(
                    "remote RIDL `{path}` requires a non-scalar adapter"
                ));
            };
            if param.convention != Convention::Owned || !remote_scalar(*scalar) {
                return Err(format!(
                    "remote RIDL `{path}` requires a non-scalar adapter"
                ));
            }
            parameters.push(*scalar);
        }
        let result = match function.result.as_ref() {
            None | Some(Type::Scalar(Scalar::Void)) => Scalar::Void,
            Some(Type::Scalar(scalar)) if remote_scalar(*scalar) => *scalar,
            _ => {
                return Err(format!(
                    "remote RIDL `{path}` requires a non-scalar result adapter"
                ));
            }
        };
        let path_bytes = format!("{}.{}", self.module_name, path);
        if path_bytes.len() > 1024 {
            return Err(format!(
                "remote RIDL function name `{path}` exceeds 1024 bytes"
            ));
        }
        let name = self.private_bytes(&format!("ridl.remote.path.{index}"), path_bytes.as_bytes());
        let ret = scalar_type(result, self.word);
        let args: Vec<_> = parameters
            .iter()
            .enumerate()
            .map(|(i, scalar)| format!("{} %a{i}", scalar_type(*scalar, self.word)))
            .collect();
        let _ = writeln!(
            self.functions,
            "define {ret} {}({}) {{",
            quote(caller),
            args.join(", ")
        );
        self.functions.push_str("entry:\n");
        if !parameters.is_empty() {
            let _ = writeln!(
                self.functions,
                "  %args = alloca [{} x i64], align 8",
                parameters.len()
            );
        }
        for (i, scalar) in parameters.iter().enumerate() {
            let raw = match scalar {
                Scalar::I8 | Scalar::I16 | Scalar::I32 => {
                    let _ = writeln!(
                        self.functions,
                        "  %w{i} = sext {} %a{i} to i64",
                        scalar_type(*scalar, self.word)
                    );
                    format!("%w{i}")
                }
                Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::Bool => {
                    let _ = writeln!(
                        self.functions,
                        "  %w{i} = zext {} %a{i} to i64",
                        scalar_type(*scalar, self.word)
                    );
                    format!("%w{i}")
                }
                Scalar::F32 => {
                    let _ = writeln!(self.functions, "  %b{i} = bitcast float %a{i} to i32");
                    let _ = writeln!(self.functions, "  %w{i} = zext i32 %b{i} to i64");
                    format!("%w{i}")
                }
                Scalar::F64 => {
                    let _ = writeln!(self.functions, "  %w{i} = bitcast double %a{i} to i64");
                    format!("%w{i}")
                }
                _ => format!("%a{i}"),
            };
            let _ = writeln!(
                self.functions,
                "  %p{i} = getelementptr [{} x i64], ptr %args, i32 0, i32 {i}",
                parameters.len()
            );
            let _ = writeln!(self.functions, "  store i64 {raw}, ptr %p{i}, align 8");
        }
        self.functions.push_str("  %out = alloca i64, align 8\n");
        let argv = if parameters.is_empty() {
            "null"
        } else {
            "%args"
        };
        let _ = writeln!(
            self.functions,
            "  %status = call i32 @klfrt_ridl_remote_scalar(ptr {name}, i64 {}, ptr {contract}, i64 {contract_len}, ptr {argv}, i64 {}, ptr %out)",
            path_bytes.len(),
            parameters.len()
        );
        self.functions.push_str("  %failed = icmp ne i32 %status, 0\n  br i1 %failed, label %trap, label %success\ntrap:\n  call void @llvm.trap()\n  unreachable\nsuccess:\n");
        if result == Scalar::Void {
            self.functions.push_str("  ret void\n}\n");
        } else {
            self.functions
                .push_str("  %word = load i64, ptr %out, align 8\n");
            match result {
                Scalar::I8
                | Scalar::U8
                | Scalar::Bool
                | Scalar::I16
                | Scalar::U16
                | Scalar::I32
                | Scalar::U32
                | Scalar::F32 => {
                    let ty = if result == Scalar::F32 {
                        "i32".to_owned()
                    } else {
                        scalar_type(result, self.word)
                    };
                    let _ = writeln!(self.functions, "  %narrow = trunc i64 %word to {ty}");
                    if result == Scalar::F32 {
                        self.functions.push_str(
                            "  %float = bitcast i32 %narrow to float\n  ret float %float\n}\n",
                        );
                    } else {
                        let _ = writeln!(self.functions, "  ret {ty} %narrow\n}}");
                    }
                }
                Scalar::F64 => self
                    .functions
                    .push_str("  %float = bitcast i64 %word to double\n  ret double %float\n}\n"),
                _ => self.functions.push_str("  ret i64 %word\n}\n"),
            }
        }
        Ok(())
    }
}

fn remote_scalar(scalar: Scalar) -> bool {
    matches!(
        scalar,
        Scalar::I8
            | Scalar::I16
            | Scalar::I32
            | Scalar::I64
            | Scalar::U8
            | Scalar::U16
            | Scalar::U32
            | Scalar::U64
            | Scalar::Bool
            | Scalar::F32
            | Scalar::F64
    )
}
