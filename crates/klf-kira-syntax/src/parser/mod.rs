//! The KLF Kira parser: token stream to AST (Appendix A.1).
//!
//! Recursive descent. Newlines are soft separators: they end statements and
//! declarations but are skipped inside bracketed groups and after binary
//! operators, so an expression may wrap after an operator.
//!
//! The parser is split across submodules by grammar area (`decls`, `types`,
//! `stmts`, `exprs`); each adds methods to [`Parser`] in its own `impl` block.

mod decls;
mod exprs;
mod stmts;
mod types;

use crate::ast::{AssignOp, BinOp, File, Ident, Import, Path};
use crate::lexer::lex;
use crate::token::{Token, TokenKind};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_source::{SourceFile, Span};

/// The result of parsing one source file.
#[derive(Clone)]
pub struct Parsed {
    pub file: File,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lexes and parses a whole source file into a [`File`] AST.
#[must_use]
pub fn parse(source: &SourceFile) -> Parsed {
    let lexed = lex(source);
    let mut parser = Parser {
        source,
        tokens: lexed.tokens,
        pos: 0,
        diagnostics: lexed.diagnostics,
        no_struct: false,
    };
    let file = parser.parse_file();
    Parsed {
        file,
        diagnostics: parser.diagnostics,
    }
}

struct Parser<'a> {
    source: &'a SourceFile,
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    no_struct: bool,
}

impl Parser<'_> {
    fn peek(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    fn peek2(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 1)
            .map_or(TokenKind::Eof, |t| t.kind)
    }

    fn current(&self) -> Token {
        self.tokens[self.pos]
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos];
        if !self.at_eof() {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(TokenKind::Newline) {
            self.bump();
        }
    }

    fn lexeme(&self, span: Span) -> &str {
        &self.source.text()[span.start as usize..span.end as usize]
    }

    fn is_word(&self, word: &str) -> bool {
        self.at(TokenKind::Ident) && self.lexeme(self.current().span) == word
    }

    fn span_from(&self, start: Span) -> Span {
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        Span::new(start.file, start.start, end.end)
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error("KLFK-E0002", message).with_label(span, "here"));
    }

    fn expect(&mut self, kind: TokenKind, context: &str) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            let found = self.peek();
            self.error(
                self.current().span,
                format!("expected {kind:?} {context}, found {found:?}"),
            );
            None
        }
    }

    /// Forces the cursor forward by one token when a loop body consumed
    /// nothing, so recovery at a stray closing brace cannot spin forever.
    fn ensure_progress(&mut self, before: usize) {
        if self.pos == before && !self.at_eof() {
            self.bump();
        }
    }

    /// Skips tokens until a newline, a closing brace or end of input, used to
    /// recover after a malformed declaration or statement.
    fn recover_to_separator(&mut self) {
        while !matches!(
            self.peek(),
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
        ) {
            self.bump();
        }
    }

    fn ident(&mut self, context: &str) -> Option<Ident> {
        let token = self.expect(TokenKind::Ident, context)?;
        Some(Ident {
            name: Name::new(self.lexeme(token.span)),
            span: token.span,
        })
    }

    fn parse_file(&mut self) -> File {
        let mut imports = Vec::new();
        let mut decls = Vec::new();
        self.skip_newlines();
        while self.at(TokenKind::Import) {
            if let Some(import) = self.parse_import() {
                imports.push(import);
            }
            self.skip_newlines();
        }
        while !self.at_eof() {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }
            let before = self.pos;
            if let Some(decl) = self.parse_decl() {
                decls.push(decl);
            } else {
                self.recover_to_separator();
            }
            self.ensure_progress(before);
            self.skip_newlines();
        }
        File { imports, decls }
    }

    fn parse_import(&mut self) -> Option<Import> {
        let start = self.current().span;
        self.bump();
        let path = self.parse_path()?;
        Some(Import {
            path,
            span: self.span_from(start),
        })
    }

    fn parse_path(&mut self) -> Option<Path> {
        let first = self.ident("in path")?;
        let start = first.span;
        let mut segments = vec![first];
        while self.at(TokenKind::Dot) && self.peek2() == TokenKind::Ident {
            self.bump();
            segments.push(self.ident("in path")?);
        }
        Some(Path {
            span: self.span_from(start),
            segments,
        })
    }
}

fn assign_op(kind: TokenKind) -> Option<AssignOp> {
    let op = match kind {
        TokenKind::Eq => AssignOp::Assign,
        TokenKind::PlusEq => AssignOp::Add,
        TokenKind::MinusEq => AssignOp::Sub,
        TokenKind::StarEq => AssignOp::Mul,
        TokenKind::SlashEq => AssignOp::Div,
        TokenKind::PercentEq => AssignOp::Rem,
        TokenKind::AmpEq => AssignOp::And,
        TokenKind::PipeEq => AssignOp::Or,
        TokenKind::CaretEq => AssignOp::Xor,
        TokenKind::ShlEq => AssignOp::Shl,
        TokenKind::ShrEq => AssignOp::Shr,
        _ => return None,
    };
    Some(op)
}

/// Binary precedence, low binds looser. `as`/`is` sit at the top (A.1).
fn binary_prec(kind: TokenKind) -> Option<u8> {
    let prec = match kind {
        TokenKind::OrOr => 1,
        TokenKind::AndAnd => 2,
        TokenKind::Pipe => 3,
        TokenKind::Caret => 4,
        TokenKind::Amp => 5,
        TokenKind::EqEq | TokenKind::NotEq => 6,
        TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge => 7,
        TokenKind::Shl | TokenKind::Shr => 8,
        TokenKind::Plus | TokenKind::Minus | TokenKind::WrapPlus | TokenKind::WrapMinus => 9,
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent | TokenKind::WrapStar => 10,
        TokenKind::As | TokenKind::Is => 11,
        _ => return None,
    };
    Some(prec)
}

fn bin_op(kind: TokenKind) -> Option<BinOp> {
    let op = match kind {
        TokenKind::OrOr => BinOp::Or,
        TokenKind::AndAnd => BinOp::And,
        TokenKind::Pipe => BinOp::BitOr,
        TokenKind::Caret => BinOp::BitXor,
        TokenKind::Amp => BinOp::BitAnd,
        TokenKind::EqEq => BinOp::Eq,
        TokenKind::NotEq => BinOp::Ne,
        TokenKind::Lt => BinOp::Lt,
        TokenKind::Le => BinOp::Le,
        TokenKind::Gt => BinOp::Gt,
        TokenKind::Ge => BinOp::Ge,
        TokenKind::Shl => BinOp::Shl,
        TokenKind::Shr => BinOp::Shr,
        TokenKind::Plus => BinOp::Add,
        TokenKind::Minus => BinOp::Sub,
        TokenKind::WrapPlus => BinOp::WrapAdd,
        TokenKind::WrapMinus => BinOp::WrapSub,
        TokenKind::Star => BinOp::Mul,
        TokenKind::Slash => BinOp::Div,
        TokenKind::Percent => BinOp::Rem,
        TokenKind::WrapStar => BinOp::WrapMul,
        _ => return None,
    };
    Some(op)
}
