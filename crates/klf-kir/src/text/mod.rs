//! The canonical textual form of a KIR module (section 10.4, appendix A.4).

mod control;
mod emit;
pub(crate) mod function;
mod items;
pub(crate) mod operation;
pub(crate) mod spelling;

use crate::{Module, Stage};
use std::fmt;

/// The text of one function, as it reads inside its module.
#[must_use]
pub fn function_text(function: &crate::Function) -> String {
    struct Text<'a>(&'a crate::Function);
    impl fmt::Display for Text<'_> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            function::write_function(formatter, self.0)
        }
    }
    Text(function).to_string()
}

impl fmt::Display for Module {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "module @{} lang {:?} target {:?} stage {}",
            spelling::Symbol(self.name.as_str()),
            self.language,
            self.target.triple(),
            match self.stage {
                Stage::Raw => "raw",
                Stage::Canonical => "canonical",
            }
        )?;
        for attribute in &self.attributes {
            items::write_attribute(formatter, attribute)?;
        }
        for declaration in &self.types {
            writeln!(formatter)?;
            items::write_type(formatter, declaration)?;
        }
        for global in &self.globals {
            writeln!(formatter)?;
            items::write_global(formatter, global)?;
        }
        for vtable in &self.vtables {
            writeln!(formatter)?;
            items::write_vtable(formatter, vtable)?;
        }
        for declaration in &self.traits {
            writeln!(formatter)?;
            items::write_trait(formatter, declaration)?;
        }
        for conformance in &self.conformances {
            writeln!(formatter)?;
            items::write_conformance(formatter, conformance)?;
        }
        for record in &self.imports {
            writeln!(formatter)?;
            items::write_import(formatter, record)?;
        }
        for record in &self.exports {
            writeln!(formatter)?;
            items::write_export(formatter, record)?;
        }
        for witness in &self.witnesses {
            writeln!(formatter)?;
            items::write_witness(formatter, witness)?;
        }
        for declaration in &self.functions {
            writeln!(formatter)?;
            function::write_function(formatter, declaration)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::parse_module;

    const EXAMPLE: &str = r#"module @Math lang "kira" target "aarch64-apple-darwin" stage raw

func module @Math.add(%a: i64, %b: i64) -> i64 engine(any) abi(ridl) {
bb0(%a: i64, %b: i64):
  %sum = add [trap] s %a, %b
  return %sum
}

func private @callback_holder(%callback: fn(i64) -> fn(context ptr) -> i64 abi(ridl) abi(platform), %value: i64) -> i64 engine(any) {
bb0(%callback: fn(i64) -> fn(context ptr) -> i64 abi(ridl) abi(platform), %value: i64):
  return %value
}
"#;

    #[test]
    fn canonical_text_round_trips() {
        let parsed = parse_module(EXAMPLE).unwrap();
        let rendered = parsed.to_string();
        let reparsed = parse_module(&rendered).unwrap();
        assert_eq!(parsed, reparsed);
    }

    const QUOTED: &str = r#"module @Shapes lang "kira" target "aarch64-apple-darwin" stage raw

func module @"Box<Int>.get"(%value: i64) -> i64 engine(any) {
bb0(%value: i64):
  return %value
}

func module @"identity<(Int) -> Int>"<$T>(%value: i64) -> i64 engine(any) {
bb0(%value: i64):
  %got = call @"Box<Int>.get"(%value)
  %address = func.ref @"Box<Int>.get"
  return %got
}
"#;

    #[test]
    fn symbols_outside_the_bare_spelling_round_trip_quoted() {
        let parsed = parse_module(QUOTED).unwrap();
        assert_eq!(parsed.functions[0].name.as_str(), "Box<Int>.get");
        assert_eq!(parsed.functions[1].name.as_str(), "identity<(Int) -> Int>");
        assert_eq!(parsed.functions[1].generics.len(), 1);
        let rendered = parsed.to_string();
        assert!(rendered.contains("call @\"Box<Int>.get\"(%value)"));
        assert_eq!(parsed, parse_module(&rendered).unwrap());
    }
}
