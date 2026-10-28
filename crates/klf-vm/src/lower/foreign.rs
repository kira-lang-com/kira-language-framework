//! The native side of a program: the imports its foreign calls and C entries
//! resolve to (section 15.2 `IMPORTS`, section 15.6).

use super::{LowerError, Lowerer, err};
use crate::bytecode::{Instr, Opcode};
use crate::program::{CallSite, ForeignClass, ForeignSignature, Import, ImportKind};
use klf_core::Name;
use klf_kir::{CType, Function, FunctionAbi, Type};

/// The symbol of the native entry C calls a bytecode function through. A `#`
/// in the KIR name is spelled `__k`, as in the C shims.
#[must_use]
pub fn vm_entry_symbol(name: &str) -> String {
    format!("__klf_vm_{}", name.replace('#', "__k"))
}

/// Whether C calls `function`, so the VM gives it a native entry.
#[must_use]
pub fn entered_from_c(function: &Function) -> bool {
    !function.foreign && function.c_prototype.is_some()
}

/// The C signature C calls `function`'s native entry with: each parameter
/// and the result in the class its prototype gives it, as the entry beside a
/// `.klbc` declares them.
#[must_use]
pub fn entry_signature(function: &Function) -> ForeignSignature {
    let prototype = function.c_prototype.as_ref();
    let parameters = function
        .parameters
        .iter()
        .enumerate()
        .map(|(position, parameter)| {
            foreign_class(
                &parameter.ty,
                prototype.and_then(|prototype| prototype.parameters.get(position)),
            )
        })
        .collect();
    let result = match &function.result {
        Type::Unit | Type::Never => None,
        ty => Some(foreign_class(
            ty,
            prototype.map(|prototype| &prototype.result),
        )),
    };
    ForeignSignature { parameters, result }
}

/// The class a KIR value of `ty` crosses into C with; `c` is its C type when
/// the declaration states one, which decides how a narrow integer extends.
#[must_use]
pub fn foreign_class(ty: &Type, c: Option<&CType>) -> ForeignClass {
    match ty {
        Type::Float(32) => ForeignClass::F32,
        Type::Float(_) => ForeignClass::F64,
        Type::Integer(bits) => match c {
            Some(CType::Signed(_)) => ForeignClass::Signed(*bits),
            _ => ForeignClass::Unsigned(*bits),
        },
        _ => ForeignClass::Unsigned(64),
    }
}

/// The C signature of a call to the foreign `function`, which a shim makes
/// when the declaration has a C prototype: the shim takes KIR's carriers, an
/// aggregate as an address and an aggregate result through a leading one.
fn declared_signature(function: &Function) -> ForeignSignature {
    let prototype = function.c_prototype.as_ref();
    let leading =
        usize::from(prototype.is_some_and(|prototype| matches!(prototype.result, CType::Value(_))));
    let parameters = function
        .parameters
        .iter()
        .enumerate()
        .map(|(position, parameter)| {
            let c = position
                .checked_sub(leading)
                .and_then(|index| prototype.and_then(|prototype| prototype.parameters.get(index)));
            foreign_class(&parameter.ty, c)
        })
        .collect();
    let result = match &function.result {
        Type::Unit | Type::Never => None,
        ty => Some(foreign_class(
            ty,
            prototype.map(|prototype| &prototype.result),
        )),
    };
    ForeignSignature { parameters, result }
}

impl Lowerer<'_> {
    pub(super) fn import(&mut self, import: Import) -> Result<u32, LowerError> {
        let pools = &mut *self.consts;
        let index = if let Some(&index) = pools.import_index.get(&import) {
            index
        } else {
            let index = pools.imports.len();
            pools.imports.push(import.clone());
            pools.import_index.insert(import, index);
            index
        };
        u32::try_from(index).map_err(|_| err("too many imports"))
    }

    /// The import a `call.foreign` of `symbol` calls: the C shim of a
    /// declaration with a prototype, the C symbol of one without, and for a
    /// name nothing declares (a kernel service), the name with the classes
    /// of the values passed.
    pub(super) fn foreign_import(
        &mut self,
        symbol: &Name,
        arguments: &[Name],
        results: &[Name],
    ) -> Result<u32, LowerError> {
        let import = match self.functions.get(symbol.as_str()) {
            Some(function) if function.foreign => {
                let name = function.name.as_str();
                let native = if function.c_prototype.is_some() {
                    klf_kir::c_shim_symbol(name)
                } else {
                    klf_kir::c_symbol(name).to_owned()
                };
                Import {
                    symbol: native,
                    kind: ImportKind::Call(declared_signature(function)),
                }
            }
            _ => {
                let class = |name: &Name| {
                    self.types
                        .get(name)
                        .map_or(ForeignClass::Unsigned(64), |ty| foreign_class(ty, None))
                };
                let signature = ForeignSignature {
                    parameters: arguments.iter().map(class).collect(),
                    result: results.first().map(class),
                };
                Import {
                    symbol: symbol.as_str().to_owned(),
                    kind: ImportKind::Call(signature),
                }
            }
        };
        self.import(import)
    }

    /// The import a `call.variadic` of a C import calls: its C symbol, with
    /// the classes of the fixed parameters and of this call's variadic
    /// arguments (section 29.5). C promotes a narrower integer to `int` and
    /// a `float` to `double` before a variadic call, and KIR does not say
    /// whether an integer is signed, so a frontend passes the promoted
    /// value. A function the module defines took an argument area before
    /// lowering, and one with a C prototype is reached through its shim.
    pub(super) fn variadic_import(
        &mut self,
        callee: &Name,
        fixed: &[Name],
        variadic: &[Name],
        results: &[Name],
    ) -> Result<u32, LowerError> {
        let function = self
            .functions
            .get(callee.as_str())
            .filter(|function| function.foreign && function.c_prototype.is_none())
            .ok_or_else(|| {
                err(format!(
                    "`call.variadic` of `@{callee}` uses the platform's variadics, which the VM \
                     passes only to a C import without a C prototype"
                ))
            })?;
        let class = |name: &Name| {
            self.types
                .get(name)
                .map_or(ForeignClass::Unsigned(64), |ty| foreign_class(ty, None))
        };
        let mut parameters: Vec<ForeignClass> = fixed.iter().map(class).collect();
        for argument in variadic {
            let class = class(argument);
            if matches!(class, ForeignClass::F32)
                || matches!(class, ForeignClass::Signed(bits) | ForeignClass::Unsigned(bits) if bits < 32)
            {
                return Err(err(format!(
                    "a variadic argument of `@{callee}` is one C promotes before the call; \
                     pass the promoted value"
                )));
            }
            parameters.push(class);
        }
        let signature = ForeignSignature {
            parameters,
            result: results.first().map(class),
        };
        self.import(Import {
            symbol: klf_kir::c_symbol(function.name.as_str()).to_owned(),
            kind: ImportKind::Variadic {
                fixed: u32::try_from(fixed.len()).map_err(|_| err("too many arguments"))?,
                signature,
            },
        })
    }

    /// The import a `call.indirect` through a platform-convention function
    /// type calls when its target is a C function pointer (section 26.4),
    /// with the classes its arguments and result have. A target of any other
    /// type is a bytecode function, and has none.
    pub(super) fn pointer_import(
        &mut self,
        target: &Name,
        arguments: &[Name],
        results: &[Name],
    ) -> Result<Option<u32>, LowerError> {
        let platform = matches!(
            self.types.get(target),
            Some(Type::Function(signature)) if signature.abi == FunctionAbi::Platform
        );
        if !platform {
            return Ok(None);
        }
        let class = |name: &Name| {
            self.types
                .get(name)
                .map_or(ForeignClass::Unsigned(64), |ty| foreign_class(ty, None))
        };
        let signature = ForeignSignature {
            parameters: arguments.iter().map(class).collect(),
            result: results.first().map(class),
        };
        self.import(Import {
            symbol: String::new(),
            kind: ImportKind::Pointer(signature),
        })
        .map(Some)
    }

    /// A `func.ref` of a function C calls is the address of its native
    /// entry: the C shim when it is entered by value, else the VM's entry.
    pub(super) fn lower_entry_reference(
        &mut self,
        dest: u16,
        function: &Function,
    ) -> Result<(), LowerError> {
        let name = function.name.as_str();
        let needs_shim = function
            .c_prototype
            .as_ref()
            .is_some_and(klf_kir::CPrototype::needs_shim);
        let symbol = if needs_shim {
            klf_kir::c_shim_symbol(name)
        } else {
            vm_entry_symbol(name)
        };
        let import = self.import(Import {
            symbol,
            kind: ImportKind::Address,
        })?;
        let site = u16::try_from(self.call_sites.len()).map_err(|_| err("too many call sites"))?;
        self.call_sites.push(CallSite {
            callee: 0,
            first_arg: 0,
            arg_count: 0,
            result: dest,
            has_result: true,
            foreign: Some(import),
            target: None,
            fallible: None,
        });
        self.emit(Instr::from_bx(Opcode::Call, dest, i32::from(site)));
        Ok(())
    }
}
