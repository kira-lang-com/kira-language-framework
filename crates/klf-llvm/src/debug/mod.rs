//! Debug information for the native backend (section 16.3).
//!
//! The backend emits DWARF 5 on ELF and Mach-O and `CodeView` on Windows, from
//! what KIR carries: a file table and line numbers from the `!loc` of each
//! instruction, variables from the `!var` of `debug.value` and `debug.addr`,
//! and the layouts of the declared types. Stock LLDB must show source lines,
//! step and print locals and aggregates from this alone, so every description
//! here is one stock LLDB formats on its own.

mod ditypes;
mod md5;

use klf_kir::{DebugLocation, Function, Module, Type};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

/// DWARF language code for KLF Kira: the vendor code `0x8e57`. Its types are
/// described only in DWARF C can express, which is what lets stock LLDB format
/// them.
const KIRA_LANGUAGE: u32 = 0x8e57;
/// `DW_LANG_C11`.
const C_LANGUAGE: u32 = 0x1d;
/// `DW_LANG_C_plus_plus`.
const KARC_LANGUAGE: u32 = 0x04;
const DWARF_VERSION: u32 = 5;
const DEBUG_INFO_VERSION: u32 = 3;
const CODEVIEW_FORMAT: u32 = 1;

/// The text of the virtual files of section 8.1, keyed by the path a `!loc`
/// names them by. It travels with the debug info a call emits so that a
/// debugger shows generated text it cannot read from disk.
#[derive(Clone, Default)]
pub struct DebugSources {
    pub virtual_sources: BTreeMap<String, String>,
}

/// One function's metadata: its subprogram, and the lexical blocks its `!loc`
/// files and `!var` scopes asked for.
#[derive(Clone)]
struct Subprogram {
    name: String,
    node: usize,
    file: String,
    declaration: usize,
    blocks: HashMap<String, usize>,
}

/// The debug info of one module under construction: metadata nodes numbered as
/// they are created, and the tables that let a second request for the same
/// file, type or subprogram reuse the first. Node `0` is the compile unit,
/// which every subprogram refers to.
pub(crate) struct DebugInfo<'m> {
    module: &'m Module,
    sources: &'m DebugSources,
    pointer_size: u32,
    codeview: bool,
    unit: usize,
    nodes: Vec<String>,
    files: HashMap<String, usize>,
    types: HashMap<Type, usize>,
    layouts: HashMap<String, usize>,
    typedefs: HashMap<String, usize>,
    subprograms: HashMap<String, Subprogram>,
    variables: HashMap<String, usize>,
}

impl<'m> DebugInfo<'m> {
    pub(crate) fn new(module: &'m Module, sources: &'m DebugSources) -> Self {
        let codeview = is_codeview(module.target.triple());
        let mut info = Self {
            module,
            sources,
            pointer_size: u32::from(module.target.pointer_width()),
            codeview,
            unit: 0,
            nodes: Vec::new(),
            files: HashMap::new(),
            types: HashMap::new(),
            layouts: HashMap::new(),
            typedefs: HashMap::new(),
            subprograms: HashMap::new(),
            variables: HashMap::new(),
        };
        info.unit = info.reserve();
        info
    }

    pub(crate) fn push(&mut self, text: &str) -> usize {
        self.nodes.push(text.to_owned());
        self.nodes.len() - 1
    }

    pub(crate) fn distinct(&mut self, text: &str) -> usize {
        self.push(&format!("distinct {text}"))
    }

    /// A node id whose text is filled in once its children exist, which is how
    /// a record that reaches itself terminates.
    pub(crate) fn reserve(&mut self) -> usize {
        self.push("!null")
    }

    pub(crate) fn fill(&mut self, id: usize, text: String) {
        self.nodes[id] = text;
    }

    /// A metadata list node over `elements`, each already written as text.
    pub(crate) fn list(&mut self, elements: &[String]) -> usize {
        self.push(&format!("!{{{}}}", elements.join(", ")))
    }

    /// The `DIFile` for `path`, split into the directory a debugger shows
    /// apart from the name it opens. A virtual file carries its text and its
    /// checksum, which is the DWARF 5 source extension.
    pub(crate) fn file(&mut self, path: &str) -> usize {
        if let Some(id) = self.files.get(path) {
            return *id;
        }
        let (directory, name) = match path.rsplit_once('/') {
            Some((directory, name)) => (directory.to_owned(), name.to_owned()),
            None => (".".to_owned(), path.to_owned()),
        };
        let text = match self.sources.virtual_sources.get(path) {
            Some(text) => format!(
                ", checksumkind: CSK_MD5, checksum: \"{}\", source: \"{}\"",
                md5::hex(text),
                escape(text)
            ),
            None => String::new(),
        };
        let id = self.push(&format!(
            "!DIFile(filename: \"{}\", directory: \"{}\"{text})",
            escape(&name),
            escape(&directory)
        ));
        self.files.insert(path.to_owned(), id);
        id
    }

    /// The `DISubprogram` of `function`, with its signature, or `None` when the
    /// function carries no `!loc` and so has no place in the line table.
    pub(crate) fn subprogram(&mut self, function: &Function) -> Option<usize> {
        let name = function.name.as_str();
        if let Some(entry) = self.subprograms.get(name) {
            return Some(entry.node);
        }
        let origin = function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .find_map(|instruction| instruction.debug_location.as_deref())?
            .clone();
        let file = self.file(&origin.file);
        let types = self.signature(function);
        let signature = self.push(&format!("!DISubroutineType(types: !{types})"));
        let unit = self.unit;
        let node = self.distinct(&format!(
            "!DISubprogram(name: \"{name}\", linkageName: \"kir.{name}\", scope: !{file}, file: !{file}, line: {}, type: !{signature}, unit: !{unit}, spFlags: DISPFlagDefinition)",
            origin.line
        ));
        let declaration = self.push(&format!(
            "!DILocation(line: {}, column: {}, scope: !{node})",
            origin.line, origin.column
        ));
        self.subprograms.insert(
            name.to_owned(),
            Subprogram {
                name: name.to_owned(),
                node,
                file: origin.file.clone(),
                declaration,
                blocks: HashMap::new(),
            },
        );
        Some(node)
    }

    /// The location a function's declaration-site metadata is anchored at: its
    /// first `!loc`, which is where its parameters are declared.
    pub(crate) fn declaration(&self, owner: &str) -> usize {
        self.subprograms
            .get(owner)
            .map_or(0, |entry| entry.declaration)
    }

    /// The result and parameter types of `function`. A `unit` or `never` result
    /// is the empty list, which is how DWARF spells `void`.
    fn signature(&mut self, function: &Function) -> usize {
        let result = match function.result {
            Type::Unit | Type::Never => None,
            ref ty => Some(self.di_type(ty)),
        };
        let parameters: Vec<String> = function
            .parameters
            .iter()
            .map(|parameter| format!("!{}", self.di_type(&parameter.ty)))
            .collect();
        let mut types: Vec<String> = result.iter().map(|id| format!("!{id}")).collect();
        types.extend(parameters);
        self.list(&types)
    }

    /// The `DILocalVariable` of `debug.value` or `debug.addr`. `owner` is the
    /// function the variable belongs to and `argument` its parameter index when
    /// it is one. A `!var` scope that names one of the owner's blocks becomes
    /// that block's `DILexicalBlock`.
    pub(crate) fn variable(
        &mut self,
        owner: &str,
        name: &str,
        ty: &Type,
        scope: &str,
        argument: Option<u32>,
        behind_address: bool,
    ) -> Option<usize> {
        let entry = self.subprograms.get(owner)?.clone();
        let key = format!("{owner}\u{1}{name}\u{1}{scope}\u{1}{argument:?}");
        if let Some(id) = self.variables.get(&key) {
            return Some(*id);
        }
        let ty = if behind_address {
            self.di_memory(ty)
        } else {
            self.di_type(ty)
        };
        let file = self.file(&entry.file);
        let scope = self.scope(owner, scope);
        let argument = argument.map_or_else(String::new, |index| format!(", arg: {index}"));
        let id = self.push(&format!(
            "!DILocalVariable(name: \"{}\", scope: !{scope}, file: !{file}, line: 0, type: !{ty}{argument})",
            escape(name)
        ));
        self.variables.insert(key, id);
        Some(id)
    }

    /// The `DILocalVariable` of a function parameter, whose name and line the
    /// parameter itself carries.
    pub(crate) fn parameter(
        &mut self,
        owner: &str,
        name: &str,
        ty: &Type,
        index: u32,
    ) -> Option<usize> {
        self.variable(owner, name, ty, "", Some(index), false)
    }

    /// The `DILocalVariable` of a value `debug.value` names.
    pub(crate) fn value_variable(
        &mut self,
        owner: &str,
        name: &str,
        ty: &Type,
        scope: &str,
    ) -> Option<usize> {
        self.variable(owner, name, ty, scope, None, false)
    }

    /// The `DILocalVariable` of the record an address `debug.addr` names.
    pub(crate) fn address_variable(
        &mut self,
        owner: &str,
        name: &str,
        ty: &Type,
        scope: &str,
    ) -> Option<usize> {
        self.variable(owner, name, ty, scope, None, true)
    }

    /// The `DILocation` an instruction carries. A `!loc` naming a file other
    /// than the subprogram's own is placed in a lexical block for that file, so
    /// generated text keeps its own line table.
    pub(crate) fn location(&mut self, owner: &str, location: &DebugLocation) -> usize {
        let Some(entry) = self.subprograms.get(owner).cloned() else {
            return 0;
        };
        let scope = if entry.file == location.file {
            entry.node
        } else {
            self.file_block(owner, &location.file)
        };
        self.push(&format!(
            "!DILocation(line: {}, column: {}, scope: !{scope})",
            location.line, location.column
        ))
    }

    /// The lexical block of `file` inside the function `owner`, so that a
    /// generated file keeps its own line table.
    fn file_block(&mut self, owner: &str, file: &str) -> usize {
        let Some(entry) = self.subprograms.get(owner).cloned() else {
            return 0;
        };
        if file.is_empty() || file == entry.file {
            return entry.node;
        }
        let file_node = self.file(file);
        let id = self.distinct(&format!(
            "!DILexicalBlock(scope: !{}, file: !{file_node}, line: 1, column: 1)",
            entry.node
        ));
        if let Some(entry) = self.subprograms.get_mut(&entry.name) {
            entry.blocks.insert(file.to_owned(), id);
        }
        id
    }

    /// The scope a `!var` names: the subprogram itself for the function and
    /// for an empty scope, and a lexical block of it for anything else.
    fn scope(&mut self, owner: &str, scope: &str) -> usize {
        let Some(entry) = self.subprograms.get(owner).cloned() else {
            return 0;
        };
        if scope.is_empty() || scope == owner {
            return entry.node;
        }
        let key = format!("scope\u{1}{scope}");
        if let Some(id) = entry.blocks.get(&key) {
            return *id;
        }
        let file = self.file(&entry.file);
        let id = self.distinct(&format!(
            "!DILexicalBlock(scope: !{}, file: !{file}, line: 0, column: 1)",
            entry.node
        ));
        if let Some(entry) = self.subprograms.get_mut(&entry.name) {
            entry.blocks.insert(key, id);
        }
        id
    }

    /// The `DIGlobalVariable` and its initializer for a global.
    pub(crate) fn global(&mut self, name: &str, ty: &Type) -> usize {
        let ty = self.di_type(ty);
        let variable = self.distinct(&format!(
            "!DIGlobalVariable(name: \"{}\", scope: null, file: null, line: 0, type: !{ty}, isLocal: false, isDefinition: true)",
            escape(name)
        ));
        self.push(&format!(
            "!DIGlobalVariableExpression(var: !{variable}, expr: !DIExpression())"
        ))
    }

    /// The module's debug metadata: the format flags, the compile unit and
    /// every node built for it.
    pub(crate) fn render(&mut self) -> String {
        let version = self.push(&flag(DEBUG_INFO_VERSION, "Debug Info Version"));
        let format = self.push(&if self.codeview {
            flag(CODEVIEW_FORMAT, "Debug Info Format")
        } else {
            flag(DWARF_VERSION, "Dwarf Version")
        });
        let enums = self.push("!{}");
        let unit = self.unit;
        let language = language(self.module.language.as_str());
        let name = format!("{}.kir", self.module.name);
        let file = self.unit_file(&name);
        self.nodes[unit] = format!(
            "distinct !DICompileUnit(language: {language}, file: !{file}, producer: \"klf\", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, enums: !{enums}, splitDebugInlining: false, nameTableKind: None)"
        );
        let mut out = format!(
            "!llvm.module.flags = !{{!{version}, !{format}}}\n!llvm.dbg.cu = !{{!{unit}}}\n"
        );
        for (id, node) in self.nodes.iter().enumerate() {
            let _ = writeln!(out, "!{id} = {node}");
        }
        out
    }

    /// The file the compile unit is named by: the first file the module's code
    /// came from, or the module's own name when no instruction carries a `!loc`.
    fn unit_file(&mut self, fallback: &str) -> usize {
        let first = self
            .subprograms
            .values()
            .map(|entry| entry.file.clone())
            .next()
            .unwrap_or_else(|| fallback.to_owned());
        self.file(&first)
    }
}

fn flag(value: u32, name: &str) -> String {
    format!("!{{i32 2, !\"{name}\", i32 {value}}}")
}

/// The DWARF language of a hosted language: `KArc` under `DW_LANG_C_plus_plus`,
/// C under `DW_LANG_C11`, and every KLF Kira definition under its vendor code.
fn language(name: &str) -> u32 {
    let lower = name.to_ascii_lowercase();
    if lower == "c" {
        return C_LANGUAGE;
    }
    if lower.contains("karc") || lower.contains("c++") || lower.contains("cpp") {
        return KARC_LANGUAGE;
    }
    KIRA_LANGUAGE
}

/// Whether a triple is a Windows one, the only target whose debug information
/// is `CodeView` rather than DWARF.
fn is_codeview(triple: &str) -> bool {
    let lower = triple.to_ascii_lowercase();
    lower.contains("windows") || lower.contains("mingw")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\0A"),
            '\r' => out.push_str("\\0D"),
            '\t' => out.push_str("\\09"),
            other if (other as u32) < 0x20 || other as u32 == 0x7f => {
                let _ = write!(out, "\\{:02X}", other as u32);
            }
            other => out.push(other),
        }
    }
    out
}
