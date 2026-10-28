//! Scalar replacement of aggregates (section 14.1, O2): whole-struct copies
//! between typed addresses become field copies, and a stack slot reached only
//! through its fields becomes one slot per field.

pub(super) mod forward;
pub(super) mod promote;

use super::Context;
use super::util::{fresh, instruction, is_scalar, rename_uses};
use klf_core::Name;
use klf_kir::{
    ConvertKind, DestroyPolicy, Function, Instruction, LayoutRule, LoadMode, Operation, StoreMode,
    Type, TypeKind,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// The most fields a whole-struct copy is split into.
const MOST_LEAVES: usize = 8;

/// The scalar leaves of `ty`, as field paths, when every one is a scalar and
/// the struct drops as bits.
fn leaves(
    ty: &Type,
    context: &Context<'_>,
    path: &mut Vec<u32>,
    out: &mut Vec<(Vec<u32>, Type)>,
) -> bool {
    if is_scalar(ty) {
        out.push((path.clone(), ty.clone()));
        return out.len() <= MOST_LEAVES;
    }
    let Some(declaration) = struct_declaration(ty, context) else {
        return false;
    };
    if declaration.policy.destroy != DestroyPolicy::Trivial {
        return false;
    }
    for (position, field) in declaration.fields.iter().enumerate() {
        path.push(u32::try_from(position).unwrap_or(u32::MAX));
        let fits = leaves(&field.ty, context, path, out);
        path.pop();
        if !fits {
            return false;
        }
    }
    true
}

fn struct_declaration<'a>(
    ty: &Type,
    context: &Context<'a>,
) -> Option<&'a klf_kir::TypeDeclaration> {
    let Type::Nominal(name, arguments) = ty else {
        return None;
    };
    if !arguments.is_empty() {
        return None;
    }
    let declaration = context.index.types.get(name.as_str())?;
    (declaration.kind == TypeKind::Struct
        && declaration.layout == LayoutRule::Ridl
        && declaration.generics.is_empty())
    .then_some(*declaration)
}

/// Splits `mem.copy` of a whole struct between two typed addresses into
/// loads and stores of its fields.
pub(in crate::optimize) fn split_copies(function: &mut Function, context: &Context<'_>) -> bool {
    let types = context.types(function);
    let mut casts: HashMap<Name, Name> = HashMap::default();
    let constants = super::util::constants(function);
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let (
            [result],
            Operation::Convert {
                kind: ConvertKind::Bitcast,
                value,
                ty: Type::Pointer,
            },
        ) = (instruction.results.as_slice(), &instruction.operation)
        {
            casts.insert(result.clone(), value.clone());
        }
    }
    let pointee_of = |pointer: &Name| -> Option<(Name, Type)> {
        let origin = casts.get(pointer)?;
        match types.get(origin)? {
            Type::Address(pointee) => Some((origin.clone(), (**pointee).clone())),
            _ => None,
        }
    };
    let mut changed = false;
    for block in &mut function.blocks {
        let mut rewritten: Vec<Instruction> = Vec::with_capacity(block.instructions.len());
        for original in block.instructions.drain(..) {
            let Operation::MemCopy {
                dest,
                source,
                length,
            } = &original.operation
            else {
                rewritten.push(original);
                continue;
            };
            let split = (|| {
                let (to, to_type) = pointee_of(dest)?;
                let (from, from_type) = pointee_of(source)?;
                let &(_, bytes) = constants.get(length)?;
                if to_type != from_type
                    || u64::try_from(bytes).ok()? != context.layouts.layout_of(&to_type).size
                {
                    return None;
                }
                let mut found = Vec::new();
                leaves(&to_type, context, &mut Vec::new(), &mut found).then_some((to, from, found))
            })();
            let Some((to, from, found)) = split else {
                rewritten.push(original);
                continue;
            };
            for (path, _) in &found {
                let target = field_path(&to, path, &original, &mut rewritten);
                let origin = field_path(&from, path, &original, &mut rewritten);
                let value = fresh("field");
                rewritten.push(instruction(
                    vec![value.clone()],
                    Operation::Load {
                        mode: LoadMode::Trivial,
                        address: origin,
                    },
                    &original,
                ));
                rewritten.push(instruction(
                    Vec::new(),
                    Operation::Store {
                        mode: StoreMode::Trivial,
                        value,
                        address: target,
                    },
                    &original,
                ));
            }
            changed = true;
        }
        block.instructions = rewritten;
    }
    changed
}

fn field_path(base: &Name, path: &[u32], like: &Instruction, out: &mut Vec<Instruction>) -> Name {
    let mut address = base.clone();
    for &field in path {
        let next = fresh("faddr");
        out.push(instruction(
            vec![next.clone()],
            Operation::FieldAddr {
                base: address,
                field,
            },
            like,
        ));
        address = next;
    }
    address
}

/// Gives each field of a struct slot reached only through `field.addr` a
/// slot of its own.
pub(in crate::optimize) fn split_slots(function: &mut Function, context: &Context<'_>) -> bool {
    let mut candidates: HashMap<Name, Vec<Type>> = HashMap::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let ([result], Operation::StackAlloc(ty)) =
            (instruction.results.as_slice(), &instruction.operation)
            && let Some(declaration) = struct_declaration(ty, context)
        {
            let fields = declaration
                .fields
                .iter()
                .map(|field| field.ty.clone())
                .collect();
            candidates.insert(result.clone(), fields);
        }
    }
    if candidates.is_empty() {
        return false;
    }
    let mut refused: HashSet<Name> = HashSet::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::FieldAddr { .. }
            | Operation::StackDealloc(_)
            | Operation::DebugAddr { .. } => {}
            other => {
                for used in other.used_values() {
                    if candidates.contains_key(used) {
                        refused.insert(used.clone());
                    }
                }
            }
        }
    }
    candidates.retain(|name, _| !refused.contains(name));
    if candidates.is_empty() {
        return false;
    }
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut slots: HashMap<(Name, u32), Name> = HashMap::default();
    for (slot, fields) in &candidates {
        for field in 0..fields.len() {
            let field = u32::try_from(field).unwrap_or(u32::MAX);
            slots.insert((slot.clone(), field), fresh("slot"));
        }
    }
    for block in &mut function.blocks {
        let mut rewritten: Vec<Instruction> = Vec::with_capacity(block.instructions.len());
        for original in block.instructions.drain(..) {
            match &original.operation {
                Operation::StackAlloc(_) if candidates.contains_key(&original.results[0]) => {
                    let slot = &original.results[0];
                    for (field, ty) in candidates[slot].iter().enumerate() {
                        let field = u32::try_from(field).unwrap_or(u32::MAX);
                        rewritten.push(instruction(
                            vec![slots[&(slot.clone(), field)].clone()],
                            Operation::StackAlloc(ty.clone()),
                            &original,
                        ));
                    }
                }
                Operation::FieldAddr { base, field } if candidates.contains_key(base) => {
                    let slot = &slots[&(base.clone(), *field)];
                    renames.insert(original.results[0].clone(), slot.clone());
                }
                Operation::StackDealloc(slot) | Operation::DebugAddr { address: slot, .. }
                    if candidates.contains_key(slot) => {}
                _ => rewritten.push(original),
            }
        }
        block.instructions = rewritten;
    }
    rename_uses(function, &renames);
    true
}
