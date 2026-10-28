//! Expressions (section 11). Each service takes the module and a function
//! builder; function `-1` appends to the module's global expressions, for a
//! constant's or an override's initializer.

use super::{Args, NONE, PendingFunction, ShaderBuild, ShaderState};
use klf_shader::parse::fragment;
use klf_shader::{
    BinaryOp, DerivativeAxis, DerivativeControl, Expression, Handle, ImageQuery, MathFunction,
    RelationalFunction, SampleLevel, Scalar, UnaryOp,
};

type Scope<'a> = (&'a mut ShaderBuild, Option<&'a mut PendingFunction>);

fn named<T>(found: Option<T>, what: &str, text: &str) -> Result<T, String> {
    found.ok_or_else(|| format!("unknown {what} `{text}`"))
}

/// The handle a pending word names, `-1` being none.
fn optional(word: u64) -> Result<Option<Handle<Expression>>, String> {
    if word == NONE {
        return Ok(None);
    }
    u32::try_from(word)
        .map(|index| Some(Handle::new(index)))
        .map_err(|_| "an expression handle out of range".to_owned())
}

fn required(word: u64) -> Result<Handle<Expression>, String> {
    optional(word)?.ok_or_else(|| "a required operand is absent".to_owned())
}

impl ShaderState {
    pub(super) fn scope(&mut self, module: u64, function: u64) -> Result<Scope<'_>, String> {
        let index = usize::try_from(module).map_err(|_| "unknown shader module".to_owned())?;
        let build = self
            .modules
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or_else(|| "unknown shader module".to_owned())?;
        if function == NONE {
            return Ok((build, None));
        }
        let pending = usize::try_from(function)
            .ok()
            .and_then(|function| self.functions.get_mut(function))
            .and_then(Option::as_mut)
            .filter(|pending| pending.module == index)
            .ok_or_else(|| "a function builder that is not open in this module".to_owned())?;
        Ok((build, Some(pending)))
    }

    /// The pending words of the call, taken.
    fn operands(&mut self, args: &Args) -> Result<Vec<u64>, String> {
        Ok(std::mem::take(&mut self.build(args.word(0)?)?.words))
    }

    pub(super) fn add(&mut self, args: &Args, expression: Expression) -> Result<u64, String> {
        let (build, function) = self.scope(args.word(0)?, args.word(1)?)?;
        let span = build.span;
        let handle = match function {
            None => build.builder.global_expression(expression, span),
            Some(pending) => {
                pending
                    .builder
                    .expression(&mut build.builder.module, expression, span)
            }
        }
        .map_err(|error| error.0)?;
        Ok(u64::from(handle.raw()))
    }

    /// `klf.shader.expr.literal(module, function, scalar, text)`.
    pub(super) fn expr_literal(&mut self, args: &Args) -> Result<u64, String> {
        let literal = fragment::literal(args.text(0)?, args.text(1)?)?;
        self.add(args, Expression::Literal(literal))
    }

    pub(super) fn expr_zero(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::ZeroValue(args.handle(2)?))
    }

    /// `klf.shader.expr.compose(module, function, type)` over the pending
    /// parts.
    pub(super) fn expr_compose(&mut self, args: &Args) -> Result<u64, String> {
        let components = self
            .operands(args)?
            .into_iter()
            .map(required)
            .collect::<Result<_, _>>()?;
        let ty = args.handle(2)?;
        self.add(args, Expression::Compose { ty, components })
    }

    pub(super) fn expr_splat(&mut self, args: &Args) -> Result<u64, String> {
        let size = u8::try_from(args.word(2)?).map_err(|_| "a splat size".to_owned())?;
        let value = args.handle(3)?;
        self.add(args, Expression::Splat { size, value })
    }

    /// `klf.shader.expr.swizzle(module, function, vector, pattern)`, the
    /// pattern written `xyzw` or `rgba`.
    pub(super) fn expr_swizzle(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let pattern = text
            .chars()
            .map(|component| match component {
                'x' | 'r' => Ok(0),
                'y' | 'g' => Ok(1),
                'z' | 'b' => Ok(2),
                'w' | 'a' => Ok(3),
                _ => Err(format!("unknown swizzle `{text}`")),
            })
            .collect::<Result<_, _>>()?;
        let vector = args.handle(2)?;
        self.add(args, Expression::Swizzle { vector, pattern })
    }

    pub(super) fn expr_access(&mut self, args: &Args) -> Result<u64, String> {
        let (base, index) = (args.handle(2)?, args.handle(3)?);
        self.add(args, Expression::Access { base, index })
    }

    pub(super) fn expr_access_index(&mut self, args: &Args) -> Result<u64, String> {
        let (base, index) = (args.handle(2)?, args.small(3)?);
        self.add(args, Expression::AccessIndex { base, index })
    }

    pub(super) fn expr_argument(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::FunctionArgument(args.small(2)?))
    }

    pub(super) fn expr_global(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::GlobalVariable(args.handle(2)?))
    }

    pub(super) fn expr_local(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::LocalVariable(args.handle(2)?))
    }

    pub(super) fn expr_constant(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::Constant(args.handle(2)?))
    }

    pub(super) fn expr_override(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::Override(args.handle(2)?))
    }

    pub(super) fn expr_load(&mut self, args: &Args) -> Result<u64, String> {
        self.add(
            args,
            Expression::Load {
                pointer: args.handle(2)?,
            },
        )
    }

    pub(super) fn expr_unary(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let op = named(UnaryOp::from_name(text), "unary operator", text)?;
        self.add(
            args,
            Expression::Unary {
                op,
                expr: args.handle(2)?,
            },
        )
    }

    pub(super) fn expr_binary(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let op = named(BinaryOp::from_name(text), "binary operator", text)?;
        let (left, right) = (args.handle(2)?, args.handle(3)?);
        self.add(args, Expression::Binary { op, left, right })
    }

    pub(super) fn expr_select(&mut self, args: &Args) -> Result<u64, String> {
        let condition = args.handle(2)?;
        let (accept, reject) = (args.handle(3)?, args.handle(4)?);
        self.add(
            args,
            Expression::Select {
                condition,
                accept,
                reject,
            },
        )
    }

    /// `klf.shader.expr.convert(module, function, value, scalar)`: to the
    /// scalar's kind and width, keeping the value's shape.
    pub(super) fn expr_convert(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let scalar = named(Scalar::from_text(text), "scalar", text)?;
        let expr = args.handle(2)?;
        self.add(
            args,
            Expression::Convert {
                expr,
                kind: scalar.kind,
                width: Some(scalar.width),
            },
        )
    }

    pub(super) fn expr_bitcast(&mut self, args: &Args) -> Result<u64, String> {
        let (expr, ty) = (args.handle(2)?, args.handle(3)?);
        self.add(args, Expression::Bitcast { expr, ty })
    }

    /// `klf.shader.expr.math(module, function, name)` over the pending
    /// arguments.
    pub(super) fn expr_math(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let fun = named(MathFunction::from_name(text), "math function", text)?;
        let arguments: Vec<_> = self
            .operands(args)?
            .into_iter()
            .map(required)
            .collect::<Result<_, _>>()?;
        if arguments.len() != fun.arity() {
            return Err(format!(
                "`{text}` takes {} arguments, not {}",
                fun.arity(),
                arguments.len()
            ));
        }
        self.add(
            args,
            Expression::Math {
                fun,
                args: arguments,
            },
        )
    }

    pub(super) fn expr_relational(&mut self, args: &Args) -> Result<u64, String> {
        let text = args.text(0)?;
        let fun = named(
            RelationalFunction::from_name(text),
            "relational function",
            text,
        )?;
        let argument = args.handle(2)?;
        self.add(args, Expression::Relational { fun, argument })
    }

    pub(super) fn expr_derivative(&mut self, args: &Args) -> Result<u64, String> {
        let (axis, control) = (args.text(0)?, args.text(1)?);
        let axis = named(DerivativeAxis::from_name(axis), "derivative axis", axis)?;
        let control = named(
            DerivativeControl::from_name(control),
            "derivative control",
            control,
        )?;
        let expr = args.handle(2)?;
        self.add(
            args,
            Expression::Derivative {
                axis,
                control,
                expr,
            },
        )
    }

    /// `klf.shader.expr.image_sample(module, function, level)` over nine
    /// pending words: image, sampler, coordinate, array index, offset, two
    /// level operands, depth reference and gather component. Level is
    /// `auto`, `zero`, `exact`, `bias` or `gradient`.
    pub(super) fn expr_image_sample(&mut self, args: &Args) -> Result<u64, String> {
        let words = self.operands(args)?;
        let [
            image,
            sampler,
            coordinate,
            array_index,
            offset,
            first,
            second,
            depth_ref,
            gather,
        ] = words[..]
        else {
            return Err(format!("nine sample operands, not {}", words.len()));
        };
        let level = match args.text(0)? {
            "auto" => SampleLevel::Auto,
            "zero" => SampleLevel::Zero,
            "exact" => SampleLevel::Exact(required(first)?),
            "bias" => SampleLevel::Bias(required(first)?),
            "gradient" => SampleLevel::Gradient {
                x: required(first)?,
                y: required(second)?,
            },
            other => return Err(format!("unknown sample level `{other}`")),
        };
        let gather = match gather {
            NONE => None,
            component => Some(u8::try_from(component).map_err(|_| "a gather component")?),
        };
        let expression = Expression::ImageSample {
            image: required(image)?,
            sampler: required(sampler)?,
            gather,
            coordinate: required(coordinate)?,
            array_index: optional(array_index)?,
            offset: optional(offset)?,
            level,
            depth_ref: optional(depth_ref)?,
        };
        self.add(args, expression)
    }

    /// `klf.shader.expr.image_load(module, function)` over five pending
    /// words: image, coordinate, array index, sample and level.
    pub(super) fn expr_image_load(&mut self, args: &Args) -> Result<u64, String> {
        let words = self.operands(args)?;
        let [image, coordinate, array_index, sample, level] = words[..] else {
            return Err(format!("five load operands, not {}", words.len()));
        };
        let expression = Expression::ImageLoad {
            image: required(image)?,
            coordinate: required(coordinate)?,
            array_index: optional(array_index)?,
            sample: optional(sample)?,
            level: optional(level)?,
        };
        self.add(args, expression)
    }

    /// `klf.shader.expr.image_query(module, function, image, query, level)`
    /// with query `size`, `levels`, `layers` or `samples`.
    pub(super) fn expr_image_query(&mut self, args: &Args) -> Result<u64, String> {
        let query = match args.text(0)? {
            "size" => ImageQuery::Size {
                level: args.optional(3)?,
            },
            "levels" => ImageQuery::NumLevels,
            "layers" => ImageQuery::NumLayers,
            "samples" => ImageQuery::NumSamples,
            other => return Err(format!("unknown image query `{other}`")),
        };
        let image = args.handle(2)?;
        self.add(args, Expression::ImageQuery { image, query })
    }

    pub(super) fn expr_array_length(&mut self, args: &Args) -> Result<u64, String> {
        self.add(args, Expression::ArrayLength(args.handle(2)?))
    }

    /// `klf.shader.expr.name(module, function, expression, name)`: the name
    /// the expression's value is printed and emitted under.
    pub(super) fn expr_name(&mut self, args: &Args) -> Result<u64, String> {
        let expression = args.handle(2)?;
        let name = args.text(0)?.to_owned();
        let (_, function) = self.scope(args.word(0)?, args.word(1)?)?;
        let pending = function.ok_or_else(|| "a name outside a function".to_owned())?;
        pending
            .builder
            .type_of(expression)
            .map_err(|error| error.0)?;
        pending.builder.name_expression(expression, &name);
        Ok(0)
    }
}
