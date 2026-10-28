//! The type, signature and function-shape half of the low-level KIR builder.
//!
//! A definition composes an arbitrary KIR type through a handle before it can
//! name a constant, a slot or a parameter, and it assembles a signature one
//! parameter at a time because `FunctionBuilder` takes them all at once. Those
//! services are here; the value and memory operations built against the types
//! stay in the sibling module.

use super::{BuilderFunction, DefinitionServiceState};
use crate::low_level_builder::signed;
use klf_core::Name;
use klf_kir::{FunctionAbi, FunctionType, ParameterType, Type};
use klf_services::DefaultConvention;

impl DefinitionServiceState {
    pub(super) fn ll_type_size(&mut self, arguments: &[u64]) -> u64 {
        self.ll_type_layout(arguments)
            .map_or(u64::MAX, |layout| layout.size)
    }

    pub(super) fn ll_type_align(&mut self, arguments: &[u64]) -> u64 {
        self.ll_type_layout(arguments)
            .map_or(u64::MAX, |layout| layout.align)
    }

    fn ll_type_layout(&self, arguments: &[u64]) -> Option<klf_layout::Layout> {
        let [module, type_id] = arguments else {
            return None;
        };
        let ty = self.kir_type(Some(*type_id))?;
        usize::try_from(*module)
            .ok()
            .and_then(|index| self.modules.get(index))
            .and_then(Option::as_ref)?;
        klf_layout::try_layout_with(&ty, &|name| self.types.declaration(*module, name)).ok()
    }

    pub(super) fn ll_type_float(&mut self, arguments: &[u64]) -> u64 {
        let Some(bits) = signed(arguments.first()).and_then(|bits| u16::try_from(bits).ok()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Float(bits))
    }

    pub(super) fn ll_type_integer(&mut self, arguments: &[u64]) -> u64 {
        let Some(bits) = signed(arguments.first()).and_then(|bits| u16::try_from(bits).ok()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Integer(bits))
    }

    pub(super) fn ll_type_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(inner) = self.kir_type(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Address(Box::new(inner)))
    }

    pub(super) fn ll_type_reference(&mut self, arguments: &[u64]) -> u64 {
        let Some(inner) = self.kir_type(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Reference(Box::new(inner)))
    }

    pub(super) fn ll_type_unique(&mut self, arguments: &[u64]) -> u64 {
        let Some(inner) = self.kir_type(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Unique(Box::new(inner)))
    }

    pub(super) fn ll_type_array(&mut self, arguments: &[u64]) -> u64 {
        let Some(length) = signed(arguments.first()).and_then(|len| u64::try_from(len).ok()) else {
            return u64::MAX;
        };
        let Some(element) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Array(length, Box::new(element)))
    }

    pub(super) fn ll_type_unit(&mut self, _arguments: &[u64]) -> u64 {
        self.kir_new_type(Type::Unit)
    }

    pub(super) fn ll_type_pointer(&mut self, _arguments: &[u64]) -> u64 {
        self.kir_new_type(Type::Pointer)
    }

    pub(super) fn ll_type_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(ty) = self.kir_type(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.kir_type_parameters.push(ty);
        0
    }

    /// The type of a function over the parameter types pushed since the last
    /// function type was built. `func.ref` is typed this way, so a definition
    /// that stores a function reference needs the same spelling for it.
    pub(super) fn ll_type_function(&mut self, arguments: &[u64]) -> u64 {
        let abi = self
            .file
            .and_then(|file| self.file_conventions.get(file.index()).copied().flatten())
            .map_or(FunctionAbi::Platform, |convention| match convention {
                DefaultConvention::Ridl => FunctionAbi::Ridl,
                DefaultConvention::Platform => FunctionAbi::Platform,
            });
        self.function_type(arguments, abi)
    }

    /// The type of a C function pointer over the parameter types pushed since
    /// the last function type was built: the platform convention whatever the
    /// file's, because C calls it and is called through it (section 26.4).
    pub(super) fn ll_type_foreign_function(&mut self, arguments: &[u64]) -> u64 {
        self.function_type(arguments, FunctionAbi::Platform)
    }

    fn function_type(&mut self, arguments: &[u64], abi: FunctionAbi) -> u64 {
        let Some(result) = self.kir_type(arguments.first().copied()) else {
            return u64::MAX;
        };
        let parameters = self
            .kir_type_parameters
            .drain(..)
            .map(|ty| ParameterType {
                convention: None,
                context: false,
                ty,
            })
            .collect();
        self.kir_new_type(Type::Function(FunctionType {
            abi,
            parameters,
            result: Box::new(result),
            failure: None,
        }))
    }

    /// An address handle viewed as the value it already is. Emits nothing: a
    /// stack slot, a global address and a derived address are all ordinary SSA
    /// values of address type, and a definition that stores or converts one
    /// needs it in the value handle space.
    pub(super) fn ll_value_from_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, name, element, _)) = self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        self.kir_new_value(function_id, name, Type::Address(Box::new(element)))
    }

    /// The reverse view: a value that holds an address, addressed as memory of
    /// the given element type. Emits nothing; the value is already live, so the
    /// address counts as initialized.
    pub(super) fn ll_address_from_value(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(name) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(element) = self.kir_type(arguments.get(2).copied()) else {
            return u64::MAX;
        };
        self.kir_new_address(function_id, name, element, true)
    }

    pub(super) fn ll_address_cast(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, address, _, _)) = self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.addr_cast(address, ty.clone());
        self.kir_new_address(function_id, name, ty, true)
    }

    pub(super) fn ll_function_define(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(result_type) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        if usize::try_from(module)
            .ok()
            .and_then(|index| self.modules.get(index))
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &[], result_type.clone(), span),
            result_type,
            parameters: Vec::new(),
            parameter_types: Vec::new(),
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    pub(super) fn ll_function_result(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.ret(Some(value));
        0
    }

    pub(super) fn ll_signature_new(&mut self, arguments: &[u64]) -> u64 {
        self.ll_signature_new_with_name(arguments, None)
    }

    pub(super) fn ll_signature_new_named(&mut self, arguments: &[u64]) -> u64 {
        let Some(suffix) = arguments.get(4).copied() else {
            return u64::MAX;
        };
        self.ll_signature_new_with_name(arguments, Some(suffix))
    }

    fn ll_signature_new_with_name(&mut self, arguments: &[u64], suffix: Option<u64>) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(result) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let name = suffix.map_or_else(
            || name.clone(),
            |suffix| Name::new(format!("{}#{suffix}", name.as_str())),
        );
        if usize::try_from(module)
            .ok()
            .and_then(|index| self.modules.get(index))
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let id = u64::try_from(self.kir_signatures.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.kir_signatures.push(Some(super::PendingSignature {
                module,
                name,
                name_bytes: None,
                span,
                result,
                parameters: Vec::new(),
                c_parameters: Vec::new(),
                c_result: None,
                c_variadic: None,
                c_syscall: None,
                exported: false,
            }));
        }
        id
    }

    pub(super) fn ll_signature_name_byte(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = arguments
            .first()
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(byte) = signed(arguments.get(1)).and_then(|byte| u8::try_from(byte).ok()) else {
            return u64::MAX;
        };
        let Some(Some(signature)) = self.kir_signatures.get_mut(index) else {
            return u64::MAX;
        };
        signature.name_bytes.get_or_insert_with(Vec::new).push(byte);
        0
    }

    pub(super) fn ll_signature_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .first()
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(Some(signature)) = self.kir_signatures.get_mut(index) else {
            return u64::MAX;
        };
        signature.parameters.push(ty);
        0
    }

    /// `klf.kir.signature.c_parameter(signature, code, type)`: the next
    /// parameter's C type. `code` is 0 void, 1 bool, 2 signed, 3 unsigned,
    /// 4 float, 5 pointer or 6 a record by value; the scalar codes take their
    /// width from `type`, and 6 names the record `type` is.
    pub(super) fn ll_signature_c_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(ty) = self.c_type(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        match self.pending_signature(arguments.first()) {
            Some(signature) => {
                signature.c_parameters.push(ty);
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.signature.c_result(signature, code, type)`: the result's C
    /// type, coded as for a parameter.
    pub(super) fn ll_signature_c_result(&mut self, arguments: &[u64]) -> u64 {
        let Some(ty) = self.c_type(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        match self.pending_signature(arguments.first()) {
            Some(signature) => {
                signature.c_result = Some(ty);
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.signature.c_variadic(signature)`: the C parameters given so
    /// far are the callee's fixed ones, and the rest this call's variadic
    /// arguments.
    pub(super) fn ll_signature_c_variadic(&mut self, arguments: &[u64]) -> u64 {
        match self.pending_signature(arguments.first()) {
            Some(signature) => {
                signature.c_variadic = Some(signature.c_parameters.len());
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.signature.export(signature)`: the function is exported from
    /// the program under its own name, for C code linked with it to call.
    pub(super) fn ll_signature_export(&mut self, arguments: &[u64]) -> u64 {
        match self.pending_signature(arguments.first()) {
            Some(signature) => {
                signature.exported = true;
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.signature.c_syscall_byte(signature, byte)`: the next byte of
    /// the `man 2` name of the system call a foreign import makes.
    pub(super) fn ll_signature_c_syscall_byte(&mut self, arguments: &[u64]) -> u64 {
        let Some(byte) = signed(arguments.get(1)).and_then(|byte| u8::try_from(byte).ok()) else {
            return u64::MAX;
        };
        match self.pending_signature(arguments.first()) {
            Some(signature) => {
                signature.c_syscall.get_or_insert_with(Vec::new).push(byte);
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.module.attribute_byte(module, byte)`: one byte of the next
    /// module attribute's value.
    pub(super) fn ll_module_attribute_byte(&mut self, arguments: &[u64]) -> u64 {
        match signed(arguments.get(1)).and_then(|byte| u8::try_from(byte).ok()) {
            Some(byte) => {
                self.kir_attribute_bytes.push(byte);
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.module.attribute(module, kind)`: adds the bytes given so far
    /// as a `link.library` (kind 0), `link.flag` (kind 1), `link.c_source`
    /// (kind 2: a C source's path, then its compile flags, tab-separated),
    /// `link.resource` (kind 3: a file's path, then where the build copies it,
    /// tab-separated), `ridl.surface` (kind 4: the module's `.ridl` text) or
    /// `ridl.entry` (kind 5: a RIDL path, the KIR function implementing it and
    /// its value shape, tab-separated), `ridl.module` (kind 6: the name the
    /// surface goes by), `ridl.type` (kind 7: a type the surface spells,
    /// then the function its descriptor destroys a value with, or nothing,
    /// tab-separated), `ridl.import` (kind 8: a module imported through RIDL,
    /// then the `.ridl` text of what the program declares from it,
    /// tab-separated), `ridl.import.entry` (kind 9: that module, then an
    /// entry as `ridl.entry` writes it) or `ridl.contract` (kind 10: the
    /// `.ridl` text the module publishes, importing the types of other
    /// modules instead of declaring them) attribute, or one of the trait
    /// object attributes `ridl.witness.trait`, `.table`, `.import`, `.call`
    /// and `.default` (kinds 11 to 15, as `klf-frontend-rt` reads them), or
    /// `program.export` (kind 16: a function an executable exports under its
    /// own name, as C programs do for the modules they load), or
    /// `klf.debug.name` (kind 17: the suffix a generated item's name carries,
    /// then what it is generated for, tab-separated, which the debug
    /// information shows in place of the suffix).
    pub(super) fn ll_module_attribute(&mut self, arguments: &[u64]) -> u64 {
        let bytes = std::mem::take(&mut self.kir_attribute_bytes);
        let name = match arguments.get(1) {
            Some(0) => "link.library",
            Some(1) => "link.flag",
            Some(2) => "link.c_source",
            Some(3) => "link.resource",
            Some(4) => "ridl.surface",
            Some(5) => "ridl.entry",
            Some(6) => "ridl.module",
            Some(7) => "ridl.type",
            Some(8) => "ridl.import",
            Some(9) => "ridl.import.entry",
            Some(10) => "ridl.contract",
            Some(11) => "ridl.witness.trait",
            Some(12) => "ridl.witness.table",
            Some(13) => "ridl.witness.import",
            Some(14) => "ridl.witness.call",
            Some(15) => "ridl.witness.default",
            Some(16) => "program.export",
            Some(17) => "klf.debug.name",
            _ => return u64::MAX,
        };
        let Ok(value) = String::from_utf8(bytes) else {
            return u64::MAX;
        };
        let module = arguments
            .first()
            .and_then(|module| usize::try_from(*module).ok());
        match module
            .and_then(|module| self.modules.get_mut(module))
            .and_then(Option::as_mut)
        {
            Some(builder) => {
                builder.add_attribute(klf_kir::ModuleAttribute {
                    name: name.to_owned(),
                    value: Some(value),
                });
                0
            }
            None => u64::MAX,
        }
    }

    /// `klf.kir.signature.c_parameter(signature, code, type)`: the next
    /// parameter's C type. `code` is 0 void, 1 bool, 2 signed, 3 unsigned,
    /// 4 float, 5 pointer or 6 a record by value; the scalar codes take their
    /// width from `type`, and 6 names the record `type` is.
    fn pending_signature(&mut self, id: Option<&u64>) -> Option<&mut super::PendingSignature> {
        let index = usize::try_from(*id?).ok()?;
        self.kir_signatures.get_mut(index)?.as_mut()
    }

    fn c_type(&self, code: Option<&u64>, ty: Option<&u64>) -> Option<klf_kir::CType> {
        let ty = self.kir_type(ty.copied());
        let bits = match ty {
            Some(Type::Integer(bits) | Type::Float(bits)) => Some(bits),
            _ => None,
        };
        Some(match code? {
            0 => klf_kir::CType::Void,
            1 => klf_kir::CType::Bool,
            2 => klf_kir::CType::Signed(bits?),
            3 => klf_kir::CType::Unsigned(bits?),
            4 => klf_kir::CType::Float(bits?),
            5 => klf_kir::CType::Pointer,
            6 => match ty? {
                Type::Nominal(name, arguments) if arguments.is_empty() => {
                    klf_kir::CType::Value(name)
                }
                _ => return None,
            },
            _ => return None,
        })
    }

    pub(super) fn ll_signature_define(&mut self, arguments: &[u64]) -> u64 {
        self.ll_signature_commit(arguments, false)
    }

    /// Commits an accumulated signature. A foreign commit is a declaration: it
    /// carries no body until `klf.kir.function.body` opens one.
    pub(super) fn ll_signature_commit(&mut self, arguments: &[u64], foreign: bool) -> u64 {
        let Some(index) = arguments
            .first()
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(mut signature) = self.kir_signatures.get_mut(index).and_then(Option::take) else {
            return u64::MAX;
        };
        if let Some(bytes) = signature.name_bytes.take() {
            let Ok(name) = String::from_utf8(bytes) else {
                return u64::MAX;
            };
            signature.name = Name::new(name);
        }
        let parameters = signature
            .parameters
            .iter()
            .enumerate()
            .map(|(position, ty)| (Name::new(format!("arg{position}")), ty.clone()))
            .collect::<Vec<_>>();
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        let mut builder = klf_services::FunctionBuilder::new(
            signature.name.clone(),
            &parameters,
            signature.result.clone(),
            signature.span,
        );
        builder.set_foreign(foreign);
        if signature.exported {
            builder.set_linkage(klf_kir::Linkage::Exported);
        }
        if signature.c_result.is_some() || !signature.c_parameters.is_empty() {
            builder.set_c_prototype(klf_kir::CPrototype {
                parameters: signature.c_parameters,
                result: signature.c_result.unwrap_or(klf_kir::CType::Void),
                variadic: signature.c_variadic,
                syscall: signature
                    .c_syscall
                    .map(|bytes| Name::new(String::from_utf8_lossy(&bytes))),
            });
        }
        self.functions.push(Some(BuilderFunction {
            module: signature.module,
            name: signature.name,
            builder,
            result_type: signature.result,
            parameters: parameters.iter().map(|(name, _)| name.clone()).collect(),
            parameter_types: signature.parameters,
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    pub(super) fn ll_function_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(position) = signed(arguments.get(1)).and_then(|p| usize::try_from(p).ok()) else {
            return u64::MAX;
        };
        let Some(index) = usize::try_from(function_id).ok() else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get(index) else {
            return u64::MAX;
        };
        let Some(name) = function.parameters.get(position).cloned() else {
            return u64::MAX;
        };
        let Some(ty) = function.parameter_types.get(position).cloned() else {
            return u64::MAX;
        };
        self.kir_new_value(function_id, name, ty)
    }

    pub(super) fn ll_function_result_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.ret(None);
        0
    }
}
