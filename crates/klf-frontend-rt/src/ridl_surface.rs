//! The RIDL surface of a library (section 20): what it exports, as a
//! `.ridl` module, and the mangled symbol of each export (section 24.2).
//!
//! A definition that describes its own surface records it in module
//! attributes: `ridl.module` names the module, `ridl.surface` is its `.ridl`
//! text, each `ridl.entry` maps an export's path to the KIR function behind
//! it, and each `ridl.type` names the function destroying a value of an
//! exported type. Such a library gets a `ridlcc` entry per export
//! (`ridl_entries`).
//!
//! Without those attributes only functions whose parameters and result are
//! scalars are described. At the register level `ridlcc` and the platform C
//! convention agree for them (section 22.7), so a library exports each one
//! under its plain name for C callers and under its RIDL symbol through a
//! forwarding function.

use klf_core::Name;
use klf_kir::{Block, Function, FunctionAbi, Instruction, Linkage, Module, Operation, Type};
use klf_ridl::module::{Convention, FunctionDecl, Item, ItemKind, Module as RidlModule, Param};
use klf_ridl::types::{Scalar, Type as RidlType};
use std::collections::BTreeMap;

const MODULE_ATTRIBUTE: &str = "ridl.module";
const SURFACE_ATTRIBUTE: &str = "ridl.surface";
const ENTRY_ATTRIBUTE: &str = "ridl.entry";
const TYPE_ATTRIBUTE: &str = "ridl.type";
const CONTRACT_ATTRIBUTE: &str = "ridl.contract";

/// A surface the definition described itself.
pub(crate) struct Surface {
    /// The `.ridl` text as the definition wrote it, comments included.
    pub text: String,
    pub module: RidlModule,
    /// Export path (`name` or `Type.method`) to the KIR function.
    pub entries: BTreeMap<String, String>,
    /// Type spelling without whitespace to its destroy function; empty for a
    /// type without cleanup.
    pub destroys: BTreeMap<String, String>,
}

fn attributes<'a>(module: &'a Module, name: &'a str) -> impl Iterator<Item = &'a str> {
    module
        .attributes
        .iter()
        .filter(move |attribute| attribute.name == name)
        .filter_map(|attribute| attribute.value.as_deref())
}

/// The `.ridl` text `module` publishes, if the definition wrote one: its
/// surface with the types of other modules imported (KLF-RIDL section 27.7).
pub(crate) fn contract(module: &Module) -> Option<String> {
    attributes(module, CONTRACT_ATTRIBUTE)
        .next()
        .map(str::to_owned)
}

/// The surface `module` describes, if it describes one.
pub(crate) fn described(module: &Module) -> Result<Option<Surface>, String> {
    let Some(text) = attributes(module, SURFACE_ATTRIBUTE).next() else {
        return Ok(None);
    };
    let name = attributes(module, MODULE_ATTRIBUTE)
        .next()
        .ok_or("a RIDL surface without a `ridl.module` attribute")?;
    let parsed = klf_ridl::parse(klf_core::FileId::from_u32(0), name, text)
        .map_err(|diagnostic| format!("the RIDL surface of {name} does not parse: {diagnostic}"))?;
    Ok(Some(Surface {
        text: text.to_owned(),
        module: parsed,
        entries: entries(module)?,
        destroys: destroys(module),
    }))
}

/// Export path (`name` or `Type.method`) to the KIR function behind it, as
/// the `ridl.entry` attributes record.
pub(crate) fn entries(module: &Module) -> Result<BTreeMap<String, String>, String> {
    let mut entries = BTreeMap::new();
    for entry in attributes(module, ENTRY_ATTRIBUTE) {
        let mut fields = entry.split('\t');
        let (Some(path), Some(function)) = (fields.next(), fields.next()) else {
            return Err(format!("malformed `ridl.entry` attribute `{entry}`"));
        };
        entries.insert(path.to_owned(), function.to_owned());
    }
    Ok(entries)
}

/// The name of the module the program's RIDL surface declares, if it has one.
pub(crate) fn module_name(module: &Module) -> Option<&str> {
    attributes(module, MODULE_ATTRIBUTE).next()
}

/// The destroy function of each RIDL type spelling the program records in a
/// `ridl.type` attribute, keyed by the spelling without whitespace.
pub(crate) fn destroys(module: &Module) -> BTreeMap<String, String> {
    let mut destroys = BTreeMap::new();
    for spelled in attributes(module, TYPE_ATTRIBUTE) {
        let (spelling, destroy) = spelled.split_once('\t').unwrap_or((spelled, ""));
        let key: String = spelling.chars().filter(|c| !c.is_whitespace()).collect();
        destroys.insert(key, destroy.to_owned());
    }
    destroys
}

fn scalar(ty: &Type) -> Option<Scalar> {
    Some(match ty {
        Type::Integer(1) => Scalar::Bool,
        Type::Integer(8) => Scalar::I8,
        Type::Integer(16) => Scalar::I16,
        Type::Integer(32) => Scalar::I32,
        Type::Integer(64) => Scalar::I64,
        Type::Integer(128) => Scalar::I128,
        Type::Float(32) => Scalar::F32,
        Type::Float(64) => Scalar::F64,
        Type::Pointer | Type::Address(_) => Scalar::RawPtr,
        _ => return None,
    })
}

/// The functions of `module` a library exports.
pub(crate) fn exported(module: &Module) -> impl Iterator<Item = &Function> {
    module
        .functions
        .iter()
        .filter(|function| !function.foreign && function.linkage != Linkage::Private)
}

fn describe(function: &Function) -> Option<FunctionDecl> {
    if function.failure.is_some() || !function.generics.is_empty() {
        return None;
    }
    let params = function
        .parameters
        .iter()
        .map(|parameter| {
            Some(Param {
                label: parameter.name.to_string(),
                convention: Convention::Owned,
                ty: RidlType::Scalar(scalar(&parameter.ty)?),
                default: None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let result = match &function.result {
        Type::Unit => None,
        other => Some(RidlType::Scalar(scalar(other)?)),
    };
    Some(FunctionDecl {
        name: function.name.to_string(),
        generics: Vec::new(),
        self_param: None,
        params,
        result,
    })
}

/// The `.ridl` module describing the scalar exports of `module`.
pub(crate) fn surface(module: &Module, name: &str) -> RidlModule {
    let mut ridl = RidlModule::new(name);
    ridl.items = exported(module)
        .filter_map(describe)
        .map(|function| Item {
            annotations: Vec::new(),
            kind: ItemKind::Function(function),
        })
        .collect();
    ridl
}

/// Adds a forwarding function named by the RIDL symbol of every described
/// export, and returns `(kir name, exported symbol)` for every export.
/// A described surface, named by its module, gets no forwarders: its entries
/// carry the RIDL symbols, and each function keeps its own convention under
/// `<module>.<name>`, so two libraries' functions never share a symbol.
pub(crate) fn add_ridl_symbols(
    module: &mut Module,
    name: &str,
    export: Option<&str>,
    described: Option<&str>,
) -> Vec<(String, String)> {
    let mut exports = Vec::new();
    let mut forwarders = Vec::new();
    for function in exported(module) {
        if function.name.as_str() == "main" && export != Some("main") {
            continue;
        }
        if let Some(described) = described {
            if function.linkage != Linkage::Exported {
                continue;
            }
            // A name qualified apart from another package's (`name#Module`,
            // or `Type#Module.member`) keeps its own name in the contract, as
            // the RIDL mangler writes each component.
            let surface: Vec<&str> = function
                .name
                .as_str()
                .split('.')
                .map(|part| part.split_once('#').map_or(part, |(plain, _)| plain))
                .collect();
            exports.push((
                function.name.to_string(),
                format!("{described}.{}", surface.join(".")),
            ));
            continue;
        }
        exports.push((function.name.to_string(), function.name.to_string()));
        if describe(function).is_none() {
            continue;
        }
        let symbol = klf_ridl::mangle::function_symbol(name, function.name.as_str(), None);
        forwarders.push(forwarder(function, &symbol));
        exports.push((symbol.clone(), symbol));
    }
    module.functions.extend(forwarders);
    exports
}

fn forwarder(function: &Function, symbol: &str) -> Function {
    let parameters = function.parameters.clone();
    let result = Name::new("forwarded");
    let has_result = function.result != Type::Unit;
    let instructions = vec![
        Instruction {
            results: if has_result {
                vec![result.clone()]
            } else {
                Vec::new()
            },
            operation: Operation::Call {
                callee: function.name.clone(),
                generic_args: Vec::new(),
                arguments: parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect(),
            },
            debug_location: None,
            span: function.span,
        },
        Instruction {
            results: Vec::new(),
            operation: Operation::Return(has_result.then_some(result)),
            debug_location: None,
            span: function.span,
        },
    ];
    Function {
        name: Name::new(symbol),
        linkage: Linkage::Module,
        abi: FunctionAbi::Platform,
        nounwind: function.nounwind,
        foreign: false,
        c_prototype: None,
        attributes: Vec::new(),
        generics: Vec::new(),
        parameters: parameters.clone(),
        result: function.result.clone(),
        failure: None,
        engine: function.engine,
        blocks: vec![Block {
            name: Name::new("forward"),
            parameters,
            instructions,
            span: function.span,
        }],
        span: function.span,
    }
}
