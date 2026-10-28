use super::struct_copy::{
    CopyStep, copy_struct_fields, copyable_struct, field_index, struct_copy_plan_with_variants,
};
use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{StoreMode, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_enum",
        state,
        DefinitionServiceState::type_struct_field_enum,
    );
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_struct",
        state,
        DefinitionServiceState::type_struct_field_struct,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_enum",
        state,
        DefinitionServiceState::value_struct_set_enum,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_struct",
        state,
        DefinitionServiceState::value_struct_set_struct,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_enum",
        state,
        DefinitionServiceState::value_struct_get_enum,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_struct",
        state,
        DefinitionServiceState::value_struct_get_struct,
    );
}

/// What `value_struct_set_enum` emits once every handle checks out.
struct EnumFieldPlan {
    struct_id: u64,
    struct_index: usize,
    field: usize,
    value_id: u64,
    variant: u32,
    copy: PayloadCopy,
}

/// How a variant's payload moves from the enum value into the struct field.
enum PayloadCopy {
    None,
    Scalar,
    Struct(Vec<Vec<CopyStep>>),
    Array(usize),
}

impl DefinitionServiceState {
    fn type_struct_field_enum(&mut self, arguments: &[u64]) -> u64 {
        let Some(struct_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(enum_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(2), arguments.get(3)) else {
            return u64::MAX;
        };
        let Ok(struct_index) = usize::try_from(struct_id) else {
            return u64::MAX;
        };
        let Ok(enum_index) = usize::try_from(enum_id) else {
            return u64::MAX;
        };
        let Some((struct_module, struct_declaration)) = self.types.get(struct_index) else {
            return u64::MAX;
        };
        let Some((enum_module, enum_declaration)) = self.types.get(enum_index) else {
            return u64::MAX;
        };
        if *struct_module != *enum_module
            || !struct_declaration.variants.is_empty()
            || enum_declaration.variants.is_empty()
        {
            return u64::MAX;
        }
        let enum_name = enum_declaration.name.clone();
        let Some((_, declaration)) = self.types.get_mut(struct_index) else {
            return u64::MAX;
        };
        if declaration.fields.iter().any(|field| field.name == name) {
            return u64::MAX;
        }
        declaration.fields.push(klf_kir::Field::new(
            name,
            Type::Nominal(enum_name, Vec::new()),
        ));
        0
    }

    fn type_struct_field_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(struct_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(inner_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(2), arguments.get(3)) else {
            return u64::MAX;
        };
        let Ok(struct_index) = usize::try_from(struct_id) else {
            return u64::MAX;
        };
        let Ok(inner_index) = usize::try_from(inner_id) else {
            return u64::MAX;
        };
        if struct_index == inner_index {
            return u64::MAX;
        }
        let Some((struct_module, struct_declaration)) = self.types.get(struct_index).cloned()
        else {
            return u64::MAX;
        };
        let Some((inner_module, inner_declaration)) = self.types.get(inner_index).cloned() else {
            return u64::MAX;
        };
        if struct_module != inner_module
            || !struct_declaration.variants.is_empty()
            || !copyable_struct(&self.types, inner_module, &inner_declaration)
        {
            return u64::MAX;
        }
        let Some((_, declaration)) = self.types.get_mut(struct_index) else {
            return u64::MAX;
        };
        if declaration.fields.iter().any(|field| field.name == name) {
            return u64::MAX;
        }
        declaration.fields.push(klf_kir::Field::new(
            name,
            Type::Nominal(inner_declaration.name, Vec::new()),
        ));
        0
    }

    fn value_struct_set_enum(&mut self, arguments: &[u64]) -> u64 {
        let (Some(function_id), Some(struct_id), Some(field), Some(value_id)) = (
            arguments.first().copied(),
            arguments.get(1).copied(),
            field_index(arguments.get(2)),
            arguments.get(3).copied(),
        ) else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Some(plan) = self.plan_enum_field(struct_id, field, value_id) else {
            return u64::MAX;
        };
        self.emit_enum_field(function_id, &base, &value, &plan)
    }

    /// Checks that the field accepts this enum value and works out how its
    /// payload has to be copied.
    fn plan_enum_field(
        &self,
        struct_id: u64,
        field: usize,
        value_id: u64,
    ) -> Option<EnumFieldPlan> {
        let struct_index = self
            .struct_values
            .iter()
            .position(|(id, _, _)| *id == struct_id)?;
        let type_id = self.struct_values[struct_index].1;
        if self.struct_values[struct_index].2.get(field).copied() != Some(false) {
            return None;
        }
        let enum_type_id = self
            .enum_values
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, type_id)| *type_id)?;
        let type_index = usize::try_from(type_id).ok()?;
        let enum_type_index = usize::try_from(enum_type_id).ok()?;
        let field_type = self
            .types
            .get(type_index)
            .and_then(|(_, declaration)| declaration.fields.get(field))
            .map(|field| field.ty.clone())?;
        let enum_declaration = self
            .types
            .get(enum_type_index)
            .map(|(_, declaration)| declaration)?;
        if field_type != Type::Nominal(enum_declaration.name.clone(), Vec::new())
            || enum_declaration.variants.is_empty()
        {
            return None;
        }
        let variant = self
            .enum_variants
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, variant)| *variant)?;
        let copy = self.payload_copy(value_id, enum_type_index, variant)?;
        Some(EnumFieldPlan {
            struct_id,
            struct_index,
            field,
            value_id,
            variant,
            copy,
        })
    }

    /// The copy this variant's payload needs. `None` for a payload shape the
    /// services do not build.
    fn payload_copy(
        &self,
        enum_value_id: u64,
        enum_type_index: usize,
        variant: u32,
    ) -> Option<PayloadCopy> {
        let (enum_module, declaration) = self.types.get(enum_type_index)?;
        let variant = declaration.variants.get(variant as usize)?;
        match variant.payload.as_slice() {
            [] => Some(PayloadCopy::None),
            [Type::Integer(32 | 64)] => Some(PayloadCopy::Scalar),
            [Type::Array(length, element)]
                if matches!(element.as_ref(), Type::Integer(32 | 64)) =>
            {
                usize::try_from(*length).ok().map(PayloadCopy::Array)
            }
            [Type::Nominal(name, arguments)] if arguments.is_empty() => {
                let payload = self
                    .types
                    .iter()
                    .find(|(module, declaration)| {
                        module == enum_module && declaration.name == *name
                    })
                    .map(|(_, declaration)| declaration.clone())?;
                let source_struct_id = self
                    .enum_struct_payloads
                    .iter()
                    .find(|(value, _)| *value == enum_value_id)
                    .map(|(_, source)| *source)?;
                let variants = self.known_enum_variants(source_struct_id);
                struct_copy_plan_with_variants(&self.types, *enum_module, &payload, &variants)
                    .map(PayloadCopy::Struct)
            }
            _ => None,
        }
    }

    /// Emits the field address, the payload copy and the tag write.
    fn emit_enum_field(
        &mut self,
        function_id: u64,
        base: &klf_core::Name,
        value: &klf_core::Name,
        plan: &EnumFieldPlan,
    ) -> u64 {
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let Ok(field) = u32::try_from(plan.field) else {
            return u64::MAX;
        };
        let variant = plan.variant;
        let address = function.builder.field_addr(base.clone(), field);
        let source = function.builder.enum_payload_addr(value.clone(), variant);
        let destination = function.builder.enum_payload_addr(address.clone(), variant);
        match &plan.copy {
            PayloadCopy::None => {}
            PayloadCopy::Scalar => {
                let payload = function.builder.load(source);
                function
                    .builder
                    .store(StoreMode::Init, payload, destination);
            }
            PayloadCopy::Struct(fields) => {
                copy_struct_fields(&mut function.builder, fields, &source, &destination);
            }
            PayloadCopy::Array(length) => {
                for index in 0..*length {
                    let index_name = function
                        .builder
                        .const_int(Type::Integer(64), i128::try_from(index).unwrap_or(0));
                    let source_field = function
                        .builder
                        .elem_addr(source.clone(), index_name.clone());
                    let payload = function.builder.load(source_field);
                    let destination_field =
                        function.builder.elem_addr(destination.clone(), index_name);
                    function
                        .builder
                        .store(StoreMode::Init, payload, destination_field);
                }
            }
        }
        function.builder.enum_inject(address, variant);
        self.struct_values[plan.struct_index].2[plan.field] = true;
        self.struct_enum_variants
            .push((plan.struct_id, plan.field, variant));
        self.struct_known_enum_variants.push((
            plan.struct_id,
            vec![CopyStep::Field(field)],
            variant,
        ));
        if let Some(source_struct_id) = self
            .enum_struct_payloads
            .iter()
            .find(|(value, _)| *value == plan.value_id)
            .map(|(_, source)| *source)
        {
            for (path, nested_variant) in self.known_enum_variants(source_struct_id) {
                let mut destination_path =
                    vec![CopyStep::Field(field), CopyStep::EnumPayload(variant)];
                destination_path.extend(path);
                self.struct_known_enum_variants.push((
                    plan.struct_id,
                    destination_path,
                    nested_variant,
                ));
            }
        }
        0
    }

    fn value_struct_set_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = field_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Some(struct_index) = self
            .struct_values
            .iter()
            .position(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        let Some((source_type_id, source_initialized)) = self
            .struct_values
            .iter()
            .find(|(id, _, _)| *id == value_id)
            .map(|(_, type_id, initialized)| (*type_id, initialized.clone()))
        else {
            return u64::MAX;
        };
        if self.struct_values[struct_index].2.get(field).copied() != Some(false)
            || !source_initialized.iter().all(|initialized| *initialized)
        {
            return u64::MAX;
        }
        let type_id = self.struct_values[struct_index].1;
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Ok(source_type_index) = usize::try_from(source_type_id) else {
            return u64::MAX;
        };
        let Some((outer_module, outer_declaration)) = self.types.get(type_index).cloned() else {
            return u64::MAX;
        };
        let Some((source_module, source_declaration)) = self.types.get(source_type_index).cloned()
        else {
            return u64::MAX;
        };
        let Some(Type::Nominal(field_name, arguments)) = outer_declaration
            .fields
            .get(field)
            .map(|field| field.ty.clone())
        else {
            return u64::MAX;
        };
        if outer_module != source_module
            || !arguments.is_empty()
            || field_name != source_declaration.name
            || source_initialized.len() != source_declaration.fields.len()
        {
            return u64::MAX;
        }
        let variants = self.known_enum_variants(value_id);
        let Some(plan) = struct_copy_plan_with_variants(
            &self.types,
            source_module,
            &source_declaration,
            &variants,
        ) else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let Ok(field) = u32::try_from(field) else {
            return u64::MAX;
        };
        let destination = function.builder.field_addr(base, field);
        copy_struct_fields(&mut function.builder, &plan, &value, &destination);
        self.struct_values[struct_index].2[field as usize] = true;
        let prefix = vec![CopyStep::Field(field)];
        for (path, variant) in variants {
            let mut destination_path = prefix.clone();
            destination_path.extend(path);
            self.struct_known_enum_variants
                .push((struct_id, destination_path, variant));
        }
        0
    }

    pub(super) fn known_enum_variants(&self, struct_id: u64) -> Vec<(Vec<CopyStep>, u32)> {
        self.struct_known_enum_variants
            .iter()
            .filter(|(owner, _, _)| *owner == struct_id)
            .map(|(_, path, variant)| (path.clone(), *variant))
            .collect()
    }

    fn known_enum_variants_in_field(
        &self,
        struct_id: u64,
        field: u32,
    ) -> Vec<(Vec<CopyStep>, u32)> {
        let prefix = CopyStep::Field(field);
        self.known_enum_variants(struct_id)
            .into_iter()
            .filter_map(|(path, variant)| {
                (path.first() == Some(&prefix)).then(|| (path[1..].to_vec(), variant))
            })
            .collect()
    }

    fn value_struct_get_enum(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = field_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some((_, type_id, initialized)) = self
            .struct_values
            .iter()
            .find(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        if initialized.get(field).copied() != Some(true) {
            return u64::MAX;
        }
        let Ok(type_index) = usize::try_from(*type_id) else {
            return u64::MAX;
        };
        let Some(Type::Nominal(name, arguments)) = self
            .types
            .get(type_index)
            .and_then(|(_, declaration)| declaration.fields.get(field))
            .map(|field| &field.ty)
        else {
            return u64::MAX;
        };
        if !arguments.is_empty() {
            return u64::MAX;
        }
        let Some(enum_type_id) = self
            .types
            .iter()
            .position(|(_, declaration)| {
                declaration.name == *name && !declaration.variants.is_empty()
            })
            .and_then(|index| u64::try_from(index).ok())
        else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let field_index = field;
        let Ok(field) = u32::try_from(field_index) else {
            return u64::MAX;
        };
        let name = function.builder.field_addr(base, field);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        self.enum_values.push((id, enum_type_id));
        if let Some((_, _, variant)) = self
            .struct_enum_variants
            .iter()
            .find(|(owner, field, _)| *owner == struct_id && *field == field_index)
        {
            self.enum_variants.push((id, *variant));
        }
        id
    }

    fn value_struct_get_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = field_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some((_, type_id, initialized)) = self
            .struct_values
            .iter()
            .find(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        if initialized.get(field).copied() != Some(true) {
            return u64::MAX;
        }
        let Ok(type_index) = usize::try_from(*type_id) else {
            return u64::MAX;
        };
        let Some((module, declaration)) = self.types.get(type_index).cloned() else {
            return u64::MAX;
        };
        let Some(Type::Nominal(name, arguments)) =
            declaration.fields.get(field).map(|field| field.ty.clone())
        else {
            return u64::MAX;
        };
        if !arguments.is_empty() {
            return u64::MAX;
        }
        let Some((nested_type_id, nested_declaration)) = self
            .types
            .iter()
            .enumerate()
            .find(|(_, (owner, declaration))| *owner == module && declaration.name == name)
            .and_then(|(index, (_, declaration))| {
                Some((u64::try_from(index).ok()?, declaration.clone()))
            })
        else {
            return u64::MAX;
        };
        if !copyable_struct(&self.types, module, &nested_declaration) {
            return u64::MAX;
        }
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let Ok(field) = u32::try_from(field) else {
            return u64::MAX;
        };
        let name = function.builder.field_addr(base, field);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        self.struct_values.push((
            id,
            nested_type_id,
            vec![true; nested_declaration.fields.len()],
        ));
        for (path, variant) in self.known_enum_variants_in_field(struct_id, field) {
            self.struct_known_enum_variants.push((id, path, variant));
        }
        id
    }
}
