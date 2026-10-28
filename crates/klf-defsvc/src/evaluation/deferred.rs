//! Compile-time calls a unit cannot fold alone. A unit's module only
//! declares the functions other units define, so a compile-time function
//! that reaches one of them runs only once the units are merged into the
//! program (section 14.4: an evaluation makes no foreign calls, and a call
//! into another unit is not one). The unit records each such function as a
//! module attribute, which merging keeps, and the merged module folds them.

use super::{Buffer, BufferResult};
use klf_core::Name;
use klf_kir::{Module, ModuleAttribute, Operation};
use std::collections::HashSet;

/// The attribute naming one compile-time function whose calls wait for the
/// merged module.
const ATTRIBUTE: &str = "klf.comptime.deferred";

/// Whether the functions at `reached` call one the module only declares. A
/// kernel service (`klfsvc_*`, section 8.12) is declared without a body in
/// every unit and defined by none, so merging adds nothing to it: a call to
/// one, such as the `klf.trap.operands` a failing check makes on its way to
/// the trap, does not wait for the merged module.
pub(super) fn calls_undefined(module: &Module, reached: &HashSet<usize>) -> bool {
    let declared_only: HashSet<&Name> = module
        .functions
        .iter()
        .filter(|function| function.foreign && function.blocks.is_empty())
        .filter(|function| !function.name.as_str().starts_with("klfsvc_"))
        .map(|function| &function.name)
        .collect();
    reached
        .iter()
        .flat_map(|position| &module.functions[*position].blocks)
        .flat_map(|block| &block.instructions)
        .any(|instruction| {
            matches!(&instruction.operation, Operation::Call { callee, .. }
                if declared_only.contains(callee))
        })
}

/// Records `marked` and their buffer results on the module.
pub(super) fn defer(module: &mut Module, marked: &[Name], buffers: &[BufferResult]) {
    for name in marked {
        let value = match buffers.iter().find(|buffer| buffer.name == *name) {
            None => name.as_str().to_owned(),
            Some(BufferResult { buffer: None, .. }) => format!("{name} record"),
            Some(BufferResult {
                buffer: Some(buffer),
                ..
            }) => format!(
                "{name} buffer {} {} {} {}",
                buffer.allocator, buffer.data_field, buffer.length_field, buffer.element_bytes
            ),
        };
        let attribute = ModuleAttribute {
            name: ATTRIBUTE.to_owned(),
            value: Some(value),
        };
        if !module.attributes.contains(&attribute) {
            module.attributes.push(attribute);
        }
    }
}

/// Takes the deferred functions off the module: their names and buffer
/// results.
pub(super) fn take(module: &mut Module) -> Result<(Vec<Name>, Vec<BufferResult>), String> {
    let mut marked = Vec::new();
    let mut buffers = Vec::new();
    let mut kept = Vec::new();
    for attribute in std::mem::take(&mut module.attributes) {
        if attribute.name != ATTRIBUTE {
            kept.push(attribute);
            continue;
        }
        let value = attribute.value.unwrap_or_default();
        let words: Vec<&str> = value.split(' ').collect();
        let name = Name::from(words[0]);
        match words.as_slice() {
            [_] => {}
            [_, "record"] => buffers.push(BufferResult {
                name: name.clone(),
                buffer: None,
            }),
            [_, "buffer", allocator, data, length, bytes] => buffers.push(BufferResult {
                name: name.clone(),
                buffer: Some(Buffer {
                    allocator: Name::from(*allocator),
                    data_field: number(data)?,
                    length_field: number(length)?,
                    element_bytes: number(bytes)?,
                }),
            }),
            _ => return Err(format!("malformed `{ATTRIBUTE}` attribute `{value}`")),
        }
        if !marked.contains(&name) {
            marked.push(name);
        }
    }
    module.attributes = kept;
    Ok((marked, buffers))
}

fn number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("malformed `{ATTRIBUTE}` attribute number `{text}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_functions_round_trip_through_module_attributes() {
        let target = "x86_64-unknown-linux-gnu".parse().expect("a target");
        let mut module = Module::empty(Name::from("unit"), "kira", target, klf_kir::Stage::Raw);
        let buffers = vec![
            BufferResult {
                name: Name::from("table"),
                buffer: Some(Buffer {
                    allocator: Name::from("allocate"),
                    data_field: 0,
                    length_field: 1,
                    element_bytes: 4,
                }),
            },
            BufferResult {
                name: Name::from("point"),
                buffer: None,
            },
        ];
        let marked = [
            Name::from("table"),
            Name::from("point"),
            Name::from("count"),
        ];
        defer(&mut module, &marked, &buffers);
        defer(&mut module, &marked, &buffers);
        assert_eq!(module.attributes.len(), 3);
        let (names, results) = take(&mut module).expect("the attributes read back");
        assert_eq!(module.attributes, Vec::new());
        assert_eq!(names, marked);
        let table = results[0].buffer.as_ref().expect("the table has a buffer");
        assert_eq!(table.allocator, Name::from("allocate"));
        assert_eq!(
            (table.data_field, table.length_field, table.element_bytes),
            (0, 1, 4)
        );
        assert!(results[1].buffer.is_none());
    }
}
