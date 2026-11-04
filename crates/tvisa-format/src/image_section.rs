use tir_format::StringTable;
use tir_format::TirVersion;
use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::{FormatError, TvisaImage};
use crate::text::{ImageKind, TextImage};

const IMAGE_SECTION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageFunctionRecord {
    pub function_index: u64,
    pub priority: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageRecord {
    pub name: String,
    pub kind: ImageKind,
    pub package_digest: [u8; 32],
    pub tirc_build: String,
    pub tir_version: TirVersion,
    /// Module digests sorted in ascending byte order, retaining duplicates.
    pub module_hashes: Vec<[u8; 32]>,
    pub entry_function: Option<u64>,
    /// Records are sorted by priority, then function index.
    pub initializers: Vec<ImageFunctionRecord>,
    /// Records use the same order as initializers; unload runs them backwards.
    pub finalizers: Vec<ImageFunctionRecord>,
    pub gpu_capabilities: u64,
    pub minimum_driver_api: Option<(u8, u8)>,
}

impl ImageRecord {
    /// Resolves the text IMAGE metadata into its binary record model.
    pub fn from_text(
        image: &TextImage,
        mut resolve_function: impl FnMut(&str) -> Option<u64>,
    ) -> Result<Self, FormatError> {
        let resolve = |name: &str, resolve_function: &mut dyn FnMut(&str) -> Option<u64>| {
            resolve_function(name)
                .ok_or_else(|| FormatError::malformed(None, "IMAGE function is unresolved"))
        };
        let entry_function = image
            .image_metadata
            .entry_function
            .as_deref()
            .map(|name| {
                resolve(
                    name.strip_prefix('@').unwrap_or(name),
                    &mut resolve_function,
                )
            })
            .transpose()?;
        let mut initializers =
            resolve_image_functions(&image.image_metadata.initializers, &mut resolve_function)?;
        let mut finalizers =
            resolve_image_functions(&image.image_metadata.finalizers, &mut resolve_function)?;
        initializers.sort_by_key(|function| (function.priority, function.function_index));
        finalizers.sort_by_key(|function| (function.priority, function.function_index));
        Ok(Self {
            name: image.header.image.clone(),
            kind: image.header.kind,
            package_digest: image.image_metadata.package_digest,
            tirc_build: image.image_metadata.tirc_build.clone(),
            tir_version: TirVersion {
                year: image.header.version.year,
                month: image.header.version.month,
                week: image.header.version.week,
            },
            module_hashes: image.image_metadata.module_hashes.clone(),
            entry_function,
            initializers,
            finalizers,
            gpu_capabilities: image
                .image_metadata
                .gpu_capability_mask()
                .ok_or_else(|| FormatError::malformed(None, "unknown IMAGE GPU capability"))?,
            minimum_driver_api: image.image_metadata.minimum_driver_api,
        })
    }

    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let mut bytes = Vec::new();
        write_string_index(&self.name, strings, "image name", &mut bytes)?;
        bytes.push(image_kind_code(self.kind));
        bytes.extend_from_slice(&self.package_digest);
        write_string_index(
            &self.tirc_build,
            strings,
            "tirc build identifier",
            &mut bytes,
        )?;
        bytes.extend_from_slice(&[
            self.tir_version.year,
            self.tir_version.month,
            self.tir_version.week,
        ]);
        write_count(self.module_hashes.len(), &mut bytes)?;
        for hash in &self.module_hashes {
            bytes.extend_from_slice(hash);
        }
        let encoded_entry = match self.entry_function {
            Some(index) => index
                .checked_add(1)
                .ok_or_else(|| malformed(bytes.len(), "IMAGE entry function index overflows"))?,
            None => 0,
        };
        write_uleb(encoded_entry, &mut bytes)
            .map_err(|error| malformed(bytes.len(), error.to_string()))?;
        encode_functions(&self.initializers, &mut bytes)?;
        encode_functions(&self.finalizers, &mut bytes)?;
        bytes.extend_from_slice(&self.gpu_capabilities.to_le_bytes());
        if let Some((major, minor)) = self.minimum_driver_api {
            bytes.extend_from_slice(&[major, minor]);
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8], strings: &StringTable) -> Result<Self, FormatError> {
        let mut cursor = 0;
        let name = read_string_index(bytes, &mut cursor, strings, "image name")?;
        let kind = read_image_kind(bytes, &mut cursor)?;
        let package_digest = read_array::<32>(bytes, &mut cursor, "package digest")?;
        let tirc_build = read_string_index(bytes, &mut cursor, strings, "tirc build identifier")?;
        let tir_version = TirVersion {
            year: read_byte(bytes, &mut cursor, "TIR version year")?,
            month: read_byte(bytes, &mut cursor, "TIR version month")?,
            week: read_byte(bytes, &mut cursor, "TIR version week")?,
        };
        let hash_count = read_count(bytes, &mut cursor, "module hash")?;
        let mut module_hashes = reserve(hash_count, bytes, cursor)?;
        for _ in 0..hash_count {
            module_hashes.push(read_array::<32>(bytes, &mut cursor, "module hash")?);
        }
        let encoded_entry = read_index(bytes, &mut cursor, "entry function")?;
        let entry_function = encoded_entry.checked_sub(1);
        let initializers = read_functions(bytes, &mut cursor, "initializer")?;
        let finalizers = read_functions(bytes, &mut cursor, "finalizer")?;
        let gpu_capabilities =
            u64::from_le_bytes(read_array::<8>(bytes, &mut cursor, "GPU capability mask")?);
        let minimum_driver_api = if kind == ImageKind::Driver {
            Some((
                read_byte(bytes, &mut cursor, "minimum Driver API major version")?,
                read_byte(bytes, &mut cursor, "minimum Driver API minor version")?,
            ))
        } else {
            None
        };
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in IMAGE section"));
        }
        let record = Self {
            name,
            kind,
            package_digest,
            tirc_build,
            tir_version,
            module_hashes,
            entry_function,
            initializers,
            finalizers,
            gpu_capabilities,
            minimum_driver_api,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.name.is_empty() || self.tirc_build.is_empty() {
            return Err(malformed(
                0,
                "IMAGE name and tirc build identifier must be nonempty",
            ));
        }
        if !(1..=99).contains(&self.tir_version.year)
            || !(1..=12).contains(&self.tir_version.month)
            || !(1..=5).contains(&self.tir_version.week)
        {
            return Err(malformed(0, "invalid IMAGE TIR version"));
        }
        if self.module_hashes.is_empty()
            || self.module_hashes.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(FormatError::noncanonical(
                None,
                "IMAGE module hashes must be unique, sorted and nonempty",
            ));
        }
        if (self.kind == ImageKind::Executable) != self.entry_function.is_some() {
            return Err(malformed(
                0,
                "only executable IMAGE records have an entry function",
            ));
        }
        if (self.kind == ImageKind::Driver) != self.minimum_driver_api.is_some() {
            return Err(malformed(
                0,
                "minimum Driver API version is present only for driver images",
            ));
        }
        if self
            .minimum_driver_api
            .is_some_and(|(major, minor)| !(1..=99).contains(&major) || !(1..=12).contains(&minor))
        {
            return Err(malformed(0, "invalid minimum Driver API version"));
        }
        if self.gpu_capabilities >> 19 != 0 {
            return Err(malformed(0, "IMAGE GPU capability mask has unknown bits"));
        }
        if self
            .entry_function
            .into_iter()
            .chain(self.initializers.iter().map(|item| item.function_index))
            .chain(self.finalizers.iter().map(|item| item.function_index))
            .any(|index| index >= 1 << 28)
        {
            return Err(malformed(0, "IMAGE function index exceeds the image limit"));
        }
        validate_functions(&self.initializers, "initializer")?;
        validate_functions(&self.finalizers, "finalizer")?;
        Ok(())
    }
}

fn resolve_image_functions(
    functions: &[crate::text::TextImageFunction],
    mut resolve_function: impl FnMut(&str) -> Option<u64>,
) -> Result<Vec<ImageFunctionRecord>, FormatError> {
    let mut records = Vec::new();
    records
        .try_reserve_exact(functions.len())
        .map_err(|_| FormatError::malformed(None, "IMAGE function allocation failed"))?;
    for function in functions {
        let function_index = resolve_function(
            function
                .function
                .strip_prefix('@')
                .unwrap_or(&function.function),
        )
        .ok_or_else(|| FormatError::malformed(None, "IMAGE function is unresolved"))?;
        records.push(ImageFunctionRecord {
            function_index,
            priority: function.priority,
        });
    }
    Ok(records)
}

impl TvisaImage {
    pub fn image_record(&self, memory_limit: u64) -> Result<ImageRecord, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == IMAGE_SECTION)
            .ok_or_else(|| malformed(0, "IMAGE section is missing"))?;
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let record = ImageRecord::decode(&bytes, &strings)?;
        if record.tir_version
            != (TirVersion {
                year: self.header.year,
                month: self.header.month,
                week: self.header.week,
            })
        {
            return Err(malformed(
                0,
                "IMAGE TIR version does not match the TVISA image version",
            ));
        }
        let profile_matches = matches!(
            (record.kind, self.header.profile),
            (
                ImageKind::Executable | ImageKind::Framework | ImageKind::Plugin,
                tir_target::Profile::App | tir_target::Profile::System
            ) | (ImageKind::Driver, tir_target::Profile::Driver)
                | (ImageKind::Kernel, tir_target::Profile::Kernel)
        );
        if !profile_matches {
            return Err(malformed(0, "IMAGE kind does not match the TVISA profile"));
        }
        let imports = self.import_counts(memory_limit)?;
        let defined_start = imports.functions;
        let defined_end = defined_start
            .checked_add(u64::from(self.header.function_count))
            .ok_or_else(|| malformed(0, "IMAGE function count overflows"))?;
        let functions = self.function_records(memory_limit)?;
        for function in record
            .entry_function
            .into_iter()
            .chain(record.initializers.iter().map(|item| item.function_index))
            .chain(record.finalizers.iter().map(|item| item.function_index))
        {
            if function < defined_start || function >= defined_end {
                return Err(malformed(
                    0,
                    "IMAGE entry, initializer or finalizer is not a defined function",
                ));
            }
            let defined_index = usize::try_from(function - defined_start)
                .map_err(|_| malformed(0, "IMAGE function index does not fit in memory"))?;
            if !matches!(functions[defined_index].target, 0 | 2) {
                return Err(malformed(
                    0,
                    "IMAGE entry, initializer or finalizer must target the CPU",
                ));
            }
        }
        let gpu_capabilities = if self.sections.iter().any(|section| section.kind == 11) {
            self.gpu_entry_records(memory_limit)?
                .iter()
                .fold(0, |union, entry| union | entry.capabilities)
        } else {
            0
        };
        if gpu_capabilities != record.gpu_capabilities {
            return Err(malformed(
                0,
                "IMAGE GPU capability mask does not match the GPU entry union",
            ));
        }
        let canonical = record.encode(&strings)?;
        if canonical != bytes {
            return Err(FormatError::noncanonical(
                Some(0),
                "IMAGE section is not canonically encoded",
            ));
        }
        Ok(record)
    }
}

fn image_kind_code(kind: ImageKind) -> u8 {
    match kind {
        ImageKind::Executable => 0,
        ImageKind::Framework => 1,
        ImageKind::Plugin => 2,
        ImageKind::Driver => 3,
        ImageKind::Kernel => 4,
    }
}

fn read_image_kind(bytes: &[u8], cursor: &mut usize) -> Result<ImageKind, FormatError> {
    let offset = *cursor;
    match read_byte(bytes, cursor, "image kind")? {
        0 => Ok(ImageKind::Executable),
        1 => Ok(ImageKind::Framework),
        2 => Ok(ImageKind::Plugin),
        3 => Ok(ImageKind::Driver),
        4 => Ok(ImageKind::Kernel),
        _ => Err(malformed(offset, "invalid IMAGE kind")),
    }
}

fn encode_functions(
    functions: &[ImageFunctionRecord],
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(functions.len(), bytes)?;
    for function in functions {
        write_index(
            function.function_index,
            "initializer/finalizer function index",
            bytes,
        )?;
        bytes.extend_from_slice(&function.priority.to_le_bytes());
    }
    Ok(())
}

fn read_functions(
    bytes: &[u8],
    cursor: &mut usize,
    label: &str,
) -> Result<Vec<ImageFunctionRecord>, FormatError> {
    let count = read_count(bytes, cursor, label)?;
    let mut functions = reserve(count, bytes, *cursor)?;
    for _ in 0..count {
        let function_index = read_index(bytes, cursor, "initializer/finalizer function index")?;
        let priority = u16::from_le_bytes(read_array::<2>(bytes, cursor, "function priority")?);
        functions.push(ImageFunctionRecord {
            function_index,
            priority,
        });
    }
    Ok(functions)
}

fn validate_functions(functions: &[ImageFunctionRecord], label: &str) -> Result<(), FormatError> {
    if functions.windows(2).any(|pair| {
        (pair[0].priority, pair[0].function_index) >= (pair[1].priority, pair[1].function_index)
    }) {
        return Err(FormatError::noncanonical(
            None,
            format!("IMAGE {label} records are not strictly ordered"),
        ));
    }
    Ok(())
}

fn write_string_index(
    value: &str,
    strings: &StringTable,
    label: &str,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(bytes.len(), format!("IMAGE {label} is empty")));
    }
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed(bytes.len(), format!("IMAGE {label} is absent from STRINGS")))?;
    write_index(
        u64::try_from(index).map_err(|_| malformed(bytes.len(), "IMAGE string index overflows"))?,
        label,
        bytes,
    )
}

fn read_string_index(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let index = read_index(bytes, cursor, label)?;
    usize::try_from(index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| malformed(*cursor, format!("IMAGE {label} is not a nonempty string")))
}

fn read_array<const N: usize>(
    bytes: &[u8],
    cursor: &mut usize,
    label: &str,
) -> Result<[u8; N], FormatError> {
    let end = cursor
        .checked_add(N)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(*cursor, format!("truncated IMAGE {label}")))?;
    let mut result = [0; N];
    result.copy_from_slice(&bytes[*cursor..end]);
    *cursor = end;
    Ok(result)
}

fn read_byte(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u8, FormatError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, format!("truncated IMAGE {label}")))?;
    *cursor += 1;
    Ok(value)
}

fn read_count(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<usize, FormatError> {
    let count = read_index(bytes, cursor, &format!("{label} count"))?;
    if count > bytes.len() as u64 {
        return Err(malformed(
            *cursor,
            format!("IMAGE {label} count exceeds the section size"),
        ));
    }
    usize::try_from(count).map_err(|_| malformed(*cursor, format!("IMAGE {label} count overflows")))
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset.min(bytes.len()), "IMAGE record allocation failed"))?;
    Ok(values)
}

fn write_count(count: usize, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_index(
        u64::try_from(count).map_err(|_| malformed(bytes.len(), "IMAGE count overflows"))?,
        "count",
        bytes,
    )
}

fn write_index(index: u64, label: &str, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes)
        .map_err(|error| malformed(bytes.len(), format!("IMAGE {label}: {error}")))
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid IMAGE {label}: {error:?}")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
