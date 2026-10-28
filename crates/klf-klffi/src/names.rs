//! C names that cannot be written in Kira, and the ones a header leaves out.
//!
//! C and Kira share an identifier grammar but not a keyword set, so a C
//! function called `type` or a field called `function` has no Kira spelling.
//! A generator that renamed a declared name would produce a binding whose name
//! is not the C symbol, which is the one thing every call site depends on, so a
//! declaration named for a keyword is refused with the reason. A field or a
//! parameter binds nothing outside its own declaration, so it is written under
//! a renamed spelling that keeps the C name visible.
//!
//! The keyword set comes from the Kira lexer, not from a list copied here: a
//! keyword added to the language must not start generating bindings that do
//! not parse.

use klf_kira_syntax::reserved_word;

/// Why `name` cannot be written as a declared Kira name, or `None` when it can.
pub(crate) fn unbindable_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("it has no name to declare".to_owned());
    }
    if reserved_word(name).is_some() {
        return Some(format!(
            "`{name}` is a Kira keyword, so no declaration can carry the C name"
        ));
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return Some(format!("`{name}` does not start with a letter or `_`"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii())
    {
        return Some(format!("`{name}` is not spellable as a Kira identifier"));
    }
    None
}

/// The name a parameter is written under, given what C called it.
///
/// C prototypes may name no parameters, and a parameter named for a Kira keyword
/// cannot keep that name. Neither refuses the function: a parameter name is
/// local to its declaration.
pub(crate) fn parameter_name(written: &str, index: usize) -> String {
    if written.is_empty() {
        return format!("arg{index}");
    }
    local_name(written)
}

/// The name a struct field is written under, given what C called it and the
/// names the struct already has. A keyword-named field is renamed rather than
/// refusing the whole struct, and a renamed field never collides with one the
/// struct already has.
pub(crate) fn field_name(written: &str, taken: &[String]) -> Option<String> {
    if written.is_empty() {
        return None;
    }
    let mut candidate = local_name(written);
    while taken.iter().any(|already| already == &candidate) {
        candidate.push('_');
    }
    Some(candidate)
}

/// A local name that is spellable in Kira, renamed only when it has to be.
///
/// The `_value` suffix is the spelling the generated dialect already uses for
/// this, so a program written against an earlier binding still compiles.
fn local_name(written: &str) -> String {
    match unbindable_name(written) {
        None => written.to_owned(),
        Some(_) => format!("{written}_value"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyword_is_refused_as_a_declared_name_and_renamed_as_a_local_one() {
        assert!(unbindable_name("type").is_some());
        assert!(unbindable_name("function").is_some());
        assert!(unbindable_name("match").is_some());
        assert!(unbindable_name("font_path").is_none());
        assert_eq!(parameter_name("type", 2), "type_value");
        assert_eq!(parameter_name("", 0), "arg0");
        assert_eq!(parameter_name("font_path", 0), "font_path");
    }

    #[test]
    fn a_renamed_field_never_collides_with_one_the_struct_already_has() {
        let taken = vec!["type_value".to_owned()];
        assert_eq!(field_name("type", &taken).as_deref(), Some("type_value_"));
        assert_eq!(field_name("width", &taken).as_deref(), Some("width"));
        assert_eq!(field_name("function", &[]).as_deref(), Some("function_value"));
        assert_eq!(field_name("", &taken), None);
    }

    #[test]
    fn an_ordinary_c_name_is_spellable() {
        assert_eq!(unbindable_name("kira_text_draw_run"), None);
        assert_eq!(unbindable_name("_opaque_pthread_t"), None);
    }
}
