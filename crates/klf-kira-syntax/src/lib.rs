//! The KLF Kira frontend syntax layer: lexer, tokens and (later) the parser
//! and AST for the definition language of Appendix A.1.

pub mod ast;
mod lexer;
mod parser;
mod token;

pub use lexer::{Lexed, lex};
pub use parser::{Parsed, parse};
pub use token::{Token, TokenKind, reserved_word};
