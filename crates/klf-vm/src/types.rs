//! Minimal value typing used to pick bytecode widths during lowering.

use klf_core::Name;
use klf_kir::{Function, Operation, Type};
use std::collections::HashMap;

pub struct ValueTypes {
    values: klf_kir::ValueTypes,
    block_parameters: HashMap<Name, Vec<Name>>,
}

impl ValueTypes {
    #[must_use]
    pub fn get(&self, name: &Name) -> Option<&Type> {
        self.values.get(name)
    }

    #[must_use]
    pub fn block_parameters(&self, block: &Name) -> Option<&[Name]> {
        self.block_parameters.get(block).map(Vec::as_slice)
    }

    #[must_use]
    pub fn infer(
        function: &Function,
        results: &HashMap<&str, Type>,
        global_types: &HashMap<&str, Type>,
        decls: &[klf_kir::TypeDeclaration],
    ) -> Self {
        let mut values = klf_kir::ValueTypes::default();
        let mut block_parameters = HashMap::new();
        for parameter in &function.parameters {
            values.insert(parameter.name.clone(), parameter.ty.clone());
        }
        for block in &function.blocks {
            let mut names = Vec::with_capacity(block.parameters.len());
            for parameter in &block.parameters {
                values.insert(parameter.name.clone(), parameter.ty.clone());
                names.push(parameter.name.clone());
            }
            block_parameters.insert(block.name.clone(), names);
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
                    if let Operation::BinaryOverflow { lhs, .. } = &instruction.operation {
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
                    if let Operation::AtomicCompareExchange { expected, .. } =
                        &instruction.operation
                    {
                        if let ([value, exchanged], Some(ty)) = (
                            instruction.results.as_slice(),
                            values.get(expected).cloned(),
                        ) {
                            if !values.contains_key(value) {
                                values.insert(value.clone(), ty);
                                changed = true;
                            }
                            if !values.contains_key(exchanged) {
                                values.insert(exchanged.clone(), Type::Integer(1));
                                changed = true;
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
                    if let Some(ty) = result_type(
                        &instruction.operation,
                        &values,
                        results,
                        global_types,
                        decls,
                    ) {
                        values.insert(result.clone(), ty);
                        changed = true;
                    }
                }
            }
        }
        Self {
            values,
            block_parameters,
        }
    }
}

fn result_type(
    operation: &Operation,
    values: &klf_kir::ValueTypes,
    results: &HashMap<&str, Type>,
    global_types: &HashMap<&str, Type>,
    decls: &[klf_kir::TypeDeclaration],
) -> Option<Type> {
    match operation {
        Operation::ConstInt { ty, .. }
        | Operation::ConstFloat { ty, .. }
        | Operation::ConstZero(ty)
        | Operation::Convert { ty, .. }
        | Operation::FromRaw { ty, .. }
        | Operation::RawLoad { ty, .. }
        | Operation::AtomicLoad { ty, .. } => Some(ty.clone()),
        Operation::PointerDiff { .. }
        | Operation::TypeSize(_)
        | Operation::TypeAlign(_)
        | Operation::TypeStride(_) => Some(Type::Integer(64)),
        Operation::SetJmp(_) => Some(Type::Integer(32)),
        Operation::AddrToPointer(_) | Operation::RefUnowned(_) => Some(Type::Pointer),
        Operation::Select { then_value, .. } => values.get(then_value).cloned(),
        Operation::FloatFma { first, .. } => values.get(first).cloned(),
        Operation::PointerOffset { pointer, .. } => values.get(pointer).cloned(),
        Operation::LoadBorrow(address) => values.get(address).and_then(pointee).cloned(),
        Operation::ConstBool(_)
        | Operation::IntegerCompare { .. }
        | Operation::FloatCompare { .. }
        | Operation::RefIsUnique(_) => Some(Type::Integer(1)),
        Operation::ConstNull | Operation::HeapAllocRaw { .. } | Operation::IntoRaw(_) => {
            Some(Type::Pointer)
        }
        Operation::Binary { lhs, .. } | Operation::FloatBinary { lhs, .. } => {
            values.get(lhs).cloned()
        }
        Operation::FloatUnary { value, .. }
        | Operation::Copy(value)
        | Operation::Move(value)
        | Operation::AtomicRmw { value, .. } => values.get(value).cloned(),
        Operation::StackAlloc(ty) | Operation::AddrCast { ty, .. } => {
            Some(Type::Address(Box::new(ty.clone())))
        }
        Operation::HeapAllocCounted(ty) => Some(Type::Reference(Box::new(ty.clone()))),
        Operation::HeapAllocUnique(ty) => Some(Type::Unique(Box::new(ty.clone()))),
        Operation::HeapAllocArray { element, .. } => Some(Type::Address(Box::new(Type::Array(
            0,
            Box::new(element.clone()),
        )))),
        Operation::Call { callee, .. } => results.get(callee.as_str()).cloned(),
        Operation::CallForeign { symbol, .. } | Operation::CallVariadic { callee: symbol, .. } => {
            results.get(symbol.as_str()).cloned()
        }
        Operation::CallIndirect(call) => Some(call.result.clone()),
        Operation::FunctionReference(_) => Some(Type::Integer(64)),
        Operation::Load { address, .. } => values.get(address).and_then(pointee).cloned(),
        Operation::RefAddr(value) | Operation::UniqAddr(value) => values
            .get(value)
            .and_then(pointee)
            .map(|inner| Type::Address(Box::new(inner.clone()))),
        Operation::FieldAddr { base, field } => {
            let member = match values.get(base).and_then(pointee)? {
                Type::Nominal(name, _) => decls
                    .iter()
                    .find(|d| d.name == *name)?
                    .fields
                    .get(*field as usize)?
                    .ty
                    .clone(),
                Type::Tuple(elements) => elements.get(*field as usize)?.clone(),
                _ => return None,
            };
            Some(Type::Address(Box::new(member)))
        }
        Operation::ElemAddr { base, .. } => match values.get(base).and_then(pointee) {
            Some(Type::Array(_, element) | Type::Vector(_, element)) => {
                Some(Type::Address(element.clone()))
            }
            _ => None,
        },
        Operation::GlobalAddress { global, .. } => global_types
            .get(global.as_str())
            .map(|ty| Type::Address(Box::new(ty.clone()))),
        Operation::EnumTag { value } => enum_tag_type(values.get(value), decls),
        Operation::EnumPayloadAddr { base, variant } => {
            enum_payload_type(values.get(base), *variant, decls)
        }
        _ => None,
    }
}

fn enum_tag_type(value: Option<&Type>, decls: &[klf_kir::TypeDeclaration]) -> Option<Type> {
    let Type::Nominal(name, _) = pointee(value?)? else {
        return None;
    };
    let declaration = decls.iter().find(|declaration| declaration.name == *name)?;
    let bits = if declaration.open {
        32
    } else if declaration.variants.len() <= 256 {
        8
    } else if declaration.variants.len() <= 65_536 {
        16
    } else {
        32
    };
    Some(Type::Integer(bits))
}

fn enum_payload_type(
    value: Option<&Type>,
    variant: u32,
    decls: &[klf_kir::TypeDeclaration],
) -> Option<Type> {
    let Type::Nominal(name, _) = pointee(value?)? else {
        return None;
    };
    let declaration = decls.iter().find(|declaration| declaration.name == *name)?;
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
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) => Some(inner),
        _ => None,
    }
}

#[must_use]
pub fn integer_width(ty: &Type) -> Option<u16> {
    match ty {
        Type::Integer(bits) => Some(*bits),
        _ => None,
    }
}

#[must_use]
pub fn is_float64(ty: &Type) -> bool {
    matches!(ty, Type::Float(64))
}
