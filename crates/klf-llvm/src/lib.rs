//! `klf-llvm`: the native backend. It lowers verified KIR to textual LLVM IR
//! one function at a time (section 16). The IR is compiled by the LLVM the rest
//! of Kira ships (D-110); this crate itself only produces text and stays
//! `unsafe`-free.

mod debug;
mod emit;
mod lltype;

use debug::DebugInfo;
use emit::FnEmitter;
use klf_kir::{Module, Type};
use lltype::{llvm_result_type, llvm_type};
use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Write as _};

pub use debug::DebugSources;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmitError(pub String);

impl fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EmitError {}

const RUNTIME_DECLS: &str = "\
declare void @klfrt_trap(i32)
declare void @klfrt_trap_message(ptr, i64)
declare void @klfrt_frame_enter()
declare void @klfrt_frame_exit()
declare ptr @klfrt_stack_alloc(i64, i64)
declare void @klfrt_stack_dealloc(ptr)
declare ptr @klfrt_heap_alloc_counted(i64, i64)
declare ptr @klfrt_heap_alloc_unique(i64, i64)
declare ptr @klfrt_heap_alloc_untracked(i64, i64)
declare ptr @klfrt_global_alloc(i64, i64, i32, i64)
declare void @klfrt_retain(ptr)
declare void @klfrt_release(ptr)
declare i8 @klfrt_is_unique(ptr)
declare void @klfrt_free_raw(ptr)
declare void @klfrt_access_begin(ptr, i32, i64)
declare void @klfrt_access_end(ptr)
declare i64 @klfrt_load(ptr, i32)
declare void @klfrt_store(ptr, i32, i64)
declare void @klfrt_finish(i64, i32)
declare void @klfrt_fail(i64)
declare ptr @klfrt_error_box(i64)
declare i64 @klfrt_error_value()
";

/// Emit the whole module as LLVM IR text, with the debug information of
/// section 16.3.
///
/// # Errors
/// Returns [`EmitError`] for any operation, type or width the native backend
/// does not lower yet.
pub fn emit_module(module: &Module) -> Result<String, EmitError> {
    emit_module_with(module, &DebugSources::default())
}

/// [`emit_module`] with the text of the module's virtual files, which travels
/// in the DWARF source extension so a debugger can show generated text.
///
/// # Errors
/// Returns [`EmitError`] for anything the native backend does not lower yet.
pub fn emit_module_with(module: &Module, sources: &DebugSources) -> Result<String, EmitError> {
    let legal = klf_kir::legalize(module);
    let module = legal.as_ref().unwrap_or(module);
    let mut intrinsics = BTreeSet::new();
    let (strings, string_declarations) = collect_strings(module);
    let mut info = DebugInfo::new(module, sources);
    let mut body = String::new();
    let mut imported = BTreeSet::new();
    for function in &module.functions {
        if function.foreign {
            // Two imports of one C symbol, or one the runtime declares,
            // share a single declaration; each call states its own type.
            let symbol = klf_kir::c_symbol(function.name.as_str());
            if !imported.insert(symbol) || RUNTIME_DECLS.contains(&format!("@{symbol}(")) {
                continue;
            }
        }
        let mut emitter = FnEmitter::new(module, function, &strings, &mut intrinsics, &mut info);
        emitter.emit()?;
        body.push_str(&emitter.finish());
        body.push('\n');
    }
    for global in &module.globals {
        body.push_str(&emit_global(global, &mut info));
        body.push('\n');
    }
    let mut out = String::new();
    out.push_str(RUNTIME_DECLS);
    for declaration in &intrinsics {
        out.push_str(declaration);
        out.push('\n');
    }
    out.push('\n');
    for declaration in string_declarations {
        out.push_str(&declaration);
        out.push('\n');
    }
    if !strings.is_empty() {
        out.push('\n');
    }
    out.push_str(&body);
    out.push('\n');
    out.push_str(&info.render());
    Ok(out)
}

fn collect_strings(module: &Module) -> (HashMap<String, String>, Vec<String>) {
    let mut map = HashMap::new();
    let mut declarations = Vec::new();
    for value in module
        .functions
        .iter()
        .flat_map(|function| function.blocks.iter())
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| match &instruction.operation {
            klf_kir::Operation::ConstString(value) => Some(value),
            klf_kir::Operation::Trap(code) if emit::trap_number(code).is_none() => Some(code),
            _ => None,
        })
    {
        if map.contains_key(value) {
            continue;
        }
        let symbol = format!(".kir.str.{}", map.len());
        declarations.push(emit_string(&symbol, value));
        map.insert(value.clone(), symbol);
    }
    (map, declarations)
}

fn emit_string(symbol: &str, value: &str) -> String {
    let length = value.len().max(1);
    let bytes = if value.is_empty() {
        "\\00".to_owned()
    } else {
        value.bytes().fold(String::new(), |mut bytes, byte| {
            let _ = write!(bytes, "\\{byte:02X}");
            bytes
        })
    };
    format!("@\"{symbol}\" = private unnamed_addr constant [{length} x i8] c\"{bytes}\"")
}

fn emit_global(global: &klf_kir::GlobalDeclaration, info: &mut DebugInfo<'_>) -> String {
    let debug = info.global(global.name.as_str(), &global.ty);
    format!(
        "@\"kirg.{}\" = internal global ptr null, !dbg !{debug}",
        global.name.as_str()
    )
}

/// Emit the module plus a `main` that calls `entry` with `arguments`, then hands
/// the outcome to the runtime's `klfrt_finish` (which runs the heap-balance
/// check and prints the result in the VM's `Outcome` shape).
///
/// # Errors
/// Returns [`EmitError`] if `entry` is missing or anything fails to lower.
pub fn emit_program(module: &Module, entry: &str, arguments: &[u64]) -> Result<String, EmitError> {
    emit_program_with(module, entry, arguments, &DebugSources::default())
}

/// [`emit_program`] with the text of the module's virtual files, which travels
/// in the DWARF source extension (section 16.3).
///
/// # Errors
/// Returns [`EmitError`] if `entry` is missing or anything fails to lower.
pub fn emit_program_with(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    sources: &DebugSources,
) -> Result<String, EmitError> {
    let function = module
        .functions
        .iter()
        .find(|function| function.name.as_str() == entry)
        .ok_or_else(|| EmitError(format!("no function named `{entry}`")))?;

    let mut out = emit_module_with(module, sources)?;
    out.push_str("\ndefine i32 @main() {\nentry:\n");

    for (index, global) in module.globals.iter().enumerate() {
        let layout = klf_layout::layout_of(&global.ty, &module.types);
        let size = layout.size.max(1);
        let align = layout.align.max(1);
        let _ = writeln!(
            out,
            "  %gptr{index} = call ptr @klfrt_global_alloc(i64 {size}, i64 {align}, i32 0, i64 0)"
        );
        let bytes = global
            .init
            .init_bytes(usize::try_from(size).unwrap_or(usize::MAX));
        let mut offset = 0usize;
        while offset < bytes.len() {
            let width = (bytes.len() - offset).min(8);
            let mut value = 0u64;
            for (shift, byte) in bytes[offset..offset + width].iter().enumerate() {
                value |= u64::from(*byte) << (shift * 8);
            }
            let _ = writeln!(
                out,
                "  %gp{index}_{offset} = getelementptr inbounds i8, ptr %gptr{index}, i64 {offset}"
            );
            let _ = writeln!(
                out,
                "  call void @klfrt_store(ptr %gp{index}_{offset}, i32 {width}, i64 {value})"
            );
            offset += width;
        }
        let _ = writeln!(
            out,
            "  store ptr %gptr{index}, ptr @\"kirg.{}\"",
            global.name.as_str()
        );
    }

    for initializer in module.functions.iter().filter(|function| {
        !function.foreign
            && function
                .attributes
                .contains(&klf_kir::FunctionAttribute::Initializer)
    }) {
        let cc = emit::calling_convention(initializer);
        let _ = writeln!(
            out,
            "  call {cc}void @\"kir.{}\"()",
            initializer.name.as_str()
        );
    }

    let mut call_args = Vec::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        let value = arguments.get(index).copied().unwrap_or(0);
        let mut operand = argument_operand(&parameter.ty, value)?;
        if parameter.context {
            operand = operand.replacen("ptr ", "ptr swiftself ", 1);
        }
        call_args.push(operand);
    }
    let ret = llvm_result_type(&function.result)?;
    let cc = emit::calling_convention(function);
    let fails = function.failure.is_some();
    if fails {
        out.push_str("  %err.slot = alloca ptr\n  store ptr null, ptr %err.slot\n");
        call_args.push("ptr %err.slot".to_owned());
    }
    let call = format!("call {cc}{ret} @\"kir.{entry}\"({})", call_args.join(", "));

    let void = matches!(function.result, Type::Unit | Type::Never);
    if void {
        let _ = writeln!(out, "  {call}");
    } else {
        let _ = writeln!(out, "  %ret = {call}");
    }

    if fails {
        out.push_str("  %err.val = load ptr, ptr %err.slot\n");
        out.push_str("  %err.failed = icmp ne ptr %err.val, null\n");
        out.push_str("  br i1 %err.failed, label %entry.failed, label %entry.ok\n");
        out.push_str("entry.failed:\n");
        out.push_str("  %fail.value = call i64 @klfrt_error_value()\n");
        out.push_str("  call void @klfrt_fail(i64 %fail.value)\n");
        out.push_str("  unreachable\n");
        out.push_str("entry.ok:\n");
    }

    if void {
        out.push_str("  call void @klfrt_finish(i64 0, i32 0)\n");
    } else {
        let widened = widen_to_i64(&mut out, &function.result)?;
        let _ = writeln!(out, "  call void @klfrt_finish(i64 {widened}, i32 1)");
    }
    out.push_str("  unreachable\n}\n");
    Ok(out)
}

fn argument_operand(ty: &Type, value: u64) -> Result<String, EmitError> {
    match ty {
        Type::Integer(bits) => {
            let bits = u32::from(*bits);
            let masked = if bits >= 64 {
                value
            } else {
                value & ((1u64 << bits) - 1)
            };
            Ok(format!("i{bits} {masked}"))
        }
        Type::Float(64) => Ok(format!("double 0x{value:016X}")),
        Type::Float(32) => {
            let low = u32::try_from(value & 0xFFFF_FFFF).expect("masked to 32 bits");
            let promoted = f64::from(f32::from_bits(low)).to_bits();
            Ok(format!("float 0x{promoted:016X}"))
        }
        Type::Pointer | Type::Reference(_) | Type::Unique(_) | Type::Weak(_) | Type::Address(_) => {
            Ok(format!("ptr inttoptr (i64 {value} to ptr)"))
        }
        other => Err(EmitError(format!(
            "entry argument of type `{other}` is not supported yet"
        ))),
    }
}

fn widen_to_i64(out: &mut String, ty: &Type) -> Result<String, EmitError> {
    match ty {
        Type::Integer(64) => Ok("%ret".to_owned()),
        Type::Integer(_) => {
            let _ = writeln!(out, "  %ret64 = zext {} %ret to i64", llvm_type(ty)?);
            Ok("%ret64".to_owned())
        }
        Type::Float(32) => {
            out.push_str("  %retb = bitcast float %ret to i32\n");
            out.push_str("  %ret64 = zext i32 %retb to i64\n");
            Ok("%ret64".to_owned())
        }
        Type::Float(64) => {
            out.push_str("  %ret64 = bitcast double %ret to i64\n");
            Ok("%ret64".to_owned())
        }
        Type::Pointer | Type::Reference(_) | Type::Unique(_) | Type::Weak(_) | Type::Address(_) => {
            out.push_str("  %ret64 = ptrtoint ptr %ret to i64\n");
            Ok("%ret64".to_owned())
        }
        other => Err(EmitError(format!("cannot return `{other}` from entry yet"))),
    }
}
