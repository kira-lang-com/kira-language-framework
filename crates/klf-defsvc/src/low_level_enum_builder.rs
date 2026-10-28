//! Enum types and their operations for the general low-level KIR builder.
//!
//! The enum counterpart of the record services: a definition declares an enum
//! by handle and adds its cases one at a time, each with one payload type or
//! none, then builds values in memory. An enum value is its storage address:
//! the payload is written through `enum.payload.addr`, `enum.inject` sets the
//! tag, and a `switch.enum` over the case targets accumulated by
//! `klf.kir.switch.case` dispatches on it.

use super::DefinitionServiceState;
use super::low_level_builder::signed;
use klf_core::Name;
use klf_kir::{
    BlockTarget, ConvertKind, EnumVariant, LayoutRule, Policy, Type, TypeDeclaration, TypeKind,
};
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
    service!(state, capabilities, "klfsvc_kir_type_enum", ll_type_enum);
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_enum_open",
        ll_type_enum_open
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_enum_case_numbered",
        ll_type_enum_case_numbered
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_enum_named",
        ll_type_enum_named
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_enum_case",
        ll_type_enum_case
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_enum_inject",
        ll_value_enum_inject
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_enum_payload_address",
        ll_value_enum_payload_address
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_enum_tag",
        ll_value_enum_tag
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_switch_enum",
        ll_switch_enum
    );
}

impl DefinitionServiceState {
    fn ll_type_enum_open(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = self.kir_enum_index(arguments.first().copied()) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(index) else {
            return u64::MAX;
        };
        declaration.open = true;
        0
    }

    /// `(module, name start, name end) -> type`: an enum with no cases yet,
    /// named by a span of the unit's source.
    fn ll_type_enum(&mut self, arguments: &[u64]) -> u64 {
        self.ll_type_enum_with_name(arguments, None)
    }

    fn ll_type_enum_named(&mut self, arguments: &[u64]) -> u64 {
        let Some(suffix) = signed(arguments.get(3)) else {
            return u64::MAX;
        };
        self.ll_type_enum_with_name(arguments, Some(suffix))
    }

    fn ll_type_enum_with_name(&mut self, arguments: &[u64], suffix: Option<i64>) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((source_name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
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
        let name = match suffix {
            Some(suffix) => Name::new(format!("{source_name}_G{suffix}")),
            None => source_name,
        };
        let mut declaration =
            TypeDeclaration::empty(name.clone(), LayoutRule::Ridl, Policy::trivial(), span);
        declaration.kind = TypeKind::Enum;
        let index = self.types.len();
        self.types.push(module, declaration);
        let id = self.kir_new_type(Type::Nominal(name, Vec::new()));
        if id != u64::MAX {
            self.kir_records.entry(id).or_insert(index);
        }
        id
    }

    /// `(enum, name start, name end, payload)`: the next case. A unit payload
    /// type declares a case that carries nothing.
    fn ll_type_enum_case(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = self.kir_enum_index(arguments.first().copied()) else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        self.ll_append_enum_case(index, name, arguments.get(3).copied())
    }

    fn ll_type_enum_case_numbered(&mut self, arguments: &[u64]) -> u64 {
        let Some(index) = self.kir_enum_index(arguments.first().copied()) else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(number) = signed(arguments.get(3)) else {
            return u64::MAX;
        };
        self.ll_append_enum_case(
            index,
            Name::new(format!("{name}#{number}")),
            arguments.get(4).copied(),
        )
    }

    fn ll_append_enum_case(&mut self, index: usize, name: Name, payload: Option<u64>) -> u64 {
        let Some(payload) = self.kir_type(payload) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(index) else {
            return u64::MAX;
        };
        if declaration.variants.iter().any(|case| case.name == name) {
            return u64::MAX;
        }
        let payload = if payload == Type::Unit {
            Vec::new()
        } else {
            vec![payload]
        };
        declaration.variants.push(EnumVariant { name, payload });
        0
    }

    /// `(function, address, case)`: sets the tag of the enum at `address`,
    /// after which the whole value counts as initialized.
    fn ll_value_enum_inject(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((index, address, _, _)) = self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(case) = signed(arguments.get(2)).and_then(|case| u32::try_from(case).ok()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.enum_inject(address, case);
        if let Some(Some(address)) = self.addresses.get_mut(index) {
            address.initialized = true;
        }
        0
    }

    /// `(function, address, case, payload type) -> address`: where the case's
    /// payload lives inside the enum at `address`.
    fn ll_value_enum_payload_address(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, base, _, initialized)) =
            self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(case) = signed(arguments.get(2)).and_then(|case| u32::try_from(case).ok()) else {
            return u64::MAX;
        };
        let Some(payload) = self.kir_type(arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.enum_payload_addr(base, case);
        self.kir_new_address(function_id, name, payload, initialized)
    }

    /// `(function, address) -> value`: the case index of the enum at
    /// `address`, as an `i64`.
    fn ll_value_enum_tag(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, address, _, _)) = self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let tag = function.builder.enum_tag(address);
        let name = function
            .builder
            .convert(ConvertKind::ZeroExtend, tag, Type::Integer(64));
        self.kir_new_value(function_id, name, Type::Integer(64))
    }

    /// `(function, address)`: branches on the case of the enum at `address`
    /// to the targets `klf.kir.switch.case` accumulated, one per case index.
    /// There is no default: a `switch.enum` names every case it can reach.
    fn ll_switch_enum(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((_, address, _, _)) = self.kir_address(function_id, arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let mut cases = Vec::new();
        let mut kept = Vec::new();
        for (owner, case, block, edge) in self.kir_switch_cases.drain(..) {
            if owner == function_id {
                cases.push((case, block, edge));
            } else {
                kept.push((owner, case, block, edge));
            }
        }
        self.kir_switch_cases = kept;
        let Some(cases) = cases
            .into_iter()
            .map(|(case, block, arguments)| {
                let case = u32::try_from(case).ok()?;
                Some((case, BlockTarget { block, arguments }))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.switch_enum(address, cases, None);
        0
    }

    fn kir_enum_index(&self, handle: Option<u64>) -> Option<usize> {
        let handle = handle?;
        let index = self.kir_records.get(&handle).copied()?;
        let (_, declaration) = self.types.get(index)?;
        (declaration.kind == TypeKind::Enum).then_some(index)
    }
}
