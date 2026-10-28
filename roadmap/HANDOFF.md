# Handoff

Spec: `KLF-RIDL.md`. Done: `IMPLEMENTED.md`. Decisions: `docs/decisions.md`.

## The Kira formatter is done

Committed with the framework's `format` work. It lays Kira out through the
shared token layout of `KLF-FORMAT.md` section 7: `../klf-kira`'s `include`
names `languages/format`, and its `definitions/syntax/format.klfd`
merges `+=`-style pairs and `#{...}` quote holes into one span before calling
`layoutTokens`. A format run sets `emit` to `format`, so Kira's `parse` reads
the file alone (`formatRun` in `definitions/package/loader.klfd`).

All 377 `.kira` files of `../klf-kira` were checked: the 365 that parse format
with no rejection and are stable when formatted again, and the other 12 are the
syntax corpus's intentionally broken files. `cargo test -p klf-driver --test
format` passes, `kira_formats_literals_inline_blocks_generics_and_quotes`
included.

Open question for the user: whether MSL should align wrapped parameters under
`(`, as Apple's style does.

## In flight: shader outs in KLFD, S10 to S12 (session KLF shader outs)

Nothing is uncommitted. S10 (741955f) and S11 (5a8d9b5) are committed;
see the S10 and S11 entries in `IMPLEMENTED.md` and D-359.

- Done and tested: the MSL, HLSL, GLSL and WGSL outs under
  `../klf-kira/languages/<lang>/definitions/out` share
  `../klf-kira/languages/text` (KLF-KSL.md 22.10). The Rust text backends are
  deleted, and only SPIR-V is still written in Rust (`klf-shader-spirv`).
  `knvm binstall` builds `ksl`, `kmsl`, `khlsl`, `kglsl` and `kwgsl`, each
  linking the other shader languages, and `klf dist` ships none of them.
  Every ecosystem KSL shader that builds, plus its MSL and KSL outputs,
  builds for all six targets, and the outputs pass `spirv-val` and
  `glslangValidator`.
- Build and test, from the framework and `../klf-kira` in turn:
  `knvm klf binstall`, then `knvm sinstall && knvm binstall`;
  `klf test ../klf-kira/languages/msl` (70 cases) and
  `klf test ../klf-kira/languages/ksl`. A one-off toolchain:
  `klf build languages/ksl --language languages/msl ... -o /tmp/ksl`, then
  `/tmp/ksl build X.ksl --target hlsl --emit source`. The target names are
  msl, hlsl, `glsl_430`, wgsl, spirv and ksl ("glsl" is not one).
- Left (S12, the ins): for each of hlsl, glsl and wgsl, step by step:
  1. Write a lexer and parser in `languages/<lang>/definitions`,
     modelled on `msl` (`syntax/`, `semantics/`, lowering to KSIR through
     the `klf.shader.*` builder services).
  2. Replace the `units` stub, which reports code 1, and add a diagnostic
     code for each refusal.
  3. Add `corpus/` with the outputs of the KSL corpus for that target, and
     check that each reads back and rebuilds for every target (KLF-KSL.md
     26.2, S12).
  4. Then drop the "out-only until S12" sentence from 22.10.
- Known approximation: for an unusual module-scope expression, the WGSL
  out's refusal text is "a module-scope expression {kind}", not Rust's
  Debug text.

## In flight: the driver CLI and property wrappers (uncommitted)

The goal is to give every generated driver a full command line, built on a
kernel parser modelled on Swift ArgumentParser, and to let a definition add its
own commands. Program-side CLIs (option B) wait on RIDL, so they are out of
scope here.

Nothing below is committed. It sits in the working tree next to two other
in-progress sessions: the formatter, and interrupt/cancellation in
klf-native-rt, klf-cranelift, klf-ide and klf-kernel-ide. When you commit,
stage only these paths. `crates/klf-frontend-rt/src/{lib.rs,editor.rs}` hold
hunks from both this work and the interrupt work, so split them by hunk.

### Done

- `crates/klf-args` (new, no dependencies). This is the parser
  (`parse.rs`), Swift-style help (`help.rs`), bash, zsh and fish completion
  (`complete.rs`), and a line codec for embedding a tree in frontend
  metadata (`codec.rs`).
  - Error codes: `KLF-DRV-0040` unknown option, `-0041` missing value,
    `-0042` invalid value, `-0043` missing argument, `-0044` unexpected
    argument.
  - It reserves `help`, `--version` and `--generate-completion-script`.
  - It supports a default subcommand, aliases, and "did you mean"
    suggestions.
  - 7 tests pass and clippy is clean.
- Property wrappers in KLF Kira, implemented in
  `klf-kira-lower/src/wrappers.rs`.
  - An `@PropertyWrapper` struct with `wrappedValue` and at most one generic
    is a template. Errors are `KLFK-E0220` and `-E0221`.
  - An annotated member `x` becomes the backing field `_x: W(T)`.
  - Member access goes through `structs::member`. Struct literals fill the
    wrapper in `lower/literals.rs`.
  - `tests/property_wrappers.rs` passes.
- Definition commands, in `klf-kira-lower/src/cli/`.
  - `mod.rs` holds the prelude: `NameSpecification`, and `Option`, `Flag` and
    `Argument` as property wrappers. It also strips `ParsableCommand` and
    `configuration`.
  - `spec.rs` reads `CommandConfiguration` and the wrapped fields into a
    `klf_args::Command`. Errors are `KLF-DEF-0050`.
  - `entry.rs` generates `__klf_command(command: Int) -> Int`, which reads
    values through `klf.cli.count`, `klf.cli.int` and `klf.cli.string_*`.
    They are lowered in `lower/cli.rs`.
  - The Language's own `@Flag`/`@Option` members become driver flags, still
    read through `session.flag`. `definition_metadata.rs` attaches the tree
    as `LanguageDescriptor.cli`.
- Kernel plumbing:
  - The four `klf.cli.*` services are in `kernel/services.manifest` and
    `klf-native-rt/src/service_abi.rs`, and registered by
    `klf-defsvc/src/command_values.rs`.
  - Trampoline selector 8 is `__klf_command`, in `klf-cranelift`
    `lower/entry.rs`.
  - On the VM, `phase_runner::run_command` and
    `LanguageDefinition::run_command` run commands.
  - `FrontendMetadata.cli` is encoded as a `cli` record.
- The driver tree, in `klf-defsvc/src/driver_cli.rs`.
  - Subcommands: `build` (the default), `run`, `check`, `format` (when the
    definition formats), `lsp`, a hidden `lsp-worker`, then the definition's
    own commands.
  - The Language options appear on each compile command.
  - It also provides `invocation()`, `parse_build_arguments()` and
    `validate()`. `klf build` calls `validate()` in `klf-driver/src/frontend.rs`.
  - `Session::validate_driver_flags` reserves the built-in option names.
- `klf-frontend-rt` now uses klf-args.
  - `arguments.rs` re-exports `Invocation`.
  - `command.rs` runs a definition command through selector 8.
  - `format.rs` builds its request from `Matches`.
  - `lib.rs` handles the shader path first, then applies compat when the first
    word is not a subcommand, then `dispatch()`. Usage errors exit 64. `build()`
    returns early for `check`.
  - `cargo check -p klf-frontend-rt` passes.
- `klf-driver/tests/definition_commands.rs`: the two VM tests pass. The native
  test has not been rerun since `lib.rs` was restored. Another session had
  overwritten it, and `editor.rs` line 205, with the pre-CLI code.

### Left to do

1. Run `cargo nextest run -p klf-driver --test definition_commands`, and fix
   the native test. It expects:
   - exit 105 and 220 from the two runs
   - `build (default)` in the help, and no `lsp-worker`
   - exit 64 with `KLF-DRV-0042`
   - `docs 0.2.0` from `--version`
   - `complete -F _docs docs` from the bash completion script
2. Move `klf-shader-driver/src/arguments.rs` and `klf-cli/src/main.rs`, which
   both parse by hand, onto klf-args.
3. In `../klf-kira/definitions/language.klfd`, turn the `test` DriverFlag into
   `@Flag(name: .ShortAndLong, help: ...) let test: Bool = false` and give
   klf-kira a real command, such as `test`, that runs the test functions.
4. Docs:
   - Write `roadmap/KLF-CLI.md`. It should cover:
     - wrappers and `@PropertyWrapper` rules
     - `ParsableCommand` and `CommandConfiguration` (`commandName`,
       `abstract`, `discussion`, `aliases`, `subcommands`,
       `defaultSubcommand`, `hidden`)
     - value types: Bool, Int, String, `[T]`, `Optional(T)`, and payload-free
       enums as kebab-case choices
     - the `__klf_command` entry, selector 8, and the `klf.cli.*` services
     - the driver tree
     - the error codes above
   - In KLF-RIDL.md, update §5.5 (subcommands, with `build` as the default),
     §6.2 (`@PropertyWrapper`) and §7 (`configuration`, and `@Flag`/`@Option`
     members).
   - Update `IMPLEMENTED.md` and `docs/decisions.md`.
5. Run clippy and fmt on the touched crates. Run scoped nextest on:
   - `klf-args`
   - `klf-kira-lower`
   - `klf-defsvc`
   - `klf-frontend-rt`
   - the `klf-driver` tests `definition_commands`, `definition_flags`,
     `native_frontend` and `format`

   Keep every file under 700 lines. `klf-args/src/parse.rs` is at 618.
6. Commit signed on main. Do it later, so that KArc gets GNU-style spellings
   (`-c`, `-o`, `-I`) through its `compatDriver`.

## In flight: an external RIDL API on KLF binaries (uncommitted, session external-api)

The goal is the one in the brief: a binary `klf build` produces — a compiler, a
shader compiler — should carry a RIDL export, so that a caller can compile a
shader through `ksl` or run an app through `klf-kira` by driving the built
binary instead of shelling out to its bespoke CLI. Nothing is implemented yet.
What follows is the reading of the tree that the design has to start from.

### What already exists

- `klf-ridl` (layer 4, D-127) is a complete RIDL *front end* in Rust: the type
  system of section 20, the layout engine of 21, `.ridl` parse and canonical
  emit, the section-24.2 mangler, the 24.3 metadata blob, the 24.4 freeze and
  apply, the 24.5 link checks, section-23 descriptors as byte images with named
  relocations, and Rust/Swift bindgen. It operates on `.ridl` text. There is no
  `.ridl` file anywhere in the tree.
- The one working native RIDL export path is `klf-cranelift`'s
  `emit_library_object_with_init(module, exports, init)`: each `(kir name,
  symbol)` pair becomes a public symbol. It emits with the *default* Cranelift
  convention, which `klf-frontend-rt/src/ridl_surface.rs` justifies by the
  register-level agreement of `ridlcc` and the platform convention for trivial
  scalars (D-130).
- `ridl_surface.rs` is the closest thing to automatic export derivation: from a
  KIR module it derives a `.ridl` of the scalar, non-failing, non-generic
  functions, and synthesizes forwarding functions named by the mangled RIDL
  symbol. It is wired to `--emit ridl`, `--emit lib` and `--emit shared` for a
  *program*, and to `share/<lang>/ridl/<lang>.ridl` for the *std*
  (`klf-driver/src/sysroot.rs` `install_surface`). A *driver* binary never
  goes through it.
- `Linkage::Exported` (D-328) plus `klf.kir.signature.export` give a KIR function
  a global symbol under its own plain name. That is a C entry point, not a RIDL
  name.
- KIR has `ExportRecord` (`export @sym as "ridl_name" : ty`), parsed and printed.
  It is inert: nothing constructs one, and no backend reads `ridl_name`. Only
  `klf-kir-passes/src/function_abi.rs` reads `export.symbol`, for name
  uniqueness.
- The native frontend binary exports four global symbols, verified with `nm` on
  a real `ksl` and a real `karc`: `main`, `klffe_main`, `klffe_metadata`,
  `klffe_phase`, plus the registry's `klffe_select`,
  `klffe_language_count` and `klffe_language_metadata` (D-357). Nothing else.
  No `_RIDL1…` symbol, no `.ridl`, no metadata section.

### The gap

1. No RIDL surface on a driver binary. The whole external contract of a
   built toolchain is `klffe_phase(selector, i64, i64, i64) -> i64` — a word
   oriented internal ABI, not RIDL.
2. No `.ridl` describing a driver. Nothing in the tree says what a caller may
   ask a toolchain to do.
3. No `.ridlmeta` section and no `klfrt_ridl_register`. `grep ridlmeta` over
   `crates/` returns nothing; the section-24.5 checks in `klf-ridl::meta` have
   no caller outside their own tests.
4. No loader. `dlopen` exists only in `klf-vm/src/foreign.rs`, for a user
   program's foreign calls. Nothing loads a driver binary.
5. A built driver is not loadable as it stands: `nm -D` on `ksl` lists no
   defined dynamic symbol, and `klf-driver/src/frontend.rs`'s `link()` passes no
   `-rdynamic` or `--export-dynamic`. `dlopen` + `dlsym` finds nothing today.
6. `ridlcc` is not reachable from this workspace. `klf-cranelift` depends on
   crates.io `cranelift 0.135.2`, sets no calling convention anywhere
   (`klf-cranelift/src/lower.rs` `signature` never touches it), and there is no
   `[patch]` in any manifest. RIDL is realized only on the LLVM path, as
   `swiftcc` (D-111).
7. D-126 stands: the `Language`/`Session` boundary is Rust-hosted, so a
   definition cannot contribute to the surface even in principle. Any surface
   here is a kernel-owned ABI over Rust, not a definition-owned one.
8. A `.klfd` definition cannot reach RIDL at all. The only native route a
   definition has is `call.foreign` through `.klffi`, which is the platform C
   ABI; RIDL is deliberately not that (section 19.2). So "a Kira CLI compiles a
   shader through `ksl`" needs either a new kernel service that loads and calls
   the toolchain, or a C-ABI-shaped surface that `.klffi` can already reach.

### The `extra_clobbers` lead, re-derived

The brief said `extra_clobbers` appears nowhere in this tree. That is right for
this repo, and the work is *not* missing — it is already done, in the sibling
`../cranelift-ridl` fork, on branch `ridl`, uncommitted:

- `cranelift/codegen/src/machinst/mod.rs` has the hook, empty by default.
- `cranelift/codegen/src/machinst/vcode.rs` unions it into the clobber set and
  stops counting an `args` pseudo-instruction's operand registers as writes, so
  the callee-saved RIDL context register is not treated as clobbered.
- `isa/aarch64/inst/mod.rs` implements it: a call whose callee is RIDL and whose
  caller is not adds only `RIDL_ERROR`; the two conventions otherwise compare by
  their base.
- `isa/x64/abi.rs` takes the other route: `get_regs_clobbered_by_call` adds the
  x64 error register for a RIDL callee directly, so x64 needs no hook.
- `cranelift/filetests/filetests/runtests/ridl.clif` already uses `iconst` plus
  `iadd`/`imul`; there is no `iadd_imm` or `imul_imm` anywhere in the fork.

That fork is **not** what this workspace builds against. Nothing in
`Cargo.toml`, `Cargo.lock` or `~/.cargo/config.toml` points at it. So the
`ridlcc` work is stranded until the workspace is patched onto the fork, which
is its own decision.

### Decisions taken (session external-api)

- **Reach: dynamic linking, option 3.** A caller `dlopen`s the driver
  executable and resolves its `_RIDL1…` symbols. `KLF-RIDL.md` 24.5 settles
  this: "RIDL modules link with the platform's ordinary static and dynamic
  linkers". 16.5 adds that shared linking is used when a dependency is a RIDL
  dynamic library "so that one runtime instance serves the process".
- **Module: one kernel-owned `.ridl`**, the same in every binary — a
  generated, FFI-style declaration file describing what a KLF toolchain
  binary exports. Not per-driver, and not the `Language` phases (that is
  D-126).
- **Convention: real `ridlcc`,** from the `../cranelift-ridl` fork, wired in
  with a `[patch.crates-io]`.

### Done and tested

- **The fork is complete and green, and its two `isa` filetests needed
  blessing.** `cargo build -p cranelift-tools --bin clif-util` in
  `../cranelift-ridl` succeeds. `clif-util test` over
  `filetests/isa/x64` (303 tests), `filetests/isa/aarch64`,
  `filetests/runtests` and `filetests/verifier` (418 tests) all pass.
  - `isa/x64/ridl.clif` and `isa/aarch64/ridl.clif` first failed on stale
    expected text, not on a wrong result. Both diffs are the same thing: the
    expected output still had the `stp`/`str x20` prologue and the matching
    `ldr`/`ldp` epilogue around a function taking a context register. The
    `vcode.rs` `args` fix — an incoming register that the `args` pseudo
    instruction defines but writes nothing is not a clobber — removes exactly
    that save and restore. Blessing produced the correct text. Both files are
    untracked in the fork, so this is new work, not a change to a committed
    expectation.
- **`[patch.crates-io]` is in the workspace `Cargo.toml`**, pointing
  `cranelift`, `cranelift-codegen`, `cranelift-module` and `cranelift-object`
  at `../cranelift-ridl/cranelift/{umbrella,codegen,module,object}`. Cargo
  resolves it and pulls the fork's `bforest`, `bitset`, `entity`, `isle`,
  `assembler-x64` and `srcgen` with it. `cargo check -p klf-cranelift`
  passes against the fork with **no source change**, which is the proof the
  patch is a drop-in.

### What the fork gives us, precisely

`CallConv::Ridl(RidlBase)` with `RidlBase::{SystemV, AppleAarch64,
WindowsFastcall}`, spelled `ridl_system_v`, `ridl_apple_aarch64` and
`ridl_windows_fastcall` in textual CLIF. Lowering lives in
`isa/x64/abi/ridl.rs` and `isa/aarch64/abi/ridl.rs`. Three
`ArgumentPurpose` markers carry the RIDL-specific parameters: `Context` for
the context register, `Error` for the error register in and out, and the
existing `StructReturn` for the indirect result. A signature therefore holds
only scalars and pointers; the frontend does the section-22.3 classification.

Register assignments, per `isa/call_conv.rs`'s own table and section 22.2:
context is x20 on aarch64, r13 on x86_64; error is x21 on aarch64, r12 on
x86_64. Four direct result registers, rax/rdx/rcx/r8 and xmm0 to xmm3 on
x86_64, x0 to x3 and v0 to v3 on aarch64.

### Left to do

1. Set `CallConv::Ridl` on a KIR function's Cranelift signature. Today
   `klf-cranelift/src/lower.rs`'s `signature` never touches the convention,
   so every function gets the target default. The KIR information it needs is
   already computed by `klf-kir-passes/src/function_abi.rs`'s
   `function_type`: `abi`, `parameters[..].context` and `failure`.
2. Give the driver binary a RIDL surface: the `.ridl` describing it, the
   `_RIDL1…` symbols emitted from `emit_frontend_object`, and the
   `.ridlmeta` blob plus `klfrt_ridl_register`.
3. Make the driver loadable: `--export-dynamic` (or `-rdynamic`) in
   `klf-driver/src/frontend.rs`'s `link()`, so the symbols reach `.dynsym`.
   Today `nm -D` on a built `ksl` lists nothing defined.
4. A caller that resolves those symbols. Nothing loads a driver today;
   `dlopen` exists only in `klf-vm/src/foreign.rs` for a program's foreign
   calls.
5. Sound the two-runtime boundary. RIDL is zero-copy same-address-space
   (19.4) and `klf-frontend-rt` declares a `#[global_allocator]`. A caller
   that loads a driver into its own process gets a second Rust runtime and a
   second allocator; a `String` or `[T]` allocated on one side and released on
   the other is wrong. Either the surface is restricted to values with no
   destructor, or the allocator is shared, or 16.5's "one runtime instance
   serves the process" has to be arranged deliberately.

## Toolchain model (authoritative — do not treat as hypothetical)

KIR is fixed by the spec. There is exactly **one** VM (`klf-vm`) and **one** set
of backends (Cranelift/LLVM), shared and **unspecialized**. Nothing per-language
exists below the frontend — no per-language VM, no per-language backend cases.

- A language's runtime (ARC, RIDL shims, everything it declares as runtime
  support) is **ordinary std code that lowers to spec KIR**. It is not VM
  opcodes and not backend special cases.
- `klf-vm` interpreting a `.klfd` to compile source is **bootstrap only**. It is
  never the compiler.
- `klf build <lang>.klfd` must **Cranelift-compile the definition's phases into a
  native frontend binary** (e.g. `karc`: `.cpp`/`.krc` → KIR). **That binary is
  the compiler.** One artifact — the native frontend.
- `klf-vm`'s permanent jobs: executing definition phases at build time,
  VM/hybrid modes for user KIR, hot module push, wasm, and being the parity
  oracle for the backends. Never rewritten in `.klfd` (D-001: kernel stays Rust,
  no bootstrap cycle).

Corrections to earlier notes: there is **no** per-language `<lang>-vm` artifact
and no "specialized VM" — that framing was wrong. `klf build` produces one
thing, the native frontend compiler; the language runtime is std, compiled by
that frontend to spec KIR and run on the one shared VM/backends. And the
definition running on `klf-vm` is the **bootstrap path**, not the product.

An earlier note here claimed the frontend emits **textual** KIR and so needs no
`klf.kir.*` builder-service bridge. That was wrong and is now moot: karc builds
its module through the kernel's handle services and produces no KIR text
anywhere, and the native frontend calls the same services. The bridge is not
avoided; it is shared (D-254).

## Status (exit tests, not "code exists")

- M0 done for its exit test (section 12 instructions, section 10.1 / A.4 module
  items, generics, the V1-V3 fixture matrix, all gates). M1 done. M2/M4/M5 pass with stand-ins. M3 done-ish.
- M6: the `.klffi`/`call.foreign` boundary, plus a C definition package
  (`../klf-c`, driver `kcc`) that compiles a tested subset of C. The exit
  test corpus is untouched.
  Post-M6 now has one executable `.klfd` runtime vertical; the full definition
  object and Language/Session service ABI remain incomplete. Typed metadata,
  source services, phase queries, VM Session-artifact services, typed
  `Session.flag`, and per-file `Session.file_mode`, `Session.file_memory`, and
  `Session.file_convention` services are implemented. Effective per-file
  memory/convention values resolve optional mode overrides against Language
  defaults on both VM-hosted and generated-native paths; they are exposed to
  `.klfd` code but not yet applied to KIR policies or function conventions.
  `Session.flag` uses bounded bulk buffer access shared by VM and native
  execution; the scoped driver integration test proves flag values and both
  mode overrides and package-default fallback reach `.klfd` on both paths.
- Gates green: `cargo nextest run --workspace`, `cargo clippy --all-targets` (0),
  `cargo fmt --check` (clean repo-wide), wasm build of
  `klf-vm`/`klf-rt`/`klf-layout`.
- `.config/nextest.toml` puts every test that links a native program in one
  single-threaded group (D-270). They share the `klf-native-rt` archive cargo
  replaces in place; running them in parallel failed a different test each run.
- Gates pass on both aarch64 macOS and aarch64 Linux hosts. The provisioned
  LLVM is found under `~/.kira/toolchains/llvm/<version>/<host key>`
  (`klf_target::llvm_host_key`). Every test in these crates that links and then
  runs a program builds it for `klf_target::host_triple()`, whatever triple the
  KIR fixture declares — that is a property of the test suite's fixtures, not a
  limit on the toolchain: `CToolchain::for_target` in
  `crates/klf-frontend-rt/src/toolchain.rs` takes `--target=<triple>` for any
  target, an Apple SDK by `xcrun`, and `KLF_SYSROOT_<TRIPLE>` else
  `KLF_SYSROOT` for a non-host target's C library, so `klf build --target
  aarch64-unknown-linux-gnu` on an x86_64 host emits an ARM aarch64 ELF. Tests
  of the macOS cross-target sysroot matrix skip on other hosts.

## Shader pipeline (KLF-KSL.md)

- S0 to S7 done on Linux; see `IMPLEMENTED.md` "KLF-KSL". S8 (hardware
  parity) needs Metal, D3D12 and GPUs, and is not started. Nothing here has
  been through Apple's `metal`.
- The shader definitions live in `../klf-kira/languages/` (DS-020): `ksl`,
  `msl`, `hlsl`, `glsl` and `wgsl`, each a language its own toolchain.
  Build one with `klf build languages/msl --language languages/ksl -o
  /tmp/kmsl`; run its corpus with `klf test languages/msl`, which reads the
  languages it needs from the corpus manifest.
- In progress: S9 to S12 (outs written in KLF Kira, one linked toolchain
  binary); see `KLF-KSL.md` 5.8, 5.9, 6.7, 22.10 and 26.2. S9 to S11 are
  done: every text target is written by a KLFD out (`kmsl x.metal --target
  ksl --emit source`), only SPIR-V is still written in Rust, and each
  shader language's toolchain links the other four. Next, S12: the HLSL,
  GLSL and WGSL ins (their packages refuse every source with code 1).
- `klf-vm`'s `decoded_instruction_is_one_word` and
  `floating_arithmetic_is_predecoded_*` fail on Linux (instruction size 12,
  not 8), and most `klf-cranelift` tests fail to link (`clang link failed
  for add`); both predate the shader work.
- Shaders have no VM path (DS-017).
- Validators used: `spirv-val`, naga's CLI for WGSL, `glslangValidator` for
  GLSL and for HLSL (`-D -V -e <entry> -S <stage>`).
- Decisions D-331 to D-359 cover the shader work.

## Must-know

- Hosted languages are `.klfd` in KLF Kira, NOT Rust (spec 122, 468). Only KLF
  Kira's frontend is Rust. A Rust `klf-c` was built and removed for this.
- Root debt = D-126: the full `Language`/`Session` definition boundary is still
  Rust-hosted. The service bridge, typed metadata/source surface, and a
  fixed-expression `.klfd` runtime vertical exist, but they do not yet load a
  definition object or expose the complete compiler services needed by a real
  language implementation.
- Reference: `kira` 1.9.1 at `../kira` (spec's oracle; `tests-kik`, `examples`).
  D-139 permits reusing its code/assets. Different IR, so algorithms + corpus,
  not drop-in.

## Critical path to real definitions

phase lowering → Session artifact services → KIR builder services →
`.klfd` execution → full Language/Session service ABI → real `calc.klfd` →
`c.klfd` (M6).

## Native frontend build (`klf build <package>` → native `<lang>`)

Done. `klf build <definition-package-directory>` links a native compiler driver
for the language the package declares; `../karc` builds to a `karc` binary that
takes `.cpp` and writes `.kir`.

How it fits together:

- `klf-defsvc` (D-254) owns the definition-facing services and the phase
  schedule. It has no backend dependency: handlers go into a `ServiceSink`, the
  VM copies that into `Capabilities`, the native runtime installs it in a
  thread-local table its `klfsvc_*` shims read. One implementation of the
  `klf.kir.*` builder serves both.
- `klf_cranelift::emit_frontend_object` (D-255) compiles the phase KIR and adds
  `main` (globals prologue, then the runtime's driver), `klffe_phase` (phase
  index → phase function) and `klffe_metadata` (the descriptor blob).
- `klf-frontend-rt` holds the driver loop: argv, read the inputs, build a
  `Session`, run `klf_defsvc::drive`, write the KIR. It links `klf-native-rt`,
  and only a frontend links it (D-257).
- `klf-driver/src/frontend.rs` is the build step: load the package, read the
  descriptor, compute the per-phase artifact flags (D-256), emit, link with the
  provisioned clang.

Proof: `crates/klf-driver/tests/native_frontend.rs` (mini) and
`karc_native_frontend.rs` (karc over all seven examples) assert the native
frontend's KIR is byte-identical to the VM-hosted driver's, passes the raw
verifier and runs to the same result. The VM stays the oracle.

What is not done:

- P2 (std seam): done for KArc (D-293..D-297). `klf build ../karc --out <dir>`
  compiles `../karc/std` with the new `karc`; programs link `libstd.a` natively
  and have `std.klbc` linked into their bytecode. `../klf-c` has no std yet.
- A native frontend runs the per-unit heap-balance check against the block
  count live when the unit opened, not against zero (D-260, supersedes D-258).
- `--emit` has one mode, `kir`. A frontend cannot yet emit an object or link a
  program; that is the driver surface of section 5.2 beyond the frontend itself.
- The frontend is built for the host only. The recorded target comes from the
  build and is what the emitted KIR is stamped with; cross-compiling the
  frontend binary itself is not wired.

## This session (tested, VM + both native backends)

New crate `klf-layout` (layer 1: KIR size/align/offsets).
- Service bridge (D-140): `Capabilities` + `run_with`; `kernel_services`
  (`klf.io`); `klfsvc_*` in `klf-native-rt`; `kernel/services.manifest`. KLF
  Kira `putc`.
- Aggregates (D-141): KIR `field.addr`/`elem.addr`; `stack.alloc` sizes via
  `klf-layout`.
- KLF Kira structs (D-141): decl, literal, field read/write, by-ref params.
- KIR/KLF Kira enums (D-142): enum declarations, tag/payload operations,
  `switch.enum`, enum literals and exhaustive scalar-payload `match`; VM,
  Cranelift and LLVM parity tests pass.
- KLF Kira arrays/strings (D-143): fixed integer array literals, indexed reads
  and writes, plus a builtin four-field `String` literal with length, capacity,
  data and byte indexing. VM coverage and native string execution pass;
  growable arrays and string operations remain deferred.
- KLF Kira generics (D-144): generic function declarations lower to KIR generic
  parameters; calls infer concrete arguments, including aggregate addresses,
  and the driver monomorphizes them before VM or native execution.
- KLF Kira floats and `for` (D-147): `F64`/`F32` types, float literals, float
  arithmetic/comparison/unary-neg, compound float assignment, and int↔float and
  float↔float `as` casts; `for x in array` over fixed-size scalar arrays with
  `break`/`continue`. VM end-to-end coverage; float KIR ops already had native
  parity.
- KLF Kira aggregate returns and String `==` (D-148): a function whose declared
  return type is a concrete struct/enum returns by an sret out-pointer (caller
  allocates, callee field-copies into it, call site yields the destination);
  generic `-> T` still returns its address by value. `==`/`!=` on the builtin
  `String` lower to a length check plus an inline byte-compare loop.
- KLF Kira growable arrays and karc frontend lowering (D-149): the full karc
  frontend — `../karc/frontend/frontend.klfd` + `lowering.klfd`, 227 functions —
  now **lowers to KIR with zero frontend diagnostics** and round-trips through
  the KIR parser. Delivered:
  - New KIR `heap.alloc.array <element>, %count` op (`HeapAllocArray`), a
    byte-sized heap allocation seam, wired through printer/parser/analysis/
    monomorphize/verify, Cranelift, LLVM, and the VM (runtime-sized `HallocR`).
  - Growable `[T]` as a `{count, cap, data}` header: literals (empty `[]` typed
    from context), `.count`, `.append` with geometric growth, indexed
    read/write of scalar and aggregate elements, `for` over lists, list params
    and returns. Array annotations/literals now lower to lists; fixed arrays
    remain only for the String buffer.
  - The five host text fns (`utf8Count`, `utf8Byte`, `utf8Slice`, `joinText`,
    `decimalText`) lowered inline over `String`.
  - Reassignable aggregate `var`s (String/list/struct) via mutable
    address-holding slots; contextual empty-list typing at let/field/arg/assign.
- KLF Kira karc runs end-to-end on the VM (D-150): the real karc frontend now
  **compiles `examples/fibonacci/app.cpp` to ~1263 bytes of textual KIR on the
  VM** (gated test `karc_execution.rs`, skipped when `../karc` is absent). Fixes:
  string-literal bytes are heap-allocated (they were stack-allocated and dangled
  once the lexer returned — every token string was garbage); growable-array and
  string buffers use an untracked heap allocation (`alloc_untracked` / `HallocR`)
  that neither frees at frame exit nor trips the heap-balance check, so escaping
  lists/strings are safe; and the VM treats `i1` as a byte lane so `Bool` locals
  and fields work.
  - The karc KIR **verifies clean** (V1–V3, 0 diagnostics) and karc **compiles
    fibonacci to 1263 bytes of KIR on BOTH the VM and the native Cranelift
    backend, byte-for-byte identical** (`karc_execution.rs`, `karc_native.rs`,
    both gated on `../karc`). The emitted KIR is asserted **byte-for-byte equal
    to karc's reference `examples/fibonacci/app.kir`**, and that KIR then parses
    and runs `smallcpp_main` to **fibonacci(10) = 55** — the full chain C++
    source → karc → KIR text → running program, matching the reference compiler. The last native divergence was a backend bug: a
    `Bool` (`i1`) load/store computed `1/8 = 0` bytes, so a `Bool` local written
    in a loop never persisted natively (karc's `var mainFound`). Fixed by
    rounding the load/store byte count up (`scalar_bits.div_ceil(8)`) in both
    the Cranelift and LLVM backends.
  - **Remaining follow-ups (none block execution):**
    1. Typed reference-counting for buffers. Untracked allocation leaks until the
       heap/process exits — fine for a run-once frontend and the VM oracle, but
       proper ARC (retain on aggregate copy, release on scope exit / reassign /
       append-grow, recursive destroy) is still wanted for long-running programs.
       The runtime counts by address (`klfrt_retain/release`).
    2. `klf build karc.klfd` → native `karc` binary (Native frontend build P1):
       the definition KIR now verifies and runs, so the entry-wrapper +
       native-host-symbol machinery is the remaining piece.
    3. Broadening the C++ subset karc accepts is karc-side work in
       `frontend.klfd`/`lowering.klfd`, not a framework limitation.
  - karc's C++ subset has since grown far past fibonacci (all karc-side, zero
    framework changes), covering: `int`/`char`/`bool`/`long` + fixed arrays +
    array/scalar pointers + `void`; `if`/`else`/`while`/`for`/`do`-`while`/
    `switch`(fall-through)/`break`/`continue`/early-`return`; the full operator
    set (arithmetic, comparison, short-circuit `&&`/`||`, `!`, ternary, compound
    assign, `++`/`--`, unary `-`); decimal/hex/char literals; typed params
    (scalar/array/pointer, pass-by-reference), expression statements, `const`.
    Proven by compiling+running bubble sort, binary search, strlen, swap, gcd,
    isPrime over ~40 gated tests (`karc_execution.rs`). Each feature reuses
    existing `.klfd` capabilities — the strongest evidence the framework side is
    done.
  - **Global variables: done.** KIR modules carry a `globals` section
    (`global @name : ty = zero|<int>`, optional `const`), `global.addr` resolves
    to the typed address, and all three engines realize it: the VM allocates each
    global once at start (untracked heap, out of the balance check) and resolves
    the address through a `GlobalA` opcode; Cranelift and LLVM heap-allocate the
    storage in the `main` prologue via `klfrt_global_alloc` and load the address
    from a per-global pointer slot, so runtime load/store bounds checks pass on
    native code exactly as on the VM. karc parses file-scope variables (scalar
    with optional initializer, or a fixed array) and emits these, reusing its
    slot machinery so indexing, compound assignment and pass-by-reference work
    unchanged; globals persist across calls and are shadowed by same-named
    locals. Covered VM + native + karc end-to-end (366 workspace tests green).
  - **karc now compiles libc-style code.** Since globals landed, karc gained
    (all karc-side, plus one framework addition noted below): `sizeof(type)`,
    comma-separated and uninitialized (zero-init) locals, multi-dimensional
    arrays, the bitwise/shift operators `& | ^ ~ << >>` and their compound
    assignments, structs (scalar/array fields, `v.field`), string literals as
    pointer arguments, and `typedef`. Proven by two combined mini-libc
    translation units (`karc_execution.rs`) — strlen/strcpy/strcmp/reverse/
    isdigit/toupper/atoi plus a unit mixing typedefs, string literals and
    structs — that compile and run on the VM, and the first also runs on the
    Cranelift native backend (`karc_native.rs`).
  - **One framework addition this round:** byte-string global initializers
    (`GlobalInit::Bytes`, `global @s : [N x i8] = "..."`), so string literals
    lower to byte-initialized globals; realized on the VM and both native
    backends (chunked stores in the `main` prologue).
  - **karc grew a broad C subset and a clean emission architecture.** Beyond the
    above it now has: first-class pointers (pointer locals incl. null, `char*`
    returns, array-name decay, pointer arithmetic `p±i`/`p-q`, address compares,
    string literals as `char*` values), C casts `(type)e`, `void*` (byte
    pointer), structs with scalar/array fields (`v.field`, `v.field[i]`) and
    struct-pointer parameters (`p->field`), enums, `typedef` (incl. pointer and
    struct aliases), storage/cv qualifiers, `sizeof`, multi-var and
    zero-initialized locals, nested arrays, and the bitwise/shift operators and
    their compound assignments. Proven by two mini-libc units plus a
    comprehensive libc translation unit (`karc_execution.rs`) and native runs
    (`karc_native.rs`).
  - **Emission is now a Kira builder.** `../karc/frontend/kir.klfd` holds typed
    KIR instruction constructors (an IRBuilder in Kira); lowering never
    hand-writes KIR text. The execution and native harnesses load `kir.klfd`
    alongside `frontend.klfd`/`lowering.klfd`.
  - **Unsigned integers are correct.** karc carries signedness as u8/u16/u32/u64
    type tags (normalized to KIR widths by the builder), zero-extends unsigned
    values on use, and selects wrapping add/sub/mul, udiv/urem/lshr and unsigned
    ordering compares for unsigned operands — so `unsigned char` buffers and
    unsigned math behave as C requires. There are also strings/bytebuf/libc
    examples in `../karc/examples/`, each with a byte-exact `app.kir`.
  - **More landed since:** `p->field[i]` (arrow to an array field), 64-bit
    integer literals (a literal beyond i32 lowers as i64), a type-preserving
    ternary (pointer- and long-valued `?:`), and aggregate initializers for
    global arrays (`int t[] = {..}`, byte-initialized). 54 VM + 4 native karc
    tests green; fibonacci stays byte-exact.
  - **Function pointers landed (framework + karc).** KIR gained a `call.indirect`
    op and types `func.ref` as a function pointer; the VM (dynamic callee index)
    and both native backends (Cranelift `call_indirect`, LLVM call through the
    pointer) lower it. karc supports `typedef RET (*NAME)(PARAMS);` parameters:
    a function name decays to `func.ref`, and calling the parameter lowers to
    `call.indirect`. Covered on the VM and native (`apply(add/mul) == 54`).
  - **Also landed:** whole-struct assignment (`p2 = p1;`), aggregate initializers
    for global arrays, `p->field[i]`, 64-bit literals, type-preserving ternary.
  - **Struct value returns landed.** A struct-returning function lowers with an
    sret out-pointer (hidden first parameter, unit KIR result) and copies the
    returned struct's fields into it; `struct P p = make(..)` passes the local's
    address as the out-pointer. Covered on the VM and native (`divmod` → 32).
  - **Both hallmark features are now done** (function pointers, struct value
    returns), each on the VM and native. Remaining are smaller or rarer: struct
    value *parameters*, nested structs, `goto`, the preprocessor, local array
    aggregate initializers and function-pointer locals/returns. A structural
    KIR-builder kernel service (so karc skips the text round-trip entirely) is
    the architectural follow-up.
- KLF Kira definition vertical (D-145, D-146): integer `as` casts, void service
  calls, and field-wise copies for nested aggregate values allow
  `definitions/calc.klfd` to parse a supplied arithmetic expression, mutate a
  parser struct, call `puti`, and return its result on the VM and native
  backend. This is executable KLF Kira source, not yet the full `Language`
  object ABI or a user-input compiler.
- Typed Language/Session surface (D-147): `LanguageDescriptor` carries the
  section-7 metadata, `Session` exposes registered files, positions, symbol
  interning and text lookup, and CLI definition selection uses that metadata.
  The `.klfd` corpus is loaded through the descriptor-selected KLF Kira
  definition.
- Typed source service (D-148): `source_len` and `source_byte` cross the
  `call.foreign` bridge through VM capabilities and `klf-native-rt`; the driver
  exposes the same buffer with `run_with_source`, and `calc.klfd` now parses
  supplied input on both backends. The coarse buffer/pointer ABI remains
  pending; diagnostic registration, transport and Session resolution now exist.
- Definition package and diagnostics (D-149, D-150): `DefinitionPackage` and
  `DefinitionSource` are the first package phase boundary; the generic default
  still accepts one source while KLF Kira groups a package into one unit.
  `diag_error(code, start, end)` records a source range through the VM
  capability and native runtime paths. Runtime diagnostic registration,
  transport and resolution now exist; the full definition object ABI remains
  deferred.
- Definition units (D-151): `DefinitionUnit` is a `FileId`-backed phase handle;
  `Definition::units` registers package sources once and `compile_package`
  invokes `compile_unit`. KLF Kira and the calculator consume that registered
  file directly, so the package path no longer re-registers its source.
- Typed diagnostics (D-152): `Diagnostic` carries a primary `Span` and typed
  note spans, existing labels promote their first span to primary, and
  `Session::report_error` exposes the kernel-side path. CLI rendering uses the
  primary span while preserving the existing labels.
- KLF Kira phase artifacts (D-153): the definition path now calls
  `parse_unit`, `elaborate_file` and `lower_elaborated` in order. The typed
  `ElaboratedFile` owns resolved struct, enum and signature tables; the public
  `lower_file` helper remains as the direct parsed-file compatibility entry.
- Generic phase boundary (D-154): `DefinitionArtifact` carries a
  `DefinitionUnit` plus a language-owned `Any` payload, and `Definition`
  exposes opt-in `parse`, `elaborate` and `lower` queries. KLF Kira and Calc
  use the boundary; legacy definitions still use `compile_unit`.
- Session-owned artifacts (D-155): `Session` stores immutable phase payloads
  behind `ArtifactId`; `DefinitionArtifact` carries the unit and handle, and
  definitions retrieve payloads through the active session. Artifacts cannot
  outlive their session storage.
- Query cache (D-156): parse and elaborate artifacts are cached by language,
  unit and phase; `Session::invalidate_queries` clears those handles, and KLF
  Kira has a regression test for cache hits. Source files are immutable in this
  slice, so invalidation is explicit.
- Cross-build query cache (D-370, section 5.6): a native frontend answers the
  `lower` query of its units from `$KLF_CACHE/queries` when nothing the
  phases read has changed. `klf_defsvc::drive_cached` runs `units`, asks a
  `ModuleCache`, and on a miss runs the pipeline and stores the verified module
  or the phase rejection. The phases' file-system reads, existence probes and
  directory listings are recorded as `Session::observations`; an entry keeps
  their SHA-256 hashes and is valid only while each still holds (red-green
  validation of 8.9). Keys hash the KLF release, the definition hash
  (`FrontendMetadata::definition`), the frontend binary, target, emit, flags,
  file modes and every input source. A hit still verifies the raw KIR. The
  module is stored as `.kirb` (`klf_kir::kirb`): postcard over serde-derived
  KIR, split into parts with their own name tables so the parts encode and
  decode on all cores. Entries live in one directory per (profile, unit files)
  slot and a new entry replaces the others of its slot. AppKit `kira check`:
  about 8.6 s cold, 1.2 to 1.5 s on a hit. Per-file `parse` sub-queries are
  ahead: they need `Cacheable` encoders for the Kira tree, and the spec does
  not define `Encoder`/`Decoder` yet.
- KLF Kira package assembly (D-157): KLF Kira overrides `units` to register all
  package sources and return one grouped unit; `parse_unit` combines their
  imports and declarations in source order. A helper in one `.klfd` file can
  now be called from `main` in another. Calc retains the one-source boundary.
- Assembled namespace validation (D-158): KLF Kira rejects duplicate named
  top-level declarations across grouped sources with `KLFK-E0200`, pointing at
  the later identifier and retaining a typed note for the first declaration.
  Struct and enum KIR type declarations retain their original source spans.
- Assembled name and import resolution (D-159): functions and nominal types are
  visible across the grouped package namespace; `import KLF` is accepted and
  other import paths receive a source diagnostic until external module loading
  exists.
- Session scopes and import edges (D-160): `Session` exposes typed scope and
  entity handles, namespace-aware declarations, parent lookup, imported
  overload sets, transitive import edges and cycle-safe traversal.
- Session arenas (D-161): `Session::arena<T>` owns reusable named typed arenas;
  `Arena::alloc`, `get`, `set` and `count` use type-checked `Id<T>` handles.
- Session types and layouts (D-162): `Session` owns typed `TypeId` records for
  primitive/composite/function/nominal shapes, KIR policy records and nominal
  layouts. Opaque nominals can be completed, and target-aware size/alignment
  queries handle recursive aggregates with cycle detection.
- Session solver and speculation (D-163): `Session` owns trait requirements,
  parent-trait conformances and witness records. Its solver creates typed
  variables, unifies them, checks conformances, applies definition-supplied
  conversions and chooses the lowest-cost disjunction within a search limit.
  `Session::speculate` snapshots session services and transactionally rolls back
  failed nested operations, including arena allocation and replacement.
- KLF Kira typed elaboration handles (D-164): elaboration now creates session
  `TypeId`s for Kira primitives, arrays, addresses, nominals and function
  signatures. Struct and enum handles are completed with session layouts and
  policies; lowering checks registered function handles and validates call
  arguments through the session solver after language-level generic inference.
- KLF Kira generic bounds (D-165): trait declarations register requirements and
  parent edges in `Session`; struct, enum and `extend` conformances register
  typed witness records. Generic function bounds resolve to `TraitId`s, and
  calls run solver conformance checks after inferring each generic argument.
- Language queries and KIR witnesses (D-166): `Session` records parse,
  elaborate and lower query dependencies on source files and child queries;
  invalidating one file propagates through the dependency graph. Registered
  Kira conformances now emit typed KIR witness declarations with text
  round-tripping, while the runtime witness-table ABI remains ahead.
- Definition query and service parity (D-167): Calc now records the same
  parse/elaborate/lower dependencies as KLF Kira, and `klf-services` parses
  `kernel/services.manifest` into typed integer, float, pointer and void
  signatures with duplicate-name and duplicate-symbol checks. Runtime service
  registration on the VM is checked against the manifest; Session-owned
  manifest data now drives prelude wrapper enumeration and declarations.
- Mutable inputs and native service coverage (D-168): `Session::replace_file`
  rebuilds a registered source and invalidates its dependent query cache.
  Native linking now smoke-tests manifest I/O and diagnostic symbols, while
  the existing `.klfd` corpus covers source and integer-output symbols.
- Session-owned service registry (D-169): `Session` parses the kernel service
  manifest once and exposes typed lookup to KLF Kira. Wrapper symbol lookup,
  foreign declarations and service-call width adaptation now use the active
  session registry instead of a frontend-local manifest cache.
- Session input lookup (D-170): `Session::input(path)` returns the current
  registered source text by name, completing the direct source-read primitive
  used by the query contract alongside file handles and replacement
  invalidation.
- Unified compile entry (D-171): KLF Kira and Calc route their public direct
  `compile` methods through the package phase boundary, so CLI and embedding
  callers record the same parse/elaborate/lower queries as explicit package
  callers.
- Runtime diagnostic registry (D-172): `Session` stores typed numeric
  `DiagnosticSpec` entries with duplicate rejection and speculation rollback,
  providing the kernel-side registry for the existing `diag_error` service.
- Runtime diagnostic transport (D-173): VM and native runs retain
  `diag_error` calls as typed `RuntimeDiagnostic` records, and the driver
  exposes them alongside the existing `Outcome` without changing callers that
  only need the outcome. Session spec resolution and manifest-driven wrappers
  are covered by D-174.
- Executable diagnostic boundary and prelude generation (D-174): `Session`
  resolves runtime records atomically through registered specs, the driver can
  report them into a session, and KLF Kira enumerates its service wrappers from
  the typed manifest while preserving the language-level facade names.
- Definition metadata and CLI diagnostics (D-175): language descriptors carry
  definition-owned `DiagnosticSpec` records, compilation registers them in the
  Session, and `klf run --input` retains the runtime input file and renders
  resolved diagnostic title, explanation and span.
- Definition registry and CLI selection (D-176): `klf-services` owns a typed
  registry of `Definition` objects, validates descriptor names and extension
  collisions, and the CLI selects its built-in definitions through that
  registry. The executable `.klfd` loader remains the next boundary.
- Language construct metadata (D-177): KLF Kira extracts the required
  section-7 `Language` metadata fields from a parsed `construct`, validates its
  shape and enum/string/array values, and reports definition diagnostics. This
  is metadata loading only; phase entrypoints still need an executable image.
- Language phase contract (D-178): the metadata loader records the spans of
  the four required `units`, `parse`, `elaborate` and `lower` method bodies and
  rejects missing, duplicate or declaration-only entrypoints. Their typed
  invocation ABI remains the next step.
- Definition image envelope (D-179): `klf-services` now validates a compiled
  module as a `DefinitionImage` carrying its descriptor and exactly one symbol
  for each Language phase. Incomplete images fail before invocation.
- Language phase signatures (D-180): the KLF Kira metadata loader validates
  the required phase parameter/result types and rejects generic, bodyless or
  mismatched entrypoints before image binding.
- Definition image invocation (D-181): `klf-driver` runs a validated image
  phase through the normal VM/native backend path using the bootstrap integer
  handle argument transport.
- Executable phase bodies (D-182): `DefinitionImage` rejects phase symbols
  that are foreign imports or bodyless declarations, so image invocation can
  only reach executable KIR.
- Source-owned Calc metadata (D-183): `definitions/calc.klfd` now carries the
  required Language metadata construct, and the Rust Calc parity definition
  loads that descriptor from the parsed source once while retaining its phase
  implementation.
- Dynamic definition identity (D-184): Definition names/extensions may be
  owned runtime values and Session query keys/records store owned language
  identifiers, removing the static-lifetime blocker for loaded definitions.
- Executable Definition adapter (D-185): `ExecutableDefinition` exposes a
  validated `DefinitionImage` through the registry surface and reports typed
  `KLF-DEF-0005` until Session-handle marshalling replaces the bootstrap
  compile leaf. The image boundary now lives in its own size-bounded module.
- Typed Session handles (D-186): `Session` owns a unique session handle and
  session-local `UnitId` records, while phase arguments carry registered file,
  unit and artifact handles before the driver converts them to backend words.
  Invalid or cross-session handles fail before execution.
- Typed phase results (D-187): phase return words resolve through the same
  Session into unit lists, language artifacts or KIR modules; failed, missing
  and wrong-kind returns are rejected before the caller sees a compiler value.
- Executable phase executor (D-188): `ExecutableDefinition` can run a supplied
  typed executor through units, parse, elaborate and lower, enforcing one
  registered unit and matching artifact ownership; unconfigured images retain
  the explicit `KLF-DEF-0005` diagnostic.
- Image registry bridge (D-189): `klf-driver` registers a compiled
  `DefinitionImage` as an `ExecutableDefinition` with a selected VM/native
  `BackendDefinitionExecutor`, so image selection no longer needs a Rust
  frontend object.
- Session artifact ownership (D-190): `DefinitionArtifact` records its
  originating `SessionHandle`; downcasts and phase-argument marshalling reject
  same-number artifact IDs created by another Session.
- Backend phase proof (D-191): a VM-backed `BackendDefinitionExecutor` now
  drives all four typed phase calls end to end when the image returns handles
  for Session-owned values; the remaining work is making KLF Kira bodies create
  those values through kernel services.
- Kira phase lowering and VM artifact services (D-192): construct methods for
  the four Language phases lower to executable KIR with distinct kernel handle
  types. The VM bridge validates the active Session/unit, reserves Tree and
  Program artifact slots, and replaces them with payloads emitted by
  `definition_tree` and `definition_program`; native definition-service symbols
  remain an invalid sentinel until native gets an equivalent Session bridge.
- KIR builder service slice (D-193): KLF Kira phase bodies can create a
  Session-owned module builder, add a source-named i64 function with a return
  constant, and finish it into a real `KirModule`. The VM bridge owns those
  builders and replaces the reserved module slot; the source-level proof
  compiles a Kira `Language` construct and runs its returned module. Native
  builder symbols remain invalid until native receives Session context.
- KIR value services (D-194): builder handles now carry typed `KirValue`
  identities. KLF Kira can emit i64 constants, add two values in one function,
  return the resulting value and finish the Session module; the source-level
  proof runs the computed result. Native value-builder symbols remain invalid
  until native receives Session context.
- KIR function and block services (D-195): the VM builder can create a
  parameterized i64 function, expose its parameter as a `KirValue`, create and
  switch to a block, branch to it and return a computed value. The source-level
  proof runs the branched function with an argument; native parameter/block
  symbols remain invalid until native receives Session context.
- KIR call service (D-196): the VM builder can call one builder-created i64
  function from another, checking that the value belongs to the caller and
  both functions belong to the same module. The source-level proof builds and
  runs `main -> helper`; native call-builder symbols remain invalid until
  native receives Session context.
- KIR aggregate value services (D-197): the VM builder can construct a checked
  two-slot i64 aggregate as a real stack array, store two caller-owned values,
  and load either field back as a `KirValue`. The source-level proof uses the
  aggregate in the returned module; native aggregate-builder symbols remain
  invalid until native receives Session context.
- KIR arithmetic and conditional services (D-198): the VM builder emits i64
  subtraction and multiplication, tracks equality results as condition values,
  and emits conditional branches to checked blocks. The source-level proof
  runs the true branch; the definition image keeps the word-backed handle ABI
  so the VM width-1 limitation is not crossed during phase execution.
- KIR extended arithmetic services (D-199): the VM builder emits signed i64
  division, remainder and less-than comparisons through Session-owned values.
  The source-level proof executes division and remainder in a helper and uses
  less-than for the real conditional branch; native symbols remain invalid
  until native receives Session context.
- KIR bitwise and comparison services (D-200): the VM builder emits i64
  bitwise and/or/xor, left shift and logical/arithmetic right shifts, plus the
  remaining signed inequality predicates. The source-level proof executes the
  shift chain and branches through `<=`; native symbols remain invalid until
  native receives Session context.
- KIR variable-length aggregate services (D-201): the VM builder can allocate
  a Session-owned i64 array, initialize each checked slot once and load only
  initialized elements. The source-level proof uses a three-slot array while
  retaining the fixed pair path; native symbols remain invalid until native
  receives Session context.
- KIR block-argument services (D-202): the VM builder can create a one-parameter
  i64 block, expose its parameter and branch with one caller-owned value. The
  source-level proof routes both conditional arms into a parameterized merge
  block; native symbols remain invalid until native receives Session context.
- KIR i32 builder services (D-203): the VM builder can create an i32 function,
  validate i32 parameter access, emit range-checked i32 constants and add them.
  The source-level proof materializes a one-parameter i32 function; native
  symbols remain invalid until native receives Session context.
- KIR integer conversion services (D-204): the VM builder emits checked ABI
  services for i32-to-i64 sign extension and i64-to-i32 truncation. The source
  proof executes both directions in the i64 result path; native symbols remain
  invalid until native receives Session context.
- KIR complete i32 arithmetic services (D-205): the VM builder exposes the
  i32 binary and signed comparison family already present for i64, including
  division, remainder, bitwise operations and shifts. The source proof builds
  an i32 conditional function; native symbols remain invalid until native
  receives Session context.
- KIR i32 aggregate services (D-206): the VM builder allocates typed i32
  arrays, checks their element kind, initializes each slot once and loads only
  initialized elements. The source proof returns a value loaded from a typed
  i32 array; native symbols remain invalid until native receives Session
  context.
- KIR typed i32 calls (D-207): the VM builder validates an i32 callee result
  and parameter width before emitting a call. The source proof builds an i32
  caller invoking the i32 function; native symbols remain invalid until native
  receives Session context.
- KIR two-argument calls (D-208): the VM builder validates two-parameter i64
  and i32 callees and emits calls with both caller-owned values. The source
  proof builds an i64 two-parameter callee and caller; native symbols remain
  invalid until native receives Session context.
- KIR integer switch service (D-209): the shared builder emits `SwitchInt`
  through a checked one-case-plus-default i64 service. The source proof uses
  the switch in the two-argument caller; native symbols remain invalid until
  native receives Session context.
- KIR conditional block-argument service (D-210): the VM builder can pass one
  i64 value into each parameterized conditional target after checking both
  blocks and the condition handle. The source proof returns the selected then
  or else block parameter; native symbols remain invalid until native receives
  Session context.
- KIR scalar memory services (D-211): the VM builder exposes typed `KirAddress`
  handles for i64 stack allocation, initializing store and load. Addresses are
  tied to their function and reject foreign, repeated-initialization or
  uninitialized access; the source proof stores and reloads a scalar in the
  returned module. Native symbols remain invalid until native receives Session
  context.
- KIR typed scalar memory services (D-212): i64 assignment and i32 stack,
  initializing-store, assignment and load services now share the typed address
  path. Value-width tracking rejects mismatched scalar stores; the source proof
  updates both widths before loading them. Native symbols remain invalid until
  native receives Session context.
- KIR i32 control-flow services (D-213): the VM builder exposes i32 block
  parameters, value-carrying branches, conditional branches and integer
  switches with width checks. The source proof uses i32 entry and join block
  arguments and an i32 switch; native symbols remain invalid until native
  receives Session context.
- KIR typed pair aggregates (D-214): the VM builder generalizes the checked
  two-slot aggregate service to i32 pairs, preserving element width through
  construction and indexed extraction. The source proof builds and reads an
  i32 pair; native symbols remain invalid until native receives Session
  context.
- KIR aggregate call contracts (D-215): the VM builder can create i64 and i32
  functions with checked two-slot aggregate parameters, expose those
  parameters as typed pair values and call them from another function. The
  source proof passes both i64 and i32 pairs through callees; native symbols
  remain invalid until native receives Session context.
- KIR enum type and value services (D-216): the VM builder can declare a
  no-payload enum, append named variants, construct an enum value and extract
  its tag as a typed integer. The source proof adds a two-case enum to the
  finished module and executes its tag; native symbols remain invalid until
  native receives Session context.
- KIR enum payload and dispatch services (D-217): the VM builder can append
  typed i64/i32 payload variants, construct and extract payload values, and
  emit a checked one-case enum switch with a default block. The source proof
  executes payload extraction and dispatches a payload-bearing variant; native
  symbols remain invalid until native receives Session context.
- KIR typed struct field services (D-218): the VM builder can declare a
  module-owned struct, append i64/i32 fields, allocate a struct value, store
  initialized fields and load them with field-width checks. The source proof
  constructs and reads an i64 field; native symbols remain invalid until
  native receives Session context.
- KIR multi-case enum dispatch (D-219): the VM builder can emit a checked
  two-case enum switch with a required default block, validating variant
  membership and same-function block ownership. The source proof executes the
  second case; native symbols remain invalid until native receives Session
  context.
- KIR nominal struct fields (D-220): the VM builder can declare an enum field
  in a struct, store an owning enum value into it and recover its address-backed
  enum value for tag operations. The source proof combines a scalar and enum
  field in one struct; native symbols remain invalid until native receives
  Session context.
- KIR struct aggregate calls (D-221): the VM builder can create an i64-returning
  function with one nominal struct parameter, expose that parameter as an
  initialized address-backed struct value and call it from another function.
  The source proof constructs a struct, passes it through the callee and uses
  the returned value; native symbols remain invalid until native receives
  Session context.
- KIR enum aggregate calls (D-222): the VM builder can create an i64-returning
  function with one address-backed enum parameter, expose that parameter as a
  typed enum value and call it from another function. The source proof passes
  a declared enum through the callee and consumes its tag; native symbols
  remain invalid until native receives Session context.
- KIR void function calls (D-223): the VM builder can create a parameterless
  void function, terminate it with a void return and call it from another
  function after checking the callee signature and module ownership. The
  source proof executes the void call; native symbols remain invalid until
  native receives Session context.
- KIR typed stack deallocation (D-224): the VM builder can deallocate an
  i64 or i32 stack address after checking its owning function and element
  type, then invalidates the address handle. The source proof deallocates a
  live scalar slot; native symbols remain invalid until native receives
  Session context.
- KIR boolean constants (D-225): the VM builder can emit a checked boolean
  constant from the definition ABI, recording the returned value as a boolean
  handle for conditional branches. The source proof selects its existing i64
  join with the constant; native symbols remain invalid until native receives
  Session context.
- KIR nominal enum-field copying (D-226): enum fields now copy a known
  constructed variant by emitting its scalar payload and `enum.inject` into the
  destination field instead of storing the source address as a word. The source
  proof makes the recovered tag part of the returned value; native symbols
  remain invalid until native receives Session context.
- KIR nested scalar struct fields (D-227): the VM builder can declare a
  module-owned struct field whose type is another scalar-only struct, copy all
  initialized i64/i32 fields through field-address loads and stores, and recover
  the nested field address with initialization tracking. The source proof passes
  an inner struct through an outer struct; nested enum and aggregate fields remain
  follow-up work.
- KIR enum struct payloads (D-228): the VM builder can declare a variant carrying
  a module-owned scalar-only struct, construct it by copying initialized scalar
  fields into the enum payload area, and recover the payload as an address-backed
  struct value. The source proof constructs and extracts a struct payload; nested
  enum payloads and non-scalar aggregate payloads remain follow-up work.
- KIR parameterized void calls (D-229): the VM builder can create a one-`i64`
  parameter void function, expose its parameter and call it with a checked scalar
  value while preserving the result-free `Unit` signature. The source proof passes
  the main argument through the void callee; wider parameter lists remain follow-up
  work.
- KIR typed parameterized void calls (D-230): the VM builder now supports a
  one-`i32` parameter void function and checked i32 void calls alongside the
  existing i64 contract. The source proof passes the existing narrow value
  through the result-free callee; wider parameter lists remain follow-up work.
- KIR struct enum-payload fields (D-231): struct field assignment now copies a
  known scalar-only struct payload inside an enum field field-by-field, and enum
  field reads preserve the known variant for payload extraction. The source proof
  stores and recovers the enum's struct payload; other aggregate field shapes
  remain follow-up work.
- KIR typed array struct fields (D-232): struct declarations can add fixed i64 or
  i32 array fields, and field assignment copies initialized array elements through
  `elem.addr`. Field reads return an address-backed array with initialized slots;
  the source proof stores and reloads an i64 array field while other aggregate
  field shapes remain follow-up work.
- KIR typed enum array payloads (D-233): enum declarations can add fixed i64 or
  i32 array payload variants, and construction/extraction copy or expose the
  address-backed payload through the existing array ledger. The source proof
  constructs and reloads an i64 array payload; other aggregate payload shapes
  remain follow-up work.
- KIR array aggregate calls (D-234): the VM builder can create one-array-parameter
  i64 functions, expose initialized i64/i32 array parameters and call those
  functions with checked address-backed arrays. The source proof passes its
  existing i64 pair through the array call path; other aggregate result shapes
  remain follow-up work.
- KIR array void calls (D-235): the VM builder can create one-array-parameter
  `Unit` functions and call them with checked i64 or i32 arrays, reusing the
  existing typed array parameter handles and initialization ledger. The source
  proof executes an i64 array void call; additional parameter positions and
  aggregate result shapes remain follow-up work.
- KIR enum-array struct fields (D-236): struct enum fields now copy fixed i64 or
  i32 array payloads element-by-element and preserve the known variant for a
  later payload extraction. The source proof stores and reloads an i64 enum
  array payload through an existing `Record` field; other aggregate field
  shapes remain follow-up work.
- KIR wider void calls (D-237): typed void constructors now accept one or two
  same-width i64 or i32 parameters, and the bridge exposes checked two-argument
  calls with scalar ownership, width, arity and module validation. The source
  proof executes a two-i64 void call; additional arities remain follow-up work.
- KIR array-bearing nested struct fields (D-238): `type_struct_field_struct`,
  `value_struct_set_struct` and `value_struct_get_struct` now accept inner
  nominal structs that carry fixed i64/i32 array fields (via `copyable_struct`),
  copying array elements with `elem.addr` and scalars with `field.addr` while
  keeping full initialization tracking. The source proof copies an array-bearing
  inner struct through an outer struct; nested enum and deeper aggregate fields
  remain follow-up work.
- KIR array-bearing struct enum payloads (D-239): `enum_payload_builder` reuses
  the now `pub(super)` `copyable_struct`/`copy_struct_fields` from
  `struct_builder`, so `type_enum_variant_struct`, `value_enum_new_struct` and
  `value_enum_payload_struct` accept an inner struct with fixed i64/i32 array
  fields. The source proof carries an array-bearing struct enum payload; deeper
  aggregate payloads remain follow-up work.
- KIR array-bearing enum fields in structs (D-240): `value_struct_set_enum`
  now routes its struct-payload copy through `copy_struct_fields`, dropping the
  last `scalar_struct` use (helper removed). A struct enum field can hold an
  enum whose variant carries an array-bearing inner struct. The source proof
  stores `Bag::Full(Inner{cells})` into a `Holder` field and reads the array
  back; deeper aggregate payloads remain follow-up work.
- KIR recursive nested aggregate copies (D-241): `copyable_struct`/
  `copy_struct_fields` were rebuilt around a `CopyStep` plan. `struct_copy_plan`
  walks a struct's nominal/array/scalar fields (resolving nominal names against
  the type table, rejecting cycles) into a flat list of leaf `field.addr`/
  `elem.addr` paths; `copyable_struct` is just `struct_copy_plan(..).is_some()`.
  All call sites pass `&self.types` and the owning module. Inner struct fields
  and struct enum payloads may now nest to arbitrary depth. The source proof
  copies a three-level `Outer{mid{inner{value}}}` struct.
- KIR trap/unreachable terminators (D-242): added `kir_function_trap` and
  `kir_function_unreachable` end to end — `kernel/services.manifest`,
  `builtins.rs` `wrapper_name`/`wrapper_types` (+ `builtins_tests.rs` expected
  list), a new `terminator_builder` VM bridge module, `klf-vm` sentinels and
  `klf-native-rt` stubs. First terminator family beyond return; needed for
  exhaustive-match defaults and panics. Proofs in
  `tests/definition_terminators.rs`.
- KIR foreign service calls in generated code (D-243): `kir_value_call_service_i64`
  and `kir_value_call_service_i64_1` (new `foreign_builder` module) emit a
  `call.foreign` to a source-named kernel symbol with 0/1 i64 args, binding the
  i64 result. Lowered modules can now reach kernel services; proofs in
  `tests/definition_foreign.rs` read `klfsvc_source_len`/`klfsvc_source_byte`
  through `run_with_source`. Wider arities and non-i64 results are open.
- KIR void service calls + output capture (D-244):
  `kir_function_call_service_void_i64_1` emits a void `call.foreign` (one i64
  arg), and `klf_driver::run_with_output` returns a VM run's outcome plus the
  `klf.io` sink bytes. Generated modules can now emit output; the proof in
  `tests/definition_foreign.rs` calls `klfsvc_io_puti` and checks `"5\n"`.
- KIR f64 value services (D-245): added `FunctionBuilder::float_binary` in
  klf-services, plus `kir_value_i64_to_f64`/`f64_to_i64` (reusing
  `conversion_builder::value_convert`) and `kir_value_add/sub/mul/div_f64` in a
  new `float_builder`. First float support in the definition surface. The proof
  in `tests/definition_float.rs` runs the whole arithmetic chain and converts
  back to i64 (no bit-casting needed).
- KIR f64 constants (D-246): `FunctionBuilder::const_float` + the
  `kir_value_const_f64(function, bits)` service build an f64 literal from a bit
  pattern (formatted to a decimal literal for `ConstFloat`). Proof in
  `tests/definition_float.rs` adds `2.5`. f64 compares need a NEW KIR
  float-compare op across model/text/parse/verify + all 3 backends, so they are
  out of the incremental path.
- KIR f64 unary ops (D-247): `FunctionBuilder::float_unary` + the
  `kir_value_neg/abs/sqrt/floor/ceil/trunc/round_f64` services (registered in a
  loop in `float_builder`). Proof in `tests/definition_float.rs`.
- KIR f64 function signatures (D-248): `kir_function_new_f64`/`param_f64`/
  `call_f64` reuse the generic `function_new_with_type`/`function_param`/
  `function_call_typed` helpers. Proof builds a `(f64)->f64` doubler called from
  `main`. f64 comparisons remain the cross-backend KIR-op gap.
- KIR float comparison op (D-249): added `Operation::FloatCompare` +
  `FloatPredicate` end to end — `klf-kir` model/text/parse, `klf-kir-verify`,
  VM (`FCmp` opcode was reserved but unimplemented; now `float_compare` in
  numeric.rs via `partial_cmp` for NaN-correct ordered semantics), Cranelift
  (`fcmp`/`float_cc`→`FloatCC`), LLVM (`fcmp oeq`…). Services
  `kir_value_{eq,ne,lt,le,gt,ge}_f64` in `float_builder` (results tracked in
  `bool_values` for `cond.br`). Parity: `FCMP` corpus entry runs on VM +
  Cranelift + LLVM; definition proof branches on `x < 2.5`. Pattern for a new
  KIR op: model (variant + `used_values`), analysis/verify result type + operand
  check, text print + parse, each backend's lower/emit + predicate map, then the
  service layer.
- Earlier: `call.foreign` + `klf-klffi` + `klf ffi gen` (D-133–137).

## Shutdown state (2026-09-22)

- Implementation changes through D-276 are committed on `main`. The only
  working-tree change to leave unstaged is the user-owned `AGENTS.md`.
- Native definition execution is solved, and not by IPC: the whole service
  bridge moved into `klf-defsvc`, a backend-neutral crate, and the native
  frontend carries its own `DefinitionServiceState` in-process (D-254, D-257).
  The earlier note here — that the bridge could not cross the subprocess
  boundary — described the old `klf run --backend native` harness, which is a
  different thing from a frontend binary and still runs one call at a time.
- Adding a new definition service touches five places: `kernel/services.manifest`,
  `builtins.rs` `wrapper_name` + `wrapper_types`, the `builtins_tests.rs`
  expected wrapper list, a `*_builder` module registered in
  `klf-defsvc/src/lib.rs`, and a `klf-vm/src/services.rs` sentinel. The native
  side is no longer a per-service stub: `klf-native-rt/src/service_abi.rs` needs
  one `service!` line with the symbol's arity, and it dispatches to the same
  handler the VM uses.
- D-238 generalized struct field-struct to array-bearing inner structs; D-239
  reused the helpers for struct enum payloads; D-240 reused them for
  `value_struct_set_enum` (dropping `scalar_struct`); D-241 rebuilt them around a
  `CopyStep` plan so nominal fields nest recursively. The scalar
  D-227/D-228/D-231 behaviours are unchanged. `struct_builder` exposes
  `pub(super)` `CopyStep`, `copyable_struct`, `struct_copy_plan` and
  `copy_struct_fields`.
- `klf build <package>` now links a native frontend and karc builds to a `karc`
  binary whose KIR matches the VM's byte for byte on all seven examples
  (D-254..D-259). Files split on the way: `klf-cranelift/src/lower.rs` into
  `lower/entry.rs`, and `klf-defsvc/src/low_level_builder.rs` into
  `low_level_type_builder.rs`.
- Phase artifacts (D-250) are a definition-owned payload word on the
  definition's untracked heap. The three per-unit phases share one
  `klf_vm::Machine`, built in `klf-driver/src/phase_runner.rs`, so the heap
  outlives a single entry call; the balance check moved from `run_with` to the
  machine. Only the VM backend hosts artifacts (`phase_runner::hosts_artifacts`)
  — a native definition phase still runs one call at a time and cannot carry a
  tree, which is the same address-space problem as the builder services above.
- `units` returns a real list (D-251) and the executor merges one module per
  input file, reconciling a cross-unit declaration against its definition
  (D-252). `../karc` reports `prototype without definition` (1032) and
  `missing main` (1033) again, as whole-program checks: they run in `units`,
  read each input with `fileText` and report through `klf.diag.error_in`, which
  names the file the span is in (D-261).
- Declared diagnostics (D-253): `diagnostics: [DiagnosticSpec]` on the
  `Language` declaration is read by `klf-kira-lower/src/definition_diagnostics.rs`
  and registered by `LanguageDefinition::compile_sources` before the phases run.
- String constants (D-275): `const.string` is a two-result KIR operation for a
  static UTF-8 pointer and byte length. The VM uses synthetic globals, LLVM and
  Cranelift use static data, and the definition bridge exposes the operation and
  its retained length handle through `klf.kir.value.const_string*`. The VM
  definition proof and native parity corpus pass.
- Known enum copies in nominal aggregates (D-276): `CopyStep` plans carry
  source-ledger variant paths through `enum.inject` and `enum.payload.addr`.
  Structs containing a known enum field now copy through nominal struct fields
  and struct enum payloads; the six-test nested aggregate proof passes.
- `cargo fmt --check` is clean repo-wide and the tree was formatted in one
  commit of its own; keep it that way rather than reformatting alongside a
  change.
- Static audit: no file is over the 700-line cap. The largest are
  `klf-kira-lower/tests/language.rs` 684, `klf-defsvc/src/kir_builder.rs` 672 and
  `klf-defsvc/src/aggregate_call_builder.rs` 636, so a service addition in
  `kir_builder.rs` needs a split with it. Keep new phase proofs in their own
  test files.
- The next unimplemented slices are the section 5.2 driver surface and the std
  seam, as described below. The native side now dispatches every
  `klfsvc_kir_*` symbol to the shared bridge. For future services, keep the
  manifest + `builtins.rs` + driver builder + VM sentinel + native ABI line
  pattern, then add a source proof and one scoped gate.
- The C definition exists: `../klf-c`, driver `kcc`, 19 `.klfd` files, about
  6500 lines. It compiles the subset written down in `../klf-c/docs/subset.md` —
  integer types, pointers, arrays, records, the full operator set, every
  statement form including `switch` and `goto`, multi-unit programs — and
  rejects everything else with a declared code (`../klf-c/docs/diagnostics.md`).
  No `#include`, no floating point; both are kernel service gaps, not C
  decisions (D-264, D-265). Tests are `crates/klf-driver/tests/c_*.rs` over
  `tests/c/mod.rs`, which points at the sibling checkout and skips when it is
  absent; `c_native_frontend` proves the native `kcc` emits the same KIR as the
  VM-hosted path. M6's exit test is untouched.
- Two kernel changes came out of writing it. KLF Kira's `&&`/`||` now
  short-circuit (D-262, amending D-123) — a guard like
  `i < list.count && list[i] == x` is unwritable otherwise. And the unit merge
  now treats two type declarations as the same type when everything but the
  span agrees (D-269, amending D-252), which is what lets two C units declare
  the same struct.
- Before resuming, inspect the live `AGENTS.md`, keep work on `main`, keep
  implementation commits signed, and avoid frequent Cargo runs.

## Gotchas

- Struct value = its address. Locals bind `Access::Ssa(addr)`; params are
  `Address(Nominal)`. Struct returns unsolved (stack dangles).
- VM has no width-1 register: i1 ok as `cond.br` cond only, never stored/zext/
  block-param. Bool→int via branch, not zext.
- `p.x` parses as a 2-segment `Path`, not `Field`.
- KLF Kira struct literals use `=`: `Point { x = 1 }`.
- LLVM: never name a KIR block `entry` (collides with preamble). Use `bb0`.
- Native test temp dirs keyed by source hash, else `main`/0 tests race.
- New KIR `Operation`: touch model/text/parse/analysis/verify + all 3 backends
  (catch-alls hide misses as runtime errors — grep the variant).

## Next brick

As of 2026-09-24 the `compatDriver` hook, the function-pointer adapters
(closures, foreign, aggregate-by-address, failing), memberwise lifecycle
synthesis, the M-stage sysroot (`klf build <defdir> --out <dir>`) and the
`klf test` corpus runner are done (D-285..D-292). What remains:

1. Use the seams from real definitions: `../karc` has a std (see
   `../karc/docs/std.md`) but no `corpus/`; `../klf-c` has neither, and kcc
   still reports a prototype without a definition (1032) even when a std would
   provide it. KArc's `std/karc_link.cpp` is still a clang-compiled bootstrap
   (`native.list`) of zero-returning stubs for Harmony's unresolved calls; it
   should shrink as the real std grows.
2. The rest of section 5.4: `<driver>-run`, `libklfvm.a`, `libklfhybrid.a`,
   shared std libraries, std `.ridl`/`.ridlsig`, more targets, and a runtime
   that does not need Rust to build. Std libraries cannot define globals.
3. Done (D-300..D-304, D-312): KLF Kira methods, constants, generic enums
   (`Optional`, `Result`), multi-payload enum cases, and a low-level enum
   slice with concrete specialization names. `../klf-kira`, Kira itself as a
   definition, is written against it;
   its own `HANDOFF.md` has its status. It loads, and `klf test ../klf-kira`
   passes all 45 package-owned corpus cases (D-308 to D-311), including
   packages: `app/` as one module and `import` for path-dependency libraries.
4. Memberwise synthesis rejects generic, `weak`, and array/tuple-of-managed
   members.

## 2026-09-23 implementation update

The hosted `.kir` binary path built `v1-positive.kir` with `--entry choose -- 1 42`; running the output printed `RET 42`.

Built-in program builds now support `klf build <file.kir> --emit bin -o <out>` (and the other source inputs currently compiled by the built-in driver). `bin` defaults to the `native` backend; explicitly selecting another backend is unsupported. The emitted `aarch64-apple-darwin` executable uses the Cranelift object emitter linked with `klf-native-rt` and currently reports results through the `klfrt_finish` RET/FAIL/TRAP protocol. This does not complete the section 5.3 sysroot/std pipeline or the normative LLVM backend; D-114 records the LLVM deferral.

Generated frontends now infer binary output for `-o app`; explicit `.kir` and `.klbc` paths retain their existing output modes. The native binary path is limited to `aarch64-apple-darwin`, links against a runtime archive obtained through the workspace Cargo build, and reports outcomes through `klfrt_finish`. They parse declared typed flags, apply defaults and CLI overrides to Rust `Session` values, and list forms/defaults in help. `.klfd` code reads typed flags and queries per-file mode selection. `klf ridl bindgen --lang swift` emits type-checkable declarations for owned scalar functions; unsupported shapes remain documented comments without layout/call shims. An explicit `compatDriver` override is carried in frontend metadata, but execution reports that `[String]` input marshalling and `Optional<DriverRequest>` decoding are unavailable. The hook body is not emitted or called by the KLF-Kira lowerer, which currently lowers only the four compilation phases; generic `Optional<DriverRequest>` is also outside its type resolver. The standalone argument marshaller is not wired to a hook call. An absent override preserves original arguments for uniform parsing. Automatic memory/convention policy application and a self-contained runtime/sysroot remain unfinished.

## In flight: shaders in the editor and the S12 ins (session KLF editor and shaders)

The order agreed with the user: section 14 of `KLF-EDITOR.md` first (S9 to
S12 of the shader pipeline are committed, see the shader outs section above),
then the S12 ins, then code actions and command parity.

**Testing is off for the rest of this session, at the user's instruction.** So
everything after commit `91a03d7` is written but unrun: no `cargo build`, no
`cargo nextest`, no `klf build`, no `klf test`. What `91a03d7` proved is
recorded under "Done and tested" below. Read the "Left" list for what the
later commits still need checked, and expect to run the commands in "How to
build and test" yourself before trusting it.

### Done and tested

- `<driver> lsp` works for a shader language at all. Two bugs, both found by
  reading the code rather than a test: `analyze` called the program
  `drive()`, which wants `DefinitionPhaseResult::Module` while a shader's
  `lower` returns `DefinitionPhaseResult::Shader`, so every shader `lsp` died
  with `Mismatch(Lower)`; and `new_session` never called `set_shader`, so the
  shader builder answered `KLF-SHD-9001` for every module. Both fixed.
- `klf-shader-ide` (new crate, 3 files): the resource, option, interface and
  host-shareable struct facts of `KLF-EDITOR.md` 14.3 and 14.4, computed from
  the module and its binding assignment.
- `crates/klf-frontend-rt/src/shader_editor.rs` (new): the shader schedule in
  the analysis, every target in memory, refusals attributed to their target.
- `editor` members on the `ksl` and `msl` definitions (21.1, 21.4).
- D-365 to D-367 in `docs/decisions.md`, and an `IMPLEMENTED.md` section.

### Files and hunks that are mine

Everything below is mine, and nothing here was uncommitted before this
session except the other two sessions' work listed above:

- new: `crates/klf-shader-ide/` (Cargo.toml, `src/lib.rs`, `src/builtins.rs`),
  `crates/klf-frontend-rt/src/shader_editor.rs`
- mine, whole file (then moved to `../klf-kira/languages/`):
  `ksl/definitions/{language.klfd, lowering/module.klfd, lowering/types.klfd,
  semantics/program.klfd, semantics/checker.klfd, semantics/shaders.klfd}`,
  `msl/definitions/language.klfd`,
  `crates/klf-shader-driver/src/{report.rs,pipeline.rs}`,
  `crates/klf-shader/src/print/mod.rs`, `Cargo.toml`
- mine, whole file except the CLI session's hunks:
  `crates/klf-frontend-rt/src/{lib.rs,editor.rs}` (the interrupt session's
  hunk in `editor.rs` is `UNWINDS`, `InProcess`, `run_jobs` and the
  `serve` rewrite; the CLI session's is in `lib.rs`),
  `crates/klf-ide/src/analysis.rs`, `crates/klf-lsp/src/features.rs`
- mine, small hunks in files other sessions also touch:
  `crates/klf-facts`-free: `crates/klf-ide/src/analysis.rs` (`IdeDiagnostic`
  gained `source`, and the codec carries it), `crates/klf-kernel-ide/src/lib.rs`
  (`source: None` at both constructors), `crates/klf-lsp/tests/protocol.rs`
  (the same).

### How to build and test

    export PATH=$HOME/.cargo/bin:$PATH
    cargo build -p klf-cli
    cd ../klf-kira/languages
    /root/Dev/kira-projects/kira-language-framework/target/debug/klf build ksl \
      --language msl --language glsl --language hlsl --language wgsl -o /tmp/kslbin/ksl
    printf 'directory\t$PWD\nargument\tDepthOnly.ksl\n' \
      | /tmp/kslbin/ksl lsp-worker          # the raw analysis: facts, diagnostics
    cargo nextest run -p klf-shader -p klf-shader-valid -p klf-shader-bind \
      -p klf-shader-out -p klf-shader-passes -p klf-shader-spirv
    cargo nextest run -p klf-ide -p klf-lsp -p klf-kernel-ide -p klf-defsvc
    cargo nextest run -p klf-driver --test native_frontend

`<driver> lsp-worker` reads an `AnalysisRequest` (tab-separated lines:
`directory`, `argument`, `document`) on standard input and writes the analysis
on standard output. It is the fastest way to see what a shader analysis
recorded; a semantic token is `t`, a symbol `sym`, a hint `h`.

### Also done, since `91a03d7`

Section 14.5's output views and 14.3's last two rows (D-368, D-369), in
`0c9dc1f`:

- `Analysis` carries `shader_outputs`; `klf-frontend-rt/src/shader_editor.rs`
  keeps every target's bytes, the KSIR text and each reflection from
  `Compiled`.
- `crates/klf-lsp/src/views.rs` (new) answers `klf/virtualFile`,
  `klf/shaderOutput` and `klf/status`; `server.rs` sends
  `klf/shaderOutputChanged` when a project's revision lands; `initialize`
  lists the four under `klf.features`.
- `crates/klf-shader-spirv/src/disasm.rs` (new) disassembles the module, and
  `from_hex` reads the text form `hex` writes back.
- A `Shader` runnable becomes a `klf.showOutput` code lens; a resource hover
  names the group's class; a sample call shows its level.

Checked by driving `ksl lsp` over a project, not by a test.

### Left, step by step

1. **14.6 build on save.** Needs the `project` hook (5.3) to return the
   `output` directory, and `klf.editor.project` to exist. It also has to run
   `<driver> build` through the same code path as the command line, writing
   each output atomically.
2. **11.10 code actions.** `klf_diag::Diagnostic` has gained `fixes: Vec<Fix>`
   (D-369), and `Fix` is `title` plus `(Span, String)` edits. What is
   missing is everything after that: no service lets a definition attach a
   fix, `IdeDiagnostic` does not carry one, and `klf-lsp` has no
   `textDocument/codeAction`. The service goes in
   `kernel/services.manifest`, `klf-native-rt/src/lib.rs` and
   `klf-vm/src/services.rs`, all three of which the interrupt session is also
   editing.
3. **S12, the ins.** Unchanged: `../klf-kira/languages/wgsl` still holds only an
   `out` and a `units` stub reporting code 1.
4. **S12, the ins, WGSL first.** `../klf-kira/languages/wgsl` holds an `out`
   and a `units` stub reporting code 1. The `msl` package is the model:
   `syntax/`, `semantics/`, `lowering/` to KSIR through `klf.shader.*`, a
   `corpus/`, and a diagnostic code per refusal. WGSL before GLSL before
   HLSL: it is the smallest grammar and the `wgsl` out already writes it.
   `glslangValidator` is installed for the GLSL and HLSL checks; naga is not,
   so WGSL round-trips are checked against the out itself.
