//! Kept overrides (section 14.3): each one an `OpSpecConstant*` with its
//! `SpecId`, and a workgroup size that names one a `WorkgroupSize` built-in
//! the host can specialize.

use crate::Writer;
use crate::words::{decoration, op};
use klf_shader::{Expression, Handle, Literal, Override, Scalar, ScalarKind, WorkgroupExtent};

/// The `WorkgroupSize` built-in.
const WORKGROUP_SIZE: u32 = 25;

impl Writer<'_> {
    /// The spec constant of a kept override, declared on first use.
    pub(crate) fn spec_constant(&mut self, handle: Handle<Override>) -> u32 {
        if let Some(id) = self.overrides.get(&handle.index()) {
            return *id;
        }
        let module = self.module;
        let item = &module.overrides[handle];
        let span = module.overrides.span(handle);
        let scalar = module.types[item.ty].inner.scalar().unwrap_or(Scalar::U32);
        let default = match item.init.map(|init| &module.global_expressions[init]) {
            Some(Expression::Literal(literal)) => *literal,
            None => zero(scalar),
            Some(_) => {
                self.refuse("a kept override whose default is computed", span);
                zero(scalar)
            }
        };
        let ty = self.scalar(scalar);
        let id = self.id();
        match default {
            Literal::Bool(true) => self.declare(op::SPEC_CONSTANT_TRUE, &[ty, id]),
            Literal::Bool(false) => self.declare(op::SPEC_CONSTANT_FALSE, &[ty, id]),
            literal => {
                let mut operands = vec![ty, id];
                operands.extend(crate::types::literal_words(literal));
                self.declare(op::SPEC_CONSTANT, &operands);
            }
        }
        match item.id {
            Some(spec) => self.decorate(id, decoration::SPEC_ID, &[u32::from(spec)]),
            None => self.refuse("a kept override without an id", span),
        }
        self.name(id, &item.name);
        self.overrides.insert(handle.index(), id);
        id
    }

    /// Declares the `WorkgroupSize` built-in when an extent names an
    /// override; the `LocalSize` mode then only gives its defaults.
    pub(crate) fn workgroup_size(&mut self, extents: [WorkgroupExtent; 3]) {
        if !extents
            .iter()
            .any(|extent| matches!(extent, WorkgroupExtent::Override(_)))
        {
            return;
        }
        let mut parts = Vec::new();
        for extent in extents {
            parts.push(match extent {
                WorkgroupExtent::Fixed(value) => self.literal(Literal::U32(value)),
                WorkgroupExtent::Override(expression) => self.override_extent(expression),
            });
        }
        let ty = self.vector(3, Scalar::U32);
        let id = self.id();
        let mut operands = vec![ty, id];
        operands.extend(parts);
        self.declare(op::SPEC_CONSTANT_COMPOSITE, &operands);
        self.decorate(id, decoration::BUILT_IN, &[WORKGROUP_SIZE]);
    }
}

impl Writer<'_> {
    /// An extent's override expression: an override, or a literal.
    fn override_extent(&mut self, expression: Handle<Expression>) -> u32 {
        match self.module.global_expressions[expression] {
            Expression::Override(item) => self.spec_constant(item),
            Expression::Literal(literal) => self.literal(literal),
            _ => {
                let span = self.module.global_expressions.span(expression);
                self.refuse("a workgroup size computed from kept overrides", span);
                self.literal(Literal::U32(1))
            }
        }
    }
}

fn zero(scalar: Scalar) -> Literal {
    match (scalar.kind, scalar.width) {
        (ScalarKind::Bool, _) => Literal::Bool(false),
        (ScalarKind::Sint, 8) => Literal::I64(0),
        (ScalarKind::Sint, _) => Literal::I32(0),
        (ScalarKind::Uint, 8) => Literal::U64(0),
        (ScalarKind::Uint, _) => Literal::U32(0),
        (ScalarKind::Float, 2) => Literal::F16(0),
        (ScalarKind::Float, 8) => Literal::f64(0.0),
        (ScalarKind::Float, _) => Literal::f32(0.0),
    }
}
