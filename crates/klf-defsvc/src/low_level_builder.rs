//! General low-level KIR builder services.
//!
//! The other builder modules are shaped around Kira's value model (structs,
//! enums, pairs) and are monomorphized per type. These services are
//! language-agnostic: a definition composes an arbitrary KIR type through a
//! handle and then builds typed constants, stack slots, loads and stores,
//! field and element addresses, conversions, and binary and comparison
//! operators against it. No hosted language's type mapping lives here.

use super::{BuilderAddress, BuilderFunction, BuilderValue, DefinitionServiceState};
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, IntegerPredicate, Overflow, StoreMode, Type};
use std::sync::{Arc, Mutex};

macro_rules! service {
    ($state:expr, $capabilities:expr, $(($name:literal, $method:ident)),+ $(,)?) => {
        $( service!($state, $capabilities, $name, $method); )+
    };
    ($state:expr, $capabilities:expr, $name:literal, $method:ident) => {{
        let shared = Arc::clone($state);
        $capabilities.register($name, move |arguments| {
            shared
                .lock()
                .map(|mut state| state.$method(arguments))
                .unwrap_or(u64::MAX)
        });
    }};
}

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    service!(
        state,
        capabilities,
        ("klfsvc_kir_type_integer", ll_type_integer),
        ("klfsvc_kir_type_float", ll_type_float),
        ("klfsvc_kir_type_address", ll_type_address),
        ("klfsvc_kir_type_reference", ll_type_reference),
        ("klfsvc_kir_type_unique", ll_type_unique),
        ("klfsvc_kir_type_array", ll_type_array),
        ("klfsvc_kir_type_size", ll_type_size),
        ("klfsvc_kir_type_align", ll_type_align),
        ("klfsvc_kir_type_unit", ll_type_unit),
        ("klfsvc_kir_function_define", ll_function_define),
        ("klfsvc_kir_function_result", ll_function_result),
        ("klfsvc_kir_value_integer", ll_value_integer),
        ("klfsvc_kir_value_zero", ll_value_zero),
        ("klfsvc_kir_value_stack_alloc", ll_value_stack_alloc),
        ("klfsvc_kir_value_store", ll_value_store),
        ("klfsvc_kir_value_load", ll_value_load),
        ("klfsvc_kir_value_field_address", ll_value_field_address),
        ("klfsvc_kir_value_element_address", ll_value_element_address),
        ("klfsvc_kir_value_convert", ll_value_convert),
        ("klfsvc_kir_value_binary", ll_value_binary),
        ("klfsvc_kir_value_compare", ll_value_compare),
        ("klfsvc_kir_signature_new", ll_signature_new),
        ("klfsvc_kir_signature_new_named", ll_signature_new_named),
        ("klfsvc_kir_signature_name_byte", ll_signature_name_byte),
        ("klfsvc_kir_signature_parameter", ll_signature_parameter),
        ("klfsvc_kir_signature_c_parameter", ll_signature_c_parameter),
        ("klfsvc_kir_signature_c_result", ll_signature_c_result),
        ("klfsvc_kir_signature_c_variadic", ll_signature_c_variadic),
        (
            "klfsvc_kir_signature_c_syscall_byte",
            ll_signature_c_syscall_byte
        ),
        ("klfsvc_kir_signature_export", ll_signature_export),
        ("klfsvc_kir_module_attribute_byte", ll_module_attribute_byte),
        ("klfsvc_kir_module_attribute", ll_module_attribute),
        ("klfsvc_kir_signature_define", ll_signature_define),
        ("klfsvc_kir_function_parameter", ll_function_parameter),
        ("klfsvc_kir_function_result_void", ll_function_result_void),
        ("klfsvc_kir_value_stack_dealloc", ll_value_stack_dealloc),
        ("klfsvc_kir_type_pointer", ll_type_pointer),
        ("klfsvc_kir_type_parameter", ll_type_parameter),
        ("klfsvc_kir_type_function", ll_type_function),
        ("klfsvc_kir_type_foreign_function", ll_type_foreign_function),
        ("klfsvc_kir_value_from_address", ll_value_from_address),
        ("klfsvc_kir_address_from_value", ll_address_from_value),
        ("klfsvc_kir_address_cast", ll_address_cast),
    );
}

pub(super) fn signed(value: Option<&u64>) -> Option<i64> {
    value.map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
}

impl DefinitionServiceState {
    pub(super) fn kir_new_type(&mut self, ty: Type) -> u64 {
        let id = u64::try_from(self.kir_types.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.kir_types.push(ty);
        }
        id
    }

    pub(super) fn kir_type(&self, id: Option<u64>) -> Option<Type> {
        usize::try_from(id?)
            .ok()
            .and_then(|index| self.kir_types.get(index))
            .cloned()
    }

    pub(super) fn kir_new_value(&mut self, function: u64, name: Name, ty: Type) -> u64 {
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.values.push(Some(BuilderValue { function, name }));
            self.set_value_type(id, function, ty);
        }
        id
    }

    pub(super) fn set_value_type(&mut self, id: u64, function: u64, ty: Type) {
        let Ok(index) = usize::try_from(id) else {
            return;
        };
        let kind = if let Some(&(_, kind)) = self.recent_value_types.iter().rev().find(|(recent, _)| *recent == ty) {
            kind
        } else {
            let kind = if let Some(&kind) = self.value_type_ids.get(&ty) {
                kind
            } else {
                let kind = u32::try_from(self.value_type_table.len()).unwrap_or(u32::MAX);
                if std::env::var_os("KLF_DEBUG_DEEP_TYPES").is_some() && type_depth(&ty) > 6 {
                    eprintln!("deep value type ({}): {ty}", type_depth(&ty));
                }
                self.value_type_table.push(ty.clone());
                self.value_type_ids.insert(ty.clone(), kind);
                kind
            };
            if self.recent_value_types.len() == 8 {
                self.recent_value_types.remove(0);
            }
            self.recent_value_types.push((ty, kind));
            kind
        };
        if self.kir_value_types.len() <= index {
            self.kir_value_types.resize(index + 1, None);
        }
        self.kir_value_types[index] = Some((function, kind));
    }

    pub(super) fn kir_value_type(&self, function: u64, id: Option<u64>) -> Option<Type> {
        let id = id?;
        self.kir_value_types
            .get(usize::try_from(id).ok()?)?
            .as_ref()
            .filter(|(owner, _)| *owner == function)
            .and_then(|(_, kind)| self.value_type_table.get(usize::try_from(*kind).ok()?))
            .cloned()
    }

    /// Records an address handle. A derived address (a field or element of an
    /// already-live slot) is initialized; a fresh stack slot is not.
    pub(super) fn kir_new_address(
        &mut self,
        function: u64,
        name: Name,
        element: Type,
        initialized: bool,
    ) -> u64 {
        let id = u64::try_from(self.addresses.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.addresses.push(Some(BuilderAddress {
                function,
                name,
                element,
                initialized,
            }));
        }
        id
    }

    /// The name of address `id` of `function`.
    pub(super) fn kir_address_name(&self, function: u64, id: Option<u64>) -> Option<Name> {
        let address = self.addresses.get(usize::try_from(id?).ok()?)?.as_ref()?;
        (address.function == function).then(|| address.name.clone())
    }

    pub(super) fn kir_address(
        &self,
        function: u64,
        id: Option<u64>,
    ) -> Option<(usize, Name, Type, bool)> {
        let index = usize::try_from(id?).ok()?;
        let address = self.addresses.get(index)?.as_ref()?;
        (address.function == function).then(|| {
            (
                index,
                address.name.clone(),
                address.element.clone(),
                address.initialized,
            )
        })
    }

    pub(super) fn kir_function_mut(&mut self, function: u64) -> Option<&mut BuilderFunction> {
        let index = usize::try_from(function).ok()?;
        let function = self.functions.get_mut(index)?.as_mut()?;
        (!function.builder.is_terminated()).then_some(function)
    }

    fn ll_value_integer(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(value) = signed(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.const_int(ty.clone(), i128::from(value));
        self.kir_new_value(function_id, name, ty)
    }

    fn ll_value_zero(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.const_zero(ty.clone());
        self.kir_new_value(function_id, name, ty)
    }

    fn ll_value_stack_alloc(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.stack_alloc(ty.clone());
        self.kir_new_address(function_id, name, ty, false)
    }

    fn ll_value_store(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some((index, address, _, initialized)) =
            self.kir_address(function_id, arguments.get(2).copied())
        else {
            return u64::MAX;
        };
        let mode = if initialized {
            StoreMode::Assign
        } else {
            StoreMode::Init
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.store(mode, value, address);
        if let Some(Some(address)) = self.addresses.get_mut(index) {
            address.initialized = true;
        }
        0
    }

    fn ll_value_load(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, address, element, _)) =
            self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.load(address);
        self.kir_new_value(function_id, name, element)
    }

    fn ll_value_field_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, base, _, _)) = self.kir_address(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(field) = signed(arguments.get(2)).and_then(|field| u32::try_from(field).ok())
        else {
            return u64::MAX;
        };
        let Some(field_type) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.field_addr(base, field);
        self.kir_new_address(function_id, name, field_type, true)
    }

    fn ll_value_element_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, base, _, _)) = self.kir_address(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(index_value) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(element_type) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.elem_addr(base, index_value);
        self.kir_new_address(function_id, name, element_type, true)
    }

    fn ll_value_convert(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let kind = match signed(arguments.get(1)) {
            Some(0) => ConvertKind::Truncate,
            Some(1) => ConvertKind::ZeroExtend,
            Some(2) => ConvertKind::SignExtend,
            Some(3) => ConvertKind::Bitcast,
            Some(4) => ConvertKind::SignedToFloat,
            Some(5) => ConvertKind::UnsignedToFloat,
            Some(6) => ConvertKind::FloatToSigned,
            Some(7) => ConvertKind::FloatToUnsigned,
            Some(8) => ConvertKind::FloatExtend,
            Some(9) => ConvertKind::FloatTruncate,
            _ => return u64::MAX,
        };
        let Some(value) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.convert(kind, value, ty.clone());
        self.kir_new_value(function_id, name, ty)
    }

    fn ll_value_binary(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let operator = match signed(arguments.get(1)) {
            Some(0) => BinaryOperator::Add,
            Some(1) => BinaryOperator::Sub,
            Some(2) => BinaryOperator::Mul,
            Some(3) => BinaryOperator::SignedDiv,
            Some(4) => BinaryOperator::UnsignedDiv,
            Some(5) => BinaryOperator::SignedRem,
            Some(6) => BinaryOperator::UnsignedRem,
            Some(7) => BinaryOperator::And,
            Some(8) => BinaryOperator::Or,
            Some(9) => BinaryOperator::Xor,
            Some(10) => BinaryOperator::ShiftLeft,
            Some(11) => BinaryOperator::LogicalShiftRight,
            Some(12) => BinaryOperator::ArithmeticShiftRight,
            _ => return u64::MAX,
        };
        let overflow = match signed(arguments.get(2)) {
            Some(0) => Overflow::Wrap,
            Some(1) => Overflow::Trap,
            _ => return u64::MAX,
        };
        let is_signed = signed(arguments.get(3)) == Some(1);
        let Some(result_type) = self.kir_value_type(function_id, arguments.get(4).copied()) else {
            return u64::MAX;
        };
        let Some(lhs) = self.value_name(function_id, arguments.get(4).copied()) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, arguments.get(5).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function
            .builder
            .binary(operator, overflow, is_signed, lhs, rhs);
        self.kir_new_value(function_id, name, result_type)
    }

    fn ll_value_compare(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let predicate = match signed(arguments.get(1)) {
            Some(0) => IntegerPredicate::Eq,
            Some(1) => IntegerPredicate::Ne,
            Some(2) => IntegerPredicate::SignedLt,
            Some(3) => IntegerPredicate::SignedLe,
            Some(4) => IntegerPredicate::SignedGt,
            Some(5) => IntegerPredicate::SignedGe,
            Some(6) => IntegerPredicate::UnsignedLt,
            Some(7) => IntegerPredicate::UnsignedLe,
            Some(8) => IntegerPredicate::UnsignedGt,
            Some(9) => IntegerPredicate::UnsignedGe,
            _ => return u64::MAX,
        };
        let Some(lhs) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.compare(predicate, lhs, rhs);
        let id = self.kir_new_value(function_id, name, Type::Integer(1));
        if id != u64::MAX {
            // Register it as a boolean so the shared conditional-branch service
            // accepts it as a condition.
            self.bool_values.insert(id);
        }
        id
    }
    /// Releases a stack slot. The handle stays valid afterwards: a slot reached
    /// by several return paths is deallocated once per path, and every one of
    /// those needs the same handle.
    fn ll_value_stack_dealloc(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(address) = self.kir_address_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.stack_dealloc(address);
        0
    }
}

/// How deeply `ty` nests, for the deep-type report.
fn type_depth(ty: &Type) -> usize {
    match ty {
        Type::Vector(_, inner)
        | Type::Array(_, inner)
        | Type::Reference(inner)
        | Type::Unique(inner)
        | Type::Weak(inner)
        | Type::Address(inner) => 1 + type_depth(inner),
        Type::Tuple(elements) | Type::Nominal(_, elements) => {
            1 + elements.iter().map(type_depth).max().unwrap_or(0)
        }
        Type::Function(function) => {
            1 + function
                .parameters
                .iter()
                .map(|parameter| type_depth(&parameter.ty))
                .chain(std::iter::once(type_depth(&function.result)))
                .max()
                .unwrap_or(0)
        }
        _ => 1,
    }
}
