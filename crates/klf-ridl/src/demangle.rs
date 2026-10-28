//! Reading a RIDL symbol back as the names it was made from (section 24.2),
//! so a trap, a backtrace or a debugger shows `Geometry.parse` rather than
//! `_RIDL18Geometry5parseF`.

/// What a RIDL symbol names, as source spells it: `Module.function`,
/// `Module.Type.method`, `Module.name<Int, [String]>`, or the descriptor,
/// builder, size or witness table of a type. `None` when `symbol` is not a
/// RIDL symbol; a leading `_`, as Mach-O adds, is allowed.
#[must_use]
pub fn demangle(symbol: &str) -> Option<String> {
    let body = symbol.strip_prefix('_').unwrap_or(symbol);
    let body = body.strip_prefix("RIDL1")?;
    let mut reader = Reader { text: body, at: 0 };
    let path = reader.path()?;
    let kind = reader.next()?;
    let joined = path.join(".");
    let named = match kind {
        'F' | 'M' => joined,
        'D' => format!("descriptor of {joined}"),
        'B' => format!("builder of {joined}"),
        'Z' => format!("size of {joined}"),
        'W' => format!("witness table of {joined} for {}", reader.path()?.join(".")),
        'I' => {
            let mut arguments = Vec::new();
            while reader.peek()? != 'E' {
                arguments.push(reader.type_name()?);
            }
            reader.next()?;
            format!("{joined}<{}>", arguments.join(", "))
        }
        _ => return None,
    };
    let rest = &reader.text[reader.at..];
    let tagged = rest
        .strip_prefix("_v")
        .is_some_and(|tag| tag.len() == 16 && tag.bytes().all(|digit| digit.is_ascii_hexdigit()));
    (rest.is_empty() || tagged).then_some(named)
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn next(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.at += next.len_utf8();
        Some(next)
    }

    /// One `decimal-length identifier` component.
    fn component(&mut self) -> Option<&str> {
        let digits = self.text[self.at..].bytes().take_while(u8::is_ascii_digit).count();
        let length: usize = self.text.get(self.at..self.at + digits)?.parse().ok()?;
        let start = self.at + digits;
        let name = self.text.get(start..start.checked_add(length)?)?;
        self.at = start + length;
        Some(name)
    }

    fn path(&mut self) -> Option<Vec<String>> {
        let mut components = Vec::new();
        while self.peek()?.is_ascii_digit() {
            components.push(self.component()?.to_owned());
        }
        (!components.is_empty()).then_some(components)
    }

    /// One type code, spelled as Kira writes the type.
    fn type_name(&mut self) -> Option<String> {
        let scalar = match self.next()? {
            'a' => "I8",
            'b' => "I16",
            'c' => "I32",
            'd' => "Int",
            'e' => "I128",
            'h' => "U8",
            'i' => "U16",
            'j' => "U32",
            'k' => "U64",
            'l' => "U128",
            'f' => "F32",
            'g' => "Float",
            'o' => "Bool",
            'v' => "ISize",
            'w' => "USize",
            'p' => "RawPtr",
            's' => "String",
            'V' => "Void",
            'A' => return Some(format!("[{}]", self.type_name()?)),
            'O' => return Some(format!("{}?", self.type_name()?)),
            'R' => {
                let value = self.type_name()?;
                return Some(format!("Result<{value}, {}>", self.type_name()?));
            }
            'T' => {
                let mut items = Vec::new();
                while self.peek()? != 'E' {
                    items.push(self.type_name()?);
                }
                self.next()?;
                return Some(format!("({})", items.join(", ")));
            }
            'N' => return self.nominal(),
            _ => return None,
        };
        Some(scalar.to_owned())
    }

    /// `N component [I codes E] E`.
    fn nominal(&mut self) -> Option<String> {
        let mut name = self.component()?.to_owned();
        if self.peek()? == 'I' {
            self.next()?;
            let mut arguments = Vec::new();
            while self.peek()? != 'E' {
                arguments.push(self.type_name()?);
            }
            self.next()?;
            name = format!("{name}<{}>", arguments.join(", "));
        }
        (self.next()? == 'E').then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::demangle;
    use crate::mangle;
    use crate::types::{Scalar, Type};

    #[test]
    fn every_kind_reads_back_as_its_source_names() {
        let cases = [
            (mangle::function_symbol("Geometry", "parse", Some("6f1c2a9e44d07b31")), "Geometry.parse"),
            (mangle::method_symbol("Harmony", "ModelPicker", "choose", None), "Harmony.ModelPicker.choose"),
            (mangle::descriptor_symbol("Core", "FavoritePreset"), "descriptor of Core.FavoritePreset"),
            (
                mangle::witness_symbol("Core", "AvailableModel", "Foundation", "Codable"),
                "witness table of Core.AvailableModel for Foundation.Codable",
            ),
            (
                mangle::instance_symbol(
                    "Foundation",
                    "Array",
                    &[Type::Nominal { name: "ModelSelection".to_owned(), args: Vec::new() }],
                    None,
                ),
                "Foundation.Array<ModelSelection>",
            ),
            (
                mangle::instance_symbol(
                    "Foundation",
                    "Map",
                    &[Type::String, Type::Array(Box::new(Type::Scalar(Scalar::I64)))],
                    None,
                ),
                "Foundation.Map<String, [Int]>",
            ),
        ];
        for (symbol, shown) in cases {
            assert_eq!(demangle(&format!("_{symbol}")).as_deref(), Some(shown), "{symbol}");
        }
        assert_eq!(demangle("kir.main"), None);
    }
}
