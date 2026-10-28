//! The source name of a function a module keeps to itself (`kir.<name>`).
//!
//! The name is a path of `.`-separated parts, and each part an identifier
//! with suffixes: `#Module` for a name two modules declare, `#role` for a
//! function the build made for a declaration (`init`, `dispatch`), and
//! `#<number>` for one generated for a type, which the module's
//! `klf.debug.name` attribute spells out. An overloaded name carries its
//! parameter types as `name$Number$String`. A trap's backtrace, a crash
//! report and the debug information the native backend writes all show the
//! name as Kira spells it: `KiraUI.Picker<String>.describe`,
//! `Foundation.__globals (init)`, `max(Number, Number)`.

/// `local` as Kira spells it.
#[must_use]
pub fn local_source_name(local: &str) -> String {
    let mut roles: Vec<&str> = Vec::new();
    let parts: Vec<String> = split_path(local)
        .into_iter()
        .map(|part| shown_part(part, &mut roles))
        .collect();
    let mut shown = parts.join(".");
    for role in roles {
        shown.push_str(" (");
        shown.push_str(role);
        shown.push(')');
    }
    shown
}

/// Text that names types as the compiler keeps them, such as `destroy of
/// Array<Selection#Shop>`, with each `Name#Module` written `Module.Name` and
/// each `name#role` as the name alone: `destroy of Array<Shop.Selection>`.
#[must_use]
pub fn shown_type_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(hash) = rest.find('#') {
        out.push_str(&rest[..hash]);
        let after = &rest[hash + 1..];
        let length = after
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(after.len());
        let suffix = &after[..length];
        if suffix.starts_with(|character: char| character.is_ascii_uppercase()) {
            let word = out
                .rfind(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .map_or(0, |index| index + 1);
            out.insert_str(word, &format!("{suffix}."));
        }
        rest = &after[length..];
    }
    out.push_str(rest);
    out
}

/// The `.`-separated parts of a name, a `.` inside `<...>` or after a `$`
/// parameter list staying in its part.
fn split_path(name: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    let mut overloaded = false;
    for (index, character) in name.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            '$' => overloaded = true,
            '.' if depth == 0 && !overloaded => {
                parts.push(&name[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&name[start..]);
    parts
}

/// One part: its identifier with the module it is declared in before it,
/// its roles set aside for the end, and an overload's parameters after it.
fn shown_part<'a>(part: &'a str, roles: &mut Vec<&'a str>) -> String {
    let (head, parameters) = match part.split_once('$') {
        Some((head, parameters)) => (head, Some(parameters)),
        None => (part, None),
    };
    let mut pieces = head.split('#');
    let mut name = pieces.next().unwrap_or_default().to_owned();
    let mut module: Option<&str> = None;
    for piece in pieces {
        // A suffix is a word; what follows it, such as a generic
        // instance's `<String>`, is the name's.
        let length = piece
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(piece.len());
        let (suffix, rest) = piece.split_at(length);
        if suffix.starts_with(|character: char| character.is_ascii_uppercase()) {
            module = Some(suffix);
        } else if suffix.starts_with(|character: char| character.is_ascii_lowercase()) {
            roles.push(suffix);
        } else if !suffix.is_empty() {
            name.push('#');
            name.push_str(suffix);
        }
        name.push_str(rest);
    }
    let mut shown = match module {
        Some(module) => format!("{module}.{name}"),
        None => name,
    };
    if let Some(parameters) = parameters {
        shown.push_str(&overload(parameters));
    }
    shown
}

/// `Number$String$result$Bool` as `(Number, String) -> Bool`.
fn overload(parameters: &str) -> String {
    let (parameters, result) = match parameters.split_once("$result$") {
        Some((parameters, result)) => (parameters, Some(result)),
        None => (parameters, None),
    };
    let listed: Vec<&str> = parameters.split('$').filter(|parameter| !parameter.is_empty()).collect();
    let mut shown = format!("({})", listed.join(", "));
    if let Some(result) = result {
        shown.push_str(" -> ");
        shown.push_str(result);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::local_source_name;

    #[test]
    fn local_names_read_as_their_source_names() {
        for (local, shown) in [
            ("pick#TrapGallery", "TrapGallery.pick"),
            ("Picker#KiraUI<String>.describe", "KiraUI.Picker<String>.describe"),
            ("ExecutorTask.cancel#dispatch", "ExecutorTask.cancel (dispatch)"),
            ("__globals#init#Foundation", "Foundation.__globals (init)"),
            ("max$Number$Foundation.Number", "max(Number, Foundation.Number)"),
            ("cString$String$result$RawPtr", "cString(String) -> RawPtr"),
            ("Map<Int,Int>.value", "Map<Int,Int>.value"),
            ("divide#", "divide"),
        ] {
            assert_eq!(local_source_name(local), shown, "{local}");
        }
        assert_eq!(
            super::shown_type_text("destroy of Array<Selection#Shop>"),
            "destroy of Array<Shop.Selection>"
        );
    }
}
