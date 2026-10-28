//! Trait objects across the RIDL boundary (sections 21.7, 22.6 and 23.2):
//! the witness tables of the conformances a module defines, with a witness
//! entry per method around the frontend's own method; the accessors through
//! which the frontend reaches its own tables and those of the modules it
//! imports; and the adapters through which it calls a method of any trait
//! object, whoever made it.
//!
//! A table is a mutable global: its `td` is the frontend's descriptor of the
//! conforming type, which the frontend builds, so a constructor fills it in
//! before anything reads it.

use super::adapters::Callee;
use super::{Ir, Receiving, quote};
use crate::module::{ExtendBody, ItemKind, Module, TraitDecl};
use crate::types::Type;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A conformance `extend Member: Trait` the frontend defines.
pub struct Conformance {
    /// The module the conforming type is declared in, which names the table.
    pub module: String,
    pub member: String,
    pub trait_name: String,
    /// The frontend function answering the address of the conforming type's
    /// descriptor.
    pub descriptor: String,
    /// The object symbol the frontend calls for the table's address.
    pub accessor: String,
    /// Method name to the frontend function implementing it. A method the
    /// member does not implement has no row and traps when called.
    pub methods: BTreeMap<String, String>,
}

/// A table another module exports that the frontend reaches.
pub struct ImportedTable {
    pub module: String,
    pub member: String,
    pub trait_name: String,
    pub accessor: String,
}

/// A default a trait's module exports for a method (symbol kind `M` on the
/// trait, section 23.2), implemented by a frontend function taking the trait
/// object.
pub struct WitnessDefault {
    pub trait_name: String,
    pub method: String,
    pub function: String,
}

/// A method of a trait the frontend calls through trait objects.
pub struct WitnessCall {
    pub trait_name: String,
    pub method: String,
    /// The object symbol the frontend calls, in its C view.
    pub caller: String,
}

/// What the frontend asks of one module's traits: the tables it defines, the
/// tables it imports, the methods it calls and the defaults it implements.
pub struct WitnessSet<'a> {
    pub conformances: &'a [Conformance],
    pub imported: &'a [ImportedTable],
    pub calls: &'a [WitnessCall],
    pub defaults: &'a [WitnessDefault],
}

/// The LLVM IR of the tables, accessors and call adapters of the traits of
/// `module`, the module declaring them. `destroys` names the frontend's
/// destroy function of each type spelling the methods take, and `defined` the
/// object symbol of a frontend function. `own` says whether `module` is the
/// program's own: its descriptors are then the `D` symbols it exports, which
/// its RIDL entries may define as well, and those of another module are its
/// to export, so they stay private here.
///
/// # Errors
/// Returns a message when a trait is not in `module` or a type has no layout.
pub fn witnesses(
    module: &Module,
    witnessed: &WitnessSet<'_>,
    destroys: &BTreeMap<String, String>,
    own: bool,
    target: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let WitnessSet {
        conformances,
        imported,
        calls,
        defaults,
    } = *witnessed;
    let registry = module.registry();
    let mut ir = Ir::new(&registry, target, &module.name, destroys, defined, true);
    ir.descriptor_linkage = if own { "weak_odr " } else { "private " };
    let traits: BTreeMap<&str, &TraitDecl> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Trait(declared) => Some((declared.name.as_str(), declared)),
            _ => None,
        })
        .collect();
    let find = |name: &str| {
        traits
            .get(name)
            .copied()
            .ok_or_else(|| format!("the trait `{name}` is not in {}", module.name))
    };
    let mut fills = Vec::new();
    for conformance in conformances {
        let declared = find(&conformance.trait_name)?;
        let defaulted = defaulted(module, &conformance.trait_name);
        fills.push(ir.table(target, conformance, declared, &defaulted)?);
    }
    for table in imported {
        let symbol = crate::mangle::witness_symbol(
            &table.module,
            &table.member,
            &module.name,
            &table.trait_name,
        );
        let accessor = &table.accessor;
        ir.declare(&symbol, &format!("{} = external global i8", quote(&symbol)));
        let _ = write!(
            ir.functions,
            "define weak_odr ptr {}() {{\nentry:\n  ret ptr {}\n}}\n\n",
            quote(accessor),
            quote(&symbol)
        );
    }
    for call in calls {
        let declared = find(&call.trait_name)?;
        let Some(slot) = declared
            .methods
            .iter()
            .position(|method| method.name == call.method)
        else {
            return Err(format!(
                "the trait `{}` has no method `{}`",
                call.trait_name, call.method
            ));
        };
        let owner = Type::TraitObject(call.trait_name.clone());
        let caller = &call.caller;
        ir.define_adapter(
            target,
            caller,
            &Callee::Witness(slot),
            Some(&owner),
            &declared.methods[slot],
        )?;
    }
    for default in defaults {
        let declared = find(&default.trait_name)?;
        let Some(method) = declared
            .methods
            .iter()
            .find(|method| method.name == default.method)
        else {
            return Err(format!(
                "the trait `{}` has no method `{}`",
                default.trait_name, default.method
            ));
        };
        let owner = Type::TraitObject(default.trait_name.clone());
        let symbol =
            crate::mangle::method_symbol(&module.name, &default.trait_name, &method.name, None);
        let callee = defined(&default.function);
        // A program built in units defines a default where its tables,
        // which declare it, are built.
        ir.declarations.remove(&symbol);
        ir.define_entry(
            target,
            &callee,
            &symbol,
            Some(&owner),
            method,
            Receiving::Default,
        )?;
    }
    if !fills.is_empty() {
        let mut body = String::new();
        for (index, (table, descriptor)) in fills.iter().enumerate() {
            let _ = writeln!(body, "  %td{index} = call ptr {}()", quote(descriptor));
            let _ = writeln!(
                body,
                "  %at{index} = getelementptr inbounds i8, ptr {}, i64 8",
                quote(table)
            );
            let _ = writeln!(body, "  store ptr %td{index}, ptr %at{index}, align 1");
        }
        let _ = write!(
            ir.functions,
            "define private void @ridl.witnesses() {{\nentry:\n{body}  ret void\n}}\n\n"
        );
        let _ = writeln!(
            ir.globals,
            "@llvm.global_ctors = appending global [1 x {{ i32, ptr, ptr }}] [{{ i32, ptr, ptr }} {{ i32 65535, ptr @ridl.witnesses, ptr null }}]"
        );
    }
    Ok(ir.finish())
}

impl Ir<'_> {
    /// One conformance's table, its witness entries and its accessor.
    /// Returns the table's symbol and the descriptor function filling it.
    fn table(
        &mut self,
        target: &str,
        conformance: &Conformance,
        declared: &TraitDecl,
        defaulted: &[String],
    ) -> Result<(String, String), String> {
        let symbol = crate::mangle::witness_symbol(
            &conformance.module,
            &conformance.member,
            self.module_name,
            &conformance.trait_name,
        );
        let owner = Type::nominal(conformance.member.clone());
        let mut entries = Vec::new();
        for method in &declared.methods {
            let entry = format!(
                "ridl.witness.{}.{}.{}",
                conformance.member, conformance.trait_name, method.name
            );
            match conformance.methods.get(&method.name) {
                Some(function) => {
                    let callee = (self.defined)(function);
                    self.define_entry(
                        target,
                        &callee,
                        &entry,
                        Some(&owner),
                        method,
                        Receiving::Witness,
                    )?;
                }
                None if defaulted.contains(&method.name) => {
                    // The default the trait's module exports takes the
                    // entry's place (section 23.2).
                    let default = crate::mangle::method_symbol(
                        self.module_name,
                        &conformance.trait_name,
                        &method.name,
                        None,
                    );
                    self.declare(&default, &format!("declare void {}()", quote(&default)));
                    entries.push(format!("ptr {}", quote(&default)));
                    continue;
                }
                None => self.trap_entry(&entry),
            }
            entries.push(format!("ptr {}", quote(&entry)));
        }
        let count = entries.len();
        let _ = writeln!(
            self.globals,
            "{} = global {{ i32, i32, ptr, [{count} x ptr] }} {{ i32 1, i32 {count}, ptr null, [{count} x ptr] [{}] }}, align {}",
            quote(&symbol),
            entries.join(", "),
            self.word
        );
        let accessor = &conformance.accessor;
        let _ = write!(
            self.functions,
            "define ptr {}() {{\nentry:\n  ret ptr {}\n}}\n\n",
            quote(accessor),
            quote(&symbol)
        );
        let descriptor = (self.defined)(&conformance.descriptor);
        self.declare(
            &descriptor,
            &format!("declare ptr {}()", quote(&descriptor)),
        );
        Ok((symbol, descriptor))
    }

    /// The witness entry of a method the conforming type does not implement:
    /// a `required` value it was never given. Calling it is a contract
    /// violation, so it traps.
    fn trap_entry(&mut self, entry: &str) {
        self.declare("llvm.trap", "declare void @llvm.trap()");
        let _ = write!(
            self.functions,
            "define private swiftcc void {}() {{\nentry:\n  call void @llvm.trap()\n  unreachable\n}}\n\n",
            quote(entry)
        );
    }
}

/// The methods of `trait_name` its module exports a default for: those an
/// `extend` of the trait declares (section 20.3).
fn defaulted(module: &Module, trait_name: &str) -> Vec<String> {
    let mut names = Vec::new();
    for item in &module.items {
        let ItemKind::Extend(extend) = &item.kind else {
            continue;
        };
        let (Type::Nominal { name, .. } | Type::TraitObject(name)) = &extend.target else {
            continue;
        };
        if name != trait_name {
            continue;
        }
        if let ExtendBody::Methods(methods) = &extend.body {
            names.extend(methods.iter().map(|method| method.name.clone()));
        }
    }
    names
}
