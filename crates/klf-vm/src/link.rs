//! Static linking of bytecode: a program and a library, such as a language's
//! `std.klbc`, become one program.
//!
//! The library's functions, constants and globals are appended after the
//! program's, and every index its code carries is moved by the same amount:
//! `LOADK` and `TRAP` name a constant, `LEAG` a global, `LEAFN` a function,
//! and a call site its callee or, for `call.foreign`, the constant holding the
//! symbol. Then each `call.foreign` whose symbol a library function defines
//! becomes a direct call, so the program never leaves the VM to reach its std.
//! The library's native half comes along: its entries number the library's
//! functions, which now start where the library was appended.

use crate::bytecode::{Instr, Opcode};
use crate::program::{Import, ImportKind, Program};
use std::path::{Path, PathBuf};

/// Where a sysroot keeps `language`'s std bytecode for a pointer width:
/// `lib/<lang>/bytecode/<ptrwidth>/std.klbc` (section 5.4).
#[must_use]
pub fn std_bytecode_path(root: &Path, language: &str, pointer_width: u8) -> PathBuf {
    root.join("lib")
        .join(language)
        .join("bytecode")
        .join(pointer_width.to_string())
        .join("std.klbc")
}

/// Links the std bytecode at `path` into `program`, so the program carries its
/// std the way an executable carries `libstd.a`. The std's native half sits
/// beside the std, not beside the program, so its library imports are pointed
/// there. Returns whether there was a std to link.
///
/// # Errors
/// Returns a message when the std does not decode or does not link.
pub fn link_std(program: &mut Program, path: &Path) -> Result<bool, String> {
    let Ok(image) = std::fs::read(path) else {
        return Ok(false);
    };
    let mut library = crate::klbc::decode(&image)
        .map_err(|error| format!("cannot read {}: {error:?}", path.display()))?;
    for import in &mut library.imports {
        if matches!(import.kind, ImportKind::Library { .. }) {
            import.symbol = path
                .with_file_name(&import.symbol)
                .to_string_lossy()
                .into_owned();
        }
    }
    link(program, library)?;
    Ok(true)
}

/// Appends `library` to `program` and binds the program's foreign calls to the
/// library functions of the same name. Functions the program defines keep
/// their indices, so its entry point is unchanged.
///
/// # Errors
/// Returns a message when the library holds a code word that does not decode,
/// or the linked program outgrows an operand.
pub fn link(program: &mut Program, library: Program) -> Result<(), String> {
    let function_offset = program.functions.len();
    let const_offset = program.consts.len();
    let global_offset = program.globals.len();
    let import_offset = program.imports.len();
    for mut function in library.functions {
        for word in &mut function.code {
            *word = relocate(*word, function_offset, const_offset, global_offset)?;
        }
        for site in &mut function.call_sites {
            if let Some(symbol) = site.foreign.as_mut() {
                *symbol = offset_u32(*symbol, import_offset)?;
            } else if site.target.is_none() {
                site.callee += function_offset;
            }
        }
        program.functions.push(function);
    }
    program.consts.extend(library.consts);
    for mut import in library.imports {
        if let ImportKind::Library { first } = &mut import.kind {
            *first = offset_u32(*first, function_offset)?;
        }
        program.imports.push(import);
    }
    program.globals.extend(library.globals);
    // The library's initializers run first, so what it defines is ready for
    // the program's.
    let mut initializers: Vec<usize> = library
        .initializers
        .iter()
        .map(|index| index + function_offset)
        .collect();
    initializers.append(&mut program.initializers);
    program.initializers = initializers;
    bind_foreign_calls(program, function_offset);
    Ok(())
}

fn relocate(
    word: u64,
    function_offset: usize,
    const_offset: usize,
    global_offset: usize,
) -> Result<u64, String> {
    let instruction = Instr::decode(word)
        .ok_or_else(|| format!("library code word {word:#x} does not decode"))?;
    let offset = match instruction.op {
        Opcode::LoadK | Opcode::Trap => const_offset,
        Opcode::GlobalA => global_offset,
        Opcode::LeaFn => function_offset,
        _ => return Ok(word),
    };
    let index = usize::try_from(instruction.bx()).map_err(|_| "negative bytecode index")?;
    let moved = i32::try_from(index + offset).map_err(|_| "linked program is too large")?;
    Ok(Instr::from_bx(instruction.op, instruction.a, moved)
        .with_flags(instruction.flags)
        .encode())
}

fn offset_u32(index: u32, offset: usize) -> Result<u32, String> {
    u32::try_from(index as usize + offset)
        .map_err(|_| "linked program has too many imports".to_owned())
}

fn bind_foreign_calls(program: &mut Program, library_start: usize) {
    let defined: std::collections::HashMap<String, usize> = program
        .functions
        .iter()
        .enumerate()
        .skip(library_start)
        .filter(|(_, function)| !function.code.is_empty())
        .map(|(index, function)| (function.name.as_str().to_owned(), index))
        .collect();
    let imports = &program.imports;
    for function in &mut program.functions {
        for site in &mut function.call_sites {
            let Some(import) = site.foreign else {
                continue;
            };
            let Some(Import {
                symbol,
                kind: ImportKind::Call(_),
            }) = imports.get(import as usize)
            else {
                continue;
            };
            if let Some(&callee) = defined.get(symbol) {
                site.callee = callee;
                site.foreign = None;
            }
        }
    }
}
