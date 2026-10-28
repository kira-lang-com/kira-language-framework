//! Reading textual KSIR back into a module (section 18, Appendix A).

mod body;
mod expr;
pub mod fragment;
mod lexer;
mod types;

use crate::Span;
use crate::arena::Handle;
use crate::ir::{Expression, Literal};
use crate::layout::{self, MemberSpec};
use crate::module::{
    Constant, EarlyDepthTest, EntryPoint, FunctionArgument, FunctionResult, GlobalVariable,
    GroupClass, GroupDecl, HlslClass, Module, Override, ResourceBinding, ShaderDecl, ShaderKind,
    Stage, TargetSlot, WorkgroupExtent,
};
use crate::types::{Capability, LayoutRule, StructMember, Type, TypeInner};
use lexer::Token;

/// Parses textual KSIR.
///
/// # Errors
/// Returns `line N: message` for the first problem.
pub fn parse(text: &str) -> Result<Module, String> {
    let lexed = lexer::lex(text)?;
    let mut parser = Parser {
        tokens: lexed.tokens,
        at: 0,
        module: Module::default(),
        pending_shaders: Vec::new(),
    };
    parser.file()?;
    parser.resolve_shaders()?;
    Ok(parser.module)
}

struct PendingShader {
    decl: ShaderDecl,
    entries: Vec<(String, u32)>,
}

pub(crate) struct Parser {
    tokens: Vec<(Token, u32)>,
    at: usize,
    pub(crate) module: Module,
    pending_shaders: Vec<PendingShader>,
}

impl Parser {
    pub(crate) fn line(&self) -> u32 {
        self.tokens
            .get(self.at)
            .or_else(|| self.tokens.last())
            .map_or(0, |(_, line)| *line)
    }

    pub(crate) fn error<T>(&self, message: impl std::fmt::Display) -> Result<T, String> {
        Err(format!("line {}: {message}", self.line()))
    }

    pub(crate) fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    pub(crate) fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.at + offset).map(|(token, _)| token)
    }

    pub(crate) fn next(&mut self) -> Result<Token, String> {
        let token = self.peek().cloned();
        match token {
            Some(token) => {
                self.at += 1;
                Ok(token)
            }
            None => self.error("unexpected end of input"),
        }
    }

    pub(crate) fn is_punct(&self, punct: char) -> bool {
        let wanted = match punct {
            '%' => Token::Percent,
            '@' => Token::At,
            '$' => Token::Dollar,
            '!' => Token::Bang,
            other => Token::Punct(other),
        };
        self.peek() == Some(&wanted)
    }

    pub(crate) fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(found)) if found == word)
    }

    pub(crate) fn eat_punct(&mut self, punct: char) -> bool {
        if self.is_punct(punct) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_word(&mut self, word: &str) -> bool {
        if self.is_word(word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect_punct(&mut self, punct: char) -> Result<(), String> {
        if self.eat_punct(punct) {
            Ok(())
        } else {
            self.error(format!("expected `{punct}`, found {:?}", self.peek()))
        }
    }

    pub(crate) fn expect_word(&mut self, word: &str) -> Result<(), String> {
        if self.eat_word(word) {
            Ok(())
        } else {
            self.error(format!("expected `{word}`, found {:?}", self.peek()))
        }
    }

    pub(crate) fn ident(&mut self) -> Result<String, String> {
        match self.next()? {
            Token::Ident(name) => Ok(name),
            Token::Number(text) => Ok(text),
            other => self.error(format!("expected a name, found {other:?}")),
        }
    }

    pub(crate) fn number(&mut self) -> Result<u32, String> {
        match self.next()? {
            Token::Number(text) => text
                .parse()
                .or_else(|_| self.error(format!("`{text}` is not a number"))),
            other => self.error(format!("expected a number, found {other:?}")),
        }
    }

    /// `@name`.
    pub(crate) fn at_name(&mut self) -> Result<String, String> {
        match self.next()? {
            Token::At => self.ident(),
            other => self.error(format!("expected `@`, found {other:?}")),
        }
    }

    /// An optional `!loc("path":line:col)`.
    pub(crate) fn loc(&mut self) -> Result<Span, String> {
        if self.peek() != Some(&Token::Bang) {
            return Ok(Span::default());
        }
        self.at += 1;
        self.expect_word("loc")?;
        self.expect_punct('(')?;
        let Token::Str(path) = self.next()? else {
            return self.error("expected a path");
        };
        self.expect_punct(':')?;
        let line = self.number()?;
        self.expect_punct(':')?;
        let column = self.number()?;
        self.expect_punct(')')?;
        let file = self.module.file(&path);
        Ok(Span {
            file,
            line,
            column,
            start: 0,
            end: 0,
        })
    }

    fn file(&mut self) -> Result<(), String> {
        self.expect_word("ksir")?;
        if self.number()? != 1 {
            return self.error("only `ksir 1` is read");
        }
        self.expect_word("module")?;
        self.module.name = self.at_name()?;
        self.expect_word("lang")?;
        let Token::Str(lang) = self.next()? else {
            return self.error("expected the language as a string");
        };
        self.module.lang = lang;
        self.expect_word("capabilities")?;
        self.expect_punct('(')?;
        while !self.eat_punct(')') {
            let name = self.ident()?;
            let capability = Capability::from_name(&name)
                .map_or_else(|| self.error(format!("unknown capability `{name}`")), Ok)?;
            self.module.capabilities.insert(capability);
            self.eat_punct(',');
        }
        while let Some(token) = self.peek().cloned() {
            match token {
                Token::Ident(word) => match word.as_str() {
                    "type" => self.struct_decl()?,
                    "const" => self.constant()?,
                    "override" => self.override_decl()?,
                    "global" => self.global()?,
                    "shader" => self.shader()?,
                    "func" => self.function(None)?,
                    "entry" => {
                        self.at += 1;
                        let stage = self.ident()?;
                        let stage = Stage::from_name(&stage)
                            .map_or_else(|| self.error(format!("unknown stage `{stage}`")), Ok)?;
                        self.function(Some(stage))?;
                    }
                    other => return self.error(format!("unexpected `{other}`")),
                },
                Token::Percent => self.global_expression()?,
                other => return self.error(format!("unexpected {other:?}")),
            }
        }
        Ok(())
    }

    fn struct_decl(&mut self) -> Result<(), String> {
        self.expect_word("type")?;
        self.expect_punct('%')?;
        let name = self.ident()?;
        self.expect_punct('=')?;
        self.expect_word("struct")?;
        let rule_name = self.ident()?;
        let rule = LayoutRule::from_name(&rule_name)
            .map_or_else(|| self.error(format!("unknown rule `{rule_name}`")), Ok)?;
        let mut extent = None;
        if self.eat_word("size") {
            let size = self.number()?;
            self.expect_word("align")?;
            extent = Some((size, self.number()?));
        }
        self.expect_punct('{')?;
        let mut members = Vec::new();
        let mut specs = Vec::new();
        while !self.eat_punct('}') {
            let member_name = self.ident()?;
            self.expect_punct(':')?;
            let ty = self.ty()?;
            let mut placed = (0, 0, 0);
            if self.eat_word("offset") {
                let offset = self.number()?;
                self.expect_word("size")?;
                let size = self.number()?;
                self.expect_word("align")?;
                placed = (offset, size, self.number()?);
            }
            let binding = self.binding()?;
            members.push(StructMember {
                name: member_name.clone(),
                ty,
                binding,
                offset: placed.0,
                size: placed.1,
                align: placed.2,
                span: Span::default(),
            });
            specs.push(MemberSpec {
                name: member_name,
                ty,
                binding,
                span: Span::default(),
            });
            self.eat_punct(',');
        }
        let (members, size, align) = match (rule, extent) {
            (LayoutRule::None, _) => layout::place_members(&self.module.types, specs, rule),
            (_, Some((size, align))) => (members, size, align),
            (_, None) => return self.error("a laid-out struct states its size and alignment"),
        };
        let span = self.loc()?;
        self.module.types.insert(
            Type {
                name: Some(name),
                inner: TypeInner::Struct {
                    rule,
                    members,
                    size,
                    align,
                },
            },
            span,
        );
        Ok(())
    }

    fn global_ref(&mut self) -> Result<Handle<Expression>, String> {
        self.expect_punct('%')?;
        let word = self.ident()?;
        let index: u32 = word
            .strip_prefix('g')
            .and_then(|digits| digits.parse().ok())
            .map_or_else(
                || self.error(format!("expected `%gN`, found `%{word}`")),
                Ok,
            )?;
        if index as usize >= self.module.global_expressions.len() {
            return self.error(format!("`%g{index}` is not defined yet"));
        }
        Ok(Handle::new(index))
    }

    fn global_expression(&mut self) -> Result<(), String> {
        let handle = self.global_ref_declared()?;
        self.expect_punct('=')?;
        let expression = self.expression(None)?;
        let span = self.loc()?;
        if handle != self.module.global_expressions.len() as u32 {
            return self.error("global expressions are numbered in order");
        }
        self.module.global_expressions.append(expression, span);
        Ok(())
    }

    fn global_ref_declared(&mut self) -> Result<u32, String> {
        self.expect_punct('%')?;
        let word = self.ident()?;
        word.strip_prefix('g')
            .and_then(|digits| digits.parse().ok())
            .map_or_else(|| self.error("expected `%gN`"), Ok)
    }

    fn constant(&mut self) -> Result<(), String> {
        self.expect_word("const")?;
        let name = self.at_name()?;
        self.expect_punct(':')?;
        let ty = self.ty()?;
        self.expect_punct('=')?;
        let init = self.global_ref()?;
        let span = self.loc()?;
        self.module
            .constants
            .append(Constant { name, ty, init }, span);
        Ok(())
    }

    fn override_decl(&mut self) -> Result<(), String> {
        self.expect_word("override")?;
        let name = self.at_name()?;
        self.expect_punct(':')?;
        let ty = self.ty()?;
        let mut id = None;
        if self.eat_word("id") {
            self.expect_punct('(')?;
            id = Some(
                u16::try_from(self.number()?)
                    .or_else(|_| self.error("an override id fits 16 bits"))?,
            );
            self.expect_punct(')')?;
        }
        let init = if self.eat_punct('=') {
            Some(self.global_ref()?)
        } else {
            None
        };
        let span = self.loc()?;
        self.module
            .overrides
            .append(Override { name, id, ty, init }, span);
        Ok(())
    }

    fn global(&mut self) -> Result<(), String> {
        self.expect_word("global")?;
        let name = self.at_name()?;
        self.expect_punct(':')?;
        let ty = self.ty()?;
        self.expect_word("space")?;
        self.expect_punct('(')?;
        let space = self.space()?;
        self.expect_punct(')')?;
        let mut binding = None;
        if self.eat_word("binding") {
            self.expect_punct('(')?;
            let group = self.number()?;
            self.expect_punct(',')?;
            binding = Some(ResourceBinding {
                group,
                binding: self.number()?,
            });
            self.expect_punct(')')?;
        }
        let mut slots = Vec::new();
        while self.eat_word("slot") {
            self.expect_punct('(')?;
            slots.push(self.slot()?);
            self.expect_punct(')')?;
        }
        let init = if self.eat_punct('=') {
            Some(self.global_ref()?)
        } else {
            None
        };
        let span = self.loc()?;
        self.module.globals.append(
            GlobalVariable {
                name,
                space,
                ty,
                binding,
                slots,
                init,
            },
            span,
        );
        Ok(())
    }

    pub(crate) fn slot(&mut self) -> Result<TargetSlot, String> {
        let word = self.ident()?;
        Ok(match word.as_str() {
            "universal" => TargetSlot::Universal(self.number()?),
            "msl.buffer" => TargetSlot::MslBuffer(self.number()?),
            "msl.texture" => TargetSlot::MslTexture(self.number()?),
            "msl.sampler" => TargetSlot::MslSampler(self.number()?),
            "glsl" => TargetSlot::Glsl(self.number()?),
            "hlsl" => {
                let class = self.ident()?;
                let class = HlslClass::from_name(&class)
                    .map_or_else(|| self.error("unknown register class"), Ok)?;
                let register = self.number()?;
                TargetSlot::Hlsl {
                    class,
                    register,
                    space: self.number()?,
                }
            }
            other => return self.error(format!("unknown slot `{other}`")),
        })
    }

    fn shader(&mut self) -> Result<(), String> {
        self.expect_word("shader")?;
        let name = self.at_name()?;
        let kind = self.ident()?;
        let kind =
            ShaderKind::from_name(&kind).map_or_else(|| self.error("unknown shader kind"), Ok)?;
        self.expect_punct('{')?;
        let mut groups = Vec::new();
        let mut entries = Vec::new();
        let mut options = Vec::new();
        while !self.eat_punct('}') {
            if self.eat_word("group") {
                let group_name = self.at_name()?;
                self.expect_word("class")?;
                self.expect_punct('(')?;
                let class = self.ident()?;
                let class = GroupClass::from_name(&class)
                    .map_or_else(|| self.error("unknown group class"), Ok)?;
                self.expect_punct(')')?;
                let mut globals = Vec::new();
                for global in self.ref_list()? {
                    let found = self
                        .module
                        .globals
                        .iter()
                        .find(|(_, item)| item.name == global)
                        .map(|(handle, _)| handle);
                    globals.push(
                        found.map_or_else(
                            || self.error(format!("unknown global `@{global}`")),
                            Ok,
                        )?,
                    );
                }
                groups.push(GroupDecl {
                    name: group_name,
                    class,
                    globals,
                });
            } else if self.eat_word("entries") {
                let line = self.line();
                entries = self
                    .ref_list()?
                    .into_iter()
                    .map(|entry| (entry, line))
                    .collect();
            } else if self.eat_word("options") {
                for option in self.ref_list()? {
                    let found = self
                        .module
                        .overrides
                        .iter()
                        .find(|(_, item)| item.name == option)
                        .map(|(handle, _)| handle);
                    options.push(
                        found.map_or_else(
                            || self.error(format!("unknown override `@{option}`")),
                            Ok,
                        )?,
                    );
                }
            } else {
                return self.error("expected `group`, `entries` or `options`");
            }
        }
        let span = self.loc()?;
        self.pending_shaders.push(PendingShader {
            decl: ShaderDecl {
                name,
                kind,
                entries: Vec::new(),
                groups,
                options,
                span,
            },
            entries,
        });
        Ok(())
    }

    fn ref_list(&mut self) -> Result<Vec<String>, String> {
        self.expect_punct('{')?;
        let mut names = Vec::new();
        while !self.eat_punct('}') {
            names.push(self.at_name()?);
            self.eat_punct(',');
        }
        Ok(names)
    }

    fn resolve_shaders(&mut self) -> Result<(), String> {
        for pending in std::mem::take(&mut self.pending_shaders) {
            let mut decl = pending.decl;
            for (entry, line) in pending.entries {
                let index = self
                    .module
                    .entry_points
                    .iter()
                    .position(|point| point.name == entry)
                    .ok_or_else(|| format!("line {line}: unknown entry point `@{entry}`"))?;
                decl.entries.push(index);
            }
            self.module.shaders.push(decl);
        }
        Ok(())
    }

    fn function(&mut self, stage: Option<Stage>) -> Result<(), String> {
        if stage.is_none() {
            self.expect_word("func")?;
        }
        let name = self.at_name()?;
        self.expect_punct('(')?;
        let mut arguments = Vec::new();
        while !self.eat_punct(')') {
            self.expect_punct('%')?;
            let argument = self.ident()?;
            self.expect_punct(':')?;
            let ty = self.ty()?;
            let binding = self.binding()?;
            let unnamed = argument
                .strip_prefix('_')
                .is_some_and(|rest| rest.parse::<u32>().is_ok());
            arguments.push(FunctionArgument {
                name: (!unnamed).then_some(argument),
                ty,
                binding,
            });
            self.eat_punct(',');
        }
        let result = if self.peek() == Some(&Token::Arrow) {
            self.at += 1;
            let ty = self.ty()?;
            Some(FunctionResult {
                ty,
                binding: self.binding()?,
            })
        } else {
            None
        };
        let mut workgroup_size = None;
        let mut early_depth_test = None;
        if self.eat_word("workgroup") {
            self.expect_punct('(')?;
            let mut extents = [WorkgroupExtent::Fixed(1); 3];
            for (index, extent) in extents.iter_mut().enumerate() {
                if index > 0 {
                    self.expect_punct(',')?;
                }
                *extent = if self.peek() == Some(&Token::Percent) {
                    WorkgroupExtent::Override(self.global_ref()?)
                } else {
                    WorkgroupExtent::Fixed(self.number()?)
                };
            }
            self.expect_punct(')')?;
            workgroup_size = Some(extents);
        }
        if self.eat_word("early_depth") {
            self.expect_punct('(')?;
            let mode = self.ident()?;
            early_depth_test = Some(
                EarlyDepthTest::from_name(&mode)
                    .map_or_else(|| self.error("unknown early depth mode"), Ok)?,
            );
            self.expect_punct(')')?;
        }
        let function = self.body(name.clone(), arguments, result)?;
        let span = function.1;
        match stage {
            None => {
                self.module.functions.append(function.0, span);
            }
            Some(stage) => self.module.entry_points.push(EntryPoint {
                name,
                stage,
                workgroup_size,
                early_depth_test,
                function: function.0,
                span,
            }),
        }
        Ok(())
    }

    /// A literal from its number token or `true`/`false`.
    pub(crate) fn literal(&mut self) -> Result<Literal, String> {
        let token = self.next()?;
        let text = match token {
            Token::Ident(word) if word == "true" => return Ok(Literal::Bool(true)),
            Token::Ident(word) if word == "false" => return Ok(Literal::Bool(false)),
            Token::Number(text) => text,
            other => return self.error(format!("expected a literal, found {other:?}")),
        };
        let bad = || format!("line {}: bad literal `{text}`", self.line());
        let (digits, suffix) = text.split_at(text.len().saturating_sub(3));
        Ok(match suffix {
            "i32" => Literal::I32(digits.parse().map_err(|_| bad())?),
            "u32" => Literal::U32(digits.parse().map_err(|_| bad())?),
            "i64" => Literal::I64(digits.parse().map_err(|_| bad())?),
            "u64" => Literal::U64(digits.parse().map_err(|_| bad())?),
            "f32" => Literal::f32(digits.parse().map_err(|_| bad())?),
            "f64" => Literal::f64(digits.parse().map_err(|_| bad())?),
            "f16" => Literal::F16(f32_to_half(digits.parse().map_err(|_| bad())?)),
            _ => return Err(bad()),
        })
    }
}

/// Rounds `value` to IEEE binary16, to nearest even.
#[must_use]
pub fn f32_to_half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa == 0 { 0 } else { 0x200 };
    }
    let exponent = exponent - 127 + 15;
    if exponent >= 0x1f {
        return sign | 0x7c00;
    }
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x80_0000;
        let shift = (14 - exponent) as u32;
        let half = mantissa >> shift;
        let rest = mantissa & ((1 << shift) - 1);
        let midpoint = 1 << (shift - 1);
        let round = u32::from(rest > midpoint || (rest == midpoint && half & 1 == 1));
        return sign | (half + round) as u16;
    }
    let half = ((exponent as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    let round = u32::from(rest > 0x1000 || (rest == 0x1000 && half & 1 == 1));
    sign | (half + round) as u16
}
