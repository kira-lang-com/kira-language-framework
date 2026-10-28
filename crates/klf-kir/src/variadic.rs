//! RIDL variadics (sections 12.14 and 29.5). A variadic function receives
//! its fixed parameters and one hidden trailing pointer to an argument area
//! its caller allocates: consecutive 8-byte slots, 16 for 128-bit integers
//! and floats. Before a backend lowers a module, each function a
//! `call.variadic` names or that starts a variadic list gets that pointer
//! as a last parameter, each `call.variadic` of it fills an area on its
//! stack and passes it, and the list operations become reads and writes of
//! the cursor a list holds: `va.start` stores the area's address, `va.arg`
//! reads the slot under the cursor and moves it on, `va.copy` copies the
//! cursor and `va.end` does nothing.
//!
//! An exported function in the platform convention keeps the platform's
//! variadics, and a function a `func.ref` names keeps its signature, since
//! an indirect caller passes no area; neither is rewritten.

use crate::{
    Function, FunctionAbi, Instruction, Linkage, Module, ModuleIndex, Operation, Parameter,
    RawAccess, Type,
};
use klf_core::Name;
use std::collections::HashSet;

/// The module with its RIDL variadics rewritten, or `None` when it has none.
#[must_use]
pub fn variadics_to_areas(module: &Module) -> Option<Module> {
    let mut called: HashSet<&str> = HashSet::new();
    let mut referenced: HashSet<&str> = HashSet::new();
    for operation in module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .map(|instruction| &instruction.operation)
    {
        match operation {
            Operation::CallVariadic { callee, .. } => {
                called.insert(callee.as_str());
            }
            Operation::FunctionReference(name) => {
                referenced.insert(name.as_str());
            }
            _ => {}
        }
    }
    let variadic: HashSet<Name> = module
        .functions
        .iter()
        .filter(|function| {
            !function.foreign
                && !function.blocks.is_empty()
                && (function.abi == FunctionAbi::Ridl || function.linkage != Linkage::Exported)
                && !referenced.contains(function.name.as_str())
                && (called.contains(function.name.as_str()) || starts_a_list(function))
        })
        .map(|function| function.name.clone())
        .collect();
    if variadic.is_empty() {
        return None;
    }
    let index = ModuleIndex::of(module);
    let mut result = module.clone();
    for function in &mut result.functions {
        let types = if function.blocks.is_empty() {
            crate::ValueTypes::default()
        } else {
            crate::infer_value_types_in(function, &index)
        };
        let mut rewrite = Rewrite {
            types: &types,
            fresh: Fresh::new(function),
            out: Vec::new(),
        };
        let area = variadic
            .contains(&function.name)
            .then(|| rewrite.fresh.name("varargs"));
        if let Some(area) = &area {
            let parameter = Parameter {
                name: area.clone(),
                convention: None,
                context: false,
                ty: Type::Pointer,
                span: function.span,
            };
            function.parameters.push(parameter.clone());
            if let Some(entry) = function.blocks.first_mut() {
                entry.parameters.push(parameter);
            }
        }
        for block in &mut function.blocks {
            for instruction in std::mem::take(&mut block.instructions) {
                rewrite.instruction(instruction, area.as_ref(), &variadic);
            }
            block.instructions = std::mem::take(&mut rewrite.out);
        }
    }
    Some(result)
}

fn starts_a_list(function: &Function) -> bool {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction.operation, Operation::VaStart(_)))
}

/// The bytes one argument of `ty` takes in the area.
fn slot(ty: &Type) -> i128 {
    match ty {
        Type::Integer(bits) | Type::Float(bits) if *bits > 64 => 16,
        _ => 8,
    }
}

struct Rewrite<'a> {
    types: &'a crate::ValueTypes,
    fresh: Fresh,
    out: Vec<Instruction>,
}

impl Rewrite<'_> {
    fn instruction(
        &mut self,
        instruction: Instruction,
        area: Option<&Name>,
        variadic: &HashSet<Name>,
    ) {
        match (&instruction.operation, area) {
            (Operation::VaStart(list), Some(area)) => {
                let list = self.pointer(list, &instruction);
                self.store(area.clone(), list, &instruction);
            }
            (Operation::VaArg { ty, list }, Some(_)) => {
                let list = self.pointer(list, &instruction);
                let cursor = self.load(Type::Pointer, list.clone(), &instruction);
                self.push(
                    instruction.results.clone(),
                    Operation::RawLoad {
                        ty: ty.clone(),
                        pointer: cursor.clone(),
                        access: RawAccess::default(),
                    },
                    &instruction,
                );
                let next = self.offset(cursor, slot(ty), &instruction);
                self.store(next, list, &instruction);
            }
            (Operation::VaCopy { dest, source }, Some(_)) => {
                let dest = self.pointer(dest, &instruction);
                let source = self.pointer(source, &instruction);
                let cursor = self.load(Type::Pointer, source, &instruction);
                self.store(cursor, dest, &instruction);
            }
            (Operation::VaEnd(_), Some(_)) => {}
            (
                Operation::CallVariadic {
                    callee,
                    fixed,
                    variadic: passed,
                },
                _,
            ) if variadic.contains(callee) => {
                let (callee, fixed, passed) = (callee.clone(), fixed.clone(), passed.clone());
                let area = self.area(&passed, &instruction);
                let mut arguments = fixed;
                arguments.push(area);
                self.push(
                    instruction.results.clone(),
                    Operation::Call {
                        callee,
                        generic_args: Vec::new(),
                        arguments,
                    },
                    &instruction,
                );
            }
            _ => self.out.push(instruction),
        }
    }

    /// A stack area holding `passed`, one slot each, as a `ptr`.
    fn area(&mut self, passed: &[Name], from: &Instruction) -> Name {
        let sizes: Vec<i128> = passed
            .iter()
            .map(|value| self.types.get(value).map_or(8, slot))
            .collect();
        let words = (sizes.iter().sum::<i128>() / 8).max(1);
        let storage = self.fresh.name("area");
        self.push(
            vec![storage.clone()],
            Operation::StackAlloc(Type::Array(
                u64::try_from(words).unwrap_or(1),
                Box::new(Type::Integer(64)),
            )),
            from,
        );
        let base = self.fresh.name("area");
        self.push(vec![base.clone()], Operation::AddrToPointer(storage), from);
        let mut at = 0;
        for (value, size) in passed.iter().zip(sizes) {
            let slot = self.offset(base.clone(), at, from);
            self.store(value.clone(), slot, from);
            at += size;
        }
        base
    }

    /// `value` as a `ptr`, converting an address.
    fn pointer(&mut self, value: &Name, from: &Instruction) -> Name {
        if !matches!(self.types.get(value), Some(Type::Address(_))) {
            return value.clone();
        }
        let pointer = self.fresh.name("list");
        self.push(
            vec![pointer.clone()],
            Operation::AddrToPointer(value.clone()),
            from,
        );
        pointer
    }

    fn offset(&mut self, pointer: Name, bytes: i128, from: &Instruction) -> Name {
        let count = self.fresh.name("slot");
        self.push(
            vec![count.clone()],
            Operation::ConstInt {
                ty: Type::Integer(64),
                value: bytes,
            },
            from,
        );
        let moved = self.fresh.name("slot");
        self.push(
            vec![moved.clone()],
            Operation::PointerOffset {
                pointer,
                bytes: count,
            },
            from,
        );
        moved
    }

    fn load(&mut self, ty: Type, pointer: Name, from: &Instruction) -> Name {
        let value = self.fresh.name("cursor");
        self.push(
            vec![value.clone()],
            Operation::RawLoad {
                ty,
                pointer,
                access: RawAccess::default(),
            },
            from,
        );
        value
    }

    fn store(&mut self, value: Name, pointer: Name, from: &Instruction) {
        self.push(
            Vec::new(),
            Operation::RawStore {
                value,
                pointer,
                access: RawAccess::default(),
            },
            from,
        );
    }

    fn push(&mut self, results: Vec<Name>, operation: Operation, from: &Instruction) {
        self.out.push(Instruction {
            results,
            operation,
            debug_location: from.debug_location.clone(),
            span: from.span,
        });
    }
}

/// Names no value of the function has.
struct Fresh {
    taken: HashSet<Name>,
    next: u32,
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
        Self { taken, next: 0 }
    }

    fn name(&mut self, stem: &str) -> Name {
        loop {
            let name = Name::new(format!("{stem}.va{}", self.next));
            self.next += 1;
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }
}
