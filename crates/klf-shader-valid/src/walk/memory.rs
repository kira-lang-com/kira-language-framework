//! Statements that touch memory: stores, image stores, atomics and calls.

use super::Walker;
use klf_shader::resolve::Resolution;
use klf_shader::{AddressSpace, AtomicFunction, Expression, Handle, ImageClass, Span, TypeInner};

impl Walker<'_> {
    pub(super) fn store(
        &mut self,
        pointer: Handle<Expression>,
        value: Handle<Expression>,
        span: Span,
    ) {
        if !(self.use_expression(pointer, span) & self.use_expression(value, span)) {
            return;
        }
        match *self.ty(pointer) {
            TypeInner::Pointer { base, space } => {
                if !space.stores() {
                    self.fail(
                        "SV9",
                        "a store through a pointer that cannot be written",
                        span,
                    );
                } else if matches!(self.module.types[base].inner, TypeInner::Atomic(_)) {
                    self.fail("SV9", "a store to an atomic; use an atomic exchange", span);
                } else if !self.info.expressions[value.index()]
                    .ty
                    .same(&Resolution::Handle(base), &self.module.types)
                {
                    self.fail("SV6", "a stored value has the pointee's type", span);
                }
            }
            _ => self.fail("SV9", "a store through a value that is not a pointer", span),
        }
        self.global_use(pointer, false, true);
    }

    pub(super) fn image_store(
        &mut self,
        image: Handle<Expression>,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
        value: Handle<Expression>,
        span: Span,
    ) {
        let mut used = self.use_expression(image, span)
            & self.use_expression(coordinate, span)
            & self.use_expression(value, span);
        if let Some(index) = array_index {
            used &= self.use_expression(index, span);
        }
        if !used {
            return;
        }
        match self.ty(image) {
            TypeInner::Image {
                class: ImageClass::Storage { access, .. },
                ..
            } if access.store => {}
            _ => self.fail(
                "SV23",
                "image_store writes a storage image with `store` access",
                span,
            ),
        }
        let coordinates_fit = matches!(
            self.ty(coordinate).scalar_or_vector(),
            Some((_, scalar)) if scalar.kind.is_integer()
        );
        if !coordinates_fit {
            self.fail("SV23", "image_store takes integer coordinates", span);
        }
        self.global_use(image, false, true);
    }

    pub(super) fn atomic(
        &mut self,
        pointer: Handle<Expression>,
        fun: AtomicFunction,
        value: Handle<Expression>,
        compare: Option<Handle<Expression>>,
        result: Option<Handle<Expression>>,
        span: Span,
    ) {
        let mut used = self.use_expression(pointer, span) & self.use_expression(value, span);
        if let Some(compare) = compare {
            used &= self.use_expression(compare, span);
        }
        if (fun == AtomicFunction::CompareExchange) != compare.is_some() {
            self.fail(
                "SV6",
                "a compare value goes with compare_exchange, and only it",
                span,
            );
        }
        if used {
            match *self.ty(pointer) {
                TypeInner::Pointer { base, space } => {
                    let atomic_space = match space {
                        AddressSpace::Workgroup => true,
                        AddressSpace::Storage { access } => access.atomic || access.store,
                        _ => false,
                    };
                    if !matches!(self.module.types[base].inner, TypeInner::Atomic(_))
                        || !atomic_space
                    {
                        self.fail("SV9", "an atomic operation on a pointer to an atomic in storage or workgroup memory", span);
                    }
                }
                _ => self.fail(
                    "SV9",
                    "an atomic operation through a value that is not a pointer",
                    span,
                ),
            }
        }
        self.global_use(pointer, true, true);
        if let Some(result) = result {
            self.result(
                result,
                |expression| matches!(expression, Expression::AtomicResult { .. }),
                span,
            );
        }
    }

    pub(super) fn call(
        &mut self,
        callee: Handle<klf_shader::Function>,
        arguments: &[Handle<Expression>],
        result: Option<Handle<Expression>>,
        span: Span,
    ) {
        let Some(target) = self.module.functions.get(callee) else {
            self.fail("SV1", "a call to a function that does not exist", span);
            return;
        };
        if callee.index() >= self.module.functions.len() {
            return;
        }
        self.info.calls.insert(callee);
        if arguments.len() != target.arguments.len() {
            self.fail(
                "SV19",
                format!(
                    "`{}` takes {} arguments, not {}",
                    target.name,
                    target.arguments.len(),
                    arguments.len()
                ),
                span,
            );
        }
        for (argument, parameter) in arguments.iter().zip(&target.arguments) {
            if self.use_expression(*argument, span)
                && !self.info.expressions[argument.index()]
                    .ty
                    .same(&Resolution::Handle(parameter.ty), &self.module.types)
            {
                self.fail(
                    "SV19",
                    format!("an argument of `{}` has the wrong type", target.name),
                    span,
                );
            }
            self.global_use(*argument, true, true);
        }
        match (result, &target.result) {
            (Some(result), Some(_)) => {
                self.result(
                    result,
                    |expression| *expression == Expression::CallResult(callee),
                    span,
                );
            }
            (None, _) => {}
            (Some(_), None) => self.fail("SV19", format!("`{}` has no result", target.name), span),
        }
    }
}
