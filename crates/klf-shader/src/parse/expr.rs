//! Expressions in textual KSIR.

use super::Parser;
use super::body::Scope;
use crate::arena::Handle;
use crate::ir::{
    BinaryOp, DerivativeAxis, DerivativeControl, Expression, ImageQuery, MathFunction,
    RelationalFunction, SampleLevel, UnaryOp,
};
use crate::types::ScalarKind;

impl Parser {
    pub(crate) fn expression(&mut self, scope: Option<&Scope>) -> Result<Expression, String> {
        let opcode = self.ident()?;
        Ok(match opcode.as_str() {
            "literal" => Expression::Literal(self.literal()?),
            "constant" => {
                let name = self.at_name()?;
                let found = self
                    .module
                    .constants
                    .iter()
                    .find(|(_, item)| item.name == name)
                    .map(|(handle, _)| handle);
                Expression::Constant(
                    found.map_or_else(|| self.error(format!("unknown constant `@{name}`")), Ok)?,
                )
            }
            "override" => {
                let name = self.at_name()?;
                let found = self
                    .module
                    .overrides
                    .iter()
                    .find(|(_, item)| item.name == name)
                    .map(|(handle, _)| handle);
                Expression::Override(
                    found.map_or_else(|| self.error(format!("unknown override `@{name}`")), Ok)?,
                )
            }
            "zero" => Expression::ZeroValue(self.ty()?),
            "compose" => {
                let ty = self.ty()?;
                self.expect_punct(',')?;
                self.expect_punct('[')?;
                let mut components = Vec::new();
                while !self.eat_punct(']') {
                    components.push(self.operand(scope)?);
                    self.eat_punct(',');
                }
                Expression::Compose { ty, components }
            }
            "access" => {
                let base = self.operand(scope)?;
                self.expect_punct(',')?;
                Expression::Access {
                    base,
                    index: self.operand(scope)?,
                }
            }
            "access_index" => {
                let base = self.operand(scope)?;
                self.expect_punct(',')?;
                Expression::AccessIndex {
                    base,
                    index: self.number()?,
                }
            }
            "splat" => {
                let size = u8::try_from(self.number()?).or_else(|_| self.error("bad size"))?;
                self.expect_punct(',')?;
                Expression::Splat {
                    size,
                    value: self.operand(scope)?,
                }
            }
            "swizzle" => {
                let vector = self.operand(scope)?;
                self.expect_punct(',')?;
                self.expect_punct('[')?;
                let mut pattern = Vec::new();
                while !self.eat_punct(']') {
                    let component = self.ident()?;
                    pattern.push(match component.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        "w" => 3,
                        _ => return self.error("a swizzle component is x, y, z or w"),
                    });
                    self.eat_punct(',');
                }
                Expression::Swizzle { vector, pattern }
            }
            "arg" => Expression::FunctionArgument(self.number()?),
            "global" => {
                let name = self.at_name()?;
                let found = self
                    .module
                    .globals
                    .iter()
                    .find(|(_, item)| item.name == name)
                    .map(|(handle, _)| handle);
                Expression::GlobalVariable(
                    found.map_or_else(|| self.error(format!("unknown global `@{name}`")), Ok)?,
                )
            }
            "local" => {
                self.expect_punct_dollar()?;
                let name = self.ident()?;
                let local = scope
                    .and_then(|scope| scope.locals.get(&name))
                    .copied()
                    .map_or_else(|| self.error(format!("unknown local `${name}`")), Ok)?;
                Expression::LocalVariable(Handle::new(local))
            }
            "load" => Expression::Load {
                pointer: self.operand(scope)?,
            },
            "image_sample" => self.image_sample(scope)?,
            "image_load" => {
                let image = self.operand(scope)?;
                self.expect_punct(',')?;
                let coordinate = self.operand(scope)?;
                let (mut array_index, mut sample, mut level) = (None, None, None);
                while self.eat_punct(',') {
                    let name = self.ident()?;
                    self.expect_punct('(')?;
                    let value = Some(self.operand(scope)?);
                    self.expect_punct(')')?;
                    match name.as_str() {
                        "array_index" => array_index = value,
                        "sample" => sample = value,
                        "level" => level = value,
                        other => {
                            return self.error(format!("unknown image_load operand `{other}`"));
                        }
                    }
                }
                Expression::ImageLoad {
                    image,
                    coordinate,
                    array_index,
                    sample,
                    level,
                }
            }
            "image_query" => {
                let image = self.operand(scope)?;
                self.expect_punct(',')?;
                let query = match self.ident()?.as_str() {
                    "size" => {
                        let level = if self.eat_punct('(') {
                            let level = self.operand(scope)?;
                            self.expect_punct(')')?;
                            Some(level)
                        } else {
                            None
                        };
                        ImageQuery::Size { level }
                    }
                    "levels" => ImageQuery::NumLevels,
                    "layers" => ImageQuery::NumLayers,
                    "samples" => ImageQuery::NumSamples,
                    other => return self.error(format!("unknown query `{other}`")),
                };
                Expression::ImageQuery { image, query }
            }
            "unary" => {
                let op = self.ident()?;
                let op = UnaryOp::from_name(&op)
                    .map_or_else(|| self.error("unknown unary operator"), Ok)?;
                Expression::Unary {
                    op,
                    expr: self.operand(scope)?,
                }
            }
            "binary" => {
                let op = self.ident()?;
                let op = BinaryOp::from_name(&op)
                    .map_or_else(|| self.error(format!("unknown binary operator `{op}`")), Ok)?;
                let left = self.operand(scope)?;
                self.expect_punct(',')?;
                Expression::Binary {
                    op,
                    left,
                    right: self.operand(scope)?,
                }
            }
            "select" => {
                let operands = self.operand_list(scope)?;
                let [condition, accept, reject] = operands[..] else {
                    return self.error("select takes three operands");
                };
                Expression::Select {
                    condition,
                    accept,
                    reject,
                }
            }
            "derivative" => {
                let axis = self.ident()?;
                let axis = DerivativeAxis::from_name(&axis)
                    .map_or_else(|| self.error("unknown axis"), Ok)?;
                let control = self.ident()?;
                let control = DerivativeControl::from_name(&control)
                    .map_or_else(|| self.error("unknown control"), Ok)?;
                Expression::Derivative {
                    axis,
                    control,
                    expr: self.operand(scope)?,
                }
            }
            "relational" => {
                let fun = self.ident()?;
                let fun = RelationalFunction::from_name(&fun)
                    .map_or_else(|| self.error("unknown relational function"), Ok)?;
                Expression::Relational {
                    fun,
                    argument: self.operand(scope)?,
                }
            }
            "math" => {
                let fun = self.ident()?;
                let fun = MathFunction::from_name(&fun)
                    .map_or_else(|| self.error(format!("unknown math function `{fun}`")), Ok)?;
                Expression::Math {
                    fun,
                    args: self.operand_list(scope)?,
                }
            }
            "convert" => {
                let expr = self.operand(scope)?;
                self.expect_punct(',')?;
                let kind = self.ident()?;
                let kind =
                    ScalarKind::from_name(&kind).map_or_else(|| self.error("unknown kind"), Ok)?;
                let width = if self.eat_punct(',') {
                    Some(u8::try_from(self.number()?).or_else(|_| self.error("bad width"))?)
                } else {
                    None
                };
                Expression::Convert { expr, kind, width }
            }
            "bitcast" => {
                let expr = self.operand(scope)?;
                self.expect_punct(',')?;
                Expression::Bitcast {
                    expr,
                    ty: self.ty()?,
                }
            }
            "array_length" => Expression::ArrayLength(self.operand(scope)?),
            other => return self.error(format!("unknown opcode `{other}`")),
        })
    }

    fn image_sample(&mut self, scope: Option<&Scope>) -> Result<Expression, String> {
        let image = self.operand(scope)?;
        self.expect_punct(',')?;
        let sampler = self.operand(scope)?;
        self.expect_punct(',')?;
        let coordinate = self.operand(scope)?;
        let (mut array_index, mut offset, mut depth_ref, mut gather) = (None, None, None, None);
        let mut level = SampleLevel::Auto;
        while self.eat_punct(',') {
            let name = self.ident()?;
            self.expect_punct('(')?;
            match name.as_str() {
                "array_index" => array_index = Some(self.operand(scope)?),
                "offset" => offset = Some(self.operand(scope)?),
                "depth_ref" => depth_ref = Some(self.operand(scope)?),
                "gather" => {
                    gather = Some(match self.ident()?.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        "w" => 3,
                        _ => return self.error("a gather component is x, y, z or w"),
                    });
                }
                "level" => {
                    level = match self.ident()?.as_str() {
                        "auto" => SampleLevel::Auto,
                        "zero" => SampleLevel::Zero,
                        "exact" => {
                            self.expect_punct(',')?;
                            SampleLevel::Exact(self.operand(scope)?)
                        }
                        "bias" => {
                            self.expect_punct(',')?;
                            SampleLevel::Bias(self.operand(scope)?)
                        }
                        "gradient" => {
                            self.expect_punct(',')?;
                            let x = self.operand(scope)?;
                            self.expect_punct(',')?;
                            SampleLevel::Gradient {
                                x,
                                y: self.operand(scope)?,
                            }
                        }
                        other => return self.error(format!("unknown level `{other}`")),
                    };
                }
                other => return self.error(format!("unknown image_sample operand `{other}`")),
            }
            self.expect_punct(')')?;
        }
        Ok(Expression::ImageSample {
            image,
            sampler,
            gather,
            coordinate,
            array_index,
            offset,
            level,
            depth_ref,
        })
    }
}
