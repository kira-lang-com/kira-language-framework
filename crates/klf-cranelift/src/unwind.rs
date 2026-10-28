//! The unwind tables of a native frontend object (`KLF-EDITOR.md` section
//! 8.6): an `.eh_frame` entry for every function, so a language server can
//! unwind through definition code, and for each function with cleanup pads a
//! landing table that `klfrt_personality` reads. On Mach-O the same tables go
//! in `__TEXT,__eh_frame` and `__TEXT,__gcc_except_tab`; the linker points
//! `__unwind_info` at the frame entries.
//!
//! The landing table is the runtime's own format, not the C++ one: a 32-bit
//! count, then pairs of 32-bit offsets from the function's start, a call's
//! return address and its pad, sorted by return address.

use crate::{EmitError, err};
use cranelift::codegen::control::ControlPlane;
use cranelift::codegen::isa::TargetIsa;
use cranelift::codegen::isa::unwind::UnwindInfo;
use cranelift::codegen::{CompiledCode, Context, FinalizedMachExceptionHandler};
use cranelift_module::{FuncId, Module, ModuleReloc};
use cranelift_object::{ObjectModule, ObjectProduct};
use gimli::constants::{DW_EH_PE_indirect, DW_EH_PE_pcrel, DW_EH_PE_sdata4, DwEhPe};
use gimli::write::{
    Address, EhFrame, EndianVec, FrameTable, RelocateWriter, Relocation, RelocationTarget,
};
use object::write::{Relocation as ObjectRelocation, StandardSection, Symbol, SymbolSection};
use object::{
    BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind, SectionKind, SymbolFlags,
    SymbolKind, SymbolScope,
};

/// What one function contributes: its call frame information and its
/// landing table, empty when no call in it has a pad.
struct Entry {
    id: FuncId,
    info: UnwindInfo,
    landing: Vec<(u32, u32)>,
}

/// The tables of one object, written once every function is defined.
#[derive(Default)]
pub(crate) struct UnwindTables {
    entries: Vec<Entry>,
}

/// Whether objects for `isa` get unwind tables: ELF and Mach-O, whose
/// `.eh_frame` the system unwinder reads with the encodings written here.
pub(crate) fn supported(isa: &dyn TargetIsa) -> bool {
    matches!(
        isa.triple().binary_format,
        target_lexicon::BinaryFormat::Elf | target_lexicon::BinaryFormat::Macho
    )
}

/// The pads of `code`: each call's return address with the cleanup it
/// lands in when an unwind passes through it.
pub(crate) fn landing(code: &CompiledCode) -> Vec<(u32, u32)> {
    let mut landing: Vec<(u32, u32)> = code
        .buffer
        .call_sites()
        .filter_map(|site| {
            site.exception_handlers
                .iter()
                .find_map(|handler| match handler {
                    FinalizedMachExceptionHandler::Default(pad) => Some((site.ret_addr, *pad)),
                    _ => None,
                })
        })
        .collect();
    landing.sort_unstable();
    landing
}

impl UnwindTables {
    pub(crate) fn add(&mut self, id: FuncId, info: Option<UnwindInfo>, landing: Vec<(u32, u32)>) {
        if let Some(info @ UnwindInfo::SystemV(_)) = info {
            self.entries.push(Entry { id, info, landing });
        }
    }

    /// Compiles `context` as `id` and records its tables, where
    /// `ObjectModule::define_function` would record none.
    pub(crate) fn define(
        &mut self,
        object: &mut ObjectModule,
        id: FuncId,
        context: &mut Context,
    ) -> Result<(), EmitError> {
        let isa = object.isa();
        context
            .compile(isa, &mut ControlPlane::default())
            .map_err(|e| err(format!("compile: {}", e.inner)))?;
        let code = context
            .compiled_code()
            .ok_or_else(|| err("no compiled code"))?;
        let info = code
            .create_unwind_info(isa)
            .map_err(|e| err(format!("unwind info: {e}")))?;
        let landing = landing(code);
        let alignment = u64::from(code.buffer.alignment);
        let bytes = code.buffer.data().to_vec();
        let relocs: Vec<ModuleReloc> = code
            .buffer
            .relocs()
            .iter()
            .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, id))
            .collect();
        object
            .define_function_bytes(id, alignment, &bytes, &relocs)
            .map_err(|e| err(format!("define: {e}")))?;
        self.add(id, info, landing);
        Ok(())
    }

    /// Writes `.eh_frame` and the landing tables into `product`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write(
        self,
        product: &mut ObjectProduct,
        isa: &dyn TargetIsa,
    ) -> Result<(), EmitError> {
        let format = product.object.format();
        if self.entries.is_empty() || !matches!(format, BinaryFormat::Elf | BinaryFormat::MachO) {
            return Ok(());
        }
        // Mach-O linkers take the 8-byte PC-relative form the assembler
        // writes there, and read the personality only through a GOT entry.
        let pcrel = DwEhPe(DW_EH_PE_pcrel.0 | DW_EH_PE_sdata4.0);
        let (encoding, personality_encoding) = match format {
            BinaryFormat::MachO => (DW_EH_PE_pcrel, DwEhPe(DW_EH_PE_indirect.0 | pcrel.0)),
            _ => (pcrel, pcrel),
        };
        let personality = product.object.add_symbol(Symbol {
            name: b"klfrt_personality".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Text,
            scope: SymbolScope::Unknown,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        let (segment, name): (&[u8], &[u8]) = match format {
            BinaryFormat::MachO => (b"__TEXT", b"__gcc_except_tab"),
            _ => (b"", b".gcc_except_table"),
        };
        let tables =
            product
                .object
                .add_section(segment.to_vec(), name.to_vec(), SectionKind::ReadOnlyData);
        // A Mach-O linker reads an LSDA pointer only against a symbol, not
        // against the bare section a section symbol becomes there.
        let tables_symbol = match format {
            BinaryFormat::MachO => product.object.add_symbol(Symbol {
                name: b"klf_landing_tables".to_vec(),
                value: 0,
                size: 0,
                kind: SymbolKind::Data,
                scope: SymbolScope::Compilation,
                weak: false,
                section: SymbolSection::Section(tables),
                flags: SymbolFlags::None,
            }),
            _ => product.object.section_symbol(tables),
        };
        let mut symbols = vec![personality, tables_symbol];

        let mut frames = FrameTable::default();
        let mut plain = isa
            .create_systemv_cie()
            .ok_or_else(|| err("the target has no System V unwind information"))?;
        plain.fde_address_encoding = encoding;
        let mut landing = plain.clone();
        landing.personality = Some((
            personality_encoding,
            Address::Symbol {
                symbol: 0,
                addend: 0,
            },
        ));
        landing.lsda_encoding = Some(encoding);
        let plain = frames.add_cie(plain);
        let landing_cie = frames.add_cie(landing);

        for entry in self.entries {
            let UnwindInfo::SystemV(info) = entry.info else {
                continue;
            };
            let function = product.function_symbol(entry.id);
            // On ELF a function exported from a shared object may be
            // preempted, so a linker refuses a PC-relative reference to its
            // symbol from data; the frame entry names the code section's
            // local symbol at the function's offset instead, as an
            // assembler writes it. Mach-O reads its own symbols fine.
            let (symbol, addend) = match (format, product.object.symbol(function).section) {
                (BinaryFormat::MachO, _) | (_, SymbolSection::Undefined) => (function, 0),
                (_, SymbolSection::Section(section)) => {
                    let value = product.object.symbol(function).value;
                    (
                        product.object.section_symbol(section),
                        i64::try_from(value).unwrap_or(0),
                    )
                }
                _ => (function, 0),
            };
            symbols.push(symbol);
            let address = Address::Symbol {
                symbol: symbols.len() - 1,
                addend,
            };
            let mut fde = info.to_fde(address);
            if entry.landing.is_empty() {
                frames.add_fde(plain, fde);
                continue;
            }
            let mut table = Vec::with_capacity(4 + entry.landing.len() * 8);
            table.extend_from_slice(
                &u32::try_from(entry.landing.len())
                    .unwrap_or(0)
                    .to_le_bytes(),
            );
            for (ret, pad) in &entry.landing {
                table.extend_from_slice(&ret.to_le_bytes());
                table.extend_from_slice(&pad.to_le_bytes());
            }
            let offset = product.object.append_section_data(tables, &table, 4);
            fde.lsda = Some(Address::Symbol {
                symbol: 1,
                addend: i64::try_from(offset).unwrap_or(0),
            });
            frames.add_fde(landing_cie, fde);
        }

        let mut eh_frame = EhFrame(Section {
            writer: EndianVec::new(gimli::RunTimeEndian::Little),
            relocations: Vec::new(),
        });
        frames
            .write_eh_frame(&mut eh_frame)
            .map_err(|e| err(format!(".eh_frame: {e}")))?;
        let EhFrame(section) = eh_frame;
        let eh_frame = product.object.section_id(StandardSection::EhFrame);
        let base = product
            .object
            .append_section_data(eh_frame, section.writer.slice(), 8);
        let x86_64_macho = format == BinaryFormat::MachO
            && isa.triple().architecture == target_lexicon::Architecture::X86_64;
        // x86_64 Mach-O has no PC-relative relocation for data: a pointer
        // is written as the target less the section's start, with the
        // field's offset taken from the addend, as the assembler writes it.
        let section_start = x86_64_macho.then(|| product.object.section_symbol(eh_frame));
        for relocation in section.relocations {
            let RelocationTarget::Symbol(symbol) = relocation.target else {
                continue;
            };
            let offset = base + relocation.offset as u64;
            let relative = relocation
                .eh_pe
                .is_some_and(|pe| pe.application() == DW_EH_PE_pcrel);
            let mut addend = relocation.addend;
            let mut subtractor = None;
            let flags = if relocation.eh_pe.is_some_and(DwEhPe::is_indirect) {
                RelocationFlags::MachO {
                    r_type: if x86_64_macho {
                        object::macho::X86_64_RELOC_GOT
                    } else {
                        object::macho::ARM64_RELOC_POINTER_TO_GOT
                    },
                    r_pcrel: true,
                    r_length: 2,
                }
            } else if let (true, Some(start)) = (relative, section_start) {
                subtractor = Some(start);
                addend -= i64::try_from(offset).unwrap_or(0);
                RelocationFlags::MachO {
                    r_type: object::macho::X86_64_RELOC_UNSIGNED,
                    r_pcrel: false,
                    r_length: if relocation.size == 8 { 3 } else { 2 },
                }
            } else {
                RelocationFlags::Generic {
                    kind: if relative {
                        RelocationKind::Relative
                    } else {
                        RelocationKind::Absolute
                    },
                    encoding: RelocationEncoding::Generic,
                    size: relocation.size * 8,
                }
            };
            product
                .object
                .add_relocation_with_subtractor(
                    eh_frame,
                    ObjectRelocation {
                        offset,
                        symbol: symbols[symbol],
                        addend,
                        flags,
                    },
                    subtractor,
                )
                .map_err(|e| err(format!(".eh_frame relocation: {e}")))?;
        }
        Ok(())
    }
}

struct Section {
    writer: EndianVec<gimli::RunTimeEndian>,
    relocations: Vec<Relocation>,
}

impl RelocateWriter for Section {
    type Writer = EndianVec<gimli::RunTimeEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}
