//! VM-only Kira helpers resolve permitted scalar RIDL imports to their host
//! connection instead of falling through to the native dynamic linker.

use klf_ridl::module::{ExtendBody, ItemKind, Module};
use klf_ridl::types::{Scalar, Type};
use klf_ridl_wire::transport::Connection;
use klf_ridl_wire::{Reply, Value};
use klf_vm::{Capabilities, ForeignClass, ForeignSignature, ImportKind, Program};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// An import generated from a trusted RIDL declaration. `symbol` is the
/// compiled Kira call's foreign symbol, while `path` names the host export.
#[derive(Clone, Debug)]
pub struct ScalarImport {
    pub symbol: String,
    pub path: String,
    pub signature: ForeignSignature,
}

/// Binds imports recorded by the compiler to a private host connection.
///
/// This handles primitive RIDL values only. Nominal values, buffers, objects
/// and resource types require the RIDL layout marshaller, not pointer-sized
/// arguments; no pointer is accepted here as a proxy for those values.
pub fn bind_vm_host_imports(
    program: &Program,
    imports: &[ScalarImport],
    connection: &Arc<Connection>,
    capabilities: &mut Capabilities,
) -> Result<(), String> {
    for import in imports {
        let found = program.imports.iter().any(|candidate| {
            candidate.symbol == import.symbol
                && candidate.kind == ImportKind::Call(import.signature.clone())
        });
        if !found {
            return Err(format!(
                "RIDL import `{}` has no matching compiled signature",
                import.symbol
            ));
        }
        if !import.signature.parameters.iter().all(valid_class)
            || import
                .signature
                .result
                .is_some_and(|class| !valid_class(&class))
        {
            return Err(format!(
                "RIDL import `{}` cannot cross as scalar values",
                import.symbol
            ));
        }
        let signature = import.signature.clone();
        let path = import.path.clone();
        let connection = Arc::clone(connection);
        capabilities.register(import.symbol.clone(), move |words| {
            if words.len() != signature.parameters.len() {
                return u64::MAX;
            }
            let values = signature
                .parameters
                .iter()
                .zip(words)
                .map(|(class, word)| encode(*class, *word))
                .collect();
            let Ok(Reply::Returned(answer)) = connection.call(&path, values) else {
                return u64::MAX;
            };
            match signature.result {
                None if answer == Value::Void => 0,
                Some(class) => decode(class, answer).unwrap_or(u64::MAX),
                _ => u64::MAX,
            }
        });
    }
    Ok(())
}

/// Bind the `ridl.import` and `ridl.import.entry` attributes emitted by Kira's
/// existing `@Ridl` lowering. No second binding generator or string-based
/// application API is required. Unsupported, non-scalar RIDL signatures are
/// rejected, rather than casting pointers into wire values.
///
/// This is invoked with the KIR module *before* bytecode emission, while the
/// existing compiler-generated RIDL attributes are still available.
pub fn bind_generated_ridl_imports(
    module: &klf_kir::Module,
    program: &Program,
    connection: &Arc<Connection>,
    capabilities: &mut Capabilities,
) -> Result<(), String> {
    let mut entries: HashMap<(String, String), Vec<String>> = HashMap::new();
    for attribute in &module.attributes {
        if attribute.name != "ridl.import.entry" {
            continue;
        }
        let Some(record) = attribute.value.as_deref() else {
            return Err("RIDL import entry has no value".into());
        };
        let mut fields = record.split('\t');
        let (Some(module), Some(path), Some(symbol), Some(_shape)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("malformed generated RIDL import entry `{record}`"));
        };
        if fields.next().is_some() {
            return Err(format!("malformed generated RIDL import entry `{record}`"));
        }
        let symbols = entries
            .entry((module.to_owned(), path.to_owned()))
            .or_default();
        let symbol = klf_kir::c_symbol(symbol).to_owned();
        if !symbols.contains(&symbol) {
            symbols.push(symbol);
        }
    }
    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    for attribute in &module.attributes {
        if attribute.name != "ridl.import" {
            continue;
        }
        let Some(record) = attribute.value.as_deref() else {
            return Err("RIDL imported module has no contract".into());
        };
        let (module_name, contract) = record
            .split_once('\t')
            .ok_or_else(|| "RIDL imported module is missing its contract".to_owned())?;
        let parsed = klf_ridl::parse(klf_core::FileId::from_u32(0), module_name, contract)
            .map_err(|error| format!("invalid generated RIDL contract: {error}"))?;
        if let Some(known) = modules.get_mut(module_name) {
            known.join(parsed);
        } else {
            modules.insert(module_name.to_owned(), parsed);
        }
    }
    for imported in modules.values() {
        bind_module(imported, program, connection, capabilities, &entries)?;
    }
    Ok(())
}

fn bind_module(
    module: &Module,
    program: &Program,
    connection: &Arc<Connection>,
    capabilities: &mut Capabilities,
    entries: &HashMap<(String, String), Vec<String>>,
) -> Result<(), String> {
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => {
                bind_function(
                    module,
                    &function.name,
                    function,
                    program,
                    connection,
                    capabilities,
                    entries,
                )?;
            }
            ItemKind::Extend(extend) => {
                if let ExtendBody::Methods(methods) = &extend.body {
                    let Type::Nominal { name, .. } = &extend.target else {
                        return Err("remote RIDL methods require a declared receiver type".into());
                    };
                    for function in methods {
                        bind_function(
                            module,
                            &format!("{name}.{}", function.name),
                            function,
                            program,
                            connection,
                            capabilities,
                            entries,
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn bind_function(
    module: &Module,
    path: &str,
    function: &klf_ridl::module::FunctionDecl,
    program: &Program,
    connection: &Arc<Connection>,
    capabilities: &mut Capabilities,
    entries: &HashMap<(String, String), Vec<String>>,
) -> Result<(), String> {
    let Some(symbols) = entries.get(&(module.name.clone(), path.to_owned())) else {
        return Ok(());
    };
    if !function.generics.is_empty() || function.self_param.is_some() {
        return Err(format!(
            "remote RIDL `{path}` requires a non-scalar adapter"
        ));
    }
    let types: Vec<Scalar> = function
        .params
        .iter()
        .map(|param| match &param.ty {
            Type::Scalar(ty) if scalar_width(*ty).is_some() => Ok(*ty),
            _ => Err(format!(
                "remote RIDL `{path}` requires a non-scalar adapter"
            )),
        })
        .collect::<Result<_, _>>()?;
    let result = match function.result.as_ref() {
        None | Some(Type::Scalar(Scalar::Void)) => None,
        Some(Type::Scalar(ty)) if scalar_width(*ty).is_some() => Some(*ty),
        _ => {
            return Err(format!(
                "remote RIDL `{path}` requires a non-scalar result adapter"
            ));
        }
    };
    for symbol in symbols {
        let signature = program
            .imports
            .iter()
            .find_map(|import| match &import.kind {
                ImportKind::Call(signature) if &import.symbol == symbol => Some(signature),
                _ => None,
            })
            .ok_or_else(|| format!("compiled RIDL symbol `{symbol}` is not a direct call"))?;
        if signature.parameters.len() != types.len()
            || signature
                .parameters
                .iter()
                .zip(&types)
                .any(|(class, ty)| !class_compatible(*class, *ty))
            || match (signature.result, result) {
                (Some(class), Some(ty)) => !class_compatible(class, ty),
                (None, None) => false,
                _ => true,
            }
        {
            return Err(format!(
                "compiled RIDL symbol `{symbol}` disagrees with its contract"
            ));
        }
        let types = types.clone();
        let wire_path = format!("{}.{}", module.name, path);
        let connection = Arc::clone(connection);
        capabilities.register(symbol.clone(), move |words| {
            if words.len() != types.len() {
                return u64::MAX;
            }
            let arguments = types
                .iter()
                .zip(words)
                .map(|(ty, word)| encode_scalar(*ty, *word))
                .collect();
            let Ok(Reply::Returned(answer)) = connection.call(&wire_path, arguments) else {
                return u64::MAX;
            };
            match result {
                None if answer == Value::Void => 0,
                Some(ty) => decode_scalar(ty, answer).unwrap_or(u64::MAX),
                _ => u64::MAX,
            }
        });
    }
    Ok(())
}

pub(crate) fn scalar_width(scalar: Scalar) -> Option<u16> {
    Some(match scalar {
        Scalar::I8 | Scalar::U8 => 8,
        Scalar::I16 | Scalar::U16 => 16,
        Scalar::I32 | Scalar::U32 | Scalar::F32 => 32,
        Scalar::I64 | Scalar::U64 | Scalar::F64 | Scalar::ISize | Scalar::USize => 64,
        Scalar::Bool => 1,
        _ => return None,
    })
}

fn class_width(class: ForeignClass) -> u16 {
    match class {
        ForeignClass::Signed(width) | ForeignClass::Unsigned(width) => width,
        ForeignClass::F32 => 32,
        ForeignClass::F64 => 64,
    }
}

fn class_compatible(class: ForeignClass, ty: Scalar) -> bool {
    match (class, ty) {
        (ForeignClass::F32, Scalar::F32) | (ForeignClass::F64, Scalar::F64) => true,
        (ForeignClass::Signed(_) | ForeignClass::Unsigned(_), Scalar::F32 | Scalar::F64) => false,
        (ForeignClass::F32 | ForeignClass::F64, _) => false,
        (class, ty) => scalar_width(ty) == Some(class_width(class)),
    }
}

pub(crate) fn encode_scalar(scalar: Scalar, value: u64) -> Value {
    match scalar {
        Scalar::I8 => encode(ForeignClass::Signed(8), value),
        Scalar::I16 => encode(ForeignClass::Signed(16), value),
        Scalar::I32 => encode(ForeignClass::Signed(32), value),
        Scalar::I64 | Scalar::ISize => encode(ForeignClass::Signed(64), value),
        Scalar::U8 => encode(ForeignClass::Unsigned(8), value),
        Scalar::U16 => encode(ForeignClass::Unsigned(16), value),
        Scalar::U32 => encode(ForeignClass::Unsigned(32), value),
        Scalar::U64 | Scalar::USize => encode(ForeignClass::Unsigned(64), value),
        Scalar::Bool => Value::Bool(value != 0),
        Scalar::F32 => encode(ForeignClass::F32, value),
        Scalar::F64 => encode(ForeignClass::F64, value),
        _ => unreachable!("validated by scalar_width"),
    }
}

pub(crate) fn decode_scalar(scalar: Scalar, value: Value) -> Option<u64> {
    match (scalar, value) {
        (Scalar::Bool, Value::Bool(flag)) => Some(u64::from(flag)),
        (Scalar::I8, value) => decode(ForeignClass::Signed(8), value),
        (Scalar::I16, value) => decode(ForeignClass::Signed(16), value),
        (Scalar::I32, value) => decode(ForeignClass::Signed(32), value),
        (Scalar::I64 | Scalar::ISize, value) => decode(ForeignClass::Signed(64), value),
        (Scalar::U8, value) => decode(ForeignClass::Unsigned(8), value),
        (Scalar::U16, value) => decode(ForeignClass::Unsigned(16), value),
        (Scalar::U32, value) => decode(ForeignClass::Unsigned(32), value),
        (Scalar::U64 | Scalar::USize, value) => decode(ForeignClass::Unsigned(64), value),
        (Scalar::F32, value) => decode(ForeignClass::F32, value),
        (Scalar::F64, value) => decode(ForeignClass::F64, value),
        _ => None,
    }
}

fn valid_class(class: &ForeignClass) -> bool {
    match class {
        ForeignClass::Signed(width) | ForeignClass::Unsigned(width) => (1..=64).contains(width),
        ForeignClass::F32 | ForeignClass::F64 => true,
    }
}

fn encode(class: ForeignClass, value: u64) -> Value {
    match class {
        ForeignClass::Signed(width) => {
            let shift = 64 - width;
            Value::Int(((value << shift) as i64 >> shift) as i128)
        }
        ForeignClass::Unsigned(width) => {
            Value::Uint(u128::from(value & (u64::MAX >> (64 - width))))
        }
        ForeignClass::F32 => Value::Float(f64::from(f32::from_bits(value as u32)).to_bits()),
        ForeignClass::F64 => Value::Float(value),
    }
}

fn decode(class: ForeignClass, value: Value) -> Option<u64> {
    match (class, value) {
        (ForeignClass::Signed(width), Value::Int(value)) => {
            let min = -(1_i128 << (width - 1));
            let max = (1_i128 << (width - 1)) - 1;
            (min..=max).contains(&value).then_some(value as i64 as u64)
        }
        (ForeignClass::Unsigned(width), Value::Uint(value)) => {
            (value <= (u128::MAX >> (128 - width))).then_some(value as u64)
        }
        (ForeignClass::F32, Value::Float(bits)) => {
            Some((f64::from_bits(bits) as f32).to_bits() as u64)
        }
        (ForeignClass::F64, Value::Float(bits)) => Some(bits),
        _ => None,
    }
}
