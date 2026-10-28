//! The record format the spec and fact codecs share: one record per line,
//! fields separated by tabs, with `\\`, tab, newline and carriage return
//! escaped. An optional field is empty when absent and starts with `+` when
//! present.

#[derive(Default)]
pub struct Records {
    pub text: String,
}

impl Records {
    pub fn line(&mut self, fields: &[&str]) {
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                self.text.push('\t');
            }
            for character in field.chars() {
                match character {
                    '\\' => self.text.push_str("\\\\"),
                    '\t' => self.text.push_str("\\t"),
                    '\n' => self.text.push_str("\\n"),
                    '\r' => self.text.push_str("\\r"),
                    other => self.text.push(other),
                }
            }
        }
        self.text.push('\n');
    }
}

#[must_use]
pub fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[must_use]
pub fn optional(value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| format!("+{value}"))
}

/// `None` for a malformed field, `Some(None)` for an absent value.
#[allow(clippy::option_option)]
#[must_use]
pub fn present(field: &str) -> Option<Option<String>> {
    if field.is_empty() {
        return Some(None);
    }
    field.strip_prefix('+').map(|value| Some(value.to_owned()))
}

#[must_use]
pub const fn flag(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

#[must_use]
pub fn boolean(field: &str) -> Option<bool> {
    match field {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
