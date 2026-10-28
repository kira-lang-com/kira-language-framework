//! Places: the address and type of an assignable location.
//!
//! An aggregate value is represented by the address of its storage, so a place
//! is what both reads and writes go through.

use super::{Access, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{ConvertKind, IntegerPredicate, Type};
use klf_kira_syntax::ast::{Expr, Ident, Path};
use klf_source::Span;

impl Lowerer<'_> {
    /// Load a scalar from `address`, or keep the address itself for aggregate
    /// values (whose runtime representation is their storage address).
    pub(crate) fn load_place(&mut self, address: Name, ty: FeType) -> (Name, FeType) {
        if super::lists::is_aggregate(&ty) {
            (address, ty)
        } else {
            (self.fb.load(address), ty)
        }
    }

    pub(crate) fn index_place(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> Option<(Name, FeType)> {
        self.index_place_for(base, index, span, false)
    }

    /// The place of `base[index]`; for a `write`, every list on the way to it
    /// first takes a buffer of its own.
    pub(crate) fn index_place_for(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
        write: bool,
    ) -> Option<(Name, FeType)> {
        let (base_value, base_ty) = self.place_base(base, write)?;
        let (index_value, index_ty) = self.lower_expr(index)?;
        if !matches!(index_ty, FeType::Int { .. }) {
            self.session
                .report(type_error(index.span, "array index must be an integer"));
            return None;
        }
        let index_value = self.wide_index(index_value, &index_ty);
        match base_ty {
            FeType::List(element) => {
                let element = *element;
                let count = self.list_count(&base_value);
                self.check_index(&index_value, count, index.span, "list");
                if write {
                    self.list_make_unique(&base_value, &element);
                }
                let address = self.list_element_addr(&base_value, &element, index_value);
                Some((address, element))
            }
            FeType::Array { element, length } => {
                if let Some(length) = length {
                    let count = self.fb.const_int(Type::Integer(64), i128::from(length));
                    self.check_index(&index_value, count, index.span, "array");
                }
                Some((self.fb.elem_addr(base_value, index_value), *element))
            }
            FeType::Struct(name) if name.as_str() == "String" => {
                let info = self.structs.get(&name)?.clone();
                let (len_field, _) = info.field("len")?;
                let len_addr = self.fb.field_addr(base_value.clone(), len_field);
                let count = self.fb.load(len_addr);
                self.check_index(&index_value, count, index.span, "String");
                let (field, _) = info.field("data")?;
                let data_addr = self.fb.field_addr(base_value, field);
                let data = self.fb.load(data_addr);
                let address = self.fb.elem_addr(data, index_value);
                Some((
                    address,
                    FeType::Int {
                        bits: 8,
                        signed: false,
                    },
                ))
            }
            _ => {
                self.session.report(type_error(
                    span,
                    "indexing requires an array or String value",
                ));
                None
            }
        }
    }

    /// An index as 64 bits, the width a count is compared at.
    fn wide_index(&mut self, index: Name, ty: &FeType) -> Name {
        match ty {
            FeType::Int { bits, signed } if *bits < 64 => {
                let kind = if *signed {
                    ConvertKind::SignExtend
                } else {
                    ConvertKind::ZeroExtend
                };
                self.fb.convert(kind, index, Type::Integer(64))
            }
            _ => index,
        }
    }

    /// Trap unless `0 <= index < count`; the unsigned comparison takes a
    /// negative index as past the end. `indexed` names what is indexed.
    fn check_index(&mut self, index: &Name, count: Name, span: Span, indexed: &str) {
        let within = self
            .fb
            .compare(IntegerPredicate::UnsignedLt, index.clone(), count.clone());
        let note = format!(
            "a {indexed} index must be at least 0 and less than the {indexed}'s count"
        );
        let text = super::trap_report::TrapText {
            code: "KLF-RT-0005",
            headline: "index out of range: the index is {0} but the count is {1}",
            label: "this index",
            note: &note,
            help: "compare the index with `.count` before indexing, or walk the elements with \
                   `for element in elements`",
        };
        self.trap_unless_reported(within, span, &text, [index.clone(), count]);
    }

    /// Compute the address and type of a dotted path `a.b.c` where `a` is a
    /// local struct and `b`, `c` are field accesses.
    pub(crate) fn path_place(
        &mut self,
        path: &klf_kira_syntax::ast::Path,
        span: Span,
    ) -> Option<(Name, FeType)> {
        if let Some(path) = self.implicit_self(path) {
            return self.path_place(&path, span);
        }
        let base = &path.segments[0].name;
        let Some(local) = self.lookup(base) else {
            self.session.report(
                Diagnostic::error("KLFK-E0200", format!("unknown name `{base}`"))
                    .with_label(span, "not found"),
            );
            return None;
        };
        // For an aggregate `var`, the slot holds the storage address, so load it
        // to get the base rather than treating the slot itself as the base.
        let (base_ref, ty, aggregate_slot) = match &local.access {
            Access::Ssa(value) => (value.clone(), local.ty.clone(), false),
            Access::Slot(slot) => {
                let aggregate = super::lists::is_aggregate(&local.ty);
                (slot.clone(), local.ty.clone(), aggregate)
            }
        };
        let mut address = if aggregate_slot {
            self.fb.load(base_ref)
        } else {
            base_ref
        };
        let mut ty = ty;
        for segment in &path.segments[1..] {
            let Some(name) = struct_header_name(&ty) else {
                self.session.report(type_error(
                    segment.span,
                    "field access requires a struct value",
                ));
                return None;
            };
            let (field_address, field_ty) = self.member_place(address, &name, segment)?;
            address = field_address;
            ty = field_ty;
        }
        Some((address, ty))
    }

    /// A method body may name a member of its receiver bare: `n` is `self.n`
    /// when no local or constant is called `n`.
    pub(crate) fn implicit_self(&self, path: &Path) -> Option<Path> {
        let first = path.segments.first()?;
        if self.lookup(&first.name).is_some() || self.constants.contains_key(&first.name) {
            return None;
        }
        let FeType::Struct(owner) = &self.lookup(&Name::new("self"))?.ty else {
            return None;
        };
        crate::structs::member(self.structs, owner, first.name.as_str())?;
        let receiver = Ident {
            name: Name::new("self"),
            span: first.span,
        };
        let segments = std::iter::once(receiver)
            .chain(path.segments.iter().cloned())
            .collect();
        Some(Path {
            segments,
            span: path.span,
        })
    }

    /// Compute the address and type of `base.field`.
    pub(crate) fn field_place(
        &mut self,
        base: &Expr,
        field: &klf_kira_syntax::ast::Ident,
        span: Span,
    ) -> Option<(Name, FeType)> {
        self.field_place_for(base, field, span, false)
    }

    pub(crate) fn field_place_for(
        &mut self,
        base: &Expr,
        field: &klf_kira_syntax::ast::Ident,
        span: Span,
        write: bool,
    ) -> Option<(Name, FeType)> {
        let (base_value, base_ty) = self.place_base(base, write)?;
        let Some(name) = struct_header_name(&base_ty) else {
            self.session
                .report(type_error(span, "field access requires a struct value"));
            return None;
        };
        self.member_place(base_value, &name, field)
    }

    /// The address and type of member `field` of the `owner` at `address`: a
    /// field, or for a wrapped property its backing field's `wrappedValue`.
    pub(crate) fn member_place(
        &mut self,
        address: Name,
        owner: &Name,
        field: &klf_kira_syntax::ast::Ident,
    ) -> Option<(Name, FeType)> {
        let Some(member) = crate::structs::member(self.structs, owner, field.name.as_str()) else {
            self.session.report(type_error(
                field.span,
                format!("`{owner}` has no field `{}`", field.name),
            ));
            return None;
        };
        let mut address = address;
        let mut ty = FeType::Struct(owner.clone());
        for (index, step_ty) in member {
            address = self.fb.field_addr(address, index);
            ty = step_ty;
        }
        Some((address, ty))
    }
}

impl Lowerer<'_> {
    /// The value a place is inside of. A write reaches it through places, so
    /// the lists it passes are made unique; a read lowers it as an expression.
    fn place_base(&mut self, base: &Expr, write: bool) -> Option<(Name, FeType)> {
        if write {
            match &base.kind {
                klf_kira_syntax::ast::ExprKind::Index { base: inner, index } => {
                    let (address, ty) = self.index_place_for(inner, index, base.span, true)?;
                    return Some(self.load_place(address, ty));
                }
                klf_kira_syntax::ast::ExprKind::Field { base: inner, name } => {
                    let (address, ty) = self.field_place_for(inner, name, base.span, true)?;
                    return Some(self.load_place(address, ty));
                }
                _ => {}
            }
        }
        self.lower_expr(base)
    }
}

fn struct_header_name(ty: &FeType) -> Option<Name> {
    match ty {
        FeType::Struct(name) => Some(name.clone()),
        FeType::List(_) => Some(Name::new("List")),
        _ => None,
    }
}
