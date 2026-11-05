use crate::BasicBlock;
use crate::text::TokenKind;

pub(crate) fn simple_block_header(block: &BasicBlock) -> bool {
    let tokens = &block.header_tokens;
    if !matches!(
        tokens.last().map(|token| &token.kind),
        Some(TokenKind::Punctuation(':'))
    ) {
        return false;
    }
    if tokens.len() == 2 {
        return true;
    }
    if !matches!(
        tokens.get(1).map(|token| &token.kind),
        Some(TokenKind::Punctuation('('))
    ) {
        return false;
    }
    let mut depth = 0_u32;
    for (index, token) in tokens.iter().enumerate().skip(1) {
        match &token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => {
                depth = match depth.checked_sub(1) {
                    Some(depth) => depth,
                    None => return false,
                };
                if depth == 0 {
                    return index + 2 == tokens.len();
                }
            }
            _ => {}
        }
    }
    false
}
