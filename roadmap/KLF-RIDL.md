# KLF and RIDL Specification v1.0

Date: 2026-09-19. Status: normative draft 1.0. Audience: every agent and engineer that implements any part of KLF.

Contents

1. Status, scope and rules for implementers
2. Terminology
3. Goals, non-goals and related work
4. System overview
5. The `klf` tool, build pipeline and sysroot
6. KLF Kira, the definition language
7. Definitions: the `Language` family and `.klfd` packages
8. Kernel services API
9. Frontend pipeline, incrementality and scale
10. KIR: structure, types and textual format
11. KIR ownership model
12. KIR instruction set
13. Generics engine
14. Optimization pipeline and compile-time evaluation
15. Bytecode and the VM
16. Native backend
17. Hybrid execution and hot module push
18. KLF runtime
19. RIDL: principles and scope
20. RIDL type system and `.ridl` files
21. RIDL data layout
22. RIDL calling convention
23. RIDL generics: descriptors, witness tables, builders
24. RIDL symbols, metadata, versioning and linking
25. Implementing RIDL in Rust and Swift
26. `.klffi`: the C ABI boundary
27. Language profile: Kira
28. Language profile: KArc
29. Language profile: C
30. Language profile: Swift (expressibility)
31. Conformance, diagnostics and security
32. Implementation plan and kernel crate layout
33. Appendix A: grammars
34. Appendix B: decision log

---

## 1. Status, scope and rules for implementers

This document is the single normative specification for KLF (the Kira Language Framework), KIR (its intermediate representation), KLF Kira (its definition language), RIDL (the Runtime IDL and native ABI) and `.klffi` (the C ABI boundary). It also contains the language profiles for Kira, KArc, C and Swift. An implementation that satisfies every MUST in this document is a conforming KLF 1.0.

### 1.1 Normative language

- MUST, MUST NOT, SHOULD, SHOULD NOT and MAY carry their RFC 2119 meanings.
- Sections or paragraphs titled "Rationale" or "Example" are informative. Everything else is normative.
- Where prose and a table disagree, the table wins. Where a grammar in Appendix A and an example disagree, the grammar wins.
- Every numeric constant (register numbers, opcode values, offsets, magic numbers) is normative.

### 1.2 Scope and stability

| Artifact | Role | Stability |
| --- | --- | --- |
| KLF kernel | Rust implementation of KIR, verifier, optimizer, VM, LLVM backend, hybrid runtime, KLF runtime, `klf` tool | Internal. Free to change between KLF releases |
| KLF Kira (`.klfd`) | Language in which definitions are written | Versioned `klfkira 1.0`, additive-only afterwards |
| KIR, bytecode (`.kir`, `.kirb`, `.klbc`) | Internal program representations | Internal. Versioned per KLF release. Never a contract between toolchains |
| RIDL (`.ridl`) | Native ABI and interface language at every public module boundary | Frozen contract `ridl 1.0`, additive-only afterwards |
| `.klffi` | Description of platform C ABI libraries | Frozen grammar `klffi 1.0`; semantics are each platform's C ABI |
| Language profiles | Mapping of Kira, KArc, C and Swift onto the above | Normative for Kira, KArc, C. Swift is an expressibility proof and is not implemented |

### 1.3 Rules for implementing agents

1. Implement behavior exactly as written. Where the document is silent, choose the behavior that keeps the VM, native and hybrid backends observably identical, and record the choice in `docs/decisions.md` in the format of Appendix B.
2. Every feature MUST work on the VM backend and the LLVM backend. A feature that touches the `engine` attribute MUST also work on hybrid. A Web-portable feature MUST work on `wasm32`.
3. No placeholder, stub, hardcoded success or host-rendered substitute counts as an implementation. A section is implemented when its conformance tests (section 31) pass on every backend.
4. The kernel is written in Rust. No kernel code may be written in KLF Kira or a hosted language. No Python in tracked files.
5. Rust files stay under 700 lines, `.klfd` files under 700 lines, generated files excepted.
6. No code from the pre-KLF Kira compiler repository is reused. Its test corpus `tests-kik` is reused as the Kira conformance oracle.
7. The kernel MUST build against the latest stable LLVM release. The LLVM fork carrying the `ridlcc` calling convention (section 22.7) is part of the kernel.

---

## 2. Terminology

| Term | Meaning |
| --- | --- |
| Kernel | The Rust code of KLF: everything not written in KLF Kira or in a hosted language |
| Hosted language | A language whose compiler KLF produces from a definition: Kira, KArc, C, Swift |
| Definition | The `.klfd` package that describes one hosted language |
| KLF Kira | The language of `.klfd` files. Its frontend is part of the kernel |
| Toolchain | Output of `klf build` for one definition: compiler driver, VM, hybrid runtime, prebuilt std |
| Toolchain-build time | When `klf build` runs. Definitions are compiled here and never again |
| User-compile time | When a generated toolchain compiles a user's program |
| Std | A hosted language's standard library, written in that language, compiled by its own toolchain |
| KIR | The kernel's ownership-aware SSA intermediate representation |
| Raw KIR, canonical KIR | KIR before and after the mandatory ownership passes (section 11.6) |
| Policy record | The record (copy, destroy, relocate, exclusivity, pairing) a definition attaches to each type |
| Value kind | One of trivial, owned, borrowed, weak |
| Convention | How a parameter transfers ownership: owned, borrow, borrow mut |
| Engine | Where a function body executes: VM (`runtime`) or machine code (`native`) |
| Module | Unit of separate compilation and of RIDL export |
| Public boundary | The exported surface of a module. RIDL governs it and nothing else |
| Type descriptor | RIDL runtime record for a type: size, alignment, copy, move, destroy |
| Witness table | RIDL runtime record for one conformance of a type to a trait |
| Polymorphic entry | The single compiled body of an exported generic that takes descriptors and witness tables at run time |
| Named instance | A specialized, separately exported instance of a generic, used as a fast path |
| Opaque struct | A RIDL struct declared without a body. Other modules see only a pointer |
| RIDL box | Heap allocation with a two-word header (descriptor, free function) followed by a value |
| Counted | Memory policy where copy is retain and destroy is release |
| Affine | Memory policy where a value is used at most once and copies are explicit |
| Unmanaged | No policy. Raw addresses and explicit allocation |
| ARC mode, C++ mode | The two per-file memory modes of KArc: `.krc` counts references, `.cpp` is clang |
| RIDL mode, platform mode | The two per-file calling-convention modes of C |
| Thunk | Generated function converting between the RIDL convention and a platform C convention |
| Corpus | A body of real source code that defines conformance for a hosted language |
| Oracle | An existing trusted implementation (clang, GCC, the pre-KLF Kira compiler) whose observable output a toolchain must match on the corpus |
| Kelf-hosted | Hosted on KLF. Kira, KArc and C are kelf-hosted |

---

## 3. Goals, non-goals and related work

### 3.1 Goals

1. One kernel hosts many languages. Kira, KArc (C++ with imposed ARC), C and Swift are peers. Kira has no privileged position in the kernel.
2. Changing a hosted language never changes Rust. A breaking change to Kira is an edit to Kira's `.klfd` files and to Kira's std.
3. No bootstrap cycle and no trusted binary seed. The only language whose frontend is written in Rust is KLF Kira. Every other language is defined in KLF Kira. No definition is ever written in the language it defines.
4. `klf build` turns a definition into a specialized compiler, VM and hybrid runtime. At user-compile time no `.klfd` is read or interpreted, so genericity costs nothing at compile time.
5. One ownership discipline in KIR expresses four memory models: unmanaged (C), counted (KArc, Swift), affine (Kira), and mixed modes per file (KArc raw files).
6. Every hosted language gets all three execution modes (VM, native, hybrid), stock LLDB debugging, hot module push, and RIDL export, without per-language backend work.
7. RIDL is a real native ABI, richer than C: ownership conventions, payload enums, typed failure, traits, closures, and public generics with full support from 1.0. It conforms to no existing ABI. It is designed so that rustc and swiftc could adopt it as their native stable ABI with a contained compiler patch.
8. Scale: KArc MUST compile and run Unreal Engine 5 class codebases; C MUST compile the Linux kernel and the major C corpora; Kira MUST compile the whole Kira ecosystem.

### 3.2 Non-goals

- KLF is not marketed as a language workbench. Tooling for third parties to define languages is not a goal of 1.0, although nothing prevents it.
- KIR and bytecode are not stable interchange formats.
- Binary compatibility with the Itanium C++ ABI, the MSVC C++ ABI or Apple's Swift ABI is not provided. Prebuilt C++ or Swift binaries cannot be linked; C ABI libraries can, through `.klffi`.
- RIDL does not provide reflection, runtime conformance lookup, dynamic casts, or a type registry.
- Async functions and borrowed returns are not part of RIDL 1.0. Languages with async lower it to state machines and ordinary calls inside their own modules (section 12.14).

### 3.3 Related work (informative)

| System | What KLF shares | Where KLF differs |
| --- | --- | --- |
| Racket `#lang`, `#%kernel` | Languages defined as libraries over a small core | KLF definitions are ahead-of-time compiled compilers for systems languages, not macro expanders |
| Lean 4 | Language elaborators written in a hosted language | Lean needs a stage0 binary. KLF needs none because KLF Kira's frontend is Rust |
| PyPy RPython | A framework turns a language description into a VM | KLF also produces a native compiler and a hybrid runtime, and the input is a compiler, not an interpreter |
| MLIR | Extensible IR infrastructure | KIR is one fixed ownership-aware IR; extension happens in definition code, not in dialects |
| Swift SIL with OSSA | Ownership SSA hosting both counted and non-copyable types | KIR generalizes the policy to unmanaged and per-file mixed modes and is language-neutral |
| Truffle/Graal | One runtime for many languages | KLF is AOT-first and targets systems languages with manual and affine memory |
| WinRT, COM | A rich OS-level ABI spoken natively by several compilers | RIDL adds ownership conventions, payload enums, value generics through descriptors, and a register-level convention |
| WebAssembly component model (WIT) | Own and borrow handles, variants, results | WIT copies between memories. RIDL is same-address-space and zero-copy |
| Rust crABI experiment | A richer-than-C stable ABI for Rust | RIDL covers traits, generics and closures and is shared with other languages |
| Chromium BackupRefPtr | Reference counts in allocator slot metadata | KLF uses the same placement to impose ARC on C++ without changing object layout |

---

## 4. System overview

### 4.1 The three layers

1. Kernel (Rust). Contains: the KLF Kira frontend; KIR with its verifier, ownership passes, generics engine and optimizer; the bytecode compiler and VM; the LLVM backend; the hybrid runtime; the KLF runtime (allocator, reference counting, traps); RIDL and `.klffi` processing; the kernel services exposed to definitions; the `klf` tool.
2. Definitions (KLF Kira, `.klfd`). One package per hosted language. A definition is a compiler frontend: it reads source text of its language and produces KIR through kernel services. It also declares the language's memory model, file modes, driver flags, diagnostics and lang items.
3. Standard libraries (each in its own language). Kira's `heap` and `std` are Kira. C's libc is C. KArc's standard headers and runtime are C++. They are compiled by the generated toolchain.

### 4.2 The two times

- Toolchain-build time: `klf build` compiles the `.klfd` package to native code, links it with the prebuilt kernel libraries, producing the driver; then uses that driver to compile the language's std for every requested target and engine; then assembles the sysroot.
- User-compile time: the generated driver compiles user programs. It contains the definition as machine code. It never parses `.klfd`.

### 4.3 File types

| Extension | Content | Producer | Consumer |
| --- | --- | --- | --- |
| `.klfd` | KLF Kira source | Humans, agents | `klf build` |
| `.ridl` | RIDL interface declarations | Generated toolchains, humans | Toolchains, binding generators, rustc and swiftc integrations |
| `.ridlsig` | One frozen ABI record (delta) of a module | `klf abi freeze` | Toolchains, loaders |
| `.klffi` | Platform C ABI library description | `klf ffi gen`, humans | Toolchains |
| `.kir` | Textual KIR | Toolchains (`--emit=kir`) | Tests, humans |
| `.kirb` | Binary KIR cache | Toolchains | Toolchains of the same KLF release only |
| `.klbc` | Bytecode module | Toolchains | VM |
| `.klbundle` | Deployable: manifest, bytecode modules, native halves, assets | Toolchains | Runners |

### 4.4 What is shared and what is specialized

Shared by all toolchains of one KLF release: KIR, the verifier, the optimizer core, the bytecode format, the VM core, the LLVM backend, the runtime.

Specialized per toolchain: the frontend (the compiled definition), language passes registered by the definition, the set of runtime components linked in (a C toolchain links no reference counting fast paths it never emits), driver flags, diagnostics.

Contract between toolchains and with the outside world: RIDL only. Two modules built by different toolchains, by different KLF releases, or by rustc or swiftc with RIDL support, interoperate through RIDL and through nothing else.

### 4.5 One definition per toolchain

`klf build` takes exactly one definition and produces exactly one toolchain. A program that mixes languages is a set of modules, each built by its language's toolchain, linked together. Calls between them use RIDL. On the VM, modules of different languages run in the same VM instance because the bytecode format and the VM core are shared; a runner links the union of the runtime components its modules need.

---

## 5. The `klf` tool, build pipeline and sysroot

### 5.1 Distribution layout of KLF itself

```
klf/
  bin/klf
  kernel/<host-triple>/libklf_kernel.a        kernel for linking drivers, with embedded LLVM bitcode
  kernel/<target-triple>/libklfrt.a           KLF runtime per target
  kernel/<target-triple>/libklfvm.a           VM core per target
  kernel/<target-triple>/libklfhybrid.a       hybrid runtime per target
  kernel/services.manifest                    list of kernel service entry points (section 8.12)
  prelude/*.klfd                              KLF Kira prelude and the KLF module
  lld, llvm tools                             bundled linker and LLVM
```

Generating a toolchain MUST NOT require a Rust installation. The kernel libraries are prebuilt. `libklf_kernel.a` embeds LLVM bitcode so that link-time optimization inlines across the boundary between compiled definition code and kernel services. The Rust compiler used to build the kernel MUST use an LLVM version less than or equal to the LLVM bundled with `klf`.

### 5.2 Commands

| Command | Effect |
| --- | --- |
| `klf build <defdir> --targets <t1,t2,...> --out <dir> [--profile debug or release]` | Full pipeline of 5.3 |
| `klf check <defdir>` | Parse and type-check the definition only |
| `klf test <defdir> [--corpus <name>]` | Build, then run the definition's conformance corpus (section 31) |
| `klf ridl check <file.ridl>` | Validate a `.ridl` file |
| `klf ridl bindgen --lang rust or swift <file.ridl>` | Generate bindings for non-KLF languages |
| `klf ffi gen <header.h> --target <t> -o <file.klffi>` | Generate `.klffi` from C headers using the C definition's frontend |
| `klf abi freeze <module>` | Compute the module's RIDL surface, mint a UUID, write the `.ridlsig` delta |
| `klf abi apply <module>` | Rewrite sources: stamp superseded shapes with `@Abi(version:)`, update the current UUID |
| `klf abi retire <module> <uuid>` | Move a record to `abi/unsupported/` and rebase surviving records |
| `klf kir verify or dump <file>` | Verify or print KIR |
| `klf clean` | Remove caches |

### 5.2.1 Reporting

A command that compiles reports what it is doing. The phases of one command are
named at every boundary worth naming — reading the package, the `units`, `parse`,
`elaborate` and `lower` of each unit, verification, codegen, linking — and each
one is reported the moment it starts.

This is a property of every client CLI, not of `klf`. A generated driver (5.5) is
a client CLI of its own language and reports the same way, from the same
implementation, because the report is a kernel facility and both read it through
the kernel. A KLF Kira driver and a KArc driver therefore say the same things
`klf` does.

By default the report is animated: a title line and the most recent phases,
redrawn in place on standard error and erased when the command returns. Standard
error is where it belongs, because a build's result is what goes to standard
output, and progress is not part of that. Under a pipe there is no animation and
no report: a captured build is exactly as quiet as one that reports nothing.

The title line is repainted on a timer rather than only when a phase arrives, so
the elapsed time stays live inside a phase that reports nothing for seconds. A
band of colour sweeps across the title's words on each repaint, and the elapsed
time itself is never styled, so the numbers do not flicker between shades. The
timer repaints that one row and nothing else: repainting the whole surface at
that rate is what makes a status display flicker, so the phase rows are repainted
only when the phase list changes, which is on a phase and nothing else. Colour is
off under `NO_COLOR` and under a terminal that declares itself `dumb`.

`--timings` replaces the animation with a record: every phase goes to standard
output as one line carrying the milliseconds it took, in the order it ran, with
the total closing the list. Nothing is animated, nothing is erased, and no
terminal is required. A build is not one thread, so two phases that overlapped
both claim the same milliseconds; the report says so rather than leaving a reader
to find out why the rows do not add up.

A command that produces something says so on standard output, once the work is
over: `Check: Ok` for a check, `Built: Ok <artifact>` for a build, and the word
after the colon is `Failed` when the command did not succeed. A command whose
standard output is a payload — emitted KIR, a module dump, generated bindings —
prints nothing else on that stream.

Every print a client CLI makes goes through the report's own print path, which
stands the surface down first. A display that is repainted in place and a
diagnostic printed underneath it interleave into nonsense, and the surface is
erased by moving the cursor over the rows it drew, so a bare print walks that
erase up from the wrong row and wipes the output instead.

### 5.3 `klf build` pipeline

1. Load the definition package. Parse and check all `.klfd` files with the KLF Kira frontend. Locate the single declaration that extends `Language` (section 7).
2. Lower the definition to KIR, run the mandatory passes, optimize at the requested profile, emit a host object file through the LLVM backend.
3. Link the object with `libklf_kernel.a` using the bundled lld and LTO. The result is `bin/<driver>`. The driver's `main` is kernel code; it instantiates the definition's `Language` declaration and runs the driver loop (section 9).
4. For each target: compile the std named by `stdRoot` with the new driver, once to bytecode and once to native static and shared libraries, and emit the std's `.ridl` files.
5. Assemble the sysroot (5.4) and write `definition.lock`.

Steps 4 and 5 run the driver as a subprocess. If step 4 fails, the build fails and no sysroot is written.

### 5.4 Sysroot layout

```
<out>/
  bin/<driver>                          compiler driver (host)
  bin/<driver>-run                      runner for bytecode and bundles (host)
  lib/klf/<target>/libklfrt.a
  lib/klf/<target>/libklfvm.a
  lib/klf/<target>/libklfhybrid.a
  lib/<lang>/<target>/libstd.a
  lib/<lang>/<target>/libstd.(so or dylib or dll)
  lib/<lang>/bytecode/<ptrwidth>/std.klbc
  share/<lang>/ridl/*.ridl
  share/<lang>/abi/*.ridlsig
  definition.lock
```

`definition.lock` records: KLF version, `klfkira` version, SHA-256 of every `.klfd` file, SHA-256 of every std source file, target list, LLVM version.

### 5.5 Generated driver command line

Every driver accepts the uniform interface below. A definition MAY add flags and MAY register a compatibility front door (KArc and C register a clang-compatible argument parser, section 28.11).

| Command | Effect |
| --- | --- |
| `<driver> build [paths] --backend vm or llvm or hybrid --target <t> --emit bin or lib or dylib or klbc or bundle or kir or ridl -o <out>` | Compile |
| `<driver> run [paths] --backend vm or llvm or hybrid` | Compile and execute |
| `<driver> check [paths]` | Frontend only, report diagnostics |
| `<driver> lsp` | Language server over stdio, built from kernel queries (section 8.11) |
| `<driver> ridl emit <module>` | Write the module's `.ridl` |
| `<driver> push <host:port>` | Hot module push client (section 17.5) |

A path naming a directory stands for every file under it, recursively, whose extension is one the Language declares, in path order. The definition decides what the files mean together.

Declared flags are long options named `--<name>`. Boolean flags accept
`--<name>` for true and `--no-<name>` for false. Integer and string flags
accept either `--<name> <value>` or `--<name>=<value>`; integers must parse as
signed 64-bit values. The driver starts from each declaration's default, then
applies command-line overrides in order, so the last occurrence wins. Missing
or invalid values and unknown options are errors. Generated help lists each
flag's accepted forms and default. Names are unique lowercase hyphenated
identifiers, cannot start with `no-`, and cannot use the reserved names `emit`,
`help` or `version`.

`build --depfile <path>` writes, once the compile succeeds, a make-style
depfile at `path`: the output (`-o`, else the first input) as the target, and
as its prerequisites every path the compile read, asked the existence of or
listed, one to a line in path order: canonical where the path exists, else
absolute without `.` and `..`. A phase the query cache
(5.6) answered counts the paths its answer was validated against, so the list
is the same whether the cache answered or not. A listed directory and a path
found absent are prerequisites like a file read: a build system keying the
compile on them rebuilds when a directory gains a file or a missing path
appears. In a path, a space, `#` and `\` are escaped with `\` and `$` is
doubled.

Before parsing the uniform interface, the driver calls
`Language.compatDriver(arguments)` with the original process arguments after
`argv[0]`, without consuming or rewriting tokens. `None` sends that unchanged
vector to the uniform parser. `Some(request)` sends `request.arguments` to the
same parser; the request does not bypass uniform validation or compilation.
`DriverRequest` is deliberately limited to this normalized argument vector:

```
struct DriverRequest { let arguments: [String] }
```

The kernel `klf` CLI currently has an M-stage host-program path for `.kir` and
built-in source inputs: `klf build <file.kir> --emit bin -o <out>`. It uses
Cranelift, provisioned clang and `klf-native-rt` for `aarch64-apple-darwin`; the
generated executable reports its outcome through the runtime's
`RET`/`FAIL`/`TRAP` protocol. This implementation slice does not change the
generated-driver contract above or satisfy the LLVM backend and sysroot build
pipeline in section 5.3. See `IMPLEMENTED.md` and decision D-114 for status.

Generated frontends also have an M-stage native output path: an extensionless
`-o <out>` selects a `bin`, while `.kir` and `.klbc` retain their matching
formats; an explicit `--emit` overrides inference. This path currently targets
`aarch64-apple-darwin`, emits `main` with Cranelift, and obtains
`klf-native-rt` through a workspace Cargo build. Generated frontends embed
declared typed flags, parse overrides, and install effective values in the
Rust `Session`. `.klfd` code can read typed Boolean, Integer and String values
through `session.flag(name)` on both the VM and native frontend paths. String
buffers cross the shared service bridge in bounded bulk reads and writes, not
per-byte services. Both frontend paths select a unique file mode by extension
or standalone `#pragma`; generated native drivers also accept the declared
`<flag>=<glob>` form. A conflicting selection is `KLF-DRV-0022`. The selected
zero-based descriptor index is available to `.klfd` code through
`session.file_mode(sourceFile)`, or `-1` when no mode matches. For every input
file, both paths also configure its effective memory model and default
convention on `Session`: a selected mode's optional value overrides the
language default, and an absent override inherits it. `.klfd` code reads those
typed values through `session.file_memory(sourceFile)` and
`session.file_convention(sourceFile)`. The `.klfd` type builders derive
default KIR policies from each file's memory model and language exclusivity
when every member is trivially managed. They assign the effective convention
to each non-foreign KIR function so merged units retain per-file defaults;
LLVM lowers RIDL functions to `swiftcc`, while foreign imports stay platform C.
Types requiring memberwise lifecycle synthesis or containing unresolved
member types are rejected until that synthesis exists. KIR function-pointer
types retain Platform/RIDL ABI, and LLVM indirect calls use the target type's
ABI. The kernel synthesizes adapters for direct, signature-matching
function references, including foreign functions and aggregates passed by
address. Failing function pointers keep the RIDL convention and propagate
through `call.indirect`; converting one to the platform convention is a
verifier error. `call.closure` lowers to a `call.indirect` over the record's
`code` word with `ctx` in the context register, and `closure.callback`
produces a platform thunk that recovers a closure from a user-data parameter.
Default policies for types with `ref`/`uniq` members, or with members of such
types, are memberwise: the kernel synthesizes `destroy(obj: ptr)` and, for
Counted files, an implicit `copy(dst: ptr, src: ptr)`. Generic members, weak
members and arrays of managed members are rejected. The self-contained sysroot
contract above also remains unmet.

The generated frontend metadata records whether the KLF Kira definition
declares a `compatDriver` override. An absent override follows the default
`None` path and preserves the original arguments. A declared override is
lowered with the phases, and the lowerer generates a `__klf_compat_driver`
entry around it: the entry rebuilds `[String]` from the `klf.driver.argument_*`
services, calls the override, and returns each `Some` argument through
`klf.driver.request_push`. Native frontends call the entry through the phase
trampoline; the VM path calls it by name (`LanguageDefinition::compat_driver`).
`Optional<DriverRequest>` is a built-in enum instance; other `Optional<T>`
types remain outside the KLF Kira subset.

The kernel `klf` CLI implements steps 3 to 5 at M stage as
`klf build <defdir> --out <dir>`: it links `bin/<driver>`, copies the runtime
archive to `lib/klf/<target>/libklfrt.a`, runs the new driver as a subprocess
over every file under `stdRoot` with a claimed extension (`--emit lib` to
`lib/<lang>/<target>/libstd.a`, `--emit klbc` to
`lib/<lang>/bytecode/64/std.klbc`), and writes `definition.lock` with the KLF,
`klfkira` and LLVM versions, the target, and the SHA-256 of every definition
and std source. The tree is staged and only moved to `<dir>` when every step
succeeds; an existing `<dir>` is replaced only when it holds a
`definition.lock` or is empty. A driver installed in a sysroot links
executables against that sysroot's `libklfrt.a` and `libstd.a`. A std library
exports each non-private function under its own name with the platform C
convention, so programs declare std functions as foreign functions; a std that
defines globals is rejected because globals are allocated by a program
prologue. Still missing from 5.4: `<driver>-run`, `libklfvm.a`,
`libklfhybrid.a`, shared std libraries, std `.ridl`/`.ridlsig` files, targets
other than `aarch64-apple-darwin`, and a runtime built without Rust.

### 5.6 Caching

The cache directory is `$KLF_CACHE` or `~/.cache/klf`. Keys are SHA-256 over (KLF version, definition hash, target, profile, query key, input hashes). Cached values: query results serialized through `Cacheable` (section 8.9), `.kirb` modules, generic instances, object files, and the executable of a `run` without `-o`, which runs again without compiling while every path its compile read, asked the existence of or listed (section 5.5) and every archive it links holds what it did. Cache entries from a different KLF release or definition hash MUST be ignored.

### 5.7 Reproducibility

Given identical inputs, flags and KLF release, a driver MUST produce bit-identical `.klbc`, `.ridl` and `.ridlsig` files, and bit-identical object files when the linker and LLVM are the bundled ones. Drivers MUST NOT embed timestamps or absolute paths unless `--debug-paths=absolute` is given.

### 5.8 Targets of 1.0

`aarch64-linux`, `aarch64-android`, `aarch64-macos`, `aarch64-ios`, `aarch64-windows`, `x86_64-linux`, `x86_64-macos`, `x86_64-windows`, `aarch64-tessera`, `x86_64-tessera`, `wasm32`. A Linux target built freestanding uses the kernel syscall interface, RIDL as the system ABI, and no platform C library requirement. No other target is part of 1.0.

Tessera is an operating system with Linux behind it. Its triples are `aarch64-unknown-tessera` and `x86_64-unknown-tessera`: the operating system field names Tessera, and no C library environment is part of the name, because a Tessera program's system interface is RIDL and LibTessera, not a C library. A Tessera target is its own platform to a program (section 27.8) and runs on the Linux kernel:

1. The kernel's runtime archives for a Tessera triple are built with Rust for a custom target of the same name, `kernel/targets/<triple>.json`. It is the Linux target of the same architecture, with the Linux kernel ABI and the C library Tessera stages for the programs that carry one, under Tessera's own name; Rust's standard library is built for it from source.
2. LLVM, the C toolchain and the linker take the LLVM target that custom target names, which is the Linux one of the same architecture: LLVM knows no Tessera, and the object files and executables are Linux ELF.
3. A native library's `NativeTarget` row for a Tessera target names the Tessera triple itself. A Tessera triple is not a Linux triple, and a Linux row never applies to a Tessera target: a library with no Tessera row has no row there.
4. A Tessera target is never the host: building for it is a cross build, which takes its C library from `KLF_SYSROOT_<TRIPLE>` or `KLF_SYSROOT` as any cross build does.

---

## 6. KLF Kira, the definition language

### 6.1 Position

KLF Kira is the only language whose frontend is written in Rust. At the freeze of `klfkira 1.0` it is a strict subset of Kira plus the `KLF` module. It is versioned independently of Kira and does not track Kira afterwards. Its grammar is Appendix A.1. After 1.0 it changes additively only: a program valid under `klfkira 1.x` stays valid with the same meaning under `klfkira 1.y`, y greater than x.

### 6.2 What is in the language

- Declarations: `import`, `function`, `struct`, `enum`, `trait`, `construct`, `extend`, `type` aliases, `distinct` types, top-level `let` constants.
- Generics on functions, structs, enums, traits and constructs, with trait bounds: `function f<T: Bound>(x: T) -> T`.
- Constructs: a family is `construct Name { ... }` with `@Required` members and default members; a declaration is `construct Name(params) extends Family { ... }`. A family value is written `Any Family`. Construct content blocks, `lifecycle` sections and `@Content` members are not part of KLF Kira.
- Statements: `let`, `var`, assignment, `if`/`else`, `while`, `for x in xs`, `match` (arrow arms; over an enum exhaustive with no wildcard, over a `String` string-literal arms and one required `else` arm), `attempt`/`try`/`handle` over Result-shaped enums, `return`, `break`, `continue`.
- Expressions: literals, struct literals `Name { field = value }`, enum values with a leading dot `.Variant(payload)`, calls with labeled arguments, member access, indexing, closures `{ a, b in ... }`, the conditional `c ? a : b`, `as` conversions (between numeric types, and between a kernel handle type and `Int`, which share a representation), `is` tests on family values, and the operators of A.1 with C precedence.
- Annotations of the form `@Name`, `@Name(args)` and `@Name { key: value, ... }`. The only annotations with meaning are `@Required`, `@Service`, `@Inline`, `@NoInline`, `@Cold` and `@Test`.

### 6.3 What is not in the language

Classes; macros of any kind; `comptime`; `@Runtime`, `@Native`, `@Main`, `@Export`; FFI annotations; the `Number` tower; tasks and threads as language features; top-level `var` (mutable global state is forbidden; all mutable state lives in the `Session`, section 8). A construct in the right column is a compile error with code `KLFK-E0001` naming the feature.

### 6.4 Types

| Type | Meaning |
| --- | --- |
| `Int`, `I8` `I16` `I32` `I64`, `U8` `U16` `U32` `U64` | Integers. `Int` is `I64`. Arithmetic traps on overflow; `&+`, `&-`, `&*` wrap |
| `Float`, `F32` | IEEE 754 binary64 and binary32 |
| `Bool`, `String`, `Void` | `String` is immutable UTF-8 with O(1) byte length and copy |
| `[T]`, `Map<K, V>`, `Set<T>` | Copy-on-write collections from the prelude. `Map` and `Set` iterate in insertion order |
| `Optional<T>` | Prelude enum with `.Some(T)` and `.None` |
| `Result<Value, Failure>` | Prelude enum with `.Ok(Value)` and `.Error(Failure)` |
| `(A, B) -> R` | Function and closure type |
| `Id<T>`, `Arena<T>`, `Symbol`, `Span` and the rest of section 8 | Kernel service types |

### 6.5 Memory model

KLF Kira is a counted language in KIR terms. Strings, arrays, maps, sets, enum payload boxes and closure contexts are reference-counted with copy-on-write. Structs are value aggregates whose copy is memberwise. Copies are implicit. There is no borrow checker.

- Parameters are passed by value. The spellings `borrow`, `move` and `copy` are accepted for source compatibility with Kira and have no effect.
- `borrow mut` is the one observable mode: the argument MUST name storage, the callee mutates it in place, and one call MUST NOT pass the same binding to two `borrow mut` parameters (`KLFK-E0207`).
- Cyclic structures MUST be expressed with `Arena<T>` and `Id<T>` (section 8.3). A closure that captures a `var` holding itself is refused (`KLFK-E0210`).

### 6.6 Determinism and re-entrancy

Definition code MUST be deterministic: no clock, no randomness, no environment access except through `Session`. All iteration orders are defined. Definition code runs on many worker threads at once; because there is no global mutable state and every `Session` store is internally synchronized, definition code needs no locks.

### 6.7 Compilation of KLF Kira

The kernel's KLF Kira frontend is an ordinary frontend over the kernel's own services: it produces KIR with the counted policy for heap types, exclusivity `none`, and RIDL is not involved (a definition is linked into one driver and exports nothing). Calls into kernel services use the kernel service ABI (section 8.12).

---

## 7. Definitions: the `Language` family and `.klfd` packages

### 7.1 Package layout

```
karc/
  language.klfd        contains the one declaration extending Language
  lexer.klfd, parser/*.klfd, sema/*.klfd, lower/*.klfd, driver.klfd, ...
  std/                 the language's std sources, in the language itself
  corpus/              conformance manifests (section 31)
  ffi/*.klffi          C ABI libraries the std needs
```

Every `.klfd` file of the directory tree, except under `std/` and `corpus/`, belongs to the package. All files share one namespace; `import KLF` brings in the kernel services.

### 7.2 The `Language` family (provided by the `KLF` module)

```
construct Language<Tree: Cacheable, Program: Cacheable> {
    @Required let name: String
    @Required let driver: String
    @Required let version: String
    @Required let extensions: [String]
    @Required let memory: MemoryModel
    @Required let exclusivity: Exclusivity
    @Required let convention: DefaultConvention
    @Required let stdRoot: String

    let fileModes: [FileMode] { return [] }
    let langItems: [LangItem] { return [] }
    let flags: [DriverFlag] { return [] }
    let diagnostics: [DiagnosticSpec] { return [] }

    @Required function units(session: Session, inputs: [SourceFile]) -> [Unit]
    @Required function parse(session: Session, unit: Unit) -> Tree
    @Required function elaborate(session: Session, unit: Unit, tree: Tree) -> Program
    @Required function lower(session: Session, unit: Unit, program: Program) -> KirModule

    function passes(session: Session) -> [Pass] { return [] }
    function compatDriver(arguments: [String]) -> Optional<DriverRequest> { return .None }
    function hover(session: Session, at: Position) -> Optional<String> { return .None }
    function complete(session: Session, at: Position) -> [Completion] { return [] }
    function definition(session: Session, at: Position) -> Optional<Span> { return .None }
}
```

Supporting enums:

```
enum MemoryModel { Unmanaged Counted Affine }
enum Exclusivity { Static Dynamic None }
enum DefaultConvention { Ridl Platform }
enum DriverFlagValue { Boolean: Bool  Integer: Int  String: String }

struct DriverFlag {
    let name: String
    let default_value: DriverFlagValue
}

struct FileMode {
    let name: String
    let pragma: String
    let flag: String
    let extensions: [String] { return [] }
    let memory: Optional<MemoryModel>
    let convention: Optional<DefaultConvention>
}

struct LangItem { let key: String  let required: Bool }
```

The generated driver initializes a Rust `Session` with these defaults before
applying command-line overrides. The Rust API exposes the effective values to
Rust callers through `Session::flag`; `.klfd` code also reads the effective
typed `DriverFlagValue` through `session.flag(name)`. The VM and native paths
share the same service handlers and checked bulk-buffer access.

### 7.3 Meaning of the members

| Member | Meaning |
| --- | --- |
| `name` | Lowercase language identifier. Used in sysroot paths, KIR `lang` headers and metadata |
| `driver` | File name of the generated compiler driver |
| `extensions` | File extensions claimed by the language. The first is the primary one |
| `memory` | Default memory model of modules in this language. Determines the default policy records the kernel assigns to pointer-like and aggregate types when the definition does not set one explicitly |
| `exclusivity` | Default enforcement of `access.begin` for this language |
| `convention` | `Ridl`: internal and exported functions use the RIDL convention. `Platform`: functions use the platform C convention unless exported through RIDL |
| `stdRoot` | Directory of std sources, relative to the package |
| `fileModes` | Per-file exceptions. A file enters a mode by carrying one of the mode's `extensions`, by containing a standalone `#pragma <pragma>` line, by using the definition's equivalent comment directive, or by matching the driver flag `<flag>=<glob>`. Globs use `*` for any sequence of path characters and `?` for one character. An extension listed by a mode MUST also appear in the language's `extensions`, and MUST NOT be listed by two modes. A mode's optional `memory` and `convention` override the language defaults independently; omitted values inherit. KArc declares `arc`; C declares `platform` |
| `langItems` | Names the definition must find in the std by lookup (for example Kira's `String`, `Array`, `Optional`). The driver fails with `KLF-DRV-0021` if a required item is missing when compiling user code, and skips the check while compiling the std itself |
| `flags` | Extra driver flags, each declared as `DriverFlag { name, default_value }`. `name` is a unique bare identifier matching `[a-z][a-z0-9]*(?:-[a-z0-9]+)*` (without `--`); `emit`, `help`, `version`, and names starting `no-` are reserved. The default case declares the type: `.Boolean(Bool)`, `.Integer(Int)` or `.String(String)`. CLI forms and override order are defined in section 5.5; the effective typed value is available as `session.flag(name)`. |
| `diagnostics` | The definition's diagnostic registry (section 31.4) |
| `units` | Groups input files into translation units. A unit is named by an input file or by a file the phase read through the Session, such as the manifest of a package the inputs depend on; the Session registers the latter once the phase has committed its reads. Kira returns one unit per package; C and KArc return one unit per source file |
| `parse`, `elaborate`, `lower` | The three mandatory phases. Each is invoked by the kernel as an incremental query (section 9). `parse` MAY call into elaboration code of the definition; the split only defines cache boundaries |
| `passes` | Language-specific KIR passes and where they run (section 14.2) |
| `compatDriver` | Receives the original argument vector and may return normalized uniform-driver arguments in a `DriverRequest`; `None` preserves the original vector |
| `hover`, `complete`, `definition` | Language server hooks |

If multiple selectors choose the same mode, the selection is valid. If they
choose different modes for one source, the driver reports `KLF-DRV-0022`.
The driver exposes the selected zero-based entry in `Language.fileModes` as
`session.file_mode(sourceFile) -> Int`; `-1` means no mode was selected. A
definition uses that per-file result when applying its mode semantics. The
effective values are available as `session.file_memory(sourceFile) ->
MemoryModel` and `session.file_convention(sourceFile) -> DefaultConvention`;
each mode override falls back independently to the corresponding language
default. The kernel applies the effective memory model and language
exclusivity when `.klfd` type builders construct KIR defaults for types with
trivial member lifecycles, and assigns the effective convention to each
non-foreign KIR function. KIR function-pointer types carry their ABI and
indirect calls use it and preserve context-register parameters; direct
compatible scalar function references receive a thunk when their ABI changes.
One RIDL context pointer can be forwarded from an ordinary platform callback
parameter through a thunk. The C definition reports `KCC-E0310` for a cast from
a data pointer or integer to a function pointer; KIR itself accepts
`bitcast ptr -> fn` as a reload from untyped storage. Closure callbacks,
foreign and aggregate adapters, failing function pointers and memberwise
lifecycle synthesis are implemented as described in section 5.5's notes.

### 7.4 Example skeleton (informative)

```
import KLF

construct KArc() extends Language<CppTree, CppProgram> {
    let name = "karc"
    let driver = "karc"
    let version = "1.0.0"
    let extensions = ["cpp", "cc", "cxx", "h", "hpp", "inl", "krc", "krch"]
    let memory = .Unmanaged
    let exclusivity = .None
    let convention = .Ridl
    let stdRoot = "std"

    let fileModes: [FileMode] {
        return [FileMode { name = "arc", pragma = "karc arc", flag = "-fkarc-arc",
                           extensions = ["krc", "krch"],
                           memory = .Some(.Counted), convention = .None }]
    }

    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        return inputs.map({ file in return Unit.single(file) })
    }
    function parse(session: Session, unit: Unit) -> CppTree {
        let tokens = Preprocessor.run(session, unit)
        return CppParser.run(session, tokens)
    }
    function elaborate(session: Session, unit: Unit, tree: CppTree) -> CppProgram {
        return Sema.run(session, unit, tree)
    }
    function lower(session: Session, unit: Unit, program: CppProgram) -> KirModule {
        return Lowering.run(session, unit, program)
    }
}
```

Tables such as operator precedence are plain values inside the definition:

```
let binaryOperators: [Operator] = [
    Operator { symbol = "*", precedence = 120, associativity = .Left },
    Operator { symbol = "+", precedence = 110, associativity = .Left },
]
```

### 7.5 Rules

1. A package MUST contain exactly one declaration extending `Language` (`KLF-DEF-0001` otherwise). It MUST take no parameters.
2. Definition code MUST NOT perform I/O except through `Session`.
3. A definition MUST NOT be written in the language it defines. This holds by construction: `.klfd` is always KLF Kira.
4. A definition MUST produce raw KIR that passes the verifier's raw rules (section 11.7). Verifier failures in KIR produced from a well-formed user program are definition bugs and are reported as internal errors `KLF-KIR-9xxx` with the definition's name.

---

## 8. Kernel services API

All services are reached through the `Session` value handed to every phase, or through types of the `KLF` module. Signatures are KLF Kira. Identifiers of kernel-owned objects are 32-bit handles wrapped in `distinct` types; they are valid for the lifetime of the `Session`.

### 8.1 Sources and spans

```
struct SourceFile { let id: FileId  let path: String  let text: String }
struct Span { let file: FileId  let start: Int  let end: Int }
struct Position { let file: FileId  let offset: Int }

extend Session {
    function read(path: String) -> Result<SourceFile, IoError>
    function readDirectory(path: String, extension: String) -> Result<[SourceFile], IoError>
    function virtualFile(name: String, text: String, origin: Span) -> SourceFile
    function lineColumn(position: Position) -> (Int, Int)
    function includeSearch(name: String, from: FileId, system: Bool) -> Optional<String>
}
```

`read` returns the file already registered for a path when there is one. `readDirectory` reads every file under `path`, recursively, whose extension is `extension`, in path order. A file read during a phase is registered in the Session when the phase returns, before its diagnostics are resolved, so a diagnostic may point into it. In KLF Kira they are `readFile(session, path)` and `readDirectory(session, path, extension)`, which return a file id or a count, -1 on failure, and `source_directory_file(session, index)`.

`virtualFile` registers generated text (macro expansion, preprocessor output, template instantiation notes) so diagnostics and debug info can point into it with an origin chain.

### 8.2 Diagnostics

```
enum Severity { Error Warning Note Remark }
struct Diagnostic {
    let code: String
    let severity: Severity
    let span: Span
    let message: String
    let notes: [(Span, String)]
    let fixes: [(Span, String)]
}
extend Session {
    function report(diagnostic: Diagnostic)
    function errorCount() -> Int
}
```

`code` MUST be registered in the definition's `diagnostics` list. Diagnostics reported inside a speculation (8.7) are buffered and dropped on rollback.

### 8.3 Interning and arenas

```
distinct Symbol = Int
struct Arena<T> { }
distinct Id<T> = Int

extend Session {
    function intern(text: String) -> Symbol
    function text(symbol: Symbol) -> String
    function arena<T>(name: String) -> Arena<T>
}
extend Arena<T> {
    function alloc(value: T) -> Id<T>
    function get(id: Id<T>) -> T
    function set(id: Id<T>, value: T)
    function count() -> Int
}
```

Arenas are the only way to build cyclic or shared mutable structures. `get` returns a copy (cheap under copy-on-write); `set` replaces the element. Arena names are unique per `Session`; asking twice for the same name and type returns the same arena.

### 8.4 Scopes and name lookup

```
distinct ScopeId = Int
distinct EntityId = Int
enum Namespace { Value Type Tag Label Macro Custom(Symbol) }

extend Session {
    function scope(parent: Optional<ScopeId>, kind: Symbol) -> ScopeId
    function declare(in: ScopeId, name: Symbol, namespace: Namespace, entity: EntityId) -> Bool
    function lookup(from: ScopeId, name: Symbol, namespace: Namespace) -> [EntityId]
    function lookupLocal(in: ScopeId, name: Symbol, namespace: Namespace) -> [EntityId]
    function importEdge(from: ScopeId, to: ScopeId, transitive: Bool)
    function entity(payload: Int, kind: Symbol, span: Span) -> EntityId
}
```

`lookup` walks parent edges, then import edges, and returns all entities found at the innermost scope that has any (an overload set). `declare` returns false when the name already exists in that scope and namespace; redeclaration policy is the definition's. Argument-dependent lookup is performed by the definition using `lookupLocal` on associated scopes.

### 8.5 Types, layouts and policies

```
distinct TypeId = Int

extend Session {
    function intType(bits: Int, signed: Bool) -> TypeId
    function floatType(bits: Int) -> TypeId
    function pointerType() -> TypeId
    function vectorType(element: TypeId, count: Int) -> TypeId
    function tupleType(elements: [TypeId]) -> TypeId
    function arrayType(element: TypeId, count: Int) -> TypeId
    function functionType(signature: KirSignature) -> TypeId
    function nominalType(name: Symbol, layout: Layout, policy: Policy) -> TypeId
    function opaqueNominal(name: Symbol) -> TypeId
    function completeNominal(type: TypeId, layout: Layout, policy: Policy)
    function genericParam(name: Symbol, bounds: [TraitId]) -> TypeId
    function refType(pointee: TypeId) -> TypeId
    function uniqType(pointee: TypeId) -> TypeId
    function weakType(pointee: TypeId) -> TypeId
    function sizeOf(type: TypeId) -> Optional<Int>
    function alignOf(type: TypeId) -> Optional<Int>
}

enum Layout {
    Struct(fields: [Field], rule: LayoutRule)
    Enum(variants: [Variant], rule: LayoutRule)
    Explicit(size: Int, align: Int, fields: [PlacedField])
}
enum LayoutRule { Ridl PlatformC Packed(Int) }
```

`LayoutRule.Ridl` applies section 21. `LayoutRule.PlatformC` applies the target's C layout rules, including bit-fields, through the kernel's `CLayout` tables. `Layout.Explicit` lets a definition compute layouts itself (C++ classes with vtable pointers and virtual bases). `sizeOf` returns `.None` for types whose size depends on a generic parameter.

`Policy` is the policy record of section 11.2. When a definition passes `Policy.default`, the kernel derives the record from the language's `memory` model and the member types.

### 8.6 Traits, conformances and the constraint solver

```
distinct TraitId = Int
extend Session {
    function trait(name: Symbol, requirements: [Requirement], parents: [TraitId]) -> TraitId
    function conformance(type: TypeId, trait: TraitId, witnesses: [KirFunctionRef], associated: [TypeId])
    function conforms(type: TypeId, trait: TraitId) -> Bool
    function solver() -> Solver
}
extend Solver {
    function variable(origin: Span) -> TypeId
    function equate(a: TypeId, b: TypeId, origin: Span)
    function conform(type: TypeId, trait: TraitId, origin: Span)
    function convertible(from: TypeId, to: TypeId, cost: Int, origin: Span)
    function disjunction(choices: [Choice], origin: Span)
    function solve(limit: Int) -> Solution
}
enum Solution { Unique(Bindings) Ambiguous([Bindings]) Failed([Conflict]) TooComplex }
```

The solver is a unification engine with scored disjunctions. It is sufficient for Kira's local inference, C++ overload resolution (each candidate is a choice whose cost vector is compared lexicographically) and Swift-style bidirectional inference. Conversions and their costs are supplied by the definition. `limit` bounds the number of explored states; `TooComplex` lets the definition report its own diagnostic.

### 8.7 Speculation

```
extend Session {
    function speculate<T>(body: () -> Optional<T>) -> Optional<T>
}
```

Runs `body`. If it returns `.None`, every arena write, scope declaration, conformance, instantiation and buffered diagnostic made inside is rolled back. Speculations nest. This is the mechanism for SFINAE, tentative parsing and overload probing.

### 8.8 Instantiation

```
extend Session {
    function instance(generic: EntityId, arguments: [TypeId], build: () -> EntityId) -> EntityId
}
```

Memoized by (generic, arguments). Thread-safe: concurrent requests for the same key run `build` once. Instances are shared across units of one build and persisted in the cache keyed additionally by the definition hash and the hashes of the source ranges of the generic.

### 8.9 Queries and caching

```
trait Cacheable {
    function encode(borrow self, into: borrow mut Encoder)
    function decode(from: borrow mut Decoder) -> Self
}
extend Session {
    function query<K: Cacheable, V: Cacheable>(name: String, key: K, compute: (K) -> V) -> V
    function input(path: String) -> String
}
```

`query` memoizes `compute` by (name, key). The kernel records which inputs and which other queries `compute` read; a later build re-runs it only if a dependency changed (red-green validation). Query results are persisted through `Cacheable`. The three phases of 7.2 are queries named `parse`, `elaborate`, `lower` keyed by unit. `compute` MUST be a pure function of its key and of what it reads through `Session`.

### 8.10 Compile-time evaluation

```
extend Session {
    function evaluate(function: KirFunctionRef, arguments: [ConstValue], limits: EvalLimits) -> Result<ConstValue, EvalError>
}
struct EvalLimits { let steps: Int  let memoryBytes: Int  let callDepth: Int }
```

Compiles the KIR function and its callees to bytecode and runs it in a sandboxed VM instance with no host capabilities (section 14.4). Defaults: 50,000,000 steps, 256 MiB, call depth 4,096.

### 8.11 KIR builder, targets, driver, tooling

- `KirBuilder`: one method per instruction of section 12, plus `module`, `function`, `block`, `blockParam`, `global`, `vtable`. Builder methods return `ValueId`. The builder checks operand types eagerly and reports `KLF-KIR-9001` on misuse.
- Definition services include `klf.kir.type.enum_named(module, nameSpan, specializationIndex)`. It declares an ordinary enum named `<source-name>_G<specializationIndex>` so a language frontend can monomorphize a generic enum into concrete KIR types.
- `klf.kir.type.size(module, type)` queries concrete type size using the same layout rules as the backends. Low-level float builders accept a type handle and source span for literals, and typed handles for arithmetic, comparisons and negation. Literal spans use the same package-file selectors as symbol spans.
- Definition services include `klf.kir.type.enum_named(module, nameSpan, specializationIndex)`. It declares an ordinary enum named `<source-name>_G<specializationIndex>` so a language frontend can monomorphize a generic enum into concrete KIR types.
- `session.target`: triple, pointer width, endianness, `CLayout` (sizes and alignments of the C scalar types, bit-field rules, `long double` format), and the feature set.
- `session.flag(name)`: the effective typed value of a declared driver flag. Its type is the `DriverFlagValue` case and its default is the declaration's `default_value`.
- `session.file_mode(sourceFile)`: zero-based selected `FileMode` index for that source, or `-1` when no mode was selected.
- `session.file_memory(sourceFile)` and `session.file_convention(sourceFile)`: typed effective per-file values, each resolving a selected FileMode override against its Language package default. The memory model and Language exclusivity derive KIR defaults for `.klfd` types whose member lifecycles are known trivial, and the effective convention is stored on each non-foreign KIR function and function-pointer type. LLVM indirect calls use the pointer ABI and preserve context-register parameters. Direct matching scalar function references can be adapted with a thunk; one RIDL context pointer can be forwarded from an ordinary platform callback parameter. The C definition rejects casts from data pointers or integers to function pointers with `KCC-E0310`. Closure callbacks, foreign and aggregate adapters, failing function pointers and memberwise lifecycle synthesis are implemented.
- `Lexer` and `Pratt` helpers: an optional table-driven lexer (longest match over literal and character-class rules) and an operator-precedence expression parser taking `[Operator]`. Definitions MAY hand-write both.
- Language server and editors: specified by [`KLF-EDITOR.md`](KLF-EDITOR.md), which also amends the editor hooks of 7.2.

### 8.12 Kernel service ABI (internal)

Each service is a Rust function exported with the platform C convention, taking and returning only integers, floats and (pointer, length) pairs. `kernel/services.manifest` lists every entry point with its signature; the KLF Kira prelude's wrappers are generated from it and marked `@Service`. This ABI is internal to one KLF release. Services are coarse: no service is called per character or per token; lexing helpers operate on whole buffers, and arena access is inlined by LTO.

---

## 9. Frontend pipeline, incrementality and scale

### 9.1 Driver loop

1. Parse the command line (or `compatDriver`), resolve inputs, create the `Session`.
2. `units(session, inputs)`.
3. For each unit, in parallel on a work-stealing pool sized to the hardware: query `parse`, then `elaborate`, then `lower`.
4. For each `KirModule`: verify raw KIR, run mandatory passes, verify canonical KIR, run the generics engine, optimize, then hand to the selected backend.
5. Link.

### 9.2 Phase contract

| Phase | Input | Output | May |
| --- | --- | --- | --- |
| `parse` | Unit | Tree | Read files through `Session`, create virtual files, call elaboration helpers for context-sensitive grammars, speculate |
| `elaborate` | Unit, Tree | Program | Declare scopes and entities, create types, solve constraints, instantiate, evaluate at compile time |
| `lower` | Unit, Program | KirModule | Build raw KIR, declare exports, attach debug info |

A language with pre-parse phases (the C preprocessor, Kira's macro expansion) performs them inside `parse`, optionally as sub-queries so their results cache separately (for example one query per included header keyed by file hash and by the hash of the macro state the header observed).

### 9.3 Scale requirements

An implementation MUST satisfy all of the following on the UE5 corpus:

1. Translation units compile in parallel with no shared lock held across a phase.
2. Generic and template instances are built once per build and reused from the cache across builds.
3. Header trees are cached and reused across units when the observed macro state matches.
4. Peak memory per unit MUST NOT exceed 1.5 times clang's peak memory on the same unit, and wall-clock time of a full build MUST NOT exceed 2.0 times clang's at the same optimization level on the same machine.
5. An edit to one source file re-runs only the queries whose inputs changed.

### 9.4 Determinism under parallelism

Handles (`TypeId`, `EntityId`, ...) are allocated per thread in disjoint ranges and are never written to output. Every output ordering (symbols, KIR functions, diagnostics) MUST be a function of source order, not of scheduling.

---

## 10. KIR: structure, types and textual format

### 10.1 Module

A KIR module has: a name, the `lang` that produced it, a target triple, a stage (`raw` or `canonical`), type declarations with layouts and policy records, globals, function declarations and definitions, vtables, trait and conformance records, RIDL export and import records, and debug info.

### 10.2 Types

| Type | Syntax | Notes |
| --- | --- | --- |
| Integers | `i1 i8 i16 i32 i64 i128` | Two's complement, signedness is per operation |
| Floats | `f32 f64` | IEEE 754 |
| Raw pointer | `ptr` | Trivial. Width from target |
| Unit, never | `unit`, `never` | |
| Vector | `vec<N x T>` | SIMD |
| Tuple | `(T, U, ...)` | |
| Fixed array | `[N x T]` | |
| Nominal | `%Name` or `%Name<T, ...>` | Declared with a layout and a policy |
| Generic parameter | `$T` | Only inside generic functions and types |
| Counted reference | `ref T` | Strong reference to a heap object |
| Unique reference | `uniq T` | Sole owner of a heap object |
| Weak reference | `weak T` | |
| Function | `fn([context] [conv] T, ...) -> R` with optional `fails E` and `abi(platform|ridl)` | `context` marks the single pointer parameter passed in the RIDL context register; `conv` is `owned`, `borrow`, `borrowmut`; omitted for trivial types. ABI defaults to platform in legacy text; `abi(ridl)` marks RIDL. |
| Address | `*T` | Address of storage holding a `T`. Exists only as an SSA operand type; never stored |

A type is loadable if its layout is known and its relocate policy is `bitwise`. Any other type is address-only: values of it exist only in memory and are manipulated through `*T`.

### 10.3 Functions

A function has: a symbol, a linkage (`private`, `module`, `exported`), a signature with conventions, optional generic parameters with bounds, an engine attribute (`runtime`, `native`, `any`), an ABI (`platform` or `ridl`; omitted text means platform), attributes (`inline`, `noinline`, `cold`, `noreturn`, `returns_twice`, `fastmath`, `unmanaged`, `nounwind`), and a body of basic blocks. The optional `abi(platform|ridl)` token follows `engine(...)`. Function-pointer types carry the same ABI and per-parameter context marker; `call.indirect` uses the target type's ABI and context-register parameter. Blocks take parameters (there are no phi nodes). The first block's parameters are the function's parameters.

### 10.4 Textual format (example)

```
module @Geometry lang "kira" target "aarch64-linux" stage canonical

type %Vec3 = struct ridl { x: f64, y: f64, z: f64 }
     policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: none }

func exported @Geometry.length(%v: borrow %Vec3) -> f64 engine(any) {
bb0(%v: borrow %Vec3):
  %x = struct.extract %v, 0            !loc("geo.kira":4:12)
  %y = struct.extract %v, 1
  %z = struct.extract %v, 2
  %xx = fmul %x, %x
  %yy = fmul %y, %y
  %zz = fmul %z, %z
  %s0 = fadd %xx, %yy
  %s1 = fadd %s0, %zz
  %r = fsqrt %s1
  return %r
}
```

The full grammar is Appendix A.4. `.kirb` is the same structure in a binary encoding private to each KLF release.

---

## 11. KIR ownership model

### 11.1 Value kinds

| Kind | Obligation |
| --- | --- |
| trivial | None. May be used any number of times |
| owned | Consumed exactly once on every path from its definition to a function exit |
| borrowed | Produced by `borrow.begin`, `load.borrow` or a `borrow` parameter. Never consumed. Every use is dominated by the begin and post-dominated by the matching end. A borrowed value MUST NOT be stored, returned, or captured by an escaping closure |
| weak | An owned value of type `weak T`. Loading it yields `Optional<ref T>` |

The kind of a value follows from its type's policy record: a type whose copy and destroy are both `trivial` produces trivial values; every other type produces owned values (or borrowed ones inside borrow scopes).

### 11.2 Policy record

| Field | Values | Meaning |
| --- | --- | --- |
| `copy` | `trivial`, `implicit(@f)`, `explicit(@f)`, `none` | How a second owner comes to exist. `implicit`: the ownership pass may insert copies. `explicit`: only a source-level copy operation may. `none`: never |
| `destroy` | `trivial`, `fn(@f)` | What runs when an owned value dies |
| `relocate` | `bitwise`, `fn(@f)` | How a value moves between addresses. `fn` makes the type address-only |
| `exclusivity` | `static`, `dynamic`, `none` | Default enforcement for `access.begin [modify]` on storage of this type |
| `pairing` | `counted`, `none` | `counted` promises that copy and destroy are a retain and release pair on shared state, so the optimizer may cancel adjacent pairs and reorder them across code that cannot observe the count |

Signature of policy functions: `copy(dst: ptr, src: ptr)`, `relocate(dst: ptr, src: ptr)`, `destroy(obj: ptr)`; for generic types each takes the type's descriptor as a final `ptr` argument.

Defaults derived from `MemoryModel` when a definition passes `Policy.default`:

| Model | Aggregates | Pointer-like |
| --- | --- | --- |
| Unmanaged | copy trivial, destroy trivial | `ptr`, trivial |
| Counted | memberwise: implicit copy if any member is non-trivial | `ref T`: copy implicit(retain), destroy release, pairing counted |
| Affine | memberwise: copy `none` unless the definition marks the type copyable, then `explicit` | `uniq T`: copy none, destroy fn |

### 11.3 Conventions

| Convention | Caller | Callee |
| --- | --- | --- |
| `owned` | Gives up the value | Must consume it |
| `borrow` | Keeps ownership, guarantees the value outlives the call | Receives a borrowed value |
| `borrowmut` | Holds exclusive access to the storage for the call | Receives `*T` with modify access |

Results are owned or trivial. A function type with `fails E` has a second, owned, error result delivered on the error edge of `try.call`.

### 11.4 Borrow scopes and exclusivity

`borrow.begin %v` opens a scope in which `%v` must not be consumed; `borrow.end` closes it. Scopes are lexical: properly nested and closed on every path. Kira's `with` regions, Swift's `borrowing` and C++ reference parameters in ARC mode lower to these scopes.

`access.begin [read or modify] [static or dynamic or unchecked] %addr` brackets a memory access. Under `static` the verifier proves that no two overlapping accesses to the same storage conflict (two modifies, or a modify and a read). Under `dynamic` the backend emits a runtime check through `klfrt_access_begin` and traps with `KLF-RT-0003` on conflict. Under `unchecked` nothing is enforced.

### 11.5 Unmanaged escape

`into.raw %v` consumes an owned value and yields a `ptr`; the verifier stops tracking it. `from.raw T, %p` yields an owned value from a pointer. Both, together with `raw.load`, `raw.store`, `ptr.offset`, `heap.alloc.raw`, `heap.free.raw`, `asm` and `call.foreign`, are the unmanaged instructions. A function that contains one is recorded in the module's unmanaged manifest (section 31.6).

### 11.6 Mandatory passes (raw to canonical)

Frontends emit raw KIR: every source-level use is a use, source-level explicit copies and moves are `copy` and `move` instructions, and there are no `destroy` instructions except those with source-level meaning (for example C++ `delete` in KArc's C++ mode, which is a call anyway). The kernel then runs, in order:

1. Definite initialization. Every `load`, `copy.addr` source and `destroy.addr` operand is initialized on every path; every `store [init]` target is uninitialized. Violations are reported through the language's registered codes `init.use-before-init` and `init.double-init`.
2. Copy and move resolution. For each owned value with more than one consuming use on some path: if its copy policy is `implicit`, insert `copy` before all but the last consuming use on each path; otherwise report `own.use-after-move` at the second use. A `move` instruction marks its operand dead afterwards even for `implicit` types.
3. Borrow checking. Enforce 11.1 for borrowed values and 11.4 for `static` accesses; report `own.borrow-escape`, `own.consume-while-borrowed`, `own.exclusivity`.
4. Destroy insertion. For each owned value not consumed on a path, insert `destroy` at the earliest point after its last use on that path (end of lexical scope if the type's definition requests `scoped` lifetimes through the module attribute `lifetimes(scoped)`, which C++ and Kira `Drop` types require).
5. Address-only lowering. Rewrite value operations on address-only and generic types into `copy.addr`, `move.addr`, `destroy.addr` on stack slots.

Each diagnostic key above maps to a code in the definition's registry, so a Kira user sees `KSEM107` and a KArc user sees a KArc code.

### 11.7 Verifier rules

Raw rules (R) apply to raw KIR; all rules apply to canonical KIR.

| Rule | Statement |
| --- | --- |
| V1 (R) | SSA: every value is defined once and dominates its uses. Block arguments match parameters in count and type |
| V2 (R) | Every instruction is well typed per section 12 |
| V3 (R) | Every block ends in exactly one terminator |
| V4 | Every owned value is consumed exactly once on every path to `return`, `fail`, `resume` or `unreachable` (paths ending in `trap` are exempt) |
| V5 | No borrowed value is an operand of a consuming instruction, of `store`, of `return`, or of a closure capture marked escaping |
| V6 | Every `borrow.begin` and `load.borrow` has exactly one matching end on every path, properly nested |
| V7 | `static` accesses do not conflict |
| V8 | Address-only types never appear as loadable SSA values |
| V9 | A function with engine `runtime` or `any` contains no `asm` |
| V10 | `try.call` is used if and only if the callee type has `fails`; `invoke` is used only in functions without `nounwind` |
| V11 | Generic parameters appear only in generic functions; every `$T` operation that needs size or witnesses has the descriptor available (section 13) |
| V12 | Exported functions use only RIDL-representable types in their signatures (section 20) |

---

## 12. KIR instruction set

Notation: `T` a type, `%v` a value, `@f` a symbol, `bbN` a block. Unless stated, results are trivial for trivial types and owned otherwise. Every instruction may carry `!loc(file:line:col)`.

### 12.1 Constants

| Instruction | Result |
| --- | --- |
| `const.int T, n` | Integer of type T |
| `const.float T, x` | Float |
| `const.bool b` | `i1` |
| `const.null` | `ptr` |
| `const.zero T` | Zero value of a trivial T |
| `const.string "text"` | `(ptr, i64)`: address and byte length of immutable UTF-8 data |
| `func.ref @f [generic args]` | Function value |
| `global.addr @g`, `tls.addr @g` | `ptr` |
| `descriptor.ref T` | `ptr` to T's type descriptor |
| `witness.ref T, Trait` | `ptr` to the witness table |
| `block.addr bbN` | `ptr` for `br.indirect` |

### 12.2 Integer operations

`add`, `sub`, `mul` take an overflow mode `[wrap]`, `[trap]` or `[sat]` and a signedness `s` or `u` when the mode is not `wrap`. `add.ovf`, `sub.ovf`, `mul.ovf` return `(T, i1)`. `sdiv`, `udiv`, `srem`, `urem` trap with `KLF-RT-0010` on division by zero; `sdiv` and `srem` of the minimum value by -1 trap in mode `[trap]` and wrap otherwise. `and`, `or`, `xor`, `not`, `neg`. `shl`, `lshr`, `ashr`: the shift amount is reduced modulo the bit width in mode `[wrap]` and traps when out of range in mode `[trap]`. `icmp pred` with `eq ne slt sle sgt sge ult ule ugt uge` returns `i1`. `ctlz`, `cttz`, `ctpop`, `bswap`, `bitreverse`, `rotl`, `rotr`. `select %c, %a, %b`. KIR has no undefined behavior and no poison: every instruction has a defined result or a defined trap.

### 12.3 Floating point

`fadd fsub fmul fdiv frem fneg fabs fsqrt fmin fmax ffloor fceil ftrunc fround fma`, `fcmp pred` with `oeq one olt ole ogt oge ord uno ueq une ult ule ugt uge`. Evaluation is strict IEEE 754 in round-to-nearest-even unless the function has `fastmath`. `fmin` and `fmax` follow IEEE 754-2019 `minimum` and `maximum`.

### 12.4 Conversions

`trunc`, `zext`, `sext`, `fptrunc`, `fpext`, `sitofp`, `uitofp`, `fptosi [sat or trap]`, `fptoui [sat or trap]`, `ptrtoint`, `inttoptr`, `bitcast` (same size, both trivial).

### 12.5 Vectors and intrinsics

`vec.splat`, `vec.extract`, `vec.insert`, `vec.shuffle`. Arithmetic instructions accept vectors elementwise. `intrinsic "name"(args) -> T` names a target intrinsic from the kernel's intrinsic table (the table maps each name to an LLVM intrinsic and to a portable VM implementation; a name without a VM implementation forces engine `native`).

### 12.6 Aggregates

`struct.new %T(fields)`, `struct.extract %v, i` (trivial field or borrowed operand only), `struct.destructure %v` (consumes, yields all fields), `tuple.new`, `tuple.extract`, `tuple.destructure`, `array.new`, `enum.new %E, variant(payload)`, `enum.tag %v` (yields the variant index as the enum's tag type), `enum.take %v, variant` (consumes, yields payload; precondition: tag equals variant), `enum.project %b, variant` (borrowed payload from a borrowed enum).

### 12.7 Storage and addresses

`stack.alloc T` (or `stack.alloc $T` with dynamic size from the descriptor) yields `*T`; `stack.dealloc %a` in LIFO order. `field.addr %a, i`, `elem.addr %a, %index`, `enum.payload.addr %a, variant`, `enum.inject %a, variant` (writes the tag after the payload was initialized), `addr.cast %a to *U` (layout-compatible types only), `addr.toptr %a` yields `ptr` (unmanaged).

### 12.8 Memory

| Instruction | Meaning |
| --- | --- |
| `load [trivial] %a` | Read a trivial value |
| `load [copy] %a` | Read and copy (requires a copy policy) |
| `load [take] %a` | Move out; storage becomes uninitialized |
| `load.borrow %a` ... `borrow.end %b` | Borrow the stored value in place |
| `store [trivial or init or assign] %v to %a` | `assign` destroys the old value first |
| `copy.addr %src to [init or assign] %dst` | Copy through the type's policy or descriptor |
| `move.addr %src to [init or assign] %dst` | Relocate; source becomes uninitialized |
| `destroy.addr %a` | Destroy in place |
| `raw.load T, %p [align n] [volatile]`, `raw.store %v, %p [align n] [volatile]` | Unmanaged access to trivial T |
| `ptr.offset %p, %bytes`, `ptr.diff %p, %q` | Byte arithmetic |
| `mem.copy`, `mem.move`, `mem.set` | Byte operations |

### 12.9 Heap and references

`heap.alloc.counted T` yields an owned `ref T` with strong count 1 and uninitialized payload; `heap.alloc.unique T` yields `uniq T`; `ref.addr %r` and `uniq.addr %u` yield the payload address inside a borrow of the reference; `heap.alloc.raw %size, %align` and `heap.free.raw %p` are unmanaged, and `heap.free.raw` of a slot whose strong count is already zero traps with `KLF-RT-0006` instead of freeing it twice; `weak.new %r` yields an owned `weak T` from a borrowed `ref T`; `weak.load %w` yields `%Optional<ref T>`; `ref.unowned %r` yields `ptr` without ownership; `ref.isunique %r` yields `i1` (used for copy-on-write).

### 12.10 Ownership

`copy %v`, `move %v`, `destroy %v`, `borrow.begin %v`, `borrow.end %b`, `into.raw %v`, `from.raw T, %p`, `access.begin [read or modify] [static or dynamic or unchecked] %a`, `access.end %acc`.

### 12.11 Calls

| Instruction | Meaning |
| --- | --- |
| `call @f<generic args>(args)` | Direct call |
| `call.indirect %fn(args)` | Through a function value |
| `call.closure %c(args)` | Closure call; context travels in the context register |
| `closure.callback <i>, fn(...)` | A platform function whose parameter `i` is a user-data pointer to a closure record; calling it calls that closure (section 26.4) |
| `call.witness T, Trait.method(args)` | Through T's witness table; resolved statically when T is concrete |
| `call.vtable %obj, @Vtable, slot(args)` | Through a declared `vtable`; eligible for devirtualization |
| `try.call callee(args) normal bbN, error bbE` | For `fails` callees. `bbN` receives the result, `bbE` the owned error |
| `call.foreign @sym(args)` | Platform C convention, symbol declared by a `.klffi` import. Unmanaged |
| `call.variadic callee(fixed args; variadic args)` | C variadics (section 29.5) |
| `invoke callee(args) to bbN unwind bbU` | Call that may unwind |

### 12.12 Unwinding

`landingpad [cleanup] [catch %typeinfo ...]` begins an unwind block and yields `(ptr, i32)`; `resume %lp` continues unwinding; `throw %box` starts unwinding with an owned RIDL box. Unwinding never crosses a RIDL boundary: an exception reaching an exported function's frame traps with `KLF-RT-0020`. Modules compiled with `nounwind` on every function (the default for Kira, C and KArc without `-fexceptions`) contain none of these.

### 12.13 Terminators

`br bbN(args)`, `cond.br %c, bbT(args), bbF(args)`, `switch.int %v, [n: bbN]..., default bbD`, `switch.enum %v, [variant: bbN]..., [default bbD]` (consumes an owned enum; each target receives the payload as block parameters; the default is mandatory unless all variants are listed), `br.indirect %addr, [bb...]`, `return %v`, `fail %e`, `unreachable` (executing it traps with `KLF-RT-0001`), `trap code`.

### 12.14 Concurrency, variadics, non-local jumps, assembly

- Atomics: `atomic.load`, `atomic.store`, `atomic.rmw op` (xchg add sub and or xor nand max min umax umin), `atomic.cmpxchg [weak]`, each with C11 memory orders; `fence`.
- Variadics: `va.start`, `va.arg T`, `va.copy`, `va.end`.
- `setjmp %buf` yields `i32` in a function marked `returns_twice`; `longjmp %buf, %v`. Jumping over a frame that holds owned values with non-trivial destroy is a definition-level error for KArc and Kira and is permitted for C.
- `asm "template", "constraints"(args)`: native engine only.
- There are no coroutine instructions. A language with async functions lowers them in its definition to a state-machine struct plus a resume function, using ordinary KIR.

### 12.15 Debug and hints

`debug.value %v, !var(name, type, scope)`, `debug.addr %a, !var(...)`, `assume %c`, `expect %v, n`, `nop`.

### 12.16 Generic support

`type.size T`, `type.align T`, `type.stride T` yield `i64` and read the descriptor when T is generic.

---

## 13. Generics engine

### 13.1 Two strategies

1. Specialization. For every use of a generic with concrete arguments inside a module, the engine clones the generic's KIR with arguments substituted, deduplicating by (generic, arguments). Languages that only monomorphize (Kira, KArc templates after instantiation, C has none) use this for all internal code.
2. Polymorphic entry. For every generic function and generic type that is exported, the engine also emits exactly one unspecialized body that receives, per type parameter, a descriptor pointer and one witness table pointer per bound, with generic values handled by address (section 23).

### 13.2 Rules

- A definition requests strategy per generic: `specialize`, `polymorphic` or `both`. Exported generics are forced to `both`. Swift's definition uses `polymorphic` for non-inlinable generics and lets the optimizer specialize.
- A generic whose body cannot be expressed polymorphically is rejected at export with `ridl.generic-not-polymorphic`. The cases are: a type parameter used as a compile-time constant in a type (array length from `type.size`), a non-type generic parameter, and a body that specializes behavior on the concrete type. Such a generic may still be exported as named instances.
- Named instances requested by the definition (for example through a Kira annotation or a KArc explicit instantiation in an exported header) are emitted with the symbols of section 24.2. A caller that sees a matching named instance in the provider's `.ridl` MUST call it in preference to the polymorphic entry.
- Inside a polymorphic body, the engine materializes descriptors for nested instances (`Pair<$T, i64>`) by calling the nested generic type's builder (section 23.4) once per function activation and caching the result in a stack slot.

---

## 14. Optimization pipeline and compile-time evaluation

### 14.1 Pipeline

The kernel optimizes KIR itself, in two tiers, before any backend sees it. Tier 1 is target- and backend-independent. Tier 2 runs once per backend and reshapes the tier-1 result into the KIR that backend optimizes best. A backend's own pipeline runs last, on what tier 2 returns.

| Stage | Passes |
| --- | --- |
| O0 mandatory | Section 11.6 only |
| O1 (tier 1) | Inlining of `@Inline` and trivial functions, constant folding, dead code elimination, CFG simplification, copy propagation, retain and release pairing for `pairing: counted` types |
| O2 (tier 1) | Everything in O1, then: promotion of stack slots to SSA values, generic specialization of hot polymorphic calls, devirtualization of `call.vtable` and `call.witness`, scalar replacement of aggregates, redundant load elimination under access scopes, loop-invariant code motion, borrow-to-copy and copy-to-borrow conversion, destroy hoisting, uniqueness-check elision for copy-on-write, inlining by cost model |
| Tier 2 | Per backend, below |
| Backend | LLVM's pipeline at the matching level for native through LLVM; Cranelift's `speed` level for native through Cranelift; bytecode peephole and register coalescing for the VM |

Tier 2 per backend:

| Backend | Tier 2 |
| --- | --- |
| Cranelift | Cranelift neither inlines nor promotes memory, so tier 2 inlines by a larger cost budget than tier 1 and repeats tier 1's promotion, folding and simplification over the result |
| LLVM | LLVM inlines and promotes memory itself, so tier 2 returns the tier-1 result unchanged |
| VM | Every instruction is a dispatch, so tier 2 removes the moves, empty blocks and argument-only branches tier 1 leaves |

A build runs O0 unless it asks for the KIR optimizer (`--profile release`, or a language whose `optimize` driver flag is true); then it runs O2, tier 2 and the backend's pipeline. A language's `release` flag alone does not run the optimizer. The observable behavior after optimization MUST equal the behavior of the O0 program. Destroy timing of types under `lifetimes(scoped)` is observable and MUST NOT change.

### 14.2 Language passes

`Language.passes` returns values of:

```
struct Pass {
    let name: String
    let stage: PassStage
    let run: (Session, KirFunctionRef) -> Bool
}
enum PassStage { AfterMandatory BeforeBackend }
```

Language passes run on canonical KIR and MUST leave it canonical; the verifier runs after each in debug builds of the driver.

### 14.3 Retain and release optimization

For types with `pairing: counted` the optimizer may: cancel a `copy` immediately followed on all paths by a `destroy` of the copy or of the original; replace an owned argument plus a callee-side destroy by a borrowed argument when the callee is known; hoist and sink retains and releases across instructions that cannot release the object. It MUST treat any call to unknown code, any `raw.store` and any `destroy` of another value as able to release.

### 14.4 Compile-time evaluation

`Session.evaluate` and language features built on it (C++ `constexpr`, Kira `comptime`, template non-type argument evaluation) execute KIR on a VM instance with: no host capabilities, no foreign calls, no threads, no `asm`; a private heap limited by `memoryBytes`; a step counter. Results are converted to `ConstValue` (integers, floats, aggregates of those, strings, and addresses of globals with offsets). An evaluation that traps returns `EvalError.trap(code, stack)`. Evaluation is deterministic and independent of the host.

---

## 15. Bytecode and the VM

### 15.1 Portability

The VM core MUST compile for `wasm32-unknown-unknown` with no filesystem, process, thread or dynamic loading dependency. It reaches the outside world only through host capabilities passed at instance creation. Bytecode is specific to a pointer width (4 or 8 bytes) and a layout target, both recorded in the header, because addresses and layouts are those of KIR.

### 15.2 Module file `.klbc`

All integers little-endian.

```
header:  magic 4B 4C 42 43, u16 major = 1, u16 minor = 1, u8 ptrwidth, u8 reserved[3],
         u32 flags, u32 sectionCount
section table entry: u32 kind, u32 offset, u32 byteSize, u32 count
```

| Kind | Section | Content |
| --- | --- | --- |
| 1 | STRINGS | u32 length plus UTF-8, repeated |
| 2 | TYPES | Size, alignment, policy function indices per nominal type |
| 3 | CONSTS | 64-bit and 128-bit constants, float constants, string references |
| 4 | GLOBALS | Size, alignment, initializer bytes, relocations |
| 5 | FUNCS | name, signature, register count u32, frame bytes u32, code offset u32, code length u32, call-site table, handler table, flags |
| 6 | CODE | Instruction words |
| 7 | DESCRIPTORS | Type descriptors in the layout of section 23.1 with function indices in place of addresses |
| 8 | WITNESSES | Witness tables likewise |
| 9 | IMPORTS | Module, symbol, kind (bytecode, native, runtime, foreign), signature |
| 10 | EXPORTS | Symbol, function index |
| 11 | DEBUG | Line table, variable locations, source file list |
| 12 | META | RIDL metadata of section 24.3 and the unmanaged manifest |
| 13 | BRANCHES | Per-function branch sites, targets and parallel register transfers |

Version 1.0 has no `BRANCHES` section and retains its relative `JMP`, `JIF` and
`JIFN` control flow. Version 1.1 requires `BRANCHES`; its section-table count is
the function count. For each function the section stores a `u32` site count,
followed by each site's condition tag and payload, its then-edge, and an
else-edge tag (0 or 1) followed by the else-edge when present. Condition tag 0
has no payload, tag 1 has a `u32` register, and tag 2 has `u32` result/lhs/rhs
registers followed by a `u8` width in bits and a `u8` integer predicate. An
edge stores a `u32` code offset, a `u32` move count, then source/destination
register pairs as `u32`s. Transfers read all sources before writing
destinations, so swaps are parallel.

### 15.3 Machine model

A frame has N registers of 64 bits (N at most 65,535) and a byte-addressed frame area for aggregates, address-taken locals and `i128` spills. Registers hold integers, floats (bit patterns), and addresses. `i128` values occupy two consecutive registers, low word first. Addresses are host addresses.

### 15.4 Instruction encoding

Each instruction is one 64-bit word: bits 0 to 7 opcode, 8 to 15 flags, 16 to 31 operand A, 32 to 47 operand B, 48 to 63 operand C. Forms: ABC (three registers), ABx (A register, B and C form a signed 32-bit immediate or index), Ax (A, B, C form a 48-bit value).

Flags for arithmetic: bits 0 to 2 width code (0: 8, 1: 16, 2: 32, 3: 64, 4: 128), bit 3 signed, bits 4 to 5 overflow mode (0 wrap, 1 trap, 2 saturate). Flags for loads and stores: width code, bit 3 sign-extend, bit 6 volatile.

### 15.5 Opcodes

Opcodes are assigned sequentially within each range, in the order listed.

| Range | Opcodes |
| --- | --- |
| 0x00 to 0x0F | NOP, MOV, MOV2, LOADK, LOADI, LOADNULL, LEAF (frame address), LEAG (global address), LEAFN (function value), LEAD (descriptor), LEAW (witness), SELECT |
| 0x10 to 0x2F | ADD, SUB, MUL, SDIV, UDIV, SREM, UREM, AND, OR, XOR, NOT, NEG, SHL, LSHR, ASHR, ROTL, ROTR, ADDO, SUBO, MULO, CTLZ, CTTZ, CTPOP, BSWAP, BITREV, ICMP (predicate in C), ADDI, MULI |
| 0x30 to 0x47 | FADD, FSUB, FMUL, FDIV, FREM, FNEG, FABS, FSQRT, FMIN, FMAX, FFLOOR, FCEIL, FTRUNC, FROUND, FMA, FCMP |
| 0x48 to 0x57 | TRUNC, ZEXT, SEXT, FPTRUNC, FPEXT, SITOFP, UITOFP, FPTOSI, FPTOUI, BITCAST |
| 0x58 to 0x6F | LOAD, STORE, MCPY, MMOVE, MSET, PTRADD, PTRDIFF, ALLOCA (dynamic), HALLOCC (counted), HALLOCU (unique), HALLOCR (raw), HFREER, RETAIN, RELEASE, WNEW, WLOAD, WRELEASE, ISUNIQ, CPYADDR (through descriptor), MOVADDR, DSTADDR, ACCBEG, ACCEND |
| 0x70 to 0x7F | JMP (0x70), JIF (0x71), JIFN (0x72), SWITCH (0x73), JIND (0x74), CALL (0x75, A dest and Bx call-site index), CALLI (0x76), CALLC (0x77), RET (0x78), RETV (0x79), FAIL (0x7A), TRAP (0x7B), UNREACH (0x7C), BR (0x7D), BR_IF (0x7E), BR_CMP (0x7F) |

`BR`, `BR_IF` and `BR_CMP` carry a branch-site index in signed `Bx`; targets
and block-parameter transfers come from `BRANCHES`. `BR` takes the then-edge.
`BR_IF` tests register A's low bit and takes the then-edge when it is set;
`BR_CMP` performs the recorded integer comparison, writes its boolean result
register, and selects the then- or else-edge.
| 0x80 to 0x8F | ALOAD, ASTORE, ARMW, ACMPXCHG, FENCE, VASTART, VAARG, VACOPY, VAEND, SETJMP, LONGJMP, THROW, RESUME, LPAD |
| 0x90 to 0x9F | VSPLAT, VEXT, VINS, VSHUF, VOP (vector arithmetic, sub-opcode in C), INTRIN (intrinsic table index) |
| 0xF0 to 0xFF | DBGVAL, BREAK, PROF, EXT (extends operand ranges of the following word) |

A call-site entry holds: callee kind (bytecode function, import, native stub, runtime function, foreign), callee index, argument registers, result registers, normal and error targets for `try.call`, unwind target for `invoke`.

### 15.6 Execution

- The interpreter is a threaded-dispatch loop. There is no JIT.
- The VM predecodes registers and branch sites. Unmetered execution may use an
  interpreted numeric-loop superinstruction for a straight-line integer loop;
  it does not generate host code. Budgeted evaluation dispatches bytecode
  instructions and charges each branch-edge transfer individually.
- Reference counting, allocation, access checks and traps call the same KLF runtime as native code, so counts, quarantine and heap accounting behave identically on both engines.
- Foreign calls (`.klffi`) are performed through per-signature native thunks. For `build --emit bundle` the compiler generates the thunks into the runner's native half. For `run` on a development host the VM uses the kernel's dynamic call trampoline (hand-written assembly per host architecture). On `wasm32` foreign calls resolve to imports of the WebAssembly module.
- Unwinding on the VM walks frames using each function's handler table.
- Every VM run verifies heap balance at exit: allocations minus releases minus blocks deliberately retained by foreign code MUST be zero, otherwise the run fails with `KLF-RT-0100` listing the unaccounted blocks. This applies to languages whose memory model is `Affine` or `Counted`; for `Unmanaged` modules the check reports leaks as remarks only.

### 15.7 Host capabilities

An embedder creates an instance with a capability table: `{ name: String, signature, function pointer }`. Bytecode imports of kind runtime resolve against it. The standard capability sets are `klf.io`, `klf.time`, `klf.thread`, `klf.net`, `klf.process`; a runner grants sets explicitly.

---

## 16. Native backend

### 16.1 Lowering

KIR lowers to LLVM IR one function at a time. Loadable nominal types become LLVM struct types with the exact layout the kernel computed (explicit padding fields; LLVM is never asked to lay out a type). Address-only and generic values are `ptr` plus runtime sizes. Policy functions are direct calls; for `pairing: counted` types on `ref T` the backend emits inline fast paths for retain and release (an atomic increment at the slot metadata address computed from the pointer, section 18.3) and calls the runtime only on the slow path.

### 16.2 Calling conventions

Functions use `ridlcc` (section 22) when their source file's effective convention is `Ridl` or the function is exported; otherwise they use the platform C convention. KIR retains the resolved ABI per function so merged units preserve file-mode overrides. `call.foreign` always uses the platform C convention. Function-pointer conversions and thunks are generated on demand (section 26.4).

### 16.3 Debug information

The backend emits DWARF 5 on ELF and Mach-O and CodeView on Windows. Requirements: stock LLDB MUST show source lines, step correctly, print locals and print aggregates of every hosted language using only the emitted debug info (language id `DW_LANG_C_plus_plus` for KArc, `DW_LANG_C11` for C, a vendor code `0x8e57` for Kira with type descriptions restricted to C-expressible DWARF so that stock LLDB formats them). Enum payloads are described as variant parts. Virtual files carry their text in the DWARF source extension.

### 16.4 Unwinding and traps

Native unwinding uses the platform's zero-cost table mechanism through LLVM landing pads with the personality function `klfrt_personality`. Traps call `klfrt_trap(code)` which prints the code, a symbolized backtrace when debug info is present, and aborts.

### 16.5 Linking

The driver links with the bundled lld. Static linking of the KLF runtime is the default for executables; shared linking is used when any dependency is a RIDL dynamic library, so that one runtime instance serves the process.

---

## 17. Hybrid execution and hot module push

### 17.1 Engine assignment

Every function has engine `runtime`, `native` or `any`. The definition sets it from source annotations (Kira's `@Runtime`, `@Native`; KArc's `[[klf::runtime]]`, `[[klf::native]]`). Under `--backend hybrid`, functions marked `any` go to the engine chosen by `--hybrid-default` (default `native`). A function containing `asm` or an intrinsic without a VM implementation is forced native.

### 17.2 Shared world

Both halves run in one address space with identical layouts, one KLF runtime, one heap. A value crossing engines is not copied or marshaled.

### 17.3 Cross-engine calls

- Bytecode to native: the compiler emits, into the native half, one entry stub per distinct native-callee signature used from bytecode: `klf_n_<hash>(args: ptr, results: ptr, callee: ptr)`. The VM's CALL with kind native stub fills the argument block from registers and calls the stub.
- Native to bytecode: for every `runtime` function called from native code the compiler emits a native function with the original symbol and convention that packs its arguments and calls `klfvm_invoke(instance, functionIndex, args, results)`.
- `try.call` and unwinding propagate across both stub kinds.

### 17.4 Bundles

A `.klbundle` is a directory or archive with `manifest.json` (modules, engines, targets, capability sets, entry point), `.klbc` files, native halves per target, and assets. Building a bundle needs LLVM; running one never does.

### 17.5 Hot module push

Only bytecode is pushed. Protocol: TCP, each frame is `u32 length, u8 type, payload`.

| Type | Name | Payload |
| --- | --- | --- |
| 1 | HELLO | Protocol version u16, KLF version, bundle id, layout hash of every loaded module |
| 2 | PUSH_MODULE | Module name, KIR text |
| 3 | ACK | Module name, count of replaced functions |
| 4 | NACK | Module name, reason code, message |
| 5 | LOG | Text from the device |

The client opens the exchange: it sends `HELLO` first, naming its protocol
version and the bundle it speaks for, and the device answers with its own
`HELLO` carrying the layout hash of every module it holds. A client that
receives no hello, or one whose protocol major version differs, stops there.

`PUSH_MODULE` carries KIR rather than `.klbc`. KIR is the portable form and the
device lowers it with the toolchain it already runs, so one build serves devices
on any KLF release; bytecode is specific to a pointer width and layout target
(15.1) and to the encoding of the release that produced it, so pushing it ties
a device to the release its compiler had. The unit of a push is one module: a
whole package's KIR, with its libraries statically linked as 15.2 describes for
bytecode.

Reason codes: 1 `LAYOUT_CHANGED` (a global's or exported type's layout hash differs; restart required), 2 `SIGNATURE_CHANGED` (a function called from the native half changed signature), 3 `VERIFY_FAILED`, 4 `VERSION_MISMATCH`, 5 `OLD_CODE_BUSY`, 6 `UNKNOWN_MODULE` (the device holds a module, and not the one the push names).

A debug device that holds no module installs the first module pushed to it:
the device lowers it, runs its initializers, and answers `ACK` with the count
of functions it installed. That module is then the one the device holds, and
later pushes replace it. This is how a live runtime shell, which ships with no
app, receives the app it runs.

A push is refused, not applied, when the module is new to a release device:
section 17.5 allows hot push only in a debug bundle, so a release device
answers every `PUSH_MODULE` with `VERIFY_FAILED` and a message saying so.

Semantics (Erlang-style two versions): after a successful push, new calls enter the new code; frames already executing old code finish in old code; at most two versions of a module exist, and a second push while old frames remain waits up to 5 seconds then NACKs with reason 5 `OLD_CODE_BUSY`. Globals keep their addresses and values. Hot push is available only in debug bundles.

---

## 18. KLF runtime

`libklfrt` is linked into every program. It is written in Rust with `no_std` plus `alloc`-free core so that it builds for `wasm32`.

### 18.1 Allocator

- The heap is made of spans of 64 KiB aligned to 64 KiB. A span serves one size class. The span header (first 64 bytes) holds: magic, size class, slot size, slot count, free list head, owning thread cache. Slot metadata lives in a table directly after the header: 8 bytes per slot.
- Size classes: multiples of 16 bytes up to 256; then eight classes per power of two up to 16,384 bytes. Larger allocations get a dedicated run of spans with one slot.
- A global page map (two-level radix tree keyed by address shifted right by 16) answers whether an address belongs to the KLF heap. On `wasm32` the page map is a flat bitmap.
- Per-thread caches hold free slots per class; cross-thread frees go to a lock-free list on the span.
- `klfrt_alloc(size, align)`, `klfrt_realloc`, `klfrt_free(ptr)`. Alignments above 16 use the class whose slot size is a multiple of the alignment.

### 18.2 Slot metadata

| Bits | Field |
| --- | --- |
| 0 to 31 | Strong count (atomic). Starts at 1 |
| 32 to 55 | Weak count (atomic). Starts at 1, held collectively by the strong references |
| 56 | `deleted`: the destructor ran through a raw-mode delete |
| 57 | `quarantined` |
| 58 | `immortal`: counts are frozen (static objects, saturated counts) |
| 59 to 63 | Reserved, zero |

### 18.3 Counting by address

`klfrt_retain(p)` and `klfrt_release(p)`:

1. If `p` is null or not in the page map, return.
2. `span = p & ~0xFFFF`; `slot = (p - span.dataStart) / span.slotSize`. Interior pointers therefore resolve to their allocation.
3. Atomically adjust the strong count unless `immortal`.
4. On release reaching zero: if not `deleted`, run the object's destroy function (registered at allocation through `klfrt_alloc_counted(size, align, destroy)` and stored in the first word of the span's per-class destroy table or, for mixed classes, in a side table keyed by slot); then drop the collective weak reference; when the weak count reaches zero, free the slot.

Counts that would overflow saturate and set `immortal`. Because metadata lives in the allocator, object layout is identical whether or not code that touches the object counts it. This is what allows ARC and raw files to share types (section 28).

### 18.4 Raw deletion, poison and quarantine

`klfrt_delete_raw(p, destroy)` implements raw-mode `delete` on a slot: run `destroy`, set `deleted`, decrement the strong count once (the manual owner's reference). If the count is now zero, free the slot. Otherwise fill the payload with the byte `0xEF`, set `quarantined`, and keep the slot until the count reaches zero. In debug builds every counted-mode dereference of a pointer checks `deleted` first and traps with `KLF-RT-0004`.

### 18.5 Weak references

A weak reference is a pointer to the slot payload plus one weak count. `klfrt_weak_load(p)` atomically increments the strong count if it is non-zero and the slot is not `deleted`, returning the pointer, else null.

### 18.6 Cycle detector (debug builds)

Counted types compiled in debug mode carry a trace map: the offsets of strong references inside the payload, emitted by the compiler next to the destroy function. Every release that leaves a non-zero count records the slot as a candidate root. `klfrt_cycles_collect()` runs synchronous trial deletion (Bacon and Rajan) over the candidates; it never frees, it reports. At exit, and on demand, the runtime prints each leaked cycle as a list of `Type.field -> Type.field` edges with allocation sites and fails the process with `KLF-RT-0101` when `KLF_CYCLES=error` (the default under `<driver> test`).

### 18.7 Dynamic exclusivity

`klfrt_access_begin(addr, size, modify, buffer)` and `klfrt_access_end(buffer)` maintain a per-thread list of active accesses; a conflict traps with `KLF-RT-0003`.

### 18.8 Traps

| Code | Meaning |
| --- | --- |
| `KLF-RT-0001` | Unreachable executed |
| `KLF-RT-0002` | Integer overflow in trap mode |
| `KLF-RT-0003` | Exclusivity violation |
| `KLF-RT-0004` | Use of deleted object |
| `KLF-RT-0005` | Out-of-bounds access checked by a hosted language |
| `KLF-RT-0006` | Raw free of a slot that is already free |
| `KLF-RT-0010` | Division by zero |
| `KLF-RT-0011` | Invalid float-to-int conversion in trap mode |
| `KLF-RT-0020` | Exception reached a RIDL boundary |
| `KLF-RT-0021` | Stack overflow |
| `KLF-RT-0100` | Heap imbalance at exit |
| `KLF-RT-0101` | Reference cycle leak |
| `KLF-RT-0200` | RIDL link check failed |

### 18.9 Threads

The runtime provides thread creation, thread-local storage and parking through the `klf.thread` capability on the VM and the platform on native. Strong and weak counts are always atomic in 1.0.

The `klf.thread` services are `spawn(entry, argument)`, which starts a thread
running the C function `void entry(int64_t)` on `argument` and answers a
handle; `join(handle)` and `detach(handle)`; `current()`, the calling
thread's number, above 0 and never reused; `park(timeout)`, which blocks
until the thread is unparked, `timeout` nanoseconds pass (forever when
negative) or spuriously, and returns at once when an `unpark` came first;
`unpark(number)`; `yield()`; and `parallelism()`. Thread-local storage is a
`thread_local` KIR global. A thread allocates from a heap of its own on the
shared page map (section 18.1); when its entry returns the heap is orphaned,
not freed, so blocks other threads hold stay valid. A VM instance runs on one
thread: it refuses `spawn`, is thread 1, and returns from `park` once any
timeout has passed.

---

## 19. RIDL: principles and scope

1. RIDL is a native ABI: data layout, calling convention, symbol naming, runtime records for generics, versioning and linking rules. It is also the text format `.ridl` that declares an interface under that ABI.
2. RIDL conforms to no other ABI. The platform C ABI is external and reachable only through `.klffi`.
3. RIDL governs public module boundaries only. Inside a module a compiler may inline, reorder fields, monomorphize and use any convention.
4. RIDL is same-address-space and zero-copy. Nothing is serialized or marshaled.
5. Every construct is statically typed. There is no runtime type registry, no conformance lookup, no dynamic cast and no reflection through RIDL.
6. RIDL is implementable in three kinds of compiler: KLF toolchains (automatic), rustc and swiftc (compiler integration, section 25). The machine-level convention is one LLVM calling convention shared by all three.
7. On platforms where a language already has a stable ABI (Swift on Apple platforms), RIDL sits beside it. Elsewhere, and always on a freestanding Linux system, RIDL is the system ABI.

---

## 20. RIDL type system and `.ridl` files

### 20.1 File form

A `.ridl` file is a sequence of declarations in Kira declaration syntax without bodies. The module name is the file's base name; a module's interface MAY span several files in one directory. `import Other` makes another module's types visible. The grammar is Appendix A.2.

### 20.2 Scalars

`I8 I16 I32 I64 I128 U8 U16 U32 U64 U128 F32 F64 Bool ISize USize RawPtr Void`. `Int` is an alias of `I64`, `Float` of `F64`. The sized numbers `N8 N16 N32 UN8 UN16 UN32` (section 27.5) are types of their own, not aliases, and are spelled by those names. `Bool` is one byte holding 0 or 1. `RawPtr` is an unmanaged address. `Void` appears only as a result.

### 20.3 Declarations

| Declaration | Form | Meaning |
| --- | --- | --- |
| Struct | `struct Name<T> { let field: Type [= value] ... }` | By-value aggregate with frozen layout |
| Opaque struct | `struct Name` (no body) | Handle. Other modules hold a pointer and use the exported descriptor |
| Enum | `enum Name<T> { Case  Case(Type)  Case(Type, Type) }` | Tagged union with frozen layout |
| Open enum | `@Open enum Name { ... }` | Enum that may gain cases additively (section 24.5) |
| Trait | `trait Name<T>: Parent { type Assoc: Bound  function m(borrow self, ...) -> R }` | Interface. Usable as a bound and as a trait object type |
| Function | `function name<T: Bound>(label: convention Type [= value], ...) -> Result` | Free function |
| Method | A `function` inside `extend TypeName { ... }` | Function in the type's namespace. `self` has a convention |
| Conformance | `extend TypeName: Trait` | Declares that the module exports the witness table |
| Alias | `type Name = Type` | No ABI effect |
| Constant | `let NAME: Type = value` | Compile-time constant, inlined by consumers. `value` is a literal or any constant a field default may be (below) |

A constant's value is a literal when its type is a scalar or `String`. Otherwise it is a constant as a field default's is, built of literals, enum cases, arrays, structs and distincts. A module writes a constant whose value is an expression of other constants, such as `EdgeSet.leading | EdgeSet.trailing` or `6 * 7`, as the value it folds to. Integer arithmetic folds when no step traps or leaves its type's range. A constant that names another is written with that constant's value in its place, so a contract never names a constant it leaves out. A public `let` whose value is none of these, because it is worked out at run time, is not written.

A struct field MAY carry a default, `let field: Type = value`. The value is a constant built of literals, enum cases `.Case` and `.Case(value)`, arrays of values, structs `Name { field: value, ... }` and distincts `Name(value)` of the module's types or Foundation's, a struct the defining module builds from its own defaults `Name()` or `Name {}`, and the names of the module's constants. A call to one of the module's own free functions whose body is a single `return` of such a constant, built from its parameters, is that constant with the call's arguments (or the parameters' own constant defaults) written in its parameters' places: `resolvedLength(0.0)` whose body is `return ScaledMetric { points: points }` is written `ScaledMetric { points: 0.0 }`. A reader that builds the struct and leaves the field out writes the value in, as it writes in a constant; the default has no ABI effect. A module writes the default of a field it declares one for when the default is such a constant, and leaves it out otherwise: the field then has none for the reader.

A function's or method's parameter MAY carry a default the same way, `label: Type = value`, with the same values a field default may have. A caller that leaves the slot empty passes the value, written in at the call; the default has no ABI effect. A module writes the default of a parameter it declares one for when the default is such a constant, and leaves it out otherwise: the slot then has to be filled.

### 20.4 Type expressions

Scalars; nominal types with arguments; `String`; `[T]` (owned growable array); `Optional<T>` and `Result<T, E>` (the two built-in enums); tuples `(A, B)` (laid out as anonymous structs); function types `(A, B) -> R` (owned closures) and `borrow (A, B) -> R` (non-escaping closures); trait object types `Any Trait`; generic parameters.

### 20.5 Conventions

A parameter is written `label: Type` (owned), `label: borrow Type` or `label: borrow mut Type`. `self` likewise: `self`, `borrow self`, `borrow mut self`. Trivial types ignore the convention. Results are always owned.

### 20.6 Failure

A function whose declared result is `Result<T, E>` is a failing function: `T` is returned normally and `E` travels through the error register (section 22.5). A `Result` anywhere else is an ordinary enum.

### 20.7 Annotations

`@Abi(version: "uuid")` (section 24.4), `@Open`, `@Instance(Type, ...)` on a generic declaration (declares an exported named instance), `@Deprecated("text")`, `@Engine(runtime)` (the function exists only as bytecode; native callers reach it through the hybrid stub). No other annotation has meaning in `.ridl`.

### 20.8 Example

```
import Canvas2D

struct Vec3 {
    let x: Float
    let y: Float
    let z: Float
}

enum Shape {
    Circle(Float)
    Box(Vec3, Vec3)
    Empty
}

struct Scene

trait Drawable {
    function draw(borrow self, on: borrow mut Canvas)
    function bounds(borrow self) -> Vec3
}

extend Scene {
    function add(borrow mut self, shape: Shape)
    function largest<T: Drawable>(borrow self, of: borrow [T]) -> Optional<T>
}

extend Shape: Drawable

function parse(text: borrow String) -> Result<Shape, ParseError>

@Instance(Shape)
function sort<T: Comparable>(items: borrow mut [T])
```

### 20.9 What cannot be exported

Borrowed results; async functions; variadic functions; non-type generic parameters; types whose layout depends on a generic parameter used as a constant; global mutable variables; unwinding. A toolchain reports `ridl.not-exportable` with the reason when a definition tries.

---

## 21. RIDL data layout

All rules assume little-endian targets (every 1.0 target is). `W` is the pointer width in bytes (8, or 4 on `wasm32`).

### 21.1 Scalars

| Type | Size | Alignment |
| --- | --- | --- |
| `I8 U8 Bool` | 1 | 1 |
| `I16 U16` | 2 | 2 |
| `I32 U32 F32` | 4 | 4 |
| `I64 U64 F64` | 8 | 8 |
| `I128 U128` | 16 | 16 |
| `ISize USize RawPtr` | W | W |
| `N8 UN8` | 1 | 1 |
| `N16 UN16` | 2 | 2 |
| `N32 UN32` | 4 | 4 |

### 21.2 Structs and tuples

Fields are placed in declaration order, each at the next offset that is a multiple of its alignment. The struct's alignment is the maximum field alignment (1 if empty). Its size is the end of the last field rounded up to the alignment. An empty struct has size 0. There is no reordering and no packing.

### 21.3 Enums

1. The tag is `U8` if the enum has at most 256 cases, `U16` up to 65,536, else `U32`. Cases are numbered from 0 in declaration order. An `@Open` enum always has a `U32` tag.
2. Each case's payload is laid out as a tuple of its payload types.
3. Payload offset `P` is the tag size rounded up to the maximum payload alignment `A`. Enum alignment is the maximum of tag alignment and `A`. Size is `P` plus the maximum payload size, rounded up to the alignment. An enum without payloads is just its tag.
4. Pointer niche. An enum with exactly two cases, one without payload and one whose single payload is an opaque struct, a trait object or a closure, has no tag: the payload's first pointer word being zero encodes the payload-less case. `Optional<Scene>` is one pointer.
5. Bytes of the payload area not used by the active case, and padding, are unspecified and MUST NOT be read.

### 21.4 Strings and arrays

Owned `String` and `[T]` share one shape, four words:

| Offset | Field | Meaning |
| --- | --- | --- |
| 0 | `data: RawPtr` | First element or byte. May be null when `cap` is 0 |
| W | `len: USize` | Bytes for `String`, elements for `[T]` |
| 2W | `cap: USize` | Capacity in the same unit |
| 3W | `ops: RawPtr` | Pointer to a static `BufferOps`, or null when `cap` is 0 |

```
BufferOps { abi: U32 = 1, flags: U32 = 0,
            realloc: fn(data: RawPtr, oldCap: USize, newCap: USize, elemSize: USize, align: USize) -> RawPtr,
            free:    fn(data: RawPtr, cap: USize, elemSize: USize, align: USize) }
```

`String` holds well-formed UTF-8 without a terminator; passing ill-formed UTF-8 is a contract violation. The owner of a buffer destroys live elements through `T`'s descriptor, then calls `ops.free`. Any holder may grow the buffer through `ops.realloc`, which returns null on failure leaving the buffer intact.

Borrowed forms `borrow String`, `borrow [T]` and `borrow mut [T]` are two words: `data`, `len`. A `borrow mut [T]` cannot change length. `borrow mut String` is a pointer to the owned four-word value.

### 21.5 Opaque structs

A value of opaque struct type is one non-null pointer. What it points to is private to the defining module. The defining module exports the type's descriptor (symbol kind `D`), which describes the handle: size `W`, `copy` retains and writes the pointer (null `copy` if the type is unique), `destroy` releases or frees.

### 21.6 RIDL box

```
offset 0:  td:   RawPtr   descriptor of the boxed value's type
offset W:  free: fn(box: RawPtr)
offset round_up(2W, align): the value
```

To destroy an owned box: call `td.destroy(valueAddress, td)` if non-null, then `free(box)`. The allocator is whoever created the box; `free` carries that knowledge.

### 21.7 Trait objects

`Any Trait` is two words: `box: RawPtr` (a RIDL box holding the value) and `wt: RawPtr` (the witness table). Owned trait objects own the box. A borrowed trait object is the same two words without ownership. Copying an owned trait object requires the boxed type's `copy`; the copier allocates a new box with its own `free`.

### 21.8 Closures

A closure is two words: `code: RawPtr` and `ctx: RawPtr`. `ctx` is null or a RIDL box owning the captured state. `code` follows the RIDL convention for the closure's declared signature and receives `ctx` in the context register. A `borrow` closure has the same shape and does not own `ctx`. Destroying an owned closure destroys its box.

### 21.9 Built-in enums

`Optional<T>` is `enum { None  Some(T) }` and `Result<T, E>` is `enum { Ok(T)  Error(E) }` under 21.3, including the pointer niche for `Optional`.

---

## 22. RIDL calling convention

### 22.1 Base

The convention is named `ridlcc`. On each target it is the platform's base procedure call standard (register preservation, stack alignment, red zone, shadow space, unwinding rules) with the additions below. Unwinders, profilers and debuggers therefore work unchanged.

### 22.2 Registers

| Target family | Integer arguments | Float arguments | Direct results | Indirect result | Context | Error |
| --- | --- | --- | --- | --- | --- | --- |
| aarch64 (all OS) | x0 to x7 | v0 to v7 | x0 to x3, v0 to v3 | x8 | x20 | x21 |
| x86_64 System V | rdi rsi rdx rcx r8 r9 | xmm0 to xmm7 | rax rdx rcx r8, xmm0 to xmm3 | rax | r13 | r12 |
| x86_64 Windows | rcx rdx r8 r9 (positional with xmm0 to xmm3) | xmm0 to xmm3 | rax rdx rcx r8, xmm0 to xmm3 | rax | r13 | r12 |
| wasm32 | WebAssembly parameters | WebAssembly parameters | one WebAssembly result | leading `i32` parameter | trailing `i32` parameter | trailing `i32` out-pointer parameter |

The context register is callee-saved. The error register is set to zero by the caller before every call to a failing function and written by the callee; it is not preserved.

Rationale: these assignments coincide with LLVM's `swiftcc` (`swiftself`, `swifterror`), so the existing LLVM lowering is reused.

### 22.3 Classifying a parameter

1. Generic-typed, address-only, or larger than four words: passed by reference (one integer register holding the address of caller-owned storage). For `owned`, the callee takes ownership of the contents, not of the storage.
2. Otherwise flatten the value into scalar leaves in layout order. Integer, pointer and `Bool` leaves are integer class; `F32` and `F64` are float class; `I128` and `U128` are two integer leaves, low first. Enums that are not payload-less are passed as raw words: `ceil(size / W)` integer leaves.
3. If the value has at most four leaves and enough registers of each class remain, assign each leaf to the next register of its class. Otherwise pass it by reference as in rule 1.
4. When registers of a class run out for a by-value scalar, it goes on the stack per the base standard.
5. `borrow` of a loadable trivial-copy value of at most four leaves is passed like the value. Every other `borrow` and every `borrow mut` is passed as an address.
6. `String`, `[T]`, closures and trait objects are ordinary aggregates under rules 2 and 3 (four, four, two and two integer leaves).

### 22.4 Results

Results flatten like parameters. Up to four leaves return in the direct result registers of their class. Anything else returns through the indirect result pointer: the caller passes the address of uninitialized storage and the callee initializes it.

### 22.5 Failure

On success the callee leaves the error register zero and returns `T` normally. On failure it stores in the error register a pointer to a RIDL box holding the `E` value, and the normal result is unspecified. The caller tests the register after the call, moves `E` out of the box and calls the box's `free`.

### 22.6 Generic parameters and witness methods

After the declared parameters, in order, for each type parameter: its descriptor pointer, then one witness table pointer per bound in bound order. They are integer-class parameters.

A trait method called through a witness table receives: the declared parameters with `self` always passed as an address, then the `Self` descriptor pointer, then the `Self` witness table pointer.

### 22.7 LLVM implementation

`ridlcc` is a calling convention added to the KLF LLVM fork as a thin alias of `swiftcc` argument and return assignment with: the context register mapped to the `swiftself` attribute, the error register to `swifterror`, and results of up to four registers. Frontends perform all classification of 22.3 themselves and hand LLVM only scalars and pointers.

---

## 23. RIDL generics: descriptors, witness tables, builders

### 23.1 Type descriptor

```
offset 0       abi: U32 = 1
offset 4       flags: U32
offset 8       size: USize          (offset 8 on all targets)
offset 8+W     align: USize
offset 8+2W    copy:    fn(dst: RawPtr, src: RawPtr, td: RawPtr)
offset 8+3W    move:    fn(dst: RawPtr, src: RawPtr, td: RawPtr)
offset 8+4W    destroy: fn(obj: RawPtr, td: RawPtr)
offset 8+5W    extension area (private to the producer)
```

| Flag bit | Meaning |
| --- | --- |
| 0 | Copyable. If set and `copy` is null, copy is a byte copy of `size` bytes |
| 1 | Bitwise relocatable. If set, `move` is null and moving is a byte copy |
| 2 | Trivially destroyable (`destroy` is null) |
| 3 | The type is an opaque handle |
| 4 | Runtime-built instance (extension area holds argument descriptors) |

A type that is not copyable has bit 0 clear and `copy` null; consumers MUST NOT copy it. The stride of a type equals its size. Descriptors of concrete types are immutable static data. Descriptor identity carries no meaning: two descriptors for the same type MAY have different addresses and consumers MUST NOT compare them.

### 23.2 Witness table

```
offset 0      abi: U32 = 1
offset 4      count: U32                 number of entries
offset 8      td: RawPtr                 descriptor of the conforming type
offset 8+W    entries[count]: RawPtr
```

Entry order: for each parent trait in declaration order, a pointer to the parent's witness table; then for each associated type in declaration order, its descriptor followed by one witness table per bound; then each method in declaration order. Methods added in later ABI versions are appended; a caller that needs entry `i` on a table with `count` not greater than `i` uses the default implementation exported by the trait's module (symbol kind `M` on the trait).

### 23.3 Generic values

Inside a polymorphic entry every value of generic type is handled by address through its descriptor: `copy`, `move`, `destroy`, and byte copies when the flags allow. Stack storage for a generic value is allocated dynamically with `size` and `align`.

### 23.4 Builders for generic types

For each exported generic type `G<T1..Tn>` the module exports:

- `Z` symbol: a `USize` constant, the byte size of a runtime-built descriptor of `G`.
- `B` symbol: `fn(out: RawPtr, args: RawPtr, witnesses: RawPtr)`. `args` points to `n` descriptor pointers, `witnesses` to the witness table pointers of all bounds in order. The builder computes size and alignment with the rules of section 21, stores the argument pointers in the extension area, sets flag 4 and installs copy, move and destroy functions that read the extension area.

A runtime-built descriptor is valid while its storage and its argument descriptors are. Callers allocate it on the stack for the duration of the call that needs it.

### 23.5 Named instances

`@Instance(Shape)` on a generic function exports, in addition to the polymorphic entry, a specialized symbol (kind `I`) with the non-generic convention for that instantiation. Callers MUST prefer it.

---

## 24. RIDL symbols, metadata, versioning and linking

### 24.1 Identifiers

RIDL identifiers match `[A-Za-z_][A-Za-z0-9_]*`. Languages with richer identifiers export under a RIDL-safe name chosen by their definition.

### 24.2 Symbol names

```
symbol   = "_RIDL1" path kind [ "_v" abitag ]
path     = component { component }
component= decimal-length identifier
kind     = "F" | "M" | "D" | "B" | "Z" | "W" path | "I" typelist "E"
abitag   = 16 lowercase hex digits (first 64 bits of the ABI UUID in which this shape was introduced)
typelist = { typecode }
```

Kinds: `F` free function, `M` method (path ends with type then method), `D` descriptor, `B` builder, `Z` descriptor size, `W` witness table (first path is the type, second the trait), `I` named instance.

Type codes: `a b c d e` for `I8 I16 I32 I64 I128`; `h i j k l` for `U8` to `U128`; `f g` for `F32 F64`; `o` Bool; `v w` for `ISize USize`; `p` RawPtr; `s` String; `A` followed by element code for arrays; `O` followed by a code for Optional; `R` followed by two codes for Result; `T` codes `E` for tuples; `N` path `E` for nominal types, with `I` codes `E` before the final `E` when generic.

Example: module `Geometry`, function `parse`, introduced in ABI `6f1c2a9e44d07b31...`: `_RIDL18Geometry5parseF_v6f1c2a9e44d07b31`.

### 24.3 Module metadata

Every RIDL module embeds a metadata blob and registers it at load through `klfrt_ridl_register(blob)`. Section names: `.ridlmeta` (ELF), `__DATA,__ridlmeta` (Mach-O), `.ridlmt` (PE), custom section `ridlmeta` (WebAssembly), and the META section of `.klbc`.

```
magic "RIDL", u16 major = 1, u16 minor = 0
string moduleName
u32 abiCount, then abiCount x { u8[16] uuid, u8 status }    status: 0 current, 1 supported, 2 retired
u32 importCount, then importCount x { string moduleName, u8[16] requiredUuid }
u8[32] sha256 of the canonical .ridl text
u32 unmanagedCount, then function symbol names containing unmanaged instructions
```

Strings are `u32` length plus UTF-8.

### 24.4 Versioning

1. A module's interface evolves additively only. A shape that was ever frozen is never changed or removed until retired.
2. `klf abi freeze` computes the module's current surface, compares it with the latest record, mints a version 4 UUID and writes `abi/<uuid>.ridlsig` containing only the delta (added declarations, superseded shapes). Unchanged declarations appear in no later record.
3. `klf abi apply` edits sources: each superseded shape is kept and stamped `@Abi(version: "<uuid of the ABI that introduced it>")`; the new shape is unstamped. The compiler exports a stamped shape under the ABI tag of its stamp and the unstamped one under the tag of the ABI that introduces it, so old and new callers bind to different symbols and both keep working.
4. The compiler reports `ridl.abi-broken` when a frozen shape differs from its record.
5. Allowed additions: new functions, types, conformances and named instances; new trait methods that have default implementations (appended to witness tables); new cases on `@Open` enums whose payload fits the payload area frozen in the first record; new parameters only by adding a new shape and keeping the old one as a stamped forwarder.
6. Consumers that match on an `@Open` enum from another module MUST have a default arm; hosted languages enforce this in their definitions (`ridl.open-enum-needs-default`).
7. `klf abi retire` moves a record to `abi/unsupported/`, marks its UUID retired in metadata, deletes the stamped shapes that only it needed, and rebases surviving records onto the new floor. The recommended support window is four years.

### 24.5 Linking and loading

RIDL modules link with the platform's ordinary static and dynamic linkers. At registration the runtime checks every import: if the provider lists the required UUID as current or supported, loading proceeds; if retired, the runtime prints warning `KLF-RT-0201` and proceeds unless `KLF_RIDL_STRICT=1`; if absent, the process fails with `KLF-RT-0200` naming both modules and UUIDs. One UUID per module per process: two loaded copies of a module with different current UUIDs fail with `KLF-RT-0202`.

### 24.6 Emission and canonical text

A toolchain emits `.ridl` from the exported surface of a module. The canonical text used for hashing is the emitted file with comments removed, declarations sorted by path, one space between tokens.

### 24.7 Objects across a process boundary

Sections 24.1 to 24.6 describe a module inside one process. This section describes the same surface when the two sides are separate processes, which is what lets a `.klbc` be a server a program talks to over a pipe rather than a library it loads. The rules follow XPC, where an object that crosses stays where it lives and the far side holds a proxy.

#### 24.7.1 Connections

A connection is a duplex channel between two processes with an identity both sides can name. A module holds one per peer; a connection is established before any object crosses it and lasts until either side closes it or the peer dies.

Every message names its connection. A connection carries one request at a time and every reply arrives on a second channel opened for that request, so a handler is never waiting on the channel it is servicing.

#### 24.7.2 Objects and proxies

A box, an opaque struct, an `Any Trait` value or a closure that crosses a connection is an *exported object*: it stays in the process that created it, is registered there under an id, and arrives on the other side as a *proxy*. A proxy carries the exporting connection and the id, and every method a consumer calls on it becomes a message on that connection. The layout in 24.7.4 is what travels; the pointee never does.

The same holds in the other direction: an object a consumer passes to a module crosses back as an export on the module's side, and the module holds a proxy. An object may therefore be referenced from either process and the handles are not interchangeable.

An opaque struct that crosses becomes a proxy over its exported object, not a pointer. `Drop` on a proxy sends a release; the object is destroyed when the last handle on the exporting side is gone.

#### 24.7.3 Reentrancy

A call is a send with a reply channel. A handler that needs a value before it can answer sends its own call and waits for *that* reply, on a different channel from the one it is servicing. This is what keeps module to host to module from deadlocking: the inner call never blocks on the channel the outer call is holding.

A cycle is therefore a program error, not a runtime possibility: a handler must not block on a reply to a message it sent on the connection it is servicing. Each side tracks the channel stack it is waiting down, and a message arriving for a channel already on that stack fails the call with `KLF-RT-0203` naming the connection and the depth. Trapping is deliberate — a hang would be indistinguishable from a slow peer.

#### 24.7.4 Values on the wire

A value crosses in the same shape it has in memory, because 21.4 was written for a crossing whose two sides do not share an allocator: a buffer travels as its length and a reference to the owner's `BufferOps` rather than a raw address, and a descriptor travels as its `copy`, `move` and `destroy` behaviour rather than the storage they apply to. A consumer reconstitutes the value through the ops the producer named, so an allocation made on one side is freed by whoever allocated it.

Widths do not travel. A consumer re-materializes a value through the C type its declaration gives it, so an `I8` and an `U8` are the same wire form and a value that does not fit the declared width is a trap there, not a truncation here.

Bulk data does not travel inside a message. A buffer past `24.7.5`'s limit is sent as a separate transfer the receiver pulls, so one large value cannot fill a channel and stall a peer that is not reading it.

#### 24.7.5 Limits

A single message carries at most 64 KiB of payload. A buffer larger than that is a transfer, and a sequence of buffers whose total exceeds the limit is sent as transfers. Depth of nested values is bounded at 64 and reported as `KLF-RT-0204`, since a cycle in the value graph would otherwise recurse without end.

---

## 25. Implementing RIDL in Rust and Swift

This section states what a rustc or swiftc integration consists of. Until such integrations exist, `klf ridl bindgen` generates source-level bindings that reach RIDL functions through small assembly or LLVM-IR shims shipped with the bindings.

### 25.1 Mapping

| RIDL | Rust | Swift |
| --- | --- | --- |
| owned parameter | `T` | `consuming T` |
| `borrow T` | `&T` | `borrowing T` |
| `borrow mut T` | `&mut T` | `inout T` |
| `RawPtr` | `*mut c_void` | `UnsafeMutableRawPointer?` |
| struct | `#[repr(ridl)] struct` | `@frozen struct` with RIDL layout |
| enum | `#[repr(ridl)] enum` | `@frozen enum` with RIDL layout |
| opaque struct | A type exported with `#[ridl(opaque)]`; consumers see an owning newtype over a non-null pointer whose `Drop` calls the descriptor | `final class` or non-copyable struct wrapper |
| trait | `trait` | `protocol` |
| `Any Trait` | `Box<dyn Trait>` re-boxed as a RIDL box | `any Protocol` re-boxed |
| closure | `Box<dyn FnMut>` adapter | escaping closure adapter |
| `String`, `[T]` | `String`, `Vec<T>` with a `BufferOps` that reconstitutes and drops the `Vec` | Native `String`, `Array` copied into a malloc-backed buffer when not already contiguous and uniquely owned |
| `Result<T, E>` result | `Result<T, E>` | typed `throws(E)` |
| generic function | One polymorphic body per exported generic | Native unspecialized generic body |

### 25.2 Compiler work items

1. A new ABI string: `extern "ridl"` in Rust, `@abi(ridl)` in Swift, lowering to LLVM `ridlcc` with the classification of section 22.3 done in the frontend.
2. `repr(ridl)` layout for structs and enums (section 21).
3. Emission of descriptors, witness tables, builders and metadata; symbol mangling of 24.2.
4. Polymorphic bodies for exported generics. In rustc this is the largest item: one dictionary-passing body per exported generic function, generated from MIR with type parameters kept abstract; generics that fail the rules of 13.2 are exportable only as named instances. swiftc already compiles unspecialized generic bodies and needs only an adapter from its runtime metadata to RIDL descriptors.
5. Import: reading `.ridl` and presenting it as a native module (`extern crate` for Rust, a Clang-importer-like module for Swift).

---

## 26. `.klffi`: the C ABI boundary

### 26.1 Form

A `.klffi` file uses Kira's FFI declaration syntax, unchanged (grammar A.3). It is language-neutral: any KLF toolchain can import it.

```
@FFI.Struct { layout: c }
struct SappDesc {
    let width: I32
    let height: I32
    let title: CString
}

@FFI.Pointer { target: SappDesc, ownership: borrowed }
struct SappDescPtr

@FFI.Callback { abi: c, params: [I32, RawPtr], result: Void }
struct EventCallback

@FFI.Extern { library: sokol, symbol: sapp_run, abi: c, retains: desc }
function sappRun(desc: SappDescPtr) -> Void

@FFI.Address { library: sokol, symbol: sapp_default_desc }
function sappDefaultDesc() -> RawPtr

@FFI.Syscall { name: write }
function sysWrite(fd: I32, buffer: RawPtr, count: USize) -> ISize
```

A package names the `.klffi` of each native library it declares, beside the library's `binary` and `header`; the file is compiled as one more source of the package:

```
let nativeLibraries: [NativeLibrary] = [
    NativeLibrary { name: "sokol", binary: "native/libsokol.a",
                    header: "native/sokol_app.h", klffi: "native/sokol.klffi" }
]
```

### 26.2 Semantics

| Annotation | Meaning |
| --- | --- |
| `@FFI.Extern` | Function with the platform C calling convention. `library`, `symbol`, `abi: c` required once each. Repeatable `retains: <parameter>` marks parameters whose storage the callee keeps after returning |
| `@FFI.Address` | Address of a data symbol |
| `@FFI.Struct { layout: c }` | Struct laid out by the target's C rules, including bit-fields written `let flags: U32 @Bits(3)` |
| `@FFI.Union { layout: c }` | C union |
| `@FFI.Pointer` | Named pointer type. `ownership` is `borrowed`, `owned` (the importer frees it with the function named by `free:`) or `unmanaged` |
| `@FFI.Array { element, count }` | Fixed C array |
| `@FFI.Callback` | C function pointer type |
| `@FFI.Alias` | Name for a C scalar whose width depends on the target (`long`, `size_t`, `wchar_t`), resolved through `CLayout` |
| `@FFI.Syscall { name }` | Direct system call by its `man 2` name; refused at compile time on a target that cannot make it |
| `@FFI.Variadic` | Marks an extern function as C variadic after its fixed parameters |

Scalar types allowed in signatures: the RIDL scalars, `CString` (NUL-terminated `char*`), and named FFI types. Foreign parameters borrow by default: storage materialized for a call lives until the call returns. A `retains:` argument MUST be moved by the caller and is released with the process, the VM instance or the hybrid session, whichever owns the runtime.

### 26.3 Calls

A call to an extern function is `call.foreign`. It is unmanaged: the verifier trusts the declaration. The native backend emits a direct platform-convention call. The VM uses thunks (section 15.6).

### 26.4 Function pointers and thunks

A function pointer that crosses into C (as a `@FFI.Callback` argument or stored in a C struct) MUST have the platform convention. When a hosted function using `ridlcc` is converted to a callback type, the compiler emits a static thunk with the platform convention that calls it. Closures with captured state cannot convert to a callback type unless the C API provides a user-data pointer named by `context: <parameter>` in the `@FFI.Callback`; the thunk then recovers the closure from it. The reverse direction (calling a C function pointer) is `call.foreign` through the pointer.

### 26.5 Generation

`klf ffi gen` runs the C definition's preprocessor and parser over the headers and emits one declaration per function, struct, union, enum constant (as `let` constants), typedef and function-pointer type that is reachable from the requested symbols. Macros that expand to integer or string literals become constants. Everything else in a header is ignored with a remark.

The preprocessor follows C17 for `#define` and `#undef` (object- and function-like, `#`, `##`, variadic), `#include` in quote and angle form (quoted includes search the including file's directory first, then the `-I` directories; a cycle is cut at a depth limit), and the `#if`/`#ifdef`/`#ifndef`/`#elif`/`#else`/`#endif` family with `defined` and C integer operators. Predefined macros come from the `--target` triple, never from the host. A diagnostic names its file and line.

Type names follow the pointee. `T *` is `T_ptr`, declared as `@FFI.Pointer { target: T, ownership: borrowed }`; `T **` is `T_ptr_ptr`; `void *` is `RawPtr` and `char *` is `CString` where allowed. An opaque struct tag (`typedef struct X_T *X`) is `@FFI.Alias { target: X_T }`. A C array field is `<element>_array_<count>`, and an array parameter decays to a pointer. A function-pointer typedef is `@FFI.Callback { abi: c, params, result }`.

---

## 27. Language profile: Kira

### 27.1 Definition constants

`name = "kira"`, `driver = "kira"`, `extensions = ["kira"]`, `memory = .Affine`, `exclusivity = .Static`, `convention = .Ridl`, one unit per package, no file modes.

### 27.2 Layers

- Core: the `.klfd` definition. It owns the lexer, the macro phase, the parser, semantic analysis, and lowering. It defines the primitive types, operators, literal forms, statements and declaration kinds of Kira. Changing any of these is a `.klfd` edit.
- `heap` and `std`: Kira source under `std/`. Lang items the definition requires: `String`, `Array`, `Optional`, `Result`, `Drop`, `Copyable`, `Number`, `Any`.

### 27.3 Mapping of Kira features to KIR

| Kira | KIR |
| --- | --- |
| Owned parameter, `borrow`, `borrow mut` | Conventions `owned`, `borrow`, `borrowmut` |
| `move x` at a call site | `move %x` |
| `copy x` | `copy %x`; requires the type's copy policy `explicit` |
| Trivially copyable scalars | Policy copy `trivial` |
| Struct | Loadable nominal; copy `explicit` if `Copyable`, else `none`; binding `var w = v` on a `Copyable` struct emits `copy` because Kira defines that a struct copies when bound |
| Array | Nominal wrapping `ref` to a buffer; copy-on-write through `ref.isunique`; binding moves |
| Enum with payload | RIDL-rule enum; binding moves |
| `String` | Four-word RIDL string |
| `Drop` types | Policy destroy `fn(@T.drop)`, copy `none`, module attribute `lifetimes(scoped)` |
| `with` regions and non-escaping borrows | `borrow.begin` and `borrow.end`; the verifier's V5 and V6 are the region rules. Borrows cannot be returned, stored or captured |
| Traits | Kernel traits. Concrete calls resolve statically; a trait in type position is a trait object |
| Constructs | A family lowers to an enum whose cases carry the backed structs; a call through a family value lowers to `switch.enum` plus direct calls. `Any Family` values are that enum |
| Generics | `specialize` internally; `both` when exported |
| `attempt`, `try`, `handle` | `try.call` for failing functions; `switch.enum` on Result-shaped enums otherwise |
| `match` | `switch.enum` without default |
| `@Runtime`, `@Native` | Engine attribute |
| `@Main` | Module entry point |
| `@Export` | RIDL export; the definition emits the `.ridl` |
| `@Abi(version:)` | Section 24.4 |
| Closures | RIDL closure shape; captured `var`s live in a counted cell |
| Tasks | Calls into `std`'s scheduler; no kernel support |
| FFI annotations | The same declarations as `.klffi`; a Kira file may contain them inline |

### 27.4 Macros and compile-time execution

The macro phase runs inside `parse`: declarative macros are expanded by the definition; `comptime macro` and `comptime function` bodies are compiled to KIR by the definition itself and run through `Session.evaluate`. The reflection values (`Syntax`, `Declaration`) are Kira std types implemented over text spans, so expansion stays a source-to-source transform and backend parity is structural.

### 27.5 `Number`

`Number` is Kira's default numeric type for integer and decimal literals. It is a 16-byte loadable nominal with trivial copy. Encoding of the 128-bit word: the low two bits are a tier tag. Tag 0: integer; the value is the word arithmetically shifted right by 2 (126-bit signed). Tag 1: decimal; bits 2 to 7 hold a decimal scale from 0 to 63 and the upper 120 bits a signed mantissa. Tag 2: rational; the upper bits hold a pointer to a counted `{ numerator, denominator }` of big integers. Tag 3: big decimal; pointer to a counted big decimal. Tags 2 and 3 make the type's policy copy `implicit` with `pairing: counted`; the definition emits inline fast paths for tag 0 and tag 1 (an add is one 128-bit add with overflow check plus a tag test) and calls `std` for the rest. Division stays integer when exact, promotes to rational otherwise. `Float` remains IEEE binary64.

The sized numbers `N8`, `N16`, `N32`, `UN8`, `UN16` and `UN32` are `Number` with a width: `N` names a signed range of that many bits, `UN` an unsigned one. Each is stored as the two's-complement integer of its width (section 21.1), so an array of `UN8` is a byte buffer. Conversion goes one way only: a sized number converts implicitly to any number type that holds all its values, and to `Number` always; the reverse is an explicit conversion such as `UN32(x)`, which truncates toward zero as `Int(n)` does and traps outside the range; a literal of a sized type must lie in its range. Comparison, `+`, `-`, `*`, `%` and the bitwise operators act at the width, and `+`, `-` and `*` trap when the result leaves the range; a shift drops the bits it moves past the width, as an integer shift does. `/` and the negation of an unsigned sized number answer a `Number`, since their results need not be whole or in range. There is no 64-bit sized number: a 64-bit value is `Number`. Each is a type of its own, never an alias of `Number` or of its storage integer: a `.ridl` spells it by its name (section 20.2) and a reader keeps it as that type, laid out as its storage integer.

### 27.6 Worked example (informative)

Source:

```
struct Vec3 { let x: Float  let y: Float  let z: Float }

@Export
function scale(v: borrow Vec3, by: Float) -> Vec3 {
    return Vec3 { x = v.x * by, y = v.y * by, z = v.z * by }
}
```

Emitted `.ridl`:

```
struct Vec3 { let x: Float  let y: Float  let z: Float }
function scale(v: borrow Vec3, by: Float) -> Vec3
```

Canonical KIR:

```
func exported @Demo.scale(%v: borrow %Vec3, %by: f64) -> %Vec3 engine(any) {
bb0(%v: borrow %Vec3, %by: f64):
  %x = struct.extract %v, 0
  %y = struct.extract %v, 1
  %z = struct.extract %v, 2
  %sx = fmul %x, %by
  %sy = fmul %y, %by
  %sz = fmul %z, %by
  %r = struct.new %Vec3(%sx, %sy, %sz)
  return %r
}
```

Convention on aarch64: `Vec3` has three float leaves, so the borrowed `v` arrives in v0, v1, v2, `by` in v3, and the result returns in v0, v1, v2. Symbol: `_RIDL14Demo5scaleF_v<abitag>`.

### 27.7 Frameworks and SDKs

A program reaches a RIDL library installed on its target system through the library's `.ridl` contract, read in place. No bindings are generated into the program's sources.

1. An SDK is a directory that stands for a target system's installed interfaces. The Kira driver accepts `--sdk <path>` on `build`, `check` and `run`; the default is no SDK.
2. A framework is the directory `<Module>.framework` in one of the SDK's two framework directories, `<sdk>/System/Library/Frameworks` for the frameworks any program may use and `<sdk>/System/Library/PrivateFrameworks` for those only the system itself uses. An SDK given to programs outside the system has no `PrivateFrameworks`. A framework holds the library's shared binary, `<Module>`, and its contract, every `.ridl` file under `Resources/` (section 20.1).
3. `import Name` resolves, in order: to a dependency the package's manifest declares, to a package the toolchain bundles, then to the framework `Name.framework` in the SDK, looked for in `Frameworks` and then in `PrivateFrameworks`. A name none of them answers is an unknown import, as before. A framework whose `Resources/` holds no `.ridl` file is a diagnostic naming the framework. When the SDK holds a framework of a dependency's name, that dependency is the framework: the package the manifest names is not read, and nothing of it is compiled into the program. A library a target system installs as a framework is linked from that framework by every program built for the system, never compiled in, so a process holds one copy of it and of its state; a build without the SDK, for another platform, compiles the package as the manifest names it.
4. A framework import is a module of its own whose declarations are its contract, read as Kira declarations with the meaning `klf ridl bindgen --lang kira` gives them: each type as declared, each function and method a RIDL import from the module. An `import Other` in the contract resolves by rule 3 without the manifest: to a package the toolchain bundles, then to a framework. A bundled package is part of the toolchain on every target, as Foundation is, so a type a contract takes from one is the type the program compiles from the same package, and a contract never reaches a package only its library's manifest names. A module is read once however many modules import it.
5. A program that imports a framework, directly or through another module, links it dynamically: the link names the framework's `<Module>` in the SDK and records as run path the framework's directory with the SDK's path removed, `/System/Library/Frameworks/<Module>.framework` or `/System/Library/PrivateFrameworks/<Module>.framework`, where the framework is installed on the target. The manifest names nothing for it.
6. A module's emitted `.ridl` declares only the types the module defines. A type it takes from another RIDL module is made visible with `import Other` and is never declared again, so a type has one declaration across every contract.
7. A construct family that crosses a module boundary is a RIDL trait, and `Any Family` is its trait object (sections 21.7 and 22.6). A family crosses when a framework's contract declares it or a library makes it `public`; any other family stays the enum of section 27's table. The trait's methods are the family's members ordered by name, so every module that writes the trait gives each member the same witness-table entry (section 23.2). A value member is a method taking `borrow self` and nothing else, and such a method of a trait without a default reads back as a `@Required let`. A member with a default is also declared in an `extend` of the trait, whose module exports the default under the trait's `M` symbol. A declaration seen through the family is a struct of its members with `extend Declaration: Family`, and reads back as `construct Declaration(fields) extends Family`, its fields its inputs with their defaults. Kira allocates the box of every trait object it makes as it allocates any of its boxes, holder-counted, so copying a trait object shares the box rather than copying the value; a member taking `borrow mut self` or `self` first copies the value into a box of its own when another holder shares it. A member taking `self` through a witness table takes over the whole box and frees its storage through the box's `free`. `is` and `as` do not apply to a crossing family's values (section 19 rule 5).

### 27.8 Platforms and platform imports

`Build.platform()` names the platform a build is for, from its target triple: `"tessera"` for a Tessera triple (section 5.8), else `"ios"`, `"macos"`, `"windows"`, `"android"`, `"linux"` or `"web"` as the triple names them, else `"unknown"`. A Tessera build is `"tessera"`, never `"linux"`, though Linux is behind it.

Foundation's `TargetPlatform` has the case `Tessera`, and `targetPlatform!()` folds to it for a Tessera build. A library that matches over `TargetPlatform` says what it does on Tessera in an arm of its own: Tessera is never folded into `Linux`.

An import may name the platforms it is made on:

```
import AppKit on tessera
import Metal on macos, ios
```

1. The names after `on` are platform names as `Build.platform()` spells them, separated by commas. A name that is not one of them is a diagnostic.
2. A file that holds such an import is a platform file, and every import in it comes before its first declaration; an import with `on` after a declaration is a diagnostic. A platform file takes part in its module only when the build's platform is named by every `on` clause in the file. Otherwise the file is not read past its imports: its declarations do not exist, its imports are not resolved, and nothing in it is checked or compiled.
3. When a platform file takes part, each of its imports resolves and is visible exactly as an `import` without `on`. An import that does not resolve is an unknown import, as for any other import: a platform file is never left out because a name it imports is missing.
4. A file outside the platform file reaches the platform file's declarations only through code that exists on that platform alone, which is code a macro emits by `Build.platform()` (section 27.4). A reference that remains on another platform is an unknown name there.

Rationale: a library written for every platform reaches one platform's system interfaces in files of its own, and a build for any other platform never sees those interfaces, so it needs no SDK for them.

---

## 28. Language profile: KArc

### 28.1 Definition constants

`name = "karc"`, `driver = "karc"`, `memory = .Unmanaged`, `exclusivity = .None`, `convention = .Ridl`, one unit per source file, file mode `arc`.

KArc compiles two file kinds and the extension decides which:

| Extension | Mode | Memory |
| --- | --- | --- |
| `.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.inl` | C++ (section 28.6) | Manual, exactly as clang |
| `.krc`, `.krch` | ARC (section 28.5) | Counted, mandatory |

`.cpp` is a clang-compatible C++ compiler and nothing else: the semantics are clang's, `new`/`delete` and the C allocator are the programmer's to balance, and no reference counting is imposed. `.krc` is the KArc dialect: automatic reference counting is mandatory, manual memory management is removed from the language, and the grammar differs as section 28.5.1 lists. The mode belongs to the file in which code is written, not to the translation unit that includes it, so an inline function or template defined in a `.krch` header has ARC semantics wherever it is instantiated and C++ semantics if defined in a `.hpp`.

### 28.2 Language accepted

Both modes accept C++20 as implemented by clang, measured on the corpus, including: classes with single, multiple and virtual inheritance; virtual functions; all template forms (class, function, variable, alias; partial and explicit specialization; variadic; non-type and template template parameters); SFINAE; concepts and `requires`; `constexpr`, `consteval`, `constinit`, `if constexpr`; lambdas; overload resolution with the full conversion ranking; argument-dependent lookup; user-defined conversions and literals; operator overloading; references and rvalue references; move semantics; RAII; placement new; unions; bit-fields; scoped and unscoped enums; namespaces; `alignas`; attributes; structured bindings; range-for; fold expressions; designated initializers; three-way comparison; inline variables; `thread_local`; `std::atomic` through builtins; the complete preprocessor; GNU extensions required by system and libc++ headers (`__attribute__`, `__builtin_*` as listed in the definition's builtin table, `__has_include`, `__has_builtin`, `__has_feature`, statement expressions, `typeof`, `__int128`, `#pragma once`, `#pragma pack`); inline assembly on the native engine. The rows section 28.5.1 removes are the exceptions, and they are the only ones.

### 28.3 Rejected or gated

| Feature | Behavior |
| --- | --- |
| Exceptions | Off by default with the semantics of `-fno-exceptions`: `throw` and `try` are errors `KARC-E0301`. Enabled per module with `-fexceptions`; then `throw` lowers to `throw`, calls to `invoke`, handlers to `landingpad` with RIDL-box exception objects |
| RTTI, `dynamic_cast`, `typeid` | Off by default (`KARC-E0302`). Enabled with `-frtti`; type info objects are module-private |
| C++20 modules, C++20 coroutines | Error `KARC-E0303`, `KARC-E0304` |
| `setjmp` and `longjmp` across frames with non-trivial destructors | Error `KARC-E0305` |
| Trigraphs, `register`, `export template` | Ignored with warning or error as clang |
| Linking prebuilt C++ binaries | Impossible by design. C libraries through `.klffi` |

### 28.4 Layout and lowering

Class layout follows the Itanium rules (vtable pointer placement, base subobject order, virtual base offsets, empty base optimization, bit-fields) through `Layout.Explicit`, so that code depending on `offsetof`, `sizeof` and layout-sensitive tricks behaves as under clang on the same target. Virtual dispatch uses KIR `vtable` declarations and `call.vtable`. Internal symbols use Itanium mangling. Classes with user-provided move constructors or destructors that make them non-trivially relocatable get relocate `fn` and are address-only.

### 28.5 ARC mode (`.krc`, `.krch`)

1. `new T(args)` and `new T[n]` allocate a counted slot (`klfrt_alloc_counted`) whose destroy function runs the destructor (all element destructors for arrays). `delete` and `delete[]` are errors `KARC-E0101`. Explicit destructor calls are allowed only on storage obtained by placement new.
2. Every storage location of object pointer type (locals, parameters, members, globals, array elements), including `void*`, is a strong reference: initialization retains, assignment retains the new value then releases the old, end of lifetime releases. In KIR the type is the nominal `%karc.ptr` with policy copy `implicit(@klfrt_retain)`, destroy `fn(@klfrt_release)`, relocate `bitwise`, pairing `counted`. Because counting is by address (section 18.3), pointers to stack objects, to members and into arrays are safe: they are no-ops or resolve to the enclosing allocation.
3. `[[karc::weak]] T* p` declares a weak reference: reads yield a strong temporary or `nullptr`. `[[karc::unowned]] T* p` declares an uncounted pointer.
4. `this` is borrowed for the duration of a member call. References (`T&`, `T&&`) are borrowed and uncounted; storing the address of a referent into a pointer location retains.
5. Function pointer types and member pointers are trivial.
6. Pointer arithmetic yields interior pointers, counted through the slot lookup. Files that hide pointers in integers, XOR them, or tag their low bits MUST be `.cpp`.
7. A class type that contains strong pointers (directly or in members or bases) is not trivially copyable and not trivially destructible: `std::is_trivially_copyable`, `__is_trivially_copyable` and related traits report false, and its implicit copy operations retain. `memcpy`, `memmove`, `memset` and `realloc` on storage whose static type contains strong pointers are errors `KARC-E0110`. All of this holds in whichever mode the type is used, because it is a property of the type and the type has one defining file (section 28.7.1).
8. Pointer members of unions are an error `KARC-E0104`.
9. Class-specific and replaced global `operator new` and `operator delete` are errors `KARC-E0105`.
10. `[[karc::foreign_managed]]` on a class makes every pointer to it or to a class derived from it unowned. Unreal's `UObject` hierarchy receives the attribute from the UE5 support header `karc/ue5.h`, which the driver force-includes when `-fkarc-ue5` is given; those objects stay under the engine's garbage collector.
11. Destructors run at the last release, on the releasing thread, at a deterministic point.
12. Cycles leak until broken with `[[karc::weak]]`. The cycle detector of section 18.6 is on in debug builds and is the oracle for migration.

### 28.5.1 How `.krc` differs from C++

The dialect is C++20 minus manual memory management, plus the two ownership qualifiers ARC needs. Every difference below is a hard error in `.krc`, never a warning, so a `.krc` file cannot express an unbalanced lifetime.

| Construct | In `.krc` | Code |
| --- | --- | --- |
| `weak T* p`, `unowned T* p` | Declaration specifiers, where C++ spells them `[[karc::weak]]` and `[[karc::unowned]]`. The attribute forms are accepted as synonyms | — |
| `delete p`, `delete[] p` | Removed from the grammar | `KARC-E0101` |
| Placement `new (p) T(args)` | Removed; an explicit destructor call has no storage it may run on | `KARC-E0102` |
| `reinterpret_cast` between a pointer type and an integer type, and the equivalent C cast | Removed | `KARC-E0103` |
| Pointer members of a `union` | Removed | `KARC-E0104` |
| Declaring `operator new` or `operator delete` | Removed | `KARC-E0105` |
| `malloc`, `calloc`, `realloc`, `free` | Not declared; `std/` does not export them to `.krc` | `KARC-E0106` |
| `memcpy`, `memmove`, `memset`, `realloc` over storage whose static type holds strong pointers | Removed | `KARC-E0110` |
| `#pragma karc raw` | Not a mode a `.krc` file may enter | `KARC-E0107` |

Everything else — classes, templates, concepts, lambdas, overloading, `constexpr`, the preprocessor — is the same language `.cpp` gets, and a declaration written once in a `.krch` header parses identically in both modes unless it uses one of the rows above.

### 28.6 C++ mode (`.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.inl`)

C++ mode is clang: pointers are trivial `ptr`; `new` allocates from the KLF allocator with strong count 1 held by the manual owner; `delete` is `klfrt_delete_raw` (section 18.4); all standard C++ memory semantics apply exactly; type traits report what clang reports; nothing is retained or released behind the programmer's back. A `.cpp` file is expected to compile under clang with the same meaning, and the corpus measures that.

`#pragma karc arc` before any declaration, or a match against `-fkarc-arc=<glob>`, puts a C++-extension file into ARC mode; this exists to migrate a tree file by file without renaming it, and a file that has entered ARC mode is bound by section 28.5.1 in full.

"Supports ARC" is defined mechanically: a file supports ARC if it compiles in ARC mode. The driver flag `-fkarc-report` prints, per file, the errors that keep it in C++ mode; the size of that list is the migration metric.

### 28.7 Crossing between modes

The two modes are one compiler, not two languages meeting at a boundary. A `.krc` file and a `.cpp` file in the same build share a preprocessor, a parser, an overload set, Itanium mangling and one KIR module; they call each other with ordinary `call` instructions and inline into each other. There is no wrapper, no trampoline, no thunk and no `.klffi` between them. The mode changes which instructions lowering emits for a storage location, and nothing else.

#### 28.7.1 The declaring file decides

Every storage location takes its policy from the file whose source text declares it — not from the file that uses it, and not from the translation unit it ends up in. This is section 28.6's rule about function bodies, applied to every declaration.

| Declaration | Mode that decides its policy |
| --- | --- |
| Local, parameter, temporary | The file containing the function body |
| Non-static data member | The file containing the class definition |
| Global, static data member, string literal | The file containing the definition |
| Entity produced by a template instantiation | The file containing the template definition, wherever it is instantiated |

Three things follow, and together they are the whole crossing story.

**A C++ type has exactly one KIR type.** A class has one defining file, that file has one mode, so its members have one policy. `struct Node { Node* next; };` in a `.krch` header has a strong `next` in every file that includes it, `.cpp` files included: their copies retain, their destructions release, and `std::is_trivially_copyable<Node>` is false in both modes. The same struct in a `.hpp` has a trivial `next` everywhere, `.krc` files included. There is never a second layout, a second mangling or a second nominal type for one class, so "shared between modes" needs no conversion — the sharing is the normal case.

**Locations a `.krc` file declares are strong even when the types are not.** A `.krc` function holding a `Node*` local, where `Node` came from a `.hpp`, counts that local: the location is written in a `.krc` file. Its `next` member is still trivial, because the member is written in a `.hpp`. Policy attaches to locations, and each location has exactly one declaring file.

**Values in flight are borrowed.** A pointer that is being passed, returned or evaluated is not a storage location and is never counted. Only storing it is.

#### 28.7.2 What happens at each crossing

| Situation | Behavior |
| --- | --- |
| `.krc` code calls a `.cpp` function, or the reverse | A direct call. Arguments are borrowed for the call: no retain, and the caller's strong reference keeps the object alive across it |
| The callee keeps a pointer it was passed | Its declaration MUST carry `[[karc::retains(param)]]`. The caller then performs a retain that the callee owns and must balance with `karc::release(p)`. Without the attribute, keeping it is a dangling reference and the debug cycle detector reports it |
| A `.cpp` function returns a pointer to `.krc` code | Unowned until stored; storing into a strong location retains |
| A `.krc` function returns a pointer to `.cpp` code | The returned value carries no count. If the `.krc` side held the last strong reference, the object is already gone; returning it MUST go through `karc::retained(p)`, which hands the `.cpp` caller a +1 to balance |
| `.krc` code holds a pointer to an object `.cpp` code deletes | The destructor runs, the payload is poisoned, the slot is quarantined until the `.krc` holders release. Debug builds trap on the next ARC dereference |
| `.cpp` code allocates, `.krc` code ends up the last holder | The slot has count 1 owned by the manual owner; ARC releases never reach zero before the manual `delete`, so C++ semantics are preserved |
| A class defined in one mode, used in the other | No conversion. One type, one layout, one policy, as 28.7.1 says |
| A template defined in one mode, instantiated in the other | Instantiated with the defining file's mode. Instantiating it in both modes produces one instance, not two |
| The address of a storage location | A `T*` pointing at a strong location and a `T*` pointing at a trivial one are distinct types and neither converts to the other (`KARC-E0108`). Pass and return by value, or route through an `unowned` local, whose storage is trivial in both modes |
| The same entity defined in both modes | Error `KARC-E0109`. A mangled name is lowered once, so it has one mode |
| `memcpy`, `memmove`, `memset`, `realloc` over a type holding strong pointers | Error `KARC-E0110` in both modes. The policy travels with the type, so the ban does too |

The last four rows are the only places a mixed tree needs to be written differently from a pure one. Everything above them is code calling code.

### 28.8 Templates, overloads, constant evaluation

Templates are instantiated by the definition through `Session.instance` (one instance per argument list per build, cached across builds). Two-phase lookup, SFINAE and tentative parsing use `Session.speculate`. Overload resolution uses the solver with clang's ranking as the cost vector. `constexpr` evaluation lowers the function to KIR and calls `Session.evaluate`.

### 28.9 Standard library

`std/` contains KArc's C++ standard library: headers and sources written in ordinary C++20, compiled in C++ mode except the files listed in `std/arc.list`. The allocator entry points it exports to `.cpp` (`malloc`, `free`, `operator new`) are not visible to `.krc` (`KARC-E0106`). It MUST provide every standard header the UE5 corpus includes. Its allocator functions route to the KLF runtime. The C library headers come from the C toolchain's std.

### 28.10 RIDL export

`extern "ridl"` linkage specification or `[[klf::export]]` exports a declaration. Classes export as opaque structs with methods; pure-virtual interface classes export as traits; enums and trivially laid-out structs export as RIDL enums and structs; templates export polymorphically when they meet 13.2 and as named instances for explicit instantiations.

### 28.11 Driver compatibility

`compatDriver` accepts clang's command line (`-c`, `-o`, `-I`, `-D`, `-U`, `-include`, `-isystem`, `-std=`, `-O`, `-g`, `-f` and `-m` flags that affect semantics, `-W` flags, `-MD` dependency output, response files) so that UnrealBuildTool's Linux toolchain can invoke `karc` in place of clang on a tree of `.cpp` files it never has to know is not clang. Unknown warning flags are ignored; unknown code generation flags are errors.

This is the target compatibility surface, not a claim that KArc or C currently
implements clang parsing. The first request contract only transports normalized
arguments to the uniform driver; compatibility parsing and clang flag
semantics remain language-definition work.

### 28.12 Worked example (informative)

```
struct Node {
    Node* next = nullptr;
    [[karc::weak]] Node* prev = nullptr;
    int value = 0;
};

Node* push(Node* head, int value) {
    Node* n = new Node();
    n->value = value;
    n->next = head;
    if (head) head->prev = n;
    return n;
}
```

Canonical KIR (abridged):

```
func module @_Z4pushP4Nodei(%head: borrow %karc.ptr, %value: i32) -> %karc.ptr engine(any) {
bb0(%head: borrow %karc.ptr, %value: i32):
  %n = call @karc.new.Node()                      ; owned, count 1
  %na = call @karc.deref(%n)                      ; ptr to payload
  %vaddr = ptr.offset %na, 16
  raw.store %value, %vaddr
  %h2 = copy %head                                ; retain for the member
  %nextaddr = ptr.offset %na, 0
  call @karc.assign(%nextaddr, %h2)               ; releases old (null), stores
  %isnull = call @karc.isnull(%head)
  cond.br %isnull, bb2, bb1
bb1:
  %ha = call @karc.deref(%head)
  %prevaddr = ptr.offset %ha, 8
  call @karc.weak.assign(%prevaddr, %n)           ; weak store, no strong retain
  br bb2
bb2:
  return %n
}
```

The list owns forward through `next` and refers backward through `prev`, so dropping the head releases every node.

---

## 29. Language profile: C

### 29.1 Definition constants

`name = "c"`, `driver = "kcc"`, `extensions = ["c", "h"]`, `memory = .Unmanaged`, `exclusivity = .None`, `convention = .Ridl`, one unit per source file, file mode `platform`.

### 29.2 Language accepted

ISO C17 plus the GNU C extensions the corpus requires: `__attribute__` (including `section`, `aligned`, `packed`, `weak`, `alias`, `visibility`, `noreturn`, `always_inline`, `noinline`, `used`, `unused`, `cold`, `hot`, `format`, `constructor`, `destructor`, `naked`, `cleanup`), statement expressions, `typeof` and `__auto_type`, labels as values and computed `goto`, case ranges, designated initializer ranges, zero-length and flexible arrays, variable length arrays, `__int128`, `_Static_assert`, `__builtin_*` per the definition's table, `asm` labels, inline assembly, `#pragma pack`, `#include_next`, `__has_include`, thread-local storage, atomics, nested functions are rejected (`KCC-E0201`).

### 29.3 Memory and layout

C is unmanaged: pointers are `ptr`, `malloc` and `free` come from the C std and route to the KLF allocator. C objects use the target's C layout rules (`LayoutRule.PlatformC`), because C programs observe layout through memory: network headers, device registers, file formats and system call structures. This is a property of C inside the C definition; RIDL is not involved.

### 29.4 Calling convention

- RIDL mode (default): functions use `ridlcc` with C types flattened as scalars and structs passed under section 22.3. Code compiled by `kcc` does not interoperate at the binary level with code compiled by a platform C compiler. Everything is recompiled.
- Platform mode: a file containing `#pragma klf abi(platform)` or matching `-fklf-platform-abi=<glob>`, or a whole build with `--abi=platform`, uses the platform C convention for every function it defines. This mode exists for code bound to assembly, firmware or hardware conventions. The Linux kernel is built with `--abi=platform`.
- A function pointer's type carries its convention. Pointers declared in RIDL-mode files are `ridlcc`; pointers whose type comes from a `.klffi` import or a platform-mode file are platform convention. Converting between them inserts a thunk (section 26.4); converting a pointer of unknown provenance is error `KCC-E0310`.
- External C libraries are reached only through `.klffi` imports. An ordinary `extern` declaration in a RIDL-mode file refers to a `kcc`-compiled definition.

### 29.5 Variadic functions

In RIDL mode a variadic function receives its fixed parameters normally and one hidden trailing pointer to a caller-allocated argument area: consecutive 8-byte slots (16 for 128-bit integers and `long double`), arguments after the default promotions. `va.arg` reads the next slot. In platform mode and through `.klffi`, variadics follow the platform.

### 29.6 RIDL export

`__attribute__((klf_export))` exports a function. Parameter mapping: fixed-width integers and floats to RIDL scalars (`int` is `I32`, `long` by target); pointers to `RawPtr`; structs whose fields are all scalars of fixed width and whose C layout equals the RIDL layout export as RIDL structs, any other struct is passed as `RawPtr`. C cannot express ownership conventions, so every exported C parameter of pointer type is unmanaged.

### 29.7 Freestanding and kernel builds

`-ffreestanding`, `-nostdlib`, `-mgeneral-regs-only`, `-fno-pic`, `-mcmodel=`, `-fno-strict-aliasing` (KIR has no type-based aliasing; the flag is accepted and has no effect), linker scripts through the bundled lld, custom sections, and `naked` functions are supported. `compatDriver` accepts the GCC and clang command lines used by Kbuild.

---

## 30. Language profile: Swift (expressibility)

Swift is not implemented. This section shows that each Swift mechanism has a home in the kernel, which is the test that the kernel is general.

| Swift | KLF mechanism |
| --- | --- |
| ARC on classes, closures, existential boxes | `ref T` with policy copy `implicit(retain)`, pairing `counted`; the optimizer of 14.3 |
| Ownership conventions (`borrowing`, `consuming`, `inout`, guaranteed and owned parameters) | Conventions of 11.3 |
| Non-copyable types | Policy copy `none` |
| Exclusivity enforcement, static then dynamic | `access.begin [static]` with `[dynamic]` fallback |
| Unspecialized generics, value witness tables, protocol witness tables | Strategy `polymorphic`; descriptors and witness tables of section 23 used internally as well as at boundaries |
| Associated types | Witness table entries of 23.2 |
| Existentials | Trait objects. Swift's inline three-word buffer is an internal optimization behind the boundary |
| Resilient structs and enums | Internally: address-only types whose size comes from a descriptor. At RIDL boundaries: opaque structs or frozen types |
| Enums with payloads, indirect cases | KIR enums; `indirect` is a `uniq` box |
| Typed and untyped `throws` | `fails E`; untyped `throws` uses `E = Any Error` |
| Bidirectional type inference | The solver of 8.6 with disjunctions |
| Property wrappers, result builders, macros | Definition-level desugaring |
| Async and actors | State-machine lowering in the definition plus a Swift std executor; no kernel support needed |
| Objective-C interop | Out of scope; would be a `.klffi`-like foreign boundary |
| Dynamic casts, reflection | Swift-internal metadata emitted by the definition as ordinary globals; never crosses RIDL |

What Swift forced into the design: the polymorphic strategy, descriptors as first-class KIR operands (`descriptor.ref`, `type.size`), dynamic exclusivity, and scored disjunctions in the solver.

---

## 31. Conformance, diagnostics and security

### 31.1 Corpus-defined conformance

A hosted language conforms when its toolchain matches the oracle on the language's corpus. `corpus/<name>.klfd` manifests list sources, build commands, the oracle, and what is compared.

A manifest holds one `construct <Name>() extends Corpus` with three members.
`cases: [CorpusCase]` lists the cases; a `CorpusCase` literal has `name`,
`sources` (paths relative to the manifest), optional `arguments` for the
driver, and the recorded expectations `accept` (default `true`),
`diagnostics` (`"<code>:<line>"` strings, default `[]`), `stdout` (default
`""`) and `exit` (default `0`). `oracle: [String]` (default `[]`) is a command
prefix; when present its results replace the recorded expectations.
`compare: [String]` (default `["accept", "stdout", "exit"]`) selects from
`accept`, `diagnostics`, `stdout` and `exit`; `diagnostics` requires recorded
expectations. Each case builds with `<command> <sources> <arguments> -o
<program>` in the manifest's directory, for the driver and for the oracle.
A command accepts when it exits with status 0; each driver stderr line
`file:line:col: error[code]` yields `"<code>:<line>"`. When both sides accept,
both programs run with no arguments and their stdout and exit status are
compared. A case's `cArchives` lists C sources, relative to the manifest,
that its package links as native libraries; each is compiled for the host by
the bundled clang into `lib<stem>.a` beside it before the case builds. A
case's `hosts` (default `[]`, every host) names the operating systems it runs
on, as Rust's `std::env::consts::OS` spells them (`"linux"`, `"macos"`); on
any other host the case is skipped and reported `SKIP`, neither passing nor
failing. A case of a Linux system call is one.
`klf test <defdir> [--corpus <name>]` builds the toolchain into a
scratch sysroot, runs every manifest (or `<name>.klfd`), prints `PASS`/`FAIL`
per case, and fails when any case fails.

| Language | Corpus | Oracle | Compared |
| --- | --- | --- | --- |
| Kira | `tests-kik` and the Kira ecosystem packages (Kira UI, Ember, Foundation) | Pre-KLF Kira compiler 1.9.x | Accepted or rejected with the same diagnostic code at the same line; stdout, exit code |
| C | Linux kernel (boot to userspace in QEMU on aarch64 and x86_64), musl, SQLite, zlib, Lua, the GCC C torture tests | GCC and clang | Program output, test suite results, boot success |
| KArc | Unreal Engine 5 engine and editor in C++ mode, then progressively in ARC mode; libc++ test suite for the accepted language; LLVM's C++ frontend tests restricted to the accepted language | clang | Accept or reject, program output, engine automation tests |

### 31.2 Kernel conformance

| Suite | Content |
| --- | --- |
| KIR verifier | One positive and one negative `.kir` file per rule V1 to V12 |
| Ownership passes | Golden raw-to-canonical pairs for each step of 11.6 across the three memory models |
| Backend parity | Every `.kir` test program runs on VM, native and hybrid and MUST produce identical output and traps |
| RIDL layout | For every construct of section 21, a table of expected sizes, alignments and offsets per target |
| RIDL convention | Assembly-level callers and callees per target verifying register assignment for the classification cases of 22.3 |
| RIDL evolution | Old client against new provider for each allowed addition of 24.4; each forbidden change MUST be rejected by `klf abi freeze` |
| Runtime | Allocator, counting by interior pointer, quarantine, weak loads, cycle detector, heap balance |
| Hot push | Replace, NACK reasons, two-version semantics |

### 31.3 Differential testing loop for agents

For KArc and C, every stage has an oracle: preprocessed text (against `clang -E`), accept or reject, constant evaluation results, record layouts (against `-fdump-record-layouts`), and program behavior. An agent implementing a frontend feature MUST add the smallest corpus excerpt that exercises it to the definition's regression suite.

### 31.4 Diagnostics

- Kernel codes: `KLF-<AREA>-<NNNN>` with areas `DEF`, `DRV`, `KIR`, `RT`, `RIDL`, `FFI`, `VM`, `HYB`. KLF Kira frontend codes: `KLFK-E<NNNN>`.
- `KLF-DRV-0022`: one source matched different file modes through its extension, pragma, or mode-flag glob selectors.
- Each definition owns its codes and registers them in `diagnostics` as `DiagnosticSpec { code, severity, title, explanation }`. Kira keeps its existing families (`KPAR`, `KSEM`, `KMAC`, `KIR`), KArc uses `KARC-E` and `KARC-W`, C uses `KCC-E` and `KCC-W`.
- The ownership passes report through keys (`own.use-after-move`, `own.borrow-escape`, `own.consume-while-borrowed`, `own.exclusivity`, `init.use-before-init`, `init.double-init`, `ridl.not-exportable`, `ridl.abi-broken`, `ridl.generic-not-polymorphic`, `ridl.open-enum-needs-default`). A definition MUST map every key to one of its codes.
- Diagnostics render as `severity[code]: message` followed by the source line, a caret range, notes and fixes. `--diagnostics=json` emits one JSON object per line.

### 31.5 Trust model

- Definitions are trusted: they are compiled to native code inside the driver. Installing a toolchain is equivalent to installing a compiler.
- User code executed at compile time (macros, `comptime`, `constexpr`) is untrusted: it runs only in the sandboxed VM of 14.4.
- Bytecode loaded by a runner is verified before execution: instruction operands in range, call-site signatures consistent, register counts within the frame, jump targets on instruction boundaries.

### 31.6 Unmanaged manifest

Every module records the symbols of functions that contain unmanaged instructions (11.5). The list is part of the RIDL metadata (24.3). A platform may gate loading on it, as an entitlement check. A module whose list is empty and whose imports have empty lists is memory-safe up to kernel and runtime correctness.

---

## 32. Implementation plan and kernel crate layout

### 32.1 Crates

| Layer | Crates |
| --- | --- |
| 0 | `klf-core` (ids, interner, errors), `klf-source`, `klf-diag`, `klf-target` (triples, `CLayout`, feature sets) |
| 1 | `klf-query` (incremental engine, cache), `klf-layout` (RIDL and C layout), `klf-kir` (data model, text and binary formats) |
| 2 | `klf-kir-verify`, `klf-kir-own` (mandatory passes), `klf-generics`, `klf-kir-opt` |
| 3 | `klf-bytecode`, `klf-vm` (wasm32-clean), `klf-rt` (runtime, wasm32-clean), `klf-llvm`, `klf-hybrid` |
| 4 | `klf-ridl` (parser, checker, emitter, mangler, metadata, abi freeze and apply), `klf-klffi` |
| 5 | `klf-services` (the API of section 8 over layers 0 to 4), `klf-kira-syntax`, `klf-kira-sema`, `klf-kira-lower` (the KLF Kira frontend) |
| 6 | `klf-driver` (driver loop, linking, sysroot), `klf-lsp`, `klf-push` |
| 7 | `klf-cli` (the `klf` binary), `klf-test` (conformance runner) |

No crate depends on a higher layer. `klf-vm` and `klf-rt` and everything beneath them MUST build for `wasm32-unknown-unknown`.

### 32.2 Milestones

Each milestone is complete when its exit test passes on every backend it names.

| Milestone | Content | Exit test |
| --- | --- | --- |
| M0 | `klf-kir` data model, text format, verifier raw rules | Verifier suite V1 to V3 |
| M1 | Runtime allocator and counting; VM executing hand-written `.kir` | Backend parity suite on VM |
| M2 | LLVM backend; `ridlcc` in the LLVM fork | Backend parity suite on VM and native; RIDL convention suite |
| M3 | Ownership passes, full verifier, generics engine | Ownership and verifier suites |
| M4 | KLF Kira frontend and services; `klf build` producing a driver for a toy definition (integer calculator language) | The toy toolchain compiles and runs its corpus on all backends |
| M5 | RIDL: layout, emission, metadata, linking, freeze and apply, bindgen | RIDL layout and evolution suites; Rust client calling a KLF library through bindgen |
| M6 | C definition, `.klffi`, `klf ffi gen` | SQLite, Lua, zlib, musl; then the Linux kernel in platform mode |
| M7 | Kira definition and std | `tests-kik` parity with the oracle; Kira ecosystem builds |
| M8 | Hybrid and hot push | Hybrid parity and hot push suites |
| M9 | KArc definition in C++ mode, then ARC mode, std | libc++ tests; UE5 builds and passes automation in C++ mode; the list of files that cannot move to ARC shrinks monotonically |
| M10 | Optimizer O2, scale work of 9.3 | UE5 time and memory budgets |

### 32.3 Working rules

One agent owns one crate or one definition subdirectory at a time. Every change updates this specification in the same commit if behavior changes. Reviews are performed by at least one automated reviewer before merge.

---

## 33. Appendix A: grammars

Notation: `{ x }` repetition, `[ x ]` option, `|` alternative, quoted terminals. Newlines separate statements and declarations; `;` is also accepted. Comments: `//` to end of line, `///` documentation, `/* */` nesting.

### A.1 KLF Kira

```
file        = { import } { decl }
import      = "import" path
path        = ident { "." ident }

decl        = { annotation } ( function | struct | enum | trait | construct | extend | alias | distinct | constant )
annotation  = "@" path [ "(" [ args ] ")" | "{" [ pair { "," pair } ] "}" ]
pair        = ident ":" expr

function    = "function" ident [ generics ] "(" [ params ] ")" [ "->" type ] ( block | )
params      = param { "," param }
param       = [ ident ] ident ":" [ mode ] type [ "=" expr ]      (first ident is an optional external label)
            | [ mode ] "self"
mode        = "borrow" [ "mut" ] | "move" | "copy"
generics    = "<" gparam { "," gparam } ">"
gparam      = ident [ ":" type { "&" type } ]

struct      = "struct" ident [ generics ] [ ":" type { "," type } ] "{" { member } "}"
member      = { annotation } ( ( "let" | "var" ) ident ":" type [ "=" expr | block ] | function | "init" "(" [ params ] ")" block )
enum        = "enum" ident [ generics ] [ ":" type { "," type } ] "{" { case | function } "}"
case        = ident [ "(" type { "," type } ")" | ":" type [ "=" expr ] ]
trait       = "trait" ident [ generics ] [ ":" type { "," type } ] "{" { "type" ident [ ":" type ] | function } "}"
construct   = "construct" ident [ generics ] [ "(" [ params ] ")" ] [ "extends" type { "," type } ] "{" { cmember } "}"
cmember     = member | ident block | "requires" "{" { function } "}"
extend      = "extend" type [ ":" type { "," type } ] "{" { function | member } "}"
alias       = "type" ident [ generics ] "=" type
distinct    = "distinct" ident "=" type
constant    = "let" ident [ ":" type ] "=" expr

type        = path [ "<" type { "," type } ">" ] | "[" type "]" | "(" [ type { "," type } ] ")" [ "->" type ]
            | "Any" type | "some" type

block       = "{" { stmt } "}"
stmt        = ( "let" | "var" ) ident [ ":" type ] "=" ( expr | "try" expr )
            | lvalue assignop expr | expr
            | "if" expr block { "else" "if" expr block } [ "else" block ]
            | "while" expr block | "for" ident "in" expr block
            | "match" expr "{" { pattern "->" ( stmt | block ) } "}"
            | "attempt" block "handle" "{" { pattern block } "}"
            | "return" [ expr ] | "break" | "continue"
pattern     = ident [ "(" ident { "," ident } ")" ]
assignop    = "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>="

expr        = ternary
ternary     = binary [ "?" expr ":" expr ]
binary      = unary { binop unary }            precedence, low to high:
              "||" ; "&&" ; "|" ; "^" ; "&" ; "==" "!=" ; "<" "<=" ">" ">=" ; "<<" ">>" ;
              "+" "-" "&+" "&-" ; "*" "/" "%" "&*" ; "as" "is"
unary       = [ "-" | "!" | "~" | "move" | "copy" ] postfix
postfix     = primary { "." ident | "(" [ args ] ")" | "[" expr "]" }
args        = arg { "," arg }
arg         = [ ident ":" ] expr
primary     = literal | path | "." ident [ "(" [ args ] ")" ] | "(" expr { "," expr } ")"
            | type "{" [ ident "=" expr { "," ident "=" expr } ] "}"
            | "[" [ expr { "," expr } ] "]" | "[" expr ":" expr { "," expr ":" expr } "]"
            | "{" [ ident { "," ident } ] "in" { stmt } "}"
literal     = integer | float | string | "true" | "false"
```

Reserved words: `function let var return if else while true false import as is struct enum construct trait extends match attempt try for in break continue type distinct extend`. Contextual words: `borrow mut move copy handle requires init self Any some`.

### A.2 `.ridl`

```
file        = { "import" path } { rdecl }
rdecl       = { annotation } ( rstruct | renum | rtrait | rextend | rfunction | alias | rconst )
rstruct     = "struct" ident [ generics ] [ "{" { "let" ident ":" rtype [ "=" rvalue ] } "}" ]
rvalue      = literal | "." ident [ "(" rvalue ")" ] | "[" [ rvalue { "," rvalue } ] "]"
            | path [ "(" [ rvalue { "," rvalue } ] ")" | "{" [ ident ":" rvalue { "," ident ":" rvalue } ] "}" ]
renum       = "enum" ident [ generics ] "{" { ident [ "(" rtype { "," rtype } ")" ] } "}"
rtrait      = "trait" ident [ generics ] [ ":" rtype { "," rtype } ] "{" { "type" ident [ ":" rtype ] | rfunction } "}"
rextend     = "extend" rtype ( ":" rtype { "," rtype } | "{" { rfunction } "}" )
rfunction   = "function" ident [ generics ] "(" [ rparam { "," rparam } ] ")" [ "->" rtype ]
rparam      = [ "borrow" [ "mut" ] ] "self" | ident ":" [ "borrow" [ "mut" ] ] rtype
rconst      = "let" path ":" rtype "=" rvalue
rtype       = path [ "<" rtype { "," rtype } ">" ] | "[" rtype "]" | "(" [ rtype { "," rtype } ] ")" [ "->" rtype ]
            | "borrow" "(" [ rtype { "," rtype } ] ")" "->" rtype | "Any" path
```

### A.3 `.klffi`

```
file        = { fdecl }
fdecl       = ffiann { ffiann } ( "struct" ident [ "{" { "let" ident ":" ftype [ "@Bits" "(" integer ")" ] } "}" ]
                                | "function" ident "(" [ ident ":" ftype { "," ident ":" ftype } ] ")" [ "->" ftype ] )
            | "let" ident ":" ftype "=" literal
ffiann      = "@FFI." ( "Extern" | "Address" | "Struct" | "Union" | "Pointer" | "Array" | "Callback" | "Alias" | "Syscall" | "Variadic" )
              [ "{" [ pair { "," pair } ] "}" ]
ftype       = ident
```

### A.4 Textual KIR

```
module      = "module" "@" ident "lang" string "target" string "stage" ( "raw" | "canonical" ) { attr } { item }
item        = typedecl | global | func | vtable | traitdecl | conformance | import | export
typedecl    = "type" "%" name [ "<" "$" ident { "," "$" ident } ">" ] "=" ( "struct" | "enum" | "explicit" ) layoutrule body "policy" "{" policyfields "}"
global      = "global" linkage "@" name ":" ktype [ "=" constant ] [ "tls" ]
func        = "func" linkage "@" name [ kgenerics ] "(" [ kparam { "," kparam } ] ")" "->" ktype [ "fails" ktype ]
              "engine" "(" ( "runtime" | "native" | "any" ) ")" { fattr } ( "{" { kblock } "}" | )
kparam      = "%" name ":" [ "owned" | "borrow" | "borrowmut" ] ktype
kblock      = ident [ "(" [ kparam { "," kparam } ] ")" ] ":" { inst } terminator
inst        = [ "%" name "=" ] opcode operands [ "!loc" "(" string ":" integer ":" integer ")" ]
linkage     = "private" | "module" | "exported"
```

Operand syntax per instruction follows section 12 literally.

---

## 34. Appendix B: decision log

Format for `docs/decisions.md`: one entry per decision with id, date, decision, reason, sections affected.

| Id | Decision | Reason |
| --- | --- | --- |
| D-001 | The kernel is Rust, written from scratch, with no legacy code from the pre-KLF compiler | A kernel written in a hosted language would reintroduce the bootstrap cycle |
| D-002 | Kira, KArc, C and Swift are peer hosted languages. Swift is a generality proof only | The kernel must not be shaped by one language |
| D-003 | Definitions are written in KLF Kira (`.klfd`), a frozen dialect whose frontend is in the kernel | One meta-language, no staging, no binary seed |
| D-004 | KLF Kira is counted with copy-on-write, has no borrow checker, keeps constructs, forbids global mutable state | Small frozen surface; re-entrant definitions |
| D-005 | A language definition is `construct X() extends Language<Tree, Program>`; tables are plain values | No new declaration shapes |
| D-006 | `klf build` compiles the `.klfd`, then the std, then links and assembles a sysroot; one definition per toolchain | Specialized compilers without compile-time cost |
| D-007 | One ownership SSA (KIR) with per-type policy records expresses unmanaged, counted and affine memory | One verifier and one optimizer for all languages |
| D-008 | Memory models: C unmanaged, KArc imposed ARC with per-file raw exceptions, Kira affine, Swift counted | Stated requirements |
| D-009 | Reference counts live in allocator slot metadata and are found by address | Identical object layout across ARC and raw files; interior and stack pointers are safe |
| D-010 | Raw deletion of an object still referenced from ARC code poisons and quarantines | Deterministic failure instead of undefined behavior |
| D-011 | RIDL is the only contract between toolchains and with other compilers. KIR and bytecode are internal | One frozen surface |
| D-012 | RIDL conforms to no existing ABI. The platform C ABI is external, reachable through `.klffi` only. KLF C uses `ridlcc` by default, so all C is recompiled | Stated requirement |
| D-013 | C objects keep the platform's C data layout inside the C definition | C programs observe layout through memory |
| D-014 | `ridlcc` equals the platform base standard plus a context register and an error register, with register assignments equal to LLVM `swiftcc` | One LLVM implementation serves KLF, rustc and swiftc; tools keep working |
| D-015 | RIDL supports public generics from 1.0 through descriptors, witness tables and builders; named instances are optional fast paths | Full support, small mechanism |
| D-016 | RIDL records are frozen; evolution is additive through new shapes, appended trait methods and `@Open` enums; versions are UUIDs stamped with `@Abi(version:)` | Matches the existing Kira ABI workflow |
| D-017 | `.ridl` is Kira declaration syntax without bodies; an opaque handle is a bodyless `struct`; `.klffi` is Kira's `@FFI` declaration syntax unchanged | No new syntax |
| D-018 | Exceptions and RTTI are off by default in KArc and fully specified when enabled; unwinding never crosses RIDL | UE5 defaults; bounded boundary semantics |
| D-019 | No coroutine instructions in KIR 1.0; async lowers to state machines in definitions | Keeps the kernel small without blocking any language |
| D-020 | Conformance is defined by corpora and oracles | Makes every frontend stage testable by agents |
| D-021 | `Number`'s 128-bit tagged encoding (27.5) is fixed by this specification | Needed for a complete profile; revisable by editing Kira's definition and std only |
