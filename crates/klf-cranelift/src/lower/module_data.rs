//! The data every function of a module reads: the slot of each global and
//! the bytes of each string constant and trap message.

use super::{GlobalMap, StringMap, trap_number};
use crate::{EmitError, err};
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::ObjectModule;
use klf_kir::{Module as KirModule, Operation};
use rustc_hash::FxHashMap as HashMap;

/// Declare one pointer-sized slot per module-level global, named `kirg.<name>`.
/// Each global's storage is heap-allocated at program start (matching the VM)
/// and its address is written into this slot; `global.addr` loads it back.
/// A thread-local global is instead the object's own thread-local data,
/// `kirt.<name>`, holding its initial bytes: each thread starts with a copy,
/// and `global.addr` is the running thread's.
pub(crate) fn declare_globals<'a>(
    object: &mut ObjectModule,
    module: &'a KirModule,
) -> Result<GlobalMap<'a>, EmitError> {
    let mut map = HashMap::default();
    let layouts = klf_layout::Layouts::new(&module.types);
    for global in &module.globals {
        if global.thread_local {
            let symbol = format!("kirt.{}", global.name);
            let id = object
                .declare_data(&symbol, Linkage::Local, true, true)
                .map_err(|e| err(format!("declare thread-local `{}`: {e}", global.name)))?;
            let layout = layouts.layout_of(&global.ty);
            let size = usize::try_from(layout.size.max(1)).unwrap_or(usize::MAX);
            let mut description = DataDescription::new();
            description.set_align(layout.align.max(1));
            // The initial bytes cover the whole object: a zero or short
            // initializer is padded, so the next thread-local starts past it.
            let mut bytes = global.init.init_bytes(size);
            bytes.resize(size, 0);
            description.define(bytes.into_boxed_slice());
            object
                .define_data(id, &description)
                .map_err(|e| err(format!("define thread-local `{}`: {e}", global.name)))?;
            map.insert(global.name.as_str(), id);
            continue;
        }
        let symbol = format!("kirg.{}", global.name);
        let id = object
            .declare_data(&symbol, Linkage::Local, true, false)
            .map_err(|e| err(format!("declare global `{}`: {e}", global.name)))?;
        let mut description = DataDescription::new();
        description.set_align(8);
        description.define_zeroinit(8);
        object
            .define_data(id, &description)
            .map_err(|e| err(format!("define global `{}`: {e}", global.name)))?;
        map.insert(global.name.as_str(), id);
    }
    Ok(map)
}

pub(crate) fn declare_strings(
    object: &mut ObjectModule,
    module: &KirModule,
) -> Result<StringMap, EmitError> {
    let mut map = HashMap::default();
    for value in module
        .functions
        .iter()
        .flat_map(|function| function.blocks.iter())
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| match &instruction.operation {
            Operation::ConstString(value) => Some(value),
            Operation::Trap(code) if trap_number(code).is_none() => Some(code),
            _ => None,
        })
    {
        if map.contains_key(value) {
            continue;
        }
        // Named by what it holds, so code an earlier build kept names it
        // again (`reuse`).
        let digest = <sha2::Sha256 as sha2::Digest>::digest(value.as_bytes());
        let symbol = format!("kirstr.{}", hex(&digest[..16]));
        let id = object
            .declare_data(&symbol, Linkage::Local, false, false)
            .map_err(|e| err(format!("declare string `{value}`: {e}")))?;
        let mut description = DataDescription::new();
        description.set_align(1);
        description.define(value.as_bytes().into());
        object
            .define_data(id, &description)
            .map_err(|e| err(format!("define string `{value}`: {e}")))?;
        map.insert(value.clone(), id);
    }
    Ok(map)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 15)],
            ]
        })
        .map(char::from)
        .collect()
}
