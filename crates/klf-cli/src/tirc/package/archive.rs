use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::validate_package_path;

const HEADER_SIZE: u64 = 24;

pub(super) enum ArchiveContent {
    Bytes(Vec<u8>),
    File(PathBuf),
}

struct PreparedEntry<'a> {
    name: &'a str,
    content: &'a ArchiveContent,
    offset: u64,
    size: u64,
    hash: [u8; 32],
}

pub(super) fn write_archive(
    path: &Path,
    entries: &BTreeMap<String, ArchiveContent>,
) -> Result<(), String> {
    if path.exists() {
        return Err(format!("output '{}' already exists", path.display()));
    }
    if entries.is_empty() {
        return Err("cannot write an empty TIRA archive".to_owned());
    }
    let count =
        u32::try_from(entries.len()).map_err(|_| "TIRA archive has too many entries".to_owned())?;
    let mut prepared = Vec::with_capacity(entries.len());
    let mut next_offset = HEADER_SIZE;
    for (name, content) in entries {
        validate_package_path(name)?;
        let (hash, size) = content_digest(content, name)?;
        prepared.push(PreparedEntry {
            name,
            content,
            offset: next_offset,
            size,
            hash,
        });
        next_offset = next_offset
            .checked_add(size)
            .ok_or_else(|| format!("TIRA entry '{name}' offset overflows"))?;
    }
    let table_offset = next_offset;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create output directory '{}': {error}",
            parent.display()
        )
    })?;
    let (temporary_path, mut output) = create_temporary(path)?;
    let result = write_archive_contents(&mut output, count, table_offset, &prepared);
    if let Err(error) = result {
        drop(output);
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(output);
    if path.exists() {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "output '{}' appeared while packaging",
            path.display()
        ));
    }
    fs::rename(&temporary_path, path).map_err(|error| {
        let _ = fs::remove_file(&temporary_path);
        format!(
            "cannot publish package archive '{}': {error}",
            path.display()
        )
    })
}

fn create_temporary(path: &Path) -> Result<(PathBuf, File), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| format!("output '{}' has no file name", path.display()))?
        .to_string_lossy();
    for index in 0..32_u32 {
        let candidate = parent.join(format!(".{name}.partial-{}-{index}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create temporary package file '{}': {error}",
                    candidate.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot reserve a temporary file beside '{}': too many collisions",
        path.display()
    ))
}

fn write_archive_contents(
    output: &mut File,
    count: u32,
    table_offset: u64,
    entries: &[PreparedEntry<'_>],
) -> Result<(), String> {
    output
        .write_all(&[0; HEADER_SIZE as usize])
        .map_err(|error| format!("cannot write TIRA header: {error}"))?;
    for entry in entries {
        let position = output
            .stream_position()
            .map_err(|error| format!("cannot locate TIRA entry '{}': {error}", entry.name))?;
        if position != entry.offset {
            return Err(format!(
                "internal TIRA offset mismatch for '{}'",
                entry.name
            ));
        }
        write_content(output, entry)?;
    }
    let position = output
        .stream_position()
        .map_err(|error| format!("cannot locate TIRA archive table: {error}"))?;
    if position != table_offset {
        return Err("internal TIRA table offset mismatch".to_owned());
    }
    for entry in entries {
        write_uleb(entry.name.len() as u64, output)?;
        output
            .write_all(entry.name.as_bytes())
            .and_then(|()| output.write_all(&entry.offset.to_le_bytes()))
            .and_then(|()| output.write_all(&entry.size.to_le_bytes()))
            .and_then(|()| output.write_all(&entry.size.to_le_bytes()))
            .and_then(|()| output.write_all(&[0]))
            .and_then(|()| output.write_all(&entry.hash))
            .map_err(|error| {
                format!("cannot write TIRA entry '{}' metadata: {error}", entry.name)
            })?;
    }
    let mut header = [0_u8; HEADER_SIZE as usize];
    header[..4].copy_from_slice(b"TIRA");
    header[4..6].copy_from_slice(&1_u16.to_le_bytes());
    header[8..12].copy_from_slice(&count.to_le_bytes());
    header[16..24].copy_from_slice(&table_offset.to_le_bytes());
    output
        .seek(SeekFrom::Start(0))
        .and_then(|_| output.write_all(&header))
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("cannot finalize TIRA archive header: {error}"))
}

fn content_digest(content: &ArchiveContent, name: &str) -> Result<([u8; 32], u64), String> {
    match content {
        ArchiveContent::Bytes(bytes) => Ok((*blake3::hash(bytes).as_bytes(), bytes.len() as u64)),
        ArchiveContent::File(path) => {
            let file = File::open(path).map_err(|error| {
                format!(
                    "cannot open package file for '{name}' '{}': {error}",
                    path.display()
                )
            })?;
            super::hash_reader(file, u64::MAX)
                .map_err(|error| format!("cannot hash package file '{name}': {error}"))
        }
    }
}

fn write_content(output: &mut File, entry: &PreparedEntry<'_>) -> Result<(), String> {
    match entry.content {
        ArchiveContent::Bytes(bytes) => output
            .write_all(bytes)
            .map_err(|error| format!("cannot write TIRA entry '{}': {error}", entry.name)),
        ArchiveContent::File(path) => {
            let mut input = File::open(path).map_err(|error| {
                format!("cannot open package file for '{}': {error}", entry.name)
            })?;
            let mut hasher = blake3::Hasher::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = input.read(&mut buffer).map_err(|error| {
                    format!("cannot read package file '{}': {error}", entry.name)
                })?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count]).map_err(|error| {
                    format!("cannot write TIRA entry '{}': {error}", entry.name)
                })?;
                hasher.update(&buffer[..count]);
                size = size.saturating_add(count as u64);
                if size > entry.size {
                    return Err(format!(
                        "package file '{}' grew while archiving",
                        entry.name
                    ));
                }
            }
            if size != entry.size || hasher.finalize().as_bytes() != &entry.hash {
                return Err(format!(
                    "package file '{}' changed while archiving",
                    entry.name
                ));
            }
            Ok(())
        }
    }
}

fn write_uleb(mut value: u64, output: &mut impl Write) -> Result<(), String> {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output
            .write_all(&[byte])
            .map_err(|error| format!("cannot write TIRA table: {error}"))?;
        if value == 0 {
            return Ok(());
        }
    }
}
