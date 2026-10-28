//! The KLF Kira lexer: source text to a token stream (Appendix A.1).

use crate::token::{Token, TokenKind, reserved_word};
use klf_diag::Diagnostic;
use klf_source::{SourceFile, Span};

/// The result of lexing one source file.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lexes a whole source file into a token stream ending in [`TokenKind::Eof`].
///
/// Newline runs become a single [`TokenKind::Newline`]; leading and trailing
/// runs are dropped. Comments (`//`, `///`, nested `/* */`) are trivia.
#[must_use]
pub fn lex(file: &SourceFile) -> Lexed {
    Lexer::new(file).run()
}

struct Lexer<'a> {
    file: &'a SourceFile,
    text: &'a str,
    offset: u32,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(file: &'a SourceFile) -> Self {
        Self {
            file,
            text: file.text(),
            offset: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> Lexed {
        loop {
            let saw_newline = self.skip_trivia();
            if self.rest().is_empty() {
                break;
            }
            let separates = saw_newline && self.wants_newline();
            let at = self.offset;
            let next = self.tokens.len();
            self.lex_token();
            if separates
                && !self
                    .tokens
                    .get(next)
                    .is_some_and(|token| continues_line(token.kind))
            {
                let span = self.span(at, at);
                self.tokens
                    .insert(next, Token::new(TokenKind::Newline, span));
            }
        }
        let end = self.offset;
        self.push(TokenKind::Eof, end, end);
        Lexed {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn wants_newline(&self) -> bool {
        !matches!(
            self.tokens.last().map(|t| t.kind),
            None | Some(TokenKind::Newline)
        )
    }

    fn rest(&self) -> &'a str {
        &self.text[self.offset as usize..]
    }

    fn first(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn second(&self) -> Option<char> {
        let mut chars = self.rest().chars();
        chars.next();
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.first()?;
        self.offset += u32::try_from(ch.len_utf8()).expect("utf-8 char length is at most 4");
        Some(ch)
    }

    fn span(&self, start: u32, end: u32) -> Span {
        Span::new(self.file.id(), start, end)
    }

    fn push(&mut self, kind: TokenKind, start: u32, end: u32) {
        self.tokens.push(Token::new(kind, self.span(start, end)));
    }

    /// Skips whitespace and comments, returning whether a newline was crossed.
    fn skip_trivia(&mut self) -> bool {
        let mut saw_newline = false;
        loop {
            match self.first() {
                Some('\n') => {
                    saw_newline = true;
                    self.bump();
                }
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if matches!(self.second(), Some('/')) => {
                    self.skip_line_comment();
                }
                Some('/') if matches!(self.second(), Some('*')) => {
                    self.skip_block_comment();
                }
                _ => return saw_newline,
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.first() {
            if c == '\n' {
                return;
            }
            self.bump();
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.offset;
        self.bump();
        self.bump();
        let mut depth = 1u32;
        while depth > 0 {
            match self.first() {
                None => {
                    self.diagnostics.push(
                        Diagnostic::error("KLFK-E0100", "unterminated block comment")
                            .with_label(self.span(start, self.offset), "comment starts here"),
                    );
                    return;
                }
                Some('/') if matches!(self.second(), Some('*')) => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                Some('*') if matches!(self.second(), Some('/')) => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn lex_token(&mut self) {
        let start = self.offset;
        let ch = self.first().expect("lex_token called at end of input");
        if is_ident_start(ch) {
            self.lex_ident(start);
        } else if ch.is_ascii_digit() {
            self.lex_number(start);
        } else if ch == '"' {
            self.lex_string(start);
        } else {
            self.lex_operator(start);
        }
    }

    /// A `"""` literal runs to the next unescaped `"""`, across lines.
    fn lex_block_string(&mut self, start: u32) {
        for _ in 0..3 {
            self.bump();
        }
        loop {
            if self.text[self.offset as usize..].starts_with("\"\"\"") {
                for _ in 0..3 {
                    self.bump();
                }
                self.push(TokenKind::Str, start, self.offset);
                return;
            }
            match self.first() {
                None => {
                    self.diagnostics.push(
                        Diagnostic::error("KLFK-E0101", "unterminated string literal")
                            .with_label(self.span(start, self.offset), "string starts here"),
                    );
                    self.push(TokenKind::Str, start, self.offset);
                    return;
                }
                Some('\\') => {
                    self.bump();
                    self.bump();
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    fn lex_ident(&mut self, start: u32) {
        while self.first().is_some_and(is_ident_continue) {
            self.bump();
        }
        let lexeme = &self.text[start as usize..self.offset as usize];
        let kind = reserved_word(lexeme).unwrap_or(TokenKind::Ident);
        self.push(kind, start, self.offset);
    }

    fn lex_number(&mut self, start: u32) {
        if self.first() == Some('0')
            && matches!(self.second(), Some('x' | 'X' | 'b' | 'B' | 'o' | 'O'))
        {
            self.bump();
            self.bump();
            while self
                .first()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                self.bump();
            }
            self.push(TokenKind::Int, start, self.offset);
            return;
        }
        self.eat_digits();
        let mut is_float = false;
        if self.first() == Some('.') && self.second().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.bump();
            self.eat_digits();
        }
        if matches!(self.first(), Some('e' | 'E')) {
            is_float = true;
            self.bump();
            if matches!(self.first(), Some('+' | '-')) {
                self.bump();
            }
            self.eat_digits();
        }
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };
        self.push(kind, start, self.offset);
    }

    fn eat_digits(&mut self) {
        while self.first().is_some_and(|c| c.is_ascii_digit() || c == '_') {
            self.bump();
        }
    }

    fn lex_string(&mut self, start: u32) {
        if self.text[self.offset as usize..].starts_with("\"\"\"") {
            self.lex_block_string(start);
            return;
        }
        self.bump();
        loop {
            match self.first() {
                None | Some('\n') => {
                    self.diagnostics.push(
                        Diagnostic::error("KLFK-E0101", "unterminated string literal")
                            .with_label(self.span(start, self.offset), "string starts here"),
                    );
                    self.push(TokenKind::Str, start, self.offset);
                    return;
                }
                Some('\\') => {
                    self.bump();
                    self.bump();
                }
                Some('"') => {
                    self.bump();
                    self.push(TokenKind::Str, start, self.offset);
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn lex_operator(&mut self, start: u32) {
        let Some(kind) = self.operator_kind() else {
            let ch = self.bump().expect("operator start present");
            self.diagnostics.push(
                Diagnostic::error("KLFK-E0102", format!("unexpected character `{ch}`"))
                    .with_label(self.span(start, self.offset), "here"),
            );
            return;
        };
        self.push(kind, start, self.offset);
    }

    /// Consumes and classifies the operator or delimiter at the cursor,
    /// longest match first.
    fn operator_kind(&mut self) -> Option<TokenKind> {
        let mut chars = self.rest().chars();
        let a = chars.next()?;
        let b = chars.next();
        let c = chars.next();
        if let (Some(kind), Some('=')) = (shift_op(a, b), c) {
            self.bump();
            self.bump();
            self.bump();
            return Some(kind);
        }
        if let Some(kind) = two_char(a, b) {
            self.bump();
            self.bump();
            return Some(kind);
        }
        let kind = one_char(a)?;
        self.bump();
        Some(kind)
    }
}

fn shift_op(a: char, b: Option<char>) -> Option<TokenKind> {
    match (a, b) {
        ('<', Some('<')) => Some(TokenKind::ShlEq),
        ('>', Some('>')) => Some(TokenKind::ShrEq),
        _ => None,
    }
}

fn two_char(a: char, b: Option<char>) -> Option<TokenKind> {
    let b = b?;
    let kind = match (a, b) {
        ('-', '>') => TokenKind::Arrow,
        ('&', '&') => TokenKind::AndAnd,
        ('|', '|') => TokenKind::OrOr,
        ('=', '=') => TokenKind::EqEq,
        ('!', '=') => TokenKind::NotEq,
        ('<', '=') => TokenKind::Le,
        ('>', '=') => TokenKind::Ge,
        ('<', '<') => TokenKind::Shl,
        ('>', '>') => TokenKind::Shr,
        ('+', '=') => TokenKind::PlusEq,
        ('-', '=') => TokenKind::MinusEq,
        ('*', '=') => TokenKind::StarEq,
        ('/', '=') => TokenKind::SlashEq,
        ('%', '=') => TokenKind::PercentEq,
        ('&', '=') => TokenKind::AmpEq,
        ('|', '=') => TokenKind::PipeEq,
        ('^', '=') => TokenKind::CaretEq,
        ('&', '+') => TokenKind::WrapPlus,
        ('&', '-') => TokenKind::WrapMinus,
        ('&', '*') => TokenKind::WrapStar,
        ('.', '.') => TokenKind::DotDot,
        _ => return None,
    };
    Some(kind)
}

fn one_char(a: char) -> Option<TokenKind> {
    let kind = match a {
        '(' => TokenKind::LParen,
        ')' => TokenKind::RParen,
        '{' => TokenKind::LBrace,
        '}' => TokenKind::RBrace,
        '[' => TokenKind::LBracket,
        ']' => TokenKind::RBracket,
        ',' => TokenKind::Comma,
        '.' => TokenKind::Dot,
        ':' => TokenKind::Colon,
        '@' => TokenKind::At,
        '?' => TokenKind::Question,
        ';' => TokenKind::Semicolon,
        '=' => TokenKind::Eq,
        '+' => TokenKind::Plus,
        '-' => TokenKind::Minus,
        '*' => TokenKind::Star,
        '/' => TokenKind::Slash,
        '%' => TokenKind::Percent,
        '&' => TokenKind::Amp,
        '|' => TokenKind::Pipe,
        '^' => TokenKind::Caret,
        '~' => TokenKind::Tilde,
        '!' => TokenKind::Bang,
        '<' => TokenKind::Lt,
        '>' => TokenKind::Gt,
        _ => return None,
    };
    Some(kind)
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// A line that begins with a binary operator no expression can start with
/// continues the line before it, so a long condition can break before its
/// `||` or `&&`. `-` is not among them: a line may begin with a negation.
fn continues_line(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::OrOr
            | TokenKind::AndAnd
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Amp
            | TokenKind::EqEq
            | TokenKind::NotEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Plus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::WrapPlus
            | TokenKind::WrapMinus
            | TokenKind::WrapStar
            | TokenKind::Question
            | TokenKind::Colon
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_core::FileId;

    fn kinds(source: &str) -> Vec<TokenKind> {
        let file = SourceFile::new(FileId::from_u32(0), "t.klfd", source);
        lex(&file).tokens.into_iter().map(|t| t.kind).collect()
    }

    fn lexemes(source: &str) -> Vec<(TokenKind, String)> {
        let file = SourceFile::new(FileId::from_u32(0), "t.klfd", source);
        lex(&file)
            .tokens
            .into_iter()
            .map(|t| {
                let text = file.text()[t.span.start as usize..t.span.end as usize].to_string();
                (t.kind, text)
            })
            .collect()
    }

    #[test]
    fn reserved_words_and_contextual_idents() {
        use TokenKind::{Eof, Function, Ident};
        assert_eq!(kinds("function"), vec![Function, Eof]);
        // `move`, `borrow`, `self`, `Any` are contextual, not reserved.
        assert_eq!(
            kinds("move borrow self Any"),
            vec![Ident, Ident, Ident, Ident, Eof]
        );
    }

    #[test]
    fn operators_longest_match() {
        use TokenKind::{AndAnd, Arrow, Eof, Eq, EqEq, Le, Lt, ShlEq, Shr, WrapPlus};
        assert_eq!(
            kinds("-> && == = <= < >> <<= &+"),
            vec![Arrow, AndAnd, EqEq, Eq, Le, Lt, Shr, ShlEq, WrapPlus, Eof,]
        );
    }

    #[test]
    fn numbers_int_and_float() {
        use TokenKind::{Eof, Float, Int};
        assert_eq!(
            kinds("42 1_000 0xff 3.14 1e9 2.0e-3"),
            vec![Int, Int, Int, Float, Float, Float, Eof,]
        );
        // A trailing dot is member access, not a float.
        assert_eq!(
            kinds("1.foo"),
            vec![Int, TokenKind::Dot, TokenKind::Ident, Eof]
        );
        assert_eq!(
            kinds("0..count"),
            vec![Int, TokenKind::DotDot, TokenKind::Ident, Eof]
        );
    }

    #[test]
    fn strings_and_escapes() {
        let out = lexemes(r#""hi" "a\"b""#);
        assert_eq!(out[0], (TokenKind::Str, r#""hi""#.to_string()));
        assert_eq!(out[1], (TokenKind::Str, r#""a\"b""#.to_string()));
    }

    #[test]
    fn comments_are_trivia_and_nest() {
        use TokenKind::{Eof, Ident};
        assert_eq!(
            kinds("a // line\n/* /* nested */ */ b"),
            vec![Ident, TokenKind::Newline, Ident, Eof,]
        );
    }

    #[test]
    fn unterminated_string_reports_e0101() {
        let file = SourceFile::new(FileId::from_u32(0), "t.klfd", "\"oops");
        let out = lex(&file);
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, "KLFK-E0101");
    }

    #[test]
    fn unterminated_block_comment_reports_e0100() {
        let file = SourceFile::new(FileId::from_u32(0), "t.klfd", "/* open");
        let out = lex(&file);
        assert_eq!(out.diagnostics[0].code, "KLFK-E0100");
    }

    #[test]
    fn unexpected_character_reports_e0102() {
        let file = SourceFile::new(FileId::from_u32(0), "t.klfd", "a $ b");
        let out = lex(&file);
        assert_eq!(out.diagnostics[0].code, "KLFK-E0102");
    }
}
