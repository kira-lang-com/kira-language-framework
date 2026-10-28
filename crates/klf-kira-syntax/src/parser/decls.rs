//! Declaration parsing: functions, types, constructs and their members.

use super::Parser;
use crate::ast::{
    AliasDecl, Annotation, AnnotationArgs, CMember, Case, CasePayload, ConstantDecl, ConstructDecl,
    Decl, DeclKind, DistinctDecl, EnumDecl, EnumItem, Expr, FieldInit, FunctionDecl, GParam, Ident,
    Member, MemberKind, Mode, NamedParam, Param, StructDecl, TraitDecl, TraitItem, Type,
};
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_annotations(&mut self) -> Vec<Annotation> {
        let mut annotations = Vec::new();
        while self.at(TokenKind::At) {
            let start = self.current().span;
            self.bump();
            let Some(path) = self.parse_path() else {
                break;
            };
            let args = if self.eat(TokenKind::LParen) {
                let args = self.parse_call_args(TokenKind::RParen);
                self.expect(TokenKind::RParen, "to close annotation arguments");
                AnnotationArgs::Call(args)
            } else if self.eat(TokenKind::LBrace) {
                let pairs = self.parse_annotation_pairs();
                self.expect(TokenKind::RBrace, "to close annotation fields");
                AnnotationArgs::Fields(pairs)
            } else {
                AnnotationArgs::None
            };
            annotations.push(Annotation {
                path,
                args,
                span: self.span_from(start),
            });
            self.skip_newlines();
        }
        annotations
    }

    fn parse_annotation_pairs(&mut self) -> Vec<(Ident, Expr)> {
        let mut pairs = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let Some(key) = self.ident("in annotation field") else {
                break;
            };
            self.expect(TokenKind::Colon, "after annotation field name");
            let Some(value) = self.parse_expr() else {
                break;
            };
            pairs.push((key, value));
            self.skip_newlines();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        pairs
    }

    pub(super) fn parse_decl(&mut self) -> Option<Decl> {
        let annotations = self.parse_annotations();
        let start = self.current().span;
        let kind = match self.peek() {
            TokenKind::Function => DeclKind::Function(self.parse_function()?),
            TokenKind::Struct => DeclKind::Struct(self.parse_struct()?),
            TokenKind::Enum => DeclKind::Enum(self.parse_enum()?),
            TokenKind::Trait => DeclKind::Trait(self.parse_trait()?),
            TokenKind::Construct => DeclKind::Construct(self.parse_construct()?),
            TokenKind::Extend => DeclKind::Extend(self.parse_extend()?),
            TokenKind::Type => DeclKind::Alias(self.parse_alias()?),
            TokenKind::Distinct => DeclKind::Distinct(self.parse_distinct()?),
            TokenKind::Let => DeclKind::Constant(self.parse_constant()?),
            other => {
                self.error(
                    self.current().span,
                    format!("expected a declaration, found {other:?}"),
                );
                return None;
            }
        };
        Some(Decl {
            annotations,
            kind,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_generics(&mut self) -> Vec<GParam> {
        let mut generics = Vec::new();
        if !self.eat(TokenKind::Lt) {
            return generics;
        }
        self.skip_newlines();
        while !self.at(TokenKind::Gt) && !self.at_eof() {
            let Some(name) = self.ident("as a generic parameter") else {
                break;
            };
            let start = name.span;
            let mut bounds = Vec::new();
            if self.eat(TokenKind::Colon) {
                loop {
                    if let Some(bound) = self.parse_type() {
                        bounds.push(bound);
                    }
                    if !self.eat(TokenKind::Amp) {
                        break;
                    }
                }
            }
            generics.push(GParam {
                span: self.span_from(start),
                name,
                bounds,
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::Gt, "to close generic parameters");
        generics
    }

    pub(super) fn parse_function(&mut self) -> Option<FunctionDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a function name")?;
        let generics = self.parse_generics();
        self.expect(TokenKind::LParen, "to open the parameter list")?;
        let params = self.parse_params();
        self.expect(TokenKind::RParen, "to close the parameter list")?;
        let ret = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            self.parse_type()
        } else {
            None
        };
        let body = if self.at(TokenKind::LBrace) {
            Some(self.parse_block()?)
        } else {
            None
        };
        Some(FunctionDecl {
            span: self.span_from(start),
            name,
            generics,
            params,
            ret,
            body,
        })
    }

    pub(super) fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RParen) && !self.at_eof() {
            let Some(param) = self.parse_param() else {
                break;
            };
            params.push(param);
            self.skip_newlines();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        params
    }

    fn parse_mode(&mut self) -> Option<Mode> {
        if self.is_word("borrow") {
            self.bump();
            if self.is_word("mut") {
                self.bump();
                Some(Mode::BorrowMut)
            } else {
                Some(Mode::Borrow)
            }
        } else if self.is_word("move") {
            self.bump();
            Some(Mode::Move)
        } else if self.is_word("copy") {
            self.bump();
            Some(Mode::Copy)
        } else {
            None
        }
    }

    fn parse_param(&mut self) -> Option<Param> {
        let start = self.current().span;
        let mode = self.parse_mode();
        if self.is_word("self") {
            self.bump();
            return Some(Param::SelfParam {
                mode,
                span: self.span_from(start),
            });
        }
        let (label, name) =
            if mode.is_none() && self.at(TokenKind::Ident) && self.peek2() == TokenKind::Ident {
                let label = self.ident("as a parameter label")?;
                let name = self.ident("as a parameter name")?;
                (Some(label), name)
            } else {
                (None, self.ident("as a parameter name")?)
            };
        self.expect(TokenKind::Colon, "after the parameter name")?;
        let inner_mode = self.parse_mode();
        let ty = self.parse_type()?;
        let default = if self.eat(TokenKind::Eq) {
            self.skip_newlines();
            self.parse_expr()
        } else {
            None
        };
        Some(Param::Named(Box::new(NamedParam {
            span: self.span_from(start),
            label,
            name,
            mode: mode.or(inner_mode),
            ty,
            default,
        })))
    }

    fn parse_conforms(&mut self) -> Vec<Type> {
        let mut conforms = Vec::new();
        if self.eat(TokenKind::Colon) {
            loop {
                if let Some(ty) = self.parse_type() {
                    conforms.push(ty);
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
        }
        conforms
    }

    fn parse_struct(&mut self) -> Option<StructDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a struct name")?;
        let generics = self.parse_generics();
        let conforms = self.parse_conforms();
        self.expect(TokenKind::LBrace, "to open the struct body")?;
        let mut members = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if let Some(member) = self.parse_member() {
                members.push(member);
            } else {
                self.recover_to_separator();
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close the struct body")?;
        Some(StructDecl {
            span: self.span_from(start),
            name,
            generics,
            conforms,
            members,
        })
    }

    fn parse_member(&mut self) -> Option<Member> {
        let annotations = self.parse_annotations();
        let start = self.current().span;
        let kind = match self.peek() {
            TokenKind::Function => MemberKind::Method(self.parse_function()?),
            TokenKind::Let | TokenKind::Var => {
                let is_var = self.bump().kind == TokenKind::Var;
                let name = self.ident("as a field name")?;
                self.expect(TokenKind::Colon, "after the field name")?;
                let ty = self.parse_type()?;
                let init = if self.eat(TokenKind::Eq) {
                    self.skip_newlines();
                    Some(FieldInit::Expr(self.parse_expr()?))
                } else if self.at(TokenKind::LBrace) {
                    Some(FieldInit::Block(self.parse_block()?))
                } else {
                    None
                };
                MemberKind::Field {
                    is_var,
                    name,
                    ty,
                    init,
                }
            }
            _ if self.is_word("init") => {
                self.bump();
                self.expect(TokenKind::LParen, "to open the initializer parameters")?;
                let params = self.parse_params();
                self.expect(TokenKind::RParen, "to close the initializer parameters")?;
                let body = self.parse_block()?;
                MemberKind::Init { params, body }
            }
            other => {
                self.error(
                    self.current().span,
                    format!("expected a member, found {other:?}"),
                );
                return None;
            }
        };
        Some(Member {
            annotations,
            kind,
            span: self.span_from(start),
        })
    }

    fn parse_enum(&mut self) -> Option<EnumDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as an enum name")?;
        let generics = self.parse_generics();
        let conforms = self.parse_conforms();
        self.expect(TokenKind::LBrace, "to open the enum body")?;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if self.at(TokenKind::Function) {
                if let Some(function) = self.parse_function() {
                    items.push(EnumItem::Method(function));
                }
            } else if let Some(case) = self.parse_case() {
                items.push(EnumItem::Case(case));
            } else {
                self.recover_to_separator();
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close the enum body")?;
        Some(EnumDecl {
            span: self.span_from(start),
            name,
            generics,
            conforms,
            items,
        })
    }

    fn parse_case(&mut self) -> Option<Case> {
        let name = self.ident("as an enum case")?;
        let start = name.span;
        let payload = if self.eat(TokenKind::LParen) {
            let mut types = Vec::new();
            loop {
                if let Some(ty) = self.parse_type() {
                    types.push(ty);
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "to close the case payload")?;
            CasePayload::Tuple(types)
        } else if self.eat(TokenKind::Colon) {
            let ty = self.parse_type()?;
            let default = if self.eat(TokenKind::Eq) {
                self.parse_expr()
            } else {
                None
            };
            CasePayload::Typed { ty, default }
        } else {
            CasePayload::None
        };
        Some(Case {
            span: self.span_from(start),
            name,
            payload,
        })
    }

    fn parse_trait(&mut self) -> Option<TraitDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a trait name")?;
        let generics = self.parse_generics();
        let conforms = self.parse_conforms();
        self.expect(TokenKind::LBrace, "to open the trait body")?;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if self.at(TokenKind::Type) {
                self.bump();
                let assoc = self.ident("as an associated type name")?;
                let bound = if self.eat(TokenKind::Colon) {
                    self.parse_type()
                } else {
                    None
                };
                items.push(TraitItem::AssocType { name: assoc, bound });
            } else if self.at(TokenKind::Function) {
                if let Some(function) = self.parse_function() {
                    items.push(TraitItem::Method(function));
                }
            } else {
                self.error(self.current().span, "expected a trait member");
                self.recover_to_separator();
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close the trait body")?;
        Some(TraitDecl {
            span: self.span_from(start),
            name,
            generics,
            conforms,
            items,
        })
    }

    fn parse_construct(&mut self) -> Option<ConstructDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a construct name")?;
        let generics = self.parse_generics();
        let params = if self.eat(TokenKind::LParen) {
            let params = self.parse_params();
            self.expect(TokenKind::RParen, "to close the construct parameters")?;
            Some(params)
        } else {
            None
        };
        let mut extends = Vec::new();
        if self.eat(TokenKind::Extends) {
            loop {
                if let Some(ty) = self.parse_type() {
                    extends.push(ty);
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
        }
        self.expect(TokenKind::LBrace, "to open the construct body")?;
        let mut members = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if let Some(member) = self.parse_cmember() {
                members.push(member);
            } else {
                self.recover_to_separator();
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close the construct body")?;
        Some(ConstructDecl {
            span: self.span_from(start),
            name,
            generics,
            params,
            extends,
            members,
        })
    }

    fn parse_cmember(&mut self) -> Option<CMember> {
        if self.is_word("requires") {
            self.bump();
            self.expect(TokenKind::LBrace, "to open a requires block")?;
            let mut functions = Vec::new();
            self.skip_newlines();
            while !self.at(TokenKind::RBrace) && !self.at_eof() {
                if let Some(function) = self.parse_function() {
                    functions.push(function);
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RBrace, "to close a requires block")?;
            return Some(CMember::Requires(functions));
        }
        if self.at(TokenKind::Ident) && self.peek2() == TokenKind::LBrace {
            let name = self.ident("as a content block name")?;
            let body = self.parse_block()?;
            return Some(CMember::ContentBlock { name, body });
        }
        Some(CMember::Member(Box::new(self.parse_member()?)))
    }

    fn parse_extend(&mut self) -> Option<crate::ast::ExtendDecl> {
        let start = self.current().span;
        self.bump();
        let ty = self.parse_type()?;
        let conforms = self.parse_conforms();
        self.expect(TokenKind::LBrace, "to open the extend body")?;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if let Some(member) = self.parse_member() {
                items.push(member);
            } else {
                self.recover_to_separator();
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close the extend body")?;
        Some(crate::ast::ExtendDecl {
            span: self.span_from(start),
            ty,
            conforms,
            items,
        })
    }

    fn parse_alias(&mut self) -> Option<AliasDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a type alias name")?;
        let generics = self.parse_generics();
        self.expect(TokenKind::Eq, "in a type alias")?;
        let ty = self.parse_type()?;
        Some(AliasDecl {
            span: self.span_from(start),
            name,
            generics,
            ty,
        })
    }

    fn parse_distinct(&mut self) -> Option<DistinctDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a distinct type name")?;
        self.expect(TokenKind::Eq, "in a distinct type")?;
        let ty = self.parse_type()?;
        Some(DistinctDecl {
            span: self.span_from(start),
            name,
            ty,
        })
    }

    fn parse_constant(&mut self) -> Option<ConstantDecl> {
        let start = self.current().span;
        self.bump();
        let name = self.ident("as a constant name")?;
        let ty = if self.eat(TokenKind::Colon) {
            self.parse_type()
        } else {
            None
        };
        self.expect(TokenKind::Eq, "in a constant")?;
        self.skip_newlines();
        let value = self.parse_expr()?;
        Some(ConstantDecl {
            span: self.span_from(start),
            name,
            ty,
            value,
        })
    }
}
