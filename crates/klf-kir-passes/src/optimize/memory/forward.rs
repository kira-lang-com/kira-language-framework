//! Redundant load elimination (section 14.1, O2): a load of a place whose
//! value an earlier load or store in the same block, or in the one block
//! before it, already holds becomes that value. `enum.tag` of an address
//! reads the tag at the enum's start, so it is a load too.

use super::super::Context;
use super::super::util::{
    fresh, instruction, is_scalar, predecessors, rename_uses, reverse_postorder,
};
use klf_core::Name;
use klf_kir::{Function, Instruction, LoadMode, Operation, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// The most places a block tracks at once.
const MOST_PLACES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Step {
    Field(u32),
    Payload(u32),
    Cast(Type),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Place {
    root: Name,
    path: Vec<Step>,
}

struct Roots {
    slots: HashSet<Name>,
    parameters: HashSet<Name>,
    escaped: HashSet<Name>,
}

impl Roots {
    fn distinct(&self, left: &Name, right: &Name) -> bool {
        let slot = |name: &Name| self.slots.contains(name);
        let private = |name: &Name| slot(name) && !self.escaped.contains(name);
        private(left)
            || private(right)
            || (slot(left) && (slot(right) || self.parameters.contains(right)))
            || (slot(right) && self.parameters.contains(left))
    }
}

fn overlaps(left: &Place, right: &Place, roots: &Roots) -> bool {
    if left.root != right.root {
        return !roots.distinct(&left.root, &right.root);
    }
    for (a, b) in left.path.iter().zip(&right.path) {
        match (a, b) {
            (Step::Field(x), Step::Field(y)) if x != y => return false,
            (Step::Field(x), Step::Field(y)) if x == y => {}
            (Step::Payload(x), Step::Payload(y)) if x == y => {}
            (Step::Cast(x), Step::Cast(y)) if x == y => {}
            _ => return true,
        }
    }
    true
}

/// Whether the operation leaves memory as it was.
fn reads_only(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::ConstInt { .. }
            | Operation::ConstBool(_)
            | Operation::ConstNull
            | Operation::ConstFloat { .. }
            | Operation::ConstZero(_)
            | Operation::ConstString(_)
            | Operation::FunctionReference(_)
            | Operation::GlobalAddress { .. }
            | Operation::Binary { .. }
            | Operation::BinaryOverflow { .. }
            | Operation::IntegerUnary { .. }
            | Operation::IntegerCompare { .. }
            | Operation::Select { .. }
            | Operation::FloatBinary { .. }
            | Operation::FloatCompare { .. }
            | Operation::FloatUnary { .. }
            | Operation::FloatFma { .. }
            | Operation::Convert { .. }
            | Operation::StructNew { .. }
            | Operation::StructExtract { .. }
            | Operation::TupleNew(_)
            | Operation::TupleExtract { .. }
            | Operation::EnumNew { .. }
            | Operation::StackAlloc(_)
            | Operation::FieldAddr { .. }
            | Operation::ElemAddr { .. }
            | Operation::EnumPayloadAddr { .. }
            | Operation::AddrCast { .. }
            | Operation::AddrToPointer(_)
            | Operation::LoadBorrow(_)
            | Operation::RawLoad { .. }
            | Operation::PointerOffset { .. }
            | Operation::PointerDiff { .. }
            | Operation::Move(_)
            | Operation::BorrowBegin(_)
            | Operation::BorrowEnd(_)
            | Operation::Assume(_)
            | Operation::Expect { .. }
            | Operation::TypeSize(_)
            | Operation::TypeAlign(_)
            | Operation::TypeStride(_)
            | Operation::Nop
            | Operation::Branch(_)
            | Operation::ConditionalBranch(_)
            | Operation::SwitchInt(_)
            | Operation::SwitchEnum(_)
            | Operation::Return(_)
            | Operation::Unreachable
    ) || matches!(
        operation,
        Operation::Load {
            mode: LoadMode::Trivial,
            ..
        }
    )
}

fn roots(function: &Function) -> Roots {
    let mut slots = HashSet::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let ([result], Operation::StackAlloc(_)) =
            (instruction.results.as_slice(), &instruction.operation)
        {
            slots.insert(result.clone());
        }
    }
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    // An address escapes when anything but a place operation takes it.
    let mut derived: HashMap<Name, Name> = slots
        .iter()
        .map(|slot: &Name| (slot.clone(), slot.clone()))
        .collect();
    let mut escaped = HashSet::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::FieldAddr { base, .. }
            | Operation::EnumPayloadAddr { base, .. }
            | Operation::AddrCast { address: base, .. } => {
                if let Some(root) = derived.get(base).cloned() {
                    derived.insert(instruction.results[0].clone(), root);
                }
            }
            Operation::Load { .. }
            | Operation::EnumTag { .. }
            | Operation::EnumInject { .. }
            | Operation::StackDealloc(_) => {}
            Operation::Store { value, .. } => {
                if let Some(root) = derived.get(value) {
                    escaped.insert(root.clone());
                }
            }
            other => {
                for used in other.used_values() {
                    if let Some(root) = derived.get(used) {
                        escaped.insert(root.clone());
                    }
                }
            }
        }
    }
    Roots {
        slots,
        parameters,
        escaped,
    }
}

type State = Vec<(Place, Name)>;

fn kill(state: &mut State, place: &Place, roots: &Roots) {
    state.retain(|(known, _)| !overlaps(known, place, roots));
}

fn remember(state: &mut State, place: Place, value: Name) {
    if state.len() >= MOST_PLACES {
        state.remove(0);
    }
    state.push((place, value));
}

fn tag_bits(ty: Option<&Type>, context: &Context<'_>) -> Option<u16> {
    let Some(Type::Address(pointee)) = ty else {
        return None;
    };
    let Type::Nominal(name, arguments) = &**pointee else {
        return None;
    };
    let declaration = context.layouts.declaration(name.as_str())?;
    (arguments.is_empty() && !declaration.variants.is_empty())
        .then(|| klf_layout::enum_declaration_tag_bits(declaration))
}

#[allow(clippy::too_many_lines)]
pub(in crate::optimize) fn forward(function: &mut Function, context: &Context<'_>) -> bool {
    if function.blocks.is_empty() {
        return false;
    }
    let types = context.types(function);
    let roots = roots(function);
    let incoming = predecessors(function);
    let order = reverse_postorder(function);
    let mut places: HashMap<Name, Place> = HashMap::default();
    let mut ends: Vec<Option<State>> = vec![None; function.blocks.len()];
    let mut renames: HashMap<Name, Name> = HashMap::default();
    // The tags written by an `enum.inject`, kept only when a read of the tag
    // is forwarded one: left behind unread, they would be the round's only
    // change and removing them the next round's, so the function would
    // never come to rest.
    let mut added: Vec<Name> = Vec::new();
    let mut changed = false;
    let resolve = |name: &Name, renames: &HashMap<Name, Name>| {
        let mut current = name.clone();
        while let Some(next) = renames.get(&current) {
            current = next.clone();
        }
        current
    };
    for position in order {
        let mut state: State = match incoming[position].as_slice() {
            [single] => ends[*single].clone().unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut kept: Vec<Instruction> =
            Vec::with_capacity(function.blocks[position].instructions.len());
        for original in std::mem::take(&mut function.blocks[position].instructions) {
            let place_of = |address: &Name, places: &HashMap<Name, Place>| {
                places.get(address).cloned().unwrap_or_else(|| Place {
                    root: address.clone(),
                    path: Vec::new(),
                })
            };
            let derived = |base: &Name, step: Step, places: &HashMap<Name, Place>| {
                let mut place = place_of(base, places);
                place.path.push(step);
                place
            };
            match &original.operation {
                Operation::FieldAddr { base, field } => {
                    let place = derived(base, Step::Field(*field), &places);
                    places.insert(original.results[0].clone(), place);
                }
                Operation::EnumPayloadAddr { base, variant } => {
                    let place = derived(base, Step::Payload(*variant), &places);
                    places.insert(original.results[0].clone(), place);
                }
                Operation::AddrCast { address, ty } => {
                    let place = derived(address, Step::Cast(ty.clone()), &places);
                    places.insert(original.results[0].clone(), place);
                }
                _ => {}
            }
            match &original.operation {
                Operation::Load {
                    mode: LoadMode::Trivial,
                    address,
                } if types.get(&original.results[0]).is_some_and(is_scalar) => {
                    let place = place_of(address, &places);
                    if let Some((_, value)) = state.iter().find(|(known, _)| *known == place) {
                        renames.insert(original.results[0].clone(), value.clone());
                        changed = true;
                        continue;
                    }
                    remember(&mut state, place, original.results[0].clone());
                }
                Operation::EnumTag { value }
                    if let Some(bits) = tag_bits(types.get(value), context) =>
                {
                    let mut place = place_of(value, &places);
                    place.path.push(Step::Cast(Type::Integer(bits)));
                    if let Some((_, known)) = state.iter().find(|(known, _)| *known == place) {
                        renames.insert(original.results[0].clone(), known.clone());
                        changed = true;
                        continue;
                    }
                    remember(&mut state, place, original.results[0].clone());
                }
                Operation::Store { value, address, .. } => {
                    let place = place_of(address, &places);
                    kill(&mut state, &place, &roots);
                    if types.get(value).is_some_and(is_scalar) {
                        remember(&mut state, place, resolve(value, &renames));
                    }
                }
                Operation::EnumInject { address, variant } => {
                    let place = place_of(address, &places);
                    kill(&mut state, &place, &roots);
                    if let Some(bits) = tag_bits(types.get(address), context) {
                        let tag = fresh("tag");
                        added.push(tag.clone());
                        kept.push(original.clone());
                        kept.push(instruction(
                            vec![tag.clone()],
                            Operation::ConstInt {
                                ty: Type::Integer(bits),
                                value: i128::from(*variant),
                            },
                            &original,
                        ));
                        let mut tagged = place;
                        tagged.path.push(Step::Cast(Type::Integer(bits)));
                        remember(&mut state, tagged, tag);
                        continue;
                    }
                }
                operation if reads_only(operation) => {}
                _ => state.retain(|(known, _)| {
                    roots.slots.contains(&known.root) && !roots.escaped.contains(&known.root)
                }),
            }
            kept.push(original);
        }
        function.blocks[position].instructions = kept;
        ends[position] = Some(state);
    }
    rename_uses(function, &renames);
    let read: HashSet<&Name> = renames.values().collect();
    let unread: HashSet<&Name> = added.iter().filter(|tag| !read.contains(tag)).collect();
    if unread.len() < added.len() {
        context.forget_types();
    }
    if !unread.is_empty() {
        for block in &mut function.blocks {
            block.instructions.retain(|instruction| {
                !matches!(instruction.results.as_slice(), [result] if unread.contains(result))
            });
        }
    }
    changed
}
