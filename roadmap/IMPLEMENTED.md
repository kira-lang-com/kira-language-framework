# KLF implementation status

This file records only behavior present in the current tree and covered by an
executable test. The normative requirements remain in `KLF-RIDL.md`.

## What remains (audited against the tree, 2026-10-17)

The dated sections below are a log. Their "deferred" and "not done" lists are
as of their date; this list replaces them. Since those lists were written,
these landed: closure and indirect calls, field and element addresses, the
VM's floating-point foreign calls, `klf.io` host capabilities, generated
standalone frontends compiled from `.klfd`, the `klf-layout` crate, the
`ridlcc` classifier (`klf-ridl/src/cc.rs`), `.ridlmeta` and `_RIDL1` exports
from the LLVM path, the unmanaged manifest in module metadata,
`closure.callback` thunks (section 26.4), `compatDriver`, `klf test`, and
`klf abi freeze`, `apply` and `retire`.

Still open, by spec section:

- 12, 16: KIR operations that are parsed, printed and verified but that no
  backend lowers and no builder service emits: `intrinsic` (section 12.5
  names no entry of the kernel's intrinsic table, nor the VM's `VOP`
  sub-opcodes), `weak.new`, `weak.load` (the VM interpreter has a `WLOAD`
  opcode its lowerer never emits), `call.witness` on a generic type or
  through a witness table the module does not declare (that needs the
  section 23 descriptors), `call.vtable`,
  a `landingpad` with a `catch` clause (section 12.14 says neither what a
  typeinfo is matched against nor what the selector holds, nor whether an
  `invoke`'s normal block receives the callee's result; the verifier
  delivers none). `setjmp`/`longjmp` run on the VM and LLVM; Cranelift
  refuses them, having no way to keep a value a second return needs out
  of a register it gives to another. `asm` lowers on LLVM only, as
  section 12.14 makes it native-engine only, and names no result: the
  section spells no result type for it. `call.variadic` and `va.*`
  follow section 29.5 for a function defined in the module. Every backend
  calls a C import's variadic tail as the platform does, except that the
  VM and Cranelift refuse any variadic call on SysV x86_64, as neither can
  set `al`; every backend refuses an exported
  platform-convention variadic. A
  vector crossing an exported or foreign signature, a vector `bitcast`, or
  a vector operand of an aggregate instruction keeps its function out of
  the lane rewrite, and its backend refuses it. LLVM calls an import
  with a C prototype through its C shim, and a `func.ref` of a function C
  enters through one names that shim. LLVM checks every load and store
  against the runtime heap, so a function C calls back cannot yet read a
  record C passed it, which lives in C's memory; it can only hand the
  record on (section 12.8 names no such check; to do).
  The `volatile`
  and `align` of `raw.load`/`raw.store` change nothing on any backend.
  Aggregate values (section 12.6) are moved into memory before every
  backend, except across an exported or foreign signature, a function
  named by `func.ref` or with a failure type, and a copy or destroy of an
  aggregate that is not memberwise trivial (an array or enum with a
  counted member, a struct with a `copy` or `destroy` function): such a
  function is left as it was and its backend refuses it.
- 13, 23: descriptor-based generics for separately compiled code; the engine
  monomorphizes.
- 14.1: generic specialization of hot polymorphic calls, devirtualization of
  `call.vtable` (section 12.11 does not say whether the receiver is passed
  to the slot's function, nor when `@Vtable` is the object's own),
  uniqueness-check elision for a reference other than a fresh allocation
  (a check that passed proves nothing later: a `weak.load` or `from.raw`
  elsewhere can raise the count without using the reference).
  The aggregate rewrite before every backend
  moves a struct into storage only when its copy and destroy are
  memberwise, so a struct with a copy or destroy function stays a value no
  backend lowers.
- 14.2: `Language.passes`; no definition can add a pass. Section 14.2
  hands a pass a `KirFunctionRef`, but no section gives that handle an
  operation that reads or edits the function's body (section 8.11's
  builder only appends), so a pass written in KLF Kira could only answer
  `false`; the run order and the debug-build verification after each pass
  wait on that surface.
- 18.6: the cycle detector on the native backends and in the driver. The
  VM runs it in memory (`lower_module_traced` + `run_traced`), but native
  code has no way to register a trace map: section 18 names no entry point
  and no byte layout for one, and `.klbc` (section 15.2) has no section for
  them. Also missing: `klfrt_cycles_collect` on demand, `KLF_CYCLES=error`
  as the default under `<driver> test`, and source locations for allocation
  sites. Kira's frontend lowers `ref` fields to raw pointers, so a Kira
  type's trace map stays empty until it keeps them as `ref`.
- 22.7: the `ridlcc` convention in an LLVM fork; the LLVM path uses `swiftcc`
  with an out-parameter error register (D-111, D-118).
- 26.2: a KIR field has no bit width (appendix A.4), so the `explicit c`
  type of an `@FFI.Struct` leaves its bit-fields out of its fields: their
  bits count in the size and the later offsets, but KIR code cannot name
  one. An `@FFI.Alias` target is a one-word C name, so `unsigned long`,
  `long long` and `long double` have none; section 26.2 spells no other
  form. A `.klffi` function marked `@FFI.Variadic` gets no C prototype:
  `foreign_declarations` makes one declaration per function, while a C
  prototype of a variadic names one call's argument shape. `klf ffi gen`
  writes `unsigned long` as `U64`, wrong on Windows, as it has no alias
  target. A KIR-built type with layout rule `c` is laid out by the RIDL
  rules in `klf-layout`, which agree with C for KIR's scalar types on
  every 1.0 target but know no bit-fields.
- 26.5: `klf ffi gen` runs a C preprocessor (`klf-klffi/src/preprocess/`):
  macros including `#`, `##` and variadic, `#include` through `-I` directories,
  the `#if` family with `defined`, and predefined macros per target. Its
  declaration scanner (`klf-klffi/src/cgen.rs`, `cdecl.rs`) maps pointers to
  `<pointee>_ptr` types, handles to opaque `@FFI.Alias` tags, and function-pointer
  typedefs to `@FFI.Callback` structs (D-393). Union members and inline function
  pointers in struct fields still get a remark, not a binding.
- 29: C is compiled by KArc (`../karc`) since `../klf-c` was retired; the
  C17 and GNU surface and the SQLite, Lua, zlib, musl and Linux corpus of
  section 29.2 are not met.
- 5.8: native output covers macOS and Linux on aarch64 and x86_64, x86_64
  Windows, `aarch64-apple-ios` and Tessera
  (`klf_cranelift::SUPPORTED_TARGETS`); a build for `aarch64-android` or
  `aarch64-windows` is refused. An iOS program links but has not been run.
- VM: foreign calls on hosts other than Unix and Windows resolve nothing;
  `wasm32` imports are left to the embedder.
- `KLF-EDITOR.md`: 14.6 build on save (needs the `project` hook of 5.3), the
  unwind tables for COFF, releasing
  counted values held in memory on cancellation (D-363), and stages E5 and
  E8 (E7, with RIDL `@Source`, the KIR and macro expansion views, runnables
  and hierarchies, is served). The editor corpus runner (Appendix A) cannot check `.Symbols` or
  `.Hints`: the appendix gives their golden files no format.
- `KLF-FORMAT.md` 10: on-type formatting (section 6 says it is not
  served and gives it no behaviour), and shader outs printing through the
  engine (section 8 leaves it out of this version).
- `KLF-KSL.md`: S8, hardware parity suites in CI.

## M0 — KIR model, text format, raw verifier

Status: complete for the exit test. Every section 12 instruction and every
section 10.1 / Appendix A.4 module item parses, prints, reparses and verifies,
the V1–V3 clauses have positive and negative fixtures, and the native and
`wasm32-unknown-unknown` gates pass. Later milestones still add behavior behind
these forms; what is complete is the representation, the text format and the
raw verifier, not every backend's treatment of them.

Implemented:

- Rust workspace and layer-0 support crates: `klf-core`, `klf-source`,
  `klf-diag`, and `klf-target`.
- KIR modules, functions, blocks, block parameters, typed SSA values, nominal
  RIDL/C structs, policy records, debug locations, and single or multiple
  instruction results.
- KIR scalar, aggregate, nominal, generic, reference, address, vector, array,
  tuple, and function types.
- Deterministic textual parsing and printing for the implemented model.
- Section 12.1 constants and references: integer, boolean, null, float, zero,
  and string constants, plus function, global, thread-local, descriptor,
  witness, and block-address references.
- Integer arithmetic and comparison, floating-point binary/unary operations,
  `struct.extract`, basic ownership operations, `br`, `cond.br`, `return`,
  `fail`, `trap`, and `unreachable`.
- V1: unique SSA definitions, dominance, entry signature agreement, block
  target existence, and block argument count/type agreement.
- V2: result arity and type checks for every currently parsed instruction.
- V3: exactly one final terminator per block.
- Positive and negative `.kir` fixtures for V1, V2, and V3.
- The normative Geometry example parses, prints, reparses, and verifies.

Completed last:

- Sections 12.2 through 12.16: the whole instruction set in the model, the text
  format and the V2 signature checks.
- The module items of section 10.1 and Appendix A.4: globals, explicit-layout
  types, vtables, traits, conformances, imports, exports and module attributes.
- Type-declaration generics and function generic parameters.
- The fixture matrix in `crates/klf-kir-verify/tests/fixtures/matrix`: one
  positive fixture per instruction family and one negative fixture per V1–V3
  clause, driven by `matrix.rs`.
- `func.ref` is typed as a function value, per section 12.1, rather than as
  `ptr`, so it matches a parameter declared with the callee's signature.

## M1 — Runtime allocator and counting; VM executing hand-written `.kir`

Status: complete. Exit test — the backend-parity suite on the VM — passes: every
`.kir` program in `crates/klf-vm/tests` runs on the VM and produces the expected
return value and trap code, and `klf-rt`'s own suite covers the allocator and
counting. Native and hybrid parity arrive with the LLVM backend in M2.

Implemented:

- `klf-vm` crate: the section 15.4/15.5 bytecode encoding (one 64-bit word per
  instruction, arithmetic width/sign/overflow flags), a compiled program model
  with a constant pool, and a `Program::function_index` lookup.
- Lowering of a single KIR function to bytecode: SSA values to registers,
  branch-site tables with parallel block-parameter transfers, and target fixups.
- `.klbc` version 1.1 branch opcodes (`BR`, `BR_IF`, `BR_CMP`) and the
  `BRANCHES` section; the decoder still accepts version 1.0 images.
- Threaded-dispatch interpreter for integer arithmetic (wrap, trap, saturate),
  signed/unsigned division and remainder with `KLF-RT-0010` and `KLF-RT-0002`
  traps, shifts, rotates, bitwise operations, integer comparison, floating-point
  binary and unary operations, moves, branches, conditional branches, `return`,
  `fail`, `trap`, and `unreachable` (`KLF-RT-0001`).
- Direct calls (`call @f(args)`) with a call-site table, a recursive multi-frame
  call stack with a depth limit (`KLF-RT-0021`), argument passing through
  contiguous registers, and trap and fail propagation across frames.
- Backend-parity tests running hand-written `.kir` on the VM and checking the
  entry function's returned value and trap code, including recursion and
  cross-frame traps.
- `docs/decisions.md` recording bytecode-encoding choices where the spec is
  silent (D-100 through D-102).

- Conversions: `trunc`, `zext`, `sext`, `fptrunc`, `fpext`, `sitofp`, `uitofp`,
  `fptosi`, `fptoui` (invalid or out-of-range float-to-int traps with
  `KLF-RT-0011`), `bitcast`, `ptrtoint`, and `inttoptr`, in the model, text
  format, verifier, lowering and interpreter.
- `switch.int` lowered to comparison-and-jump chains.
- The `klf-rt` runtime crate (section 18): 64 KiB spans and size classes, a
  two-level page map, 8-byte slot metadata, counting by address with interior
  pointers, saturating immortal counts, raw deletion with `0xEF` poison and
  quarantine, weak loads, and the live-block accounting behind the heap-balance
  check. Builds for `wasm32-unknown-unknown`; 23 unit tests.
- VM stack and heap memory: `stack.alloc`/`stack.dealloc` (frame slots freed at
  exit), `load` and `store` through the runtime's bounds-checked accessors
  (`KLF-RT-0005` on an out-of-bounds access), `heap.alloc.counted`/`.unique`,
  `ref.addr`/`uniq.addr`, and counting through `copy` (retain) and `destroy`
  (release/free). A clean return with a non-empty heap fails with `KLF-RT-0100`.

- The `.klbc` module file (section 15.2): header, section table, and the
  STRINGS, CONSTS, FUNCS, CODE, GLOBALS, INITIALIZERS, IMPORTS and BRANCHES
  sections, with a byte round-trip that reruns on the VM.
- The VM predecodes common integer operations and branch metadata. An
  unmetered straight-line numeric loop can execute as an interpreted
  superinstruction; budgeted evaluation keeps per-instruction dispatch and
  charges branch-edge transfers individually.

- The interpreter runs over an explicit call stack (section 15.3), so deep
  recursion traps with `KLF-RT-0021` rather than overflowing the host stack.

Deferred to the milestones that first require them (not part of M1's content):

- Indirect, closure, witness, and vtable calls — needed by the generics engine
  (M3) and the LLVM backend (M2).
- Aggregates and their addresses (`struct.new`, `field.addr`, `elem.addr`),
  which depend on the RIDL layout engine (M5) and the frontends (M4+).
- Destroy callbacks from the runtime into VM functions, first needed by
  non-trivial counted types in the Kira and KArc std (M7, M9). The runtime
  already accepts a destroy function; only trivial payloads are exercised now.
- `weak.new`/`weak.load` and raw `delete` at the KIR/VM level (the runtime
  already implements weak loads, poison and quarantine).
- Host capabilities (section 15.7) for `klf.io` output, first needed by the toy
  toolchain corpus (M4).

## M2 — LLVM backend; `ridlcc` in the LLVM fork

Status: exit test passing. The backend-parity suite runs every `.kir` program in
the shared corpus on the VM and on two native backends (LLVM and Cranelift) and
gets the same return value, fail value and trap code. The RIDL convention suite
passes: exported functions use `swiftcc`, a `context` parameter lowers to the
context register (`swiftself` on LLVM), and a `fail` returns through the error
register. Two native backends stand in for the single LLVM fork the milestone
names (D-114); DWARF, unwinding and the counted-`ref` fast paths are M2 content
deferred below since the exit test does not gate them.

Implemented:

- `klf-llvm` crate: lowering of a verified KIR module to textual LLVM IR, one
  function at a time (section 16). The crate only produces text and stays
  `unsafe`-free; the IR is compiled by the Kira-provisioned LLVM 23.1.0 clang
  (D-110).
- SSA lowering: function and block parameters merge through per-parameter
  `alloca` slots (store on each incoming edge, load at block entry), other
  values are direct LLVM SSA registers (D-113). Value types come from the same
  forward-propagation the VM lowering uses, extended with call result types.
- Full instruction coverage matching the VM: integer arithmetic in wrap, trap
  (`llvm.sadd/ssub/smul.with.overflow` guarded to `KLF-RT-0002`) and saturate
  (`llvm.sadd/ssub.sat`, manual signed-mul clamp) modes; signed/unsigned
  division and remainder with `KLF-RT-0010` zero guards and the `INT_MIN / -1`
  case; shifts and their trap-on-overshift; bitwise ops; integer comparison;
  floating-point binary/unary ops through the matching LLVM intrinsics; every
  conversion, including `fptosi`/`fptoui` range and NaN guards to
  `KLF-RT-0011`; `switch.int` as an LLVM `switch`; branches, conditional
  branches, `return`, `fail`, `trap` and `unreachable`.
- Direct and recursive calls to KIR functions (namespaced under `kir.` so a KIR
  `@main` never collides with the C entry point). Exported functions and their
  call sites use LLVM `swiftcc`, the realization of `ridlcc` (D-111).
- Memory and counting through the runtime: `stack.alloc`/`stack.dealloc`,
  `load`/`store` (with `KLF-RT-0005` out-of-bounds trap),
  `heap.alloc.counted`/`.unique`, `ref.addr`/`uniq.addr`, and counting through
  `copy` (retain) and `destroy` (release/free).
- `klf-native-rt` crate: the section-18 runtime exposed as `extern "C"`
  `klfrt_*` symbols over one thread-local `klf-rt` `Heap`, a per-frame stack
  list that frees stack slots at frame exit, and a depth counter that traps with
  `KLF-RT-0021` (D-112). A clean top-level return runs the heap-balance check
  and traps with `KLF-RT-0100` when the heap is not empty. Traps cross the
  boundary as numeric codes, so the shim needs no `unsafe`.
- KIR enums: RIDL-rule variant declarations, tag and payload-address operations,
  injection, and `switch.enum`; the VM, Cranelift and LLVM backends agree on
  tag width, payload placement and dispatch.
- `klf-cranelift` crate: a second native backend lowering KIR to Cranelift IR
  and emitting a relocatable object, linked against the same `klf-native-rt`
  runtime (D-114). Cranelift's native SSA block parameters map KIR blocks
  across directly. Scalar saturation and the `INT_MIN / -1` division case are
  handled explicitly because Cranelift's `*_sat` is vector-only and its `sdiv`
  traps (D-115). It shares one value-type inference with the LLVM backend
  (`klf-kir::infer_value_types`, D-116).
- Cross-backend parity harness: `klf-llvm/tests/native_parity.rs` and
  `klf-cranelift/tests/native_parity.rs` share one corpus, compile each program
  with the provisioned clang (LLVM IR, or the Cranelift object), run it, read
  the outcome back in the VM's `Outcome` shape, and assert it equals the VM's.

- The `ridlcc` context and error registers. A parameter marked `context` in KIR
  text (`%c: context *T`) lowers to a `swiftself` `swiftcc` parameter on LLVM and
  to an ordinary leading pointer on Cranelift (D-117). A function that can `fail`
  carries an error out-pointer: null is success, non-null is failure with the
  value in a runtime box (`klfrt_error_box`/`klfrt_error_value`). The LLVM
  backend appends it to `swiftcc` failing functions and propagates it at each
  call site; Cranelift does the same on its own calling convention (D-118,
  D-374).

Deferred M2 content:

- Promotion of the error out-pointer to the LLVM `swifterror` attribute (the
  actual error register), which the fork of section 22.7 removes as a step.

What remains of M2, item by item:

- **The `swifterror` promotion is off the list by the spec's own direction.**
  D-111 and D-118 make the error register a plain out-parameter on `swiftcc` a
  stopgap, and line 74 puts a real `ridlcc` — with its own error register — in
  an LLVM fork that is part of the kernel. With the fork, the error register is
  the register, and there is no `swifterror` to promote to. Implementing the
  fork is not this milestone's work.
- **Section 26.4 thunks are largely in place.** D-288 synthesizes a platform
  thunk per callback type that recovers a closure from the named user-data
  parameter, and D-289 covers foreign callees and every non-generic parameter
  and result type, since KIR passes aggregates by address. `klf-llvm`'s parity
  suite runs `aggregates_by_address_cross_an_abi_thunk` and
  `closures_call_directly_and_through_a_callback_thunk`. What is still missing
  is recorded with M6: the section-25 shims for non-trivial bindings.
- **The inline counting fast path is done.** A `ref T` whose `T` is declared
  `pairing: counted` and whose allocation the runtime serves from one pooled
  span is counted inline: the backend reaches the slot's metadata word the way
  section 18.3 does — `span = p & ~0xFFFF`, `slot_size` at header offset 8,
  `data_start` at 20, the table at 64 with eight bytes per slot, and
  `slot = (p - (span + data_start)) / slot_size` — and moves the count with one
  `atomicrmw`. A null pointer, a frozen count, an already-zero or saturated
  count, and the last reference all branch to `klfrt_retain`/`klfrt_release`,
  which own the page-map lookup, the saturating count, the destructor, the
  collective weak reference and the free at zero. A larger allocation gets a
  dedicated run of spans holding one slot with the header at the run base,
  which only the page map finds, so it is not counted inline at all. The
  `COUNTED` corpus fixture covers pooled aliasing, deep aliasing, a loop of
  allocate-and-drop, the last reference and the wide fallback, and the runtime's
  exit balance check makes a wrong count a failure rather than a silent leak.

`heap.alloc.counted` and `heap.alloc.unique` also size a nominal type through
`klf-layout` now, matching `stack.alloc` and the Cranelift backend, which is
what makes a counted record allocatable at all.

Debug information (section 16.3) is done on the backend, with the Kira
frontend now feeding it, and the trap half of section 16.4:

- `klf-llvm` emits a full DWARF compile unit for every module: the format flags
  (`Dwarf Version` 5 on ELF and Mach-O, `Debug Info Format` for `CodeView` on a
  Windows triple), the compile unit, the file table and one `DISubprogram` per
  function with its `DISubroutineType`. A `!loc` on an instruction becomes a
  `DILocation` on every instruction the backend writes from it, and a `!loc`
  naming another file gets a `DILexicalBlock` so generated text keeps its own
  line table. Variables are named with debug records (`#dbg_declare` for a
  parameter spilled to its slot and for the address `debug.addr` names,
  `#dbg_value` for the value `debug.value` names), not with the
  `llvm.dbg.value` and `llvm.dbg.addr` intrinsics, which LLVM removed in 17.
- The Kira frontend fills in `!loc` from the spans of the statements and
  expressions it walks (`FunctionBuilder::at`). An expression places what it
  builds at its own span and restores the enclosing one before returning, so an
  operand does not claim the operation its parent emits. The canonical passes
  carry the locations, so a Kira program reaches the backend with a real line
  table.
- Types are described only in DWARF C can express, which is what lets stock
  LLDB format them. A record is a `DW_TAG_structure_type` of its members at
  their computed offsets and an enum is that record plus a `DW_TAG_variant_part`
  naming every case with its discriminator, all reached through a typedef on a
  pointer, which is the shape LLDB already prints for C and C++ on a target with
  opaque pointers. A handle prints under its KLF spelling, `ref Point *`, and
  the record `debug.addr` names is described as the record rather than as a
  pointer to it, since `#dbg_declare` already dereferences.
- `DebugSources` carries the text of the virtual files of section 8.1, which
  reaches the line table as the DWARF 5 source extension (a `!DIFile` with
  `checksumkind: CSK_MD5`, a `checksum` and a `source`), so a debugger shows
  generated text it cannot read from disk.
- `klfrt_trap` prints the code, a symbolized backtrace and then aborts
  (section 16.4). The backtrace is printed only when the program carries the
  debug information to symbolize it with: `klfrt_image` reads the image's
  section table once — a `.debug_info` section on ELF, `__debug_info` on
  Mach-O, a `.debug` section on COFF — and a stripped image prints nothing extra
  rather than a column of addresses. A fault raised while the frames are being
  taken is not reported twice.
- Landing-pad unwinding and `klfrt_personality` were already in place for
  definition code (`klf-cranelift`): a `try_call` whose frame owns counted values
  lands in a pad that releases them, the object writer emits the `.eh_frame` and
  the landing table the personality reads.

What the vendor language code of section 16.3 costs, measured against LLDB
21.1.8 rather than assumed:

- With `DW_AT_language` set to `0x8e57`, LLDB reports `no plugin for the
  language "unknown"`, resolves breakpoints and steps correctly, and prints
  nothing at all for `frame variable`. The plugin is chosen from the compile
  unit's language, so the vendor code disables variable inspection however
  C-expressible the types are. The same happens for `DW_LANG_Rust`.
- With `DW_LANG_C11` or `DW_LANG_C_plus_plus` in place of the vendor code,
  stock LLDB sets a breakpoint on a `.kira` line, steps, and prints the locals
  with their values. `klf-kira-lower`'s `stock_lldb_stops_on_a_kira_line_and_
  prints_the_locals` is that run.
- `DW_TAG_variant_part` is not rendered as a tagged union by LLDB's Clang type
  system. LLDB exposes it the way it exposes a rustc enum: a `union` whose
  members are the discriminator and one `$variant$N` struct per case. That is
  the best a stock debugger does with this encoding, and the encoding is the
  same one rustc emits.

The two halves of section 16.3's parenthetical — a vendor code `0x8e57` for
Kira, and type descriptions restricted to C-expressible DWARF "so that stock
LLDB formats them" — contradict each other in one respect. The vendor code is
implemented as the spec states, and what it costs is measured rather than
assumed: LLDB 21.1.8 resolves breakpoints and steps correctly on a Kira
compile unit tagged `0x8e57` and prints nothing for `frame variable`, because it
chooses its language plugin from the compile unit's language. The same binary
tagged `DW_LANG_C11` or `DW_LANG_C_plus_plus` gives the same breakpoints and
stepping and prints its locals with their values. The workaround of a clang
producer string alongside the vendor code was measured and does not change the
plugin choice. Changing the code is a change to the spec, so it has not been
made.

## M3 — Ownership passes, full verifier, generics engine

Status: complete. The exit test — the ownership and verifier suites — passes.
All five mandatory raw-to-canonical passes run, the verifier enforces rules V1
through V12, and the generics engine monomorphizes generic calls. The IR grew
the instructions these need: `asm`, `access.begin`/`access.end`, `store`
init/assign modes, `try.call`, and `copy.addr`/`move.addr`/`destroy.addr`.

Implemented:

- `klf-kir-passes` crate: the mandatory raw-to-canonical pipeline
  (`canonicalize`). Copy/move resolution (section 11.6 pass 2): when an owned
  value is consumed more than once in a block, an implicit-copy type (`ref`,
  `weak`) gets a `copy` inserted before all but the last consuming use and that
  use rewritten to the copy, while an affine type (`uniq`) or a consume after a
  `move` is reported as `own.use-after-move` (`KLF-KIR-1101`). A `ref` consumed
  by two calls becomes balanced canonical KIR that runs on the VM; a `uniq`
  consumed twice is rejected.
  Destroy insertion (section 11.6 pass 4) is done: for each
  owned SSA value it computes backward liveness and forward availability over
  the CFG and inserts `destroy` where the value dies unconsumed — before an exit
  terminator, once before a branch when the value is dead on every edge, or at
  the start of a single-predecessor successor on a dying edge. Raw KIR that
  leaks an owned value becomes canonical KIR that passes V4 and runs balanced on
  the VM (no `KLF-RT-0100`); fixtures cover a straight-line leak, a diamond, and
  a value consumed on one branch and destroyed on the other.
- Verifier rule V4 (section 11.7): every owned SSA value (`ref`, `uniq`, `weak`)
  is consumed exactly once on every path to `return`, `fail` or `unreachable`;
  paths ending in `trap` are exempt. It is a per-value forward dataflow over the
  CFG that tracks a consumed flag, counting `move`/`destroy`/`store`/owned call
  arguments and block-argument transfers as consumes, and reports a leak, a
  double consume, or a consume that happens on some paths but not others
  (`KLF-KIR-1004`). It runs only on canonical KIR (the mandatory passes produce
  the balancing copies and destroys). Positive and negative fixtures cover a
  balanced function, per-path consumption, the trap exemption, a leak, a double
  consume, and a conditional leak.
- Verifier rules V5 and V6 (section 11.7): a borrowed value (the result of
  `borrow.begin`) is never consumed, stored, returned or transferred
  (`KLF-KIR-1005`); and every borrow scope is matched and closed on every path,
  with `borrow.end` requiring a matching open borrow and no scope left open at a
  function exit (`KLF-KIR-1006`), checked by a forward open-scope dataflow.
  Borrowed values are excluded from V4 (they carry a `ref` type but are not
  owned). Positive and negative fixtures included.
- Verifier rule V12 (section 11.7): an exported function's parameters, result
  and failure use only RIDL-representable types — raw pointers, addresses, bare
  generics, function values and vectors are rejected (`KLF-KIR-1012`). Positive
  and negative fixtures included.
- The generics engine (section 13) by monomorphization (`monomorphize`, D-119).
  KIR gained generic function parameters (`func @f<$T>(...)`) and generic call
  arguments (`call @f<i64>(...)`) in the model, parser and printer. Each generic
  call is specialized into a copy of the callee with the generic parameters
  substituted through every type (parameters, results, block parameters and the
  type operands of instructions), the call is rewritten to the mangled
  specialization (`f$i64`), and the templates are dropped. A generic function
  instantiated at `i64` monomorphizes to concrete KIR that verifies and runs on
  the VM; the generic syntax round-trips.

- Verifier rule V11 (section 11.7, first clause): a generic parameter `$T`
  appears only in a function that declares it (`KLF-KIR-1011`), checked by
  walking every type in the signature and body. Positive and negative fixtures
  included.
- Verifier rules V7–V10: static-access conflicts on the same storage
  (`access.begin`/`access.end`, `KLF-KIR-1007`); address-only types never held
  as loadable SSA values (`relocate: fn`, `KLF-KIR-1008`); no `asm` in a
  `runtime`/`any` function (`KLF-KIR-1009`); and `try.call` used if and only if
  the callee is fallible (`KLF-KIR-1010`). Each with positive and negative
  fixtures.
- The other three mandatory passes: definite initialization (`store [init]`
  before `load`/`assign`, `init.use-before-init`/`init.double-init`), borrow
  checking (adds `own.consume-while-borrowed` on top of V5/V6/V7), and
  address-only lowering (value copy/move/destroy on an address-only type become
  `copy.addr`/`move.addr`/`destroy.addr` on stack slots). With tests.

Deferred beyond M3:

- Descriptor-based generics (runtime type descriptors and witness/vtable calls,
  section 13/23), needed for separately-compiled generic code; the current
  engine monomorphizes concrete instantiations (D-119).
- Lowering of `asm`; `try.call`, `access.*` and
  `copy.addr`/`move.addr`/`destroy.addr` now lower on all three backends, and
  the rest are modeled, parsed, printed and verified, and lower when the
  backends and RIDL layout (M5) need them to execute.

Of those six, `access.*`, `try.call` and the address-ownership trio lower on all three
backends. `access.begin [read or modify] [static or dynamic or unchecked] %addr`
brackets a memory access: `static` is proved by the verifier and `unchecked`
asks for nothing, so both emit nothing, and only `dynamic` reaches
`klfrt_access_begin`, which holds the claims a thread has open and traps with
`KLF-RT-0003` when two overlapping accesses conflict and either is a `modify`.
The VM keeps the same claims per frame and traps identically.

`copy.addr`, `move.addr` and `destroy.addr` (section 11.6 pass 5, section 12.8)
act on storage rather than registers. An address is a pointer on both native
backends and a register in the VM, so the payload move is a `memcpy` of the
pointee's own layout and the policy is the retain, release or free the value
form applies. `copy.addr` retains once for the second owner it leaves,
`move.addr` retains nothing because a relocation moves the one value there is,
and `[assign]` destroys what the destination held before either runs.

`try.call` lowers on all three backends (D-374). `bbN` receives the result
and `bbE` the error as their leading block parameters, ahead of any arguments
the target names, and the verifier checks both against the callee's signature
(V1). LLVM and Cranelift share D-118's error out-pointer: `try.call` tests it
and branches instead of propagating, and Cranelift no longer aborts in `fail`.
On the VM the call site names a `failed` register and the branch site of its
two targets; a `fail` unwinds to the nearest frame a `try.call` entered.

`asm` remains. It cannot be lowered from the model as it
stands. `Operation::Asm` carries
one opaque string, while section 12.5 spells the instruction as
`asm "template", "constraints"(args)` — a template, a constraint string and
operands, which is three things and not one. The parser unquotes the remainder
whole and the spec defines no grammar for it, so any lowering would be reading
an encoding the spec never fixed. It needs either the model split into the
parts the section names or a stated grammar; that is a question for the user,
not a decision to invent here.

## M4 — KLF Kira frontend and services

Status: the exit test passes. The toy integer-calculator toolchain compiles and
runs its corpus on the VM and on the native (Cranelift) backend. The KLF Kira
frontend (syntax, checking, lowering), the kernel services substrate, the
`Definition` interface, the driver and the `klf` CLI are all in place. What is
deferred (below) is loading a complete definition object written in KLF Kira
rather than Rust-hosting it (D-126), and the full section-8 services for the
whole language.

Implemented:

- `klf-kira-syntax` crate (layer 5), the KLF Kira frontend's syntax layer for
  the Appendix A.1 grammar:
  - Lexer (`lex`): source text to a token stream ending in `Eof`. Reserved
    words become keyword tokens; the ten contextual words (`borrow`, `mut`,
    `move`, `copy`, `handle`, `requires`, `init`, `self`, `Any`, `some`) stay
    `Ident` and are resolved by the parser. Handles `//`/`///` line comments,
    nested `/* */` block comments, decimal/hex/binary/octal integers with `_`
    separators, floats with fraction and exponent, string literals, and every
    operator of A.1 by longest match (`<<=`, `&+`, `->`, ...). Newline runs
    collapse to one soft-separator `Newline` token, leading and trailing runs
    dropped. Lexical errors: `KLFK-E0100` unterminated block comment,
    `KLFK-E0101` unterminated string, `KLFK-E0102` unexpected character (D-121).
  - AST (`ast`): full node set for A.1 — imports, all nine declaration kinds
    (function, struct, enum, trait, construct, extend, alias, distinct,
    constant) with annotations and generics, members, enum cases, construct
    members, types, blocks, every statement form (`let`/`var`, assignment,
    `if`/`else`, `while`, `for`, `match`, `attempt`/`handle`, `return`,
    `break`, `continue`), patterns, and expressions.
  - Parser (`parse`): recursive descent, split by grammar area
    (`parser/{decls,types,stmts,exprs}.rs`). Precedence-climbing binary
    expressions matching A.1's low-to-high table with `as`/`is` at the top;
    ternary, unary, postfix (member/call/index); struct, array, map, tuple and
    closure literals; labeled call arguments. A struct literal is suppressed in
    condition and scrutinee position so `if p { ... }` reads the brace as the
    body, not `p { ... }` (matches the Rust/Swift rule). Newlines are skipped
    inside bracketed groups and after binary operators so an expression may
    wrap after an operator. Error recovery resynchronizes to the next
    separator, with a progress guard so a stray brace cannot loop; malformed
    declarations do not stop later ones from parsing. Structural errors report
    `KLFK-E0002`.
  - Where A.1's grammar and the section 7.4 example skeleton disagree (the
    example writes `let name = "karc"` with no type; the grammar requires
    `let ident : type`), the grammar wins per rule 1.1.

- `klf-services` crate (layer 5): the kernel-side substrate a frontend runs
  against (D-122). `Session` carries registered sources, diagnostics, name
  interning, scopes/import edges, named arenas, typed type/layout records and
  the target and counts errors.
  `ModuleBuilder`/`FunctionBuilder`
  are an ergonomic KIR builder over the section-12 model: fresh SSA value and
  block names, a current-block cursor, one method per instruction the frontend
  needs (constants, integer binary ops, comparisons, calls, stack slots,
  load/store, branches, returns, traps), and a `finish` that closes any block
  left without a terminator with `unreachable`. Session scope/import-edge
  services are present; the trait solver/conformance registry and nested
  speculation and language-wide query dependency tracking are present, while
  arbitrary definition execution remains later.
- `klf-kira-lower` crate (layer 5): parses, elaborates and lowers a KLF Kira
  subset to raw KIR. Declaration resolution and signature typing are an
  explicit elaboration artifact; expression resolution and type checking remain
  folded into the lowering walk (D-122).
  The subset is top-level functions over `Int`/sized integers/`Bool`/`Void`,
  structs and enums, with arithmetic (trapping and `&`-wrapping), comparisons,
  bitwise and shift operators, `let`/`var` bindings and assignment,
  `if`/`else if`/`else`, `while` with `break`/`continue`, direct calls,
  `return`, unary `-`/`!`/`~`, the `? :` conditional (lowered through a join
  block with a parameter), enum literals and exhaustive `match` with scalar
  payload bindings.
  `&&`/`||` lower to bitwise `and`/`or` on the pure operands of the subset,
  `var` to a stack slot, `let` to an SSA value (D-123). Semantic errors report
  in the `KLFK-E02xx` band (unknown name, type mismatch, arg count, condition
  not Bool, return mismatch, assign-to-immutable, out-of-subset construct).
  End-to-end tests parse KLF Kira source, lower it and run it on the VM:
  calls and bindings return 42, a `while` loop sums, `if`/`else if` classifies,
  the ternary picks a max, `+` traps on overflow and `&+` wraps to `i64::MIN`.
  A pipeline test confirms the emitted KIR re-parses, canonicalizes clean
  (ownership passes) and passes the verifier before running.
- `klf-cli` crate (the `klf` binary, layer 7): an M4 command surface over the
  frontend (D-125). `klf run <file.klfk>` lowers the source and runs its entry
  (default `main`) on the VM, printing the return value or the fail/trap;
  `klf build <file.klfk> --emit kir` prints the lowered KIR. Diagnostics render
  as `file:line:col: severity[CODE]: message`. Demonstrated end to end: a
  `factorial` program prints `120` for `main` and `720` for `factorial(6)`.
- VM fix (D-124): `klf-vm`'s SSA type inference now types `call` results from
  the callee's declared result, matching the shared inference; frontend code
  that stores a call result (`s = add(s, i)`) previously failed to lower.
- `klf-services::Definition` (D-126): the kernel-side Rust form of the
  section-7 `Language` family (`name`, `extensions`, metadata, package units,
  phase artifacts and the compatibility `compile` entry). Implemented by
  `KiraDefinition` (the KLF Kira frontend) and `CalcDefinition`.
- `klf-toy-calc` crate: the toy integer-calculator language definition. A tiny
  language — `def name(params) = expr` over i64 with arithmetic, calls, and
  `if c cmp c then e else e` — with its own lexer, parser and KIR lowering
  through the kernel builder. Its `corpus/` holds `arith.calc`, `control.calc`
  and `recur.calc`.
- `klf-driver` crate (layer 6): the driver loop. Runs a compiled module on a
  chosen `Backend` — `Vm`, or `Native` (Cranelift object linked against
  `klf-native-rt` with the provisioned clang), reading the outcome back in the
  VM's shape; skips native cleanly when the toolchain is absent.
- `klf-cli` selects the definition by file extension (`.calc` vs KLF Kira) and
  takes `--backend vm|native`.
- Exit test: `klf-toy-calc`'s conformance suite compiles every corpus program
  through `CalcDefinition` and runs it on the VM and (when the toolchain is
  present) native, asserting the oracle result — `fact(5)` = 120, `fib(10)` =
  55, `clamp`, `abs`, `max`, arithmetic and calls all agree across backends.

Deferred beyond M4:

- Loading and executing an arbitrary KLF Kira definition object (`.klfd`)
  rather than Rust-hosting it: the runtime bridge exists, and the typed
  `Language`/`Session` metadata, source services, phase boundary and service
  manifest exist, but the full runtime service ABI and executable arbitrary
  definition loader do not. The arithmetic `.klfd` corpus is recorded in the
  post-M6 groundwork below.
- `klf-kira-sema` split out from `klf-kira-lower` once the language grows past
  the single-pass subset; remaining full section-8 type/layout/policy
  integration in the elaborator and parallel query scheduling.
- The LLVM backend and `klf build` emitting a standalone per-language driver
  binary (the driver runs modules in-process today).

## M5 — RIDL: layout, emission, metadata, linking, freeze and apply, bindgen

Status: exit test passing. Both halves pass: the RIDL layout and evolution
suites, and a Rust client calling a KLF library through bindgen. The end-to-end
test compiles a KLF module to a native object exporting its functions under
their `_RIDL1…` symbols (Cranelift), generates Rust bindings from the matching
`.ridl`, and compiles a Rust client that links the object and gets the right
answers back (`add(20, 22) == 42`, `mul(6, 7) == 42`). The mangling, metadata,
descriptor and bindgen suites also pass.

Implemented in the new `klf-ridl` crate (layer 4, D-127):

- The RIDL type system (section 20): scalars, `String`, arrays, `Optional`,
  `Result`, tuples, nominal types with generic arguments, owned and `borrow`
  closures, trait objects and generic parameters, with type substitution. A
  `Registry` of struct and enum declarations (opaque structs carry no body).
- The data-layout engine (section 21) with a fixed-value layout suite on a
  64-bit target: scalars; structs and tuples (declaration order, per-field
  alignment padding, empty struct is zero-sized); enums (`U8`/`U16`/`U32` tag by
  case count, `@Open` always `U32`, tag-then-payload placement, payload-less
  enums are just the tag); the two-case pointer niche for `Optional` of an
  opaque struct/trait object/closure; four-word `String`/`[T]`; one-pointer
  opaque structs; two-word trait objects and closures; and generic-struct
  argument substitution. Field placements report concrete offsets.
- The `.ridl` parser (grammar A.2): a self-contained lexer and recursive-descent
  parser for imports, structs (incl. opaque), enums, traits (parents, associated
  types, methods), `extend` (conformance and methods), free functions and
  methods with `borrow`/`borrow mut`/`self` conventions and generics, aliases,
  constants and annotations; `@Open` folds into the enum. Errors report in the
  `KLF-RIDL-0001` band.
- Emission and canonical text (section 24.6): readable `.ridl` output that
  reparses to the same module, and a canonical form (declarations sorted by
  path, one space between tokens, no comments) that is order-independent and is
  what the metadata hash covers.
- Symbol mangling (section 24.2): `_RIDL1`-prefixed free-function, method,
  descriptor, builder, descriptor-size, witness-table and named-instance
  symbols, with the full type-code alphabet; matches the spec's worked example.
- Module metadata (section 24.3) with an in-tree SHA-256 (verified against the
  `""`/`"abc"` test vectors): the `RIDL` blob encodes/decodes the module name,
  ABI records (uuid + current/supported/retired status), imports with required
  UUIDs, the canonical-text hash, and unmanaged function names.
- Linking checks (section 24.5): an import of a current/supported UUID links, a
  retired one warns (`KLF-RT-0201`) and is blocked only under strict mode, an
  absent one fails (`KLF-RT-0200`), and two copies of a module with different
  current UUIDs conflict (`KLF-RT-0202`).
- ABI freeze and apply (section 24.4, D-128), the evolution suite: a pure
  `freeze` computes the additive delta against the frozen history (added and
  superseded shapes) given an externally-minted UUID, reports `ridl.abi-broken`
  when a frozen shape is changed or removed in place, and `apply` stamps each
  superseded shape with `@Abi(version: "<origin uuid>")` so a re-freeze is a
  no-op.
- Rust bindgen (section 25, D-129): `klf ridl bindgen` emits `#[repr(C)]`
  structs, `#[repr(transparent)]` opaque-handle newtypes, and, for trivial
  scalar/float free functions, an `unsafe extern "C"` declaration of the mangled
  symbol plus a safe wrapper; shapes needing a shim are emitted as remarks. The
  generated source is validated by type-checking it with `rustc`.
- Descriptors, witness tables and builders as emitted data (section 23, D-131):
  the section-23.1 type descriptor (abi, flags, size, align, `copy`/`move`/
  `destroy`), the section-23.2 witness table, and the section-23.4 builder
  descriptor size, each laid out as a byte image whose function-pointer and
  entry slots are reported as named relocations for the linker. Trivial and
  opaque-handle descriptors, and narrow (`wasm32`) words, are covered.
- The native library path (D-130): `klf-cranelift::emit_library_object` lowers a
  module and emits each `(kir name, symbol)` export as a public symbol under the
  RIDL name with the platform C convention (the register-level `ridlcc` for
  trivial scalar functions), no `main`, so the object links into another program.
- ABI freeze/apply/retire on disk (D-132): `mint_uuid_v4` (seeded from the clock
  and pid, no external RNG), the `.ridlsig` record format, and `klf abi
  freeze|apply|retire <file.ridl>` writing `abi/<uuid>.ridlsig`, stamping
  superseded shapes back into the source, and moving retired records to
  `abi/unsupported/`.
- `klf ridl emit|layout|mangle|bindgen <file.ridl>` and `klf abi
  freeze|apply|retire` in the `klf` CLI.

Deferred beyond the current M5 work:

- Emitting descriptors, witness tables, builders and the `.ridlmeta` section into
  the native objects (the data models and relocations exist; the backend does
  not yet write them), and the LLVM backend's `_RIDL1…` exports (Cranelift does).
- The section-25 assembly/LLVM-IR shims for non-trivial bindings (owned
  aggregates, borrows, generics, failing functions, methods, trait objects); the
  section-22.3 calling-convention classifier as a lowering pass.
- The section-32.1 split of layout into a layer-1 `klf-layout` crate (D-127).

## M6 — C ABI boundary (`.klffi`, `call.foreign`, `klf ffi gen`); a C definition compiling a subset

Status: the FFI boundary is built, and a C definition package now exists and
compiles a subset of C. M6's exit test — `kcc` building SQLite, Lua, zlib, musl
and the Linux kernel — is untouched and remains far off.

The three prerequisites this section used to list as blockers are closed:
(a) KLF Kira expresses a lexer, preprocessor, parser and lowerer — the C
definition is about 6500 lines of it; (b) the section-8.12 kernel service ABI
runs, both hosted on the VM and natively through `klf-native-rt`'s `klfsvc_*`
symbols; (c) the `.klfd` package loader and the four-phase `Language` executor
run a definition of that size. What remains before the exit test is approachable
is listed under the C definition below.

### The C definition (`../klf-c`, driver `kcc`)

The package lives in its own repository, `../klf-c` (D-263, D-272), not in this
one: it is a language definition, not kernel code. It is written in KLF Kira,
loaded by `klf build`, and its `Language` declares the section-29.1 metadata —
`name = "c"`, `driver = "kcc"`, `extensions = ["c", "h"]`, `memory =
.Unmanaged`, `exclusivity = .None`, `convention = .Ridl`, one unit per source
file and the `platform` file mode. C objects are laid out by
`LayoutRule.PlatformC` and a C pointer is a plain address; nothing is retained
or released (section 29.3).

Layout: `token`, `preprocess`, `preprocess_directives`, `ast`, `types`,
`parser_top`/`parser_decl`/`parser_stmt`/`parser_expr`, `elaborate`,
`lower_module`/`lower_core`/`lower_expr`/`lower_stmt`/`lower_call`/`lower_values`,
`kir` as the single place that maps a construct to a `klf.kir.*` service,
`diagnostics` as the code registry, and one `language.klfd` carrying the
`extends Language` declaration.

The subset that works, each part covered by a test: the integer types with C's
promotions and usual arithmetic conversions; pointers, pointer arithmetic in
both directions and pointer difference; arrays, including multidimensional ones
and brace initializers; `struct`, `union`, `enum` and `typedef`; `static` and
`extern`; the full operator set with C's precedence, including short-circuit
`&&`/`||`, the conditional and comma operators, compound assignment and the
increment forms as expressions; `if`/`while`/`for`/`do`/`switch` with
fallthrough and `goto` with labels; functions with forward declarations,
recursion, records passed and returned by value, and calls through function
pointers; string literals with escapes; `sizeof` on a type and on an
expression; and programs that span several translation units, where a prototype
in one unit resolves to a definition in another and two units may declare the
same struct. `klf-c/docs/subset.md` states it precisely.

The preprocessor is a token-to-token pass with `#define` object- and
function-like, `#undef`, the `#if` family with `defined` and constant-expression
evaluation, `#error`, and `#line`/`#pragma` accepted and ignored. `#include` is
rejected (`KCC-E0115`) because a phase cannot read a file other than its own
unit, and `#`/`##` are rejected (`KCC-E0114`). Floating point is rejected
(`KCC-E0101`): the KIR builder services expose no float type. See D-264 and
D-265.

Diagnostics are a `DiagnosticSpec` registry on the `Language`, reported from the
phase that finds the problem: lexical, preprocessing and grammatical codes from
`parse`, the statement rules from `elaborate`, the type rules from `lower`, and
the whole-program rules from `units`, which is the only call that sees every
input. `klf-c/docs/diagnostics.md` lists all 41 codes and their phases.

Proof, in `crates/klf-driver/tests/c_*.rs` over the shared `tests/c/mod.rs`
harness (29 VM-hosted tests and 2 native):

- `c_programs`, `c_types`, `c_control_flow`, `c_pointers`, `c_aggregates`,
  `c_preprocessor` and `c_diagnostics` compile several hundred small C programs
  through the four phases and check the value `main` returns on the VM, or the
  code and position of the diagnostic that rejects it.
- `c_examples` compiles the five checked-in example programs — an insertion
  sort with binary search, a string library, a table-driven CRC-32, a bit-array
  sieve to 100000 and a stack bytecode VM computing fib(10) — checks each module
  against the raw KIR verifier, and runs it to the result clang produces for the
  same source.
- `c_native_frontend` builds `kcc` as a native binary with `klf build --emit
  frontend`, and asserts that the KIR it writes for every example is byte-for-byte
  the KIR the VM-hosted path produces, that it passes the raw verifier, and that
  it runs to the same result — the same proof `karc_native_frontend` gives for
  karc. It also checks that a program kcc rejects is rejected natively with the
  declared diagnostic.

What remains before M6's exit test is approachable: `#include` and the header
search, which needs a service that lets a phase read an input other than its own
unit; the `#`/`##` operators; floating point, which needs float types and float
arithmetic in the `klf.kir.*` builder; calling a variadic function; designated
initializers, compound literals, `_Generic` and the rest of C17's declaration
surface; the GNU extensions section 29.2 lists; and then the SQLite, Lua, zlib,
musl and Linux-kernel corpus itself, none of which has been attempted.

What IS built is the FFI boundary: a `.klffi` description of a C library is
imported, its `@FFI.Extern` functions become KIR foreign declarations, and a KIR
program that calls one through `call.foreign` runs on both backends — the native
(Cranelift) object links against libc, and the VM resolves the symbol through its
dynamic call trampoline (section 15.6) — returning the C function's real result.
This is a genuine building block (the §8.12 service transport), not the C
definition.

Implemented:

- KIR `call.foreign @sym(args)` (section 26.3) and a `foreign` marker on
  functions (D-133). A foreign function is a bodyless declaration whose name is
  the C symbol, printed and parsed as
  `func <linkage> foreign @sym(params) -> ty engine(native)` with no block body.
  `call.foreign` resolves its callee's signature against that declaration in
  both `klf-kir::infer_value_types` and the verifier; the verifier trusts it
  (arity and argument-type checks only) and skips a foreign declaration's body.
  It round-trips through the text format.
- The unmanaged manifest (section 31.6): `Operation::is_unmanaged` (`asm`,
  `call.foreign`) and `klf-kir::unmanaged_manifest`, which lists every function
  whose body contains an unmanaged instruction. A foreign declaration itself is
  not listed; a caller of one is.
- Native lowering of `call.foreign` (Cranelift, D-133): a foreign function is
  declared with `Linkage::Import` under its raw C symbol and not defined;
  `call.foreign` lowers to a direct platform-convention call. `klf-driver`'s
  native run links the object against libc through the provisioned clang, so the
  call executes. Demonstrated end to end: `abs(-7)` returns `7` and `strlen`
  runs through libc.
- VM lowering and the dynamic call trampoline (section 15.6, D-137): the VM
  lowers `call.foreign` to a call site carrying the C symbol; the interpreter
  resolves it with `dlsym(RTLD_DEFAULT, ...)` and calls it through the platform
  C convention, passing each argument register as a machine word. This is the
  kernel's one unsafe region, gated to non-`wasm32` and reached only from a
  trusted `call.foreign`; the workspace lint is `deny`, not `forbid`, so the
  trampoline module opts in with a narrow `#[allow(unsafe_code)]`. Backend
  parity holds: `abs(-5)` returns `5` on both the VM and native. Integer and
  pointer arguments and results are covered (the libc surface exercised so far).
- `klf-klffi` crate (layer 4, D-134): a self-contained lexer and
  recursive-descent parser for grammar A.3 — all ten `@FFI.*` annotations,
  structs with `@Bits` bit-field widths, `extern`/`address`/`syscall`/`variadic`
  functions, and `let` constants — plus an emitter that round-trips and
  `foreign_declarations`, which maps every `@FFI.Extern` to a KIR foreign
  declaration named by its `symbol:`. FFI scalars map to KIR types
  (`I32`→`i32`, `F64`→`f64`, `USize`→`i64`, `Void`→unit); `CString`, `RawPtr`
  and named FFI types map to `ptr`. Parse errors report in the `KLF-FFI-00xx`
  band.
- `klf ffi gen <header.h>` (section 26.5, D-135): a minimal C declaration
  scanner. It turns integer/string `#define`s into `let` constants, drops
  preprocessor lines and `{…}` bodies, and emits one `@FFI.Extern` per top-level
  prototype, mapping C types to FFI types (`int`→`I32`, `long`→`I64`,
  `size_t`→`USize`, `char *`→`CString`, other pointers→`RawPtr`, …). Typedefs,
  function pointers, variadics and unmapped types are skipped with a remark.
- `klf ffi gen|emit|decls` in the `klf` CLI. `gen` writes or prints a `.klffi`;
  `emit` reparses and reprints one; `decls` shows the KIR foreign declarations a
  `.klffi` maps to.
- Tests: `klf-klffi` parses and round-trips the section-26.1 example, maps its
  externs to foreign declarations, and generates a `.klffi` from a C header;
  `klf-cranelift` runs a `call.foreign` to libc `abs` natively and checks the
  round-trip and the unmanaged manifest; `klf-cli`'s conformance test drives the
  whole path — `.klffi` → foreign declarations → a KIR program calling
  `call.foreign` → native link against libc → the right answer.

Deferred beyond the current M6 work:

- The VM foreign-call trampoline's floating-point path (arguments and results
  in the float registers) and, on `wasm32`, resolving foreign calls to
  WebAssembly imports rather than trapping. The integer/pointer word path runs
  today (D-137).
- The rest of the C definition: the ISO C17 + GNU-extension surface beyond the
  subset above, and the SQLite/Lua/zlib/musl and Linux-kernel corpus that is the
  milestone's actual exit test (D-136).
- `klf ffi gen`'s real engine: running the C definition's preprocessor and
  parser (section 26.5) in place of the declaration scanner, so macros, typedefs
  and function-pointer types translate instead of being skipped with a remark.
- Wiring the unmanaged manifest into the RIDL module metadata section (24.3);
  the KIR-level query exists, the metadata field does not yet carry it.
- The section-26.4 static thunks that convert a `ridlcc` function to a C
  callback type. (FFI struct and union layout by the target's C rules,
  section 26.2, landed in the 2026-10-17 update.)

## Post-M6 groundwork (in progress) — toward real definitions

Not a milestone; the pieces that unblock KLF Kira growing into a language that
can express a definition (`.klfd`), built on the `kira` 1.9.1 reference (D-139).

Implemented and tested (VM + both native backends unless noted):

- Kernel service bridge / host capabilities (D-140): the VM `Capabilities` table
  (`run_with`, `run_module_with_capabilities`) resolves a `call.foreign` name to
  a host function before the dynamic path; `klf-vm::kernel_services` provides the
  `klf.io` set, `klf-native-rt` exports the matching `klfsvc_*` symbols, and
  `kernel/services.manifest` lists them. A KLF Kira program reaches a service
  through a built-in (`putc` → `klf.io.put`) — tested end to end.
- `klf-layout` crate (layer 1, D-141): KIR type size/align/field offsets, the
  section-32.1 split from `klf-ridl`.
- Aggregates (D-141): KIR `field.addr` and `elem.addr` (section 12.6); the VM,
  Cranelift and LLVM lower them (base + offset / base + index·stride), and
  `stack.alloc` sizes nominal/array/tuple types through `klf-layout`. A struct
  and an array KIR program run identically on all three backends.
- KLF Kira structs (D-141): a `struct` declaration becomes a KIR nominal type; a
  struct literal allocates storage and stores its fields; dotted paths read and
  write fields through `field.addr`. Struct locals work end to end on the VM.
  Deferred: struct parameters/returns (a by-value ABI), the growable `Array`
  std type, methods and initializers.
- KLF Kira enums (D-142): an `enum` declaration becomes a RIDL-rule KIR enum;
  leading-dot literals inject fieldless or scalar-payload cases, and exhaustive
  `match` lowers to `switch.enum` with scalar payload bindings. VM, Cranelift
  and LLVM parity are covered. Multiple payloads, defaults, and aggregate
  payloads remain deferred.
- KLF Kira arrays/strings (D-143): fixed integer array literals use address-based
  KIR arrays with `elem.addr` for indexing and assignment. String literals use
  a builtin nominal `{ data, len, cap, ops }` layout and static byte storage;
  dynamic arrays, append/concat/equality and string operation buffers remain
  deferred.
- KLF Kira generics (D-144): generic function declarations lower to KIR generic
  parameters; calls infer concrete arguments, including aggregate addresses,
  and `klf-driver` monomorphizes them before VM or native execution.
- KLF Kira definition vertical (D-145, D-146): integer `as` casts, void service
  calls, and field-wise copies for nested aggregate values allow
  `definitions/calc.klfd` to parse a supplied arithmetic expression, mutate a
  parser struct, call `puti`, and return its result on the VM and native
  backend. This proves runtime execution of KLF Kira source; it is not yet the
  `Language` object ABI or a user-input compiler.
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
  invokes `compile_unit`. KLF Kira and the calculator consume the registered
  file directly, avoiding duplicate source registration on the package path.
- Typed diagnostics (D-152): `Diagnostic` carries a primary `Span` and typed
  note spans, existing labels promote their first span to primary, and
  `Session::report_error` exposes the kernel-side path. CLI rendering uses the
  primary span while preserving the existing labels.
- KLF Kira phase artifacts (D-153): the definition path calls `parse_unit`,
  `elaborate_file` and `lower_elaborated` in order. `ElaboratedFile` owns the
  resolved struct, enum and signature tables; `lower_file` remains the direct
  parsed-file compatibility entry.
- Generic phase boundary (D-154): `DefinitionArtifact` carries a
  `DefinitionUnit` plus a language-owned type-erased payload, and `Definition`
  exposes opt-in `parse`, `elaborate` and `lower` queries. KLF Kira and Calc use
  the boundary; legacy definitions still use `compile_unit`.
- Session-owned artifacts (D-155): `Session` stores immutable phase payloads
  behind `ArtifactId`; `DefinitionArtifact` carries the unit and handle, and
  definitions retrieve payloads through the active session. Artifacts cannot
  outlive their session storage.
- Query cache (D-156): parse and elaborate artifacts are cached by language,
  unit and phase; `Session::invalidate_queries` clears those handles, and KLF
  Kira has a regression test for cache hits. Source files are immutable in this
  slice, so invalidation is explicit.
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
  file invalidation propagates through dependent records and cached artifacts.
  Registered Kira conformances emit typed KIR witness declarations, and the
  KIR text format round-trips those records.
- Definition query and service parity (D-167): Calc records the same phase
  query dependencies as KLF Kira, and `klf-services` parses
  `kernel/services.manifest` into typed ABI signatures with duplicate entry
  checks. The VM capability table is checked against every manifest symbol;
  Session-owned manifest data now drives wrapper enumeration and declarations.
- Mutable inputs and native service coverage (D-168): `Session::replace_file`
  rebuilds a source file and invalidates dependent query records and cached
  artifacts. Native linking smoke-tests manifest I/O and diagnostic symbols;
  the `.klfd` corpus covers source and integer-output symbols.
- Session-owned service registry (D-169): `Session` parses the kernel service
  manifest once and exposes typed lookup to KLF Kira. Wrapper symbols, foreign
  declarations and service-call width adaptation use the active session
  registry instead of a frontend-local manifest cache.
- Session input lookup (D-170): `Session::input(path)` returns current
  registered source text by name, alongside file handles and replacement
  invalidation for query inputs.
- Unified compile entry (D-171): KLF Kira and Calc route direct `compile`
  calls through the package phase boundary, giving CLI and embedding callers
  the same parse/elaborate/lower query records as package callers.
- Runtime diagnostic registry (D-172): `Session` stores typed numeric
  `DiagnosticSpec` entries with duplicate rejection and speculation rollback,
  providing the kernel-side registry for `diag_error`.
- Runtime diagnostic transport (D-173): VM and native runs retain
  `diag_error` calls as typed `RuntimeDiagnostic` records, and the driver
  returns them alongside the existing `Outcome` API. Session spec resolution
  and manifest-driven prelude wrappers are covered by D-174.
- Executable diagnostic boundary and prelude generation (D-174): `Session`
  resolves runtime records atomically through registered specs, the driver can
  report them into a session, and KLF Kira enumerates service wrappers from the
  typed manifest while preserving its language-level facade names.
- Definition metadata and CLI diagnostics (D-175): language descriptors carry
  definition-owned `DiagnosticSpec` records, compilation registers them in the
  Session, and `klf run --input` renders runtime diagnostics against the input
  source file.
- Definition registry and CLI selection (D-176): `klf-services` owns a typed
  registry of `Definition` objects, validates descriptor names and extension
  collisions, and the CLI selects its built-in definitions through that
  registry. The executable `.klfd` loader remains deferred.
- Language construct metadata (D-177): KLF Kira extracts the required
  section-7 `Language` metadata fields from a parsed `construct`, validates its
  shape and enum/string/array values, and reports definition diagnostics. This
  does not execute phase entrypoints.
- Language phase contract (D-178): the metadata loader records spans for the
  required `units`, `parse`, `elaborate` and `lower` method bodies and rejects
  missing, duplicate or declaration-only entrypoints. Invocation is deferred.
- Definition image envelope (D-179): `klf-services` validates a compiled
  module as a `DefinitionImage` carrying its descriptor and exactly one symbol
  for each Language phase. Incomplete images fail before invocation.
- Language phase signatures (D-180): the KLF Kira metadata loader validates
  required phase parameter/result types and rejects generic, bodyless or
  mismatched entrypoints before image binding.
- Definition image invocation (D-181): `klf-driver` runs a validated image
  phase through the normal VM/native backend path using the bootstrap integer
  handle argument transport.
- Executable phase bodies (D-182): `DefinitionImage` rejects foreign imports
  and bodyless declarations, so invocation only reaches executable KIR.
- Source-owned Calc metadata (D-183): `definitions/calc.klfd` carries the
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
  builders and replaces the reserved module slot; a source-level proof compiles
  a Kira `Language` construct and runs its returned module. Native builder
  symbols remain invalid until native receives Session context.
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
- KIR array-bearing nested struct fields (D-238): the struct field-struct
  services accept inner nominal structs that carry fixed i64 or i32 array fields
  alongside scalars. Setting the field copies each array element through
  `elem.addr` and each scalar through `field.addr`, and reading it recovers an
  address-backed struct value whose array fields load back. The source proof
  copies an array-bearing inner struct through an outer struct; nested enum and
  deeper aggregate fields remain follow-up work.
- KIR array-bearing struct enum payloads (D-239): the enum struct-payload
  services (`type_enum_variant_struct`, `value_enum_new_struct`,
  `value_enum_payload_struct`) reuse the D-238 `copyable_struct`/
  `copy_struct_fields` helpers, so a variant can carry an inner struct with
  fixed i64/i32 array fields. The source proof stores and recovers an
  array-bearing struct enum payload; deeper aggregate payloads remain follow-up
  work.
- KIR array-bearing enum fields in structs (D-240): `value_struct_set_enum`
  copies an enum value carrying an array-bearing struct payload into a struct's
  enum field through the shared `copy_struct_fields` helper, replacing the
  scalar-only path. The source proof stores a `Bag::Full(Inner)` enum whose
  `Inner` holds an i64 array into a `Holder` struct field and reads the array
  back after `struct.get_enum`; deeper aggregate payloads remain follow-up work.
- KIR recursive nested aggregate copies (D-241): the struct/enum-payload copy
  helpers now validate and flatten a nominal aggregate into a `CopyStep` plan
  (`struct_copy_plan`), so an inner struct field or enum payload may itself be
  another nominal struct to arbitrary depth (cycles rejected). `copy_struct_fields`
  replays the plan with `field.addr`/`elem.addr`. The source proof copies a
  three-level `Outer{mid: Mid{inner: Inner{value}}}` struct and reads the leaf
  scalar back; nominal fields whose payload is itself an enum remain follow-up.
- KIR trap and unreachable terminators (D-242): `kir_function_trap(function,
  message)` and `kir_function_unreachable(function)` are new definition
  services, wired through the manifest, the KLF Kira prelude wrappers, the VM
  bridge (`terminator_builder`), the VM sentinels and the native stubs. A
  language lowerer can now close dead match arms and panic paths. Two source
  proofs run a const-true conditional whose dead else arm traps or is
  unreachable while the module still returns.
- KIR foreign service calls in generated code (D-243): `kir_value_call_service_i64`
  and `kir_value_call_service_i64_1` let a lowered module emit a `call.foreign`
  to a named kernel symbol (0 or 1 i64 argument) and bind its i64 result. This
  is how a generated language reaches kernel services like `klfsvc_source_len`/
  `klfsvc_source_byte`. Two source proofs build a module that reads the source
  buffer length and a byte through `run_with_source`; wider arities and void/
  non-i64 result services remain follow-up work.
- KIR void service calls and output capture (D-244):
  `kir_function_call_service_void_i64_1` emits a result-free `call.foreign` with
  one i64 argument, and the new `klf_driver::run_with_output` returns a VM run's
  outcome alongside the bytes written to the `klf.io` sink. A generated module
  can now produce output; the source proof calls `klfsvc_io_puti(arg)` and
  asserts the captured `"5\n"`. Wider arities and native output capture remain
  follow-up work.
- KIR f64 value services (D-245): the klf-services `FunctionBuilder` gained a
  `float_binary` method, and new definition services expose `kir_value_i64_to_f64`,
  `kir_value_f64_to_i64` (via the shared convert path) and
  `kir_value_add_f64`/`sub`/`mul`/`div_f64` (a new `float_builder` module). A
  lowerer can now compute in f64 and convert back. The source proof runs the
  full add/sub/mul/div chain with both conversions on the VM; f64 comparisons,
  parameters/returns and float unary ops remain follow-up work.
- KIR f64 constants (D-246): `FunctionBuilder::const_float` plus the
  `kir_value_const_f64(function, bits)` service materialize an f64 literal from a
  passed bit pattern (formatted back to a decimal literal for `ConstFloat`). The
  source proof adds `2.5` to a converted argument and truncates the result. f64
  comparisons still need a new KIR float-compare operation across all backends,
  so they stay out of the incremental service path.
- KIR f64 unary ops (D-247): `FunctionBuilder::float_unary` plus the
  `kir_value_neg/abs/sqrt/floor/ceil/trunc/round_f64` services expose the full
  `FloatUnaryOperator` set. The source proof runs `sqrt`/`neg`/`abs` on an
  argument and `floor`/`ceil`/`trunc` on a `2.5` constant, summing the truncated
  results; f64 params/returns and float comparisons remain follow-up work.
- KIR f64 function signatures (D-248): `kir_function_new_f64`,
  `kir_function_param_f64` and `kir_function_call_f64` reuse the generic
  `function_new_with_type`/`function_param`/`function_call_typed` helpers, so a
  lowerer can build and call f64-returning functions with f64 parameters. The
  source proof builds a doubling helper `(f64) -> f64` and calls it from `main`;
  f64 comparisons still need a new cross-backend KIR float-compare op.
- KIR float comparison operation (D-249): a new `Operation::FloatCompare` with a
  `FloatPredicate` (ordered eq/ne/lt/le/gt/ge) threaded through the whole stack —
  model, text (`fcmp oeq` …), parser, `klf-kir-verify`, and all three backends
  (VM `FCmp` opcode + `float_compare`, Cranelift `fcmp`/`FloatCC`, LLVM
  `fcmp`). Exposed as `kir_value_eq/ne/lt/le/gt/ge_f64` definition services. VM
  and native (Cranelift + LLVM) parity tests agree, and a source proof branches
  a definition on `x < 2.5`. This is the first new KIR op added end to end this
  cycle.
- String constants (D-275): `Operation::ConstString` now verifies as two results —
  pointer and `i64` byte length — and shared inference records both types. The
  VM allocates string bytes as synthetic globals; LLVM and Cranelift emit static
  data and return its address plus length. Definition services expose
  `klf.kir.value.const_string` and `const_string_length`, with a VM definition
  proof and the shared native parity corpus covering the operation.
- Known enum copies in nominal aggregates (D-276): `CopyStep` plans now carry a
  source-ledger variant through `enum.inject` and `enum.payload.addr`, so a
  struct containing a known enum field can be copied through another nominal
  struct or a struct enum payload. Scalar, array and deeper nominal leaves keep
  using typed address copies; unknown runtime variants remain rejected. VM
  proofs cover both nominal-field and enum-payload paths.
- CLI runs and re-checks textual KIR (D-250): `klf run <file>.kir` parses the
  textual module with `klf_kir::parse_module` and runs it on the chosen backend
  (`--entry`/`--backend`/`-- ARGS`), and `klf build <file>.kir` round-trips it
  through the parser and reprints/`-o` the canonical text. This lets a hosted
  language's already-emitted KIR (for example `../karc`'s frontend output) be
  executed from the CLI without a language definition; e.g.
  `klf run karc/examples/fibonacci/app.kir --entry smallcpp_main` prints
  55. A stopgap while `klf-kira-lower` grows to compile real frontends.
- Payload-less enums are first-class values in `klf-kira-lower`: `store_value`
  copies an enum into a struct field or array slot by its tag (via a
  reinterpretation of the destination as its tag integer), and `lower_binary`
  lowers `==`/`!=` on enum operands to a tag comparison rather than comparing the
  aggregate handles. This lets a hosted frontend keep a real token/kind enum in
  its arena AST; `../karc`'s frontend is rebuilt on a `TokenKind` enum this way.
- General control flow and declarations in the low-level KIR builder (D-238,
  D-239): `klf.kir.block.parameter`/`.define`/`.value`/`.switch`, `.branch`,
  `.conditional_branch` with an independent `.else_argument` list, `.trap`,
  `.unreachable`, `klf.kir.switch.case`/`.int`, `klf.kir.signature.declare` and
  `klf.kir.function.body`, plus `klf.kir.value.call_indirect_void`. A definition
  can now build a block with any number of typed parameters, read them back by
  index, branch with edge arguments, switch over integer cases with a default,
  forward-declare a function and give it a body later — with no hosted-language
  type mapping in the kernel. Covered by `klf-driver`'s
  `definition_control_flow` tests: a definition builds a loop, a conditional
  branch with different then/else arguments, a `switch.int` with a trap and an
  unreachable default, and a call to a forward-declared function; the module
  passes the raw verifier and returns 42 on the VM.
- Service symbol coverage is derived from `kernel/services.manifest` (D-240) in
  both the VM capability table and `klf-native-rt`, so every manifest symbol
  resolves and unimplemented ones return the error sentinel.
- Definition package loading and the `Language` executor (D-241, D-242, D-243):
  `klf_driver::load_package` reads every `.klfd` of a package directory except
  under `std/` and `corpus/`, and `klf_driver::LanguageDefinition` finds the
  single declaration extending `Language` (`KLF-DEF-0001` otherwise), reads its
  typed metadata — including the optional `fileModes`, whose entries may be
  keyed by source extension as well as by pragma and flag (D-273) — compiles it
  to a
  definition image, and drives `units`, `parse`, `elaborate` and `lower` on a
  backend. The lowered module is checked against the raw verifier and failures
  are reported as `KLF-KIR-9001` naming the definition. `definitions/mini` is a
  two-file package proving the path end to end: its `lower` builds the module
  structurally through the low-level services, no KIR text involved, and the
  result runs on the VM.
- Real phase artifacts and multiple translation units (D-250, D-251, D-252):
  `units` returns a list built with `klf.definition.input`,
  `klf.definition.unit` and `klf.definition.units_push`/`.units_finish`, one
  unit per input file. `klf.definition.tree` and `klf.definition.program` carry
  a definition-owned payload word that `klf.definition.tree_payload` and
  `.program_payload` hand back, so `parse` produces a real tree, `elaborate`
  consumes it and `lower` consumes the program without reparsing. The value
  lives on the definition's untracked heap and the three per-unit phases share
  one `klf_vm::Machine`, so the heap outlives a single entry call; the machine
  is checked for balance once per unit (`KLF-RT-0100`). The executor merges the
  unit modules, reconciling a declaration against the definition of the same
  name so one unit can call a function another defines.
- Declared diagnostics (D-253): a `Language` may carry
  `diagnostics: [DiagnosticSpec]`, read like `fileModes` and registered before
  the phases run, so a code a phase reports through `klf.diag.error` renders as
  `file:line:col: error[code]: title: explanation`.
- `../karc` is built and run as a `Language` definition package (D-244 to D-249).
  The frontend keeps its own type spellings and materializes them into `KirType`
  handles through `klf.kir.type.*`, memoized per module; values and places are
  kernel handles rather than text; `kir.klfd` is the one module that maps a
  construct to a service call, and the `lower` phase builds the module with
  `klf.kir.module.new`/`.finish`, `klf.kir.signature.*`, `klf.kir.function.body`
  and real block handles. No KIR text is produced anywhere in the path. Two
  services were added for it: `klf.kir.type.parameter` and
  `klf.kir.type.function`, which spell the type the verifier gives `func.ref`,
  so a definition can store, pass and indirectly call a function reference.
  Function values are pointer-sized in the VM, Cranelift and LLVM lowerers, and
  the raw verifier takes a `call.indirect` result from the instruction's own
  annotation. `crates/klf-driver/tests/karc_*.rs` drive the package through
  `LanguageDefinition`: 66 tests over expressions, control flow, aggregates,
  pointers, whole programs and the seven example programs, each compiled through
  the four phases, checked by the raw verifier and run on the VM, plus six tests
  running the lowered programs on the native backend. `karc_units` compiles a
  three-file program where one file calls a function another defines, and
  `karc_diagnostics` asserts the rendered text of a rejected program for a code
  reported from `parse`, from `elaborate` and from `lower`. Definitions no longer run only on the VM: see the native frontend entry
  below.

- Native frontend binaries. `klf build <definition-package>` links a native
  compiler driver for the language the package declares. The definition-facing
  services moved out of the driver into `klf-defsvc`, which has no backend
  dependency: handlers are collected into a `ServiceSink`, the VM copies it into
  its `Capabilities` and the native runtime installs it in a thread-local table
  that its `klfsvc_*` shims dispatch through, so the `klf.kir.*` builder has one
  implementation rather than two (D-254). The `units`/`parse`/`elaborate`/`lower`
  schedule, the cross-unit merge and the raw-verifier check moved with it, behind
  a `PhaseRunner` trait. `klf_cranelift::emit_frontend_object` compiles the phase
  KIR and adds `main`, a `klffe_phase` trampoline and a `klffe_metadata`
  descriptor blob; `klf-frontend-rt` holds the driver loop that reads the inputs,
  builds a `Session`, runs the schedule and writes the KIR (D-255, D-257).
  Whether each phase materializes its artifact is computed at build time and
  carried in the blob, since a native process has no call graph left to walk
  (D-256). `../karc` builds to a `karc` binary that accepts `.cpp` and writes
  `.kir`; `crates/klf-driver/tests/karc_native_frontend.rs` checks that its
  output is byte-identical to the VM-hosted driver's for all seven example
  programs, passes the raw verifier and runs to the same results, and that a
  rejected program renders the same declared diagnostic. The VM remains the
  parity oracle. Not done: a frontend emits textual KIR only — no object, no
  link, no declared flags or file modes — and the frontend is built for the
  host only. The per-unit heap-balance check runs natively as a delta against
  the unit's opening block count (D-260).

- Whole-program rules in a hosted language (D-261). A phase can read any input
  the compilation was given through `klf.source.file_len` and
  `klf.source.file_byte`, behind the `fileText` builtin, and can report a
  diagnostic against a named file with `klf.diag.error_in`, which
  `RuntimeDiagnostic` carries through to the session. Both are implemented the
  same way by the VM and the native runtime. `../karc` uses them from `units`,
  the one phase call that sees every input, to report a prototype no unit
  defines (1032) and a program with no `main` (1033);
  `crates/klf-driver/tests/karc_units.rs` covers both, including the case where
  the prototype is defined by a sibling unit and must be accepted.
## 2026-09-23 update: native executable output from built-in `klf build`

`klf build <file.kir> --emit bin -o <out>` emits an `aarch64-apple-darwin` executable for built-in KIR input and the other source inputs currently compiled by the built-in driver. For `--emit bin`, the default backend is `native`; an explicitly selected backend must also be `native`. The executable is emitted by the existing Cranelift object emitter and linked with `klf-native-rt`. Its process result currently follows the `klfrt_finish` RET/FAIL/TRAP protocol.

This is the built-in source-to-program path only. It does not complete generated frontend `karc app.cpp -o app`, the section 5.3 sysroot/std pipeline, or the normative LLVM backend. D-114 in `docs/decisions.md` records the LLVM deferral; the implementation note in `roadmap/KLF-RIDL.md` does not change those target requirements.

## 2026-09-23 update: generated frontend binary output

Generated language frontends accept `-o app` as native binary output. With no explicit `--emit`, `.kir` and `.klbc` output extensions retain their text-KIR and bytecode modes; an extensionless or other `-o` path selects `bin`. Without `-o` or `--emit`, the default remains textual KIR. `--emit bin` is also explicit. The generated compiler monomorphizes and verifies the module, emits its `main` through Cranelift, and links with the `klf-native-rt` archive. On **this path only** the output is host-only for `aarch64-apple-darwin`; its result protocol remains `RET`/`FAIL`/`TRAP`. The host-only limit is not a property of native emit: `crates/klf-frontend-rt/src/toolchain.rs` selects a target's clang, an Apple target's SDK by `xcrun`, and for any other non-host target the C library from `KLF_SYSROOT_<TRIPLE>` else `KLF_SYSROOT`, so the built-in driver's `klf build --target <triple>` cross-compiles. A cross target gets its C library from that sysroot, which is checked for `libgcc_s` before the link rather than left to fail as `-lgcc_s`; a sysroot without the target's GCC runtime is refused with that named error.

The M-stage frontend obtains the runtime archive through a nested Cargo build in the KLF workspace. This does not satisfy the no-Rust distribution and sysroot contract in sections 5.1–5.4. `FileMode` descriptors and Language memory/convention/exclusivity defaults are carried in generated binary metadata and validated so mode extensions are claimed by the language and assigned only once. VM-hosted and generated native frontends select modes by extension or standalone pragma; generated native drivers also support the declared `<flag>=<glob>` form. Before definition phases run, both paths configure each source's effective memory model and convention, plus its language exclusivity: each selected-mode memory/convention override wins independently, otherwise the Language package default applies. `.klfd` queries effective memory/convention through `session.file_memory(sourceFile)` and `session.file_convention(sourceFile)`; `session.file_mode(sourceFile)` still returns the selected zero-based index or `-1`. The `.klfd` KIR type builders apply default lifecycle policies using that file's memory model and language exclusivity when all member lifecycles are trivial. Affine defaults prohibit copying; Unmanaged and Counted defaults allow trivial copying. Types whose member lifecycles cannot be proven trivial fail explicitly and are not emitted with the placeholder trivial destroy/relocate policy. Each non-foreign KIR function also retains the effective convention of its source file, so merged modules preserve per-file defaults; older KIR text defaults to Platform and RIDL is printed as `abi(ridl)`. LLVM lowers RIDL functions to `swiftcc`, keeps exported/context/failing functions on the existing RIDL path, and uses platform C for foreign imports. Memberwise lifecycle synthesis remains incomplete. Function-pointer types carry Platform/RIDL ABI and context-register parameter markers; LLVM indirect calls honor the ABI and pass context parameters with `swiftself`. Monomorphization synthesizes thunks for matching direct non-foreign scalar signatures, including forwarding one RIDL context pointer from an ordinary Platform parameter. Captured-closure recovery and aggregate, failing, generic and foreign adapters remain unsupported; unknown-provenance pointer-to-function casts fail with `KCC-E0310`, while other unsupported ABI-changing casts are rejected by verification. Native frontends parse declared typed flags, initialize Rust `Session` values from defaults, apply CLI overrides, and show accepted forms/defaults in generated help. `.klfd` code reads typed Boolean, Integer and String values through `session.flag(name)` on VM and native frontends. The shared service bridge copies whole UTF-8 buffers through checked backend heap access; `crates/klf-driver/tests/definition_flags.rs` proves typed flag overrides, mode overrides, package-default fallback, and default KIR policy/ABI parity reach `.klfd` on VM and native paths. `crates/klf-llvm/tests/native_parity.rs` verifies a direct adapter to a RIDL scalar function and a platform callback forwarding context into the RIDL context register. `DriverRequest.arguments` is the normalized token vector; an absent `compatDriver` preserves original arguments for uniform parsing. An explicit `compatDriver` override runs before the arguments are parsed (2026-09-24 update below). The KArc native frontend proof builds `app.cpp -o app` and runs Fibonacci to 55; the existing KIR parity proof still passes.

## 2026-09-23 update: kernel CLI inspection commands

The hosted `.kir` binary path also built `v1-positive.kir` with `--entry choose -- 1 42`; running the output printed `RET 42`.

`klf test <defdir> [--unit] [--corpus <name>] [--language <pkg>]` runs the package's `@Test` functions on the VM (`KLF-TESTING.md`), then builds its toolchain and runs its corpus manifests (`crates/klf-cli/src/test.rs`).

`klf check <definition-package-dir>` compiles and validates a package through the VM definition loader without linking a generated frontend. `klf kir verify|dump <file.kir>` parses KIR and verifies it or prints canonical text; `klf ridl check <file.ridl>` parses RIDL. `klf ridl bindgen --lang rust` selects the existing Rust generator; `--lang swift` emits direct declarations for owned scalar functions whose ABI matches Swift's scalar calling convention, with unsupported RIDL declarations reported as comments. Legacy bindgen invocations still default to Rust. `klf clean` removes the configured `KLF_CACHE` path or `~/.cache/klf`, rejecting filesystem roots, ancestors of home/workspace, and system temporary roots. Only its absent-cache path was exercised in this update. The VM directly ran `v1-positive.kir`'s `choose(1, 42)` and returned `42`.

## 2026-09-23 update: RIDL Swift bindings and compatDriver status

`klf ridl bindgen --lang swift` now emits type-checkable Swift declarations for owned scalar functions using the RIDL `swiftcc` mapping and maps `@Deprecated` to Swift's `@available` attribute. It does not generate aggregate/layout shims; methods, borrowed values, generics, failures, traits, and declarations needing imports or descriptors remain comments. Generated scalar and deprecated-function samples passed `swiftc -typecheck`.

`compatDriver` runs end to end; see the 2026-09-24 update below.

## 2026-09-24 update: compatDriver, adapters, lifecycles, sysroot and corpus runner

- `compatDriver` runs end to end on the VM (`LanguageDefinition::compat_driver`) and in generated frontends (D-285). KLF Kira lowers the override and a generated `__klf_compat_driver` entry; `Optional<DriverRequest>` is a built-in enum instance. Proof: `crates/klf-driver/tests/compat_driver.rs`.
- Payload-carrying enums are stored by copying the live variant (D-286). `KCC-E0310` moved from the raw verifier to the C definition (D-287).
- `call.closure` lowers to `call.indirect` over the RIDL closure record; `closure.callback` makes platform thunks that recover a closure from user data (D-288). ABI thunks cover foreign callees and aggregates by address; failing function pointers propagate through `call.indirect` (D-289). Parity: `klf-llvm` and `klf-cranelift` `native_parity`, `klf-cranelift/tests/foreign_call.rs`.
- Definition-built types with `ref`/`uniq` members get synthesized memberwise `destroy` and implicit `copy` (D-290). Proof: `crates/klf-driver/tests/definition_lifecycle.rs`.
- `klf build <defdir> --out <dir>` assembles a sysroot with the std compiled by the generated driver, and installed drivers link against it (D-291). `klf test` runs corpus manifests (D-292). Proof: `crates/klf-driver/tests/sysroot.rs`.

## 2026-09-28 update: the std seam, used by KArc

- `klf build ../karc --out <dir>` compiles KArc's std (`../karc/std`, C++ in the KArc subset) with the generated `karc` into `libstd.a`, `libstd.so` and `std.klbc`, and installs `std/include` as `include/karc/` (D-297).
- KIR `initializer` functions run before the entry on the VM (a `.klbc` section, library first) and natively (called from the synthesized `main` or a library's load-time init) (D-298).
- Programs link it on both engines: `--emit bin` links `libstd.a`, and `--emit klbc` links `std.klbc` into the program's bytecode (`klf_vm::link`, D-296), so `karc-run` executes one image. Function values use `LEAFN` so they relocate (D-295).
- `heap.alloc.raw`/`heap.free.raw` run on the VM, Cranelift and LLVM, and are reachable from definitions through two builder services; raw blocks stay outside the heap-balance check (D-294). KArc's `malloc`/`free` are written over them.
- The kernel-owned `emit` flag tells a definition whether it links an executable (D-293).
- Proof: `crates/klf-driver/tests/karc_std.rs` builds the sysroot from `../karc`, compiles a program that uses `<cstring>`, `<cstdlib>`, `<cstdio>` and `<cctype>`, and checks the same output natively and on the VM.

## 2026-09-28 update: KLF Kira methods, generic enums and the enum slice

- KLF Kira methods in struct and enum bodies and `extend` blocks, with `self` receivers, static methods, bare member names and sibling calls (D-300); literal top-level constants (D-301); monomorphized generic enums with a KLF Kira prelude for `Optional` and `Result` (D-302); enum cases with several payloads and aggregate payloads (D-303). Proof: `crates/klf-kira-lower/tests/methods_and_generics.rs`.
- KLF Kira `match` on a `String` with string-literal arms and a required `else` arm (D-325). Proof: `crates/klf-kira-syntax/tests/parse.rs` (`match_arms_name_texts_and_else`) and `crates/klf-kira-lower/tests/language.rs` (`match_on_a_string`).
- Low-level enum builder services: declare an enum by handle, declare a concrete specialization with `enum_named`, inject, payload address, tag and `switch.enum` (D-304, D-312). Proof: `crates/klf-driver/tests/definition_enums.rs`, on the VM and natively.

## 2026-09-29 update: Kira loads and passes its corpus

- `../klf-kira` loads and builds to a native `kira` driver. `klf test ../klf-kira` passes 32 cases across its `core`, `examples`, `semantics` and `syntax` corpora: structs and methods, enums and `match`, integer widths and conversions, wrapping operations, overflow and division traps, `Bool` in stack slots and fields, and semantic diagnostics.
- `sdiv`/`srem` accept `[trap]` as section 12.2 states (D-308).
- Package diagnostics render with file and line, phase diagnostics print one per line, and the corpus runner resolves a relative package path (D-309).

## 2026-09-29 update: Kira packages and library imports

- `Session.read` and `Session.readDirectory` are kernel services; KLF Kira reaches them through `readFile` and `readDirectory`, and `as` converts between a handle and `Int` (D-310). Generated drivers expand a directory argument to its source files.
- `klf.source.name_byte` returns the current file's name (D-311).
- `../klf-kira` compiles a package: every file under `app/` is one module, and `import Name` reaches a path dependency whose `moduleRoot` is `Name`. `klf test ../klf-kira` passes 45 cases, 13 of them in the `packages` corpus; `package.kira` is a `construct Name extends Package` declaration.

## 2026-10-01 update: shared services for Kira buffers and scalars

- Concrete type-size queries use shared KIR layout rules. Typed float literals,
  arithmetic, comparisons, negation and conversions are available to definitions
  through the low-level builder (D-313, D-315).
- String literal source spans honor package-file selectors, including literals
  in imported standard-library files (D-314).
- Call-verifier diagnostics identify the callee and expected/actual argument
  counts (D-316).
- Proof: `definition_typed_float` exercises layout and float operations on VM
  and native. The Kira definition's focused strings, floats, arrays, aliases and
  Any corpora exercise imported std source and these services through a native
  generated frontend. This does not complete Kira M7 or the copied 1,510-case
  harness.

KIR string constants now decode escaped UTF-8 when reading textual KIR, so
quotes, backslashes, control characters and Unicode emitted by the printer
roundtrip. The focused decoder regression passes. Kira's `harness-quotes`
source case returns 42 on VM and native, including typed source splices, escaped
string literals and syntax arrays. Generic text-builder services accept UTF-8
bytes produced by a definition without requiring a source literal span.

## 2026-10-03 update: the `.klffi` boundary, every form

- C prototypes on KIR functions, C shims for aggregates by value, variadic calls and Linux system calls, C unions as `record_union` records, bit-fields, owned pointers with their free function, callbacks with a context parameter, and native libraries linked from the package manifest (D-326). Proof: `klf test ../klf-kira --corpus ffi` (25 cases, including `ffi-variadic`, `ffi-context-callback`, `ffi-bit-fields`, `ffi-syscall` and `ffi-klffi`, the last reading its declarations from the `.klffi` its `NativeLibrary` names) and `--corpus ffi-diagnostics` (every FFI diagnostic at its line); `cargo test -p klf-kir` covers the shim source and the system call tables.
- The corpus runner compiles a case's `cArchives` into `lib<stem>.a` before it builds (section 31.1).

## 2026-10-04 update: clocks and the exit function

- `klf.time.monotonic` and `klf.time.wall` answer nanoseconds since the first clock read and since the Unix epoch, from `klf-host-process` on both backends.
- `klf.runtime.at_exit` registers one `(reason, detail) -> unit` function that runs once as the program ends: reason 0 when it returns (detail is the value or exit status), 1 when it traps (the `KLF-RT` code), 2 when it fails (the failure value), and on the VM 3 for a `KLF-VM` fault. The native runtime calls it from `klfrt_finish`, `klfrt_exit`, `klfrt_fail` and `klfrt_trap`, and the VM from `run_with` and `run_process`. It is taken before it runs, so a trap inside it ends the program with that trap. Proof: `crates/klf-driver/tests/runtime_exit.rs` on the VM and natively.

## 2026-10-08 update: every shader language moves to `klf-kira`

- The MSL, HLSL, GLSL and WGSL definitions left `definitions/shaders/` for
  `klf-kira/languages/`, beside KSL's, which moved there on 2026-10-06. A
  shader language is a language: `klf dist` ships none of them, because a
  distribution is `klf` and a user who writes no shader should not carry four
  languages' sources.

- `klf build <package> --language <package>` links the other shader languages
  into a driver, and nobody is bundled. `knvm`'s `toolchain.klfd` lists each
  language's `shaderOuts`, so `knvm binstall` builds six toolchains that each
  write every target their language declares. `klf test` reads the
  `shaderLanguages` of each corpus manifest for the same reason.

- A driver asked for no target writes every target it can write, rather than
  every target the language declares: naming targets it holds no out for
  reported each as one the toolchain cannot express, so a shader that compiles
  answered nothing. `crates/klf-shader-driver/src/lib.rs` and the six tests
  beside `targets`. Entries below that name `definitions/shaders/<lang>`
  describe where it was.

## 2026-10-06 update: `klf.process.invoked_as`, a program's own name

- `klf.process.invoked_as` answers the path the program was started as, its
  `argv[0]`, which `klf.process.argument` excludes. Both backends answer from
  `klf-host-process`; the VM runner records the program path it was handed. A
  launcher installed under several names reads which one it was run under from
  here: `klf-kira`'s `tools/launcher` dispatches `kira`, `ksl` and `klf` that way.

## 2026-10-06 update: `klf.host`, naming the host to a program

- `klf.host.key_length` and `klf.host.key_copy` answer this machine's
  `<arch>-<os>` key, both backends, from `klf_target::llvm_host_key` — the same
  string the framework names its provisioned bundles by, so one machine is
  spelled one way across a toolchain tree. A length and a copy rather than one
  call, because the key has no fixed size and a program hands these services a
  buffer it owns; the same shape as `klf.text.float` and `klf.process.argument`.
- Nothing answered this before, so a program that had to name its host had to
  reach past the service set for `uname`. That is the case `klf.vm`'s own version
  manager is: it fetches the artifact a release published for one machine and
  cannot choose without being told what machine it is on. A raw syscall is the
  platform's to make, not an app's. Proof: `crates/klf-vm/tests/services.rs`
  (the length/copy pair agree) and `../klf-kira`'s `tools/knvm-core`, whose only
  `@FFI.Syscall` this removed.
- A host the framework does not name reads as `unknown`, which is a non-empty
  answer: a caller asking "is this a host I can install for" still has to say no.

## 2026-10-04 update: editor facts (KLF-EDITOR E0–E1)

- `klf-text`: positions in UTF-8, UTF-16 and UTF-32 columns, line indexes and
  incremental document edits.
- `klf-facts`: the token, symbol, reference, hint, fold, link and runnable
  vocabularies, the fact store and its queries, `EditorSpec` with validation,
  the Appendix C pattern dialect with its engine translations, and the lexical
  classifier generated from a spec.
- `klf.editor.*` services on both the VM and native paths. A `Session` carries
  an `AnalysisMode` and the facts its phases recorded; each phase's facts are
  committed with the files it read, whether the phase succeeded or not. In
  `Build` mode every recording service returns 0. KLF Kira reaches the
  integer services as `editor_*` wrappers and the text-taking ones as
  `editorSymbolText`, `editorTypeAt`, `editorValueAt`, `editorHint`,
  `editorLink` and `editorRunnable`.
- The `editor` member of a `Language` declaration is read from its
  `EditorSpec` literal, checked (`KLF-DEF-0040`, `KLF-DEF-0041`), stored in
  the `LanguageDescriptor` and carried in a native frontend's metadata. A
  definition without one gets `EditorSpec.minimal(name)`. Proof:
  `crates/klf-kira-lower/tests/editor_spec.rs` and the metadata round trip.
- `klf-ide` and `klf-lsp`: the language server over an analysis host. Open
  documents replace their files for every phase read (`Session::set_overlay`).
  A project is the nearest directory with one of the editor spec's root
  markers, else the workspace folder, else the file alone. One analysis runs
  at a time, in a worker process for generated drivers; a change cancels the
  running analysis of its project and requests wait for the current one.
  Served from facts: semantic tokens (lexical tokens from the editor spec with
  the recorded ones painted over), definition, declaration, type definition,
  implementation, references, highlights, hover, completion, signature help,
  outline, workspace symbols, rename, inlay hints, folding and selection
  ranges, document links, code lenses for runnables, call hierarchies and
  push diagnostics. Proof: `crates/klf-lsp/tests/protocol.rs`.
- `<driver> lsp` in every native frontend. The server starts
  `<driver> lsp-worker` for each analysis, which runs the phases in `Editor`
  mode over the request's project with the open documents as an overlay and
  writes the diagnostics and facts back, so a trap in definition code ends the
  worker and not the server. Proof:
  `native_frontend_analyzes_for_the_language_server` in
  `crates/klf-driver/tests/native_frontend.rs`, where `definitions/mini`
  records a symbol and a token.
- `klf lsp` (`klf-kernel-ide`): one server for `.klfd`, `.kir`, `.ridl` and
  `.ridlsig`, picking the editor spec per file extension. A directory holding
  `language.klfd` is compiled as a definition package with its open documents;
  every `.klfd` file gets an outline of its declarations. `.kir` files get
  parse and verifier diagnostics, an outline of functions and blocks, and
  navigation for `%values`, blocks and `@functions`. `.ridl` files get parse
  diagnostics. Proof: `crates/klf-kernel-ide/tests/kernel.rs`.
- `klf editor gen <driver-or-package> [--out <dir>]` (`klf-editor-gen`):
  the editor packages of section 19 from a language's editor spec. One tree
  holds `manifest.json`, a TextMate grammar, `language-configuration.json`,
  a tree-sitter grammar with `parser.c`, an external `scanner.c` for nested
  block comments and strings, and queries, a `.vsix`, a Zed extension, a
  Neovim plugin, Helix `languages.toml` with queries and an Emacs mode.
  Proof: `crates/klf-editor-gen/tests/generate.rs`, where the grammar is
  compiled, loaded and parses with its queries.
- The Zed extension installs as a dev extension. Its highlight queries use
  Zed's capture names (`@comment.doc`, `@preproc`, `@namespace`), and when the
  language has no grammar of its own, `klf editor gen` commits the generated
  `tree-sitter` directory to a local git repository and pins
  `extension.toml` to `file://` and that commit, since Zed fetches grammars
  by commit. Proof: `the_zed_extension_uses_zed_capture_names_and_a_pinned_grammar`.

## KLF-KSL: the shader pipeline (S0–S7)

Status: S0 to S7 pass their exit tests on Linux. S8, hardware parity, is not
started: it needs Metal, D3D12 and GPUs this host does not have. The parts of
S6 that need Apple's `metal` compiler (acceptance by `metal`, Metal against
Vulkan compute parity) are likewise untested here.

Implemented:

- S0: `klf-shader`, the KSIR model, layout rules (`std140`, `std430`,
  `scalar`, `msl`, explicit), the builder and the text form. Proof: the
  layout and fragment-parser unit tests in `crates/klf-shader/src`, and
  `the_section_18_example_is_valid_and_round_trips` in
  `crates/klf-shader-valid/tests/validate.rs`.
- S1: `klf-shader-valid`, SV1 to SV24, `ModuleInfo`, uniformity and constant
  evaluation, with keyed diagnostics. Proof:
  `crates/klf-shader-valid/tests/validate.rs`.
- S2: passes, binding assignment, reflection, and the SPIR-V and MSL
  backends. Proof: `crates/klf-shader-passes/tests/passes.rs`,
  `crates/klf-shader-spirv/tests/spirv.rs` (every unit through
  `spirv-val`). The MSL backend's tests went with it in S10.
- S3: the `ShaderLanguage` family with `keys` and `capabilities`, the
  `klf.shader.*` builder services, the shader driver loop, `klf build` of a
  shader definition into a sysroot (`bin/<driver>`, `share/<lang>/library`,
  `definition.lock`), and `klf ksir verify|dump`. The `ksl` definition
  (`klf-kira/languages/ksl`) builds all 34 corpus shaders, and its reflection and
  `.resources` match the oracle's (D-334, D-337).
- S4: the HLSL, GLSL and WGSL backends over a shared text writer, checked
  through `glslangValidator` and naga; replaced by outs in S10.
- S5: `klf-kira`'s `ksl!` runs `ksl build` and reads its output (D-336).
- S6: `kmsl`, the MSL definition, under `definitions/shaders/msl`
  (DS-020). `klf test definitions/shaders/msl` passes 65 cases: five accepted shaders
  (textures, atomics and threadgroup memory, function constants kept and
  baked, depth comparison and fragment built-ins, packed vectors) and one
  refusal per diagnostic code. The MSL `ksl` writes for the 34 corpus
  shaders builds through `kmsl` for every target and pass `spirv-val`, naga (WGSL) and `glslangValidator`
  (GLSL and HLSL).
- S7: `--overrides keep`, `--bounds restrict|read-zero-skip-write`,
  `--debug-info`, and capabilities narrowed per module and refused per
  target (D-333). Proof: `kept_overrides_*`, `restrict_*` and
  `read_zero_skip_write_*`, `a_declared_capability_the_module_does_not_use_is_dropped`
  and `a_capability_the_target_cannot_express_is_refused` in
  `crates/klf-shader-passes/tests/passes.rs`;
  `debug_info_places_statements_in_their_source` in
  `crates/klf-shader-spirv/tests/spirv.rs`.
- `klf test` runs a shader definition's corpus (D-349). Proof:
  `diagnostics_read_code_and_line` in `crates/klf-driver/src/corpus.rs` and
  `klf test definitions/shaders/msl`.

- S9: outs written in KLF Kira (section 5.8) and the bundled toolchain
  (5.9). A shader definition may declare `emit(unit: ShaderUnit) -> Bool`;
  the driver calls it per unit through Cranelift selector 6, and it reads
  the unit through `klf.shader.read.*` (`crates/klf-shader-out`, D-354).
  Every frontend binary is indexed definition objects plus a registry
  (`crates/klf-cranelift/src/lower/registry.rs`, D-357); a shader
  toolchain holds every bundled definition, so `kmsl --target ksl` runs the
  `ksl` definition's out (`definitions/shaders/ksl/definitions/out`).
  Proof: every valid ecosystem shader converts KSL to KSL and rebuilds,
  and every one with entry points converts KSL to MSL to KSL and rebuilds;
  `crates/klf-shader-out/tests/view.rs`; `klf test definitions/shaders/ksl`
  and `klf test definitions/shaders/msl` (`flow-ksl` writes flagged loops
  and a switch).
- S10: the MSL, HLSL, GLSL and WGSL outs (`definitions/shaders/<lang>/
  definitions/out`) over the shared `definitions/shaders/text` library
  (section 22.10, D-359). The Rust text backends and `klf-shader-text` are
  deleted. Proof: before the deletion, every corpus shader and KSIR fixture
  either target accepted came out byte-identical from both, with and
  without `--debug-info` and `--overrides keep`.
- S11: `klf dist` installs `bin/klf-shaders`, the links `ksl`, `kmsl`,
  `khlsl`, `kglsl` and `kwgsl`, `share/ksl/library` and
  `definitions/shaders` (`crates/klf-driver/src/shader_sysroot.rs`). A
  graphics MSL file may hold one stage alone (code 102 now refuses only a
  second `vertex` or `fragment`), so the MSL written for a vertex-only
  shader reads back. Proof: every ecosystem KSL shader the `ksl` link
  accepts builds for all six targets, and its MSL and KSL outputs build
  again through `kmsl` and `ksl` for all six; every SPIR-V passes
  `spirv-val` and every GLSL and HLSL `glslangValidator`; `klf build` of a
  shader definition from the distribution links the copied definitions.
  `klf test definitions/shaders/msl` covers `depth-only`.

Known limits: under `read-zero-skip-write` a whole struct or array read out
of range is clamped, not zeroed (D-338). GLSL cannot place a `packed_float3`
member at offset 4, and WGSL refuses a uniform struct of size 28 with
alignment 16; both are refused with a diagnostic. The MSL layout asserts of
`--debug-info` are unchecked without a Metal compiler (D-342).

## 2026-10-05 update: one formatter for every language (KLF-FORMAT)

- `crates/klf-format`: the kernel's formatting engine. A definition
  describes a layout (tokens, groups, indentation, line breaks); the engine
  matches each token against the source, places comments read with the
  `EditorSpec` comment syntax, keeps one blank line where the source had
  some, fits groups to the width, and diffs the result into whitespace
  edits. A layout that drops, adds, reorders, joins or splits a token is
  rejected (`KLF-LSP-9002`).
- `klf.format.*` services on the VM and native paths. The `format(session,
  unit, tree) -> Bool` hook runs after `parse` in the same heap, through
  `__klf_format` (trampoline selector 7).
- `LanguageDefinition::format_source` on the VM, `<driver> format` in native
  frontends, and formatting and range formatting in the language server
  through `<driver> format --stdin`.
- Proof: `crates/klf-format/src/tests.rs`; `crates/klf-driver/tests/format.rs`
  formats nested lists with comments and blank lines through a KLF Kira
  hook on the VM.
- KSL and MSL format through a shared token layout
  (`definitions/shaders/format`, `KLF-FORMAT.md` section 7), and `klf format`
  runs a definition's hook on the VM. Proof: `ksl_formats_*` and
  `msl_formats_*` in `crates/klf-driver/tests/format.rs`.
- Kira (`../klf-kira`) formats through the same layout, which gained
  struct literals, one-line blocks, implicit members and a byte order mark.
  A format run sets `emit` to `format`, so Kira's `parse` reads the file
  alone. Proof: `kira_formats_*` in `crates/klf-driver/tests/format.rs`, and
  all 377 `.kira` files of `../klf-kira`: the 365 that parse format with no
  rejection and are stable, the other 12 are the syntax corpus's
  intentionally broken files.
- Not done: a formatter for KArc, on-type formatting, `.Format`
  editor corpus checks, a native test of `<driver> format`, shader outs
  printing through the engine (`KLF-FORMAT.md` section 10).

## 2026-10-05 update: cancellable definition code (KLF-EDITOR E1)

- `klf-cranelift` gives definition objects unwind tables on ELF hosts
  (`unwind.rs`), cleanup pads that release the counted and unique SSA values
  a frame owns (`lower/owned.rs`, `lower/calls.rs`), and a poll at every
  function entry and loop back edge (D-360, D-361). Program objects are
  unchanged.
- `klf-native-rt::interrupt`: `klfrt_poll`, `klfrt_personality`, and traps,
  uncaught failures and kernel services that raise when the thread serves a
  language server. `catching` at the phase boundary restores the call depth
  and frames, and the phase's records are discarded.
- `<driver> lsp` analyzes in its own process on ELF hosts, on the main
  thread (D-362), and reports whether the heap is balanced after each
  cancellation. Elsewhere it keeps `<driver> lsp-worker`, which a
  cancellation now kills.
- The VM stops a run whose machine is cancelled at its next call or
  backward jump with `KLF-VM-0030`, and unwinds its frames.
- `klf-lsp` schedules analyses (`schedule.rs`): requests the user waits on
  first, then other requests, then the diagnostics of open documents after
  a 50 ms debounce. A write cancels the running analysis of its project.
  Semantic tokens, inlay hints, code lenses, document symbols and folding
  ranges get `ContentModified`, and other requests are retried at the new
  revision. `$/cancelRequest` answers `RequestCancelled`. A trap fails the
  request with `RequestFailed` and `KLF-LSP-9001` (D-364).
- Proof: `crates/klf-lsp/tests/cancellation.rs` (the protocol rules and the
  debounce); `crates/klf-vm/tests/cancellation.rs`;
  `crates/klf-driver/tests/native_cancellation.rs`, which builds
  `tests/spin` and drives `<driver> lsp`. In it, writes during a long loop,
  a deep recursion and an allocating loop, in each of `parse`, `elaborate`
  and `lower`, release within 20 ms with the heap balanced. Traps in each
  phase fail their request with `KLF-LSP-9001`, shown once per phase, and the
  server keeps answering.
- Not done: tables for Mach-O and COFF; releasing counted values held in
  memory (D-363); rolling back speculation state inside a phase, which no
  phase uses yet.


## 2026-10-05 update: shaders in the editor (KLF-EDITOR 14)

- `<driver> lsp` works for a shader language. `analyze` runs the shader
  schedule (`drive_shaders`) instead of the program one, and `new_session`
  gives the session its `ShaderSettings`, without which the shader builder
  refused every module a `lower` produced. Both were broken: a shader `lsp`
  died with `Mismatch(Lower)` and then with `KLF-SHD-9001`.
- `klf-shader-ide` records what a shader module shows, from the module, its
  binding assignment and what the definition wrote (sections 14.3 and 14.4,
  D-365):
  - a resource is a symbol with the group, the neutral `(group, binding)`,
    its paired sampler, its Metal length slot, any authored slot and a table
    of every configured target's slot, checked against the bytes the outs
    write: `[[buffer(1)]]`, `b0`, `binding = 0`, `@group(0) @binding(0)`;
  - an option is a symbol with its type, default, id and whether the targets
    keep or bake it;
  - a stage interface member is a symbol with its location, interpolation,
    sampling and blend source; a built-in carries its name in KSL and its
    spelling on every target;
  - a host-shareable struct is a symbol with its layout rule, size,
    alignment and a table of its members, and each member and each gap before
    one is a `Layout` hint (`offset 16 · size 4`, `12 bytes padding`).
- The pipeline runs in the analysis, over every target the language lists, in
  memory only. A refusal carries its target in the diagnostic's `source`, so
  the client shows `ksl (hlsl): ...` beside the driver's own diagnostics.
- `editor` members on the `ksl` and `msl` definitions (sections 21.1 and
  21.4), so both serve real token classification: their lexers' reserved
  words, the words the parsers read by position as contextual, the built-in
  tables as a `Builtin` group, and for MSL `[[...]]` attributes and `#`
  preprocessor lines as patterns.
- A struct member carries the span it was declared at (D-367), which KSL's
  lowering now sets for its types, members, resources and options.
- Proof: `klf test definitions/shaders/msl` (69) and
  `klf test definitions/shaders/ksl` (4); `ksl lsp` over a project
  publishes the definition's diagnostics and answers a hover on
  `uniform camera: Camera` with the slot table above, and the layout hints on
  a uniform struct's members. Scoped nextest over `klf-shader*`, `klf-ide`,
  `klf-lsp`, `klf-kernel-ide`, `klf-defsvc` and the `klf-driver` native
  frontend tests passes; clippy is clean on every crate touched.
- Not done: section 14.5 output views (`klf/shaderOutput`,
  `klf/virtualFile`, `klf-shader-spirv`'s `disasm`) and 14.6 build on save.
  A resource hover shows the group's name but not the group's class, and a
  sample call shows no level (section 14.3's last row).

- Section 14.5's output views, and 14.3's two remaining rows (D-368, D-369):
  - A shader declaration records a runnable of kind `Shader`, which the code
    lens turns into a `klf.showOutput` command naming the configured targets.
  - `klf/shaderOutput` answers with a `klf-virtual:` URI and a language id;
    `klf/virtualFile` returns that view's text. Every target's source, the
    KSIR text, each target's reflection and the SPIR-V as disassembly are
    views. `klf-shader-spirv`'s `disasm` decodes the instruction stream and
    names the opcodes the writer emits, so no external tool is needed.
  - `klf/shaderOutputChanged` names the URIs whose content moved with a
    revision, and `klf/status` reports health and indexing. `initialize`
    lists all four under `klf.features`.
  - A resource hover now names the group's class, and a sample call shows the
    level it lowers to (`Auto`, zero, explicit, bias or gradient, DS-009).
- Verified by driving `ksl lsp` over a project: the lens appears on
  `shader DepthOnly` listing `msl,spirv,hlsl,glsl_430,wgsl,ksl`, and
  `klf/shaderOutput` for `hlsl` answers
  `klf-virtual:/tmp/shadertest/DepthOnly.vert.hlsl` with `languageId` `hlsl`,
  whose text is the HLSL the `hlsl` out writes. The SPIR-V view disassembles
  to real instructions (`%32 = OpMatrixTimesVector 3 32 28 31`).
- Still open in section 14: 14.6 build on save, which needs the `project` hook
  of section 5.3 to name the output directory, and a client that dispatches
  `klf.showOutput` (DE-010's VS Code client is still unwritten).

## 2026-10-05 update: unit tests in KLF Kira (KLF-TESTING)

- `KLF-TESTING.md`: a definition package's `tests/*.klfd` `import Testing`,
  mark functions `@Test`, and check with `expect!(condition)`. They compile
  into the package's image, so a test calls the definition's helpers
  directly, and `klf test` runs each one on a fresh VM machine. `--unit` runs
  only the tests, and a package without corpus manifests builds nothing.
- `Testing` drives the language under test: `run`, `runFiles`, `runPath` and
  `runPaths` compile and run `main`, `compiles` and `compilesFiles` only
  compile, `formats(source, expected)` checks the format hook,
  `diagnosed(code, line, column)` and `reported(text)` check the
  last compilation's diagnostics. They sit over the `klf.test.*` services,
  which only `klf test` serves.
- KLF Kira parses `name!(...)` as a macro call and `"""` string literals
  across lines, without the closing delimiter's indentation. `expect!` is the
  first macro.
- `LanguageDefinition` lowers its image to bytecode once instead of on every
  `compile_sources`, so a process that compiles many programs with one
  language pays for it once.
- The karc behaviour tests moved out of `klf-driver` into
  `../karc/tests/*.klfd` (36 tests) and `../karc/corpus/programs.klfd` (the
  karc std program and a native fibonacci application). The `c_*` suites are
  gone with `../klf-c`, which is retired: C goes through KArc. The karc
  suites took 777s under nextest because every test process reloaded the
  package; `klf test ../karc --unit` takes about 10s.
- Native links on Linux pass `-lm`: `libklfrt.a` and the bytecode runner
  carry the shader validator, which calls libm.
- The KSL, MSL and Kira formatter tests moved out of `klf-driver` into
  `definitions/shaders/{ksl,msl}/tests/format.klfd` and
  `../klf-kira/tests/format.klfd`. A formatter now reuses the definition's
  cached phase program. `klf-cli`'s `distribution` test builds a release
  distribution, so it is ignored by default (`--run-ignored all`).
- Native-linking tests run in parallel: a nextest setup script builds the
  kernel archives once, so the tests' own `cargo build` calls find them
  fresh and never rewrite one under a link. The workspace suite went from
  226s to 15s: native links use the release runtime archive and, off
  macOS, the bundled lld.

## 2026-10-07 update: RIDL on the VM

- A module built for the VM speaks RIDL through the same `_RIDL1…` entries a
  native build does. Its natives library holds the RIDL entries of what it
  exports (named `<Module>.<name>` for C) and the adapters of what it
  imports, so a VM program calls a native RIDL library, and a native program
  or another VM program calls a VM module, with neither side knowing how the
  other runs.
- `klf_vm::Instance` is one program with a machine, heap and globals of its
  own. Its natives libraries are bound to it through `klf_vm_bind`, so each
  entry calls `klfvm_invoke(instance, function, words, results)` on the
  instance that owns the function; a gate admits one thread at a time and
  lets the thread inside call back in. A RIDL module's natives library runs
  `klfvm_attach` as it loads, which loads its image beside it into a new
  instance and runs its initializers: a plugin is a `.klbc` a program links.
- A `.klbc` named as a package's library links `<image>.natives.so` (or
  `.dylib`) with an rpath to it. A native program that links one takes
  `libklfvm.a` in place of `libklfrt.a` and exports `klfvm_invoke` and
  `klfvm_attach`; the runner exports both too. Natives libraries link
  `-Bsymbolic` with their own soname, and a machine resolves imports in its
  own libraries before the process.
- A `.klbc` library import may start at a function other than the first
  (klbc import kind 3), so a linked image keeps every library's indices;
  std's import paths are absolute after linking, and std links into RIDL
  module images as well as programs.
- Kira gives the buffer operations' `reallocate` and `free` a C prototype, so
  the ops table a VM module hands across RIDL holds native entries rather
  than VM function indices.
- The runner writes `klfsvc_io_write` to stdout; VM programs printed nothing
  before.
- `klf test` corpus cases take `engine = "vm"` (built with `--emit klbc`, run
  by the runner) and `kiraImages` (library packages built into `.klbc`
  beside them). Proof: `../klf-kira/languages/kira/corpus/ridl.klfd` runs one
  RIDL surface native to native, VM to native, VM to a VM plugin and native
  to a VM plugin, with the same output.

## 2026-10-09 update: every client CLI says what it is doing

- `klf-diag` owns the whole reporting facility: the progress channel
  (`klf_diag::progress`), the phase recorder (`klf_diag::timeline`), and the
  status surface (`klf_diag::status`) with the `out!`/`err!`/`raw!`/`raw_err!`
  print macros every client CLI prints through. It is a kernel crate, not a
  `klf`-binary one, because it is not the `klf` binary's display: every generated
  driver links `klf-frontend-rt` and runs the same surface, so a KLF Kira driver
  and a KArc driver report exactly as `klf` does, from one implementation.
- `klf-frontend-rt` installs the surface around the driver's compile and around a
  shader driver's compile, and names its artifact: `Check: Ok`, `Built: Ok
  <artifact>`. A driver that runs a program stands the surface down first, since
  the program owns the terminal from there.
- A command that compiles names its phases at every boundary worth naming —
  reading the package, `parse`/`elaborate`/`lower` per unit, verification,
  codegen, linking, the driver and std of a sysroot, bytecode lowering — each
  reported as it starts, through `progress!` at the boundaries in `klf-services`,
  `klf-defsvc` and `klf-driver`. A library consumer installs nothing and pays one
  uncontended read lock.
- The animation is a title line whose elapsed seconds repaint on a 50ms ticker,
  so the timer stays live inside a phase that reports nothing for seconds, with a
  256-colour band sweeping across the title's words on each repaint. The timer
  itself is never styled, so the numbers do not flicker between shades. The
  ticker repaints that one row and nothing else: repainting the whole block at
  that rate is what flickers, so the phase rows are repainted only when the phase
  list changes. Colour is off under `NO_COLOR` and a `dumb` terminal, and the
  whole surface is erased when the command returns.
- The ticker holds a `Weak` handle, so it exits on its own once the last surface
  is gone; progress is best-effort and a failed spawn is silently no ticker
  rather than a failed command.
- `--timings` (on `klf` `run`, `build`, `check`, `test`, `format`, `editor gen`,
  `dist`) replaces the animation with a record: every phase goes to stdout as
  one line with the milliseconds it took, in the order it ran, with the total
  and the unattributed remainder closing the list. Nothing is animated, nothing
  erased, no terminal needed. Phases that overlapped both claim the same
  milliseconds and the report says so.
- `Check: Ok`, `Built: Ok <artifact>` and `Generated: Ok <language>` go to stdout
  once the work is over, with `Failed` after the colon when the command did not
  succeed. `klf build <file>` writing KIR to stdout prints nothing else on that
  stream: it installs no surface at all rather than lines a reader must strip.

## 2026-10-10 update: the Kira Live Runtime's wire and its two-version VM

- `klf-live` is a new crate holding section 17.5's hot module push: the frame
  codec (`u32 length, u8 type, payload`; HELLO, PUSH_MODULE, ACK, NACK, LOG),
  the TCP transport that frames them off a socket, the client that pushes, and
  the `Device` that drives an engine through the exchange. A push carries KIR
  and the device lowers it (D-371), so one build serves a device on any KLF
  release rather than the one its compiler had.
- The `Engine` trait is the seam a live device is built around: `install` a
  module, `swap` a running one for a new version keeping its values, `call` an
  entry, `reap` what nothing is running. `VmEngine` is its VM body. The native
  and hybrid bodies are written against these same signatures; where this lowers
  KIR and hands it to an interpreter, they re-sign and launch a binary for the
  same KIR.
- The VM now holds two code versions (D-372). A frame records the version it
  entered, so a push never disturbs frames already running; `Machine::adopt`
  installs a pushed version only when the slot it retires is idle, and reports
  `PushBusy` for the device to answer with `OLD_CODE_BUSY`. `Instance` exposes
  the same, so a swap goes through the gate that admits one thread at a time.
- `klf push <file> <host:port>` is section 5.5's hot module push client. It
  compiles to KIR, opens the exchange with a hello, compares the device's
  layout of the module against its own, pushes, and reports what the device
  said. A device may be a tailnet address like any other.
- Proof: `crates/klf-vm/tests/hot_push.rs` (a pushed version is what the next
  call enters, globals keep their values, a third push is refused while the
  retired version runs, and a version is reaped once idle) and
  `crates/klf-live/tests/{exchange,transport}.rs` (the whole exchange including
  a layout change refused as needing a restart, a release device refusing every
  push, every frame type round-tripping, and a push travelling over a real
  socket from a client to a device and back). `klf-live/examples/device.rs` is
  a device on a loopback socket that answers and reports what the new code
  returns.

## 2026-10-11 update: the live engine seam fits native and hybrid devices

- `Engine` is now `build` a pushed module into an `Artifact`, `install` the
  first version, `swap` a new one in, `run` an entry as a `RunRequest`, and
  `reap` what nothing runs (D-373). An artifact is bytecode, a binary, or
  bytecode with its native half, named by a hash of what the device built.
  `Signer` is the step a native or hybrid device takes between building a
  binary and launching it: it re-signs the binary with the device's identity.
  A `RunRequest` carries the `argv`, `envp` and directory that device gives
  the process. Only `VmEngine` has a body; it decodes the bytecode artifact it
  built, so what runs is what the digest names.
- A device starts with `Device::install`, and a push naming a module it does
  not hold is refused with `UNKNOWN_MODULE`. The device's `Bundle` is the id
  its hello reports and whether it accepts pushes at all.
- Proof: `crates/klf-live/tests/exchange.rs` adds the unknown-module refusal
  and the outgoing version being retired once no frame runs it.

## 2026-10-11 update: `push` reaches a running VM device

- Every driver has `<driver> push <host:port>` (section 5.5): it compiles the
  package in the current directory to KIR, says hello, and pushes the module.
  A layout the device holds differently is refused on the client before
  anything is sent. The address may be any the device listens on, a tailnet
  one included.
- `VmEngine::with_sysroot` links the language's std bytecode into what it
  builds, from `lib/<lang>/bytecode/<ptrwidth>/std.klbc` (section 5.4).
- The VM supplies native entries for the functions C calls (D-375): a pool
  of 64 integer-result and 64 float-result entries, bound at install to each
  `__klf_vm_<name>` import no loaded library defines.
- A pushed version's foreign call sites are rebased onto the running import
  table; a push calling a native the device never imported is refused.
- `Device::answer` and `Device::serve` run the device side of the exchange,
  one client after another.
- Proof: a Kira package pushed with `kira push 127.0.0.1:47100` to the
  `klf-live` `device` example changed what its `main` returned, twice.
  `crates/klf-vm/tests/hot_push.rs` covers the import rebase and refusal, and
  `crates/klf-live/tests/transport.rs` a device serving a second client.

## 2026-10-14 update: the kernel optimizes KIR in two tiers (section 14.1)

- `klf_kir_passes::optimize(module, level, backend)` runs tier 1 at O1 or
  O2 and then tier 2 for the backend. Functions are optimized callees first
  over the call graph, each simplified before its callers inline it.
- Tier 1, O1: inlining of trivial functions, constant folding, copy
  propagation, dead code and dead block parameter elimination, and CFG
  simplification (unreachable blocks, forwarding blocks, single-edge
  chains). Blocks end in reverse postorder.
- Tier 1, O2: adds promotion of scalar stack slots to SSA values, scalar
  replacement of struct slots and of whole-struct `mem.copy`, redundant
  load elimination (a load, or the `enum.tag` of an address, that an
  earlier store or load in the block or its one predecessor answers),
  removal of slots nothing reads, and inlining by a cost model. A function
  on a cycle of calls inlines only into callers off the cycle.
- Tier 2: Cranelift inlines by a larger budget and repeats tier 1 over the
  result; LLVM takes the tier-1 result unchanged; the VM repeats tier 1's
  simplification without further inlining.
- The Kira frontend's native build runs O2 and the Cranelift tier when the
  `release` driver flag is true, and O0 otherwise.
- Proof: `crates/klf-kir-passes/tests/optimize.rs` runs a loop and a struct
  slot at O0, O1 and O2 on the VM with the same results, and checks that O2
  leaves no slot, call or load behind. A Kira loop summing ten million
  `Number`s went from about 250 ms to about 80 ms; KiraDB's tests pass in a
  release build.
- Not done in section 14.1: generic specialization of hot polymorphic calls,
  devirtualization, loop-invariant code motion, borrow and copy conversion,
  destroy hoisting, uniqueness-check elision, retain and release pairing,
  and `--profile release` as a second way to ask for a release build.

## 2026-10-15 update: `Number` loops run as fast as `Int` loops (section 14.1)

- Tier 1, O2: loop versioning. A loop whose header parameter enters as a
  constant and keeps it on the paths that usually run, as a whole
  `Number`'s scale stays zero, gets a copy specialized for that constant.
  Every edge that may change the parameter goes to an unspecialized copy, so
  the specialized loop folds the tests of the value.
- Tier 2, Cranelift: a whole-word sum or difference checked by the sign of
  `(a ^ r) & (b ^ r)` or `(a ^ b) & (a ^ r)` becomes `add.ovf` or
  `sub.ovf`, read from the machine flag. A value a loop uses and a call
  after the loop needs is stored before that call and loaded after it, so it
  keeps a scratch register in the loop. Values an overflow edge passes to a
  block reach it through stack slots stored on the edge, so a slow path's
  call no longer spills the loop's own values. The edge reloads an extended
  load and recovers the left operand of the overflowed operation from its
  wrapped result rather than keeping them alive.
- The VM runs `add.ovf`, `sub.ovf` and `mul.ovf`, signed and unsigned, at
  every width.
- Proof: `crates/klf-kir-passes/tests/optimize.rs` checks the flag rewrite
  and the slot hand-off, and that both functions return what O0 returns on
  inputs that overflow. `crates/klf-vm/tests/parity.rs` checks each
  overflowing operation against Rust's at 64 and 8 bits. A Kira loop summing
  bytes into a `Number` compiles to the same loop as the `Int` one: one
  load, one `add`, one `seto` test and the counter.

## 2026-10-15 update: compile-time calls into another unit fold after the merge (section 14.4)

- A unit whose compile-time call reaches a function only another unit
  defines no longer evaluates it alone, where the call looked foreign and
  trapped with `KLF-VM-0024`. The unit records each such function, with its
  buffer result, as a `klf.comptime.deferred` module attribute. The units'
  modules merge, keeping the attributes, and the merged module folds the
  deferred calls.
- Merging units: the empty body a unit gives a function it lowers but never
  reaches (no signature, one block that only returns) yields to the unit
  that defines the function, instead of a "defines in more than one unit"
  conflict.
- Proof: `crates/klf-defsvc/src/evaluation/deferred.rs` checks that the
  deferred functions and their buffer results round-trip through module
  attributes. KiraDB's sync engine, whose units fold a `Number` table
  through Foundation's arrays, builds and passes its 26 tests.

## 2026-10-16 update: smaller frames for inlined code (section 14.1)

- Cranelift's native stack slots are no longer zeroed. Definite
  initialization (section 11.6) stores to a slot before anything reads it,
  so the memset each slot paid at entry did nothing.
- The inliner moves a callee's `stack.alloc`s to the caller's entry only
  when the call is on a cycle of the caller's control flow graph, where the
  slot would otherwise run again each time round. A call off any cycle keeps
  its slots where its body lands, so a cold error path no longer adds to the
  hot path's frame. A callee may now hold slots in any block that is not on
  a cycle.
- Proof: KiraDB's `ByteCursor.readVarint` inlined a 77 KB frame and spent
  80% of a point read zeroing it. Point reads rose from 1,164/s to 12,215/s
  on x86_64 before any change to KiraDB.

## 2026-10-17 update: `klf build --profile` (sections 5.2, 14.1)

- `klf build --profile debug or release`. `debug`, the default, runs only
  the mandatory passes; `release` runs tier 1 at O2 and tier 2 for the
  backend over what the build compiles: the definition's own KIR before
  Cranelift emits a frontend or a sysroot's driver, a module built with
  `--emit bin` after monomorphization, and a module written with
  `--emit kir`, tier 2 then shaping it for `--backend` (Cranelift when none
  is given). The profile is part of the key of a frontend object kept in
  the cache.
- Proof: the Kira definition (`../klf-kira/languages/kira`) built at both
  profiles checks the Build library with the same output, and the release
  driver does it in 3.0 s against 3.4 s, cache off. Both drivers write the
  same KIR for a program with a class.

## 2026-10-17 update: retain and release pairing (sections 14.1, 14.3)

- Tier 1, at O1 and O2, cancels a `copy` of a `ref T` or of a type whose
  policy says `pairing: counted` against the `destroy` of the copy or of the
  original that follows it in the same block. Between the two may stand
  only borrows of either value (`copy`, `ref.addr`, `borrow.begin`/`end`,
  `ref.unowned`) and instructions that can neither release a value nor read
  a count; a call of any kind, `raw.store`, a write through an address, a
  destroy of another value or `ref.isunique` keeps the pair. The copy's
  uses then read the original.
- Proof: `crates/klf-kir-passes/tests/pairing.rs` runs both cancelling
  forms and both kept forms on the VM at O0, O1 and O2 with the same
  results, `ref.isunique` included. The Kira definition's release KIR keeps
  its 14,729 copies: each hands a second owner on through `into.raw` or a
  return, so none has a release to pair with in its block, and the rebuilt
  driver checks the Build library with the same output.

## 2026-10-17 update: atomics and `fence` on every backend (section 12.14)

- `atomic.load`, `atomic.store`, `atomic.rmw` with each of its eleven
  operators, `atomic.cmpxchg` (strong and `[weak]`) and `fence` lower on the
  VM, Cranelift and LLVM. The VM runs one thread: a load and a store are
  its plain ones, `atomic.rmw` and `atomic.cmpxchg` its `ARmw` and `ACas`
  instructions, and `fence` a no-op. Cranelift's atomic instructions are
  sequentially consistent, which every weaker order allows. LLVM gets its
  own `load atomic`, `store atomic`, `atomicrmw`, `cmpxchg` and `fence` with
  natural alignment; an order LLVM forbids where it stands becomes the
  nearest stronger one, and a relaxed fence emits nothing.
- Value typing gives `atomic.cmpxchg` its two results, the value read in
  the operand's type and an `i1`, in `klf_kir::infer_value_types` and the
  VM's own typing.
- Proof: `ATOMICS` in the shared parity corpus (`crates/klf-llvm/tests/
  corpus.rs`) runs every operator on an `i64` and an `i32`, a failing and a
  succeeding compare-exchange and two fences, folding every value
  read into the result; the VM agrees with Cranelift and with LLVM for four
  seeds.

### 2026-10-17 update: the runtime half of the cycle detector

- `klf-rt` keeps, in a debug heap, a trace map for each object given one
  (`Heap::trace`) and records a candidate root on every release that leaves
  such an object alive. A freed object leaves both tables.
  `Heap::leaked_cycles` runs Bacon and Rajan's synchronous trial deletion
  (mark gray, scan, scan black) over the candidates and never frees. It
  reports each group of objects only kept alive by references among
  themselves as one cycle through it, spelled
  `Type.field -> Type.field -> ...`.
- `klf_layout::trace_map` builds a type's trace map from its layout: every
  `ref` field, through nested structs and fixed arrays, named like `a.b` or
  `x[2]`. Enums, `uniq` and weak references are skipped.
- Open: section 18.6 says the compiler emits the trace map "next to the
  destroy function", but the spec gives no bytecode opcode, runtime entry
  point or byte layout to hand it to the runtime. So neither the VM nor the
  native backends register one yet, and the byte layout `klf-rt` reads is
  internal for now.
- Proof: `crates/klf-rt/tests/cycles.rs` reports a two-object cycle only
  once the outside reference is gone, a self-reference, a cycle without the
  leaf it holds, and nothing for a freed object.

### 2026-10-17 update: the cycle detector on the VM

- `klf_vm::lower_module_traced` lowers a debug build: for each `HALLOCC` of
  a type with strong references it keeps the type's trace map in a
  `Traces` table beside the program, keyed by function and allocation
  shape. The table never goes into `.klbc`, because the spec gives trace
  maps no encoding. `lower_module` is unchanged and records nothing.
- `klf_vm::run_traced` registers each map and its allocation site
  (`function+pc`) when the allocation runs. A clean return that leaves blocks
  live prints each leaked cycle to standard error, for example
  `KLF-RT-0101: leaked cycle Node.next -> Node.next -> Node.next (allocated
  at pair+0, pair+1)`. Under `Cycles::Error` the run then fails with
  `KLF-RT-0101`; a leak with no cycle in it keeps `KLF-RT-0100`.
  `Cycles::from_env` reads `KLF_CYCLES` (`off`, `warn`, `error`).
- Proof: `crates/klf-vm/tests/cycles.rs` checks four cases. A self-linked
  node and a two-node cycle fail with `KLF-RT-0101`. A plain leak keeps
  `KLF-RT-0100`. A balanced run returns. An untraced lowering reports no
  cycles.

### 2026-10-17 update: aggregate values on every backend (section 12.6)

`struct.new`/`extract`/`destructure`, `tuple.new`/`extract`/`destructure`,
`array.new` and `enum.new`/`take`/`project` hold an aggregate as an SSA value,
which none of the three backends keeps in registers. A shared KIR to KIR step,
`klf_kir::aggregates_to_memory` (`klf-kir/src/aggregates/`), runs before the
VM, Cranelift and LLVM lower a module: every aggregate value gets a stack slot
at the top of the entry block and its SSA names become that slot's address.
The construction instructions store through `field.addr`, `elem.addr` and
`enum.payload.addr`/`enum.inject`; extraction loads through them; `move`,
`copy`, `destroy`, `store` and `load` of an aggregate become `move.addr`,
`copy.addr` or memberwise copies, and `destroy.addr`. A block parameter of
aggregate type becomes an address that the block moves into its own slot,
so a value carried around a loop is not overwritten by the next iteration.
A module or private function with no failure type, named only by direct
calls, takes an aggregate parameter by address and returns an aggregate
through a leading out address; a function the step cannot rewrite, and every
function whose calls depend on its signature, is left unchanged.

Supporting changes: `field.addr` accepts a tuple address on all three
backends (`klf_layout::Layouts::tuple_offsets`), the analysis infers result
types for `destructure`, `enum.take` and `enum.project`, and the VM's
`copy.addr`/`move.addr` copy the pointee's size rather than one word.

Proof: `klf-vm/tests/aggregate_values.rs` runs the corpus in
`klf-llvm/tests/aggregates_corpus.rs` (struct, tuple, array and enum values,
a struct passed to and returned from a function, a struct carried around a
loop, a stored and reloaded struct) against hand-worked results, and the
`aggregate_values` parity test in both `native_parity.rs` suites compares
Cranelift and LLVM with the VM on all eight functions.

### 2026-10-17 update: `select`, `fma`, raw pointers and layout queries on every backend

`select`, `fma`, `raw.load`, `raw.store`, `load.borrow`, `addr.toptr`,
`ref.unowned`, `ptr.offset`, `ptr.diff` and `type.size`/`align`/`stride` now
lower on the VM (`klf-vm/src/lower/unmanaged.rs`, emitting the `SELECT`,
`FFMA`, `PTRADD`, `PTRDIFF`, `LOAD`, `STORE` and `MOV` opcodes and a constant
for a layout query), Cranelift (`klf-cranelift/src/lower/unmanaged.rs`) and
LLVM (`klf-llvm/src/emit/unmanaged.rs`, `select`, `@llvm.fma.f32`/`f64`,
`getelementptr i8` and `ptrtoint`/`sub`). A layout query answers from
`klf-layout`; the stride is the size rounded up to the alignment. Before
this the roadmap listed `select` and `fma` as lowered by the VM and
`load.borrow` and `ref.unowned` by Cranelift; only interpreter opcodes and an
ownership check existed.

Proof: `klf-vm/tests/unmanaged_values.rs` runs the corpus in
`klf-llvm/tests/unmanaged_corpus.rs` against hand-worked results, and the
`unmanaged_values` parity test in both `native_parity.rs` suites compares
Cranelift and LLVM with the VM on eight cases.

### 2026-10-17 update: VM opcodes in their section 15.5 slots

Every VM opcode now has the number section 15.5 assigns it. Moved: `LEAG`
(the VM's `GlobalA`) to 0x07; `CTLZ` through `BITREV` to 0x24-0x28, after
`ADDO`/`SUBO`/`MULO`; `MCPY`, `MMOVE` and `MSET` to 0x5A-0x5C; `WLOAD` to
0x67, `ISUNIQ` to 0x69, `ACCBEG`/`ACCEND` to 0x6D/0x6E; `ARMW`, `ACMPXCHG`
and `FENCE` to 0x82-0x84. Atomic loads and stores emit `ALOAD` (0x80) and
`ASTORE` (0x81) instead of `LOAD` and `STORE`. The VM's own `FPTOSI.SAT` and
`FPTOUI.SAT` opcodes (0x52, 0x53) are gone: a saturating conversion is
`FPTOSI`/`FPTOUI` with overflow mode 2 (saturate) in flag bits 4-5, the
section 15.4 encoding of arithmetic overflow. `.klbc` files written before
this change decode to different instructions and must be rebuilt; the tree
holds none.

Proof: `klf-vm/tests/opcode_numbers.rs` checks every opcode against the
section 15.5 table, numbered in order from the start of each range, and that
no opcode outside the table decodes; the VM, parity and Kira lowering suites
pass unchanged.

### 2026-10-17 update: overflow-reporting arithmetic and `call.foreign` on LLVM

`add.ovf`, `sub.ovf` and `mul.ovf` lower on LLVM through
`@llvm.{s,u}{add,sub,mul}.with.overflow`, shared with trapping arithmetic.
`call.foreign` lowers to a C-convention call of the import's C symbol: a
foreign function is now declared under its C name (`klf_kir::c_symbol`)
rather than `kir.<name>`, so it links against the C library, and two imports
of one symbol share one declaration. `func.ref` of an import names the same
symbol.

Proof: the `unmanaged_values` VM test and both parity suites gained
`absolute` (`labs` through `call.foreign`) and `checked` (all three
overflow forms, with and without overflow).

### 2026-10-17 update: loop-invariant code motion and witness devirtualization (section 14.1, O2)

Two O2 passes join the tier 1 simplification loop in `klf-kir-passes`:

- `optimize/hoist.rs` moves loop-invariant instructions out of every natural
  loop whose header is entered from one block by a plain `br`: an instruction
  whose operands are all defined before the loop (or by instructions already
  moved) goes to the end of that block. Only instructions that cannot trap
  and touch no memory move: constants, wrapping integer arithmetic other
  than division, comparisons, float arithmetic, `select`, `fma`, pointer
  arithmetic, layout queries and the conversions other than trapping
  float-to-integer ones.
- `optimize/devirt.rs` turns `call.witness T, Trait.method(args)` on a
  concrete `T` into a direct call of the function in the method's slot of
  the module's `witness T : %Trait` table.

The optimizer's context now carries the bodiless module, so a pass can read
its traits and witness tables.

Proof: `klf-kir-passes/tests/optimize.rs`
`loop_invariant_arithmetic_leaves_the_loop` (the square of a loop-invariant
parameter moves to the entry block at O2, and O0, O1 and O2 return the same
sum) and `o2_calls_a_concrete_witness_directly` (no `call.witness` remains,
and the VM runs the result).

### 2026-10-17 update: uniqueness-check elision (section 14.1, O2)

`optimize/unique.rs` answers `ref.isunique` with `true` for a counted object
allocated earlier in the same block when nothing but `ref.addr` has used the
reference since, so nothing could have copied it.

Proof: `klf-kir-passes/tests/optimize.rs`
`o2_knows_a_fresh_allocation_is_unique`: of two checks, the one before a
`copy` folds and the one after stays, and O0 and O2 return the same answer.

### 2026-10-17 update: pairing across blocks and lent arguments (section 14.3)

- A `copy` now pairs with releases past its own block: a path may leave
  through `br`, `cond_br`, `switch.int` or `switch.enum` into a block that
  only that edge enters, up to four blocks on, and the copy cancels when
  every path reaches a `destroy` of the copy or the original with only
  borrows and passable instructions on the way.
- At O2, an owned counted parameter of a module-private, non-generic,
  infallible function that nothing names but a direct call, read only
  through borrows and destroyed once just before each `return`, becomes
  `borrow`: the callee's destroys go and each call is followed by a destroy
  of the argument. When the callee can neither release nor read a count (no
  calls, stores, destroys of other values or `ref.isunique`), a caller that
  copied the value only to pass it passes the original and drops the copy.

Proof: `klf-kir-passes/tests/pairing.rs` `released_on_each_path` cancels a
copy released on both arms of a `cond_br` at O1 and returns what O0 does;
`an_argument_the_callee_only_destroys_is_lent` turns `@peek`'s parameter
into `borrow ref i64` with no destroy, leaves `@caller` without its copy, and
returns 42 at O0 and O2 with `ref.isunique` after the call seeing one owner.

### 2026-10-17 update: destroy hoisting and copy sinking (sections 14.1, 14.3)

- At O2 a `destroy` moves up to just after the last use of its value, or
  of any address, borrow or unowned reference made from it, and a `copy`
  moves down to just before the first use of either value. Each crosses
  only pure instructions, `ref.addr`, `borrow.begin`/`end` and
  `ref.unowned`: nothing that can release a value, read a count, or run a
  copy or destroy function. A module with `attribute lifetimes = "scoped"`
  keeps its destroys where the frontend put them (section 14.1's last
  paragraph); copies still sink.

Proof: `klf-kir-passes/tests/motion.rs` returns the same value at O0, O1 and
O2 with and without the attribute; at O2 the copy sits right before the call
that takes it and the destroy right after the call that last reads the
reference, while under `lifetimes = "scoped"` the destroy stays at the end.

### 2026-10-17 update: borrow-to-copy and copy-to-borrow (section 14.1)

- At O2 `load.borrow` of a scalar becomes `load [trivial]` and
  `borrow.begin` of a scalar goes, each with its `borrow.end`.
- A `copy` of a value whose type copies `implicit(@f)` and is not counted,
  whose copy is only copied, borrowed or passed to parameters a known
  callee borrows and then destroyed in the same block, while the original
  is used only the same ways, is replaced by the original: the copy and
  its destroy go. Section 4's policy table lets the ownership pass insert
  such copies at will, so removing one is the same freedom. Counted types
  stay with pairing.

Proof: `klf-kir-passes/tests/convert.rs` drops `@copied`'s copy and destroy
of a `%Pt` passed to a borrowing callee at O2, and turns `@borrowed`'s
`load.borrow` into a plain read that returns 42 at O0, O1 and O2. `@copied`
is checked by shape alone: the aggregate rewrite cannot yet move a `%Pt`,
whose copy is a function, into storage.

### 2026-10-17 update: vectors on every backend (section 12.5)

- `klf_kir::vectors_to_lanes` splits each `vec<N x T>` value into its `N`
  lanes before a backend lowers the module, and `klf_kir::legalize` runs it
  ahead of the aggregate rewrite; the VM, Cranelift and LLVM call
  `legalize`. `vec.splat`, `vec.extract`, `vec.insert` and `vec.shuffle`
  only pick lanes; arithmetic, comparisons, `select` (with a scalar or a
  per-lane condition), `fma` and conversions happen once per lane; a load
  or store of a vector is one per lane through `elem.addr`; block
  parameters take the lanes. A private function only called directly
  takes a vector parameter as its lanes and returns a vector as a tuple of
  them, which the aggregate rewrite moves into storage.
- Value typing gives a vector comparison a `vec<N x i1>` result and
  `vec.shuffle` a vector as long as its mask. The verifier accepts the
  elementwise forms section 12.5 allows: integer and float arithmetic,
  comparisons, unary operations, `fma`, conversions between vectors of the
  same length, and `select` over vector arms with an `i1` or `vec<N x i1>`
  condition. `.ovf` stays scalar.

Proof: `klf-llvm/tests/vector_corpus.rs` (`lanes`, `clamped`) builds,
shuffles, adds, compares, selects, widens, stores, loads and passes vectors
through a private function; `klf-vm/tests/vector_lanes.rs` checks the
hand-worked results, and `vector_lanes` in both `native_parity.rs` suites
gets the same answers from Cranelift and LLVM. The verifier's matrix adds
elementwise forms to `vector.positive.kir` and refuses a `sext` that changes
the lane count (`v2-vector-elementwise.negative.kir`).

### 2026-10-17 update: indirect branches and RIDL variadics (sections 12.13, 12.14, 29.5)

- `klf_kir::indirect_branches_to_switches`: `block.addr bbN` becomes the
  block's position in its function plus one as a `ptr` (`inttoptr`), and
  `br.indirect %a, [...]` a `ptrtoint` and a `switch.int` over its listed
  blocks whose default is a new block holding `unreachable`. A block
  address names a block of its own function (the verifier's rule), so the
  numbers never need to mean anything elsewhere.
- `klf_kir::variadics_to_areas`, following section 29.5's RIDL rule: a
  module function a `call.variadic` names or that runs `va.start` takes
  one hidden trailing `ptr`; each `call.variadic` of it stores its variadic
  arguments into consecutive 8-byte slots (16 for 128-bit integers and
  floats) of a stack area and passes the area; `va.start` stores the area
  pointer into the list, `va.arg T` reads `T` under the list's cursor and
  moves it one slot on, `va.copy` copies the cursor, and `va.end` does
  nothing. An exported platform-convention function keeps the platform's
  variadics and a function `func.ref` names keeps its signature.
- `klf_kir::legalize` runs both ahead of the vector and aggregate
  rewrites, so every backend gets them. Value typing now types a
  `call.variadic` result from its callee.

Proof: `klf-llvm/tests/indirect_corpus.rs` (`dispatch`) picks a block
address with `select`, carries it through a block parameter and branches
to it; `klf-llvm/tests/variadic_corpus.rs` sums three `i64` slots in a loop
(`summed`) and reads an `i32` and an `f64` slot, the `f64` twice through a
`va.copy` (`mixes`). `klf-vm/tests/indirect_branches.rs` and
`variadic_areas.rs` check the hand-worked results, `indirect_branches` and
`ridl_variadics` in both `native_parity.rs` suites get them from Cranelift
and LLVM, and `klf-kir-verify/tests/legalized.rs` verifies all three
corpora before and after `legalize`, with none of the rewritten
instructions left.

### 2026-10-17 update: inline assembly (section 12.14)

- `Operation::Asm` now holds the three parts section 12.14 spells,
  `asm "template", "constraints"(args)`: an `InlineAsm` with the template,
  the constraint string and the operand values. The parser splits at the
  first comma outside quotes and reads the rest as a call; a bare
  `asm "template"` has no constraints or operands. The printer writes the
  same text back, and use/rewrite walks see the operands.
- LLVM lowers it to `call void asm sideeffect "template", "constraints"(...)`
  with each operand at its own type; a quote, backslash or non-printing
  byte is written as `\XX`. Cranelift and the VM keep refusing it, which is
  what "native engine only" asks.
- Gap: section 12.14 gives `asm` no result type, so an `asm` that names a
  result is refused with a diagnostic rather than given a guessed type.

Proof: `klf-llvm/tests/inline_asm.rs` prints and reparses both spellings
(`asm_prints_and_parses_back`), checks the LLVM text including the escaped
quotes (`asm_becomes_llvm_inline_assembly`), checks the refusal of a result
(`an_asm_result_is_refused`), and runs a function holding a `nop` and a
`mov` with a clobber natively on aarch64 macOS (`asm_runs_natively`, 41 in,
42 out).

### 2026-10-17 update: unwinding with no handler (section 12.14)

The remains list said Cranelift lowered `resume` and `throw` and every
backend but the VM lowered `invoke`; none lowered any of them. Cranelift's
unwind tables serve only its interrupt pads (`KLF-EDITOR.md` section 8.6).

- `klf_kir::unwinding_without_handlers`, first in `legalize`: in a function
  with no `catch` clause, no frame can stop an unwind, so, as the
  platform's two-phase unwinder does when its search finds no handler, a
  `throw` or `resume` ends the run with `KLF-RT-0020` (an exception
  reaching the boundary) and no cleanup runs. An `invoke` becomes a `call`
  (its result, which the verifier delivers to no block, destroyed when not
  trivial) and a branch to its normal block, and the blocks only unwind
  edges reached are dropped with their `landingpad`s.
- A function with a `catch` clause is left as it was and every backend
  refuses it: section 12.14 says neither what `catch %typeinfo` is matched
  against nor what the `i32` a `landingpad` yields holds. That gap is in the
  remains list.

Proof: `klf-llvm/tests/unwind_corpus.rs` has `guarded` `invoke` a callee
that throws for a nonzero argument, with a cleanup pad that resumes.
`klf-vm/tests/unwind_traps.rs` checks 0 returns 7 through the normal edge
and 1 traps `KLF-RT-0020`, and that a function with a `catch` clause is
left alone and refused; `unwinding_without_a_handler` in both
`native_parity.rs` suites gets the same outcomes from Cranelift and LLVM;
`klf-kir-verify/tests/legalized.rs` verifies the corpus before and after
`legalize` with no unwinding instruction left.

### 2026-10-17 update: `setjmp` and `longjmp` (section 12.14)

- VM: `SETJMP` (0x89) and `LONGJMP` (0x8A), in the slots section 15.5
  gives them. `setjmp` writes five words into its buffer (a mark, the
  number of frames below its own, its frame's depth and function, and the
  resume point with its answer register) and answers 0; `longjmp` leaves
  the frames above that one, freeing their stack slots and running nothing
  else, and resumes it answering its value, or 1 for 0. A buffer without
  the mark, or whose frame has returned, faults with `KLF-VM-0001`: the
  section names no trap for it. In a function that runs `setjmp` no two
  values share a register and the bytecode passes (local registers,
  constant propagation, slot promotion, inlining into it) leave it alone,
  since a `longjmp` resumes it with its registers as they are.
- LLVM: the C library's `setjmp`, called `returns_twice`, and `longjmp`,
  called `noreturn` with its value resized to `i32`.
- Cranelift refuses both with that reason: its register allocator may give
  a value's register to another value on the path to the `longjmp`, and it
  has no returns-twice call to stop it.

Proof: `klf-llvm/tests/jump_corpus.rs` has `caught` save its place, call
down two frames and have the deepest jump back with `7n + 1`, its value
before the `setjmp` used in a block laid out ahead of the path that jumps.
`klf-vm/tests/jumps.rs` checks 122 for 3, 94 for -1 and 101 for a jump of
0; with register sharing left on, the first case returns 29, which is the
fault the rule prevents. `set_and_long_jumps` in the LLVM parity suite
gets the same answers natively, and the Cranelift suite checks the refusal
(`set_jump_is_refused_with_its_reason`). `opcode_numbers` places both
opcodes.

### 2026-10-17 update: concrete witness calls on every backend (section 12.11)

Section 12.11 says a `call.witness` is "resolved statically when T is
concrete"; only the O2 devirtualization did it, so below O2 the call
reached backends that lower none.

- `klf_kir::witness_calls_to_direct`, first in `legalize`, turns
  `call.witness T, Trait.method(args)` on a concrete `T` whose witness
  table the module declares into a direct call of the function in the
  method's slot. `Module::witness_callee` finds that function, and the O2
  pass (`optimize/devirt.rs`) now calls it rather than its own copy.
- A witness call on a generic type, or through a table from another
  module, still reaches the backends; it needs the section 23 descriptors
  and witness tables, listed under 13 and 23.

Proof: `klf-llvm/tests/witness_corpus.rs` (`describe`, unoptimized) calls
two slots of a declared table; `klf-vm/tests/witness_calls.rs` checks 48
for 20, `concrete_witness_calls` in both `native_parity.rs` suites gets 48
from Cranelift and LLVM, and `klf-kir-verify/tests/legalized.rs` verifies
the corpus after `legalize` with no `call.witness` left.
`o2_calls_a_concrete_witness_directly` still passes through the shared
lookup.

Also fixed: a Cranelift worker whose function was refused halfway kept the
builder context of that function and asserted in Cranelift's frontend on
its next one, so a refusal could surface as a panic depending on
scheduling (seen with `set_jump_is_refused_with_its_reason`). The worker
now starts a fresh context after a refusal.

### 2026-10-17 update: C layout of FFI records and aliases (section 26.2)

Section 26.2 lays `@FFI.Struct` and `@FFI.Union` out "by the target's C
rules, including bit-fields", and resolves `@FFI.Alias` "through
`CLayout`"; no `CLayout` existed, and named FFI types all became `ptr`.

- `klf_target::CLayout` (`Target::c_layout`) tables every 1.0 target of
  section 5.8: the size and alignment of `short`, `int`, `long`,
  `long long`, pointers and `size_t`, `wchar_t`, `float`, `double` and
  `long double`, whether `char` and `wchar_t` are signed, the
  `long double` format (as `double`, x87, binary128) and the bit-field
  rule (Itanium, or Microsoft on Windows, MinGW included). A Tessera
  target takes its architecture's Linux rules (section 5.8).
- `klf_klffi::record_layout` lays out a record: fields at their
  alignment, bit-fields packed by the target's rule, a union's fields all
  at 0, nested records, `@FFI.Array`, pointers and callbacks.
  `type_declarations` makes each record an `explicit c` KIR type with
  those offsets, size and alignment, and `foreign_declarations` now takes
  the target's rules and gives an alias of `long` its width there (`i32`
  on Windows, `i64` on Linux). A bit-field wider than its type, a
  non-integer bit-field and a record holding itself are refused.

Proof: `klf-klffi/tests/c_layout.rs` compiles the same records with
clang for all 13 triples of the 1.0 targets (both Apple and both Windows
architectures, Android, Linux, Tessera, `wasm32` bare and Emscripten) and
checks clang's own size, alignment and bit offset of every field against
`record_layout`, and every scalar fact of `CLayout` with
`_Static_assert`. The run found that Emscripten aligns its 16-byte
`long double` to 8, which the table now says. The same test round-trips
and verifies the `explicit c` types and checks `labs(long)` as `i32` on
Windows and `i64` on Linux.

### 2026-10-17 update: a native test of `<driver> format` (`KLF-FORMAT.md` 10)

`<driver> format` in native frontends had no test of its own; only the
VM-hosted formatter was checked.

Proof: `native_frontend_formats_as_the_vm_does`
(`klf-driver/tests/native_frontend.rs`) builds a native frontend from the
KLF Kira definition `format.rs` formats with on the VM, now shared as
`klf-driver/tests/lists.klfd`, and gets the VM's three expected results
from `format --stdin NAME --width N`. On a file it fails `--check` and
prints the path, rewrites the file in place, and then passes `--check`.

### 2026-10-17 update: the editor corpus runner (`KLF-EDITOR.md` Appendix A, 24.4 item 2)

`klf test` now runs a definition's editor corpus. `read_editor_corpora`
reads every `corpus/editor/*.klfd` manifest, and `run_editor_corpus`
handles each case: it writes the files to a scratch directory without
their `$0`, `$<name>` and `$[name`…`$]` markers, then starts the native
`<driver> lsp`, initialized with UTF-8 positions. It opens the files and
waits for diagnostics, then runs each check over the protocol:
diagnostics, definition, references, hover, completion, signature help,
rename, semantic tokens against a golden file, folds, code actions,
formatting, and edits with nested checks, after which the edit is
reverted. `klf test` runs the program corpus as before, and runs the
editor corpus when the package has one.

Proof: `the_editor_corpus_runs_against_the_drivers_language_server`
(`klf-driver/tests/native_frontend.rs`) builds a native frontend from a
small KLF Kira definition (`tests/names.klfd`). That definition
classifies tokens, declares symbols with documentation, records
references, reports a registered error and formats. A case whose checks
are all correct passes, including the golden token file, rename, both
edits and `.Format`. In a wrong case, a wrong definition target fails
with the location it got, and `.Symbols` fails because no golden format
is given.

Gaps the spec leaves, recorded rather than invented:

- The Appendix A samples (`at = "$0"` arguments, struct fields without
  commas) do not parse under grammar A.1. The runner reads A.1: commas
  between fields, `label:` arguments, or positional arguments in the
  order of the Appendix A table.
- The `.Tokens` golden format is not given. The runner writes
  `line:col length type modifiers`, with 1-based lines and byte columns,
  after the `"<code>:<line>"` convention of `KLF-RIDL.md` 31.1.
- `.Symbols` and `.Hints` name golden files whose format is not given,
  so they fail with that reason.
- Appendix A does not say where the name of a `$[name` range ends. The
  runner ends it at the first character that is not a letter, digit or
  `_`, so a range whose text starts with one of those needs another
  marker between: `$[r$0value$]`.
- The crate table of `KLF-EDITOR.md` section 26 places the runner in the
  test-only `klf-lsp-test`, which does not exist yet. The runner is in
  `klf-driver` (`src/corpus/editor*.rs`, `lsp_client.rs`) next to the
  program corpus runner `klf test` already calls; moving it is still to
  do.

### 2026-10-17 update: unwind tables for aarch64 Mach-O (`KLF-EDITOR.md` 8.6)

Definition objects for `aarch64-apple-darwin` now get the same tables as
ELF ones: frame entries in `__TEXT,__eh_frame` and the landing tables in
`__TEXT,__gcc_except_tab`. The linker builds `__unwind_info` from them.
They are written in the form the assembler uses for that format:

- 8-byte PC-relative function and LSDA pointers, each a SUBTRACTOR and
  UNSIGNED relocation pair;
- the personality read through a GOT entry (`ARM64_RELOC_POINTER_TO_GOT`);
- the LSDA pointer made against a local symbol rather than the section.

`ld64.lld` refuses the ELF form (`unexpected personality encoding 0x1b`)
and crashed on a section-relative LSDA pointer. With the tables in place,
`<driver> lsp` on an aarch64 Mac analyzes in its own process, as on ELF
hosts.

Proof: `writes_cancel_definition_code_and_traps_do_not_end_the_server`
(`klf-driver/tests/native_cancellation.rs`) now runs on aarch64 macOS
against a distribution built from the workspace (`KLF_HOME`):

- writes during a long loop, a deep recursion and an allocating loop, in
  each of `parse`, `elaborate` and `lower`, release within budget with
  the heap balanced;
- a trap in each phase fails its request with `KLF-LSP-9001`, and the
  server keeps answering.

Still to do:

- x86_64 Mach-O: its `__eh_frame` needs subtracted PC-relative
  relocations, and the object writer gives `X86_64_RELOC_SIGNED` instead.
- COFF: needs `.pdata`/`.xdata` and an SEH personality.

Those hosts keep `<driver> lsp-worker`.

### 2026-10-17 update: `.klffi` records by value (`KLF-RIDL.md` 26.2, 26.3)

`foreign_declarations` gives a C prototype to every `@FFI.Extern`
function a record crosses by value. In that prototype each parameter is
a record by value, a pointer, or a scalar of its width and signedness;
an `@FFI.Alias` is signed or unsigned as its C target is under the
target's rules. The KIR signature passes the record's address, and a
record result is written through an address passed first. Cranelift
then calls the function through the C shim the kernel writes, which
applies the target's ABI.

`type_declarations` makes a struct with no bit-fields a `struct c` type
when the kernel's C rule (`klf-layout`) places it as the target's C rules
do, since the shims spell that form. Unions, structs with bit-fields and
any struct the kernel's rule would place differently stay `explicit c`.

Proof:

- `records_cross_a_foreign_call_by_value_through_the_c_shim`
  (`klf-cli/tests/ffi_conformance.rs`) declares two records in `.klffi`
  and links a KIR program, its shims and a C library. The C library sees
  and returns the values the program stored: a homogeneous float
  aggregate passed and returned in registers, and a 24-byte record passed
  in memory.
- `records_become_c_types_and_aliases_their_target_width`
  (`klf-klffi/tests/c_layout.rs`) checks which records become plain C
  structs.

### 2026-10-17 update: C shims on the LLVM path (`KLF-RIDL.md` 26.3)

The LLVM path no longer refuses a `call.foreign` of an import with a C
prototype. It calls `__klf_c_<name>` with the KIR signature, as Cranelift
does. A narrow integer of the prototype gets `signext` or `zeroext`, so
the target widens it as C does. A `func.ref` of a defined function that C
enters through a shim is refused with a reason; before, it silently
named the function itself.

Proof: `records_cross_a_foreign_call_by_value_through_the_c_shim`
(`klf-cli/tests/ffi_conformance.rs`) now builds the same program from
Cranelift's object and from the LLVM path's IR. Each is linked with the
same shims and C library, and both return the C library's results.

### 2026-10-17 update: callbacks through the C shim on LLVM (`KLF-RIDL.md` 26.3)

On the LLVM path, a `func.ref` of a defined function that C enters through
a shim now names `__klf_c_<name>`, the shim's symbol, as Cranelift does.
Before, it was refused. C is handed a pointer it can call with the C
signature, records by value included.

Proof: `records_cross_a_foreign_call_by_value_through_the_c_shim` now has
C call back into the program. C's `apply` takes a function pointer and a
`Point`, and calls the pointer with the `Point` and 3. The program hands it
`func.ref @scaled`. `scaled` takes the `Point` by value and passes it on to
C's `point_sum`. The Cranelift object and the LLVM IR both return 142,
which includes that round trip.

Gap recorded: the LLVM path sends every load and store through
`klfrt_load`/`klfrt_store`, which accept only addresses in the runtime
heap. A record C passes to a callback lives in C's memory. The callback
can hand the record on, but it cannot read the record's fields on LLVM,
which traps `KLF-RT-0005`. Cranelift reads them directly. Section 12.8
calls `raw.load` an unmanaged access and names no check, so the trap
departs from the spec. The check is in `klf-rt` `heap.rs`, which another
session holds, so this is still to do.

### 2026-10-17 update: C imports' variadic tails on LLVM (`KLF-RIDL.md` 29.5)

Section 29.5 says variadics follow the platform in platform mode and
through `.klffi`. The LLVM path now lowers a `call.variadic` of a C import
without a C prototype. It emits a call that names its variadic type, so
LLVM passes the tail as C does. A variadic argument narrower than `int`,
or a `float`, is refused with a reason. C promotes those before the call,
and KIR does not say whether an integer is signed, so the frontend passes
the promoted value. An import with a C prototype is reached through its
shim instead (section 26.3).

Proof: `a_c_imports_variadic_tail_follows_the_platform`
(`klf-llvm/tests/native_parity.rs`) calls libc's `snprintf` with an `int`,
a `long long` and a `double` in its tail. It returns the length C formats,
19 for one argument and 9 for another.

Still to do: Cranelift has no variadic calls, so it refuses this call. The
VM refuses it too. An exported platform-convention variadic that the module
defines is still refused everywhere.

### 2026-10-17 update: uniqueness-check elision across blocks (`KLF-RIDL.md` 14.1)

O2's uniqueness-check elision now follows a fresh allocation across
blocks. Before, it only looked within the allocating block. A forward
must-analysis keeps the counted allocations that every path to a block has
made and has not used since, except through `ref.addr` or
`ref.isunique`. A block's set is the intersection of its predecessors'.
The entry and pinned blocks start empty, and a loop's back edge shrinks
the set until nothing changes. A `ref.isunique` of an allocation in the
set becomes `true`.

Proof: `o2_follows_a_fresh_allocation_across_blocks`
(`klf-kir-passes/tests/optimize.rs`). In `paths`, the check after a join is
removed at O2, and the branch survives, so the check really does follow a
join. In `copied`, one path copies the reference and the check stays. Both
functions answer the same at O0 and O2.

Still to do: elision for a reference that is not a fresh allocation. A
check that passed proves nothing later. A `weak.load` of another weak
reference, or a `from.raw` of a pointer stashed earlier, can raise the
strong count without using the checked value. So knowing that a later
check passes needs to know that no weak or raw handle exists, and
section 14.1 does not say how a pass would know that.

### 2026-10-17 update: builder services for the machine instructions (`KLF-RIDL.md` 8.11)

Section 8.11 gives the KIR builder one method per section-12 instruction.
The builder (`klf-services/src/builder_machine.rs`) and the definition
services (`klf-defsvc/src/raw_heap_builder/machine.rs`, with entries in
`kernel/services.manifest`) now cover these instructions:

- `select` and `fma`
- `raw.load` and `raw.store`, each with an alignment and a volatile flag
- `ptr.offset` and `ptr.diff`
- `atomic.load`, `atomic.store`, `atomic.rmw` and `atomic.cmpxchg`
- `fence`
- `type.size`, `type.align` and `type.stride`

Each service's wrapper is `kir_value_<name>`.

Section 12.14 gives the atomics C11's memory orders, so an order is passed
as C11's `memory_order` value. KIR has no consume order, so 1 is refused.
An atomic operator is passed as its position in the list in 12.14.

`atomic.cmpxchg` has two results. `compare_exchange` answers the value it
read, and `compare_exchange_succeeded` maps that value to the success
flag, as `const_string_length` does for a string. The map behind both is
now named for second results generally.

Proof: `machine_instruction_services_build_a_program_both_backends_run`
(`klf-driver/tests/definition_machine_services.rs`). A KLF Kira definition
builds a program from every new service. The VM and the native backend
both return 126, a result that depends on each instruction.

The `vec.*` instructions have builder services too: `kir_type_vector`, then
`vector_splat`, `vector_extract`, `vector_insert` and `vector_shuffle`. A
shuffle is built with its first mask lane. `vector_shuffle_lane` then adds
each further lane, while the shuffle is still the instruction just built,
so nothing has used its result yet. This needs no state beyond the
instruction itself. `vector_services_build_a_program_both_backends_run`
returns 17 on both backends, from a splat, an insert, an elementwise add,
a two-lane shuffle across both operands, and extracts.

### 2026-10-17 update: C imports' variadic tails on the VM (`KLF-RIDL.md` 29.5)

The VM now runs a `call.variadic` of a C import without a C prototype. The
import is recorded as variadic with its fixed-argument count (KLBC import
kind 5, then that count, then the signature). The call plan places the
tail as each platform's C convention says. On Apple AArch64 every
variadic argument goes on the stack in an 8-byte slot. On Windows x64 a
variadic `double` in the first four positions goes in the integer
register of that position. The fixed arguments are placed as before. The
fast paths for all-integer and all-`double` calls are skipped for a
variadic plan, since they would put the tail in registers. As on LLVM, a
tail argument narrower than `int`, or a `float`, is refused, because C
promotes those first.

Proof: `a_c_imports_variadic_tail_follows_the_platform`
(`klf-llvm/tests/native_parity.rs`) now also runs the `snprintf` call on
the VM. It agrees with the native result for each argument.

Spec gap and limits: on SysV x86_64 a variadic call must say in `al` how
many vector registers it uses, and the VM's call trampoline does not set
it. So a variadic plan with a floating-point tail argument is refused
there. The Windows and Linux placements have not been run, since this
host is macOS on AArch64. Cranelift still refuses the call.

### 2026-10-17 update: C imports' variadic tails on Cranelift (`KLF-RIDL.md` 29.5)

Cranelift now lowers a `call.variadic` of a C import without a C
prototype. It has no variadic signatures, so each call gets a signature of
its own and goes through the import's address. Every tail argument fills a
full 8-byte slot: an `int` is widened, since C never reads the slot's upper
bytes. On Apple AArch64, where C passes the whole tail on the stack, the
signature first takes up every argument register the fixed arguments leave
free, x0 to x7 and v0 to v7, with zeros, so the tail lands on the stack. On
Windows x64 a `double` in the tail travels as an integer with the same
bits, which puts it in the integer register of its position, where C
expects it. AAPCS64 on Linux passes a variadic call like any other.

Proof: `a_c_imports_variadic_tail_follows_the_platform` in
`klf-cranelift/tests/native_parity.rs` runs the `snprintf` call, now shared
from `klf-llvm/tests/variadic_corpus.rs`, on Cranelift and on the VM, and
they agree. `a_c_imports_variadic_tail_is_placed_as_c_reads_it`
(`klf-vm/tests/variadic_areas.rs`) checks the VM against C's lengths, 19
and 9, which the LLVM test also checks.

Limits: on SysV x86_64 the caller must set `al`, and Cranelift cannot, so
the call is refused there. On Apple AArch64, fixed arguments that already
reach the stack are refused, since Apple packs those differently from its
variadic slots. Only the Apple AArch64 placement was run on this host.

The VM now refuses every variadic call on SysV x86_64 as well, not only
one with a floating-point tail, as the entry above said. `al` also counts
the vector registers the fixed arguments use, and an all-integer call must
still set it.

### 2026-10-17 update: `klf ffi gen` writes target-dependent scalars as aliases (`KLF-RIDL.md` 26.2)

Section 26.2 names C scalars whose width depends on the target, such as
`long` and `wchar_t`, through `@FFI.Alias`. `klf ffi gen` wrote `long` as
`I64`, which is wrong on Windows, and skipped `wchar_t`. It now writes them
as `CLong` and `CWChar`, and puts an `@FFI.Alias { target: long }` or
`{ target: wchar_t }` declaration first when the output uses one.
`intptr_t`, `ptrdiff_t` and `ssize_t` become `ISize`, and `uintptr_t`
becomes `USize`, instead of 64-bit types. A foreign declaration now gives
`ISize` and `USize` the target's pointer width, as the C record layout
already did, rather than 64 bits everywhere.

Proof: `generated_target_dependent_scalars_follow_each_targets_c`
(`klf-klffi/tests/klffi.rs`) generates from a header using `long`,
`wchar_t`, `intptr_t`, `uintptr_t` and `ptrdiff_t` and checks the exact
`.klffi`. Its foreign declarations return `i32`, `i16` and `i64` on
x86_64 Windows, `i64`, `i32` and `i64` on x86_64 Linux, and `i32` for all
three on wasm32.

Still a spec gap: `unsigned long` has no one-word alias target, so it is
still written `U64`.

### 2026-10-17 update: `aarch64-apple-ios` executables (`KLF-RIDL.md` 5.8)

`aarch64-ios` is a 1.0 target. A generated frontend now builds for
`aarch64-apple-ios`. Cranelift takes the triple and writes Mach-O objects
for the iOS platform. The C toolchain takes the iPhoneOS SDK from `xcrun`.
Every check that meant "Mach-O and `libSystem`" now covers any Apple
target, not just macOS: symbol prefixes, `.dylib` names, link flags and
system libraries. The runtime archive builds with Cargo for the target, as
any target Rust ships does. The target and tool helpers moved from
`output.rs`, which was over 700 lines, into `toolchain.rs`.

Proof: `an_ios_program_links_as_an_ios_mach_o_executable`
(`klf-frontend-rt/src/output_tests.rs`) links a KIR program for
`aarch64-apple-ios` with the kernel runtime built for it. It checks that
the image names the iOS platform and no other. With no deployment target
given, clang records iOS 7.0 in `LC_VERSION_MIN_IPHONEOS`. The spec names
no minimum iOS version, so none is chosen.

Not done: the program has not run on a device. The `klf` driver's own
native path (`klf-driver/src/native.rs`) still assumes the macOS SDK on an
Apple target.

### 2026-10-17 update: unwind tables on x86_64 Mach-O (`KLF-EDITOR.md` 8.6)

Frontend objects for `x86_64-apple-darwin` now get the same `__eh_frame`
and landing tables as aarch64 Mach-O and ELF. x86_64 Mach-O has no
PC-relative relocation for data. So each PC-relative pointer is written as
an `X86_64_RELOC_SUBTRACTOR` against the section's start, paired with an
`X86_64_RELOC_UNSIGNED` to the target, with the field's offset taken out of
the addend. This is how the assembler writes it. The personality goes
through the GOT with `X86_64_RELOC_GOT`.

Proof: `klf-cranelift/tests/unwind_tables.rs` links a frontend object whose
`units` phase calls a C function, which walks the stack with
`_Unwind_Backtrace`. The test passes only if the walk steps out of the
Cranelift frames into the C function that called the phase. It runs
natively for `aarch64-apple-darwin`, and under Rosetta for
`x86_64-apple-darwin`. With the x86_64 tables switched off, the x86_64 test
fails.

Still to do: COFF objects get no unwind tables (`.pdata`/`.xdata`).

The test's `units` phase owns a counted value across its call, so the call
has a cleanup pad. The walk checks that the unwinder finds that frame's
landing table, with its one entry, on both architectures. Since x86_64
macOS now has the tables, `<driver> lsp` analyzes in its own process on
every Unix host (`klf-frontend-rt/src/editor.rs`). Section 8.6 of
`KLF-EDITOR.md` sends a host to `<driver> lsp-worker` only when its objects
get no unwind tables. Its list of such hosts still names x86_64 Mach-O,
which is now out of date (spec gap). The personality's GOT entry is only
read when an unwind lands in a pad. No test on x86_64 does that yet, since
`native_cancellation` needs a fresh distribution.

### 2026-10-17 update: a KArc formatter (`KLF-FORMAT.md` 10)

KArc (`../karc`) now has a `format` hook. It lexes the file with KArc's own
lexer, before preprocessing, so every token is the source's own. It hands
the token spans to the shared C-like layout of section 7, which the package
takes in through its new `include` file (`../klf-kira/languages/format`).
Directives are kept as written (rule 7), so no macro-produced code is ever
laid out, which section 10 asks for. Semicolons end statements, `case` and
`default` are labels, and `<`, `>`, `*`, `&`, `&&` and `>>` keep the
source's spacing where they may be part of a declaration.

Proof: `formatsCOnTheSharedLayout` (`karc/tests/format.klfd`) formats a C
file with a `#define`, a struct, loops, a `switch` and an initializer
against its expected text, and `klf test ../karc --unit` passes all 37
tests.

Spec gap: the input file of `formats` is a `.c` file, and the parse phase
runs before the hook, so a test cannot use C++-only syntax or an
`#include` of a header the test does not register.

### 2026-10-17 update: the VS Code client (`KLF-EDITOR.md` 19.5, DE-010)

`editors/vscode-client/` holds the one JavaScript file every generated VS
Code extension runs. It is built with esbuild from `vscode-languageclient`
9.0.1, whose VS Code engine (`^1.82.0`) is the one the generated
`package.json` names; 10.x needs 1.91. It reads the `manifest.json` beside
it and implements:

- server discovery in the order of section 19.6: the setting with its
  arguments, `KLF_<LANG>_SYSROOT`, then the PATH (on macOS joined with the
  login shell's), logging the tier; failure names all three and offers the
  setting. Other drivers for `klf.run` come from `<lang>.drivers.<driver>`,
  then the sysroot variable whose `bin/` holds the driver, then the PATH.
- `klf.run` in a terminal running the driver directly, `klf.showOutput`
  (pick target and view, open beside), `klf.reloadProjects`,
  `klf.restartServer`;
- `klf-virtual:` documents through a `TextDocumentContentProvider`,
  refreshed on `klf/shaderOutputChanged`;
- the status bar item from `klf/status` (notification, or asked after
  start and on editor change and save) and `klf/fileInfo`;
- sysroot `share/<lang>/src` and `library` files made read-only in session.

Extensions are used only when the server lists them in `klf.features`.
`klf dist` now builds the client with npm and ships it as
`share/klf/editor/vscode-client.js`, where `klf editor gen` finds it. The
generated `package.json` contributes all four commands of section 17.3,
`klf.run` and `klf.showOutput` hidden from the palette since they take a
lens's arguments.

Proof: `npm test` in `editors/vscode-client` passes 6 tests: five on the
discovery tiers, and one that loads the bundle with a stand-in `vscode`
module, starts the real `target/debug/klf lsp` through the setting, and
checks the commands, the provider, the logged tier and that `klf/status`
reached the status item. `klf-editor-gen`'s
`a_vsix_with_the_client_starts_it_and_offers_the_protocol_commands`
checks the vsix layout; `klf editor gen ../karc --editor vscode` with the
bundle in place wrote a vsix whose `extension/extension.js` is
byte-identical to it. The `@vscode/test-electron` session of section 24 runs
only on editor CI hosts; VS Code is not installed here.

Spec gaps found: the server answers `klf/status` as a request and never
sends the notification of section 17.2, and does not serve `klf/fileInfo`,
`klf/runnables` or `klf/reloadProjects`, so the client hides the file line
and reloads by restarting. The `klf.showOutput` lens sends `targets`, not
the `target` and `stage` of `klf/shaderOutput`, so the client asks for the
target and passes no stage. Section 19.6 says another driver comes from
"that driver's language" sysroot but gives the client no map from driver to
language.

### 2026-10-17 update: `klf/fileInfo`, `klf/runnables`, `klf/reloadProjects` and the `klf/status` notification (`KLF-EDITOR.md` 13.5, 17.2)

The server now serves the rest of section 17.2:

- `klf/status` is sent as a notification whenever it changes, after each
  turn of the server loop, and still answers the request.
- `klf/runnables` answers a document's runnables as `{ label, kind,
  arguments, cwd, range }`, with `cwd` the project directory. A `position`
  keeps only the runnables whose range holds it.
- `klf/fileInfo` answers `{ project, fileMode, memory, convention, unit }`.
  An analysis now records, for each file, the selected file mode's name,
  the effective memory model and convention as the definition spells them
  (`Counted`, `Platform`), and the index of the unit holding the file
  (`Session::unit_holding`). These travel in the worker text as
  `file_info` lines. Anything the analysis did not record is null; the
  kernel's own file types record none.
- `klf/reloadProjects` forgets every project, withdraws their published
  diagnostics, answers waiting requests `ContentModified`, and resolves and
  analyzes each open document's project again.

All three are listed in `klf.features`, so the VS Code client uses them.

Proof: klf-lsp's protocol test asks for the runnables with and without a
position, the file info, reloads (the analyzer runs a second time and the
next `klf/fileInfo` is answered from it), and sees `klf/status`
notifications; all 5 klf-lsp tests pass. `klf-ide`'s
`file_info_survives_the_worker_text` round-trips the record.
`native_frontend_analyzes_for_the_language_server` builds the `mini`
frontend and checks that its `lsp-worker` reports a `.mini` file with the
language's defaults (no mode, `Unmanaged`, `Ridl`) and a `.minic` file in
the `counted` mode (`Counted`, `Platform`). A workspace `klf` links the
driver archive of the installed distribution, so this check needs
`KLF_HOME` naming a distribution whose `bin/libklf_kernel.a` is this
build's `klf-frontend-rt`; it passed so, and fails against the older
installed archive, which records no file info.

Spec gap: section 17.2 does not say what `unit` holds. It is the
zero-based index of the unit in the session, which is all a client can
show.

### 2026-10-17 update: the KIR view (`KLF-EDITOR.md` 13.7)

`klf/kir` answers `{ uri }` of a `klf-virtual:` document holding the KIR
text of the function at the position, raw as the unit lowered it or
canonical after the mandatory passes (`klf_kir_passes::canonicalize`), or
null. An analysis lowers to KIR text only when its request says `kir`, and
the server sets that only for projects someone asked for KIR of, so typing
in other projects pays nothing. The first `klf/kir` of a project starts a
new analysis and waits for it. `klf_kir::function_text` prints one
function as it reads in its module.

Views now keep what they show (a shader output, or a function's KIR) and
read their text again from every new analysis of their project, so
`klf/virtualFile` after `klf/shaderOutputChanged` gives the new text.
Before, a view kept the text of the revision it was opened at.

The VS Code client has a `klf.showKir` command (pick raw or canonical,
open beside), contributed to the palette as section 13.7 asks.

Proof: klf-lsp's protocol test asks for raw and canonical KIR, reads both
views through `klf/virtualFile`, sees the analyzer run once more for the
first request, and gets null before the first function. klf-ide's
`kir_survives_the_worker_text` round-trips the request flag and the
records. `native_frontend_analyzes_for_the_language_server` asks the
`mini` frontend's `lsp-worker` for KIR and checks the raw and canonical
text of the file's function (with `KLF_HOME` naming a distribution holding
this build's driver archive, as before). The client's 6 tests and
klf-editor-gen's 6 tests pass.

Spec gaps: KIR records where a function was named, not where its body
ends, so "the function at the position" is the last function named at or
before it. Section 13.7 asks for a command but names none; it is
`klf.showKir`.

### 2026-10-17 update: the macro expansion view (`KLF-EDITOR.md` 13.6)

`klf/expand` answers `{ uri, name }` of a `klf-virtual:` document holding
the text the innermost macro use around the position produced, or null.
A definition reports each expansion through the new service
`klf.source.expansion` (session, the file holding the produced text, the
file of the use, its start and end, the macro's name); the session keeps
the records (`Session::record_expansion`, saved and restored with the
rest), the frontend writes them into the analysis as `expansion` records
with the produced text, and the server opens a view over them that
follows each new analysis like the KIR and shader output views. Kira's
`evaluateMacro` reports every expansion it evaluates or recalls from the
batch, so function, attribute, derive and annotation macros all show.
The VS Code client has a `klf.showExpansion` command, opening the view
beside the source.

`klf-defsvc`'s `lib.rs` grew past 700 lines with the service; the source
span services moved to `source_spans.rs`.

Proof: klf-lsp's protocol test records an expansion, asks `klf/expand`
inside and outside it, and reads the view through `klf/virtualFile`.
klf-ide's `kir_survives_the_worker_text` round-trips the `expansion`
records. klf-services, klf-defsvc and klf-kira-lower tests pass (apart
from the known `stock_lldb` line table test). `klf check` loads the Kira
definition with the reporting call. The client's 6 tests and
klf-editor-gen's 7 tests pass.

Spec gaps: section 13.6 says expansions are virtual files but gives no
way for a definition to say which text a macro use produced or what the
macro is called; `klf.source.expansion` is that way. It names no language
for the view; the view takes the language the server serves. It names no
command; the client's is `klf.showExpansion`.

### 2026-10-17 update: RIDL `@Source` (`KLF-EDITOR.md` 15.3)

A driver given `--ridl-source` with `--emit ridl` writes
`@Source("<path>:<line>:<column>")` before each declaration whose KIR
function it compiled, naming where the function was named, relative to the
`.ridl` file. The definition's own `.ridl` text is kept, comments
included: the annotation is inserted before each declaration, which
`klf_ridl::parse_spanned` now locates (it gives each top-level
declaration's start, its name and its `@Source` strings). For a
definition-described surface the `ridl.entry` attributes say which
function stands behind an export path. `canonical_text` leaves `@Source`
out, so the ABI hash is the same with and without it.

`klf lsp` reads `@Source` in a `.ridl` file as a document link and a
definition target: the place it names is declared as an entity in that
file, and the annotation's string and the declaration's name both refer to
it, so go to definition on a RIDL function lands in the C (or other
language's) source.

A program server's calls into an imported RIDL module lead to the `.ridl`
declaration: the definition reads the module as the bindings
`<directory>/<module>.ridl#kira`, a text no editor can open, so the native
frontend's editor analysis moves every top-level declaration of those
bindings to the declaration of the same name in the directory's `.ridl`
files. From Kira, go to definition reaches the `.ridl`, and from there the
source, in two steps across two servers.

`klf-ridl`'s `parse.rs` was over 700 lines; its post-parse passes moved to
`parse/passes.rs` and the type-name helpers to `parse/names.rs`.

Proof: klf-ridl's `source_annotations_leave_the_canonical_text_alone` and
`spans_name_each_declaration_and_its_source`. klf-driver's
`native_frontend_marks_ridl_with_sources` builds the `mini` driver, emits
`.ridl` into a subdirectory with and without `--ridl-source`, sees
`@Source("../input.mini:1:1")` before `function answer`, and the same
canonical text for both (with `KLF_HOME` naming a distribution holding
this build's driver archive). klf-kernel-ide's
`ridl_sources_link_and_lead_to_the_providing_language` gets one link and
the C function's name as the definition from both the annotation and the
declaration's name. klf-frontend-rt's `bindings_declarations_move_to_the_ridl`
moves a binding's symbol to the `.ridl` and leaves others alone.

Spec gaps: section 15.3 does not say what the column counts; it is the
1-based byte column diagnostics print. A declaration with no function
behind it (a struct, an enum) has no place KIR records, so it gets no
`@Source`, and methods inside `extend` and `trait` blocks are not marked
or moved: only top-level declarations are. Item 3 asks servers to pass
`--ridl-source` when they emit `.ridl` for workspace modules; no server
emits `.ridl` today, so there is nothing to pass it to. Moving binding
declarations to the `.ridl` goes by name, since the bindings record no
mapping back to the `.ridl` text.
