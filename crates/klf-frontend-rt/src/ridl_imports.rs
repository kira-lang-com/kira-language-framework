//! Adapters for the functions a program imports through RIDL (section 20):
//! each defines the symbol the frontend calls, in the frontend's own
//! convention, and calls the providing module's `ridlcc` entry.
//!
//! The frontend records what it imports in module attributes: `ridl.import`
//! is a module name and the `.ridl` text of what the program declares from
//! it, and each `ridl.import.entry` names a module, a path and the symbol the
//! frontend calls. Values the provider hands back hold buffers of its own
//! allocator, so the adapter adopts them before the frontend sees them.

use klf_kir::Module;
use klf_ridl::module::Module as RidlModule;
use std::collections::BTreeMap;

const IMPORT_ATTRIBUTE: &str = "ridl.import";
const IMPORT_ENTRY_ATTRIBUTE: &str = "ridl.import.entry";

/// One module a program imports from.
pub(crate) struct Imported {
    pub module: RidlModule,
    /// Path (`name` or `Type.method`) to the symbol the frontend calls.
    pub entries: BTreeMap<String, String>,
}

/// The modules `module` imports from through RIDL. A program built in units
/// records each module once per unit importing from it.
///
/// Only the entries something in the module names get an adapter: the
/// frontend records every function a contract declares, and an adapter for
/// one nothing calls would leave the program needing a symbol it never uses.
/// What a program links against is then what it calls, whatever else its
/// providers declare (section 9.3, requirement 5).
pub(crate) fn imported(module: &Module) -> Result<Vec<Imported>, String> {
    let named: std::collections::HashSet<&str> = module
        .named_functions()
        .into_iter()
        .map(klf_core::Name::as_str)
        .collect();
    let mut imports: Vec<Imported> = Vec::new();
    for attribute in &module.attributes {
        let Some(value) = attribute
            .value
            .as_deref()
            .filter(|_| attribute.name == IMPORT_ATTRIBUTE)
        else {
            continue;
        };
        let (name, text) = value.split_once('\t').unwrap_or((value, ""));
        let parsed =
            klf_ridl::parse(klf_core::FileId::from_u32(0), name, text).map_err(|diagnostic| {
                format!("the RIDL import of {name} does not parse: {diagnostic}")
            })?;
        if let Some(known) = imports.iter_mut().find(|known| known.module.name == name) {
            known.module.join(parsed);
            continue;
        }
        let mut entries = BTreeMap::new();
        for entry in &module.attributes {
            let Some(entry) = entry
                .value
                .as_deref()
                .filter(|_| entry.name == IMPORT_ENTRY_ATTRIBUTE)
            else {
                continue;
            };
            let mut fields = entry.split('\t');
            if fields.next() != Some(name) {
                continue;
            }
            let (Some(path), Some(symbol)) = (fields.next(), fields.next()) else {
                return Err(format!("malformed `ridl.import.entry` attribute `{entry}`"));
            };
            if named.contains(symbol) {
                entries.insert(path.to_owned(), symbol.to_owned());
            }
        }
        imports.push(Imported {
            module: parsed,
            entries,
        });
    }
    Ok(imports)
}

/// The LLVM IR of the adapters for one imported module. Kira holds only
/// buffers of its own allocator, so the adapters adopt what they return.
pub(crate) fn source(
    imported: &Imported,
    target: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let callers: BTreeMap<String, String> = imported
        .entries
        .iter()
        .map(|(path, symbol)| (path.clone(), klf_kir::c_symbol(symbol).to_owned()))
        .collect();
    klf_ridl::llvm::adapters(&imported.module, &callers, target, defined, true)
}

/// For separately executed Kira applications, use RIDL wire adapters rather
/// than linking against the provider's `ridlcc` entry in this process.
pub(crate) fn remote_source(imported: &Imported, target: &str) -> Result<String, String> {
    let callers: BTreeMap<String, String> = imported
        .entries
        .iter()
        .map(|(path, symbol)| (path.clone(), klf_kir::c_symbol(symbol).to_owned()))
        .collect();
    klf_ridl::llvm::remote_adapters(&imported.module, &callers, target)
}

pub(crate) fn remote_imports(module: &Module) -> bool {
    module
        .attributes
        .iter()
        .any(|attribute| attribute.name == "ridl.remote")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_isolated_kira_modules_select_remote_ridl_adapters() {
        let kir = r#"
module @Plugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, 2
  %b = const.int i64, 40
  %result = call.foreign @harmony_add(%a, %b)
  return %result
}
"#;
        let mut program = klf_kir::parse_module(kir).unwrap();
        program.attributes.extend([
            klf_kir::ModuleAttribute {
                name: "ridl.import".into(),
                value: Some("Harmony\tfunction add(a: I64, b: I64) -> I64\n".into()),
            },
            klf_kir::ModuleAttribute {
                name: "ridl.import.entry".into(),
                value: Some("Harmony\tadd\tharmony_add\tI64,I64->I64".into()),
            },
        ]);
        let imports = imported(&program).unwrap();
        assert_eq!(imports.len(), 1);
        assert!(!remote_imports(&program));
        let local = source(&imports[0], "aarch64-apple-darwin", &str::to_owned).unwrap();
        assert!(local.contains("_RIDL1"));

        program.attributes.push(klf_kir::ModuleAttribute {
            name: "ridl.remote".into(),
            value: None,
        });
        assert!(remote_imports(&program));
        let remote = remote_source(&imports[0], "aarch64-apple-darwin").unwrap();
        assert!(remote.contains("klfrt_ridl_remote_scalar"));
        assert!(!remote.contains("_RIDL1"));
    }
}
