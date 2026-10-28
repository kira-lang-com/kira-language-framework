//! Record types, module globals and null pointers for the general low-level
//! KIR builder. These are the module-scope declarations a definition composes
//! by handle, kept apart from the per-function value and control-flow surface.
//! Enum types are their sibling module, registered from here.

use super::DefinitionServiceState;
use super::low_level_builder::signed;
use klf_core::Name;
use klf_kir::{ConvertKind, Field, GlobalInit, LayoutRule, Policy, Type, TypeDeclaration};
use std::sync::{Arc, Mutex};

macro_rules! service {
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
    super::low_level_enum_builder::register(state, capabilities);
    super::low_level_float_builder::register(state, capabilities);
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_record",
        ll_type_record
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_record_named",
        ll_type_record_named
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_global_new_named",
        ll_global_new_named
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_record_field",
        ll_type_record_field
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_record_union",
        ll_type_record_union
    );
    service!(state, capabilities, "klfsvc_kir_value_null", ll_value_null);
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_text_byte",
        ll_value_text_byte
    );
    service!(state, capabilities, "klfsvc_kir_value_text", ll_value_text);
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_const_string",
        ll_value_const_string
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_const_string_length",
        ll_value_const_string_length
    );
    service!(state, capabilities, "klfsvc_kir_global_new", ll_global_new);
    service!(
        state,
        capabilities,
        "klfsvc_kir_global_byte",
        ll_global_byte
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_global_integer",
        ll_global_integer
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_global_address",
        ll_value_global_address
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_global_anonymous",
        ll_global_anonymous
    );
}

impl DefinitionServiceState {
    fn ll_value_text_byte(&mut self, arguments: &[u64]) -> u64 {
        let (Some(function), Some(byte)) = (
            arguments.first().copied(),
            arguments.get(1).and_then(|byte| u8::try_from(*byte).ok()),
        ) else {
            return u64::MAX;
        };
        if self.kir_function_mut(function).is_none() {
            return u64::MAX;
        }
        self.kir_text_bytes.entry(function).or_default().push(byte);
        0
    }

    fn ll_value_text(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let bytes = self.kir_text_bytes.remove(&function_id).unwrap_or_default();
        let Ok(value) = String::from_utf8(bytes) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let (address, length) = function.builder.const_string(value);
        let address_id = self.kir_new_value(function_id, address, Type::Pointer);
        let length_id = self.kir_new_value(function_id, length, Type::Integer(64));
        self.kir_second_results
            .insert((function_id, address_id), length_id);
        address_id
    }

    fn ll_value_const_string(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(raw) = self.source_text(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(value) = decode_string_literal(&raw) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let (address, length) = function.builder.const_string(value);
        let address_id = self.kir_new_value(function_id, address, Type::Pointer);
        let length_id = self.kir_new_value(function_id, length, Type::Integer(64));
        if address_id == u64::MAX || length_id == u64::MAX {
            return u64::MAX;
        }
        self.kir_second_results
            .insert((function_id, address_id), length_id);
        address_id
    }

    fn ll_value_const_string_length(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        self.kir_second_results
            .get(&(function_id, value_id))
            .copied()
            .unwrap_or(u64::MAX)
    }

    fn ll_type_record(&mut self, arguments: &[u64]) -> u64 {
        self.ll_type_record_with_suffix(arguments, None)
    }

    /// A record whose KIR name is the source name plus `#suffix`, for a type
    /// private to its unit (a C++ anonymous namespace): two units may each
    /// declare one under the same source name.
    fn ll_type_record_named(&mut self, arguments: &[u64]) -> u64 {
        let Some(suffix) = arguments.get(4).copied() else {
            return u64::MAX;
        };
        self.ll_type_record_with_suffix(arguments, Some(suffix))
    }

    fn ll_type_record_with_suffix(&mut self, arguments: &[u64], suffix: Option<u64>) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let name = suffix.map_or(name.clone(), |suffix| {
            Name::new(format!("{}#{suffix}", name.as_str()))
        });
        let layout = match signed(arguments.get(3)) {
            Some(0) => LayoutRule::Ridl,
            Some(1) => LayoutRule::C,
            _ => return u64::MAX,
        };
        if usize::try_from(module)
            .ok()
            .and_then(|index| self.modules.get(index))
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let record = self.types.len();
        self.types.push(
            module,
            TypeDeclaration::empty(name.clone(), layout, Policy::trivial(), span),
        );
        let id = self.kir_new_type(Type::Nominal(name, Vec::new()));
        if id != u64::MAX {
            self.kir_records.entry(id).or_insert(record);
        }
        id
    }

    fn ll_type_record_field(&mut self, arguments: &[u64]) -> u64 {
        let Some(record) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(index) = self.kir_records.get(&record).copied() else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(index) else {
            return u64::MAX;
        };
        let overlaid = match declaration.kind {
            klf_kir::TypeKind::Struct => false,
            klf_kir::TypeKind::Explicit => true,
            klf_kir::TypeKind::Enum => return u64::MAX,
        };
        // Fields are addressed by index; a repeated source name (a template
        // pack declares one member per element from one spelling) gets a
        // numbered one.
        let mut unique = name.clone();
        let mut next = 1;
        while declaration.fields.iter().any(|field| field.name == unique) {
            unique = format!("{name}__{next}").into();
            next += 1;
        }
        let mut field = Field::new(unique, ty);
        if overlaid {
            field.offset = Some(0);
        }
        declaration.fields.push(field);
        0
    }

    /// `klf.kir.type.record_union(record)`: the record's fields, added before
    /// or after, all start at offset 0, as a C union's do.
    fn ll_type_record_union(&mut self, arguments: &[u64]) -> u64 {
        let Some(record) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(index) = self.kir_records.get(&record).copied() else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(index) else {
            return u64::MAX;
        };
        declaration.kind = klf_kir::TypeKind::Explicit;
        for field in &mut declaration.fields {
            field.offset = Some(0);
        }
        0
    }

    fn ll_value_null(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let null = function.builder.const_null();
        let name = function
            .builder
            .convert(ConvertKind::Bitcast, null, ty.clone());
        self.kir_new_value(function_id, name, ty)
    }

    fn ll_global_new(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let mutable = signed(arguments.get(4));
        self.push_global(module, name, ty, mutable)
    }

    /// A global whose KIR name is the source name plus `#suffix`, for storage
    /// private to its unit.
    fn ll_global_new_named(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(suffix) = arguments.get(5).copied() else {
            return u64::MAX;
        };
        let mutable = signed(arguments.get(4));
        let name = Name::new(format!("{}#{suffix}", name.as_str()));
        self.push_global(module, name, ty, mutable)
    }

    /// A global the source has no name for, such as the storage behind a
    /// literal. The generated name carries a dot, which `source_name` rejects,
    /// so it can never collide with a name taken from a span, and it carries
    /// the unit so two units of one program do not name the same storage.
    fn ll_global_anonymous(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let mutable = signed(arguments.get(2));
        let name = Name::new(format!(
            "anon.{}.{}",
            self.unit.as_u32(),
            self.kir_globals.len()
        ));
        self.push_global(module, name, ty, mutable)
    }

    /// `mutable` is 0 for constant storage, 1 for mutable and 2 for mutable
    /// storage each thread has its own copy of.
    fn push_global(&mut self, module: u64, name: Name, ty: Type, mutable: Option<i64>) -> u64 {
        let thread_local = mutable == Some(2);
        let mutable = mutable == Some(1) || thread_local;
        if usize::try_from(module)
            .ok()
            .and_then(|index| self.modules.get(index))
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let id = u64::try_from(self.kir_globals.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.kir_globals.push(Some(super::PendingGlobal {
                module,
                name,
                ty,
                mutable,
                thread_local,
                init: GlobalInit::Zero,
            }));
        }
        id
    }

    fn ll_global_byte(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = arguments
            .first()
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(byte) = signed(arguments.get(1)).and_then(|b| u8::try_from(b).ok()) else {
            return u64::MAX;
        };
        let Some(Some(global)) = self.kir_globals.get_mut(index) else {
            return u64::MAX;
        };
        match &mut global.init {
            GlobalInit::Bytes(bytes) => bytes.push(byte),
            _ => global.init = GlobalInit::Bytes(vec![byte]),
        }
        0
    }

    fn ll_global_integer(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = arguments
            .first()
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(value) = signed(arguments.get(1)) else {
            return u64::MAX;
        };
        let Some(Some(global)) = self.kir_globals.get_mut(index) else {
            return u64::MAX;
        };
        global.init = GlobalInit::Int(i128::from(value));
        0
    }

    /// The address of a global. `global.addr` is typed as an untyped pointer,
    /// so the bitcast is what makes the result usable as memory of the global's
    /// own type.
    fn ll_value_global_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(1)
            .copied()
            .and_then(|id| usize::try_from(id).ok())
        else {
            return u64::MAX;
        };
        let Some(Some(global)) = self.kir_globals.get(index) else {
            return u64::MAX;
        };
        let (name, element) = (global.name.clone(), global.ty.clone());
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let raw = function.builder.global_address(name);
        let address = function.builder.convert(
            ConvertKind::Bitcast,
            raw,
            Type::Address(Box::new(element.clone())),
        );
        self.kir_new_address(function_id, address, element, true)
    }
}

fn decode_string_literal(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut value = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        match characters.next()? {
            'n' => value.push('\n'),
            't' => value.push('\t'),
            'r' => value.push('\r'),
            '0' => value.push('\0'),
            '\\' => value.push('\\'),
            '"' => value.push('"'),
            'u' => {
                if characters.next()? != '{' {
                    return None;
                }
                let mut hex = String::new();
                for character in characters.by_ref() {
                    if character == '}' {
                        break;
                    }
                    hex.push(character);
                }
                value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(value)
}
