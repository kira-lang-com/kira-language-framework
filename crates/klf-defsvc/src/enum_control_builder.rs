use super::DefinitionServiceState;
use klf_kir::BlockTarget;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_switch_enum2",
        state,
        DefinitionServiceState::function_switch_enum2,
    );
}

impl DefinitionServiceState {
    fn function_switch_enum2(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Some(type_id) = self
            .enum_values
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, type_id)| *type_id)
        else {
            return u64::MAX;
        };
        let parse_variant = |value: Option<&u64>| {
            value
                .copied()
                .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
                .and_then(|value| u32::try_from(value).ok())
        };
        let Some(first_variant) = parse_variant(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(second_variant) = parse_variant(arguments.get(4)) else {
            return u64::MAX;
        };
        if first_variant == second_variant {
            return u64::MAX;
        }
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        if declaration.variants.get(first_variant as usize).is_none()
            || declaration.variants.get(second_variant as usize).is_none()
        {
            return u64::MAX;
        }
        let Some(first_block) = self.block_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(second_block) = self.block_name(function_id, arguments.get(5).copied()) else {
            return u64::MAX;
        };
        let Some(default_block) = self.block_name(function_id, arguments.get(6).copied()) else {
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
        function.builder.switch_enum(
            value,
            vec![
                (
                    first_variant,
                    BlockTarget {
                        block: first_block,
                        arguments: Vec::new(),
                    },
                ),
                (
                    second_variant,
                    BlockTarget {
                        block: second_block,
                        arguments: Vec::new(),
                    },
                ),
            ],
            Some(BlockTarget {
                block: default_block,
                arguments: Vec::new(),
            }),
        );
        0
    }
}
