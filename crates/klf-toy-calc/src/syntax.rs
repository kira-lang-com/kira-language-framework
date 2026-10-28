//! The toy calculator's AST and a hand-written recursive-descent parser.
//!
//! Grammar (all values are `Int`/i64):
//! ```text
//! program := { def }
//! def     := "def" ident "(" [ ident { "," ident } ] ")" "=" expr
//! expr    := "if" expr cmp expr "then" expr "else" expr | add
//! add     := mul { ("+"|"-") mul }
//! mul     := unary { ("*"|"/"|"%") unary }
//! unary   := "-" unary | prim
//! prim    := int | ident [ "(" [ expr { "," expr } ] ")" ] | "(" expr ")"
//! cmp     := "==" | "!=" | "<" | "<=" | ">" | ">="
//! ```

use klf_diag::Diagnostic;
use klf_source::{SourceFile, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Int(i64),
    Var(String),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, Span),
    If {
        lhs: Box<Expr>,
        op: CmpOp,
        rhs: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expr,
    pub span: Span,
}

pub fn parse(file: &SourceFile) -> (Vec<Def>, Vec<Diagnostic>) {
    let mut parser = Parser {
        text: file.text().as_bytes(),
        pos: 0,
        file: file.id(),
        diagnostics: Vec::new(),
    };
    let defs = parser.parse_program();
    (defs, parser.diagnostics)
}

struct Parser<'a> {
    text: &'a [u8],
    pos: usize,
    file: klf_core::FileId,
    diagnostics: Vec<Diagnostic>,
}

impl Parser<'_> {
    fn span(&self, start: usize) -> Span {
        Span::new(
            self.file,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(self.pos).unwrap_or(u32::MAX),
        )
    }

    fn error(&mut self, start: usize, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error("CALC-E0001", message).with_label(self.span(start), "here"));
    }

    fn skip_trivia(&mut self) {
        while let Some(&byte) = self.text.get(self.pos) {
            if byte == b'#' {
                while let Some(&b) = self.text.get(self.pos) {
                    if b == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else if byte.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s.as_bytes())
    }

    /// Consumes a punctuation string after trivia, returning whether it matched.
    fn eat(&mut self, s: &str) -> bool {
        self.skip_trivia();
        if self.starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn keyword(&mut self, word: &str) -> bool {
        self.skip_trivia();
        let end = self.pos + word.len();
        if self.starts_with(word) && !self.text.get(end).copied().is_some_and(is_ident_continue) {
            self.pos = end;
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Option<(String, usize)> {
        self.skip_trivia();
        let start = self.pos;
        if !self.peek().is_some_and(is_ident_start) {
            return None;
        }
        while self.peek().is_some_and(is_ident_continue) {
            self.pos += 1;
        }
        Some((
            String::from_utf8_lossy(&self.text[start..self.pos]).into_owned(),
            start,
        ))
    }

    fn parse_program(&mut self) -> Vec<Def> {
        let mut defs = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek().is_none() {
                break;
            }
            if let Some(def) = self.parse_def() {
                defs.push(def);
            } else {
                self.recover();
            }
        }
        defs
    }

    fn recover(&mut self) {
        // Skip to the next top-level `def`.
        loop {
            self.skip_trivia();
            if self.peek().is_none() || self.starts_with("def") {
                return;
            }
            self.pos += 1;
        }
    }

    fn parse_def(&mut self) -> Option<Def> {
        let start = self.pos;
        if !self.keyword("def") {
            self.error(start, "expected `def`");
            return None;
        }
        let (name, _) = self.ident().or_else(|| {
            self.error(start, "expected a function name");
            None
        })?;
        if !self.eat("(") {
            self.error(start, "expected `(`");
            return None;
        }
        let mut params = Vec::new();
        if !self.eat(")") {
            loop {
                let (param, _) = self.ident()?;
                params.push(param);
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") {
                    self.error(self.pos, "expected `,` or `)`");
                    return None;
                }
            }
        }
        if !self.eat("=") {
            self.error(self.pos, "expected `=`");
            return None;
        }
        let body = self.parse_expr()?;
        Some(Def {
            name,
            params,
            body,
            span: self.span(start),
        })
    }

    fn parse_expr(&mut self) -> Option<Expr> {
        if self.keyword("if") {
            let lhs = self.parse_add()?;
            let op = self.parse_cmp()?;
            let rhs = self.parse_add()?;
            if !self.keyword("then") {
                self.error(self.pos, "expected `then`");
                return None;
            }
            let then = self.parse_expr()?;
            if !self.keyword("else") {
                self.error(self.pos, "expected `else`");
                return None;
            }
            let els = self.parse_expr()?;
            return Some(Expr::If {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
                then: Box::new(then),
                els: Box::new(els),
            });
        }
        self.parse_add()
    }

    fn parse_cmp(&mut self) -> Option<CmpOp> {
        let op = if self.eat("==") {
            CmpOp::Eq
        } else if self.eat("!=") {
            CmpOp::Ne
        } else if self.eat("<=") {
            CmpOp::Le
        } else if self.eat(">=") {
            CmpOp::Ge
        } else if self.eat("<") {
            CmpOp::Lt
        } else if self.eat(">") {
            CmpOp::Gt
        } else {
            self.error(self.pos, "expected a comparison operator");
            return None;
        };
        Some(op)
    }

    fn parse_add(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = if self.eat("+") {
                BinOp::Add
            } else if self.eat_minus_not_arrow() {
                BinOp::Sub
            } else {
                break;
            };
            let rhs = self.parse_mul()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Some(lhs)
    }

    fn eat_minus_not_arrow(&mut self) -> bool {
        self.skip_trivia();
        if self.peek() == Some(b'-') {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_mul(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = if self.eat("*") {
                BinOp::Mul
            } else if self.eat("/") {
                BinOp::Div
            } else if self.eat("%") {
                BinOp::Rem
            } else {
                break;
            };
            let rhs = self.parse_unary()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Some(lhs)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        if self.eat_minus_not_arrow() {
            return Some(Expr::Neg(Box::new(self.parse_unary()?)));
        }
        self.parse_prim()
    }

    fn parse_prim(&mut self) -> Option<Expr> {
        self.skip_trivia();
        let start = self.pos;
        if self.eat("(") {
            let expr = self.parse_expr()?;
            if !self.eat(")") {
                self.error(start, "expected `)`");
                return None;
            }
            return Some(expr);
        }
        if self.peek().is_some_and(|b| b.is_ascii_digit()) {
            while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'_') {
                self.pos += 1;
            }
            let text: String = self.text[start..self.pos]
                .iter()
                .filter(|&&b| b != b'_')
                .map(|&b| b as char)
                .collect();
            let Ok(value) = text.parse::<i64>() else {
                self.error(start, "integer literal out of range");
                return None;
            };
            return Some(Expr::Int(value));
        }
        let (name, name_start) = self.ident().or_else(|| {
            self.error(start, "expected an expression");
            None
        })?;
        if self.eat("(") {
            let mut args = Vec::new();
            if !self.eat(")") {
                loop {
                    args.push(self.parse_expr()?);
                    if self.eat(")") {
                        break;
                    }
                    if !self.eat(",") {
                        self.error(self.pos, "expected `,` or `)`");
                        return None;
                    }
                }
            }
            return Some(Expr::Call(name, args, self.span(name_start)));
        }
        Some(Expr::Var(name))
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
