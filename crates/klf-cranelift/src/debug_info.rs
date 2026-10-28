//! The DWARF of a program object (section 16.3): one compile unit whose line
//! program maps each function's machine code to the source its KIR came
//! from, and a subprogram entry for each function under its source name, so
//! stock LLDB and a trap's backtrace name every Kira frame with its file and
//! line.
//!
//! Each instruction's `debug_location` becomes a Cranelift source location,
//! an index into the function's own list of places; the code's source
//! location ranges then give the rows of its line sequence. Mach-O keeps the
//! DWARF in the object for `dsymutil` to gather, which maps the object's own
//! addresses through the binary's debug map, so addresses and offsets are
//! written as they are; ELF links the sections of several objects together,
//! so there they are relocated against the function and the section.

use crate::{EmitError, err};
use cranelift::codegen::CompiledCode;
use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use gimli::write::{
    Address, AttributeValue, DwarfUnit, LineProgram, LineString, Range, RangeList,
    Sections, Writer,
};
use gimli::{SectionId, constants};
use object::write::{Relocation as ObjectRelocation, SymbolId};
use object::{BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind, SectionKind};
use std::collections::HashMap;

/// KLF's vendor language code for Kira (section 16.3).
const KIRA_LANGUAGE: u16 = 0x8e57;

/// One row of a function's line sequence: from `offset` in its code on, the
/// instructions come from `file` at `line` and `column`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Row {
    pub(crate) offset: u32,
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

/// The rows of `code`, whose source locations index `places`.
pub(crate) fn rows(code: &CompiledCode, places: &[klf_kir::DebugLocation]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for range in code.buffer.get_srclocs_sorted() {
        let location = range.loc;
        if location.is_default() {
            continue;
        }
        let Some(place) = places.get(location.bits() as usize) else {
            continue;
        };
        if rows.last().is_some_and(|row| row.file == place.file && row.line == place.line) {
            continue;
        }
        // The prologue, which no instruction's place covers, is the first
        // place's: a debugger stopped at the function's first instruction
        // shows where the function starts.
        let offset = if rows.is_empty() { 0 } else { range.start };
        rows.push(Row {
            offset,
            file: place.file.clone(),
            line: place.line,
            column: place.column,
        });
    }
    rows
}

struct Function {
    id: FuncId,
    name: String,
    size: u64,
    rows: Vec<Row>,
}

/// The debug information of one object, written once every function is
/// defined.
#[derive(Default)]
pub(crate) struct DebugTables {
    functions: Vec<Function>,
    /// What each generated name's suffix stands for, from the module's
    /// `klf.debug.name` attributes.
    generated: HashMap<String, String>,
}

impl DebugTables {
    /// The tables of `module`'s functions.
    pub(crate) fn of(module: &klf_kir::Module) -> Self {
        let generated = module
            .attributes
            .iter()
            .filter(|attribute| attribute.name == "klf.debug.name")
            .filter_map(|attribute| attribute.value.as_deref()?.split_once('\t'))
            .map(|(suffix, shown)| (suffix.to_owned(), shown.to_owned()))
            .collect();
        Self {
            functions: Vec::new(),
            generated,
        }
    }

    /// Function `id`, named `kir_name` in its module, `size` bytes long.
    pub(crate) fn add(&mut self, id: FuncId, kir_name: &str, size: usize, rows: Vec<Row>) {
        let name = self.source_name(kir_name);
        self.functions.push(Function {
            id,
            name,
            size: size as u64,
            rows,
        });
    }

    /// `kir_name` as the source names it: a generated function by what it
    /// is generated for, `destroy of Array<Selection>`.
    fn source_name(&self, kir_name: &str) -> String {
        let generated = kir_name
            .rsplit_once('#')
            .and_then(|(_, suffix)| self.generated.get(suffix));
        match generated {
            Some(shown) => klf_diag::names::shown_type_text(shown),
            None => klf_diag::names::local_source_name(kir_name),
        }
    }

    /// Writes the DWARF sections into `product`.
    pub(crate) fn write(self, product: &mut ObjectProduct, unit_name: &str) -> Result<(), EmitError> {
        let format = product.object.format();
        if self.functions.is_empty() || !matches!(format, BinaryFormat::Elf | BinaryFormat::MachO) {
            return Ok(());
        }
        let symbols: Vec<SymbolId> =
            self.functions.iter().map(|function| product.function_symbol(function.id)).collect();
        // Mach-O DWARF names a function by its address in the object, which
        // `dsymutil` maps through the binary's debug map. The debug sections
        // follow every other, so the object written without them places the
        // functions where it places them with them.
        let placed = match format {
            BinaryFormat::MachO => Some(std::sync::Arc::new(object_addresses(product, &symbols)?)),
            _ => None,
        };
        let mut dwarf = unit(&self.functions, unit_name, placed.as_deref().map(Vec::as_slice));
        let mut sections = Sections::new(DebugSection {
            placed,
            ..DebugSection::default()
        });
        dwarf
            .write(&mut sections)
            .map_err(|e| err(format!("debug information: {e}")))?;
        emit(product, &sections, &symbols, placed_addresses(&sections))
    }
}

/// Where the object places each of `symbols`: its address there.
fn object_addresses(product: &ObjectProduct, symbols: &[SymbolId]) -> Result<Vec<u64>, EmitError> {
    use object::{Object, ObjectSymbol};
    let bytes = product
        .object
        .write()
        .map_err(|e| err(format!("debug information: {e}")))?;
    let file = object::read::macho::MachOFile64::<object::Endianness>::parse(bytes.as_slice())
        .map_err(|e| err(format!("debug information: {e}")))?;
    let placed: HashMap<&[u8], u64> = file
        .symbols()
        .filter_map(|symbol| {
            let name = symbol.name_bytes().ok()?;
            Some((name.strip_prefix(b"_").unwrap_or(name), symbol.address()))
        })
        .collect();
    Ok(symbols
        .iter()
        .map(|symbol| {
            let name = product.object.symbol(*symbol).name.as_slice();
            placed.get(name.strip_prefix(b"_").unwrap_or(name)).copied().unwrap_or(0)
        })
        .collect())
}

/// The compile unit, its line program and a subprogram per function.
/// Where the code lies is one range on Mach-O, from the first function the
/// object places to the end of the last, as `placed` gives them; ELF lists
/// each function's range.
fn unit(functions: &[Function], unit_name: &str, placed: Option<&[u64]>) -> DwarfUnit {
    let encoding = gimli::Encoding {
        format: gimli::Format::Dwarf32,
        version: 5,
        address_size: 8,
    };
    let mut dwarf = DwarfUnit::new(encoding);
    let directory = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let working_dir = LineString::new(directory.as_bytes(), encoding, &mut dwarf.line_strings);
    let source_file = LineString::new(unit_name.as_bytes(), encoding, &mut dwarf.line_strings);
    let mut program = LineProgram::new(
        encoding,
        gimli::LineEncoding::default(),
        working_dir,
        None,
        source_file,
        None,
    );
    let mut files: HashMap<&str, gimli::write::FileId> = HashMap::new();
    let mut ranges = Vec::new();
    let root = dwarf.unit.root();
    let mut entries = Vec::new();
    for (position, function) in functions.iter().enumerate() {
        let start = Address::Symbol {
            symbol: position,
            addend: 0,
        };
        ranges.push(Range::StartLength {
            begin: start,
            length: function.size,
        });
        // A function no source place covers, such as a generated copy, has
        // its name and its range, and no line sequence.
        let Some(first) = function.rows.first() else {
            entries.push((position, function, None));
            continue;
        };
        program.begin_sequence(Some(start));
        for row in &function.rows {
            let file = *files.entry(row.file.as_str()).or_insert_with(|| {
                let directory = program.default_directory();
                let name = LineString::new(row.file.as_bytes(), encoding, &mut dwarf.line_strings);
                program.add_file(name, directory, None)
            });
            let line = program.row();
            line.address_offset = u64::from(row.offset);
            line.file = file;
            line.line = u64::from(row.line);
            line.column = u64::from(row.column);
            program.generate_row();
        }
        program.end_sequence(function.size);
        entries.push((position, function, Some((files[first.file.as_str()], first.line))));
    }
    let covered = placed.and_then(|placed| {
        let first = (0..functions.len()).min_by_key(|position| placed[*position])?;
        let end = (0..functions.len()).map(|position| placed[position] + functions[position].size).max()?;
        Some((first, end - placed[first]))
    });
    let ranges = dwarf.unit.ranges.add(RangeList(ranges));
    let name = dwarf.strings.add(unit_name);
    let producer = dwarf.strings.add("KLF Kira");
    let unit_entry = dwarf.unit.get_mut(root);
    unit_entry.set(constants::DW_AT_producer, AttributeValue::StringRef(producer));
    unit_entry.set(constants::DW_AT_language, AttributeValue::Language(constants::DwLang(KIRA_LANGUAGE)));
    unit_entry.set(constants::DW_AT_name, AttributeValue::StringRef(name));
    match covered {
        Some((first, length)) => {
            unit_entry.set(
                constants::DW_AT_low_pc,
                AttributeValue::Address(Address::Symbol {
                    symbol: first,
                    addend: 0,
                }),
            );
            unit_entry.set(constants::DW_AT_high_pc, AttributeValue::Udata(length));
        }
        None => {
            unit_entry.set(constants::DW_AT_low_pc, AttributeValue::Address(Address::Constant(0)));
            unit_entry.set(constants::DW_AT_ranges, AttributeValue::RangeListRef(ranges));
        }
    }
    for (position, function, declared) in entries {
        let entry = dwarf.unit.add(root, constants::DW_TAG_subprogram);
        let name = dwarf.strings.add(function.name.as_str());
        let subprogram = dwarf.unit.get_mut(entry);
        subprogram.set(constants::DW_AT_name, AttributeValue::StringRef(name));
        subprogram.set(
            constants::DW_AT_low_pc,
            AttributeValue::Address(Address::Symbol {
                symbol: position,
                addend: 0,
            }),
        );
        subprogram.set(constants::DW_AT_high_pc, AttributeValue::Udata(function.size));
        if let Some((file, line)) = declared {
            subprogram.set(constants::DW_AT_decl_file, AttributeValue::FileIndex(Some(file)));
            subprogram.set(constants::DW_AT_decl_line, AttributeValue::Udata(u64::from(line)));
        }
        subprogram.set(constants::DW_AT_external, AttributeValue::Flag(true));
    }
    dwarf.unit.line_program = program;
    dwarf
}

/// A debug section's bytes and the places in it that name a function's
/// address or another debug section's offset.
#[derive(Clone, Default)]
struct DebugSection {
    /// On Mach-O, each function's address in the object, written in place
    /// of a relocation.
    placed: Option<std::sync::Arc<Vec<u64>>>,
    bytes: Vec<u8>,
    addresses: Vec<(usize, usize, u8, i64)>,
    offsets: Vec<(usize, SectionId, u8, i64)>,
}

impl Writer for DebugSection {
    type Endian = gimli::LittleEndian;

    fn endian(&self) -> Self::Endian {
        gimli::LittleEndian
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn write(&mut self, bytes: &[u8]) -> gimli::write::Result<()> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn write_at(&mut self, offset: usize, bytes: &[u8]) -> gimli::write::Result<()> {
        let end = offset + bytes.len();
        if end > self.bytes.len() {
            return Err(gimli::write::Error::OffsetOutOfBounds);
        }
        self.bytes[offset..end].copy_from_slice(bytes);
        Ok(())
    }

    fn write_address(&mut self, address: Address, size: u8) -> gimli::write::Result<()> {
        match address {
            Address::Constant(value) => self.write_udata(value, size),
            Address::Symbol { symbol, addend } => {
                self.addresses.push((self.bytes.len(), symbol, size, addend));
                let placed = self.placed.as_ref().and_then(|placed| placed.get(symbol).copied());
                self.write_udata(placed.unwrap_or(0).wrapping_add(addend as u64), size)
            }
        }
    }

    fn write_offset(&mut self, value: usize, section: SectionId, size: u8) -> gimli::write::Result<()> {
        self.offsets.push((self.bytes.len(), section, size, value as i64));
        self.write_udata(value as u64, size)
    }

    fn write_offset_at(
        &mut self,
        offset: usize,
        value: usize,
        section: SectionId,
        size: u8,
    ) -> gimli::write::Result<()> {
        self.offsets.push((offset, section, size, value as i64));
        self.write_udata_at(offset, value as u64, size)
    }
}

/// Adds the sections and their relocations to `product`.
/// The Mach-O object addresses the sections were written against, none on
/// ELF.
fn placed_addresses(sections: &Sections<DebugSection>) -> Option<std::sync::Arc<Vec<u64>>> {
    sections.debug_info.0.placed.clone()
}

fn emit(
    product: &mut ObjectProduct,
    sections: &Sections<DebugSection>,
    symbols: &[SymbolId],
    placed: Option<std::sync::Arc<Vec<u64>>>,
) -> Result<(), EmitError> {
    let format = product.object.format();
    let mut ids: HashMap<SectionId, object::write::SectionId> = HashMap::new();
    sections
        .for_each(|id, section| {
            if section.bytes.is_empty() {
                return Ok::<(), EmitError>(());
            }
            let (segment, name) = match format {
                BinaryFormat::MachO => (b"__DWARF".to_vec(), id.name().replacen('.', "__", 1).into_bytes()),
                _ => (Vec::new(), id.name().as_bytes().to_vec()),
            };
            let added = product.object.add_section(segment, name, SectionKind::Debug);
            product.object.append_section_data(added, &section.bytes, 1);
            ids.insert(id, added);
            Ok(())
        })?;
    let absolute = |size: u8| RelocationFlags::Generic {
        kind: RelocationKind::Absolute,
        encoding: RelocationEncoding::Generic,
        size: size * 8,
    };
    sections.for_each(|id, section| {
        let Some(&target) = ids.get(&id) else {
            return Ok::<(), EmitError>(());
        };
        for &(offset, symbol, size, addend) in &section.addresses {
            // Mach-O relocates a debug address against the section the code
            // is in, as an assembler writes it, with the object address in
            // place: `dsymutil` keeps an entry only when a relocation marks
            // its address.
            let (symbol, addend) = match &placed {
                Some(placed) => {
                    let object::write::SymbolSection::Section(code) =
                        product.object.symbol(symbols[symbol]).section
                    else {
                        continue;
                    };
                    let address = placed.get(symbol).copied().unwrap_or(0);
                    (product.object.section_symbol(code), address as i64 + addend)
                }
                None => (symbols[symbol], addend),
            };
            product
                .object
                .add_relocation(
                    target,
                    ObjectRelocation {
                        offset: offset as u64,
                        symbol,
                        addend,
                        flags: absolute(size),
                    },
                )
                .map_err(|e| err(format!("debug relocation: {e}")))?;
        }
        if format == BinaryFormat::Elf {
            for &(offset, other, size, addend) in &section.offsets {
                let Some(&other) = ids.get(&other) else {
                    continue;
                };
                let symbol = product.object.section_symbol(other);
                product
                    .object
                    .add_relocation(
                        target,
                        ObjectRelocation {
                            offset: offset as u64,
                            symbol,
                            addend,
                            flags: absolute(size),
                        },
                    )
                    .map_err(|e| err(format!("debug relocation: {e}")))?;
            }
        }
        Ok(())
    })
}
