use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

const HEADER_SIZE: u64 = 24;

mod archive;
mod command;
mod sign;

pub(super) use command::{command, validate_manifest, verify_entry};
pub(super) use sign::verify_package_signature;

pub(super) struct PackageReader {
    source: Source,
    manifest: Value,
}

enum Source {
    Directory(PathBuf),
    Archive {
        path: PathBuf,
        entries: BTreeMap<String, ArchiveEntry>,
    },
}

struct ArchiveEntry {
    offset: u64,
    stored_size: u64,
    size: u64,
    compression: u8,
    hash: [u8; 32],
}

pub(super) fn manifest_platforms(manifest: &Value) -> Result<Vec<String>, String> {
    let platforms = match manifest.get("platforms") {
        None => vec!["tessera".to_owned()],
        Some(value) => value
            .as_array()
            .ok_or_else(|| "manifest platforms must be an array".to_owned())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "manifest platforms must contain only strings".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let mut seen = std::collections::BTreeSet::new();
    for platform in &platforms {
        if !matches!(
            platform.as_str(),
            "tessera" | "macos" | "ios" | "windows" | "linux" | "android" | "web" | "wasi"
        ) {
            return Err(format!("manifest lists unsupported platform `{platform}`"));
        }
        if !seen.insert(platform) {
            return Err(format!("manifest repeats platform `{platform}`"));
        }
    }
    Ok(platforms)
}

pub(super) fn driver_source_artifact(manifest: &Value) -> Result<(Vec<u8>, [u8; 32]), String> {
    let source = manifest
        .get("source")
        .ok_or_else(|| "driver manifest is missing source".to_owned())?;
    let object = source
        .as_object()
        .ok_or_else(|| "driver source must be an object".to_owned())?;
    for field in ["repository", "revision", "environment"] {
        if !object
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(format!(
                "driver source is missing nonempty string `{field}`"
            ));
        }
    }
    let build = object
        .get("build")
        .and_then(Value::as_array)
        .filter(|arguments| !arguments.is_empty())
        .ok_or_else(|| "driver source `build` must be a nonempty string array".to_owned())?;
    if build.iter().any(|argument| argument.as_str().is_none()) {
        return Err("driver source `build` must contain only strings".to_owned());
    }
    let toolchain = object
        .get("toolchain")
        .and_then(Value::as_object)
        .filter(|toolchain| !toolchain.is_empty())
        .ok_or_else(|| "driver source `toolchain` must be a nonempty object".to_owned())?;
    if toolchain.iter().any(|(tool, build)| {
        tool.is_empty() || !build.as_str().is_some_and(|build| !build.is_empty())
    }) {
        return Err(
            "driver source toolchain entries must have nonempty names and build IDs".to_owned(),
        );
    }
    let bytes = super::jcs::canonical_json(source)?.into_bytes();
    let hash = *blake3::hash(&bytes).as_bytes();
    Ok((bytes, hash))
}

impl PackageReader {
    pub(super) fn with_manifest(path: &Path, manifest: Value) -> Result<Self, String> {
        let root = fs::canonicalize(path).map_err(|error| {
            format!(
                "cannot resolve package directory '{}': {error}",
                path.display()
            )
        })?;
        if !root.is_dir() {
            return Err(format!(
                "package source '{}' is not a directory",
                path.display()
            ));
        }
        if !manifest.is_object() {
            return Err("package manifest must contain an object".to_owned());
        }
        Ok(Self {
            source: Source::Directory(root),
            manifest,
        })
    }

    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path)
            .map_err(|error| format!("cannot inspect package '{}': {error}", path.display()))?;
        let source = if metadata.is_dir() {
            let root = fs::canonicalize(path).map_err(|error| {
                format!(
                    "cannot resolve package directory '{}': {error}",
                    path.display()
                )
            })?;
            Source::Directory(root)
        } else if metadata.is_file() {
            let archive = fs::canonicalize(path).map_err(|error| {
                format!(
                    "cannot resolve package archive '{}': {error}",
                    path.display()
                )
            })?;
            Source::Archive {
                entries: read_archive_index(&archive)?,
                path: archive,
            }
        } else {
            return Err(format!(
                "package '{}' is not a file or directory",
                path.display()
            ));
        };
        let mut package = Self {
            source,
            manifest: Value::Null,
        };
        let bytes = package.read("manifest.json")?;
        package.manifest = super::jcs::parse_json(&bytes, "package manifest.json")?;
        if !package.manifest.is_object() {
            return Err("package manifest.json must contain an object".to_owned());
        }
        Ok(package)
    }

    pub(super) fn manifest(&self) -> &Value {
        &self.manifest
    }

    pub(super) fn verify_archive_entries(&self) -> Result<(), String> {
        let Source::Archive { path, entries } = &self.source else {
            return Ok(());
        };
        for (name, entry) in entries {
            archive_entry_digest(path, entry, name)?;
        }
        Ok(())
    }

    pub(super) fn digest(&self, path: &str) -> Result<([u8; 32], u64), String> {
        validate_package_path(path)?;
        match &self.source {
            Source::Directory(root) => {
                let joined = root.join(path);
                let resolved = fs::canonicalize(&joined)
                    .map_err(|error| format!("cannot read package file '{path}': {error}"))?;
                if !resolved.starts_with(root) || !resolved.is_file() {
                    return Err(format!(
                        "package path '{path}' is not a regular file inside the package"
                    ));
                }
                let file = File::open(&resolved)
                    .map_err(|error| format!("cannot read package file '{path}': {error}"))?;
                hash_reader(file, u64::MAX)
                    .map_err(|error| format!("cannot hash package file '{path}': {error}"))
            }
            Source::Archive {
                path: archive,
                entries,
            } => {
                let entry = entries
                    .get(path)
                    .ok_or_else(|| format!("package archive has no entry '{path}'"))?;
                archive_entry_digest(archive, entry, path)
            }
        }
    }

    pub(super) fn source_path(&self, path: &str) -> Result<PathBuf, String> {
        validate_package_path(path)?;
        let Source::Directory(root) = &self.source else {
            return Err("package source path requires a directory package".to_owned());
        };
        let joined = root.join(path);
        let resolved = fs::canonicalize(&joined)
            .map_err(|error| format!("cannot read package file '{path}': {error}"))?;
        if !resolved.starts_with(root) || !resolved.is_file() {
            return Err(format!(
                "package path '{path}' is not a regular file inside the package"
            ));
        }
        Ok(resolved)
    }

    pub(super) fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        validate_package_path(path)?;
        match &self.source {
            Source::Directory(root) => {
                let joined = root.join(path);
                let resolved = fs::canonicalize(&joined)
                    .map_err(|error| format!("cannot read package file '{path}': {error}"))?;
                if !resolved.starts_with(root) || !resolved.is_file() {
                    return Err(format!(
                        "package path '{path}' is not a regular file inside the package"
                    ));
                }
                fs::read(resolved)
                    .map_err(|error| format!("cannot read package file '{path}': {error}"))
            }
            Source::Archive {
                path: archive,
                entries,
            } => {
                let entry = entries
                    .get(path)
                    .ok_or_else(|| format!("package archive has no entry '{path}'"))?;
                read_archive_entry(archive, entry, path)
            }
        }
    }
}

fn read_archive_index(path: &Path) -> Result<BTreeMap<String, ArchiveEntry>, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("cannot open package archive '{}': {error}", path.display()))?;
    let file_size = file
        .metadata()
        .map_err(|error| {
            format!(
                "cannot inspect package archive '{}': {error}",
                path.display()
            )
        })?
        .len();
    if file_size < HEADER_SIZE {
        return Err("package archive header is truncated".to_owned());
    }
    let mut header = [0_u8; HEADER_SIZE as usize];
    file.read_exact(&mut header)
        .map_err(|error| format!("cannot read package archive header: {error}"))?;
    if &header[..4] != b"TIRA"
        || u16::from_le_bytes([header[4], header[5]]) != 1
        || u16::from_le_bytes([header[6], header[7]]) != 0
        || u32::from_le_bytes(header[12..16].try_into().expect("fixed header slice")) != 0
    {
        return Err("invalid TIRA archive header".to_owned());
    }
    let count = u32::from_le_bytes(header[8..12].try_into().expect("fixed header slice"));
    let table_offset = u64::from_le_bytes(header[16..24].try_into().expect("fixed header slice"));
    if count == 0 || table_offset < HEADER_SIZE || table_offset >= file_size {
        return Err("invalid TIRA archive table location or entry count".to_owned());
    }
    let table_size = usize::try_from(file_size - table_offset)
        .map_err(|_| "TIRA archive table is too large for this host".to_owned())?;
    let mut table = vec![0_u8; table_size];
    file.seek(SeekFrom::Start(table_offset))
        .and_then(|_| file.read_exact(&mut table))
        .map_err(|error| format!("cannot read TIRA archive table: {error}"))?;

    let mut cursor = 0;
    let mut entries = BTreeMap::new();
    let mut ranges = Vec::new();
    let mut previous_path: Option<String> = None;
    for _ in 0..count {
        let path_length = usize::try_from(read_uleb(&table, &mut cursor)?)
            .map_err(|_| "TIRA path length overflows memory".to_owned())?;
        let path_end = cursor
            .checked_add(path_length)
            .filter(|end| *end <= table.len())
            .ok_or_else(|| "truncated TIRA archive path".to_owned())?;
        let entry_path = std::str::from_utf8(&table[cursor..path_end])
            .map_err(|_| "TIRA archive path is not UTF-8".to_owned())?
            .to_owned();
        validate_package_path(&entry_path)?;
        if previous_path
            .as_ref()
            .is_some_and(|previous| entry_path <= *previous)
        {
            return Err("TIRA archive paths are not strictly ordered".to_owned());
        }
        previous_path = Some(entry_path.clone());
        cursor = path_end;
        let offset = read_u64(&table, &mut cursor, "entry offset")?;
        let stored_size = read_u64(&table, &mut cursor, "stored size")?;
        let size = read_u64(&table, &mut cursor, "uncompressed size")?;
        let compression = *table
            .get(cursor)
            .ok_or_else(|| "truncated TIRA compression field".to_owned())?;
        cursor += 1;
        if compression > 1 {
            return Err(format!("TIRA entry '{entry_path}' has unknown compression"));
        }
        let hash_end = cursor
            .checked_add(32)
            .filter(|end| *end <= table.len())
            .ok_or_else(|| "truncated TIRA entry hash".to_owned())?;
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(&table[cursor..hash_end]);
        cursor = hash_end;
        let end = offset
            .checked_add(stored_size)
            .ok_or_else(|| format!("TIRA entry '{entry_path}' range overflows"))?;
        if offset < HEADER_SIZE || end > table_offset || compression == 0 && size != stored_size {
            return Err(format!(
                "TIRA entry '{entry_path}' has an invalid data range"
            ));
        }
        ranges.push((offset, end, entry_path.clone()));
        entries.insert(
            entry_path,
            ArchiveEntry {
                offset,
                stored_size,
                size,
                compression,
                hash,
            },
        );
    }
    if cursor != table.len() {
        return Err("trailing bytes in TIRA archive table".to_owned());
    }
    ranges.sort_unstable_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(format!(
                "TIRA entries '{}' and '{}' overlap",
                pair[0].2, pair[1].2
            ));
        }
    }
    Ok(entries)
}

fn read_archive_entry(path: &Path, entry: &ArchiveEntry, name: &str) -> Result<Vec<u8>, String> {
    let stored_size = usize::try_from(entry.stored_size)
        .map_err(|_| format!("TIRA entry '{name}' is too large for this host"))?;
    let size = usize::try_from(entry.size)
        .map_err(|_| format!("TIRA entry '{name}' is too large for this host"))?;
    let mut stored = vec![0_u8; stored_size];
    let mut file = File::open(path)
        .map_err(|error| format!("cannot open package archive '{}': {error}", path.display()))?;
    file.seek(SeekFrom::Start(entry.offset))
        .and_then(|_| file.read_exact(&mut stored))
        .map_err(|error| format!("cannot read TIRA entry '{name}': {error}"))?;
    let content = if entry.compression == 0 {
        stored
    } else {
        zstd::bulk::decompress(&stored, size)
            .map_err(|error| format!("cannot decompress TIRA entry '{name}': {error}"))?
    };
    if content.len() != size || blake3::hash(&content).as_bytes() != &entry.hash {
        return Err(format!(
            "TIRA entry '{name}' has the wrong size or BLAKE3 hash"
        ));
    }
    Ok(content)
}

fn archive_entry_digest(
    path: &Path,
    entry: &ArchiveEntry,
    name: &str,
) -> Result<([u8; 32], u64), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("cannot open package archive '{}': {error}", path.display()))?;
    file.seek(SeekFrom::Start(entry.offset))
        .map_err(|error| format!("cannot seek to TIRA entry '{name}': {error}"))?;
    let stored = file.take(entry.stored_size);
    let (hash, size) = if entry.compression == 0 {
        hash_reader(stored, entry.size)
    } else {
        let decoder = zstd::stream::read::Decoder::new(stored)
            .map_err(|error| format!("cannot decompress TIRA entry '{name}': {error}"))?;
        hash_reader(decoder, entry.size)
    }
    .map_err(|error| format!("cannot hash TIRA entry '{name}': {error}"))?;
    if size != entry.size || hash != entry.hash {
        return Err(format!(
            "TIRA entry '{name}' has the wrong size or BLAKE3 hash"
        ));
    }
    Ok((hash, size))
}

fn hash_reader(mut reader: impl Read, limit: u64) -> Result<([u8; 32], u64), std::io::Error> {
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size.saturating_add(count as u64);
        if size > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "uncompressed entry exceeds its declared size",
            ));
        }
        hasher.update(&buffer[..count]);
    }
    Ok((*hasher.finalize().as_bytes(), size))
}

fn validate_package_path(path: &str) -> Result<(), String> {
    let bytes = path.as_bytes();
    let has_drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || has_drive_prefix
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("invalid package-relative path '{path}'"));
    }
    Ok(())
}

fn read_u64(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, String> {
    let end = cursor
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| format!("truncated TIRA {label}"))?;
    let value = u64::from_le_bytes(
        bytes[*cursor..end]
            .try_into()
            .map_err(|_| format!("invalid TIRA {label}"))?,
    );
    *cursor = end;
    Ok(value)
}

fn read_uleb(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let start = *cursor;
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| "truncated TIRA ULEB".to_owned())?;
        *cursor += 1;
        if shift == 63 && byte & 0xfe != 0 {
            return Err("TIRA ULEB overflows u64".to_owned());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if *cursor - start > 1 && byte == 0 {
                return Err("nonminimal TIRA ULEB".to_owned());
            }
            return Ok(value);
        }
    }
    Err("unterminated TIRA ULEB".to_owned())
}
