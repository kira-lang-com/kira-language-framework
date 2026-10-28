//! Parser for `.ridl` files (grammar A.2).

use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_source::Span;

mod lexer;
mod names;
mod passes;
mod spans;

pub use spans::ItemSpans;

use crate::module::{
    AliasDecl, AnnArg, AnnValue, Annotation, AssocType, ConstDecl, Convention, ExtendBody,
    ExtendDecl, FunctionDecl, GenericParam, Item, ItemKind, Module, Param, SelfParam, TraitDecl,
};
use crate::types::{Case, EnumDecl, Field, StructDecl, Type};

const PARSE_ERR: &str = "KLF-RIDL-0001";

use lexer::{Spanned, Tok, describe, lex};
use names::named_type;
use passes::{apply_open_annotations, expand_aliases, resolve_generics};

struct Parser {
    toks: Vec<Spanned>,
    pos: usize,
    spans: Vec<ItemSpans>,
    sources: Vec<Span>,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == word)
    }

    fn eat_ident(&mut self, word: &str) -> bool {
        if self.is_ident(word) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &Tok, what: &str) -> Result<(), Diagnostic> {
        if self.peek() == tok {
            self.bump();
            Ok(())
        } else {
            Err(self.err(&format!("expected {what}")))
        }
    }

    fn err(&self, msg: &str) -> Diagnostic {
        Diagnostic::error(PARSE_ERR, msg.to_owned())
            .with_label(self.span(), format!("found `{}`", describe(self.peek())))
    }

    fn ident(&mut self) -> Result<String, Diagnostic> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err("expected an identifier")),
        }
    }

    fn parse_module(&mut self, name: &str) -> Result<Module, Diagnostic> {
        let mut module = Module::new(name);
        while self.eat_ident("import") {
            let path = self.parse_path()?;
            module.imports.push(path);
        }
        while self.peek() != &Tok::Eof {
            let item = self.parse_item()?;
            module.items.push(item);
        }
        Ok(module)
    }

    fn parse_path(&mut self) -> Result<String, Diagnostic> {
        let mut path = self.ident()?;
        while self.peek() == &Tok::Dot {
            self.bump();
            path.push('.');
            path.push_str(&self.ident()?);
        }
        Ok(path)
    }

    fn parse_item(&mut self) -> Result<Item, Diagnostic> {
        let start = self.span().start;
        let annotations = self.parse_annotations()?;
        let named = !self.is_ident("extend");
        self.spans.push(ItemSpans {
            start,
            name: named.then(|| self.toks[(self.pos + 1).min(self.toks.len() - 1)].span),
            sources: std::mem::take(&mut self.sources),
        });
        let kind = if self.eat_ident("struct") {
            ItemKind::Struct(self.parse_struct()?)
        } else if self.eat_ident("enum") {
            ItemKind::Enum(self.parse_enum(false)?)
        } else if self.eat_ident("trait") {
            ItemKind::Trait(self.parse_trait()?)
        } else if self.eat_ident("extend") {
            ItemKind::Extend(self.parse_extend()?)
        } else if self.eat_ident("function") {
            ItemKind::Function(self.parse_function()?)
        } else if self.eat_ident("type") {
            ItemKind::Alias(self.parse_alias()?)
        } else if self.eat_ident("let") {
            ItemKind::Const(self.parse_const()?)
        } else {
            return Err(self.err("expected a declaration"));
        };
        Ok(Item { annotations, kind })
    }

    fn parse_annotations(&mut self) -> Result<Vec<Annotation>, Diagnostic> {
        let mut anns = Vec::new();
        while self.peek() == &Tok::At {
            self.bump();
            let mut name = self.ident()?;
            while self.peek() == &Tok::Dot {
                self.bump();
                name.push('.');
                name.push_str(&self.ident()?);
            }
            let mut args = Vec::new();
            if self.peek() == &Tok::LParen {
                self.bump();
                if name == "Source" && matches!(self.peek(), Tok::Str(_)) {
                    self.sources.push(self.span());
                }
                while self.peek() != &Tok::RParen {
                    args.push(self.parse_ann_arg()?);
                    if self.peek() == &Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "`)`")?;
            }
            anns.push(Annotation { name, args });
        }
        Ok(anns)
    }

    fn parse_ann_arg(&mut self) -> Result<AnnArg, Diagnostic> {
        // label: value  OR  value (a type or string or ident)
        if let Tok::Ident(word) = self.peek().clone()
            && self.toks[self.pos + 1].tok == Tok::Colon
        {
            self.bump();
            self.bump();
            let value = self.parse_ann_value()?;
            return Ok(AnnArg {
                label: Some(word),
                value,
            });
        }
        let value = self.parse_ann_value()?;
        Ok(AnnArg { label: None, value })
    }

    fn parse_ann_value(&mut self) -> Result<AnnValue, Diagnostic> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(AnnValue::String(s))
            }
            Tok::Ident(_) => {
                let ty = self.parse_type()?;
                if let Type::Nominal { name, args } = &ty
                    && args.is_empty()
                {
                    return Ok(AnnValue::Ident(name.clone()));
                }
                Ok(AnnValue::Type(ty))
            }
            _ => Err(self.err("expected an annotation value")),
        }
    }

    fn parse_generics(&mut self) -> Result<Vec<GenericParam>, Diagnostic> {
        let mut generics = Vec::new();
        if self.peek() != &Tok::Lt {
            return Ok(generics);
        }
        self.bump();
        while self.peek() != &Tok::Gt {
            let name = self.ident()?;
            let mut bounds = Vec::new();
            if self.peek() == &Tok::Colon {
                self.bump();
                bounds.push(self.parse_type()?);
            }
            generics.push(GenericParam { name, bounds });
            if self.peek() == &Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.expect(&Tok::Gt, "`>`")?;
        Ok(generics)
    }

    fn parse_struct(&mut self) -> Result<StructDecl, Diagnostic> {
        let name = self.ident()?;
        let generics = self.parse_generics()?;
        let generic_names = generics.into_iter().map(|g| g.name).collect();
        if self.peek() != &Tok::LBrace {
            return Ok(StructDecl {
                name,
                generics: generic_names,
                fields: None,
            });
        }
        self.bump();
        let mut fields = Vec::new();
        while self.peek() != &Tok::RBrace {
            self.expect_ident("let")?;
            let fname = self.ident()?;
            self.expect(&Tok::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let default = if self.peek() == &Tok::Eq {
                self.bump();
                Some(self.parse_value()?)
            } else {
                None
            };
            fields.push(Field {
                name: fname,
                ty,
                default,
            });
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(StructDecl {
            name,
            generics: generic_names,
            fields: Some(fields),
        })
    }

    fn parse_enum(&mut self, open: bool) -> Result<EnumDecl, Diagnostic> {
        let name = self.ident()?;
        let generics = self.parse_generics()?;
        self.expect(&Tok::LBrace, "`{`")?;
        let mut cases = Vec::new();
        while self.peek() != &Tok::RBrace {
            let cname = self.ident()?;
            let mut payload = Vec::new();
            if self.peek() == &Tok::LParen {
                self.bump();
                while self.peek() != &Tok::RParen {
                    payload.push(self.parse_type()?);
                    if self.peek() == &Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "`)`")?;
            }
            cases.push(Case {
                name: cname,
                payload,
            });
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(EnumDecl {
            name,
            generics: generics.into_iter().map(|g| g.name).collect(),
            open,
            cases,
        })
    }

    fn parse_trait(&mut self) -> Result<TraitDecl, Diagnostic> {
        let name = self.ident()?;
        let generics = self.parse_generics()?;
        let mut parents = Vec::new();
        if self.peek() == &Tok::Colon {
            self.bump();
            parents.push(self.parse_type()?);
            while self.peek() == &Tok::Comma {
                self.bump();
                parents.push(self.parse_type()?);
            }
        }
        self.expect(&Tok::LBrace, "`{`")?;
        let mut assoc = Vec::new();
        let mut methods = Vec::new();
        while self.peek() != &Tok::RBrace {
            if self.eat_ident("type") {
                let aname = self.ident()?;
                let bound = if self.peek() == &Tok::Colon {
                    self.bump();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                assoc.push(AssocType { name: aname, bound });
            } else if self.eat_ident("function") {
                methods.push(self.parse_function()?);
            } else {
                return Err(self.err("expected `type` or `function` in trait body"));
            }
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(TraitDecl {
            name,
            generics,
            parents,
            assoc,
            methods,
        })
    }

    fn parse_extend(&mut self) -> Result<ExtendDecl, Diagnostic> {
        let target = self.parse_type()?;
        let body = if self.peek() == &Tok::Colon {
            self.bump();
            let mut traits = vec![self.parse_type()?];
            while self.peek() == &Tok::Comma {
                self.bump();
                traits.push(self.parse_type()?);
            }
            ExtendBody::Conformance(traits)
        } else {
            self.expect(&Tok::LBrace, "`{` or `:`")?;
            let mut methods = Vec::new();
            while self.peek() != &Tok::RBrace {
                self.expect_ident("function")?;
                methods.push(self.parse_function()?);
            }
            self.expect(&Tok::RBrace, "`}`")?;
            ExtendBody::Methods(methods)
        };
        Ok(ExtendDecl { target, body })
    }

    fn parse_function(&mut self) -> Result<FunctionDecl, Diagnostic> {
        let name = self.ident()?;
        let generics = self.parse_generics()?;
        self.expect(&Tok::LParen, "`(`")?;
        let mut self_param = None;
        let mut params = Vec::new();
        let mut first = true;
        while self.peek() != &Tok::RParen {
            if !first {
                self.expect(&Tok::Comma, "`,`")?;
            }
            first = false;
            let (conv, is_self) = self.parse_convention_and_self();
            if is_self {
                self_param = Some(match conv {
                    Convention::Owned => SelfParam::Owned,
                    Convention::Borrow => SelfParam::Borrow,
                    Convention::BorrowMut => SelfParam::BorrowMut,
                });
                continue;
            }
            let label = self.ident_before_colon(conv)?;
            self.expect(&Tok::Colon, "`:`")?;
            let (pconv, _) = self.parse_convention_and_self();
            let ty = self.parse_type()?;
            let default = if self.peek() == &Tok::Eq {
                self.bump();
                Some(self.parse_value()?)
            } else {
                None
            };
            params.push(Param {
                label,
                convention: pconv,
                ty,
                default,
            });
        }
        self.expect(&Tok::RParen, "`)`")?;
        let result = if self.peek() == &Tok::Arrow {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(FunctionDecl {
            name,
            generics,
            self_param,
            params,
            result,
        })
    }

    /// Parse an optional `borrow`/`borrow mut` prefix, reporting whether the
    /// next token is the contextual `self`.
    fn parse_convention_and_self(&mut self) -> (Convention, bool) {
        let mut conv = Convention::Owned;
        if self.is_ident("borrow") {
            self.bump();
            conv = if self.eat_ident("mut") {
                Convention::BorrowMut
            } else {
                Convention::Borrow
            };
        }
        let is_self = self.is_ident("self");
        if is_self {
            self.bump();
        }
        (conv, is_self)
    }

    fn ident_before_colon(&mut self, conv: Convention) -> Result<String, Diagnostic> {
        // A parameter label is a plain ident; `borrow` cannot precede a label.
        if conv != Convention::Owned {
            return Err(self.err("a parameter label cannot start with `borrow`"));
        }
        self.ident()
    }

    fn parse_alias(&mut self) -> Result<AliasDecl, Diagnostic> {
        let name = self.ident()?;
        self.expect(&Tok::Eq, "`=`")?;
        let ty = self.parse_type()?;
        Ok(AliasDecl { name, ty })
    }

    /// `let Name: Type = literal`. The name may be a dotted namespace path,
    /// `Rights.read`, which a binding keeps where its language has
    /// namespaces.
    fn parse_const(&mut self) -> Result<ConstDecl, Diagnostic> {
        let mut name = self.ident()?;
        while self.peek() == &Tok::Dot {
            self.bump();
            name.push('.');
            name.push_str(&self.ident()?);
        }
        self.expect(&Tok::Colon, "`:`")?;
        let ty = self.parse_type()?;
        self.expect(&Tok::Eq, "`=`")?;
        // A constant's value is any a field default may be (section 20.3).
        let literal = self.parse_value()?;
        Ok(ConstDecl { name, ty, literal })
    }

    /// A field's default (section 20.3): a literal, `.Case` with an optional
    /// payload, an array of values, a struct or a distinct built of values,
    /// or the name of a constant. Returned as its text, written the way the
    /// `.ridl` reads.
    fn parse_value(&mut self) -> Result<String, Diagnostic> {
        match self.bump() {
            Tok::Number(n) => Ok(n),
            Tok::Str(s) => Ok(format!("{s:?}")),
            Tok::Dot => {
                let case = self.ident()?;
                if self.peek() == &Tok::LParen {
                    self.bump();
                    let payload = self.parse_value()?;
                    self.expect(&Tok::RParen, "`)`")?;
                    return Ok(format!(".{case}({payload})"));
                }
                Ok(format!(".{case}"))
            }
            Tok::LBracket => {
                let items = self.parse_values(&Tok::RBracket, "`]`")?;
                Ok(format!("[{}]", items.join(", ")))
            }
            Tok::Ident(name) => match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let items = self.parse_values(&Tok::RParen, "`)`")?;
                    Ok(format!("{name}({})", items.join(", ")))
                }
                Tok::LBrace => {
                    self.bump();
                    let mut fields = Vec::new();
                    while self.peek() != &Tok::RBrace {
                        let field = self.ident()?;
                        self.expect(&Tok::Colon, "`:`")?;
                        fields.push(format!("{field}: {}", self.parse_value()?));
                        if self.peek() != &Tok::Comma {
                            break;
                        }
                        self.bump();
                    }
                    self.expect(&Tok::RBrace, "`}`")?;
                    if fields.is_empty() {
                        Ok(format!("{name} {{}}"))
                    } else {
                        Ok(format!("{name} {{ {} }}", fields.join(", ")))
                    }
                }
                _ => Ok(name),
            },
            _ => Err(self.err("expected a value")),
        }
    }

    /// Values separated by commas, up to and including `close`.
    fn parse_values(&mut self, close: &Tok, what: &str) -> Result<Vec<String>, Diagnostic> {
        let mut items = Vec::new();
        while self.peek() != close {
            items.push(self.parse_value()?);
            if self.peek() != &Tok::Comma {
                break;
            }
            self.bump();
        }
        self.expect(close, what)?;
        Ok(items)
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        match self.peek().clone() {
            Tok::LBracket => {
                self.bump();
                let inner = self.parse_type()?;
                self.expect(&Tok::RBracket, "`]`")?;
                Ok(Type::Array(Box::new(inner)))
            }
            Tok::LParen => {
                self.bump();
                let mut items = Vec::new();
                while self.peek() != &Tok::RParen {
                    items.push(self.parse_type()?);
                    if self.peek() == &Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "`)`")?;
                if self.peek() == &Tok::Arrow {
                    self.bump();
                    let result = self.parse_type()?;
                    Ok(Type::Function {
                        params: items,
                        result: Box::new(result),
                    })
                } else {
                    Ok(Type::Tuple(items))
                }
            }
            Tok::Ident(word) if word == "borrow" => {
                self.bump();
                self.expect(&Tok::LParen, "`(` after `borrow`")?;
                let mut items = Vec::new();
                while self.peek() != &Tok::RParen {
                    items.push(self.parse_type()?);
                    if self.peek() == &Tok::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Tok::RParen, "`)`")?;
                self.expect(&Tok::Arrow, "`->`")?;
                let result = self.parse_type()?;
                Ok(Type::BorrowFunction {
                    params: items,
                    result: Box::new(result),
                })
            }
            Tok::Ident(word) if word == "Any" => {
                self.bump();
                let path = self.parse_path()?;
                Ok(Type::TraitObject(path))
            }
            Tok::Ident(_) => {
                let name = self.parse_path()?;
                let args = if self.peek() == &Tok::Lt {
                    self.bump();
                    let mut args = Vec::new();
                    while self.peek() != &Tok::Gt {
                        args.push(self.parse_type()?);
                        if self.peek() == &Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.expect(&Tok::Gt, "`>`")?;
                    args
                } else {
                    Vec::new()
                };
                Ok(named_type(&name, args))
            }
            _ => Err(self.err("expected a type")),
        }
    }

    fn expect_ident(&mut self, word: &str) -> Result<(), Diagnostic> {
        if self.eat_ident(word) {
            Ok(())
        } else {
            Err(self.err(&format!("expected `{word}`")))
        }
    }
}

/// Parse `.ridl` text into a [`Module`]. The module name is the file's base name.
///
/// # Errors
/// Returns a [`Diagnostic`] on the first lexical or structural error.
pub fn parse(file: FileId, name: &str, text: &str) -> Result<Module, Diagnostic> {
    parse_spanned(file, name, text).map(|(module, _)| module)
}

/// [`parse`], with the spans of every top-level declaration.
///
/// # Errors
/// Returns a [`Diagnostic`] on the first lexical or structural error.
pub fn parse_spanned(
    file: FileId,
    name: &str,
    text: &str,
) -> Result<(Module, Vec<ItemSpans>), Diagnostic> {
    let toks = lex(file, text)?;
    let mut parser = Parser {
        toks,
        pos: 0,
        spans: Vec::new(),
        sources: Vec::new(),
    };
    let mut module = parser.parse_module(name)?;
    apply_open_annotations(&mut module);
    resolve_generics(&mut module);
    expand_aliases(&mut module);
    Ok((module, parser.spans))
}
