//! Whether the running program carries the debug information a trap's
//! backtrace is symbolized from (section 16.4).
//!
//! A stripped image names its frames no better than an address, so the trap
//! prints nothing extra when there is nothing to symbolize with. The answer is
//! read once, from the image itself: a `.debug_info` section on ELF, a
//! `__debug_info` section or the debug map naming the objects that hold it
//! on Mach-O, a `.debug` section on COFF.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

/// How many load commands or sections are read before the table is treated as
/// malformed, so a corrupt image cannot spin here.
const LIMIT: usize = 4096;

/// Whether the image this program runs from carries debug information.
pub(crate) fn present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| std::env::current_exe().is_ok_and(|exe| probe(&exe)))
}

/// Whether the image at `path` carries debug information.
pub(crate) fn probe(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0_u8; 4];
    if read(&mut file, 0, &mut magic).is_err() {
        return false;
    }
    match magic {
        [0x7f, b'E', b'L', b'F'] => elf(&mut file),
        [0xfe, 0xed, 0xfa, _]
        | [0xfe, 0xed, 0xfb, _]
        | [0xce, 0xfa, 0xed, _]
        | [0xcf, 0xfa, 0xed, _] => macho(&mut file),
        [b'P', b'E', 0, 0] => coff(&mut file),
        _ => false,
    }
}

fn read(file: &mut File, at: u64, into: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(into)
}

fn wide(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn narrow(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn table(file: &mut File, at: u64, size: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0_u8; size];
    read(file, at, &mut bytes).ok()?;
    Some(bytes)
}

/// The section header table of an ELF image and the names in it.
fn elf(file: &mut File) -> bool {
    let mut header = [0_u8; 64];
    if read(file, 0, &mut header).is_err() {
        return false;
    }
    let (headers_at, entry, count, names) = if header[4] == 2 {
        (
            wide(&header, 0x28),
            half(&header, 0x3a),
            half(&header, 0x3c),
            half(&header, 0x3e),
        )
    } else {
        (
            narrow(&header, 0x20).map(u64::from),
            half(&header, 0x2e),
            half(&header, 0x30),
            half(&header, 0x32),
        )
    };
    let (Some(headers_at), Some(entry), Some(count), Some(names)) =
        (headers_at, entry, count, names)
    else {
        return false;
    };
    let (count, entry) = (usize::from(count), usize::from(entry));
    let names = usize::from(names);
    if count == 0 || count > LIMIT || names >= count || entry < 40 {
        return false;
    }
    let Some(size) = count.checked_mul(entry) else {
        return false;
    };
    // `sh_offset` and `sh_size` sit after the 64-bit section header's wide
    // fields and after the 32-bit one's narrow ones.
    let (offset_at, size_at) = if header[4] == 2 { (24, 32) } else { (16, 20) };
    let Some(headers) = table(file, headers_at, size) else {
        return false;
    };
    let at = |index: usize| headers.get(index * entry..(index + 1) * entry);
    let (Some(strings_offset), Some(strings_size)) = (
        at(names).and_then(|entry| wide(entry, offset_at)),
        at(names).and_then(|entry| wide(entry, size_at)),
    ) else {
        return false;
    };
    let Ok(size) = usize::try_from(strings_size) else {
        return false;
    };
    let Some(strings) = table(file, strings_offset, size.min(1 << 20)) else {
        return false;
    };
    (0..count).any(|index| {
        at(index)
            .and_then(|entry| narrow(entry, 0))
            .and_then(|name| string_at(&strings, name))
            .is_some_and(|section| section == ".debug_info")
    })
}

fn string_at(strings: &[u8], at: u32) -> Option<&str> {
    let start = usize::try_from(at).ok()?;
    let text = strings.get(start..)?;
    std::str::from_utf8(text.split(|byte| *byte == 0).next()?).ok()
}

/// The sections of a Mach-O image, where DWARF lives under `__DWARF`.
fn macho(file: &mut File) -> bool {
    let mut head = [0_u8; 32];
    if read(file, 0, &mut head).is_err() {
        return false;
    }
    let sixty_four = matches!(
        [head[0], head[1], head[2], head[3]],
        [0xfe, 0xed, 0xfa, _] | [0xcf, 0xfa, 0xed, _]
    );
    // `LC_SEGMENT_64` or `LC_SEGMENT`, where the section count sits, and the
    // size of one section header.
    let (segment, count_at, sections, first, start) = if sixty_four {
        (0x19_u32, 64_usize, 80_usize, 72_usize, 32_u64)
    } else {
        (0x01, 48, 68, 56, 28)
    };
    let Some(commands) = narrow(&head, 20).map(u64::from) else {
        return false;
    };
    let mut walked = start;
    let mut symbols = None;
    for _ in 0..LIMIT {
        if walked >= start + commands {
            break;
        }
        let Some(command) = table(file, walked, 8) else {
            return false;
        };
        let Some(size) = narrow(&command, 4).map(u64::from) else {
            return false;
        };
        if size < 8 {
            return false;
        }
        if narrow(&command, 0) == Some(segment) {
            let Some(fixed) = table(file, walked, first) else {
                return false;
            };
            let Some(count) = narrow(&fixed, count_at) else {
                return false;
            };
            let count = usize::try_from(count).unwrap_or(LIMIT + 1);
            if count > LIMIT {
                return false;
            }
            let Some(size) = count.checked_mul(sections) else {
                return false;
            };
            let Some(entries) = table(file, walked + first as u64, size) else {
                return false;
            };
            if entries
                .chunks_exact(sections)
                .any(|entry| fixed_name(entry) == "__debug_info")
            {
                return true;
            }
        } else if narrow(&command, 0) == Some(0x2) {
            // `LC_SYMTAB`: where the symbols and their count are.
            symbols = table(file, walked, 16)
                .and_then(|symtab| Some((narrow(&symtab, 8)?, narrow(&symtab, 12)?)));
        }
        walked += size;
    }
    // A linked image usually leaves its DWARF in the objects it was linked
    // from, naming each in an `N_OSO` debug-map symbol, which is what a
    // backtrace symbolizes through.
    symbols.is_some_and(|(at, count)| debug_map(file, at, count, sixty_four))
}

/// Whether the symbol table at `at` holds an `N_OSO` entry.
fn debug_map(file: &mut File, at: u32, count: u32, sixty_four: bool) -> bool {
    const N_OSO: u8 = 0x66;
    let entry = if sixty_four { 16 } else { 12 };
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    // Debug-map symbols come first, so a bounded prefix decides.
    let read = count.min(LIMIT * 16);
    let Some(size) = read.checked_mul(entry) else {
        return false;
    };
    table(file, u64::from(at), size).is_some_and(|entries| {
        entries
            .chunks_exact(entry)
            .any(|symbol| symbol[4] == N_OSO)
    })
}

/// The name a section header starts with, up to its padding.
fn fixed_name(entry: &[u8]) -> &str {
    let name: &[u8] = entry.get(..16).unwrap_or_default();
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    std::str::from_utf8(&name[..end]).unwrap_or_default()
}

/// The section table of a COFF image, where a linked PE keeps `.debug`.
fn coff(file: &mut File) -> bool {
    let mut pointer = [0_u8; 4];
    if read(file, 0x3c, &mut pointer).is_err() {
        return false;
    }
    let Some(pe) = narrow(&pointer, 0).map(u64::from) else {
        return false;
    };
    let Some(head) = table(file, pe, 24) else {
        return false;
    };
    let (Some(count), Some(optional)) = (half(&head, 2), half(&head, 16)) else {
        return false;
    };
    let count = usize::from(count);
    if count > LIMIT {
        return false;
    }
    let sections = pe + 24 + u64::from(optional);
    (0..count).any(|index| {
        let at = sections + index as u64 * 40;
        table(file, at, 8).is_some_and(|name| {
            let end = name.iter().position(|byte| *byte == 0).unwrap_or(8);
            std::str::from_utf8(&name[..end]).is_ok_and(|name| name.starts_with(".debug"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{fixed_name, probe, string_at};
    use std::path::{Path, PathBuf};

    #[test]
    fn a_section_name_stops_at_its_padding() {
        let name = b"__debug_info";
        let mut entry = [b'_'; 16];
        entry[..name.len()].copy_from_slice(name);
        entry[name.len()] = 0;
        assert_eq!(fixed_name(&entry), "__debug_info");
    }

    #[test]
    fn a_string_table_entry_is_the_bytes_up_to_its_nul() {
        let strings = b".debug_info\0.debug_line\0\0";
        assert_eq!(string_at(strings, 0), Some(".debug_info"));
        assert_eq!(string_at(strings, 12), Some(".debug_line"));
        assert_eq!(string_at(strings, 999), None);
    }

    /// The binary this test runs from, which cargo builds with debug info.
    fn self_image() -> PathBuf {
        std::env::current_exe().expect("the test binary")
    }

    fn objcopy() -> Option<PathBuf> {
        klf_target::llvm_tool("llvm-objcopy")
    }

    #[test]
    fn an_image_carrying_dwarf_is_recognized_and_one_without_it_is_not() {
        let image = self_image();
        assert!(probe(&image), "this test binary carries debug information");

        let Some(objcopy) = objcopy() else {
            eprintln!("skipping the stripped half: no llvm-objcopy");
            return;
        };
        let stripped = std::env::temp_dir().join("klf-image-stripped");
        std::fs::copy(&image, &stripped).expect("the image is copied");
        let status = std::process::Command::new(&objcopy)
            .args(["--strip-debug"])
            .arg(&stripped)
            .status()
            .expect("llvm-objcopy runs");
        assert!(status.success(), "llvm-objcopy failed");
        assert!(
            !probe(&stripped),
            "an image stripped of its debug information still reports some"
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_is_not_mistaken_for_one() {
        let noise = std::env::temp_dir().join("klf-image-noise");
        std::fs::write(&noise, vec![0_u8; 4096]).expect("noise written");
        assert!(!probe(&noise));
        assert!(!probe(Path::new("/nonexistent/klf-image")));
    }
}
