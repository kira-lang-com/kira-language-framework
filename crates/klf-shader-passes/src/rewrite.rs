//! Rebuilding a function's expression arena with new expressions placed
//! before the ones they serve, keeping every `emit` exact (rule SV7).

use klf_shader::visit::map_statement;
use klf_shader::{Arena, Block, Expression, Function, Handle, Range, Span, Statement};

/// Appends new expressions while a rewrite visits an old one. Each gets
/// the span of the expression it serves.
pub struct Inserter<'a> {
    expressions: &'a mut Arena<Expression>,
    span: Span,
}

impl Inserter<'_> {
    pub fn push(&mut self, expression: Expression) -> Handle<Expression> {
        self.expressions.append(expression, self.span)
    }
}

/// Rebuilds `function`'s expressions. `replace` sees each old expression
/// with its operands already mapped; it may push new ones and return the
/// handle that stands for the old expression, or `None` to keep it.
pub fn rewrite(
    function: &mut Function,
    mut replace: impl FnMut(
        &mut Inserter<'_>,
        Handle<Expression>,
        &Expression,
    ) -> Option<Handle<Expression>>,
) {
    let old = std::mem::take(&mut function.expressions);
    let mut new = Arena::new();
    let mut map: Vec<Handle<Expression>> = Vec::with_capacity(old.len());
    let mut slices = Vec::with_capacity(old.len());
    for (handle, expression) in old.iter() {
        let mut mapped = expression.clone();
        mapped.map_operands(&mut |operand| map[operand.index()]);
        let start = new.len() as u32;
        let span = old.span(handle);
        let mut inserter = Inserter {
            expressions: &mut new,
            span,
        };
        let result = match replace(&mut inserter, handle, &mapped) {
            Some(result) => result,
            None => new.append(mapped, span),
        };
        map.push(result);
        slices.push((start, new.len() as u32));
    }
    let remap = Remap {
        map: &map,
        slices: &slices,
        arena: &new,
    };
    remap.block(&mut function.body);
    for (_, local) in function.locals.iter_mut() {
        if let Some(init) = &mut local.init {
            *init = map[init.index()];
        }
    }
    function.named_expressions = std::mem::take(&mut function.named_expressions)
        .into_iter()
        .map(|(index, name)| (map[index as usize].raw(), name))
        .collect();
    function.expressions = new;
}

struct Remap<'a> {
    map: &'a [Handle<Expression>],
    slices: &'a [(u32, u32)],
    arena: &'a Arena<Expression>,
}

impl Remap<'_> {
    fn block(&self, block: &mut Block) {
        let statements = std::mem::take(&mut block.statements);
        let spans = std::mem::take(&mut block.spans);
        for (mut statement, span) in statements.into_iter().zip(spans) {
            match &mut statement {
                Statement::Emit(range) => {
                    if range.is_empty() {
                        continue;
                    }
                    let start = self.slices[range.start as usize].0;
                    let end = self.slices[range.end as usize - 1].1;
                    self.emits(start, end, span, block);
                    continue;
                }
                Statement::Block(inner) => self.block(inner),
                Statement::If { accept, reject, .. } => {
                    self.block(accept);
                    self.block(reject);
                }
                Statement::Switch { cases, .. } => {
                    for case in cases {
                        self.block(&mut case.body);
                    }
                }
                Statement::Loop {
                    body, continuing, ..
                } => {
                    self.block(body);
                    self.block(continuing);
                }
                _ => {}
            }
            map_statement(&mut statement, &mut |handle| self.map[handle.index()]);
            block.push(statement, span);
        }
    }

    /// One `emit` per run of expressions in `start..end` that need one.
    fn emits(&self, start: u32, end: u32, span: Span, block: &mut Block) {
        let mut run: Option<u32> = None;
        for index in start..=end {
            let emitted = index < end && self.arena[Handle::new(index)].needs_emit();
            match (emitted, run) {
                (true, None) => run = Some(index),
                (false, Some(first)) => {
                    block.push(Statement::Emit(Range::new(first, index)), span);
                    run = None;
                }
                _ => {}
            }
        }
    }
}

/// Emits `handles`, which are consecutive new expressions, as one or more
/// `emit` statements.
#[must_use]
pub fn emit_all(
    arena: &Arena<Expression>,
    start: Handle<Expression>,
    end: Handle<Expression>,
    span: Span,
) -> Block {
    let remap = Remap {
        map: &[],
        slices: &[],
        arena,
    };
    let mut block = Block::new();
    remap.emits(start.raw(), end.raw(), span, &mut block);
    block
}
