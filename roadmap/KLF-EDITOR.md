# KLF Editor and Language Server Specification v1.0

Date: 2026-10-04. Status: normative draft 1.0. Audience: every agent and engineer that implements the KLF language server, the editor facts a definition records, the generated editor packages, or the editor support of a definition.

Companion to [`KLF-RIDL.md`](KLF-RIDL.md) (the program pipeline) and [`KLF-KSL.md`](KLF-KSL.md) (the shader pipeline). Both name a `<driver> lsp` command and three definition hooks (`hover`, `complete`, `definition`) and stop there. This document specifies the rest: how a generated toolchain becomes a language server, what a definition records so the kernel can answer editor requests, how analysis stays correct and fast while the user types, how editor packages for VS Code, Zed, Neovim, Helix and Emacs are generated from a definition, and how all of this is tested.

Contents

1. Status, scope and rules for implementers
2. Terminology
3. Goals, non-goals and related work
4. System overview
5. The editor surface of a definition
6. Editor services
7. Error tolerance
8. The analysis host
9. Project model
10. Protocol: lifecycle, transport and documents
11. Protocol: features
12. Token classes
13. Program languages
14. Shader languages
15. Across toolchains
16. Kernel languages: `klf lsp`
17. Protocol extensions
18. Command-line parity
19. Generated editor packages
20. Language profile: Kira
21. Language profile: KSL and MSL
22. Language profiles: C and KArc
23. Performance budgets
24. Conformance
25. Diagnostics
26. Implementation plan and crate layout
27. Appendix A: editor corpus format
28. Appendix B: settings
29. Appendix C: pattern dialect
30. Appendix D: decision log

---

## 1. Status, scope and rules for implementers

### 1.1 Normative language

MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119. Text marked informative is explanatory. Everything else is normative.

### 1.2 Scope and stability

In scope: the `lsp` command of every generated driver (program and shader), the `klf lsp` command for the kernel's own file types, the editor members and hooks of the `Language` and `ShaderLanguage` families, the editor services of the `Session`, the analysis host (documents, revisions, cancellation, scheduling, caching), the protocol surface (Language Server Protocol 3.17 plus the `klf/` extensions of section 17), generated editor packages, the editor profiles of Kira, KSL, MSL, C and KArc, and conformance.

Out of scope for 1.0: debugging (Debug Adapter Protocol, DE-014), notebooks, collaborative editing, and AI completion. Section 3.2 lists the rest.

Stability: the protocol surface follows LSP 3.17 and changes when a later LSP version is adopted. The `klf/` extensions are versioned by the `klf.version` field of `initialize`'s result and change additively within a KLF major release. The editor members of the families change additively after 1.0, as the families do.

### 1.3 Amendments to the companion documents

This document amends the following sections. Where they disagree with this document, this document wins, and each amendment MUST be carried into the companion document in the commit that implements it.

| Section | Amendment |
| --- | --- |
| `KLF-RIDL.md` 5.4 | The sysroot adds `share/<lang>/src/`, a copy of the std sources, so navigation can open std declarations, and `share/<lang>/editor/`, the generated editor packages of section 19 |
| `KLF-RIDL.md` 5.5 | `<driver> lsp` is specified by this document. `<driver> check` runs the frontend and the mandatory KIR passes (raw verification, section 11.6 of that document), so ownership diagnostics are part of `check` and of the editor (section 13.1). `<driver> format` and `<driver> symbols` are added (section 18) |
| `KLF-RIDL.md` 7.2 | `Language` gains the member `editor` and the hooks of section 5.3. The signatures of `hover`, `complete` and `definition` change to those of section 5.3. `Completion` is defined in section 5.4 |
| `KLF-RIDL.md` 8.2 | `Diagnostic` gains `tags`; `DiagnosticSpec` gains `url` and `tags` (section 6.9) |
| `KLF-RIDL.md` 8.9 | Facts recorded by the services of section 6 are query outputs, replayed on reuse like diagnostics. The analysis mode is part of every query key (section 8.4) |
| `KLF-RIDL.md` 8.11 | The "Language server" item is replaced by this document |
| `KLF-RIDL.md` 16.4, 18.8 | In a server process, a trap or cancellation inside definition code unwinds to the phase boundary instead of aborting (section 8.6) |
| `KLF-RIDL.md` 20.7 | `.ridl` emission adds `@Source("<path>:<line>:<column>")` to each exported declaration when the emitting driver is given `--ridl-source` (section 15.3) |
| `KLF-RIDL.md` 31.4 | Area `LSP` is added: `KLF-LSP-<NNNN>` (section 25) |
| `KLF-RIDL.md` 32.1 | Crates of section 26.1 are added |
| `KLF-KSL.md` 5.2 | `ShaderLanguage` gains the member `editor` and the hooks of section 5.3, as `Language` does |
| `KLF-KSL.md` 5.7 | `<driver> lsp` is specified by this document; section 14 adds the shader behavior |

### 1.4 Rules for implementing agents

1. **One computation.** An editor diagnostic and a command-line diagnostic for the same files, flags and revision are the same computation and MUST be identical: code, severity, span, message, notes and fixes. The server never runs a second, lighter frontend. Section 18 states how this is tested.
2. **The kernel owns the protocol.** No definition reads or writes JSON-RPC, positions in UTF-16, URIs or LSP structures. Definitions speak spans, entities and the types of section 5 and 6. A definition that needs something the protocol has and the services lack asks for a service; it does not get the protocol.
3. **Facts before hooks.** A feature the kernel can answer from recorded facts (section 6) is answered by the kernel. Hooks exist for answers only the definition can compute (member completion, signature help, formatting). A hook MUST NOT duplicate a fact-based feature for a language that records the facts.
4. **Broken code is the normal case.** Every requirement of this document is stated for source that does not compile. A feature that works only on clean code is incomplete (section 7).
5. **The oracle is the floor.** For Kira, the pre-KLF server (`kira/crates/kira-lsp`, `kira-language-server`) is the oracle: every request it answers on `tests-kik` and the Kira ecosystem packages, the KLF server MUST answer at least as well (section 24.5). As with every oracle, its code is not reused (`KLF-RIDL.md` 1.3.6).
6. **No editor-specific language knowledge outside the definition.** Generated editor packages (section 19) carry nothing a definition did not declare. A keyword list typed into a grammar file by hand is a defect.
7. **Decisions.** Implementation decisions go to `docs/decisions.md` with the next `D-` number. Specification decisions in this document use the `DE-` series of Appendix D.

---

## 2. Terminology

| Term | Meaning |
| --- | --- |
| Server | A process speaking LSP over standard input and output: a generated driver run as `<driver> lsp`, or `klf lsp` |
| Client | The editor or its extension |
| Document | A file the client has opened. Its text is the client's, not the disk's |
| Overlay | The server's file view: open documents over the disk (section 8.1) |
| Revision | A number that increases on every change to the overlay, the configuration or a watched file |
| Snapshot | A read-only view of the analysis at one revision |
| Analysis mode | `Build` for command-line drivers, `Editor` for servers (section 8.4) |
| Fact | A record a definition makes through the services of section 6 while it runs its phases: a token class, a symbol, a reference, a type at a span, a fold, a hint, a link, a runnable |
| Entity | What facts name: a 64-bit key the definition chooses and uses consistently for one declaration (for example its file and start offset packed together), or a kernel `EntityId` (`KLF-RIDL.md` 8.4). The kernel compares keys and never interprets them |
| Project | A set of inputs and driver arguments that one `check` would analyze, found by the project hook (section 9) |
| Loose file | An open document no project claims; analyzed alone |
| Editor spec | The `EditorSpec` value a definition declares (section 5.2): the lexical facts editors need without running the server |
| Lexical grammar | The TextMate and tree-sitter grammars generated from the editor spec (section 19.3) |
| Rich grammar | A hand-written tree-sitter grammar a definition MAY reference instead of the lexical tree-sitter grammar (section 19.4) |
| Virtual file | A `Session.virtualFile` (`KLF-RIDL.md` 8.1): macro output, preprocessor output, instantiation text, or a shader backend output shown in the editor |

---

## 3. Goals, non-goals and related work

### 3.1 Goals

1. **Every toolchain is a language server.** `klf build` produces a driver whose `lsp` command is a complete server for the language, with no code written for the protocol by the definition author.
2. **Native to KLF.** The server is the kernel's query engine (`KLF-RIDL.md` 8.9) run in editor mode. Facts come from the definition's own phases. The kernel adds what only the kernel knows: ownership, RIDL, binding slots, layouts, per-target shader validity.
3. **Correct on broken code** and **fast while typing**: the budgets of section 23 hold on the largest corpus of each language, including UE5 for KArc.
4. **Program and shader languages alike.** `Language` and `ShaderLanguage` definitions get the same server. Shader servers add target-aware features (section 14).
5. **Editors from one declaration.** VS Code, Zed, Neovim, Helix and Emacs packages are generated from the definition's editor spec, so highlighting, brackets, comments, indentation and server launch never drift from the language (section 19).
6. **The kernel's own files too.** `.klfd`, `.kir`, `.ksir`, `.ridl`, `.ridlsig` and `.klffi` get a server, `klf lsp`, built on the same host (section 16).

### 3.2 Non-goals

- Debugging (DE-014). Runnables (section 11.18) let an editor start a program; stepping through it is a separate document.
- A server that loads several definitions in one process. One definition per toolchain (`KLF-RIDL.md` 4.5) holds for servers too; an editor runs one server per language (section 15.1).
- Editors not listed in section 19. Any LSP client works with the server; only the listed editors get generated packages.
- Publishing to marketplaces. Generated packages are complete and publishable; publishing needs accounts and is the language owner's step.
- Syntax highlighting computed by the client from the rich grammar alone as the source of truth. The server's semantic tokens are the truth; grammars are the first paint (DE-006).

### 3.3 Related work (informative)

- **rust-analyzer**: a query database (salsa) with revision-based cancellation by unwinding, an overlay of open files, and features computed from analysis rather than a second parser. KLF follows its architecture and its cancellation model (DE-004).
- **clangd**: per-translation-unit analysis with `compile_commands.json`, a background index, preamble caching of headers, and inactive preprocessor regions. KLF's C and KArc profiles follow it (section 22).
- **The pre-KLF Kira server** (`kira-lsp`): runs the compiler's own salsa frontend, full-text sync, diagnostics, definition and declaration from resolver links, hover showing the declaration's source line, and completion from links plus the declaration tree. It reads imported modules from disk, not from open buffers. KLF keeps its first principle (one computation) and replaces its limits (section 20.4).
- **Tree-sitter and TextMate**: the grammars editors use for the first paint and for structural editing. KLF generates both (section 19.3).
- **naga, Tint, Metal shader tooling, Shader Language Server projects**: shader editors without per-target validation and binding reflection. Section 14 is where KLF goes further, because the kernel owns binding assignment and every backend.

---

## 4. System overview

### 4.1 One server per toolchain

```
editor ──LSP/stdio── <driver> lsp
                       │
                       ├─ protocol layer (klf-lsp): JSON-RPC, capabilities, encodings, scheduling
                       ├─ analysis host (klf-ide): overlay, revisions, project model, snapshots
                       │     └─ query engine (KLF-RIDL 8.9), analysis mode Editor
                       │           └─ the compiled definition: units, parse, elaborate, [lower]
                       │                 └─ editor services (section 6) record facts
                       └─ default features (klf-ide): answered from facts
                             └─ hooks of the definition (section 5.3) where facts are not enough
```

The server is not a separate program. `<driver> lsp` is the same binary as `<driver> build`, with the same compiled definition, the same flags, and the same query engine. It differs in three ways: the analysis mode is `Editor`, inputs come from the overlay, and definition code is cancellable (section 8.6).

### 4.2 What runs in the editor

| Phase | Program language | Shader language |
| --- | --- | --- |
| `units`, `parse`, `elaborate` | Always, on every revision, for open documents first | Same |
| `lower` | When `elaborate` of the unit reported no error, to run the mandatory KIR passes for ownership diagnostics (section 13.1); deprioritized behind interactive requests | When `elaborate` reported no error, then validation, mandatory passes, binding assignment and the requested backends without writing files (section 14.1) |
| Optimization, backends, linking | Never, except for the `klf/kir` view (section 17) | Backends only as above |

### 4.3 Data flow of an edit (informative)

1. `textDocument/didChange` arrives. The overlay applies the edit; the revision increases; every running read is cancelled (section 8.5).
2. The scheduler queues diagnostics for the changed document's project after a debounce (default 50 ms).
3. The client asks for semantic tokens. The server takes a snapshot, runs `parse` for the changed file (its other files and units are reused from the query cache), and answers from the token facts.
4. After the debounce, `elaborate` runs. Its facts and diagnostics replace the previous revision's for that unit. Diagnostics are published (or answered to a pull) with the document version.
5. When idle, `lower` and the mandatory passes run for units with no elaboration errors, adding ownership diagnostics.

### 4.4 File types

| Extension | Served by |
| --- | --- |
| A program language's `extensions` | `<driver> lsp` of that language |
| A shader language's `extensions` | `<driver> lsp` of that shader language |
| `.klfd`, `.kir`, `.ksir`, `.ridl`, `.ridlsig`, `.klffi` | `klf lsp` |

---

## 5. The editor surface of a definition

### 5.1 Members added to both families

```
construct Language<Tree: Cacheable, Program: Cacheable> {
    // ... members of KLF-RIDL.md 7.2 ...
    let editor: EditorSpec { return EditorSpec.minimal(self.name) }

    function project(session: Session, file: String) -> Optional<Project> { return .None }
    function hover(session: Session, at: Position, facts: HoverFacts) -> Optional<HoverText> { return .None }
    function complete(session: Session, at: Position, trigger: CompletionTrigger) -> CompletionList { return CompletionList.kernel }
    function definition(session: Session, at: Position) -> [Span] { return [] }
    function signature(session: Session, at: Position) -> Optional<SignatureHelp> { return .None }
    function format(session: Session, unit: Unit, tree: Tree) -> Bool          // KLF-FORMAT.md section 3; optional
    function actions(session: Session, range: Span, diagnostics: [Diagnostic], only: [ActionKind]) -> [CodeAction] { return [] }
    function rename(session: Session, entity: EntityId, newName: String) -> RenameCheck { return .Allowed }
    function fileRenamed(session: Session, from: String, to: String) -> [TextEdit] { return [] }
}
```

`ShaderLanguage` gains exactly the same members with the same signatures.

### 5.2 `EditorSpec`

```
struct EditorSpec {
    let displayName: String
    let scope: String                          // TextMate root scope, e.g. "source.kira"
    let fileNames: [String] { return [] }      // exact file names claimed, e.g. "Makefile"; none for most languages
    let rootMarkers: [String] { return [] }    // file names whose directory is a project root (section 9.2), e.g. "kira.toml"
    let firstLine: Optional<Pattern> { return .None }
    let lineComments: [String]
    let blockComments: [BlockComment]
    let docComments: [String] { return [] }
    let strings: [StringForm]
    let numbers: [Pattern]
    let identifier: Pattern
    let keywords: [KeywordGroup]
    let operators: [String]
    let brackets: [BracketPair]
    let autoClose: [AutoClosePair]
    let indent: IndentRules
    let onEnter: [OnEnterRule] { return [] }
    let regionMarkers: Optional<RegionMarkers> { return .None }
    let annotation: Optional<Pattern> { return .None }
    let preprocessor: Optional<Pattern> { return .None }
    let grammar: Optional<GrammarRef> { return .None }
    let icon: String { return "" }             // path in the package to an SVG or PNG icon
}

struct BlockComment { let open: String  let close: String  let nests: Bool }
struct StringForm {
    let open: String
    let close: String
    let escape: Optional<Pattern>             // one escape sequence, e.g. \\[ntre0"\\]
    let multiline: Bool
    let raw: Bool                             // no escapes recognized
    let interpolation: Optional<BracketPair>  // e.g. \( and )
    let class: TokenClass                     // String or Character
}
struct KeywordGroup { let class: TokenClass  let modifiers: [TokenModifier]  let words: [String] }
struct BracketPair { let open: String  let close: String }
struct AutoClosePair { let open: String  let close: String  let notIn: [AutoCloseContext] }
enum AutoCloseContext { String Comment }
struct IndentRules { let unit: String  let increase: Optional<Pattern>  let decrease: Optional<Pattern> }
struct OnEnterRule { let before: Pattern  let after: Optional<Pattern>  let action: EnterAction  let append: String }
enum EnterAction { None Indent IndentOutdent Outdent }
struct RegionMarkers { let start: Pattern  let end: Pattern }
struct GrammarRef { let repository: String  let revision: String  let path: String }
distinct Pattern = String                    // the dialect of Appendix C
```

Rules:

1. Every `Pattern` MUST be in the dialect of Appendix C. `klf build` checks every pattern and fails with `KLF-DEF-0040` naming the member and the offending construct.
2. `keywords` MUST list exactly the words the definition's lexer treats as reserved, plus contextual words in a separate group with modifier `contextual`. `klf test` verifies this (section 24.4 item 1).
3. `brackets` are the pairs the definition's parser balances. Generated grammars use them for nesting, folding and indentation.
4. `grammar`, when present, names a rich tree-sitter grammar (section 19.4). When absent, tree-sitter editors use the generated lexical grammar.
5. `EditorSpec.minimal(name)` declares C-style `//` and `/* */` comments, `"` strings with `\` escapes, decimal numbers, `[A-Za-z_][A-Za-z0-9_]*` identifiers, the three bracket pairs, and no keywords. A definition that does not override `editor` gets working editors without highlighting of keywords.

### 5.3 Hooks

| Hook | Called for | Default |
| --- | --- | --- |
| `project` | Each file the server must place in a project (section 9) | `.None`: the kernel's default rule (section 9.2) |
| `hover` | `textDocument/hover`, after the kernel has built `HoverFacts` from the facts at the position | `.None`: the kernel renders the facts (section 11.5) |
| `complete` | `textDocument/completion` | `CompletionList.kernel`: the kernel's scope and keyword completion (section 11.6) |
| `definition` | `textDocument/definition` when the facts hold no reference at the position | `[]` |
| `signature` | `textDocument/signatureHelp` | `.None`: the kernel's call-site rule (section 11.7) |
| `format` | `textDocument/formatting`, `rangeFormatting`, and `<driver> format`, as `KLF-FORMAT.md` specifies | Absent: the capability is not advertised |
| `actions` | `textDocument/codeAction`, in addition to the quick fixes the kernel derives from `Diagnostic.fixes` | `[]` |
| `rename` | `textDocument/prepareRename` and `rename` | `.Allowed` after the kernel's identifier check (section 11.11) |
| `fileRenamed` | `workspace/willRenameFiles`, per affected file | `[]` |

Each hook runs as a query in editor mode with the snapshot's revision. A hook MUST be a pure function of what it reads through `Session` (`KLF-RIDL.md` 8.9). A hook MAY call the editor services' read side (section 6.12) to look up facts.

### 5.4 Types used by hooks

```
struct HoverFacts {
    let entity: Optional<EntityId>
    let symbol: Optional<SymbolInfo>
    let typeText: Optional<String>
    let value: Optional<String>               // a compile-time value recorded with `valueAt`
    let range: Span
}
struct HoverText { let markdown: String  let replaceKernel: Bool }

enum CompletionTrigger { Invoked Character(String) Incomplete }
struct CompletionList { let items: [Completion]  let incomplete: Bool  let includeKernel: Bool }
struct Completion {
    let label: String
    let kind: SymbolKind
    let detail: String { return "" }
    let documentation: String { return "" }   // Markdown
    let insert: String                        // snippet syntax when `snippet`
    let snippet: Bool { return false }
    let replace: Optional<Span> { return .None }
    let sortKey: String { return "" }
    let filter: String { return "" }
    let commitCharacters: [String] { return [] }
    let deprecated: Bool { return false }
    let entity: Optional<EntityId> { return .None }
    let additionalEdits: [TextEdit] { return [] }   // for example an import to add
}

struct SignatureHelp { let signatures: [Signature]  let active: Int  let activeParameter: Int }
struct Signature { let label: String  let documentation: String  let parameters: [Span] }   // spans are byte ranges into `label`

struct TextEdit { let span: Span  let text: String }

enum ActionKind { QuickFix Refactor RefactorExtract RefactorInline RefactorRewrite Source OrganizeImports FixAll }
struct CodeAction {
    let title: String
    let kind: ActionKind
    let edits: [TextEdit]
    let preferred: Bool { return false }
    let diagnostics: [Diagnostic] { return [] }
    let disabledReason: Optional<String> { return .None }
}

enum RenameCheck { Allowed Refused(String) }

struct Project {
    let name: String
    let root: String
    let arguments: [String]            // what `<driver> check` takes to analyze the project
    let watch: [String]
    let output: Optional<String> { return .None }   // shader languages: the directory build-on-save writes (section 14.6)
}
```

`Signature.parameters` are byte ranges into `label` so the kernel can convert them to the client's encoding.

---

## 6. Editor services

### 6.1 Principle

Facts are recorded by the code that already knows them. The lexer knows token classes, the parser knows symbols, folds and syntax nesting, name resolution knows references, the type checker knows types and hints. Each records through one service call at the moment it decides. The kernel stores facts per file and per query, replays them when the query is reused, and answers editor requests from them.

All recording services are cheap no-ops in `Build` mode: a single test of a flag in the `Session`, no allocation (section 8.4).

### 6.2 Mode

```
enum AnalysisMode { Build Editor }
extend Session {
    function mode() -> AnalysisMode
}
```

A definition MAY branch on `mode()` to skip work that only editors need (for example computing display strings for types) and to enable recovery it does not want in builds. It MUST NOT change accept or reject decisions or diagnostics by mode (rule 1.4.1).

### 6.3 Token classes

```
extend Session {
    function classify(span: Span, class: TokenClass, modifiers: [TokenModifier])
}
```

Records the class of one token or name occurrence. Classes and modifiers are section 12. Rules:

1. The lexer SHOULD classify every non-trivia token, including keywords, literals, operators and punctuation, and SHOULD classify comments, including doc comments and nested block comments.
2. Later phases refine: a name the lexer classified as `Variable` may be reclassified by `elaborate` as `Function`, `Type` or `Parameter`. For one span, the fact from the latest phase wins; within a phase, the latest call wins.
3. Spans MAY cover several lines (block comments, multiline strings). The kernel splits them for clients without multiline token support.
4. Spans in a virtual file are mapped to their origin when the origin is the identical source text (macro arguments copied unchanged); other virtual spans are not shown as tokens in the real file. Generated names get the modifier `injected` when they are shown.

### 6.4 Symbols

```
enum SymbolKind {
    File Module Namespace Package Class Method Property Field Constructor Enum Interface Function
    Variable Constant String Number Boolean Array Object Key Null EnumMember Struct Event Operator
    TypeParameter Macro Trait Resource Stage Label Test
}
struct SymbolInfo {
    let name: String
    let kind: SymbolKind
    let span: Span                      // the whole declaration
    let selection: Span                 // the name
    let container: Optional<EntityId>
    let signature: String { return "" } // one line as the language writes it, e.g. "function area(r: Float) -> Float"
    let documentation: String { return "" }
    let parameters: [String] { return [] }
    let detail: String { return "" }
    let deprecated: Bool { return false }
    let exported: Bool { return false }
    let local: Bool { return false }    // function-local; excluded from workspace symbols and outline
}
extend Session {
    function declareSymbol(entity: EntityId, info: SymbolInfo)
    function declaration(entity: EntityId, span: Span, selection: Span)
}
```

`declareSymbol` records the defining occurrence. `declaration` records an additional non-defining declaration (a C prototype, a C++ forward declaration, a header member declaration), used by `textDocument/declaration`. The kinds after `TypeParameter` that LSP lacks (`Macro`, `Trait`, `Resource`, `Stage`, `Label`, `Test`) map to LSP kinds by the table of section 12.4.

### 6.5 References

```
enum ReferenceRole { Read Write ReadWrite Call Type Import Implementation Other }
extend Session {
    function reference(span: Span, entity: EntityId, role: ReferenceRole)
    function lookupAt(from: ScopeId, name: Symbol, namespace: Namespace, at: Span, role: ReferenceRole) -> [EntityId]
    function unresolved(span: Span, namespace: Namespace, from: ScopeId)
    function scopeAt(span: Span, scope: ScopeId)
}
```

1. `reference` records that the text at `span` names `entity`. A resolution that picks one of an overload set records the picked entity.
2. `lookupAt` is `lookup` (`KLF-RIDL.md` 8.4) that also records a reference to the result when it is a single entity. Definitions that resolve names with kernel scopes get references for free.
3. `unresolved` records a name that resolved to nothing, with the scope it was looked up from. The kernel uses it for completion (section 11.6), for the `unresolved` token modifier, and for "import this" quick fixes when the `actions` hook offers them.
4. `scopeAt` records that the kernel scope `scope` is the lexical scope of `span`. Completion uses the innermost recorded scope containing the cursor.

### 6.6 Types, values and hints

```
enum HintKind { Type Parameter Ownership Layout Value Other }
struct InlayHint {
    let at: Position
    let label: String
    let kind: HintKind
    let paddingLeft: Bool { return false }
    let paddingRight: Bool { return false }
    let target: Optional<EntityId> { return .None }   // clicking the hint navigates here
    let insert: Optional<String> { return .None }     // text that makes the hint explicit when accepted
}
extend Session {
    function typeAt(span: Span, text: String, entity: Optional<EntityId>)
    function valueAt(span: Span, text: String)
    function hint(hint: InlayHint)
}
```

`typeAt` records the type of the expression or binding at `span`, as text the language would write, and the type's entity for `textDocument/typeDefinition`. `valueAt` records a compile-time value (a folded constant, a `comptime` result, a shader option). `hint` records an inlay hint. The kernel adds its own hints (sections 13.3 and 14.4).

### 6.7 Structure

```
enum FoldKind { Region Comment Imports Other }
extend Session {
    function syntaxNode(span: Span)
    function fold(span: Span, kind: FoldKind)
}
```

`syntaxNode` records the extent of a syntax node; the kernel builds selection ranges from the nesting of recorded nodes (section 11.14). A parser SHOULD record every node that a user would select as a unit: expressions, statements, blocks, declarations, argument lists. `fold` records a foldable range beyond what brackets and comments give (section 11.13).

### 6.8 Links, runnables, lenses and edges

```
enum LinkTarget { File(String) Url(String) Entity(EntityId) }
enum RunnableKind { Main Test Benchmark Corpus Shader }
struct Runnable { let label: String  let kind: RunnableKind  let span: Span  let arguments: [String] }
extend Session {
    function link(span: Span, target: LinkTarget)
    function runnable(runnable: Runnable)
    function callEdge(caller: EntityId, callee: EntityId, at: Span)
    function typeEdge(subtype: EntityId, supertype: EntityId)
    function inactive(span: Span)
}
```

1. `link`: an import path, a `ksl!("...")` argument, an `#include`, a URL in a comment.
2. `runnable`: a program entry (`@Main`), a test (`@Test`), a corpus case. `arguments` are uniform driver arguments (`KLF-RIDL.md` 5.5) that run exactly this item, for example `["test", "--filter", "parsesEmpty", "<root>"]`.
3. `callEdge` and `typeEdge` feed call and type hierarchies. A definition SHOULD record call edges during elaboration where it resolves calls.
4. `inactive` marks source a preprocessor or a build condition excluded (`#if 0`, a platform-specific region). Its tokens get the modifier `inactive`.

### 6.9 Diagnostics

`KLF-RIDL.md` 8.2 is amended:

```
enum DiagnosticTag { Unnecessary Deprecated }
struct Diagnostic {
    // ... members of KLF-RIDL.md 8.2 ...
    let tags: [DiagnosticTag] { return [] }
}
struct DiagnosticSpec {
    // ... members of KLF-RIDL.md 31.4 ...
    let url: String { return "" }        // documentation URL; "{code}" is replaced by the code
    let tags: [DiagnosticTag] { return [] }
}
```

A diagnostic's tags are the union of its own and its spec's. `fixes` (already in 8.2) become quick fixes (section 11.10). Diagnostics need no editor-specific service: the same `report` serves both modes (rule 1.4.1).

### 6.10 Recovery helpers

The `Lexer` and `Pratt` helpers of `KLF-RIDL.md` 8.11 gain:

```
extend Lexer {
    function tokensTolerant(text: String) -> [Token]    // never stops; unknown bytes become Error tokens and are reported
}
struct TokenCursor { }
extend TokenCursor {
    function expect(kind: Int, diagnostic: Diagnostic) -> Optional<Token>   // reports and does not advance on mismatch
    function recoverTo(kinds: [Int], stopAtCloser: Bool) -> Span            // skips balanced brackets; returns the skipped span
    function balancedClose(open: Int) -> Optional<Token>                   // finds the closer using brackets and indentation
}
```

`balancedClose` pairs an unclosed bracket by indentation: a closer at an indentation lower than the line of the opener closes the opener's enclosing construct instead. This is what keeps one missing `}` from swallowing the rest of a file. Definitions MAY use their own recovery; section 7 states what the result must be.

### 6.11 Classification of virtual and generated code

Facts recorded inside a virtual file belong to that file. A reference from a real file to an entity declared in a virtual file (a macro-generated function) navigates to the virtual file when the client supports `klf/virtualFile` (section 17), otherwise to the macro invocation that produced it (the origin chain's first real span).

### 6.12 Reading facts

Hooks read facts of the current snapshot:

```
extend Session {
    function referenceAt(at: Position) -> Optional<(Span, EntityId)>
    function symbolOf(entity: EntityId) -> Optional<SymbolInfo>
    function typeAtPosition(at: Position) -> Optional<(Span, String, Optional<EntityId>)>
    function scopeAtPosition(at: Position) -> Optional<ScopeId>
    function referencesTo(entity: EntityId) -> [(Span, ReferenceRole)]
}
```

`referencesTo` reads the workspace index (section 8.7) and may be incomplete while indexing runs; the result carries no promise of completeness to a hook.

### 6.13 Service ABI

The services above are added to `kernel/services.manifest` under `klf.editor.*`, with the same rules as every service (`KLF-RIDL.md` 8.12): coarse, integers and buffers only.

1. A span is three words: the file (or -1 for the file of the running phase), the start and the end byte offset. A span past the end of its file is refused.
2. An optional entity is -1 when absent. Strings are buffers; where a service takes several strings, they are one buffer separated by NUL bytes.
3. Every recording service returns 1 when it recorded the fact and 0 otherwise. In `Build` mode it returns 0 without reading its arguments.
4. `klf.editor.classify_many(file, tokens, count)` takes `count` tokens of four words each, `(start, end, class, modifiers)`, so a lexer classifies a whole file in one call, and returns how many it recorded.
5. `symbol` takes the span, the name span, the kind and the container; the name defaults to the text of the name span. `symbol_text(entity, field, text)` sets the name (0), signature (1), documentation (2) or detail (3), or appends a parameter (4); `symbol_flags(entity, bits)` sets deprecated (1), exported (2) and local (4).
6. `scope(span, key)` is `scopeAt` with a scope key the definition chooses, as entities are. `lookupAt` is a convenience over kernel scopes; a definition that resolves names itself calls `reference` and `unresolved`.
7. `hint` takes the label, then optionally a NUL and the text accepting the hint inserts. `link` takes a kind (0 file, 1 URL, 2 entity) with the entity or the text. `runnable` takes the label and then each driver argument.

KLF Kira reaches the integer services as `editor_*` wrappers (`editor_classify`, `editor_symbol`, ...) and the services that take text as builtins whose last argument is a `String`: `editorSymbolText`, `editorTypeAt`, `editorValueAt`, `editorHint`, `editorLink` and `editorRunnable`.

---

## 7. Error tolerance

### 7.1 The contract

In `Editor` mode:

1. **`parse` returns a tree for every input.** It MUST NOT stop at the first error. Every error region produces at least one diagnostic. A definition whose `Build`-mode parser stops at the first error (as the KLF Kira definition of Kira does today) MUST recover in both modes, because rule 1.4.1 forbids mode-dependent diagnostics: the diagnostics of a broken file are the same on the command line and in the editor, and both list every independent error.
2. **Recovery granularity.** A syntax error inside one declaration MUST NOT remove facts of any other top-level declaration of the file. Within a function body, an error in one statement MUST NOT remove facts of the statements before it, and SHOULD NOT remove facts of the statements after it.
3. **`elaborate` accepts trees with errors.** Error nodes elaborate to an error type that suppresses further diagnostics about values of that type (no cascade). Declarations that do not depend on an erroneous one are elaborated fully and record all their facts.
4. **Partial files.** Every prefix of a valid file, cut at any token boundary, MUST parse without an internal error, and the facts of the complete declarations in that prefix MUST be recorded. This is the state of a file while it is typed top to bottom.
5. **Budgets hold on broken code.** Recovery MUST NOT be super-linear in file size: the parse budget of section 23 applies to every fuzzed input of section 24.4.

### 7.2 Diagnostics on broken code

The first diagnostic in an error region names what was expected. Later errors caused by the same region are not reported. A definition MAY report at most one diagnostic per independent error region; the conformance suites measure independence by the cascade rule of section 24.4 item 4.

### 7.3 What the kernel guarantees in return

1. The kernel never calls `lower` on a unit whose `elaborate` reported an error (in either mode). Definitions do not need error-tolerant lowering.
2. Facts recorded before a cancellation are discarded with the query (section 8.5); a definition never sees half of its own facts.

---

## 8. The analysis host

### 8.1 Overlay

The overlay is the file view of every `Session` the server creates. Reading a path (`Session.read`, `readDirectory`, `input`, `includeSearch`) returns the open document's text when the path is open, and the disk's otherwise. An open but unsaved buffer is therefore what every import sees (the pre-KLF server read imports from disk; DE-008).

Paths are canonicalized once: `file:` URIs are percent-decoded, `.` and `..` resolved, symbolic links resolved for identity but reported with the path the client used. On case-insensitive file systems, identity is the canonical case from the file system.

Non-`file:` documents (`untitled:`) are loose files with no directory; imports from them resolve against the first workspace folder.

### 8.2 Text and positions

1. Text is stored as UTF-8 bytes. A document that is not valid UTF-8 keeps its bytes; the decoder maps each invalid byte to U+FFFD for display and counts it as one code unit of every encoding.
2. Line breaks are `\n`, `\r\n` and `\r`, as LSP defines them.
3. The position encoding is negotiated (LSP 3.17 `general.positionEncodings`): the server chooses `utf-8` when the client offers it, otherwise `utf-16`. `utf-32` is accepted when it is the only shared encoding.
4. Each file has a line index: line start offsets, and for lines containing non-ASCII bytes a sorted list of (byte offset, code unit offset) breakpoints. Conversion is O(log n) per position.
5. A client position beyond the end of a line clamps to the end of the line; beyond the last line clamps to the end of the file. This is what LSP specifies and what clients rely on during fast typing.

### 8.3 Revisions and snapshots

The revision increases on every overlay change, configuration change and watched-file change. A request is answered from a snapshot of the revision current when the request starts. Snapshots are cheap: the query engine's red-green state at that revision.

### 8.4 Analysis mode in the cache

The analysis mode is part of every query key. Results computed in `Build` mode carry no facts and MUST NOT be reused in `Editor` mode; results from `Editor` mode carry facts and are not reused by builds. Both share the disk cache directory (`KLF-RIDL.md` 5.6) under separate keys, so a server started after a build reuses neither and both stay correct (DE-005).

### 8.5 Cancellation

1. A write (any revision increase) cancels every running query of earlier revisions. Writes never wait for reads.
2. A query observes cancellation at its next kernel service call or at a cancellation poll (section 8.6), and unwinds to the query boundary. Its facts, diagnostics, arena writes and cache entries are discarded.
3. A request whose analysis was cancelled is retried at the new revision when it is still meaningful (completion, hover, signature help), or answered with `ContentModified` (`-32801`) when the client will ask again (semantic tokens, inlay hints, code lenses, document symbols, folding ranges), as LSP 3.17 recommends.
4. `$/cancelRequest` from the client cancels that request's analysis only if no other pending request shares it; the request is answered with `RequestCancelled` (`-32800`).

### 8.6 Unwinding definition code

Definitions are KLF Kira compiled to native code (`KLF-RIDL.md` 6.7) and built `nounwind`. For a server to cancel them and survive their traps:

1. `klf build` compiles definition code with unwind tables and cleanup landing pads that release counted values. Build-mode drivers pay table size only; no instruction runs on the non-unwinding path.
2. `klf build` inserts a cancellation poll at every loop back edge and every function entry of definition code: one thread-local load and a predictable branch. The poll and every kernel service check the thread's cancellation flag.
3. On cancellation, the kernel raises the kernel-private exception `klf.cancelled`. The phase trampoline (`klf-driver`'s phase runner) catches it, rolls back the query's arena writes and speculation state as `Session.speculate` does, and reports cancellation to the scheduler.
4. In a server process, `klfrt_trap` raised on a worker thread running definition code unwinds the same way with `klf.trapped(code)` instead of aborting. The request fails with `RequestFailed` (`-32803`) and an internal diagnostic `KLF-LSP-9001` naming the definition, the trap code and the query is logged and shown once per query key (section 25). The server keeps running.
5. Definitions run on the VM path (`KLF-RIDL.md` 5.5 notes) poll at back edges and unwind VM frames the same way.

On hosts whose objects `klf build` gives no unwind tables (x86_64 Mach-O and COFF; ELF and aarch64 Mach-O have them, D-360), a generated driver analyzes in a worker process, `<driver> lsp-worker`, one per analysis. The server writes the analysis request (the project's driver arguments and the open documents) to the worker's standard input and reads the analysis (files, diagnostics, facts) from its standard output. Cancelling an analysis kills the worker, and a trap in definition code ends the worker, not the server: the analysis fails, the server logs it, and requests fall back to what the editor spec gives (lexical tokens, bracket folds). The protocol thread runs one analysis at a time and answers a request once its project's analysis is current.

### 8.7 Scheduling and indexing

1. One protocol thread reads and writes messages and owns the overlay. Analysis runs on a work-stealing pool sized to the hardware minus one.
2. Priorities, highest first: requests the user is waiting on (completion, hover, signature help, definition), semantic tokens of visible documents, diagnostics of open documents, diagnostics of projects of open documents, the workspace index, the idle passes (`lower` and mandatory passes, shader backends).
3. The workspace index elaborates every unit of every project in the workspace once, records its symbols and references, and keeps them while their inputs are unchanged. Progress is reported with `$/progress` (`window/workDoneProgress/create`, title "Indexing"). Requests that need the whole workspace (references, rename, workspace symbols, call hierarchy incoming calls) answer from the index and mark results partial while indexing runs, using partial result tokens when the client supports them.
4. The index persists across server runs in the cache directory, keyed as section 8.4, so a restart is warm.

### 8.8 Memory

The server keeps facts of open documents and of the index. Query results of units with no open document are evicted least recently used when the process exceeds `klf.memoryLimit` (default: half of physical memory, at most 8 GiB). Index facts are stored compactly (interned names, delta-encoded spans) and never evicted. The budget of section 23 bounds the result.

### 8.9 Watched files

The server registers `workspace/didChangeWatchedFiles` for every extension of the language, for every path a project's `watch` names (manifests, `compile_commands.json`), and for the generated outputs another toolchain writes (section 15.2). A watched change increases the revision and re-runs what read the file.

---

## 9. Project model

### 9.1 Purpose

The server must know, for each file, which inputs and flags a `check` would use, so that editor diagnostics equal command-line diagnostics (rule 1.4.1).

### 9.2 Resolution

For each file (open document or file found while indexing):

1. Call `project(session, path)`. A returned `Project` gives its `root`, the uniform driver arguments `arguments` that `<driver> check` takes to analyze it, the files to watch, and a display name.
2. When the hook returns `.None`, the default rule applies: walk up from the file's directory to the nearest directory holding one of the editor spec's `rootMarkers`, else to the nearest workspace folder; the project is that folder with arguments `[folder]` (a directory input means every file with a claimed extension, `KLF-RIDL.md` 5.5); if the file is outside every folder, it is a loose file with arguments `[file]`.
3. A file claimed by several projects (a header included by many C files) belongs to each; editor requests use the project of the most recently focused document that includes it, as clangd does.
4. The server runs `units` over each project's inputs. A file's facts come from the unit containing it.

### 9.3 Configuration

Driver flags declared by the definition are editor settings (Appendix B): `<lang>.flags.<name>`. Their values are appended to every project's arguments as `--<name>` forms, after the project's own arguments, so a setting overrides a project default exactly as a later command-line flag does (`KLF-RIDL.md` 5.5).

### 9.4 Project errors

A project that cannot be formed (an unreadable manifest, a malformed `compile_commands.json`) is reported as a diagnostic on the file that caused it, with the definition's own code, and its files are analyzed as loose files until it is fixed. A project error never stops the server.

---

## 10. Protocol: lifecycle, transport and documents

### 10.1 Transport

JSON-RPC 2.0 over standard input and output with `Content-Length` framing, as LSP specifies. Nothing else may be written to standard output. Logs go to standard error and, when the client enables tracing, to `$/logTrace`. Command-line options of `<driver> lsp`:

| Option | Effect |
| --- | --- |
| `--log <file>` | Also write logs to `<file>` |
| `--log-level error or warn or info or debug or trace` | Default `warn` |
| `--no-index` | Do not build the workspace index |
| `--version` | Print `<driver> <version> (KLF <version>)` and exit |

### 10.2 Lifecycle

`initialize` → `initialized` → requests → `shutdown` → `exit`, as LSP 3.17. Requests before `initialize` get `ServerNotInitialized` (`-32002`). After `shutdown`, every request gets `InvalidRequest` (`-32600`). `exit` ends the process with status 0 after `shutdown`, 1 otherwise.

The `initialize` result carries `serverInfo` `{ name: "<driver>", version: "<version>" }` and the extension field `klf: { version: 1, language: "<name>", family: "program" or "shader", features: [...] }`, listing the `klf/` extensions served.

### 10.3 Capabilities

The server advertises exactly what it serves. A capability depending on a hook is advertised only when the definition overrides the hook (formatting) or when the facts it needs are recorded by the definition (checked by `klf build`, which records which services the definition calls).

| Capability | Advertised when |
| --- | --- |
| `positionEncoding` | Always (section 8.2) |
| `textDocumentSync` | Always: `openClose`, `change: Incremental`, `save: { includeText: false }` |
| `diagnosticProvider` | Client supports pull diagnostics; `interFileDependencies: true`, `workspaceDiagnostics: true` |
| `semanticTokensProvider` | Always: `full: { delta: true }`, `range: true`, legend of section 12 |
| `hoverProvider` | Always |
| `completionProvider` | Always; `triggerCharacters` from the definition's member-access and path operators (section 11.6); `resolveProvider: true` |
| `signatureHelpProvider` | Always; `triggerCharacters` `(` and `,` adjusted to the language's call brackets and separators |
| `definitionProvider`, `declarationProvider`, `typeDefinitionProvider`, `implementationProvider` | Always |
| `referencesProvider`, `documentHighlightProvider` | Always |
| `renameProvider` | Always, `prepareProvider: true` |
| `documentSymbolProvider`, `workspaceSymbolProvider` | Always; workspace symbol `resolveProvider: true` |
| `foldingRangeProvider`, `selectionRangeProvider` | Always |
| `inlayHintProvider` | Always, `resolveProvider: true` |
| `codeActionProvider` | Always; `codeActionKinds` from the definition's actions plus `quickfix` |
| `codeLensProvider` | Always (runnables, shader outputs) |
| `documentLinkProvider` | Always |
| `callHierarchyProvider`, `typeHierarchyProvider` | When the definition records the edges |
| `documentFormattingProvider`, `documentRangeFormattingProvider` | When the definition declares `format` (`KLF-FORMAT.md` section 6) |
| `executeCommandProvider` | Always: the commands of section 17.3 |
| `workspace.fileOperations.willRename` | When `fileRenamed` is overridden |
| `workspace.workspaceFolders` | Always, `changeNotifications: true` |

### 10.4 Document synchronization

1. `didOpen` puts the text in the overlay with its version. `didChange` applies incremental edits in order; an edit whose range is outside the document means client and server disagree on its text. LSP has no request for the full text, so the server logs the error, marks the document out of sync, analyzes the disk text for it, and shows one `window/showMessage` warning asking the user to reopen the file; the next `didOpen` restores it.
2. `didClose` removes the document from the overlay; the disk version is read again. Diagnostics of a closed document are cleared unless `klf.diagnostics.workspace` is on.
3. `didSave` triggers save-time actions: shader build-on-save (section 14.6) and, when configured, the idle passes immediately.

### 10.5 Configuration

The server pulls `workspace/configuration` for the section `<lang>` after `initialized` and after `workspace/didChangeConfiguration`. Settings are Appendix B. `initializationOptions` MAY carry the same object for clients without configuration pull.

---

## 11. Protocol: features

Each feature lists its source of truth and its rules. "Facts" means the facts of section 6 at the request's snapshot.

### 11.1 Diagnostics

1. Source: the diagnostics `report`ed by every query that analyzed the file's unit, plus kernel diagnostics (ownership, verifier keys mapped by the definition, shader validation and targets).
2. Pull (`textDocument/diagnostic`) when the client supports it, with `resultId` per document so unchanged results answer `unchanged`; `workspace/diagnostic` for the index when `klf.diagnostics.workspace` is on. Otherwise push (`textDocument/publishDiagnostics`) with the document `version` the diagnostics were computed from.
3. Mapping: `Error` → 1, `Warning` → 2, `Note` → 3, `Remark` → 4. `code` is the definition's code string; `codeDescription.href` is the spec's `url` with `{code}` replaced; `source` is the driver name (`ksl (msl)` for a shader target diagnostic, section 14.2); `tags` map to `Unnecessary` (1) and `Deprecated` (2).
4. Notes become `relatedInformation` when the client supports it, otherwise they are appended to the message, one per line, as `note: <path>:<line>:<column>: <text>`.
5. A diagnostic in another file of the same unit is published for that file (the pre-KLF server dropped those, section 20.4).
6. A diagnostic whose span is in a virtual file is shown at its origin span with the virtual location in `relatedInformation`.
7. Diagnostics are published at most once per revision per document and never for a revision older than the last published one.

### 11.2 Semantic tokens

1. Source: `classify` facts of the file, latest phase winning (section 6.3), plus kernel modifiers (`inactive`, `unresolved`, ownership modifiers of section 13.3, shader modifiers of section 14.4).
2. Overlapping facts are resolved by keeping the innermost span; the result is a non-overlapping sequence. Multiline tokens are split per line unless the client declares `multilineTokenSupport`.
3. `full/delta` keeps the previous result per document and returns edits against it. `range` answers from the same facts for the visible range.
4. During typing the lexical result (parse facts only) is answered first; when elaboration of the revision finishes, the server sends `workspace/semanticTokens/refresh` so the client asks again and gets refined classes.
5. Custom types and modifiers of section 12 are advertised in the legend; clients that do not know them fall back by the table of section 12.2.

### 11.3 Definition, declaration, type definition, implementation

1. `definition`: the reference fact at the position → its entity → the entity's `selection` span. With no reference fact, the `definition` hook. Overloaded resolution with several candidates (an unresolved overload set) returns all candidates.
2. `declaration`: the `declaration` facts of the entity, else the definition.
3. `typeDefinition`: the entity of the `typeAt` fact at the position.
4. `implementation`: entities with a `typeEdge` to the entity at the position, or with a `reference` of role `Implementation` to it (a method implementing a trait requirement).
5. Locations into the sysroot's `share/<lang>/src/` or `share/<lang>/library/` are returned as `file:` URIs; editors open them read-only by the `klf.readOnly` rule of the generated packages (section 19).
6. Links into a virtual file follow section 6.11.

### 11.4 References and document highlight

1. `references`: every reference fact to the entity in the workspace index, plus the declaration when `includeDeclaration`. Partial while indexing (section 8.7).
2. `documentHighlight`: references to the entity in the current document; `Write` and `ReadWrite` roles → kind `Write`, `Read`/`Call` → `Read`, others → `Text`.

### 11.5 Hover

1. The kernel builds `HoverFacts` from the reference or declaration at the position, the `typeAt` and `valueAt` facts with the innermost span containing it, and the entity's `SymbolInfo`.
2. Default rendering, in Markdown: a code block in the language (`scope`'s last component as the fence language) with `signature` (or the type for a value), then `documentation`, then `valueAt` as "Value: …", then, when the entity is declared in another file, "Declared in `<relative path>`".
3. The kernel adds language-family sections: ownership for program languages (section 13.3), layout and bindings for shader languages (section 14.3).
4. The `hover` hook receives the facts and MAY return text that is appended, or replaces the kernel text when `replaceKernel`.

### 11.6 Completion

1. Trigger characters are declared by the definition through the `KeywordGroup` of class `Operator` with modifier `access` (for example `.` and `::`), plus the `open` of `StringForm`s with modifier `path` for import paths. The default is `.`.
2. Default items (`CompletionList.kernel`): names visible from the innermost `scopeAt` scope containing the cursor (kernel scopes walked with `lookup` rules), each rendered from its entity's `SymbolInfo`; the definition's keywords; snippets are not invented by the kernel.
3. The `complete` hook returns items; when `includeKernel` is true they are merged with the kernel's, the hook's items winning on equal labels and kinds.
4. Items are filtered by the identifier prefix before the cursor (the `identifier` pattern) and sorted by `sortKey`, then by scope distance, then by label. The result is `isIncomplete` when either source says so or when the list was truncated at `klf.completion.limit` (default 200).
5. `completionItem/resolve` fills `documentation` and `additionalEdits` lazily for items that carry an entity.
6. Completion MUST work inside a statement that does not parse yet (`let x = foo.`): this is the case section 7 exists for, and the Kira and KSL corpora contain it.

### 11.7 Signature help

1. The `signature` hook when overridden.
2. Otherwise, the kernel finds the innermost call bracket pair (the first `BracketPair` in `brackets`) enclosing the cursor whose opener follows a reference fact to an entity with `parameters` in its `SymbolInfo`, counts separators (`,` unless the editor spec's operators say otherwise) at bracket depth zero to the cursor, and returns one signature built from `signature` with parameter ranges found by matching the parameter names in it.

### 11.8 Document symbols

Symbols of the file, excluding `local`, nested by `container`, in source order, as `DocumentSymbol` with `range` = `span` and `selectionRange` = `selection`, `detail` = `detail` or `signature`, and `tags: [Deprecated]` when deprecated.

### 11.9 Workspace symbols

Fuzzy match over the index: a query matches a name when its characters appear in order, case-insensitively, with matches at word starts ranked first. Exported symbols rank before non-exported ones; symbols of open projects before others. At most `klf.workspaceSymbols.limit` (default 256) results; `workspaceSymbol/resolve` fills locations lazily.

### 11.10 Code actions

1. Quick fixes from `Diagnostic.fixes`: each fix of each diagnostic overlapping the range is one `quickfix` action titled by the fix text when the definition gave none (`Replace with '<text>'`, `Remove`, `Insert '<text>'`). A diagnostic with exactly one fix marks it `isPreferred`.
2. The `actions` hook adds actions of any kind; `only` filtering of the request is applied to both.
3. `source.fixAll` applies every preferred quick fix of the document whose spans do not overlap.
4. Edits are `WorkspaceEdit` with `documentChanges` and versions when the client supports them.

### 11.11 Rename

1. `prepareRename`: the reference or declaration at the position; refused when the entity is declared in the sysroot, in a read-only path, or only in a virtual file whose text is not a copy of source; returns the selection range and the current name as placeholder.
2. The new name MUST match the editor spec's `identifier` pattern and MUST NOT be a keyword of a non-contextual group; otherwise refused with a message.
3. The `rename` hook may refuse with a reason (shadowing, a name the language reserves).
4. Edits: the declaration's selection, every `declaration` fact, and every reference fact in the workspace index. Rename waits for indexing to finish (progress shown) and fails with a message if a project of the workspace has unresolved errors that leave references unresolved in files mentioning the old name as an identifier; the message names those files. This is stricter than renaming what is known, because a silent partial rename breaks code (DE-011).

### 11.12 Inlay hints

`hint` facts in the requested range, plus kernel hints (sections 13.3, 14.4). Each `HintKind` has a setting (Appendix B); `Type` and `Parameter` are on by default, `Ownership` and `Layout` follow section 13.3 and 14.4. `inlayHint/resolve` fills tooltips from the target entity's documentation.

### 11.13 Folding ranges

Union of: `fold` facts; bracket pairs spanning at least two lines, from token facts of class `Punctuation` matching `brackets`; runs of two or more consecutive line comments and block comments (`comment` kind); `regionMarkers` pairs (`region` kind); consecutive import declarations recorded as `Imports` folds; `inactive` spans (`region`). Ranges are line-based, end line exclusive of the closer's line when the client declares `lineFoldingOnly`.

### 11.14 Selection ranges

For each requested position, the chain of `syntaxNode` spans containing it, innermost first, deduplicated, with the token at the position as the first element and the whole file as the last.

### 11.15 Document links

`link` facts. `File` targets resolve through the overlay; `Entity` targets resolve to the entity's selection; `Url` targets are passed through.

### 11.16 Call and type hierarchy

From `callEdge` and `typeEdge` facts in the index. Incoming calls read the index; outgoing calls read the caller's unit.

### 11.17 Formatting

Specified by `KLF-FORMAT.md`: the definition's `format` hook describes a layout and the kernel's engine prints it. The engine verifies the token sequence while the layout is built (`KLF-FORMAT.md` 5.1 and 5.5). A layout that changes more than whitespace and comment placement is not applied; the request fails with `RequestFailed` and `KLF-LSP-9002`. `<driver> format` performs the same check.

### 11.18 Code lenses and runnables

Each `runnable` fact is a lens titled `▶ Run`, `▶ Test`, `▶ Bench`, `▶ Corpus case` or the shader target list (section 14.5), with command `klf.run` carrying the driver arguments. The kernel adds a reference-count lens (`N references`) on exported symbols when `klf.lens.references` is on (default off). The client runs `klf.run` in a terminal: the generated packages implement the command by starting `<driver> <arguments>` in the editor's terminal with the project root as working directory (section 19).

---

## 12. Token classes

### 12.1 Classes

```
enum TokenClass {
    Comment DocComment String Character Escape Number Boolean Keyword Operator Punctuation
    Namespace Type Struct Class Enum Interface TypeParameter Function Method Macro
    Variable Parameter Property EnumMember Constant Label Annotation Builtin Error
}
enum TokenModifier {
    Declaration Definition Readonly Static Deprecated Abstract Async Modification Documentation DefaultLibrary
    Mutable Consuming Borrowed Unmanaged Control Contextual Injected Inactive Unresolved
    Resource StageIo Access Path
}
```

### 12.2 Mapping

| `TokenClass` | LSP token type | Fallback for clients without the custom type | TextMate scope | Tree-sitter capture |
| --- | --- | --- | --- | --- |
| `Comment` | `comment` | — | `comment.line` / `comment.block` | `@comment` |
| `DocComment` | `comment` + modifier `documentation` | — | `comment.block.documentation` | `@comment.documentation` |
| `String` | `string` | — | `string.quoted` | `@string` |
| `Character` | `string` | — | `string.quoted.single` | `@character` |
| `Escape` | `escapeSequence` (custom) | `string` | `constant.character.escape` | `@string.escape` |
| `Number` | `number` | — | `constant.numeric` | `@number` |
| `Boolean` | `keyword` + `readonly` | — | `constant.language.boolean` | `@boolean` |
| `Keyword` | `keyword` | — | `keyword.other` (`keyword.control` with `Control`) | `@keyword` (`@keyword.control`) |
| `Operator` | `operator` | — | `keyword.operator` | `@operator` |
| `Punctuation` | not sent | — | `punctuation` | `@punctuation.bracket` / `@punctuation.delimiter` |
| `Namespace` | `namespace` | — | `entity.name.namespace` | `@module` |
| `Type` | `type` | — | `entity.name.type` | `@type` |
| `Struct` | `struct` | — | `entity.name.type.struct` | `@type` |
| `Class` | `class` | — | `entity.name.type.class` | `@type` |
| `Enum` | `enum` | — | `entity.name.type.enum` | `@type` |
| `Interface` | `interface` | — | `entity.name.type.interface` | `@type` |
| `TypeParameter` | `typeParameter` | — | `entity.name.type.parameter` | `@type.parameter` |
| `Function` | `function` | — | `entity.name.function` | `@function` |
| `Method` | `method` | — | `entity.name.function.member` | `@function.method` |
| `Macro` | `macro` | — | `entity.name.function.macro` | `@function.macro` |
| `Variable` | `variable` | — | `variable.other` | `@variable` |
| `Parameter` | `parameter` | — | `variable.parameter` | `@variable.parameter` |
| `Property` | `property` | — | `variable.other.member` | `@property` |
| `EnumMember` | `enumMember` | — | `variable.other.enummember` | `@constant` |
| `Constant` | `variable` + `readonly` | — | `variable.other.constant` | `@constant` |
| `Label` | `label` (custom) | `variable` | `entity.name.label` | `@label` |
| `Annotation` | `decorator` | — | `meta.annotation` / `storage.type.annotation` | `@attribute` |
| `Builtin` | `type` + `defaultLibrary` | — | `support.type` / `support.function` | `@type.builtin` / `@function.builtin` |
| `Error` | not sent | — | `invalid.illegal` | `@error` |

Modifiers `Declaration` to `DefaultLibrary` are LSP's own. The rest are custom; clients that do not know a custom modifier ignore it, which LSP permits. Their meanings:

| Modifier | Meaning |
| --- | --- |
| `Mutable` | A `var`, a `borrow mut` parameter, a writable storage resource |
| `Consuming` | A use that moves the value (section 13.3) |
| `Borrowed` | A use through a borrow |
| `Unmanaged` | Code using unmanaged instructions (`KLF-RIDL.md` 11.5) |
| `Control` | Control-flow keyword |
| `Contextual` | A contextual keyword (`handle`, `borrow`) |
| `Injected` | Text that came from a macro or generator |
| `Inactive` | Excluded by a preprocessor or build condition |
| `Unresolved` | A name that resolved to nothing |
| `Resource` | A shader resource (uniform, storage, texture, sampler) |
| `StageIo` | A stage input or output member |
| `Access` | Member access operator (completion trigger) |
| `Path` | A string holding a path (import, `ksl!`, `#include`) |

### 12.3 Stability

The legend is fixed per KLF release and listed in the `initialize` result. New classes and modifiers are appended; indices never change.

### 12.4 Symbol kinds beyond LSP

| `SymbolKind` | LSP `SymbolKind` | `CompletionItemKind` |
| --- | --- | --- |
| `Macro` | `Function` (12) | `Function` |
| `Trait` | `Interface` (11) | `Interface` |
| `Resource` | `Variable` (13) | `Variable` |
| `Stage` | `Function` (12) | `Function` |
| `Label` | `Key` (20) | `Reference` |
| `Test` | `Method` (6) | `Method` |

---

## 13. Program languages

### 13.1 Ownership diagnostics in the editor

The mandatory KIR passes (`KLF-RIDL.md` 11.6) report ownership errors through keys (`own.use-after-move`, `own.borrow-escape`, `init.use-before-init`, ...) that the definition maps to its codes. They run after `lower`. The server runs `lower` and the mandatory passes for every unit whose `elaborate` reported no error, at idle priority, and on save immediately. Their diagnostics are published with the unit's other diagnostics. KIR spans come from the builder's source spans, so these diagnostics point into source.

### 13.2 Why this is the kernel's

No definition can compute these facts: they are the result of the kernel's ownership passes over the definition's KIR. A Kira user sees "use after move" while typing, from the same pass that rejects the program in a build.

### 13.3 Ownership facts

The mandatory passes know, for each use of a value, whether it is a move, a copy, a borrow or a mutable borrow, and where each value is destroyed. In editor mode they record:

1. Token modifiers `Consuming` on moving uses and `Borrowed` on borrowed uses, `Mutable` on mutable borrows.
2. Inlay hints of kind `Ownership`: `copy` before an implicitly inserted copy (a `Copyable` struct bound by `var w = v`, `KLF-RIDL.md` 27.3), `move` before an argument passed by move when the language makes moves implicit, and, at the closing bracket of a scope, `drop x` for each value with a non-trivial destructor destroyed there. Defaults: `copy` and `move` hints on, `drop` hints off (`klf.inlayHints.ownership.*`).
3. Hover sections: the type's policy record in plain words ("Copied implicitly", "Moves on binding", "Reference counted, copy-on-write", "Destroyed by `drop`"), and the parameter convention of a function parameter (`owned`, `borrow`, `borrow mut`).

A definition does nothing for these facts beyond lowering with source spans.

### 13.4 RIDL facts

1. Hover on an exported declaration (`@Export` in Kira, RIDL export in KArc and C) shows its RIDL declaration as `<driver> ridl emit` would write it, from the same emitter.
2. Diagnostics `ridl.not-exportable`, `ridl.abi-broken` and the other RIDL keys appear in the editor through the definition's key mapping, like every key.
3. A code lens on a module with `abi/` records shows the current ABI UUID and "ABI changed since freeze" when `klf abi freeze` would write a record.

### 13.5 File modes

The `klf/fileInfo` request (section 17) returns a file's selected file mode, effective memory model and convention (`KLF-RIDL.md` 7.3). Generated packages show it in the status bar, so a KArc user sees whether a file compiles in ARC mode.

### 13.6 Macros and compile-time evaluation

1. Expansions are virtual files (`KLF-RIDL.md` 8.1). `klf/expand` returns the expansion at a position as a virtual document.
2. Facts recorded while elaborating expanded code belong to the virtual file; tokens copied from macro arguments map back (section 6.3 rule 4).
3. Compile-time evaluation results are `valueAt` facts and appear in hover.
4. Compile-time evaluation runs with `EvalLimits` in the editor as in builds; a limit hit is the same diagnostic. Evaluation polls cancellation (section 8.6) like definition code.

### 13.7 KIR view

`klf/kir` lowers the unit containing the position, runs the mandatory passes, and returns the KIR text (`KLF-RIDL.md` 10.4) of the function at the position, raw or canonical, as a virtual document. It is for language and kernel developers; generated packages expose it as a command, not in menus.

---

## 14. Shader languages

### 14.1 Pipeline in the editor

For each shader unit whose `elaborate` reported no error, at idle priority and on save: `lower`, validation (`KLF-KSL.md` 16), the mandatory passes, binding assignment (`KLF-KSL.md` 20), and every backend in `<lang>.targets` (default: the definition's `targets`), producing output in memory only. Shader units are small; the budget is section 23.

### 14.2 Per-target diagnostics

1. Validation diagnostics (`KLF-KSL.md` 16.4 keys, mapped by the definition) are target-independent, except `shader.uniformity`, whose severity depends on whether `wgsl` is among the targets (`KLF-KSL.md` 16.5).
2. Backend refusals (`shader.target-unsupported`, `shader.capability`, `shader.slot-collision`) carry the target. The diagnostic's `source` is `<driver> (<target>)` and the message names the target, so a user sees "MSL can express this, HLSL cannot" in place.
3. The reference validators of `KLF-KSL.md` 25.2 are not run in the editor; they test backends, not user code.

### 14.3 Hover

| At | Shows |
| --- | --- |
| A resource | Group and class, neutral `(group, binding)`, the slot on every configured target (`[[buffer(2)]]`, `register(t1)`, `binding = 3`, `@group(1) @binding(0)`), stage visibility, paired sampler (`KLF-KSL.md` 20.4), Metal length slot, and the authored slot when one exists |
| A host-shareable struct or member | Layout rule, size, alignment; per member offset, size, alignment; uniform binding size when used as a uniform (`KLF-KSL.md` 21.1) |
| A built-in | Its spelling on every target (`KLF-KSL.md` Appendix C) |
| An option (override) | Type, default, id, and whether each target keeps or bakes it (`KLF-KSL.md` 14.3) |
| A stage interface member | Its location and interpolation |
| A sample call | The level it lowers to (`KLF-KSL.md` 22.4, DS-009) |

All of this comes from the module, the binding assignment and reflection the kernel already computed, not from the definition.

### 14.4 Layout hints and tokens

1. Inlay hints of kind `Layout` after each member of a host-shareable struct: `offset 16 · size 4`, and a separate hint `12 bytes padding` where a gap precedes a member. Default on for structs reached from a uniform or storage resource (`klf.inlayHints.layout`).
2. Token modifiers: `Resource` on resource names, `StageIo` on interface members, `Mutable` on writable storage, `Builtin` class on built-in names and functions.

### 14.5 Output views

1. A code lens on each shader declaration lists the configured targets plus `KSIR` and `Reflection`. Each opens a virtual document through `klf/shaderOutput` (section 17): the emitted source for MSL, HLSL, GLSL and WGSL with the matching language id, SPIR-V as disassembly text (the `klf-shader-spirv` disassembler, section 26.1), KSIR text (`KLF-KSL.md` 18), and the `KSLR1` reflection text.
2. The views follow the revision: the client refreshes them on `klf/shaderOutputChanged` notifications.
3. A view of a target that refused the shader shows the refusal diagnostics instead of output.

### 14.6 Build on save

When `<lang>.buildOnSave` is on (default on when the file's project has an `output`, section 21.3), saving a file runs `<driver> build <project inputs> --out <output>`, through the same code path as the command line, writing each output atomically (write to a temporary file in the same directory, then rename). Outputs are the files of `KLF-KSL.md` 24.2. A failed build writes nothing (`KLF-KSL.md` 5.7) and its diagnostics are already shown.

This is what keeps a host language's server current: the Kira server watches `generated/shaders/` (section 15.2).

---

## 15. Across toolchains

### 15.1 Several servers

A workspace with Kira and KSL sources runs two servers, `kira lsp` and `ksl lsp`, started by the editor for their extensions. Servers never talk to each other. Everything that crosses goes through files, as builds do (`KLF-KSL.md` 24.2, DE-009).

### 15.2 Kira and KSL

1. The Kira definition records a `link` fact on the path argument of `ksl!("...")`, to the `.ksl` file.
2. The Kira definition's reads of `generated/shaders/<Stem>.*` (`ksl-artifacts.klfd`) go through `Session.input`, so they are query dependencies. The Kira server watches `generated/shaders/` (section 8.9); when `ksl lsp` builds on save, the Kira query re-runs and diagnostics update.
3. A missing or stale output is the Kira definition's existing diagnostic. Its fix is a code action with command `klf.run` and arguments `["ksl", "build", "<shader dir>"]`, which the generated package runs with the `ksl` driver it finds (section 19.6). A definition names another driver only through this command.
4. Hover on `ksl!("...")` shows the shader's resources from the `.resources` digest the Kira definition already reads.

### 15.3 Program languages through RIDL

1. With `--ridl-source`, an emitted `.ridl` declaration carries `@Source("<path>:<line>:<column>")` naming the declaration in the providing language (amendment to `KLF-RIDL.md` 20.7). Paths are relative to the `.ridl` file; the annotation is excluded from the canonical text (`KLF-RIDL.md` 24.6), so it never changes an ABI hash.
2. A program server resolves a call into an imported RIDL module to the `.ridl` declaration it was checked against. `klf lsp` serves the `.ridl` file and turns `@Source` into a document link and a definition target. Navigation from Kira into a C function therefore goes Kira → `.ridl` → C source in two steps across two servers, with no server knowing the other language.
3. Drivers run by servers for indexing pass `--ridl-source` implicitly when they emit `.ridl` for workspace modules.

---

## 16. Kernel languages: `klf lsp`

### 16.1 Scope

`klf lsp` is a server for the kernel's own file types, using the same host (`klf-ide`) with native Rust analyzers in place of a compiled definition. One process serves all of them.

| Type | Analysis | Notable features |
| --- | --- | --- |
| `.klfd` | The KLF Kira frontend (`klf-kira-syntax`, `klf-kira-lower`) in editor mode, with the definition package as the project (`KLF-RIDL.md` 7.1) | Diagnostics (`KLFK-E`, `KLF-DEF`); navigation into the prelude (`prelude/*.klfd`, opened read-only from the KLF distribution); hover on kernel services showing their `services.manifest` signature; completion of `session.` services and family members; a quick fix "Add the required members" on a declaration extending `Language`, `ShaderLanguage` or `Corpus` that misses `@Required` members; runnables for corpus cases (`klf test <defdir> --corpus <name>`) |
| `.kir` | Parser and verifier (`klf-kir`, `klf-kir-verify`) | Verifier rule diagnostics (V1 to V12) at the instruction; navigation for `%values`, blocks, functions, globals and types; hover with types |
| `.ksir` | Parser and validator (`klf-shader`, `klf-shader-valid`) | SV rule diagnostics; navigation for handles; hover with types and layouts |
| `.ridl`, `.ridlsig` | `klf-ridl` parser and checker | Diagnostics of `klf ridl check`; `@Source` links (section 15.3); hover with layout and symbol names (`KLF-RIDL.md` 21, 24.2); `.ridlsig` read-only |
| `.klffi` | `klf-klffi` | Diagnostics; hover with C layouts per target |

### 16.2 The KLF Kira frontend in editor mode

The KLF Kira frontend records facts through the same fact store, natively in Rust, and follows section 7. Because definitions are large (the Kira definition has more than a hundred `.klfd` files), it parses per file in sub-queries and elaborates per package, with the latency budget of section 23.

### 16.3 Editor packages for kernel languages

The KLF distribution ships the generated packages of section 19 for `klf lsp` under `klf/share/editor/`, built from editor specs written in the kernel for each kernel file type.

---

## 17. Protocol extensions

### 17.1 Rules

Extension methods are named `klf/<name>`. A client that does not implement an extension loses only that extension. Every extension is listed in `initialize`'s `klf.features`. Request and result shapes below are JSON.

### 17.2 Requests and notifications

| Method | Direction | Params | Result |
| --- | --- | --- | --- |
| `klf/virtualFile` | client → server | `{ uri }` with scheme `klf-virtual` | `{ text, languageId }` |
| `klf/expand` | client → server | `{ textDocument, position }` | `{ uri, name }` of a `klf-virtual` document, or null |
| `klf/kir` | client → server | `{ textDocument, position, canonical: bool }` | `{ uri }` of a `klf-virtual` document, or null |
| `klf/fileInfo` | client → server | `{ textDocument }` | `{ project, fileMode, memory, convention, unit }` |
| `klf/shaderOutput` | client → server | `{ textDocument, shader, target, stage, kind: "source" or "ksir" or "reflection" }` | `{ uri, languageId }` |
| `klf/shaderOutputChanged` | server → client | `{ uris: [...] }` | — |
| `klf/runnables` | client → server | `{ textDocument, position? }` | `[{ label, kind, arguments, cwd, range }]` |
| `klf/status` | server → client | `{ health: "ok" or "warning" or "error", message, indexing: bool }` | — |
| `klf/reloadProjects` | client → server | `{}` | `null` |

`klf-virtual:` URIs are opaque to clients; the server keeps their content per revision. Clients without support for the scheme fall back as section 6.11 states.

### 17.3 Commands

`workspace/executeCommand` commands: `klf.run` (arguments: driver, driver arguments, working directory; executed by the client, the server only reports it as a lens or action command), `klf.reloadProjects`, `klf.showOutput` (opens a shader output view), `klf.restartServer` (client-side). Commands the client executes are implemented by the generated packages (section 19); a generic LSP client shows them as unsupported, which loses only those buttons.

---

## 18. Command-line parity

### 18.1 Commands

| Command | Same computation as |
| --- | --- |
| `<driver> check [paths] --diagnostics=json` | Editor diagnostics of the same projects |
| `<driver> format [paths] [--check] [--stdin NAME] [--width N] [--tab-size N] [--tabs]` | `textDocument/formatting` and `rangeFormatting` (`KLF-FORMAT.md` section 6). `--check` exits 1 if any file would change |
| `<driver> symbols [paths] [--json]` | Workspace symbols: one line per symbol, `<path>:<line>:<column> <kind> <container>.<name>` |

### 18.2 Parity test

For every corpus case of a definition (`KLF-RIDL.md` 31.1, `KLF-KSL.md` 25.3), the conformance runner opens the case's files in a server, waits for analysis to settle (the idle passes included), and compares the published diagnostics with `<driver> check --diagnostics=json` over the same files. They MUST be identical as sets (code, severity, span, message, notes, fixes, tags). This test is part of `klf test` for every definition (section 24.4 item 3).

---

## 19. Generated editor packages

### 19.1 Principle

`klf build` writes `share/<lang>/editor/` into the sysroot (amendment to `KLF-RIDL.md` 5.4). Everything in it is derived from the definition's `name`, `driver`, `extensions`, `editor`, `flags` and `version`, and from this document. Nothing is typed by hand per language (rule 1.4.6). `klf editor` regenerates and installs the packages.

```
share/<lang>/editor/
  manifest.json                        language, driver, extensions, legend, versions
  textmate/<lang>.tmLanguage.json
  language-configuration.json          VS Code format
  tree-sitter/                         lexical grammar (or a copy of the rich grammar at the pinned revision)
    src/grammar.json  src/parser.c  src/scanner.c  src/tree_sitter/*.h
    queries/highlights.scm  brackets.scm  indents.scm  folds.scm  outline.scm  textobjects.scm  injections.scm
  vscode/<lang>-<version>.vsix
  zed/                                 extension source: extension.toml  languages/<lang>/*  src/lib.rs  Cargo.toml
  neovim/<lang>.lua
  helix/languages.toml  helix/queries/<lang>/*.scm
  emacs/<lang>-mode.el
```

### 19.2 Commands

| Command | Effect |
| --- | --- |
| `klf editor gen <defdir or sysroot> --out <dir> [--editor <names>]` | Write the tree above. `--editor` takes a comma list of `vscode`, `zed`, `neovim`, `helix` and `emacs`, or `all`; without it the VS Code and Zed packages are written. `manifest.json`, the TextMate grammar and `language-configuration.json` are always written, and `tree-sitter/` when a chosen editor uses it |
| `klf editor install <sysroot> --editor vscode or zed or neovim or helix or emacs [--prefix <dir>]` | Install into the user's editor configuration: VS Code through `code --install-extension`, Zed as a dev extension directory, Neovim and Emacs into their configuration directory, Helix by merging `languages.toml` and copying queries. Never overwrites a file it did not write; reports conflicts |
| `klf editor check <dir>` | Run the checks of section 24.4 items 5 to 7 on a generated tree |

### 19.3 The lexical grammars

1. **TextMate.** One rule per editor spec construct: comments (with nesting through `begin`/`end` recursion when `nests`), strings with escapes and interpolation (interpolation includes `$self`), numbers, keyword groups as `\b(?:...)\b` alternations sorted longest first, annotations, preprocessor lines, operators, punctuation, identifiers followed by the first call bracket as `entity.name.function` (a lexical approximation that semantic tokens correct). Patterns are translated from Appendix C to Oniguruma.
2. **Tree-sitter.** A grammar whose root is a sequence of items: comment, string, number, keyword (one rule per keyword group), operator, identifier, annotation, and one rule per bracket pair holding a nested sequence. Nesting block comments, raw strings and interpolation use a generated external scanner (`scanner.c`). The kernel generates `parser.c` with the `tree-sitter-generate` crate, so no Node.js or tree-sitter CLI is needed (DE-007). Queries map rules to the captures of section 12.2; `brackets.scm`, `indents.scm` and `folds.scm` come from bracket pairs; `outline.scm` is empty for lexical grammars (outline comes from the server).
3. **Language configuration.** Comments, brackets, auto-closing and surrounding pairs, `wordPattern` from `identifier`, indentation and on-enter rules, folding markers.

Lexical grammars are approximations by design: they cannot know that a name is a type. They MUST agree with the definition's lexer on every token that the lexer classifies without context (section 24.4 item 5).

### 19.4 Rich grammars

When `editor.grammar` names a repository, revision and path, tree-sitter packages use that grammar instead: Zed's `[grammars]` entry and Helix's `grammar` source point at it, and the queries shipped are the grammar repository's. The generated lexical grammar is still written, for editors and tests. A rich grammar MUST parse every accepted file of the language's corpus without `ERROR` or `MISSING` nodes (section 24.4 item 6), so it cannot silently lag the language.

### 19.5 VS Code

1. The `.vsix` is written by the kernel directly (an OPC zip with `extension.vsixmanifest` and `[Content_Types].xml`); no Node.js or `vsce` is needed at generation time.
2. `package.json` contributes the language (id = `name`, extensions, file names, first-line pattern, configuration, icon), the TextMate grammar, the settings of Appendix B with types and defaults (driver flags included, from `flags`), the semantic token types and modifiers of section 12 with `semanticTokenScopes` fallbacks, and the commands of section 17.3.
3. The client is a fixed JavaScript file shipped in the KLF distribution, built once per KLF release from `vscode-languageclient`, parameterized at run time by `manifest.json`. It implements server discovery (section 19.6), `klf.run` in a terminal, `klf-virtual:` documents through a `TextDocumentContentProvider`, shader output views, the status bar item from `klf/status` and `klf/fileInfo`, and read-only presentation of sysroot sources.

### 19.6 Server discovery

Every generated package finds the server in this order, and reports which tier it used in its log:

1. The setting `<lang>.server.path` (with `<lang>.server.arguments`).
2. The environment variable `KLF_<LANG>_SYSROOT` (upper-case language name): `<sysroot>/bin/<driver>`.
3. `<driver>` on the `PATH` of the workspace's shell environment (Zed: `worktree.which`; VS Code: the process environment augmented by the login shell on macOS; Neovim, Helix, Emacs: their own `PATH`).
4. Failure: a message naming the three tiers and the setting to set. The package never downloads a toolchain.

The same discovery serves `klf.run` for other drivers (`ksl` from a Kira project, section 15.2): setting `<lang>.drivers.<driver>`, then `KLF_<LANG>_SYSROOT` of that driver's language, then `PATH`.

### 19.7 Zed

`extension.toml` declares the language, the grammar (lexical grammar from a path in the sysroot for local installs, or the rich grammar's repository), and the language server `<lang>-lsp`. `src/lib.rs` implements `language_server_command` with the discovery of section 19.6 (settings through `LspSettings::for_worktree`, then the environment, then `worktree.which`), and passes settings as `initializationOptions` and `workspace/configuration`. `languages/<lang>/config.toml` carries comments, brackets, `autoclose_before`, tab size from `indent.unit`, and `path_suffixes`.

### 19.8 Neovim, Helix and Emacs

1. **Neovim** (0.11 and later): `<lang>.lua` registers the filetype (`vim.filetype.add`), a server configuration through `vim.lsp.config` and `vim.lsp.enable`, root markers from the project hook's `watch` file names when the definition declares them (`editor.fileNames`), the tree-sitter parser for `nvim-treesitter` (path or repository), and user commands for the `klf/` extensions that need no UI (`:KlfExpand`, `:KlfKir`, `:KlfShaderOutput` open scratch buffers).
2. **Helix**: a `[[language]]` entry (name, scope, file types, roots, comment tokens, indent, language servers) and a `[language-server.<lang>-lsp]` entry; the grammar source; queries.
3. **Emacs** (29 and later): `<lang>-mode.el` defines a major mode on `treesit` when the grammar is installed and on font-lock from the keyword groups otherwise, registers the server with `eglot-server-programs`, and sets comment syntax and indentation.

### 19.9 Versions

A package's version is the definition's `version`. The packages declare the minimum KLF server protocol version they need (`manifest.json`); a server older than that is reported by the client with the versions involved.

---

## 20. Language profile: Kira

### 20.1 Editor spec

| Member | Value |
| --- | --- |
| `displayName` | `Kira` |
| `scope` | `source.kira` |
| `lineComments` | `//` |
| `blockComments` | `/*` `*/`, nesting |
| `strings` | `"`…`"`, escapes `\n \t \r \e \0 \" \\` and a line break (continuation), single-line otherwise |
| `numbers` | Decimal integers and decimals as the Kira lexer reads them |
| `identifier` | `[A-Za-z_][A-Za-z0-9_]*` (ASCII, as the lexer) |
| `keywords` | Reserved: the words of the lexer's keyword table (`definitions/syntax/token.klfd`), with `if else while for in match attempt try return break continue` in a `Control` group and `true false` as `Boolean`. Contextual: `handle borrow mut move copy self` |
| `brackets` | `{}` `()` `[]` |
| `annotation` | `@[A-Za-z_][A-Za-z0-9_]*` |
| `indent` | Four spaces; increase after a line ending in `{`, `(` or `[`; decrease at a line starting with a closer |
| `grammar` | The Kira tree-sitter grammar, at the revision the Kira repository pins (`kira-tree-sitter`), while it passes section 19.4. Otherwise the lexical grammar |

The keyword table and the editor spec MUST be one value in the definition: the lexer reads `editor.keywords`, or a definition test asserts they are equal. Section 24.4 item 1 checks the result.

### 20.2 Project

`project` walks up from the file to the nearest directory holding `package.kira`, as `kira build` does, and returns that directory with arguments `[<package dir>]` and `watch: ["package.kira", "generated/shaders/**"]`. Files outside every package are loose files (the oracle analyzed them alone too). Dependencies resolved by the manifest loader are part of the unit, so navigation reaches them; files of dependencies outside the workspace are opened read-only.

### 20.3 Facts the definition records

| Phase | Facts |
| --- | --- |
| Lexer | `classify` for every token and comment, in one `classify_many` call per file |
| Parser | `declareSymbol` for functions, structs, enums and variants, classes, constructs and their members, traits, type aliases, distinct types, globals, imports; `syntaxNode` for every expression, statement, block and declaration; `fold` for import groups; `runnable` for `@Main` and `@Test` functions |
| Macro phase | Virtual files for expansions with origin chains; `injected` tokens |
| Semantic analysis | `reference` for every resolved name, member, method, variant, label and import; `unresolved` for failures; `typeAt` for bindings without annotations and for every expression it types; `valueAt` for constants and `comptime` results; `hint` for parameter labels at call sites without labels; `callEdge` for resolved calls; `typeEdge` for `extends`, trait conformances and `extend` blocks; `link` for import paths and `ksl!` paths; refined `classify` for names |
| Kernel (mandatory passes) | Ownership modifiers and hints (section 13.3) |

### 20.4 Against the pre-KLF server

| Pre-KLF behavior | KLF behavior |
| --- | --- |
| Full-text sync | Incremental sync |
| Imports read from disk, unsaved buffers invisible to other files | Overlay: unsaved buffers are what imports see (section 8.1) |
| Diagnostics in other files of the program dropped | Published for those files (section 11.1 rule 5) |
| Hover shows the declaration's source line | Hover shows signature, type, documentation, value, ownership (section 11.5) |
| Completion from links and the declaration tree, no member completion | Scope completion from facts plus the definition's member completion through `complete` |
| Single-threaded, no cancellation | Worker pool, revision cancellation (section 8) |
| Diagnostics, definition, declaration, hover, completion | Every feature of section 11 |
| Hand-written tree-sitter grammar and TextMate grammar, kept in sync by a repository rule | Generated lexical grammars; the rich grammar is checked against the corpus (section 19.4) |

`kira-language-server` and `kira/editors/` are retired at milestone E5 (section 26.2), when the KLF server passes the parity suite of section 24.5.

### 20.5 Member completion

The Kira definition overrides `complete` for member access (`value.`), enum variants after `.` in a typed context, labeled arguments inside a call, and import names after `import`. It uses `typeAtPosition` and `scopeAtPosition` (section 6.12) and its own type tables. All other completion is the kernel's.

---

## 21. Language profile: KSL and MSL

### 21.1 KSL editor spec

| Member | Value |
| --- | --- |
| `displayName` | `KSL` |
| `scope` | `source.ksl` |
| `lineComments`, `blockComments` | As the KSL lexer: `//`, `/* */` |
| `keywords` | Reserved: `import as type const enum function shader group option let`, `if else while return` in a `Control` group, `true false` as `Boolean`. Contextual (identifiers the parser gives meaning by position): `uniform storage texture sampler read write read_write vertex fragment compute input output threads` |
| Built-ins | KSL types (`Float`, `Float2` … `Float4x4`, `Texture2d`, `Sampler`, …) and functions (`mul`, `sample`, `load`, `store`, `atomicAdd`, the math functions of `KLF-KSL.md` 22.4) as a `Builtin` keyword group |
| `annotation` | `@builtin`, `@interpolate`, `@binding` |
| `brackets` | `{}` `()` `[]` |

The lists are the KSL definition's lexer and built-in tables; the table above names them for the reader, the definition is the source.

### 21.2 KSL facts and features

1. Parser and checker facts as Kira's (section 20.3), with symbol kinds `Resource` for resources, `Stage` for stage blocks, `Struct` for types, `Constant` for options and constants.
2. The kernel features of section 14: per-target diagnostics, resource and layout hovers, layout hints, output views.
3. Completion: inside a group, resource kinds; after `@builtin(`, the built-ins legal in that stage and direction; after `texture read_write name:`, the storage texture types; member completion on struct values. The KSL definition overrides `complete` for these.
4. Runnables: a `Shader` runnable per shader that runs `ksl build <file> --target <t> --emit source` for each target, for users without the output views.

### 21.3 KSL project

`project` returns the nearest package root holding a `Shaders/` directory that contains the file (`KLF-KSL.md` 24.2) with arguments `[<root>/Shaders]`, `output: <root>/generated/shaders`, and `watch: []`. Builds on save write there (section 14.6). A `.ksl` file elsewhere is a loose file, checked, never built on save.

### 21.4 MSL

1. Editor spec: C++ comments and literals, the MSL keywords and attribute syntax `[[...]]` as an annotation pattern, `#` preprocessor lines, brackets including `[[ ]]` for attributes.
2. Facts: inactive preprocessor regions (`inactive`), macro expansions as virtual files, the accepted subset's symbols and references, refusals of constructs outside the subset (`KLF-KSL.md` 23.4) as diagnostics at the construct.
3. Hover on a `[[buffer(n)]]` resource shows the neutral binding and every target's slot (section 14.3), which is how an MSL author sees what SPIR-V gets.

---

## 22. Language profiles: C and KArc

### 22.1 Project

`project` reads `compile_commands.json` from the workspace root or the directory named by the setting `<lang>.compilationDatabase`, finds the entry for the file, and returns its arguments normalized by `compatDriver` (`KLF-RIDL.md` 5.5). A header not in the database takes the arguments of the most recently opened source file that includes it (section 9.2 rule 3), else those of a source file in the same directory, else the defaults. `watch` includes the database file.

### 22.2 Header caching

A translation unit's preprocessing of its includes is cached per header and macro state (`KLF-RIDL.md` 9.2). In the editor, a preamble query covers the leading `#include` block of the open file, so an edit below the includes re-parses only the file's own text. This is what makes the UE5 budget of section 23 reachable.

### 22.3 Facts

Inactive regions (`inactive`), macro definitions and uses (`Macro`), declarations versus definitions (`declaration` facts for prototypes and forward declarations), overload sets (references to the picked candidate; hover lists the set), template instantiations as virtual files when expanded through `klf/expand`. KArc adds the file mode in `klf/fileInfo` and ownership facts in ARC files (section 13.3).

---

## 23. Performance budgets

### 23.1 Reference conditions

Measured on the reference machines of `KLF-RIDL.md` 9.3 with a warm disk cache and a warm server (index built), p95 over the edit scripts of the conformance suite.

### 23.2 Budgets

| Measure | Budget |
| --- | --- |
| Edit to lexical semantic tokens of the edited file, 10,000-line file | 30 ms |
| Edit to syntax diagnostics published | 100 ms after debounce |
| Edit to semantic diagnostics published, one-file edit in a 100,000-line Kira package | 400 ms after debounce |
| Edit to semantic diagnostics, one edit in a UE5 `.cpp` below its includes | 1.5 s after debounce |
| Completion response | 100 ms |
| Hover, definition, document highlight | 50 ms |
| References, rename of a symbol with 1,000 uses, index complete | 500 ms |
| Shader: edit to per-target diagnostics for a 500-line KSL file, five targets | 300 ms after debounce |
| Cold start to first diagnostics of the open file, Kira ecosystem package | 2 s |
| Cancellation: time from a write to a running query releasing its worker | 20 ms |
| Peak memory | 1.5 times the peak of `<driver> check` over the same projects |

### 23.3 Regressions

The conformance runner records every budget per commit. A change that exceeds a budget fails; a change that slows a measure by more than 10 percent while staying in budget is reported in the change's review.

---

## 24. Conformance

### 24.1 Kernel suites

| Suite | Content |
| --- | --- |
| Protocol | Scripted JSON-RPC sessions against the toy definition (`klf-toy-calc` extended with an editor spec and facts), one per capability of section 10.3 and per lifecycle and error rule of section 10, with golden transcripts |
| Encodings | UTF-8, UTF-16 and UTF-32 negotiation; surrogate pairs, combining characters, CRLF, lone CR, invalid UTF-8, positions past line ends |
| Synchronization | Random incremental edit sequences applied by a reference client; the overlay MUST equal the reference text after every edit |
| Cancellation | A definition with long loops and deep recursion; writes during every phase; every query releases its worker within the budget; heap balance of `klf-rt` holds after each cancellation; traps in definition code produce `KLF-LSP-9001` and the server keeps answering |
| Facts | Golden facts per toy corpus file; replay after cache reuse equals fresh computation; facts are absent and no allocation happens in `Build` mode |
| Features | For each feature of section 11, golden results on the toy corpus |
| Editor packages | Generation of every package for the toy definition and for every in-tree definition; items 5 to 7 of 24.4 |

### 24.2 Definition suites

Each definition ships an editor corpus (Appendix A) under `corpus/editor/`. `klf test` runs it with the items of 24.4.

### 24.3 Oracle

The pre-KLF Kira server is the Kira oracle (section 24.5). For other languages the oracle is the language's corpus and the parity of section 18.2.

### 24.4 What `klf test` checks for a definition

1. **Keywords.** Each word of every non-contextual keyword group, lexed alone, yields one token classified with its group's class; no other identifier-shaped word in the definition's lexer tables is classified as a keyword without being listed.
2. **Editor corpus.** Every case of Appendix A passes.
3. **Parity.** Section 18.2 for every corpus case.
4. **Tolerance.** For every accepted corpus file: every token-boundary prefix of files up to 2,000 tokens (a random sample of 2,000 prefixes for larger files), and 200 seeded random single-token deletions, insertions and substitutions per file: no internal error, no trap, parse and elaborate complete within the budget scaled to file size, at least one diagnostic for every modified file that the unmodified definition rejects. Cascade rule: for single-token mutations, the number of diagnostics reported is at most 3 for 95 percent of mutations. Fact stability: for single-token mutations inside one top-level declaration, the document symbols of every other top-level declaration are unchanged.
5. **Lexical grammars.** The TextMate grammar (run by a TextMate engine in the test-only crate) and the tree-sitter lexical grammar classify every context-free token of the corpus (comments, strings, numbers, keywords, operators, punctuation) as the definition's lexer does.
6. **Rich grammar.** When declared: every accepted corpus file parses without `ERROR` or `MISSING` nodes.
7. **Packages.** Every generated package is well-formed: the `.vsix` opens and its `package.json` validates against the VS Code manifest schema; Zed's extension compiles for the WebAssembly target its `zed_extension_api` version requires when a Rust toolchain is present on the CI host; Neovim's Lua and Emacs Lisp parse; Helix's TOML parses.

### 24.5 Kira parity with the pre-KLF server

Over `tests-kik` and the Kira ecosystem packages, at every identifier occurrence:

1. Where the oracle answers `definition` or `declaration`, the KLF server answers with the same location.
2. Where the oracle's hover is non-empty, the KLF server's is non-empty and contains the declaration's name.
3. Where the oracle offers a completion item, the KLF server offers an item with the same label.
4. Diagnostics on every file: codes and lines equal (`KLF-RIDL.md` 31.1), allowing the additional diagnostics the oracle dropped (section 20.4).

### 24.6 Editor smoke tests

On CI hosts with the editors installed, a headless session per editor (VS Code through `@vscode/test-electron`, Neovim headless, Helix, Emacs batch mode) opens a corpus file, waits for diagnostics, requests hover and definition, and checks the results. Zed is tested by loading the dev extension in a headless Zed instance where available. These tests run in the editor integration job only, not in the kernel's unit suites.

---

## 25. Diagnostics

| Code | Meaning |
| --- | --- |
| `KLF-LSP-0001` | Invalid editor setting value; names the setting and the expected type |
| `KLF-LSP-0002` | A project could not be formed; shown on the file that caused it when the definition did not report its own code (section 9.4) |
| `KLF-LSP-0003` | A client capability required by a feature is missing; logged once |
| `KLF-LSP-0004` | Server and generated package protocol versions are incompatible |
| `KLF-LSP-9001` | A trap in definition code during editor analysis (section 8.6), naming the definition, the query and the trap code |
| `KLF-LSP-9002` | A formatter layout changed more than whitespace and comment placement (`KLF-FORMAT.md` 5.1, 5.5) |
| `KLF-LSP-9003` | A fact names a span outside its file or an entity that does not exist; the fact is dropped |
| `KLF-DEF-0040` | An editor spec pattern is outside the dialect of Appendix C (at `klf build`) |
| `KLF-DEF-0041` | An editor spec is inconsistent: a mode extension or keyword listed twice, a bracket pair with equal sides, an auto-close pair without a matching bracket or string form |

`9xxx` codes are internal errors that blame a definition or the kernel, as in `KLF-RIDL.md` 31.4. They are logged with the query key and shown in the editor once per query key as a `window/showMessage` warning, not as diagnostics in the user's file.

---

## 26. Implementation plan and crate layout

### 26.1 Crates

| Layer (`KLF-RIDL.md` 32.1) | Crate | Content |
| --- | --- | --- |
| 1 | `klf-facts` | Fact model, per-query recording and replay, compact index storage |
| 1 | `klf-text` | Line index, position encodings, incremental edit application |
| 5 | `klf-services` (module `editor`) | The services of section 6 and their ABI entries |
| 5 | `klf-ide` | Overlay, revisions, snapshots, project model, scheduler, default features of section 11, kernel hovers and hints of sections 13 and 14 |
| 6 | `klf-lsp` | Protocol (with `lsp-server` and `lsp-types`), capabilities, conversions, extensions of section 17, cancellation plumbing |
| 6 | `klf-kernel-ide` | Editor analysis for `.klfd`, `.kir`, `.ksir`, `.ridl`, `.ridlsig`, `.klffi` |
| 6 | `klf-editor-gen` | Generators of section 19 (TextMate, tree-sitter through `tree-sitter-generate`, language configuration, VS Code `.vsix` writer, Zed, Neovim, Helix, Emacs) |
| 3 | `klf-shader-ide` | What a shader module shows in an editor: the resource, layout, option and interface facts of sections 14.3 and 14.4, computed from the module, its binding assignment and its reflection |
| 3 | `klf-shader-spirv` (`disasm`) | SPIR-V disassembly for output views (section 14.5) |
| 7 | `klf-cli` | `klf lsp`, `klf editor` |
| Test only | `klf-lsp-test` | Scripted sessions, reference client, editor corpus runner, tolerance fuzzer, TextMate engine, budget recorder |

The JavaScript VS Code client lives in `editors/vscode-client/` of the KLF repository and is built at KLF release time into the distribution; it is the one non-Rust artifact (DE-010).

### 26.2 Milestones

| Milestone | Content | Exit test | Needs |
| --- | --- | --- | --- |
| E0 | `klf-text`, `klf-facts`, editor services, overlay and snapshots, protocol skeleton (lifecycle, sync, push diagnostics) on the toy definition | Protocol, encodings and synchronization suites | M4 |
| E1 | Unwinding definition code, cancellation polls, trap recovery, scheduler | Cancellation suite | E0 |
| E2 | Default features of section 11 over facts; `Editor` mode in the query cache; index | Facts and features suites | E1 |
| E3 | `EditorSpec`, `klf editor`, all generators, VS Code client | Editor package suite; items 5 to 7 of 24.4 on the toy definition | E0 |
| E4 | `klf lsp` for kernel file types | Kernel-language corpus | E2, E3 |
| E5 | Kira: recovery in the definition, facts, member completion, project hook; ownership facts | Kira editor corpus, tolerance, parity (18.2), oracle parity (24.5), budgets; `kira-language-server` and `kira/editors/` retired | E2, M7 |
| E6 | Shader profile: section 14, KSL facts, build on save, Kira and KSL links | KSL editor corpus; budgets | E2, S3 |
| E7 | RIDL `@Source`, KIR view, macro expansion view, runnables, call and type hierarchies | Corresponding corpus cases | E5 |
| E8 | C and KArc: compilation database, preamble caching, inactive regions; MSL profile | C and KArc editor corpora; UE5 budget | E2, M6, M9, S6 |

### 26.3 Working rules

As `KLF-RIDL.md` 32.3. A change to a fact service, a token class, a setting or an extension updates this document, the toy definition's facts, the protocol suite and the generated package templates in the same commit.

---

## 27. Appendix A: editor corpus format

A definition's editor corpus is a set of `.klfd` manifests under `corpus/editor/`, each holding one `construct <Name>() extends EditorCorpus`:

```
construct KiraNavigation() extends EditorCorpus {
    let cases: [EditorCase] = [
        EditorCase {
            name = "local definition"
            files = [EditorFile { path = "main.kira", text = "function f() {\n    let $<x>value = 1\n    print(val$0ue)\n}\n" }]
            checks = [.Definition(at = "$0", target = "x")]
        },
        EditorCase {
            name = "member completion on broken line"
            files = [EditorFile { path = "main.kira", text = "struct P { let a: Int }\nfunction f(p: P) {\n    let q = p.$0\n}\n" }]
            checks = [.Completes(at = "$0", includes = ["a"], excludes = ["f"])]
        },
    ]
}
```

Markers in `text` are removed before the file is opened:

| Marker | Meaning |
| --- | --- |
| `$0` | The cursor |
| `$<name>` | A named position |
| `$[name` … `$]` | A named range (no nesting of the same name) |

Checks:

| Check | Passes when |
| --- | --- |
| `.Diagnostics(expected: [String])` | Published diagnostics equal `"<code>:<line>"` strings, as `KLF-RIDL.md` 31.1 |
| `.Definition(at, target)` | Definition at `at` lands on the named position or range |
| `.References(at, targets: [String], includeDeclaration: Bool)` | References equal the named set |
| `.Hover(at, contains: [String])` | Hover Markdown contains each string |
| `.Completes(at, includes: [String], excludes: [String])` | Completion labels include and exclude |
| `.Signature(at, label: String, activeParameter: Int)` | Signature help matches |
| `.Rename(at, newName, expect: [EditorFile])` | The edited files equal `expect` |
| `.Tokens(golden: String)` | Semantic tokens of the first file equal the golden file (one line per token: `line:col len class modifiers`) |
| `.Symbols(golden: String)` | Document symbols equal the golden outline |
| `.Folds(ranges: [String])` | Folding ranges equal the named ranges |
| `.Hints(golden: String)` | Inlay hints equal the golden file |
| `.Action(at, title: String, expect: [EditorFile])` | Applying the action titled `title` yields `expect` |
| `.Format(expect: [EditorFile])` | Formatting yields `expect` |
| `.Edit(at, insert: String, then: [EditorCheck])` | After inserting `insert` at `at`, the nested checks pass (tests incremental behavior) |

---

## 28. Appendix B: settings

Settings live under the language's name (`kira.*`, `ksl.*`). Kernel settings (`klf.*`) apply to every KLF server and are read under each language's section too (`kira.klf.memoryLimit`), so two servers can be configured apart.

| Setting | Type | Default | Section |
| --- | --- | --- | --- |
| `<lang>.server.path` | string | `""` | 19.6 |
| `<lang>.server.arguments` | string array | `[]` | 19.6 |
| `<lang>.drivers.<driver>` | string | `""` | 19.6 |
| `<lang>.flags.<name>` | per `DriverFlag` type | the flag's default | 9.3 |
| `<lang>.targets` (shader) | string array | the definition's `targets` | 14.1 |
| `<lang>.buildOnSave` (shader) | boolean | on for projects with an `output` | 14.6 |
| `<lang>.compilationDatabase` (C, KArc) | string | `""` | 22.1 |
| `klf.diagnostics.debounceMs` | integer | 50 | 4.3 |
| `klf.diagnostics.workspace` | boolean | false | 11.1 |
| `klf.completion.limit` | integer | 200 | 11.6 |
| `klf.workspaceSymbols.limit` | integer | 256 | 11.9 |
| `klf.inlayHints.types` | boolean | true | 11.12 |
| `klf.inlayHints.parameters` | boolean | true | 11.12 |
| `klf.inlayHints.ownership.copy` | boolean | true | 13.3 |
| `klf.inlayHints.ownership.move` | boolean | true | 13.3 |
| `klf.inlayHints.ownership.drop` | boolean | false | 13.3 |
| `klf.inlayHints.layout` | boolean | true | 14.4 |
| `klf.lens.references` | boolean | false | 11.18 |
| `klf.lens.runnables` | boolean | true | 11.18 |
| `klf.memoryLimit` | integer (MiB) | half of memory, at most 8192 | 8.8 |
| `klf.index` | boolean | true | 8.7 |
| `klf.trace.server` | `off`, `messages`, `verbose` | `off` | 10.1 |

---

## 29. Appendix C: pattern dialect

Editor spec patterns are one dialect translated to every target engine: Oniguruma (TextMate), JavaScript (VS Code language configuration, tree-sitter `grammar.js`), Rust `regex` (Zed configuration, the kernel), and Emacs Lisp `rx` forms.

| Construct | Syntax |
| --- | --- |
| Literal character | Any character except `\ . [ ] ( ) { } * + ? \| ^ $`; those are escaped with `\` |
| Any character except newline | `.` |
| Class | `[abc]`, `[a-z]`, `[^...]` with ranges and escapes `\d \w \s \n \t` inside |
| Class escapes | `\d` (ASCII digit), `\w` (ASCII word), `\s` (space, tab, CR, LF) |
| Repetition | `*`, `+`, `?`, `{m}`, `{m,}`, `{m,n}`, greedy only |
| Group | `(?:...)` |
| Alternation | `a\|b` |
| Anchors | `^` and `$` (line), `\b` (ASCII word boundary) |

Not in the dialect: capturing groups, backreferences, lookaround, lazy quantifiers, Unicode classes, flags. These differ between the target engines or are unavailable in some, so a pattern using them could highlight differently in two editors. Patterns are matched against one line at a time except in `StringForm` and `BlockComment`, whose openers and closers are literal strings.

---

## 30. Appendix D: decision log

Specification decisions, numbered `DE-` to keep them apart from the `D-` and `DS-` series.

| Id | Decision | Reason |
| --- | --- | --- |
| DE-001 | The server is the generated driver's `lsp` command, not a separate program | One computation (rule 1.4.1): the editor runs the compiled definition the build runs. A separate server would be a second frontend that drifts |
| DE-002 | Facts recorded by phases, answered by the kernel; hooks only where facts cannot answer | Each language would otherwise reimplement definition, references, rename, highlight, outline, folding and tokens. Recording at the point of decision costs one service call and gives every feature to every language |
| DE-003 | Error tolerance is required in both modes | Mode-dependent diagnostics break command-line parity. A build that reports every independent error is also better for users and agents |
| DE-004 | Cancellation by unwinding compiled definition code, with polls at back edges | rust-analyzer's model; the only one that bounds cancellation latency for code the kernel did not write. Definitions are the kernel's to compile, so it can add tables and polls; build drivers pay table size only |
| DE-005 | Analysis mode is part of the query key | Facts would otherwise be missing from cached build results or bloat build caches |
| DE-006 | Semantic tokens are the truth; generated grammars are the first paint | Only the definition knows what a name is. Grammars give instant color and structure before the server answers and in editors without semantic tokens |
| DE-007 | Tree-sitter parsers are generated in-process with `tree-sitter-generate` | No Node.js or tree-sitter CLI on the machine that builds a toolchain |
| DE-008 | The overlay serves open buffers to every read, including imports | The pre-KLF server read imports from disk and showed errors that the user had already fixed in another unsaved buffer |
| DE-009 | Servers of different languages communicate only through files | Same contract as builds (`KLF-KSL.md` 24); no server protocol between toolchains to version |
| DE-010 | The VS Code client is one prebuilt JavaScript file per KLF release, parameterized by the generated manifest | VS Code requires a JavaScript client. Building it once keeps Node.js out of toolchain generation and makes every language's extension behave the same |
| DE-011 | Rename refuses when the workspace has unresolved references that might be the renamed entity | A partial rename silently breaks code; refusing with the list of blocking files is recoverable |
| DE-012 | Superseded by `KLF-FORMAT.md` DF-001: one kernel engine, the style is the definition's | Formatting is a language's style decision, but comments, width fitting, edits and safety are the same for every language |
| DE-013 | Kernel adds ownership, RIDL, layout and binding facts by itself | These are results of kernel passes no definition computes; showing them in the editor is the main thing a KLF language server can do that others cannot |
| DE-014 | Debugging is out of 1.0 | A debug adapter needs the native backend's debug information (`KLF-RIDL.md` 16.3) and the VM's debugging interface, neither specified yet. Runnables cover starting programs |
| DE-015 | Rich tree-sitter grammars are allowed but checked against the corpus | Structural editing benefits from a real grammar; the corpus check keeps it from lagging the language, which the pre-KLF repository could only enforce by a review rule |
