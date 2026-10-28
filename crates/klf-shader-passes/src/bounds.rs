//! Out-of-bounds access (section 15.5, section 17.1 step 5). Every index
//! that is not a constant, into an array, vector or matrix, and every image
//! coordinate, array layer, sample and level, is clamped to the last valid
//! one. Under `read-zero-skip-write` the access is still clamped, so it
//! stays in memory, and the condition it was in range then makes a read of
//! a scalar or vector zero and drops a store.
//!
//! An out-of-range read of a whole struct or array is clamped, not zeroed:
//! `select` takes only scalars and vectors on every target.

use crate::rewrite::rewrite;
use klf_shader::resolve::resolve_function;
use klf_shader::{
    ArraySize, BinaryOp, Block, Expression, Function, Handle, ImageQuery, Literal, MathFunction,
    Module, Override, RelationalFunction, Scalar, ScalarKind, Statement, Type, TypeInner,
};
use std::collections::HashMap;

/// The `--bounds` policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bounds {
    #[default]
    Unchecked,
    Restrict,
    ReadZeroSkipWrite,
}

type Shape = Option<(Option<u8>, Scalar)>;

/// What an index runs up to.
#[derive(Clone, Copy)]
enum Extent {
    Fixed(u32),
    Runtime,
    Override(Handle<Override>),
}

/// What the expression rewrite knows about one old expression.
struct Known {
    shape: Shape,
    pointer: bool,
    extent: Option<Extent>,
    /// The type a zero of this value has, when a read of it can be zeroed.
    zero: Option<Handle<Type>>,
}

/// Guards every function of `module` under `bounds`.
pub fn guard_bounds(module: &mut Module, bounds: Bounds) {
    if bounds == Bounds::Unchecked {
        return;
    }
    let zeroing = bounds == Bounds::ReadZeroSkipWrite;
    for which in module.function_refs() {
        let Ok(resolved) = resolve_function(module, module.function(which)) else {
            continue;
        };
        let mut known = Vec::with_capacity(resolved.len());
        for resolution in &resolved {
            let inner = resolution.inner(&module.types).clone();
            let selectable = inner.scalar_or_vector().is_some();
            known.push(Known {
                shape: inner.scalar_or_vector(),
                pointer: matches!(inner, TypeInner::Pointer { .. }),
                extent: extent(module, &inner),
                zero: (zeroing && selectable).then(|| resolution.handle(&mut module.types)),
            });
        }
        let function = module.function_mut(which);
        let mut stores = HashMap::new();
        store_operands(&function.body, &mut stores);
        let guarded = guard_expressions(function, &known, &stores, zeroing);
        let mut body = std::mem::take(&mut function.body);
        guard_statements(&mut body, &guarded, zeroing);
        function.body = body;
    }
}

/// Which image an image store's coordinate or array layer addresses, and
/// the query that counts it, by the operand's old handle.
type StoreOperands = HashMap<Handle<Expression>, (Handle<Expression>, bool)>;

fn store_operands(block: &Block, stores: &mut StoreOperands) {
    for (statement, _) in block.iter() {
        match statement {
            Statement::Block(inner) => store_operands(inner, stores),
            Statement::If { accept, reject, .. } => {
                store_operands(accept, stores);
                store_operands(reject, stores);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    store_operands(&case.body, stores);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                store_operands(body, stores);
                store_operands(continuing, stores);
            }
            Statement::ImageStore {
                image,
                coordinate,
                array_index,
                ..
            } => {
                stores.insert(*coordinate, (*image, false));
                if let Some(layer) = array_index {
                    stores.insert(*layer, (*image, true));
                }
            }
            _ => {}
        }
    }
}

/// What the expression rewrite leaves for the statements: the condition
/// each pointer or composite is in range under, and each image store
/// operand's clamped value with its own condition.
struct Guarded {
    conditions: HashMap<Handle<Expression>, Handle<Expression>>,
    clamped: HashMap<Handle<Expression>, Clamped>,
}

type Clamped = (Handle<Expression>, Option<Handle<Expression>>);

fn extent(module: &Module, inner: &TypeInner) -> Option<Extent> {
    let pointee = match inner {
        TypeInner::Pointer { base, .. } => &module.types[*base].inner,
        other => other,
    };
    match pointee {
        TypeInner::Array { size, .. } => Some(match size {
            ArraySize::Constant(count) => Extent::Fixed(*count),
            ArraySize::Runtime => Extent::Runtime,
            ArraySize::Override(item) => Extent::Override(*item),
        }),
        TypeInner::Vector { size, .. } => Some(Extent::Fixed(u32::from(*size))),
        TypeInner::Matrix { columns, .. } => Some(Extent::Fixed(u32::from(*columns))),
        _ => None,
    }
}

fn number(kind: ScalarKind, value: u32) -> Literal {
    match kind {
        ScalarKind::Sint => Literal::I32(value.cast_signed()),
        _ => Literal::U32(value),
    }
}

/// Builds guard expressions through `push`, which places each new one.
struct Guard<'a> {
    push: &'a mut dyn FnMut(Expression) -> Handle<Expression>,
}

impl Guard<'_> {
    fn push(&mut self, expression: Expression) -> Handle<Expression> {
        (self.push)(expression)
    }

    fn constant(&mut self, kind: ScalarKind, value: u32, size: Option<u8>) -> Handle<Expression> {
        let scalar = self.push(Expression::Literal(number(kind, value)));
        match size {
            Some(size) => self.push(Expression::Splat {
                size,
                value: scalar,
            }),
            None => scalar,
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: Handle<Expression>,
        right: Handle<Expression>,
    ) -> Handle<Expression> {
        self.push(Expression::Binary { op, left, right })
    }

    /// Both conditions, either of which may be absent.
    fn both(
        &mut self,
        first: Option<Handle<Expression>>,
        second: Option<Handle<Expression>>,
    ) -> Option<Handle<Expression>> {
        match (first, second) {
            (Some(first), Some(second)) => Some(self.binary(BinaryOp::LogicalAnd, first, second)),
            (first, second) => first.or(second),
        }
    }

    /// `index`, of `kind` and `size` lanes, clamped below `count` of the
    /// same shape, and whether it was below it.
    fn clamp(
        &mut self,
        index: Handle<Expression>,
        count: Handle<Expression>,
        kind: ScalarKind,
        size: Option<u8>,
    ) -> (Handle<Expression>, Handle<Expression>) {
        let one = self.constant(kind, 1, size);
        let last = self.binary(BinaryOp::Subtract, count, one);
        let below = self.binary(BinaryOp::Less, index, count);
        let (clamped, inside) = if kind == ScalarKind::Sint {
            let zero = self.constant(kind, 0, size);
            let clamped = self.push(Expression::Math {
                fun: MathFunction::Clamp,
                args: vec![index, zero, last],
            });
            let above = self.binary(BinaryOp::GreaterEqual, index, zero);
            let inside = match size {
                Some(_) => self.binary(BinaryOp::And, above, below),
                None => self.binary(BinaryOp::LogicalAnd, above, below),
            };
            (clamped, inside)
        } else {
            let clamped = self.push(Expression::Math {
                fun: MathFunction::Min,
                args: vec![index, last],
            });
            (clamped, below)
        };
        let inside = match size {
            Some(_) => self.push(Expression::Relational {
                fun: RelationalFunction::All,
                argument: inside,
            }),
            None => inside,
        };
        (clamped, inside)
    }

    /// An image query's `u32` result in an operand's shape.
    fn counted(&mut self, query: Handle<Expression>, kind: ScalarKind) -> Handle<Expression> {
        if kind == ScalarKind::Sint {
            self.push(Expression::Convert {
                expr: query,
                kind,
                width: Some(4),
            })
        } else {
            query
        }
    }

    /// One image operand clamped against the query that counts it.
    fn image_operand(
        &mut self,
        image: Handle<Expression>,
        operand: Handle<Expression>,
        shape: Shape,
        query: ImageQuery,
    ) -> (Handle<Expression>, Option<Handle<Expression>>) {
        let Some((size, scalar)) = shape else {
            return (operand, None);
        };
        let count = self.push(Expression::ImageQuery { image, query });
        let count = self.counted(count, scalar.kind);
        let (clamped, inside) = self.clamp(operand, count, scalar.kind, size);
        (clamped, Some(inside))
    }
}

/// The operands of an image access, clamped, and whether all were in range.
struct ImageOperands {
    coordinate: Handle<Expression>,
    array_index: Option<Handle<Expression>>,
    sample: Option<Handle<Expression>>,
    level: Option<Handle<Expression>>,
    inside: Option<Handle<Expression>>,
}

/// Clamps an image access's operands; `shapes` gives each one's shape.
fn image_operands(
    guard: &mut Guard<'_>,
    image: Handle<Expression>,
    operands: [Option<(Handle<Expression>, Shape)>; 4],
) -> Option<ImageOperands> {
    let [coordinate, array_index, sample, level] = operands;
    let (coordinate, coordinate_shape) = coordinate?;
    let mut inside = None;
    let mut clamp_with =
        |guard: &mut Guard<'_>, operand: Option<(Handle<Expression>, Shape)>, query| {
            operand.map(|(operand, shape)| {
                let (clamped, within) = guard.image_operand(image, operand, shape, query);
                inside = guard.both(inside, within);
                clamped
            })
        };
    let level = clamp_with(guard, level, ImageQuery::NumLevels);
    let sample = clamp_with(guard, sample, ImageQuery::NumSamples);
    let array_index = clamp_with(guard, array_index, ImageQuery::NumLayers);
    let coordinate = clamp_with(
        guard,
        Some((coordinate, coordinate_shape)),
        ImageQuery::Size { level },
    )?;
    Some(ImageOperands {
        coordinate,
        array_index,
        sample,
        level,
        inside,
    })
}

/// Rewrites every access and image load; returns, for each new pointer or
/// composite that may be out of range, the condition that it is not.
fn guard_expressions(
    function: &mut Function,
    known: &[Known],
    stores: &StoreOperands,
    zeroing: bool,
) -> Guarded {
    let old = function.expressions.clone();
    let mut conditions: HashMap<Handle<Expression>, Handle<Expression>> = HashMap::new();
    let mut clamped_stores = HashMap::new();
    rewrite(function, |inserter, handle, mapped| {
        let mut push = |expression| inserter.push(expression);
        let mut guard = Guard { push: &mut push };
        let (result, inside) = match (&old[handle], mapped) {
            (
                Expression::Access {
                    base: old_base,
                    index: old_index,
                },
                Expression::Access { base, index },
            ) => {
                let extent = known[old_base.index()].extent?;
                let (_, scalar) = known[old_index.index()].shape?;
                let (access, inside) = guard.access(*base, *index, extent, scalar.kind);
                let inside = zeroing.then_some(inside);
                (access, guard.both(conditions.get(base).copied(), inside))
            }
            (Expression::AccessIndex { .. }, Expression::AccessIndex { base, .. })
            | (Expression::Load { .. }, Expression::Load { pointer: base })
                if conditions.contains_key(base) =>
            {
                (guard.push(mapped.clone()), conditions.get(base).copied())
            }
            (
                Expression::ImageLoad {
                    coordinate: old_coordinate,
                    array_index: old_array_index,
                    sample: old_sample,
                    level: old_level,
                    ..
                },
                Expression::ImageLoad {
                    image,
                    coordinate,
                    array_index,
                    sample,
                    level,
                },
            ) => {
                let shape = |old: Handle<Expression>| known[old.index()].shape;
                let clamped = image_operands(
                    &mut guard,
                    *image,
                    [
                        Some((*coordinate, shape(*old_coordinate))),
                        array_index.zip(old_array_index.map(shape)),
                        sample.zip(old_sample.map(shape)),
                        level.zip(old_level.map(shape)),
                    ],
                )?;
                let load = guard.push(Expression::ImageLoad {
                    image: *image,
                    coordinate: clamped.coordinate,
                    array_index: clamped.array_index,
                    sample: clamped.sample,
                    level: clamped.level,
                });
                (load, clamped.inside.filter(|_| zeroing))
            }
            _ if stores.contains_key(&handle) => (guard.push(mapped.clone()), None),
            _ => return None,
        };
        let entry = &known[handle.index()];
        let value = match (inside, entry.zero.filter(|_| !entry.pointer)) {
            (None, _) => result,
            (Some(inside), Some(ty)) => {
                let zero = guard.push(Expression::ZeroValue(ty));
                guard.push(Expression::Select {
                    condition: inside,
                    accept: result,
                    reject: zero,
                })
            }
            (Some(inside), None) => {
                conditions.insert(result, inside);
                result
            }
        };
        // A store's operand is clamped beside it; its other uses keep it.
        if let Some(&(image, layer)) = stores.get(&handle)
            && let Some((clamped, inside)) =
                guard.store_operand(&old[image], value, entry.shape, layer)
        {
            clamped_stores.insert(value, (clamped, inside.filter(|_| zeroing)));
        }
        Some(value)
    });
    Guarded {
        conditions,
        clamped: clamped_stores,
    }
}

impl Guard<'_> {
    /// `base[index]` with the index clamped below the extent, and whether
    /// it was below it.
    fn access(
        &mut self,
        base: Handle<Expression>,
        index: Handle<Expression>,
        extent: Extent,
        kind: ScalarKind,
    ) -> (Handle<Expression>, Handle<Expression>) {
        let count = self.push(match extent {
            Extent::Fixed(count) => Expression::Literal(Literal::U32(count)),
            Extent::Runtime => Expression::ArrayLength(base),
            Extent::Override(item) => Expression::Override(item),
        });
        let index = self.counted_index(index, kind);
        let (clamped, inside) = self.clamp(index, count, ScalarKind::Uint, None);
        let access = self.push(Expression::Access {
            base,
            index: clamped,
        });
        (access, inside)
    }

    /// An image store's coordinate or array layer clamped against the
    /// image, which must be one a fresh expression can name again.
    fn store_operand(
        &mut self,
        image: &Expression,
        operand: Handle<Expression>,
        shape: Shape,
        layer: bool,
    ) -> Option<Clamped> {
        if !matches!(
            image,
            Expression::GlobalVariable(_) | Expression::FunctionArgument(_)
        ) {
            return None;
        }
        let image = self.push(image.clone());
        let query = if layer {
            ImageQuery::NumLayers
        } else {
            ImageQuery::Size { level: None }
        };
        Some(self.image_operand(image, operand, shape, query))
    }

    /// An index as `u32`, so one comparison bounds it on both sides.
    fn counted_index(&mut self, index: Handle<Expression>, kind: ScalarKind) -> Handle<Expression> {
        if kind == ScalarKind::Sint {
            self.push(Expression::Convert {
                expr: index,
                kind: ScalarKind::Uint,
                width: Some(4),
            })
        } else {
            index
        }
    }
}

/// Gives image stores their clamped operands, and under
/// `read-zero-skip-write` drops each store that may be out of range.
fn guard_statements(block: &mut Block, guarded: &Guarded, zeroing: bool) {
    let statements = std::mem::take(&mut block.statements);
    let spans = std::mem::take(&mut block.spans);
    for (mut statement, span) in statements.into_iter().zip(spans) {
        let mut conditions = Vec::new();
        match &mut statement {
            Statement::Block(inner) => guard_statements(inner, guarded, zeroing),
            Statement::If { accept, reject, .. } => {
                guard_statements(accept, guarded, zeroing);
                guard_statements(reject, guarded, zeroing);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    guard_statements(&mut case.body, guarded, zeroing);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                guard_statements(body, guarded, zeroing);
                guard_statements(continuing, guarded, zeroing);
            }
            Statement::Store { pointer, .. }
            | Statement::Atomic {
                pointer,
                result: None,
                ..
            } => conditions.extend(guarded.conditions.get(pointer).filter(|_| zeroing)),
            Statement::ImageStore {
                coordinate,
                array_index,
                ..
            } => {
                for operand in std::iter::once(coordinate).chain(array_index.as_mut()) {
                    if let Some((clamped, inside)) = guarded.clamped.get(operand) {
                        *operand = *clamped;
                        conditions.extend(*inside);
                    }
                }
            }
            _ => {}
        }
        for condition in conditions {
            let mut accept = Block::new();
            accept.push(statement, span);
            statement = Statement::If {
                condition,
                accept,
                reject: Block::new(),
            };
        }
        block.push(statement, span);
    }
}
