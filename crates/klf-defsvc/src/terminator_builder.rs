use super::DefinitionServiceState;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    let trap = Arc::clone(state);
    capabilities.register("klfsvc_kir_function_trap", move |arguments| {
        trap.lock()
            .map_or(u64::MAX, |mut state| state.function_trap(arguments))
    });
    let unreachable = Arc::clone(state);
    capabilities.register("klfsvc_kir_function_unreachable", move |arguments| {
        unreachable
            .lock()
            .map_or(u64::MAX, |mut state| state.function_unreachable(arguments))
    });
}

impl DefinitionServiceState {
    fn function_trap(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((message, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
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
        function.builder.trap(message.as_str());
        0
    }

    fn function_unreachable(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let message = self.unreachable_message(&function.builder);
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        function.builder.trap(message);
        0
    }

    /// The report an `unreachable` in `function` traps with (KLF-RT-0001):
    /// where the function is declared, and its source name.
    pub(crate) fn unreachable_message(&self, function: &klf_services::FunctionBuilder) -> String {
        let name = crate::unit_conflict::source_name(function.name().as_str());
        let span = function.span();
        let place = self.source_entry(span.file).map(|(_, path, text)| {
            let start = usize::try_from(span.start).unwrap_or(0);
            let end = usize::try_from(span.end).unwrap_or(start);
            klf_diag::report::Place::in_text(path, text, start, end, "in this function")
        });
        klf_diag::report::Report {
            code: "KLF-RT-0001".to_owned(),
            headline: format!("`{name}` reached code its compiler marked unreachable"),
            place,
            notes: vec![
                "the compiler ends a path it proved can never run with `unreachable`; running it \
                 means that proof was wrong"
                    .to_owned(),
            ],
            helps: vec![
                "this is a compiler bug, not a fault in the program: report it with this function's \
                 source"
                    .to_owned(),
            ],
        }
        .render()
    }
}
