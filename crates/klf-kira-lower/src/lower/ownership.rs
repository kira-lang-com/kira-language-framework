//! Reference counting for definition code.
//!
//! The buffer behind a list or a `String` is a `heap.alloc.array` block, a
//! reference-counted buffer (section 12.9). Every copy of a value into storage
//! takes a count on each buffer the value holds, and every storage that ends
//! gives its counts back: a local at the end of its scope, a temporary at the
//! end of the statement that made it, and a place's old value when the place
//! is assigned. A list grows in place only while `ref.isunique` says no other
//! value holds its buffer; otherwise it moves to a buffer of its own first.
//!
//! A buffer pointer is adopted with `from.raw`, so `copy` retains it,
//! `destroy` releases it and `into.raw` lets it go uncounted (section 12.10).
//! The counts of a struct, an enum, a fixed array, or a list whose elements
//! hold buffers of their own are taken by one generated function per type.

use super::Lowerer;
use super::lists::is_aggregate;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{BlockTarget, ConvertKind, Linkage, StoreMode, Type};
use klf_kira_syntax::ast::{Expr, ExprKind};
use klf_services::{FunctionBuilder, ModuleBuilder, Session};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Storage whose buffers the running function holds a count on.
pub(crate) struct Owned {
    pub storage: Name,
    pub ty: FeType,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Count {
    Retain,
    Release,
}

impl Count {
    fn spelling(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::Release => "release",
        }
    }
}

/// The functions the module's code asks for as it is lowered, shared by
/// every function lowered into the module: the per-type count functions, and
/// one instance of a generic function per choice of type arguments it is
/// called with.
#[derive(Default)]
pub(crate) struct NeededFunctions {
    pending: Vec<(Name, Count, FeType)>,
    requested: HashSet<Name>,
    counted: HashMap<FeType, bool>,
    pub(crate) instances: Vec<crate::lower::Instance>,
    instance_names: HashSet<Name>,
    /// The function each closure literal's body lowers to.
    pub(crate) closures: Vec<klf_kir::Function>,
    /// The struct each closure literal's context box holds: the release
    /// function of its captures, then the captures.
    pub(crate) contexts: crate::structs::StructTable,
}

impl NeededFunctions {
    /// Asks for `instance`, once however many calls name it.
    pub(crate) fn request_instance(&mut self, instance: crate::lower::Instance) {
        if self.instance_names.insert(instance.name.clone()) {
            self.instances.push(instance);
        }
    }
}

/// Adds the count function of every type the module's code asked for, and of
/// every type those functions ask for in turn.
pub(crate) fn add_count_functions(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &RefCell<NeededFunctions>,
    module: &mut ModuleBuilder,
) {
    loop {
        let pending = std::mem::take(&mut counts.borrow_mut().pending);
        if pending.is_empty() {
            return;
        }
        for (name, count, ty) in pending {
            let value = Name::new("value");
            let parameter = (value.clone(), Type::Address(Box::new(ty.to_kir())));
            let mut fb = FunctionBuilder::new(
                name,
                &[parameter],
                Type::Unit,
                klf_source::Span::empty(klf_core::FileId::from_u32(0), 0),
            );
            fb.set_linkage(Linkage::Module);
            let mut lowerer = Lowerer::new(session, elaborated, counts, fb, FeType::Void, None);
            lowerer.count_parts(count, &value, &ty);
            lowerer.fb.ret(None);
            let mut function = lowerer.fb.finish();
            function.abi = klf_kir::FunctionAbi::Ridl;
            module.add_function(function);
        }
    }
}

/// Whether `expr` reads storage inside a list's buffer, which a growth of
/// that list frees.
pub(crate) fn reads_through_index(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Index { .. } => true,
        ExprKind::Field { base, .. } => reads_through_index(base),
        _ => false,
    }
}

impl Lowerer<'_> {
    /// Whether a value of `ty` holds a buffer.
    pub(crate) fn counted(&self, ty: &FeType) -> bool {
        let known = self.counts.borrow().counted.get(ty).copied();
        if let Some(known) = known {
            return known;
        }
        let answer = match ty {
            FeType::List(_) | FeType::Function { .. } => true,
            FeType::Struct(name) if name.as_str() == "String" => true,
            FeType::Struct(name) => self
                .struct_fields(name)
                .iter()
                .any(|(_, field)| self.counted(field)),
            FeType::Enum(name) => self.enums.get(name).is_some_and(|info| {
                info.variants.iter().any(|case| {
                    case.stored(name)
                        .is_some_and(|stored| self.counted(&stored))
                })
            }),
            FeType::Array {
                element,
                length: Some(_),
            } => self.counted(element),
            _ => false,
        };
        self.counts.borrow_mut().counted.insert(ty.clone(), answer);
        answer
    }

    /// Stores `value` into `place` (section 12.9): the place takes a count on
    /// each buffer `value` holds and, when assigned, gives back the counts of
    /// the value it held.
    pub(crate) fn store_value(&mut self, mode: StoreMode, value: Name, ty: &FeType, place: Name) {
        if !self.counted(ty) {
            self.copy_bits(mode, value, ty, place);
            return;
        }
        if mode == StoreMode::Assign {
            // The new value is counted before the old one is released, and
            // read before the release can free the buffer it may sit in.
            let staged = self.fb.stack_alloc(ty.to_kir());
            self.copy_bits(StoreMode::Init, value, ty, staged.clone());
            self.count_value(Count::Retain, &staged, ty);
            self.count_value(Count::Release, &place, ty);
            self.copy_bits(StoreMode::Assign, staged, ty, place);
        } else {
            self.copy_bits(mode, value, ty, place.clone());
            self.count_value(Count::Retain, &place, ty);
        }
    }

    /// Stores `value` into fresh `place`: a temporary of the open statement
    /// hands over its counts, and anything else is counted again.
    pub(crate) fn init_from(&mut self, value: Name, ty: &FeType, place: Name) {
        if let Some(region) = self.regions.last_mut()
            && let Some(position) = region.iter().position(|owned| owned.storage == value)
        {
            region.remove(position);
            self.move_value(value, ty, place);
            return;
        }
        self.store_value(StoreMode::Init, value, ty, place);
    }

    /// Stores `value`, whose counts the caller hands over, into fresh `place`.
    pub(crate) fn move_value(&mut self, value: Name, ty: &FeType, place: Name) {
        self.copy_bits(StoreMode::Init, value, ty, place);
    }

    /// Takes or gives back one count on every buffer the `ty` at `place` holds.
    pub(crate) fn count_value(&mut self, count: Count, place: &Name, ty: &FeType) {
        if !self.counted(ty) {
            return;
        }
        match ty {
            FeType::Struct(name) if name.as_str() == "String" => {
                let field = self.builtin_field("String", "data");
                let buffer = self.buffer_of(place, field);
                self.count_buffer(count, buffer);
            }
            FeType::List(element) if count == Count::Retain || !self.counted(element) => {
                let field = self.builtin_field("List", "data");
                let buffer = self.buffer_of(place, field);
                self.count_buffer(count, buffer);
            }
            _ => {
                let function = self.count_function(count, ty);
                self.fb.call_void(function, vec![place.clone()]);
            }
        }
    }

    /// The body of a count function: the counts of each part of the `ty` at
    /// `place`.
    fn count_parts(&mut self, count: Count, place: &Name, ty: &FeType) {
        match ty {
            FeType::Struct(name) => {
                let fields = self.struct_fields(name);
                for (index, (_, field_ty)) in fields.iter().enumerate() {
                    if !self.counted(field_ty) {
                        continue;
                    }
                    let index = u32::try_from(index).unwrap_or(0);
                    let field = self.fb.field_addr(place.clone(), index);
                    self.count_value(count, &field, field_ty);
                }
            }
            FeType::Enum(name) => self.count_payloads(count, place, name),
            FeType::Array {
                element,
                length: Some(length),
            } => {
                for index in 0..*length {
                    let index = self.fb.const_int(Type::Integer(64), i128::from(index));
                    let slot = self.fb.elem_addr(place.clone(), index);
                    self.count_value(count, &slot, element);
                }
            }
            FeType::List(element) => self.release_elements(place, element),
            FeType::Function { .. } => self.count_closure(count, place),
            _ => {}
        }
    }

    /// The fields of a declared struct or of a closure's context.
    fn struct_fields(&self, name: &Name) -> Vec<(Name, FeType)> {
        if let Some(info) = self.structs.get(name) {
            return info.fields.clone();
        }
        self.counts
            .borrow()
            .contexts
            .get(name)
            .map(|info| info.fields.clone())
            .unwrap_or_default()
    }

    /// Takes or gives back the closure record at `place`'s count on its
    /// context. The last holder releases the captures first, through the
    /// release function the context starts with.
    fn count_closure(&mut self, count: Count, place: &Name) {
        let context = self.buffer_of(place, 1);
        if count == Count::Retain {
            self.count_buffer(count, context);
            return;
        }
        let unique = self.buffer_is_unique(context.clone());
        let captures = self.fb.new_block(&[]);
        let done = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            unique,
            captures.clone(),
            Vec::new(),
            done.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&captures);
        let words = self.fb.convert(
            ConvertKind::Bitcast,
            context.clone(),
            Type::Address(Box::new(Type::Pointer)),
        );
        let release = self.fb.load(words);
        let release = self.fb.convert(
            ConvertKind::Bitcast,
            release,
            Type::Function(klf_kir::FunctionType {
                parameters: vec![klf_kir::ParameterType {
                    context: false,
                    convention: None,
                    ty: Type::Pointer,
                }],
                result: Box::new(Type::Unit),
                failure: None,
                abi: klf_kir::FunctionAbi::Ridl,
            }),
        );
        self.fb
            .call_indirect_void(release, vec![Type::Pointer], vec![context.clone()]);
        self.fb.branch(done.clone(), Vec::new());
        self.fb.switch_to(&done);
        self.count_buffer(Count::Release, context);
    }

    /// The counts of the payload of whichever case the enum at `place` holds.
    fn count_payloads(&mut self, count: Count, place: &Name, name: &Name) {
        let Some(variants) = self.enums.get(name).map(|info| info.variants.clone()) else {
            return;
        };
        let join = self.fb.new_block(&[]);
        let mut arms = Vec::new();
        for (index, case) in variants.iter().enumerate() {
            let Some(stored) = case.stored(name).filter(|stored| self.counted(stored)) else {
                continue;
            };
            let variant = u32::try_from(index).unwrap_or(0);
            arms.push((variant, self.fb.new_block(&[]), stored));
        }
        let cases = arms
            .iter()
            .map(|(variant, block, _)| {
                (
                    *variant,
                    BlockTarget {
                        block: block.clone(),
                        arguments: Vec::new(),
                    },
                )
            })
            .collect();
        self.fb.switch_enum(
            place.clone(),
            cases,
            Some(BlockTarget {
                block: join.clone(),
                arguments: Vec::new(),
            }),
        );
        for (variant, block, stored) in arms {
            self.fb.switch_to(&block);
            let payload = self.fb.enum_payload_addr(place.clone(), variant);
            self.count_value(count, &payload, &stored);
            self.fb.branch(join.clone(), Vec::new());
        }
        self.fb.switch_to(&join);
    }

    /// Releases a list whose elements hold buffers: the elements go with the
    /// buffer only when this list was its last holder.
    fn release_elements(&mut self, header: &Name, element: &FeType) {
        let field = self.builtin_field("List", "data");
        let buffer = self.buffer_of(header, field);
        let unique = self.buffer_is_unique(buffer.clone());
        let elements = self.fb.new_block(&[]);
        let done = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            unique,
            elements.clone(),
            Vec::new(),
            done.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&elements);
        let count = self.list_count(header);
        self.counted_loop(&count, |lowerer, index| {
            let slot = lowerer.list_element_addr(header, element, index);
            lowerer.count_value(Count::Release, &slot, element);
        });
        self.fb.branch(done.clone(), Vec::new());
        self.fb.switch_to(&done);
        self.count_buffer(Count::Release, buffer);
    }

    /// The count function for `ty`, requested from the module on first use.
    pub(crate) fn count_function(&mut self, count: Count, ty: &FeType) -> Name {
        let name = Name::new(format!("__count.{}.{}", count.spelling(), type_key(ty)));
        let mut counts = self.counts.borrow_mut();
        if counts.requested.insert(name.clone()) {
            counts.pending.push((name.clone(), count, ty.clone()));
        }
        name
    }

    fn builtin_field(&self, owner: &str, field: &str) -> u32 {
        self.structs
            .get(&Name::new(owner))
            .and_then(|info| info.field(field))
            .map_or(0, |(index, _)| index)
    }

    /// The buffer pointer held in field `field` of `storage`.
    pub(crate) fn buffer_of(&mut self, storage: &Name, field: u32) -> Name {
        let place = self.fb.field_addr(storage.clone(), field);
        let raw = self.fb.load(place);
        self.fb.convert(ConvertKind::Bitcast, raw, Type::Pointer)
    }

    fn adopt_buffer(&mut self, buffer: Name) -> Name {
        self.fb
            .from_raw(Type::Reference(Box::new(Type::Integer(8))), buffer)
    }

    pub(crate) fn count_buffer(&mut self, count: Count, buffer: Name) {
        let reference = self.adopt_buffer(buffer);
        match count {
            Count::Retain => {
                let copy = self.fb.copy(reference.clone());
                let _ = self.fb.into_raw(reference);
                let _ = self.fb.into_raw(copy);
            }
            Count::Release => self.fb.destroy(reference),
        }
    }

    /// Whether `buffer` has no holder but the one asking.
    pub(crate) fn buffer_is_unique(&mut self, buffer: Name) -> Name {
        let reference = self.adopt_buffer(buffer);
        let unique = self.fb.ref_is_unique(reference.clone());
        let _ = self.fb.into_raw(reference);
        unique
    }

    // Temporaries: each statement, condition and conditional operand gets a
    // region, and what it built is released when the region closes.

    pub(crate) fn open_region(&mut self) {
        self.regions.push(Vec::new());
    }

    pub(crate) fn close_region(&mut self) {
        let region = self.regions.pop().unwrap_or_default();
        if self.reachable {
            self.release_all_of(&region);
        }
    }

    /// Records `value`, built fresh with a count on each buffer it holds, as a
    /// temporary of the innermost region.
    pub(crate) fn fresh(&mut self, value: Name, ty: FeType) -> (Name, FeType) {
        if self.counted(&ty)
            && let Some(region) = self.regions.last_mut()
        {
            region.push(Owned {
                storage: value.clone(),
                ty: ty.clone(),
            });
        }
        (value, ty)
    }

    /// Whether `value` is a temporary of the innermost region.
    pub(crate) fn is_temporary(&self, value: &Name) -> bool {
        self.regions
            .last()
            .is_some_and(|region| region.iter().any(|owned| owned.storage == *value))
    }

    /// Storage the caller owns holding `value`: a temporary of the innermost
    /// region is taken over, anything else is copied.
    pub(crate) fn take_owned(&mut self, value: Name, ty: &FeType) -> Name {
        if let Some(region) = self.regions.last_mut()
            && let Some(position) = region.iter().position(|owned| owned.storage == value)
        {
            region.remove(position);
            return value;
        }
        let storage = self.fb.stack_alloc(ty.to_kir());
        self.store_value(StoreMode::Init, value, ty, storage.clone());
        storage
    }

    /// A copy of `value` as a temporary, so that nothing the statement does to
    /// the storage `value` sits in can take it away.
    pub(crate) fn temporary_copy(&mut self, value: Name, ty: &FeType) -> Name {
        let storage = self.fb.stack_alloc(ty.to_kir());
        self.store_value(StoreMode::Init, value, ty, storage.clone());
        self.fresh(storage, ty.clone()).0
    }

    /// The value at `place` handed over as a temporary, with the counts the
    /// place held: the place is left holding no buffer, as empty lists and
    /// strings. A value whose buffers sit where emptying cannot reach, behind
    /// an enum's case or in an array, is copied instead.
    pub(crate) fn move_out(&mut self, place: Name, ty: &FeType) -> (Name, FeType) {
        if !self.counted(ty) {
            return (place, ty.clone());
        }
        if !self.clearable(ty) {
            return (self.temporary_copy(place, ty), ty.clone());
        }
        let storage = self.fb.stack_alloc(ty.to_kir());
        self.copy_bits(StoreMode::Init, place.clone(), ty, storage.clone());
        self.clear_value(&place, ty);
        self.fresh(storage, ty.clone())
    }

    /// Whether a value of type `from` can hold storage of type `to`, through
    /// fields, payloads and elements. A generic or an address may hold any.
    pub(crate) fn reaches(&self, from: &FeType, to: &FeType) -> bool {
        let mut seen = HashSet::new();
        self.reaches_from(from, to, &mut seen)
    }

    fn reaches_from(&self, from: &FeType, to: &FeType, seen: &mut HashSet<Name>) -> bool {
        if from == to {
            return true;
        }
        match from {
            FeType::Generic(_) | FeType::Address(_) | FeType::Pointer => true,
            FeType::List(element) | FeType::Array { element, .. } => {
                self.reaches_from(element, to, seen)
            }
            FeType::Struct(name) => {
                seen.insert(name.clone())
                    && self.structs.get(name).is_some_and(|info| {
                        info.fields
                            .iter()
                            .any(|(_, field)| self.reaches_from(field, to, seen))
                    })
            }
            FeType::Enum(name) => {
                seen.insert(name.clone())
                    && self.enums.get(name).is_some_and(|info| {
                        info.variants.iter().any(|variant| {
                            variant
                                .payload
                                .iter()
                                .any(|payload| self.reaches_from(payload, to, seen))
                        })
                    })
            }
            _ => false,
        }
    }

    /// Whether every buffer a `ty` holds can be let go of in place: a list or
    /// string, a struct of such, or an enum that can take a case holding
    /// none.
    fn clearable(&self, ty: &FeType) -> bool {
        match ty {
            FeType::List(_) => true,
            FeType::Struct(name) if name.as_str() == "String" => true,
            FeType::Struct(name) => self.structs.get(name).is_some_and(|info| {
                info.fields
                    .iter()
                    .all(|(_, field)| !self.counted(field) || self.clearable(field))
            }),
            FeType::Enum(name) => self.empty_case(name).is_some(),
            _ => !self.counted(ty),
        }
    }

    /// The case an enum `name` is left in once its buffers are let go of: the
    /// first without a payload, or else the first, its payload emptied.
    fn empty_case(&self, name: &Name) -> Option<(u32, Option<FeType>)> {
        let info = self.enums.get(name)?;
        if let Some(index) = info
            .variants
            .iter()
            .position(|case| case.payload.is_empty())
        {
            return Some((u32::try_from(index).ok()?, None));
        }
        let stored = info.variants.first()?.stored(name)?;
        self.clearable(&stored).then_some((0, Some(stored)))
    }

    /// Empties every list and string in the `ty` at `place`.
    fn clear_value(&mut self, place: &Name, ty: &FeType) {
        let (owner, every) = match ty {
            FeType::List(_) => (Name::new("List"), true),
            FeType::Struct(name) => (name.clone(), name.as_str() == "String"),
            FeType::Enum(name) => {
                if let Some((case, payload)) = self.empty_case(name) {
                    if let Some(payload) = payload {
                        let address = self.fb.enum_payload_addr(place.clone(), case);
                        self.clear_value(&address, &payload);
                    }
                    self.fb.enum_inject(place.clone(), case);
                }
                return;
            }
            _ => return,
        };
        let fields = self
            .structs
            .get(&owner)
            .map(|info| info.fields.clone())
            .unwrap_or_default();
        for (index, (_, field_ty)) in fields.iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(0);
            if every {
                let field = self.fb.field_addr(place.clone(), index);
                let zero = self.fb.const_zero(field_ty.to_kir());
                self.fb.store(StoreMode::Assign, zero, field);
            } else if self.counted(field_ty) {
                let field = self.fb.field_addr(place.clone(), index);
                self.clear_value(&field, field_ty);
            }
        }
    }

    /// Makes `value` safe to hold for the rest of the statement when `expr`
    /// reads it out of a list's buffer.
    pub(crate) fn detach(&mut self, expr: &Expr, value: Name, ty: &FeType) -> Name {
        if is_aggregate(ty) && reads_through_index(expr) {
            self.temporary_copy(value, ty)
        } else {
            value
        }
    }

    /// Records storage a scope owns: released when the scope ends.
    pub(crate) fn own_in_scope(&mut self, storage: Name, ty: FeType) {
        if self.counted(&ty)
            && let Some(scope) = self.owned.last_mut()
        {
            scope.push(Owned { storage, ty });
        }
    }

    pub(crate) fn release_all_of(&mut self, owned: &[Owned]) {
        for Owned { storage, ty } in owned.iter().rev() {
            self.count_value(Count::Release, storage, ty);
        }
    }

    /// Releases every region and scope opened since the given depths, for a
    /// jump out of them.
    pub(crate) fn release_since(&mut self, regions: usize, scopes: usize) {
        let pending: Vec<(Name, FeType)> = self.regions[regions.min(self.regions.len())..]
            .iter()
            .rev()
            .chain(self.owned[scopes.min(self.owned.len())..].iter().rev())
            .flat_map(|owned| owned.iter().rev())
            .map(|owned| (owned.storage.clone(), owned.ty.clone()))
            .collect();
        for (storage, ty) in pending {
            self.count_value(Count::Release, &storage, &ty);
        }
    }

    /// Releases everything the function holds, before it returns.
    pub(crate) fn release_all(&mut self) {
        self.release_since(0, 0);
    }
}

/// A function-name-safe spelling of `ty`, distinct for distinct types.
fn type_key(ty: &FeType) -> String {
    match ty {
        FeType::Struct(name) => format!("s{}{name}", name.as_str().len()),
        FeType::Enum(name) => format!("e{}{name}", name.as_str().len()),
        FeType::List(element) => format!("l{}", type_key(element)),
        FeType::Function { .. } => "closure".to_owned(),
        FeType::Array { element, length } => {
            format!("a{}_{}", length.unwrap_or(0), type_key(element))
        }
        other => other.display(),
    }
}
