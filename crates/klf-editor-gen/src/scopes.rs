//! What each token class is called in `TextMate` scopes and tree-sitter
//! captures (`KLF-EDITOR.md` section 12.2).

use klf_facts::spec::KeywordGroup;
use klf_facts::{TokenClass, TokenModifier};

/// The `TextMate` scope of a class, before the language suffix.
pub(crate) fn textmate(class: TokenClass, control: bool) -> &'static str {
    match class {
        TokenClass::Comment => "comment.block",
        TokenClass::DocComment => "comment.block.documentation",
        TokenClass::String => "string.quoted.double",
        TokenClass::Character => "string.quoted.single",
        TokenClass::Escape => "constant.character.escape",
        TokenClass::Number => "constant.numeric",
        TokenClass::Boolean => "constant.language.boolean",
        TokenClass::Keyword if control => "keyword.control",
        TokenClass::Keyword => "keyword.other",
        TokenClass::Operator => "keyword.operator",
        TokenClass::Punctuation => "punctuation",
        TokenClass::Namespace => "entity.name.namespace",
        TokenClass::Type => "entity.name.type",
        TokenClass::Struct => "entity.name.type.struct",
        TokenClass::Class => "entity.name.type.class",
        TokenClass::Enum => "entity.name.type.enum",
        TokenClass::Interface => "entity.name.type.interface",
        TokenClass::TypeParameter => "entity.name.type.parameter",
        TokenClass::Function => "entity.name.function",
        TokenClass::Method => "entity.name.function.member",
        TokenClass::Macro => "entity.name.function.macro",
        TokenClass::Variable => "variable.other",
        TokenClass::Parameter => "variable.parameter",
        TokenClass::Property => "variable.other.member",
        TokenClass::EnumMember => "variable.other.enummember",
        TokenClass::Constant => "variable.other.constant",
        TokenClass::Label => "entity.name.label",
        TokenClass::Annotation => "storage.type.annotation",
        TokenClass::Builtin => "support.type",
        TokenClass::Error => "invalid.illegal",
    }
}

/// The tree-sitter capture of a class.
pub(crate) fn capture(class: TokenClass, control: bool) -> &'static str {
    match class {
        TokenClass::Comment => "@comment",
        TokenClass::DocComment => "@comment.documentation",
        TokenClass::String => "@string",
        TokenClass::Character => "@character",
        TokenClass::Escape => "@string.escape",
        TokenClass::Number => "@number",
        TokenClass::Boolean => "@boolean",
        TokenClass::Keyword if control => "@keyword.control",
        TokenClass::Keyword => "@keyword",
        TokenClass::Operator => "@operator",
        TokenClass::Punctuation => "@punctuation.delimiter",
        TokenClass::Namespace => "@module",
        TokenClass::Type
        | TokenClass::Struct
        | TokenClass::Class
        | TokenClass::Enum
        | TokenClass::Interface => "@type",
        TokenClass::TypeParameter => "@type.parameter",
        TokenClass::Function => "@function",
        TokenClass::Method => "@function.method",
        TokenClass::Macro => "@function.macro",
        TokenClass::Variable => "@variable",
        TokenClass::Parameter => "@variable.parameter",
        TokenClass::Property => "@property",
        TokenClass::EnumMember | TokenClass::Constant => "@constant",
        TokenClass::Label => "@label",
        TokenClass::Annotation => "@attribute",
        TokenClass::Builtin => "@type.builtin",
        TokenClass::Error => "@error",
    }
}

pub(crate) fn is_control(group: &KeywordGroup) -> bool {
    group.modifiers.contains(&TokenModifier::Control)
}

/// Whether every word of the group is a name, so word boundaries apply.
pub(crate) fn is_wordlike(group: &KeywordGroup) -> bool {
    group.words.iter().all(|word| {
        word.chars()
            .all(|character| character.is_alphanumeric() || character == '_')
    })
}
