//! One function's vector values rewritten as lanes.

use super::{lane_tuple, lanes};
use crate::{
    ConvertKind, Function, Instruction, ModuleIndex, Operation, Parameter, Type, ValueTypes,
};
use klf_core::Name;
use std::collections::{HashMap, HashSet};

/// `function` with every vector value split into lanes, or `None` when it
/// holds a vector this does not know how to split.
pub(super) fn rewrite(
    function: &Function,
    index: &ModuleIndex<'_>,
    converted: &HashSet<&str>,
) -> Option<Function> {
    let types = crate::infer_value_types_in(function, index);
    let own = converted.contains(function.name.as_str());
    let in_signature = lanes(&function.result).is_some()
        || function
            .parameters
            .iter()
            .any(|parameter| lanes(&parameter.ty).is_some());
    if in_signature && !own {
        return None;
    }
    let mut split = Split {
        types: &types,
        converted,
        lanes: HashMap::new(),
        renames: HashMap::new(),
        fresh: Fresh::new(function),
        out: Vec::new(),
    };
    split.name_lanes(function);
    let mut result = function.clone();
    result.parameters = split.parameters(&function.parameters);
    result.result = lane_tuple(&function.result);
    for block in &mut result.blocks {
        block.parameters = split.parameters(&block.parameters);
        for instruction in std::mem::take(&mut block.instructions) {
            split.instruction(instruction)?;
        }
        block.instructions = std::mem::take(&mut split.out);
    }
    split.apply_renames(&mut result);
    Some(result)
}

struct Split<'a> {
    types: &'a ValueTypes,
    converted: &'a HashSet<&'a str>,
    /// The lane names of every vector value.
    lanes: HashMap<Name, Vec<Name>>,
    /// Names that stand for another value: a picked lane, or a lane of a
    /// copied vector.
    renames: HashMap<Name, Name>,
    fresh: Fresh,
    out: Vec<Instruction>,
}

impl Split<'_> {
    fn name_lanes(&mut self, function: &Function) {
        let mut vectors: Vec<Name> = self
            .types
            .iter()
            .filter(|(_, ty)| lanes(ty).is_some())
            .map(|(name, _)| name.clone())
            .collect();
        vectors.sort();
        for name in vectors {
            let (count, _) = lanes(&self.types[&name]).expect("a vector");
            let names = (0..count)
                .map(|lane| self.fresh.name(&format!("{name}.l{lane}")))
                .collect();
            self.lanes.insert(name, names);
        }
        for parameter in &function.parameters {
            if lanes(&parameter.ty).is_some() && !self.lanes.contains_key(&parameter.name) {
                let (count, _) = lanes(&parameter.ty).expect("a vector");
                let names = (0..count)
                    .map(|lane| self.fresh.name(&format!("{}.l{lane}", parameter.name)))
                    .collect();
                self.lanes.insert(parameter.name.clone(), names);
            }
        }
    }

    fn parameters(&self, parameters: &[Parameter]) -> Vec<Parameter> {
        let mut out = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            let Some((_, element)) = lanes(&parameter.ty) else {
                out.push(parameter.clone());
                continue;
            };
            for lane in &self.lanes[&parameter.name] {
                out.push(Parameter {
                    name: lane.clone(),
                    convention: parameter.convention,
                    context: false,
                    ty: element.clone(),
                    span: parameter.span,
                });
            }
        }
        out
    }

    fn is_vector(&self, name: &Name) -> bool {
        self.lanes.contains_key(name)
    }

    fn lane(&self, name: &Name, lane: usize) -> Option<Name> {
        self.lanes.get(name)?.get(lane).cloned()
    }

    fn push(&mut self, results: Vec<Name>, operation: Operation, from: &Instruction) {
        self.out.push(Instruction {
            results,
            operation,
            debug_location: from.debug_location.clone(),
            span: from.span,
        });
    }

    #[allow(clippy::too_many_lines)]
    fn instruction(&mut self, mut instruction: Instruction) -> Option<()> {
        for target in instruction.operation.targets_mut() {
            target.arguments = target
                .arguments
                .iter()
                .flat_map(|argument| {
                    self.lanes
                        .get(argument)
                        .cloned()
                        .unwrap_or_else(|| vec![argument.clone()])
                })
                .collect();
        }
        let uses_vector = instruction
            .operation
            .used_values()
            .into_iter()
            .any(|used| self.is_vector(used));
        let result = instruction.results.first().cloned();
        let makes_vector = result.as_ref().is_some_and(|name| self.is_vector(name));
        if !uses_vector && !makes_vector {
            self.out.push(instruction);
            return Some(());
        }
        let count = result
            .as_ref()
            .and_then(|name| self.lanes.get(name))
            .map_or(0, Vec::len);
        match &instruction.operation {
            Operation::VectorSplat { value, .. } => {
                for lane in self.lanes[result.as_ref()?].clone() {
                    self.renames.insert(lane, value.clone());
                }
            }
            Operation::VectorExtract { vector, lane } => {
                let picked = self.lane(vector, *lane as usize)?;
                self.renames.insert(result?, picked);
            }
            Operation::VectorInsert {
                vector,
                lane,
                value,
            } => {
                let made = self.lanes[result.as_ref()?].clone();
                for (position, name) in made.into_iter().enumerate() {
                    let from = if position == *lane as usize {
                        value.clone()
                    } else {
                        self.lane(vector, position)?
                    };
                    self.renames.insert(name, from);
                }
            }
            Operation::VectorShuffle { lhs, rhs, mask } => {
                let mut both = self.lanes.get(lhs)?.clone();
                both.extend(self.lanes.get(rhs)?.iter().cloned());
                let made = self.lanes[result.as_ref()?].clone();
                for (name, &pick) in made.into_iter().zip(mask) {
                    self.renames.insert(name, both.get(pick as usize)?.clone());
                }
            }
            Operation::Copy(value) | Operation::Move(value) => {
                let made = self.lanes[result.as_ref()?].clone();
                for (position, name) in made.into_iter().enumerate() {
                    let from = self.lane(value, position)?;
                    self.renames.insert(name, from);
                }
            }
            Operation::Destroy(_) | Operation::DebugValue { .. } | Operation::DebugAddr { .. } => {}
            Operation::ConstZero(Type::Vector(_, element)) => {
                for lane in self.lanes[result.as_ref()?].clone() {
                    self.push(
                        vec![lane],
                        Operation::ConstZero((**element).clone()),
                        &instruction,
                    );
                }
            }
            Operation::Load { mode, address } if makes_vector => {
                let mode = *mode;
                let address = address.clone();
                for (position, lane) in self.lanes[result.as_ref()?].clone().into_iter().enumerate()
                {
                    let at = self.lane_address(&address, position, &instruction);
                    self.push(
                        vec![lane],
                        Operation::Load { mode, address: at },
                        &instruction,
                    );
                }
            }
            Operation::Store {
                mode,
                value,
                address,
            } if self.is_vector(value) => {
                let (mode, value, address) = (*mode, value.clone(), address.clone());
                for (position, lane) in self.lanes[&value].clone().into_iter().enumerate() {
                    let at = self.lane_address(&address, position, &instruction);
                    self.push(
                        Vec::new(),
                        Operation::Store {
                            mode,
                            value: lane,
                            address: at,
                        },
                        &instruction,
                    );
                }
            }
            Operation::Convert {
                kind: ConvertKind::Bitcast,
                ..
            } => return None,
            Operation::Binary { .. }
            | Operation::IntegerUnary { .. }
            | Operation::IntegerCompare { .. }
            | Operation::Select { .. }
            | Operation::FloatBinary { .. }
            | Operation::FloatCompare { .. }
            | Operation::FloatUnary { .. }
            | Operation::FloatFma { .. }
            | Operation::Convert { .. }
                if makes_vector =>
            {
                for position in 0..count {
                    let mut operation = instruction.operation.clone();
                    for used in operation.used_values_mut() {
                        if let Some(lane) = self.lane(used, position) {
                            *used = lane;
                        }
                    }
                    if let Operation::Convert { ty, .. } = &mut operation {
                        *ty = ty.lane().clone();
                    }
                    let lane = self.lane(result.as_ref()?, position)?;
                    self.push(vec![lane], operation, &instruction);
                }
            }
            Operation::Call {
                callee,
                generic_args,
                arguments,
            } if self.converted.contains(callee.as_str()) => {
                let arguments = arguments
                    .iter()
                    .flat_map(|argument| {
                        self.lanes
                            .get(argument)
                            .cloned()
                            .unwrap_or_else(|| vec![argument.clone()])
                    })
                    .collect();
                let call = Operation::Call {
                    callee: callee.clone(),
                    generic_args: generic_args.clone(),
                    arguments,
                };
                if makes_vector {
                    let tuple = self.fresh.name(&format!("{}.lanes", result.as_ref()?));
                    self.push(vec![tuple.clone()], call, &instruction);
                    let made = self.lanes[result.as_ref()?].clone();
                    self.push(made, Operation::TupleDestructure(tuple), &instruction);
                } else {
                    self.push(instruction.results.clone(), call, &instruction);
                }
            }
            Operation::Return(Some(value)) => {
                let made = self.lanes.get(value)?.clone();
                let tuple = self.fresh.name(&format!("{value}.lanes"));
                self.push(vec![tuple.clone()], Operation::TupleNew(made), &instruction);
                self.push(Vec::new(), Operation::Return(Some(tuple)), &instruction);
            }
            _ => return None,
        }
        Some(())
    }

    /// The address of lane `position` of the vector stored at `address`.
    fn lane_address(&mut self, address: &Name, position: usize, from: &Instruction) -> Name {
        let lane = self.fresh.name(&format!("{address}.lane{position}"));
        let at = self.fresh.name(&format!("{address}.at{position}"));
        self.push(
            vec![lane.clone()],
            Operation::ConstInt {
                ty: Type::Integer(64),
                value: position as i128,
            },
            from,
        );
        self.push(
            vec![at.clone()],
            Operation::ElemAddr {
                base: address.clone(),
                index: lane,
            },
            from,
        );
        at
    }

    fn apply_renames(&self, function: &mut Function) {
        let resolve = |name: &Name| {
            let mut current = name;
            while let Some(next) = self.renames.get(current) {
                current = next;
            }
            current.clone()
        };
        for instruction in function
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
        {
            for used in instruction.operation.used_values_mut() {
                *used = resolve(used);
            }
        }
    }
}

/// Names no value of the function has.
struct Fresh {
    taken: HashSet<Name>,
}

impl Fresh {
    fn new(function: &Function) -> Self {
        let mut taken: HashSet<Name> = function
            .parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        for block in &function.blocks {
            taken.extend(block.parameters.iter().map(|p| p.name.clone()));
            for instruction in &block.instructions {
                taken.extend(instruction.results.iter().cloned());
            }
        }
        Self { taken }
    }

    fn name(&mut self, stem: &str) -> Name {
        let mut attempt = 0u32;
        loop {
            let name = if attempt == 0 {
                Name::new(stem)
            } else {
                Name::new(format!("{stem}.{attempt}"))
            };
            attempt += 1;
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }
}
