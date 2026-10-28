//! Trait objects a program makes or calls across the RIDL boundary
//! (sections 21.7, 22.6 and 23.2). The frontend records them in module
//! attributes, each grouped under the module declaring the trait:
//!
//! - `ridl.witness.trait`: the module and the `.ridl` text declaring the
//!   trait and the types its methods take.
//! - `ridl.witness.table`: a conformance the program defines: the trait's
//!   module, the conforming type's module, the type, the trait, the function
//!   answering the type's descriptor, the symbol the program calls for the
//!   table, and one `method=function` per method the type implements.
//! - `ridl.witness.import`: a table another module exports: the trait's
//!   module, the type's module, the type, the trait and the symbol the program
//!   calls for it.
//! - `ridl.witness.call`: a method the program calls through trait objects:
//!   the trait's module, the trait, the method and the symbol it calls.
//! - `ridl.witness.default`: a default the program exports for a method of
//!   its own trait: the trait's module, the trait, the method and the
//!   function taking the trait object that implements it.

use klf_kir::Module;
use klf_ridl::llvm::{Conformance, ImportedTable, WitnessCall, WitnessDefault, WitnessSet};
use std::collections::BTreeMap;

const TRAIT_ATTRIBUTE: &str = "ridl.witness.trait";
const TABLE_ATTRIBUTE: &str = "ridl.witness.table";
const IMPORT_ATTRIBUTE: &str = "ridl.witness.import";
const CALL_ATTRIBUTE: &str = "ridl.witness.call";
const DEFAULT_ATTRIBUTE: &str = "ridl.witness.default";

/// What the program does with the traits of one module.
#[derive(Default)]
pub(crate) struct Witnessed {
    /// The `.ridl` text of the traits, once per unit of the program using
    /// them.
    texts: Vec<String>,
    tables: Vec<Vec<String>>,
    imports: Vec<Vec<String>>,
    calls: Vec<Vec<String>>,
    defaults: Vec<Vec<String>>,
}

/// Per module declaring a trait the program uses, what it uses.
pub(crate) fn witnessed(module: &Module) -> BTreeMap<String, Witnessed> {
    let mut modules: BTreeMap<String, Witnessed> = BTreeMap::new();
    for attribute in &module.attributes {
        let Some(value) = attribute.value.as_deref() else {
            continue;
        };
        let fields: Vec<String> = value.split('\t').map(str::to_owned).collect();
        let Some(name) = fields.first().cloned() else {
            continue;
        };
        let rest = fields[1..].to_vec();
        match attribute.name.as_str() {
            TRAIT_ATTRIBUTE => {
                let text = value.split_once('\t').map_or("", |(_, text)| text);
                modules.entry(name).or_default().texts.push(text.to_owned());
            }
            TABLE_ATTRIBUTE => modules.entry(name).or_default().tables.push(rest),
            IMPORT_ATTRIBUTE => modules.entry(name).or_default().imports.push(rest),
            CALL_ATTRIBUTE => modules.entry(name).or_default().calls.push(rest),
            DEFAULT_ATTRIBUTE => modules.entry(name).or_default().defaults.push(rest),
            _ => {}
        }
    }
    modules
}

fn malformed(kind: &str, fields: &[String]) -> String {
    format!("malformed `{kind}` attribute `{}`", fields.join("\t"))
}

/// The LLVM IR of the tables, accessors and call adapters for the traits of
/// the module `name`. `own` is the name of the program's own module, when it
/// has a RIDL surface.
pub(crate) fn source(
    name: &str,
    witnessed: &Witnessed,
    destroys: &BTreeMap<String, String>,
    own: Option<&str>,
    target: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let mut module = klf_ridl::module::Module::new(name);
    for text in &witnessed.texts {
        let parsed =
            klf_ridl::parse(klf_core::FileId::from_u32(0), name, text).map_err(|diagnostic| {
                format!("the RIDL traits of {name} do not parse: {diagnostic}")
            })?;
        module.join(parsed);
    }
    let mut conformances = Vec::new();
    for fields in &witnessed.tables {
        let [
            member_module,
            member,
            trait_name,
            descriptor,
            accessor,
            methods @ ..,
        ] = fields.as_slice()
        else {
            return Err(malformed(TABLE_ATTRIBUTE, fields));
        };
        let mut implemented = BTreeMap::new();
        for method in methods {
            let (method, function) = method
                .split_once('=')
                .ok_or_else(|| malformed(TABLE_ATTRIBUTE, fields))?;
            implemented.insert(method.to_owned(), function.to_owned());
        }
        conformances.push(Conformance {
            module: member_module.clone(),
            member: member.clone(),
            trait_name: trait_name.clone(),
            descriptor: descriptor.clone(),
            accessor: klf_kir::c_symbol(accessor).to_owned(),
            methods: implemented,
        });
    }
    let mut imported = Vec::new();
    for fields in &witnessed.imports {
        let [member_module, member, trait_name, accessor] = fields.as_slice() else {
            return Err(malformed(IMPORT_ATTRIBUTE, fields));
        };
        imported.push(ImportedTable {
            module: member_module.clone(),
            member: member.clone(),
            trait_name: trait_name.clone(),
            accessor: klf_kir::c_symbol(accessor).to_owned(),
        });
    }
    let mut calls = Vec::new();
    for fields in &witnessed.calls {
        let [trait_name, method, caller] = fields.as_slice() else {
            return Err(malformed(CALL_ATTRIBUTE, fields));
        };
        calls.push(WitnessCall {
            trait_name: trait_name.clone(),
            method: method.clone(),
            caller: klf_kir::c_symbol(caller).to_owned(),
        });
    }
    let mut defaults = Vec::new();
    for fields in &witnessed.defaults {
        let [trait_name, method, function] = fields.as_slice() else {
            return Err(malformed(DEFAULT_ATTRIBUTE, fields));
        };
        defaults.push(WitnessDefault {
            trait_name: trait_name.clone(),
            method: method.clone(),
            function: function.clone(),
        });
    }
    klf_ridl::llvm::witnesses(
        &module,
        &WitnessSet {
            conformances: &conformances,
            imported: &imported,
            calls: &calls,
            defaults: &defaults,
        },
        destroys,
        own == Some(name),
        target,
        defined,
    )
}
