//! KLF Kira tokens (Appendix A.1).

use klf_source::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Ident,

    // Reserved words (A.1).
    Function,
    Let,
    Var,
    Return,
    If,
    Else,
    While,
    True,
    False,
    Import,
    As,
    Is,
    Struct,
    Enum,
    Construct,
    Trait,
    Extends,
    Match,
    Attempt,
    Try,
    For,
    In,
    Break,
    Continue,
    Type,
    Distinct,
    Extend,

    // Literals.
    Int,
    Float,
    Str,

    // Delimiters.
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    // Punctuation.
    Comma,
    Dot,
    DotDot,
    Colon,
    At,
    Arrow,
    Question,
    Semicolon,

    // Assignment operators.
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    // Binary and unary operators.
    OrOr,
    AndAnd,
    Pipe,
    Caret,
    Amp,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    Plus,
    Minus,
    WrapPlus,
    WrapMinus,
    Star,
    Slash,
    Percent,
    WrapStar,
    Bang,
    Tilde,

    /// A run of one or more newlines, a soft statement separator.
    Newline,
    /// End of input.
    Eof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    #[must_use]
    pub const fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// Maps an identifier lexeme to its reserved-word token, or `None` when the
/// lexeme is an ordinary identifier. Contextual words (`borrow`, `mut`, `move`,
/// `copy`, `handle`, `requires`, `init`, `self`, `Any`, `some`) are not
/// reserved and stay `Ident`.
#[must_use]
pub fn reserved_word(lexeme: &str) -> Option<TokenKind> {
    let kind = match lexeme {
        "function" => TokenKind::Function,
        "let" => TokenKind::Let,
        "var" => TokenKind::Var,
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "import" => TokenKind::Import,
        "as" => TokenKind::As,
        "is" => TokenKind::Is,
        "struct" => TokenKind::Struct,
        "enum" => TokenKind::Enum,
        "construct" => TokenKind::Construct,
        "trait" => TokenKind::Trait,
        "extends" => TokenKind::Extends,
        "match" => TokenKind::Match,
        "attempt" => TokenKind::Attempt,
        "try" => TokenKind::Try,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "type" => TokenKind::Type,
        "distinct" => TokenKind::Distinct,
        "extend" => TokenKind::Extend,
        _ => return None,
    };
    Some(kind)
}
