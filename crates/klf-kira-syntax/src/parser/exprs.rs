//! Expression parsing (Appendix A.1 `expr` and below).

use super::{Parser, bin_op, binary_prec};
use crate::ast::{Arg, Expr, ExprKind, Type, TypeKind, UnaryOp};
use crate::token::TokenKind;
use klf_core::Name;
use klf_source::Span;

impl Parser<'_> {
    pub(super) fn parse_expr(&mut self) -> Option<Expr> {
        let start = self.parse_ternary()?;
        if self.eat(TokenKind::DotDot) {
            self.skip_newlines();
            let end = self.parse_ternary()?;
            let span = Span::new(start.span.file, start.span.start, end.span.end);
            return Some(Expr {
                kind: ExprKind::Range {
                    start: Box::new(start),
                    end: Box::new(end),
                },
                span,
            });
        }
        Some(start)
    }

    fn parse_ternary(&mut self) -> Option<Expr> {
        let cond = self.parse_binary(0)?;
        if self.at(TokenKind::Question) {
            self.bump();
            self.skip_newlines();
            let then = self.parse_expr()?;
            self.expect(TokenKind::Colon, "in a conditional expression")?;
            self.skip_newlines();
            let otherwise = self.parse_expr()?;
            let span = Span::new(cond.span.file, cond.span.start, otherwise.span.end);
            return Some(Expr {
                kind: ExprKind::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    otherwise: Box::new(otherwise),
                },
                span,
            });
        }
        Some(cond)
    }

    fn parse_binary(&mut self, min_prec: u8) -> Option<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let token = self.peek();
            if token == TokenKind::As || token == TokenKind::Is {
                if binary_prec(token).expect("as/is have a precedence") < min_prec {
                    break;
                }
                self.bump();
                self.skip_newlines();
                let ty = self.parse_type()?;
                let span = Span::new(lhs.span.file, lhs.span.start, ty.span.end);
                let kind = if token == TokenKind::As {
                    ExprKind::As {
                        expr: Box::new(lhs),
                        ty,
                    }
                } else {
                    ExprKind::Is {
                        expr: Box::new(lhs),
                        ty,
                    }
                };
                lhs = Expr { kind, span };
                continue;
            }
            let Some(prec) = binary_prec(token) else {
                break;
            };
            if prec < min_prec {
                break;
            }
            let op = bin_op(token).expect("token has a binary precedence");
            self.bump();
            self.skip_newlines();
            let rhs = self.parse_binary(prec + 1)?;
            let span = Span::new(lhs.span.file, lhs.span.start, rhs.span.end);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Some(lhs)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        let start = self.current().span;
        if self.eat(TokenKind::Try) {
            let operand = self.parse_unary()?;
            let span = self.span_from(start);
            return Some(Expr {
                kind: ExprKind::Try(Box::new(operand)),
                span,
            });
        }
        let op = match self.peek() {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Tilde => Some(UnaryOp::BitNot),
            TokenKind::Ident if self.is_word("move") => Some(UnaryOp::Move),
            TokenKind::Ident if self.is_word("copy") => Some(UnaryOp::Copy),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let operand = self.parse_unary()?;
            let span = self.span_from(start);
            return Some(Expr {
                kind: ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                TokenKind::Dot if self.peek2() == TokenKind::Ident => {
                    self.bump();
                    let name = self.ident("as a member name")?;
                    let span = Span::new(expr.span.file, expr.span.start, name.span.end);
                    expr = Expr {
                        kind: ExprKind::Field {
                            base: Box::new(expr),
                            name,
                        },
                        span,
                    };
                }
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_call_args(TokenKind::RParen);
                    let end = self.expect(TokenKind::RParen, "to close a call")?.span;
                    let span = Span::new(expr.span.file, expr.span.start, end.end);
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    self.skip_newlines();
                    let index = self.parse_expr()?;
                    self.skip_newlines();
                    let end = self.expect(TokenKind::RBracket, "to close an index")?.span;
                    let span = Span::new(expr.span.file, expr.span.start, end.end);
                    expr = Expr {
                        kind: ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Some(expr)
    }

    pub(super) fn parse_call_args(&mut self, close: TokenKind) -> Vec<Arg> {
        let saved = self.no_struct;
        self.no_struct = false;
        let mut args = Vec::new();
        self.skip_newlines();
        while !self.at(close) && !self.at_eof() {
            let label = if self.at(TokenKind::Ident) && self.peek2() == TokenKind::Colon {
                let label = self.ident("as an argument label");
                self.eat(TokenKind::Colon);
                self.skip_newlines();
                label
            } else {
                None
            };
            let Some(value) = self.parse_expr() else {
                break;
            };
            args.push(Arg { label, value });
            self.skip_newlines();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.no_struct = saved;
        args
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let start = self.current().span;
        let kind = match self.peek() {
            TokenKind::Int => {
                let text = Name::new(self.lexeme(self.current().span));
                self.bump();
                ExprKind::Int(text)
            }
            TokenKind::Float => {
                let text = Name::new(self.lexeme(self.current().span));
                self.bump();
                ExprKind::Float(text)
            }
            TokenKind::Str => {
                let value = decode_string(self.lexeme(self.current().span));
                self.bump();
                ExprKind::Str(value)
            }
            TokenKind::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            TokenKind::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            TokenKind::Dot if self.peek2() == TokenKind::Ident => {
                self.bump();
                let name = self.ident("as an enum case")?;
                let args = if self.eat(TokenKind::LParen) {
                    let args = self.parse_call_args(TokenKind::RParen);
                    self.expect(TokenKind::RParen, "to close a case payload")?;
                    Some(args)
                } else {
                    None
                };
                ExprKind::EnumLit { name, args }
            }
            TokenKind::LParen => return self.parse_paren_expr(),
            TokenKind::LBracket => return self.parse_collection_literal(),
            TokenKind::LBrace => return self.parse_closure(),
            TokenKind::Ident => {
                let mut path = self.parse_path()?;
                // `name!(...)` names a macro: the `!` joins the name, and the
                // call that follows is the invocation.
                if self.at(TokenKind::Bang)
                    && self.peek2() == TokenKind::LParen
                    && self.current().span.start == path.span.end
                {
                    let bang = self.bump().span;
                    if let Some(last) = path.segments.last_mut() {
                        last.name = Name::new(format!("{}!", last.name.as_str()));
                        last.span.end = bang.end;
                    }
                    path.span.end = bang.end;
                    return Some(Expr {
                        kind: ExprKind::Path(path),
                        span: self.span_from(start),
                    });
                }
                if !self.no_struct && self.at(TokenKind::LBrace) {
                    let ty = Type {
                        span: path.span,
                        kind: TypeKind::Named {
                            path,
                            args: Vec::new(),
                        },
                    };
                    return self.parse_struct_literal(ty, start);
                }
                ExprKind::Path(path)
            }
            other => {
                self.error(
                    self.current().span,
                    format!("expected an expression, found {other:?}"),
                );
                return None;
            }
        };
        Some(Expr {
            kind,
            span: self.span_from(start),
        })
    }

    fn parse_paren_expr(&mut self) -> Option<Expr> {
        let start = self.current().span;
        self.bump();
        let saved = self.no_struct;
        self.no_struct = false;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RParen) && !self.at_eof() {
            items.push(self.parse_expr()?);
            self.skip_newlines();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.no_struct = saved;
        self.expect(TokenKind::RParen, "to close a parenthesized expression")?;
        if items.len() == 1 {
            return Some(items.into_iter().next().expect("one item"));
        }
        Some(Expr {
            kind: ExprKind::Tuple(items),
            span: self.span_from(start),
        })
    }

    fn parse_collection_literal(&mut self) -> Option<Expr> {
        let start = self.current().span;
        self.bump();
        let saved = self.no_struct;
        self.no_struct = false;
        self.skip_newlines();
        if self.eat(TokenKind::RBracket) {
            self.no_struct = saved;
            return Some(Expr {
                kind: ExprKind::ArrayLit(Vec::new()),
                span: self.span_from(start),
            });
        }
        let first = self.parse_expr()?;
        let kind = if self.eat(TokenKind::Colon) {
            self.skip_newlines();
            let value = self.parse_expr()?;
            let mut pairs = vec![(first, value)];
            self.skip_newlines();
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RBracket) {
                    break;
                }
                let key = self.parse_expr()?;
                self.expect(TokenKind::Colon, "in a map literal")?;
                self.skip_newlines();
                let value = self.parse_expr()?;
                pairs.push((key, value));
                self.skip_newlines();
            }
            ExprKind::MapLit(pairs)
        } else {
            let mut items = vec![first];
            self.skip_newlines();
            while self.eat(TokenKind::Comma) {
                self.skip_newlines();
                if self.at(TokenKind::RBracket) {
                    break;
                }
                items.push(self.parse_expr()?);
                self.skip_newlines();
            }
            ExprKind::ArrayLit(items)
        };
        self.no_struct = saved;
        self.expect(TokenKind::RBracket, "to close a collection literal")?;
        Some(Expr {
            kind,
            span: self.span_from(start),
        })
    }

    fn parse_closure(&mut self) -> Option<Expr> {
        let start = self.current().span;
        self.bump();
        let mut params = Vec::new();
        if self.closure_has_params() {
            loop {
                params.push(self.ident("as a closure parameter")?);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            if !self.eat(TokenKind::In) {
                self.error(
                    self.current().span,
                    "expected `in` after closure parameters",
                );
            }
        }
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            } else {
                self.recover_to_separator();
            }
            self.skip_newlines();
            self.eat(TokenKind::Semicolon);
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "to close a closure")?;
        Some(Expr {
            kind: ExprKind::Closure {
                params,
                body: stmts,
            },
            span: self.span_from(start),
        })
    }

    /// Looks ahead over `ident (, ident)*` for a following `in`, marking the
    /// brace group as a parameterized closure rather than a bare block.
    fn closure_has_params(&self) -> bool {
        let mut offset = self.pos;
        loop {
            let Some(token) = self.tokens.get(offset) else {
                return false;
            };
            match token.kind {
                TokenKind::In => return true,
                TokenKind::Ident | TokenKind::Comma => offset += 1,
                _ => return false,
            }
        }
    }

    fn parse_struct_literal(&mut self, ty: Type, start: Span) -> Option<Expr> {
        self.bump();
        let saved = self.no_struct;
        self.no_struct = false;
        let mut fields = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let Some(name) = self.ident("as a struct field") else {
                break;
            };
            self.expect(TokenKind::Eq, "after a struct field name")?;
            self.skip_newlines();
            let value = self.parse_expr()?;
            fields.push((name, value));
            self.skip_newlines();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.no_struct = saved;
        self.expect(TokenKind::RBrace, "to close a struct literal")?;
        Some(Expr {
            kind: ExprKind::StructLit { ty, fields },
            span: self.span_from(start),
        })
    }
}

/// Decodes a string literal's source text (with surrounding quotes) into its
/// value, resolving the escape sequences KLF Kira accepts.
pub(super) fn decode_string(raw: &str) -> String {
    if let Some(block) = raw.strip_prefix("\"\"\"") {
        return decode_escapes(&dedent(block.strip_suffix("\"\"\"").unwrap_or(block)));
    }
    let inner = raw.strip_prefix('"').unwrap_or(raw);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    decode_escapes(inner)
}

/// The text of a `"""` literal: the lines between the delimiters' own lines,
/// each without the indentation of the closing delimiter.
fn dedent(block: &str) -> String {
    let block = block.strip_prefix('\n').unwrap_or(block);
    let (body, indent) = match block.rfind('\n') {
        Some(at) if block[at + 1..].trim().is_empty() => (&block[..at], &block[at + 1..]),
        _ => (block, ""),
    };
    body.split('\n')
        .map(|line| {
            line.strip_prefix(indent)
                .unwrap_or_else(|| line.trim_start())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_escapes(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('u') => decode_unicode_escape(&mut chars, &mut out),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn decode_unicode_escape(chars: &mut std::str::Chars<'_>, out: &mut String) {
    if chars.clone().next() != Some('{') {
        out.push('u');
        return;
    }
    chars.next();
    let mut hex = String::new();
    for ch in chars.by_ref() {
        if ch == '}' {
            break;
        }
        hex.push(ch);
    }
    if let Some(value) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
        out.push(value);
    }
}
