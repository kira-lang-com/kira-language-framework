//! The reason a runtime trap names, beside its code.
//!
//! Each `KLF-RT-` code is defined by a section of `roadmap/KLF-RIDL.md`; the
//! reason here says what the section makes the code mean, so a trap reads as
//! a named failure rather than a bare number.

/// What `code` means, or `None` for a code this table does not name.
#[must_use]
pub fn reason(code: &str) -> Option<&'static str> {
    Some(match code {
        "KLF-RT-0001" => "reached an `unreachable` point in the program",
        "KLF-RT-0002" => "integer overflow in a checked operation",
        "KLF-RT-0003" => "conflicting accesses to the same storage",
        "KLF-RT-0005" => "out-of-bounds memory access",
        "KLF-RT-0006" => "freed a heap block that was already freed",
        "KLF-RT-0010" => "division or remainder by zero",
        "KLF-RT-0011" => "float to integer conversion out of range, or NaN",
        "KLF-RT-0020" => "unwinding reached a boundary that does not carry it",
        "KLF-RT-0021" => "call depth limit exceeded: the recursion is too deep",
        "KLF-RT-0100" => "heap balance check failed: a heap block was not freed",
        _ => return None,
    })
}

/// The trap line for `message`: the code and its reason when both are known.
#[must_use]
pub fn line(message: &str) -> String {
    let (code, place) = match message.split_once(' ') {
        Some((code, place)) => (code, Some(place)),
        None => (message, None),
    };
    match (reason(code), place) {
        (Some(reason), Some(place)) => format!("trap: {code}: {reason} ({place})"),
        (Some(reason), None) => format!("trap: {code}: {reason}"),
        (None, _) => format!("trap: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_code_prints_its_reason() {
        assert_eq!(
            line("KLF-RT-0005"),
            "trap: KLF-RT-0005: out-of-bounds memory access"
        );
    }

    #[test]
    fn a_code_with_its_place_prints_both() {
        assert_eq!(
            line("KLF-RT-0001 at app/main.kira:3:10 in `main`"),
            "trap: KLF-RT-0001: reached an `unreachable` point in the program (at app/main.kira:3:10 in `main`)"
        );
    }

    #[test]
    fn an_unknown_message_prints_as_it_is() {
        assert_eq!(line("KLF-RT-0777"), "trap: KLF-RT-0777");
        assert_eq!(line("something else"), "trap: something else");
    }
}
