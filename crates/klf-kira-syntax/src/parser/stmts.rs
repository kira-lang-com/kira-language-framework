//! Statement and block parsing (Appendix A.1 `block`, `stmt`, `pattern`).

use super::exprs::decode_string;
use super::{Parser, assign_op};
use crate::ast::{Block, Expr, MatchArm, Pattern, PatternKind, Stmt, StmtKind};
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_block(&mut self) -> Option<Block> {
        let start = self.expect(TokenKind::LBrace, "to open a block")?.span;
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
        self.expect(TokenKind::RBrace, "to close a block")?;
        Some(Block {
            stmts,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_stmt(&mut self) -> Option<Stmt> {
        let start = self.current().span;
        let kind = match self.peek() {
            TokenKind::Let | TokenKind::Var => self.parse_let_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => {
                self.bump();
                let cond = self.parse_cond()?;
                let body = self.parse_block()?;
                Some(StmtKind::While { cond, body })
            }
            TokenKind::For => {
                self.bump();
                let var = self.ident("as a loop variable")?;
                self.expect(TokenKind::In, "in a for loop")?;
                let iter = self.parse_cond()?;
                let body = self.parse_block()?;
                Some(StmtKind::For { var, iter, body })
            }
            TokenKind::Match => self.parse_match_stmt(),
            TokenKind::Attempt => self.parse_attempt_stmt(),
            TokenKind::Return => {
                self.bump();
                let value = if matches!(
                    self.peek(),
                    TokenKind::Newline | TokenKind::RBrace | TokenKind::Semicolon | TokenKind::Eof
                ) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Some(StmtKind::Return(value))
            }
            TokenKind::Break => {
                self.bump();
                Some(StmtKind::Break)
            }
            TokenKind::Continue => {
                self.bump();
                Some(StmtKind::Continue)
            }
            _ => self.parse_expr_or_assign(),
        }?;
        Some(Stmt {
            kind,
            span: self.span_from(start),
        })
    }

    fn parse_let_stmt(&mut self) -> Option<StmtKind> {
        let is_var = self.bump().kind == TokenKind::Var;
        let name = self.ident("in a binding")?;
        let ty = if self.eat(TokenKind::Colon) {
            self.parse_type()
        } else {
            None
        };
        self.expect(TokenKind::Eq, "in a binding")?;
        self.skip_newlines();
        let value = self.parse_expr()?;
        Some(StmtKind::Let {
            is_var,
            name,
            ty,
            value,
        })
    }

    fn parse_if_stmt(&mut self) -> Option<StmtKind> {
        let mut branches = Vec::new();
        self.bump();
        let cond = self.parse_cond()?;
        let body = self.parse_block()?;
        branches.push((cond, body));
        let mut else_block = None;
        loop {
            self.skip_newlines();
            // `else ->` opens the next arm of a `String` match, not this
            // statement's else branch.
            if !self.at(TokenKind::Else) || self.peek2() == TokenKind::Arrow {
                break;
            }
            self.bump();
            if self.at(TokenKind::If) {
                self.bump();
                let cond = self.parse_cond()?;
                let body = self.parse_block()?;
                branches.push((cond, body));
            } else {
                else_block = Some(self.parse_block()?);
                break;
            }
        }
        Some(StmtKind::If {
            branches,
            else_block,
        })
    }

    fn parse_match_stmt(&mut self) -> Option<StmtKind> {
        self.bump();
        let scrutinee = self.parse_cond()?;
        self.expect(TokenKind::LBrace, "to open a match body")?;
        let arms = self.parse_arms(true);
        self.expect(TokenKind::RBrace, "to close a match body")?;
        Some(StmtKind::Match { scrutinee, arms })
    }

    fn parse_attempt_stmt(&mut self) -> Option<StmtKind> {
        self.bump();
        let body = self.parse_block()?;
        self.skip_newlines();
        if !self.is_word("handle") {
            self.error(self.current().span, "expected `handle` after `attempt`");
            return None;
        }
        self.bump();
        self.expect(TokenKind::LBrace, "to open a handle body")?;
        let handlers = self.parse_arms(false);
        self.expect(TokenKind::RBrace, "to close a handle body")?;
        Some(StmtKind::Attempt { body, handlers })
    }

    /// Parses match or handle arms. In a match, an arm is `pattern -> stmt`;
    /// in a handle, `pattern block`.
    fn parse_arms(&mut self, with_arrow: bool) -> Vec<MatchArm> {
        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            let Some(pattern) = self.parse_pattern() else {
                self.recover_to_separator();
                self.ensure_progress(before);
                self.skip_newlines();
                continue;
            };
            let start = pattern.span;
            let body = if with_arrow {
                self.expect(TokenKind::Arrow, "in a match arm");
                self.skip_newlines();
                if self.at(TokenKind::LBrace) {
                    self.parse_block()
                } else {
                    self.parse_stmt().map(|stmt| Block {
                        span: stmt.span,
                        stmts: vec![stmt],
                    })
                }
            } else {
                self.parse_block()
            };
            if let Some(body) = body {
                arms.push(MatchArm {
                    span: self.span_from(start),
                    pattern,
                    body,
                });
            }
            self.skip_newlines();
        }
        arms
    }

    fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current().span;
        if self.eat(TokenKind::Else) {
            return Some(Pattern {
                kind: PatternKind::Otherwise,
                span: start,
            });
        }
        if self.at(TokenKind::Str) {
            let mut texts = Vec::new();
            loop {
                if !self.at(TokenKind::Str) {
                    self.error(
                        self.current().span,
                        "expected a string literal in a pattern",
                    );
                    return None;
                }
                texts.push(decode_string(self.lexeme(self.current().span)));
                self.bump();
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                // A long list continues on the next line after a comma.
                self.skip_newlines();
            }
            return Some(Pattern {
                kind: PatternKind::Text(texts),
                span: self.span_from(start),
            });
        }
        let name = self.ident("as a pattern")?;
        let mut bindings = Vec::new();
        if self.eat(TokenKind::LParen) {
            loop {
                bindings.push(self.ident("as a pattern binding")?);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "to close a pattern");
        }
        Some(Pattern {
            span: self.span_from(start),
            kind: PatternKind::Case { name, bindings },
        })
    }

    fn parse_expr_or_assign(&mut self) -> Option<StmtKind> {
        let target = self.parse_expr()?;
        if let Some(op) = assign_op(self.peek()) {
            self.bump();
            self.skip_newlines();
            let value = self.parse_expr()?;
            Some(StmtKind::Assign { target, op, value })
        } else {
            Some(StmtKind::Expr(target))
        }
    }

    /// Parses a condition or scrutinee expression: a struct literal here is
    /// ambiguous with the following block, so it is disallowed at the top
    /// level (still allowed inside brackets).
    pub(super) fn parse_cond(&mut self) -> Option<Expr> {
        let saved = self.no_struct;
        self.no_struct = true;
        let expr = self.parse_expr();
        self.no_struct = saved;
        expr
    }
}
