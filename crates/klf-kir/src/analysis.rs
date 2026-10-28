//! Forward type inference over SSA values. Both native backends and the VM need
//! the type of every value to pick widths; this is the single shared source.

use crate::{Function, FunctionAbi, FunctionType, Linkage, Module, Operation, ParameterType, Type};
use klf_core::Name;
use rustc_hash::FxHashMap as HashMap;

/// The module's unmanaged manifest (section 31.6): the names of functions whose
/// body contains an unmanaged instruction (`asm`, `call.foreign`). A platform
/// may gate loading on it. Foreign declarations themselves are not listed; a
/// caller of one is.
#[must_use]
pub fn unmanaged_manifest(module: &Module) -> Vec<&Name> {
    module
        .functions
        .iter()
        .filter(|function| {
            function.blocks.iter().any(|block| {
                block
                    .instructions
                    .iter()
                    .any(|instruction| instruction.operation.is_unmanaged())
            })
        })
        .map(|function| &function.name)
        .collect()
}

/// The type of each SSA value of a function, keyed by name.
pub type ValueTypes = HashMap<Name, Type>;

/// A module's functions and type declarations by name, built once and shared
/// by per-function analyses.
pub struct ModuleIndex<'m> {
    pub functions: HashMap<&'m str, &'m Function>,
    pub types: HashMap<&'m str, &'m crate::TypeDeclaration>,
    pub globals: HashMap<&'m str, &'m Type>,
}

impl<'m> ModuleIndex<'m> {
    #[must_use]
    pub fn of(module: &'m Module) -> Self {
        let mut functions = HashMap::default();
        for function in &module.functions {
            functions.entry(function.name.as_str()).or_insert(function);
        }
        let mut types = HashMap::default();
        for declaration in &module.types {
            types
                .entry(declaration.name.as_str())
                .or_insert(declaration);
        }
        let mut globals = HashMap::default();
        for declaration in &module.globals {
            globals
                .entry(declaration.name.as_str())
                .or_insert(&declaration.ty);
        }
        Self {
            functions,
            types,
            globals,
        }
    }
}

/// Infer the type of every SSA value in `function`, resolving call results
/// against `module`. Parameters and block parameters carry their declared
/// types; the rest reach a fixed point by forward propagation.
#[must_use]
pub fn infer_value_types(function: &Function, module: &Module) -> ValueTypes {
    infer_value_types_in(function, &ModuleIndex::of(module))
}

/// Infer using a module index already built by the caller.
#[must_use]
pub fn infer_value_types_in(function: &Function, index: &ModuleIndex<'_>) -> ValueTypes {
    let defined: usize = function
        .blocks
        .iter()
        .map(|block| block.parameters.len() + block.instructions.len())
        .sum();
    let mut values = HashMap::with_capacity_and_hasher(
        function.parameters.len() + defined,
        rustc_hash::FxBuildHasher,
    );
    for parameter in &function.parameters {
        values.insert(parameter.name.clone(), parameter.ty.clone());
    }
    for block in &function.blocks {
        for parameter in &block.parameters {
            values.insert(parameter.name.clone(), parameter.ty.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Operation::ConstString(_) = &instruction.operation {
                    if let [address, length] = instruction.results.as_slice() {
                        if !values.contains_key(address) {
                            values.insert(address.clone(), Type::Pointer);
                            changed = true;
                        }
                        if !values.contains_key(length) {
                            values.insert(length.clone(), Type::Integer(64));
                            changed = true;
                        }
                    }
                    continue;
                }
                // `add.ovf` and its kin, and `atomic.cmpxchg`: a value of the
                // operand's type and an `i1`.
                let paired = match &instruction.operation {
                    Operation::BinaryOverflow { lhs, .. } => Some(lhs),
                    Operation::AtomicCompareExchange { expected, .. } => Some(expected),
                    _ => None,
                };
                if let Some(lhs) = paired {
                    if let ([value, overflowed], Some(ty)) =
                        (instruction.results.as_slice(), values.get(lhs).cloned())
                    {
                        if !values.contains_key(value) {
                            values.insert(value.clone(), ty);
                            changed = true;
                        }
                        if !values.contains_key(overflowed) {
                            values.insert(overflowed.clone(), Type::Integer(1));
                            changed = true;
                        }
                    }
                    continue;
                }
                // `struct.destructure`, `tuple.destructure`, `enum.take` and
                // `enum.project`: one value per member.
                if let Some(members) = member_types(&instruction.operation, &values, index) {
                    if members.len() == instruction.results.len() {
                        for (result, ty) in instruction.results.iter().zip(members) {
                            if !values.contains_key(result) {
                                values.insert(result.clone(), ty);
                                changed = true;
                            }
                        }
                    }
                    continue;
                }
                let [result] = instruction.results.as_slice() else {
                    continue;
                };
                if values.contains_key(result) {
                    continue;
                }
                if let Some(ty) = result_type(&instruction.operation, &values, index) {
                    values.insert(result.clone(), ty);
                    changed = true;
                }
            }
        }
    }
    values
}

fn field_type(
    base: &Name,
    field: u32,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let member = match values.get(base).and_then(pointee)? {
        Type::Nominal(name, _) => index
            .types
            .get(name.as_str())?
            .fields
            .get(field as usize)?
            .ty
            .clone(),
        Type::Tuple(elements) => elements.get(field as usize)?.clone(),
        _ => return None,
    };
    Some(Type::Address(Box::new(member)))
}

fn result_type(
    operation: &Operation,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    declared_result_type(operation).or_else(|| derived_result_type(operation, values, index))
}

/// The result types the operation spells out on its own, with no need to look
/// at the types of its operands.
fn declared_result_type(operation: &Operation) -> Option<Type> {
    match operation {
        Operation::ConstInt { ty, .. }
        | Operation::ConstFloat { ty, .. }
        | Operation::ConstZero(ty)
        | Operation::Convert { ty, .. }
        | Operation::VectorSplat { ty, .. }
        | Operation::StructNew { ty, .. }
        | Operation::EnumNew { ty, .. }
        | Operation::AtomicLoad { ty, .. }
        | Operation::VaArg { ty, .. }
        | Operation::RawLoad { ty, .. }
        | Operation::FromRaw { ty, .. } => Some(ty.clone()),
        Operation::Intrinsic(call) => Some(call.result.clone()),
        Operation::CallIndirect(call) => Some(call.result.clone()),
        Operation::CallClosure(call) => Some(call.result.clone()),
        Operation::CallWitness(call) => Some(call.result.clone()),
        Operation::CallVtable(call) => Some(call.result.clone()),
        Operation::ConstBool(_) | Operation::RefIsUnique(_) => Some(Type::Integer(1)),
        Operation::ConstNull
        | Operation::DescriptorReference(_)
        | Operation::WitnessReference { .. }
        | Operation::BlockAddress(_)
        | Operation::AddrToPointer(_)
        | Operation::IntoRaw(_)
        | Operation::RefUnowned(_)
        | Operation::HeapAllocRaw { .. } => Some(Type::Pointer),
        Operation::PointerDiff { .. }
        | Operation::TypeSize(_)
        | Operation::TypeAlign(_)
        | Operation::TypeStride(_) => Some(Type::Integer(64)),
        Operation::SetJmp(_) => Some(Type::Integer(32)),
        Operation::StackAlloc(ty) | Operation::AddrCast { ty, .. } => {
            Some(Type::Address(Box::new(ty.clone())))
        }
        Operation::HeapAllocCounted(ty) => Some(Type::Reference(Box::new(ty.clone()))),
        Operation::HeapAllocUnique(ty) => Some(Type::Unique(Box::new(ty.clone()))),
        Operation::HeapAllocArray { element, .. } => Some(Type::Address(Box::new(Type::Array(
            0,
            Box::new(element.clone()),
        )))),
        Operation::ArrayNew { element, elements } => u64::try_from(elements.len())
            .ok()
            .map(|count| Type::Array(count, Box::new(element.clone()))),
        _ => None,
    }
}

/// The result types that follow from the operand types, the callee signatures
/// or the module's declarations.
fn derived_result_type(
    operation: &Operation,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    match operation {
        Operation::Binary { lhs: value, .. }
        | Operation::FloatBinary { lhs: value, .. }
        | Operation::FloatUnary { value, .. }
        | Operation::IntegerUnary { value, .. }
        | Operation::Copy(value)
        | Operation::Move(value)
        | Operation::Select {
            then_value: value, ..
        }
        | Operation::Expect { value, .. }
        | Operation::FloatFma { first: value, .. }
        | Operation::PointerOffset { pointer: value, .. }
        | Operation::AtomicRmw { value, .. }
        | Operation::VectorInsert { vector: value, .. } => values.get(value).cloned(),
        // Section 12.5: a comparison of vectors yields a vector of `i1`.
        Operation::IntegerCompare { lhs, .. } | Operation::FloatCompare { lhs, .. } => {
            Some(match values.get(lhs) {
                Some(Type::Vector(count, _)) => Type::Vector(*count, Box::new(Type::Integer(1))),
                _ => Type::Integer(1),
            })
        }
        Operation::VectorShuffle { lhs, mask, .. } => values.get(lhs).map(|ty| {
            Type::Vector(
                u32::try_from(mask.len()).unwrap_or(u32::MAX),
                Box::new(ty.lane().clone()),
            )
        }),
        Operation::Load { address: value, .. } | Operation::LoadBorrow(value) => {
            values.get(value).and_then(pointee).cloned()
        }
        Operation::RefAddr(value) | Operation::UniqAddr(value) => {
            pointee_wrapped(value, values, |inner| Type::Address(Box::new(inner)))
        }
        Operation::WeakNew(value) => {
            pointee_wrapped(value, values, |inner| Type::Weak(Box::new(inner)))
        }
        Operation::WeakLoad(value) => {
            pointee_wrapped(value, values, |inner| Type::Reference(Box::new(inner)))
        }
        Operation::EnumTag { value } => enum_tag_type(value, values, index),
        Operation::EnumPayloadAddr { base, variant } => {
            enum_payload_type(base, *variant, values, index)
        }
        Operation::TupleNew(elements) => elements
            .iter()
            .map(|element| values.get(element).cloned())
            .collect::<Option<Vec<_>>>()
            .map(Type::Tuple),
        Operation::VectorExtract { vector, .. } => values.get(vector).map(|ty| ty.lane().clone()),
        Operation::TupleExtract { tuple, index } => match values.get(tuple) {
            Some(Type::Tuple(elements)) => elements.get(*index as usize).cloned(),
            _ => None,
        },
        Operation::StructExtract { aggregate, field } => {
            struct_field(aggregate, *field, values, index)
        }
        Operation::Call { callee: name, .. }
        | Operation::CallVariadic { callee: name, .. }
        | Operation::CallForeign { symbol: name, .. } => index
            .functions
            .get(name.as_str())
            .map(|function| function.result.clone()),
        // Section 12.1: `func.ref` produces a function value, not a `ptr`, so
        // it matches a parameter declared with the callee's signature.
        Operation::FunctionReference(callee) => index
            .functions
            .get(callee.as_str())
            .map(|function| function_type(function)),
        Operation::ClosureCallback { ty, .. } => Some(Type::Function(ty.clone())),
        Operation::FieldAddr { base, field } => field_type(base, *field, values, index),
        Operation::ElemAddr { base, .. } => match values.get(base).and_then(pointee) {
            Some(Type::Array(_, element) | Type::Vector(_, element)) => {
                Some(Type::Address(element.clone()))
            }
            _ => None,
        },
        Operation::GlobalAddress { global, .. } => index
            .globals
            .get(global.as_str())
            .map(|ty| Type::Address(Box::new((*ty).clone()))),
        _ => None,
    }
}

/// The type of a function value naming `function`.
fn function_type(function: &Function) -> Type {
    let abi = if function.foreign {
        FunctionAbi::Platform
    } else if function.abi == FunctionAbi::Ridl
        || matches!(function.linkage, Linkage::Exported)
        || function.failure.is_some()
        || function
            .parameters
            .iter()
            .any(|parameter| parameter.context)
    {
        FunctionAbi::Ridl
    } else {
        FunctionAbi::Platform
    };
    Type::Function(FunctionType {
        parameters: function
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                convention: parameter.convention,
                context: parameter.context,
                ty: parameter.ty.clone(),
            })
            .collect(),
        result: Box::new(function.result.clone()),
        failure: function.failure.clone().map(Box::new),
        abi,
    })
}

/// The pointee of `value` wrapped back up in another pointer-like type.
fn pointee_wrapped(
    value: &Name,
    values: &HashMap<Name, Type>,
    wrap: impl FnOnce(Type) -> Type,
) -> Option<Type> {
    values
        .get(value)
        .and_then(pointee)
        .map(|inner| wrap(inner.clone()))
}

/// The member types the destructuring operations yield, in order.
fn member_types(
    operation: &Operation,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Vec<Type>> {
    match operation {
        Operation::StructDestructure(value) => match values.get(value)? {
            Type::Nominal(name, _) => Some(
                index
                    .types
                    .get(name.as_str())?
                    .fields
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect(),
            ),
            _ => None,
        },
        Operation::TupleDestructure(value) => match values.get(value)? {
            Type::Tuple(elements) => Some(elements.clone()),
            _ => None,
        },
        Operation::EnumTake { value, variant } | Operation::EnumProject { value, variant } => {
            let ty = values.get(value)?;
            let Type::Nominal(name, _) = pointee(ty).unwrap_or(ty) else {
                return None;
            };
            let declaration = index.types.get(name.as_str())?;
            Some(declaration.variants.get(*variant as usize)?.payload.clone())
        }
        _ => None,
    }
}

fn struct_field(
    aggregate: &Name,
    field: u32,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Some(Type::Nominal(name, _)) = values.get(aggregate) else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    declaration
        .fields
        .get(field as usize)
        .map(|field| field.ty.clone())
}

fn enum_tag_type(
    value: &Name,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Type::Nominal(name, _) = values.get(value).and_then(pointee)? else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    if declaration.variants.is_empty() {
        return None;
    }
    Some(Type::Integer(if declaration.open {
        32
    } else if declaration.variants.len() <= 256 {
        8
    } else if declaration.variants.len() <= 65_536 {
        16
    } else {
        32
    }))
}

fn enum_payload_type(
    base: &Name,
    variant: u32,
    values: &HashMap<Name, Type>,
    index: &ModuleIndex<'_>,
) -> Option<Type> {
    let Type::Nominal(name, _) = values.get(base).and_then(pointee)? else {
        return None;
    };
    let declaration = index.types.get(name.as_str())?;
    let payload = declaration
        .variants
        .get(variant as usize)?
        .payload
        .as_slice();
    let payload = match payload {
        [] => Type::Unit,
        [one] => one.clone(),
        many => Type::Tuple(many.to_vec()),
    };
    Some(Type::Address(Box::new(payload)))
}

fn pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) => {
            Some(inner)
        }
        _ => None,
    }
}
