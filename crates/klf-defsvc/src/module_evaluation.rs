use crate::{DefinitionServiceState, ServiceSink};
use klf_core::Name;
use klf_kir::{Module, Type};
use klf_services::ModuleBuilder;
use klf_vm::{EvaluationBudget, Machine, Outcome, Program};
use std::sync::{Arc, Mutex};

pub(super) struct EvaluatedValue {
    success: bool,
    value: u64,
    bytes: Vec<u8>,
}

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state(
        "klfsvc_kir_module_detached",
        state,
        DefinitionServiceState::module_detached,
    );
    services.register_state("klfsvc_kir_module_evaluate", state, |state, arguments| {
        state.module_evaluate(arguments, false)
    });
    services.register_state(
        "klfsvc_kir_module_evaluate_buffer",
        state,
        |state, arguments| state.module_evaluate(arguments, true),
    );
    services.register_state(
        "klfsvc_kir_module_evaluation_root",
        state,
        DefinitionServiceState::evaluation_root,
    );
    services.register_state(
        "klfsvc_kir_module_evaluator",
        state,
        DefinitionServiceState::module_evaluator,
    );
    services.register_state(
        "klfsvc_kir_evaluator_evaluate_buffer",
        state,
        DefinitionServiceState::evaluator_evaluate_buffer,
    );
    services.register_state(
        "klfsvc_kir_evaluator_release",
        state,
        DefinitionServiceState::evaluator_release,
    );
    services.register_state("klfsvc_kir_evaluation_source", state, |state, arguments| {
        let [session, origin, result] = arguments else {
            return u64::MAX;
        };
        if *session != state.session.as_u64() {
            return u64::MAX;
        }
        let Some(result) = usize::try_from(*result)
            .ok()
            .and_then(|result| state.evaluations.get(result))
            .filter(|result| result.success)
        else {
            return u64::MAX;
        };
        let Ok(text) = String::from_utf8(result.bytes.clone()) else {
            return u64::MAX;
        };
        state
            .add_virtual_source(*origin, text)
            .map_or(u64::MAX, |file| u64::from(file.as_u32()))
    });
    services.register_state("klfsvc_kir_evaluation_status", state, |state, arguments| {
        state
            .evaluated(arguments)
            .map_or(u64::MAX, |result| u64::from(result.success))
    });
    services.register_state("klfsvc_kir_evaluation_value", state, |state, arguments| {
        state
            .evaluated(arguments)
            .filter(|result| result.success)
            .map_or(u64::MAX, |result| result.value)
    });
    services.register_state("klfsvc_kir_evaluation_length", state, |state, arguments| {
        state
            .evaluated(arguments)
            .map_or(u64::MAX, |result| result.bytes.len() as u64)
    });
    services.register_state("klfsvc_kir_evaluation_byte", state, |state, arguments| {
        let index = arguments
            .get(1)
            .and_then(|index| usize::try_from(*index).ok());
        state
            .evaluated(arguments)
            .zip(index)
            .and_then(|(result, index)| result.bytes.get(index))
            .map_or(u64::MAX, |byte| u64::from(*byte))
    });
}

impl DefinitionServiceState {
    fn evaluated(&self, arguments: &[u64]) -> Option<&EvaluatedValue> {
        self.evaluations
            .get(usize::try_from(*arguments.first()?).ok()?)
    }

    fn module_detached(&mut self, arguments: &[u64]) -> u64 {
        if !self.valid_session_unit(arguments) {
            return u64::MAX;
        }
        let handle = self.modules.len() as u64;
        self.modules.push(Some(ModuleBuilder::new(
            Name::new("evaluation"),
            "definition",
            self.target.clone(),
        )));
        handle
    }

    fn module_evaluate(&mut self, arguments: &[u64], buffer: bool) -> u64 {
        let result = (|| {
            let module = *arguments.first().ok_or("missing evaluation module")?;
            let entry = self
                .ll_callee_name(arguments.get(1).copied())
                .ok_or("missing evaluation entry")?;
            let prepared = self.prepare_evaluation(module, std::slice::from_ref(&entry))?;
            let fields = if buffer {
                Some((arguments.get(2).copied(), arguments.get(3).copied()))
            } else {
                None
            };
            let extra = if buffer { 2 } else { 0 };
            prepared.evaluate(
                &entry,
                fields,
                arguments.get(2 + extra).copied(),
                arguments.get(3 + extra).copied(),
            )
        })();
        self.push_evaluation(result)
    }

    fn evaluation_root(&mut self, arguments: &[u64]) -> u64 {
        let (Some(module), Some(function)) =
            (arguments.first().copied(), arguments.get(1).copied())
        else {
            return u64::MAX;
        };
        let Some(entry) = self.ll_callee_name(Some(function)) else {
            return u64::MAX;
        };
        self.evaluation_roots.push((module, function, entry));
        0
    }

    fn module_evaluator(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let mut entries = Vec::new();
        self.evaluation_roots.retain(|(owner, function, entry)| {
            if *owner == module {
                entries.push((*function, entry.clone()));
            }
            *owner != module
        });
        let roots = entries
            .iter()
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>();
        let prepared = self.prepare_evaluation(module, &roots).map(|mut prepared| {
            prepared.entries = entries;
            prepared
        });
        let handle = self.evaluators.len() as u64;
        self.evaluators.push(Some(prepared));
        handle
    }

    fn evaluator_evaluate_buffer(&mut self, arguments: &[u64]) -> u64 {
        let result = (|| {
            let evaluator = arguments
                .first()
                .and_then(|handle| usize::try_from(*handle).ok())
                .and_then(|handle| self.evaluators.get(handle))
                .and_then(Option::as_ref)
                .ok_or("missing evaluator")?;
            let prepared = evaluator.as_ref().map_err(Clone::clone)?;
            let function = arguments.get(1).copied();
            let entry = prepared
                .entries
                .iter()
                .find(|(handle, _)| Some(*handle) == function)
                .map(|(_, entry)| entry)
                .ok_or("missing evaluation entry")?;
            prepared.evaluate(
                entry,
                Some((arguments.get(2).copied(), arguments.get(3).copied())),
                arguments.get(4).copied(),
                arguments.get(5).copied(),
            )
        })();
        self.push_evaluation(result)
    }

    fn evaluator_release(&mut self, arguments: &[u64]) -> u64 {
        let handle = arguments
            .first()
            .and_then(|handle| usize::try_from(*handle).ok());
        let Some(slot) = handle.and_then(|handle| self.evaluators.get_mut(handle)) else {
            return u64::MAX;
        };
        *slot = None;
        0
    }

    /// The finished, verified and lowered module of `builder`, keeping the
    /// functions `roots` and its initializers reach.
    fn prepare_evaluation(
        &mut self,
        builder: u64,
        roots: &[Name],
    ) -> Result<PreparedEvaluation, String> {
        let mut module = self.finish_builder(builder)?;
        crate::evaluation::retain_reached(&mut module, roots);
        let diagnostics = klf_kir_verify::verify_module(&module);
        if !diagnostics.is_empty() {
            return Err(format!("invalid evaluation KIR: {diagnostics:?}"));
        }
        let program = klf_vm::lower_module(&module).map_err(|error| error.to_string())?;
        Ok(PreparedEvaluation {
            module,
            program,
            entries: Vec::new(),
        })
    }

    fn push_evaluation(&mut self, result: Result<EvaluatedValue, String>) -> u64 {
        let value = result.unwrap_or_else(|error: String| EvaluatedValue {
            success: false,
            value: 0,
            bytes: error.into_bytes(),
        });
        let handle = self.evaluations.len() as u64;
        self.evaluations.push(value);
        handle
    }
}

/// A module lowered for the VM once and evaluated from any of its entries,
/// each run on a fresh machine whose globals its initializers set.
pub(super) struct PreparedEvaluation {
    module: Module,
    program: Program,
    /// The function handle and name of each entry.
    entries: Vec<(u64, Name)>,
}

impl PreparedEvaluation {
    fn evaluate(
        &self,
        entry: &Name,
        fields: Option<(Option<u64>, Option<u64>)>,
        steps: Option<u64>,
        memory: Option<u64>,
    ) -> Result<EvaluatedValue, String> {
        let steps = steps.ok_or("missing evaluation step budget")?;
        let memory = usize::try_from(memory.ok_or("missing evaluation memory budget")?)
            .map_err(|_| "invalid evaluation memory budget")?;
        let mut budget = EvaluationBudget {
            steps_remaining: steps,
            memory_bytes: memory,
        };
        let mut machine = Machine::for_evaluation(&self.program, &budget)
            .map_err(|outcome| format!("evaluation heap: {outcome:?}"))?;
        for initializer in &self.program.initializers {
            let outcome = machine.evaluate(*initializer, &[], &mut budget);
            if !matches!(outcome, Outcome::Return(_)) {
                return Err(format!("evaluation initializer: {outcome:?}"));
            }
        }
        let index = self
            .program
            .function_index(entry.as_str())
            .ok_or("unknown evaluation entry")?;
        if let Some((data_field, length_field)) = fields {
            let data_field = usize::try_from(data_field.ok_or("missing data field")?)
                .map_err(|_| "invalid data field")?;
            let length_field = usize::try_from(length_field.ok_or("missing length field")?)
                .map_err(|_| "invalid length field")?;
            let bytes = evaluate_buffer(
                &self.module,
                &mut machine,
                index,
                entry,
                data_field,
                length_field,
                &mut budget,
            )?;
            return Ok(EvaluatedValue {
                success: true,
                value: 0,
                bytes,
            });
        }
        match machine.evaluate(index, &[], &mut budget) {
            Outcome::Return(value) => Ok(EvaluatedValue {
                success: true,
                value: value.unwrap_or(0),
                bytes: Vec::new(),
            }),
            outcome => Err(format!("evaluation of `{entry}`: {outcome:?}")),
        }
    }
}
fn evaluate_buffer(
    module: &Module,
    machine: &mut Machine<'_>,
    entry: usize,
    name: &Name,
    data_field: usize,
    length_field: usize,
    budget: &mut EvaluationBudget,
) -> Result<Vec<u8>, String> {
    let function = module
        .functions
        .iter()
        .find(|function| function.name == *name)
        .ok_or("missing evaluation function")?;
    if function.parameters.len() != 1 {
        return Err("buffer evaluation entry requires one output parameter".to_owned());
    }
    let Some(Type::Address(result)) = function.parameters.first().map(|parameter| &parameter.ty)
    else {
        return Err("buffer evaluation entry requires an address output parameter".to_owned());
    };
    let Type::Nominal(record_name, _) = result.as_ref() else {
        return Err("buffer result must be a record".to_owned());
    };
    let record = module
        .types
        .iter()
        .find(|record| record.name == *record_name)
        .ok_or("missing evaluation result record")?;
    if record.fields.get(data_field).map(|field| &field.ty) != Some(&Type::Pointer)
        || record.fields.get(length_field).map(|field| &field.ty) != Some(&Type::Integer(64))
    {
        return Err("buffer result requires pointer and i64 length fields".to_owned());
    }
    let layout = klf_layout::layout_of(result, &module.types);
    let offsets = klf_layout::field_offsets(record_name.as_str(), &module.types);
    let slot = machine
        .evaluation_slot(host_size(layout.size)?, host_size(layout.align)?, budget)
        .map_err(|outcome| format!("evaluation result allocation: {outcome:?}"))?;
    let outcome = machine.evaluate(entry, &[slot], budget);
    if !matches!(outcome, Outcome::Return(_)) {
        return Err(format!("buffer evaluation: {outcome:?}"));
    }
    let mut data = [0; 8];
    let mut length = [0; 8];
    if !machine.evaluation_read(slot + offsets[data_field], &mut data)
        || !machine.evaluation_read(slot + offsets[length_field], &mut length)
    {
        return Err("invalid evaluation buffer result".to_owned());
    }
    let length = u64::from_le_bytes(length);
    if length > budget.memory_bytes as u64 {
        return Err("evaluation result exceeds memory budget".to_owned());
    }
    let mut bytes = vec![0; host_size(length)?];
    if !machine.evaluation_read(u64::from_le_bytes(data), &mut bytes) {
        return Err("invalid evaluation result bytes".to_owned());
    }
    Ok(bytes)
}

fn host_size(value: u64) -> Result<usize, String> {
    usize::try_from(value)
        .map_err(|_| "evaluation result exceeds the host's address space".to_owned())
}
