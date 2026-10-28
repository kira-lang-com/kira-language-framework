//! Type parsing (Appendix A.1 `type`).

use super::Parser;
use crate::ast::{Type, TypeKind};
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_type(&mut self) -> Option<Type> {
        let start = self.current().span;
        let kind = match self.peek() {
            TokenKind::LBracket => {
                self.bump();
                let element = self.parse_type()?;
                self.expect(TokenKind::RBracket, "to close an array type")?;
                TypeKind::Array(Box::new(element))
            }
            TokenKind::LParen => {
                self.bump();
                let mut types = Vec::new();
                self.skip_newlines();
                while !self.at(TokenKind::RParen) && !self.at_eof() {
                    types.push(self.parse_type()?);
                    self.skip_newlines();
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::RParen, "to close a tuple or function type")?;
                if self.eat(TokenKind::Arrow) {
                    let ret = self.parse_type()?;
                    TypeKind::Function {
                        params: types,
                        ret: Box::new(ret),
                    }
                } else {
                    TypeKind::Tuple(types)
                }
            }
            TokenKind::Ident if self.is_word("Any") => {
                self.bump();
                TypeKind::Any(Box::new(self.parse_type()?))
            }
            TokenKind::Ident if self.is_word("some") => {
                self.bump();
                TypeKind::Some(Box::new(self.parse_type()?))
            }
            TokenKind::Ident => {
                let path = self.parse_path()?;
                let mut args = Vec::new();
                if self.eat(TokenKind::Lt) {
                    loop {
                        args.push(self.parse_type()?);
                        if !self.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(TokenKind::Gt, "to close type arguments")?;
                }
                TypeKind::Named { path, args }
            }
            other => {
                self.error(
                    self.current().span,
                    format!("expected a type, found {other:?}"),
                );
                return None;
            }
        };
        Some(Type {
            kind,
            span: self.span_from(start),
        })
    }
}
