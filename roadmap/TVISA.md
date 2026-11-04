# TVISA Specification 1.0

Date: 2026-11-01. TVISA version: 26.11.1. Status: normative draft 1.0. Audience: every agent and engineer who implements the TIR-to-TVISA lowering, the TVISA verifier, the CPU finalizer, the server backend, a GPU family backend, or the paravirtual GPU host.

Contents

1. Status, scope and rules for implementers
2. Terminology
3. Goals, non-goals and related work
4. Position in the toolchain
5. Machine model
6. Image structure
7. Types
8. Functions, values and frames
9. Calling conventions
10. Control flow
11. Instruction set
12. Runtime entry points
13. Memory, atomics and device I/O
14. Exceptions and unwinding
15. Vectors and masks
16. Variant bodies, target intrinsics and inline assembly
17. Control-flow integrity and hardening sequences
18. Code model, relocations and linking
19. Thread pointer, thread control block and thread-local storage
20. Debug information and symbolication
21. The GPU profile
22. GPU family backends
23. The paravirtual GPU
24. Binary format (`.tvisa`)
25. Textual format (`.tvs`)
26. Verification
27. The finalizer
28. The server backend
29. Determinism, caching and parity
30. CPU targets and features
31. GPU families
32. Platform targets
33. Versioning
34. Conformance and diagnostics
35. Implementation plan and crate layout
36. Appendix A: grammars and container layouts
37. Appendix B: decision log

---

## 1. Status, scope and rules for implementers

This document is the single normative specification of TVISA, the Tessera Virtual ISA: the low-level, architecture-neutral form into which `tirc` lowers TIR, and from which every native image is produced, whether by the pipeline's server backend, by `tirc`'s fast backend, by the device finalizer, or by a GPU family backend. It defines TVISA's machine model, image structure, types, functions and frames, calling conventions, control flow, instruction set, runtime entry points, memory and atomics lowering, unwinding, vectors, variant bodies, target intrinsics, inline assembly, control-flow integrity and hardening sequences, code model and relocations, thread-local storage, debug information, the GPU profile, the GPU family backends, the paravirtual GPU, the binary and textual formats, verification, the finalizer, the server backend, the CPU targets and GPU families, the platform targets through which TIR programs run on other operating systems, versioning, and conformance. An implementation that satisfies every MUST in this document is a conforming implementation of TVISA version 26.11.1.

### 1.1 Normative language

- MUST, MUST NOT, SHOULD, SHOULD NOT and MAY carry their RFC 2119 meanings.
- Sections or paragraphs titled "Rationale" or "Example" are informative. Everything else is normative.
- Where prose and a table disagree, the table wins. Where a grammar in Appendix A and an example disagree, the grammar wins.
- Every numeric constant (opcodes, tags, register assignments, offsets, instruction sequences, limits) is normative.

### 1.2 Relationship to other documents

| Document | Relationship |
| --- | --- |
| `TIR.md` | The parent specification. TIR defines everything above the TIR-to-TVISA boundary (TIR.md section 26.6), every instruction semantics TVISA shares with TIR, the hardening obligations of TIR.md section 27, and the delivery and pipeline rules. TVISA defines everything below the boundary |
| `KLF.md` | RIDL data layout (KLF.md section 21) and the RIDL calling convention (KLF.md section 22), which section 9 lowers per architecture |
| `Ember.md` | Consumes GPU entry records and GPU binaries (sections 21 and 22); its hosted backends consume the translated code of section 31.5 |
| `DeviceKit.md` | GPU drivers report a GPU's family and revision (section 31.3); the paravirtual GPU's transport (section 23) |
| `Tarwin.md` | The platform image format, the loader, and the kernel code model (section 18) |
| `LibTessera.md` | Thread creation and the thread control block it initializes (section 19) |

Where TIR.md and TVISA.md disagree about anything after the TIR-to-TVISA boundary, TVISA.md wins. Where they disagree about anything before it, TIR.md wins.

### 1.3 Scope and stability

| Artifact | Role | Stability |
| --- | --- | --- |
| TVISA (`.tvisa`, `.tvs`) | The lowered form shipped to devices and consumed by every backend | One version per train, frozen once released; versions accepted for the same window as TIR (section 33) |
| CPU targets and features (sections 30 and 32.2) | Instruction-set baselines and tuned targets | Versioned with TVISA; additions only within a version's lifetime |
| GPU families (section 31) | The GPU instruction-set families with Tessera-owned backends | Versioned with TVISA |
| Target intrinsic registry (`TVISA-INTRINSICS.md`) | The admitted architecture intrinsics | Versioned with TVISA |
| GPU binary container (section 22.6) | The `.tirgpu` section of native images | Versioned with TVISA |

### 1.4 Rules for implementing agents

1. Implement behavior exactly as written. Where the document is silent, choose the behavior that keeps the finalizer, the server backend and the interpreter of TIR observably identical on every architecture and GPU family, and record the choice in `docs/decisions.md` in the format of Appendix B.
2. Every CPU feature MUST work in the finalizer and in the server backend on aarch64, x64 and rv64, for every CPU target of section 30. Every GPU feature MUST work in every GPU family backend that advertises the capability and in the paravirtual GPU host translator. For platform targets (section 32), every CPU feature MUST work in the server backend for every platform and architecture TIR.md section 32.2 lists, and in the finalizer where section 32.12 uses it.
3. No placeholder, stub, hardcoded success or host-rendered substitute counts as an implementation. A section is implemented when its conformance tests (section 34) pass.
4. The verifier, the finalizer, the GPU family backends and the server backend's translator are written in Rust. The CPU finalizer MAY be built on Tessera's Cranelift fork; the server backend uses the Tessera LLVM fork.
5. Rust files stay under 700 lines, generated files excepted. No Python in tracked files.
6. Hardening sequences (section 17) are normative byte-for-byte in their instruction choice and order. A backend that emits an equivalent but different sequence does not conform, because audit tooling (section 34.5) checks for the exact sequences.

---

## 2. Terminology

| Term | Meaning |
| --- | --- |
| TVISA image | One `.tvisa` file: the lowered form of one TIR package (or one framework of it), containing CPU code for all three architectures and GPU code for all families |
| Lowering | `tirc`'s stage 12 (TIR.md section 26.2): TIR to TVISA |
| Finalizer | The component that compiles a TVISA image to native code for one CPU target and GPU family. It runs on devices (TIR.md section 28.6) and inside `tirc` for fast builds |
| Server backend | The translation of TVISA to LLVM IR followed by the LLVM pipeline, used by the pipeline for release native images |
| GPU family backend | A Tessera-owned compiler from TVISA GPU code to one GPU family's instruction set |
| Virtual register | An SSA value of a TVISA function. Unbounded in number |
| Leaf | A scalar or vector value as classified by RIDL (KLF.md section 22.3): integer class, float class or vector class |
| Parameter group | Two to four leaves passed together in registers when they fit, and by reference otherwise (section 9.3) |
| Frame slot | A fixed-size stack object of a function, declared in its signature record |
| Runtime entry | A function of `libtirrt` identified by a number and called with `call.runtime` (section 12) |
| `rtcc` | The calling convention of runtime entries: almost every register preserved (section 9.6) |
| CPU target | A named feature set of one architecture (section 30), for example `aarch64.base` |
| Feature | A named optional instruction-set extension (section 30.1) |
| GPU family | A named GPU instruction-set family (section 31), for example `amd.gfx110` |
| Revision | A finer GPU identifier within a family, reported by the GPU's driver |
| Thread control block | The 64-byte per-thread record at the thread pointer (section 19) |
| Type tag | The 32-bit control-flow-integrity tag of a function type, derived from its `typeid` (TIR.md section 7.9) by the rule of section 17.2 |
| Audit | Mechanical inspection of native code for the hardening sequences of section 17 |

---

## 3. Goals, non-goals and related work

### 3.1 Goals

1. One form for every backend. The pipeline's release images, `tirc`'s development images, device-finalized images and GPU binaries are all compiled from the same TVISA image, so their semantics agree by construction.
2. Fast, predictable finalization on phones. Verification is linear in image size. Functions are independently decodable and compile in parallel. The finalizer sustains at least 2 MiB of TVISA per second per performance core on each train's reference devices (sections 27.5 and 34.3).
3. Near-native quality. Every target-independent optimization is already done before TVISA. The finalizer performs only target-dependent work, and its output runs within 1.20× of the server backend's for the same target (TIR.md section 34.4).
4. Architecture-neutral and vector-length-agnostic CPU code. One TVISA image serves aarch64, x64 and rv64 and every vector length. Architecture-specific code lives in variant bodies the finalizer selects.
5. Hardening fixed by the specification. Control-flow integrity, return-address protection, pointer authentication, stack protection and zeroing are exact instruction sequences, not backend choices, so native code can be audited mechanically.
6. GPU code for every family from one form, with separate stage compilation and link steps that never recompile a shader body.
7. Compactness. A TVISA image SHOULD NOT exceed 1.5 times the size of the aarch64 native code finalized from it.

### 3.2 Non-goals

- TVISA is not an interchange format. Only `tirc` produces it and only Tessera's backends consume it. It carries no ownership, no generics, no coroutines and no engines: those are lowered before the boundary.
- TVISA is never interpreted. The interpreter and JIT tiers execute TIR (TIR.md section 28).
- TVISA is not written by hand outside tests.
- No big-endian targets and no 32-bit pointers: under `wasm.base`, pointers keep their 64-bit representation (section 32.11).
- TVISA images are never delivered outside Tessera. For platform targets (section 32) TVISA is an internal stage of `tirc`.

### 3.3 Related work (informative)

| System | What TVISA shares | Where TVISA differs |
| --- | --- | --- |
| PTX and SASS | A virtual ISA compiled on the device per GPU generation | TVISA spans CPUs and GPUs from every vendor and carries hardening contracts |
| Cranelift IR | Typed SSA with block parameters, fast compilation | TVISA is a frozen, versioned shipping format with RIDL conventions, GPU code and variant bodies |
| LLVM bitcode | SSA, metadata | TVISA is far smaller, verifiable in linear time, and target-neutral including vector length |
| WebAssembly | Streaming, parallel, validated compilation | TVISA is register-based SSA, systems-level, with native calling conventions and no sandbox |
| Android dex2oat | Install-time compilation on the device | TVISA input is already optimized; the finalizer does only target-dependent work |
| Mesa NIR | A common GPU IR with per-vendor backends | TVISA GPU code is a stable shipping format; NIR is an internal compiler IR |
| HSAIL | A portable GPU virtual ISA | TVISA's GPU profile is bindless, structured and paired with a CPU profile |

---

## 4. Position in the toolchain

```
TIR package
   │  tirc stages 1–11: verify, link, analyze, specialize, optimize (target-independent)
   ▼
TIR-to-TVISA lowering (stage 12)
   │  ownership → explicit operations and runtime calls; coroutines → state machines;
   │  generics → specialized or descriptor-passing code; engines → bridges;
   │  aggregates → memory and leaves; layouts → byte offsets; RIDL calls → parameter groups
   ▼
TVISA image ──────────────┬───────────────────────────┬─────────────────────────┐
                          ▼                           ▼                         ▼
              server backend (LLVM)        finalizer (Cranelift-class)    GPU family backends
              pipeline release images      tirc development images,       server and device
                                           device finalization
```

1. Every native image is produced from a TVISA image. The only native code on a Tessera device not derived from TVISA is JIT code compiled from TIR at run time (TIR.md section 28.5) and the finalizer's own code.
2. Kernel code (TIR.md section 31.1) is lowered to TVISA inside the pipeline and compiled by the server backend into native kernel images. No kernel TVISA image is ever delivered, because nothing can finalize code before the kernel boots.
3. A TVISA image is architecture-neutral: it contains every portable function once and every variant body for every architecture. The finalizer selects bodies for its target.
4. GPU code in a TVISA image serves every GPU family.
5. For platform targets (TIR.md section 32), the server backend and, for development builds, the finalizer inside `tirc` compile the TVISA image into macOS, iOS, Windows, Linux, Android and WebAssembly images (section 32). Their GPU code is translated for the platform's hosted backends (section 31.5).

---

## 5. Machine model

TVISA inherits TIR's fixed properties (TIR.md section 5.1): 8-bit bytes, little-endian, 64-bit pointers in every address space, two's complement, IEEE 754-2019. In addition:

| Property | Value |
| --- | --- |
| Stack | Grows toward lower addresses; 16-byte aligned at every call boundary on every architecture |
| Frame pointer | Always maintained: x29 (aarch64), rbp (x64), s0 (rv64); every frame record is the pair (saved frame pointer, return address) |
| Page size | 16 KiB on aarch64, 4 KiB on x64 and rv64. `target.pagesize` folds to these at finalization |
| Red zone | 128 bytes below the stack pointer for leaf functions on x64 in user space; none on aarch64 and rv64; none in the kernel profile |
| Thread pointer | `TPIDR_EL0` (aarch64), the FS base (x64), `tp` (rv64), always pointing at the thread control block (section 19) |
| Code alignment | Function entries are aligned to 16 bytes on every architecture; the 4-byte type tag of an address-taken function occupies the 4 bytes before its entry (section 17.2) |

### 5.1 Reserved registers

| Architecture | Never allocated | Scratch reserved for veneers and sequences | Fixed roles |
| --- | --- | --- | --- |
| aarch64 | x18 (platform register), sp | x16, x17 | x29 frame pointer, x30 link register, x20 RIDL context, x21 RIDL error, x8 indirect result |
| x64 | rsp | r10, r11 | rbp frame pointer, r13 RIDL context, r12 RIDL error, rax indirect result |
| rv64 | x0, gp, tp, sp | t0, t1, t3 | s0 frame pointer, ra link register, t2 Zicfilp label, s10 RIDL context, s11 RIDL error, t4 indirect result |

`gp` is never used for linker relaxation on Tessera. x18 on aarch64 is reserved for future platform use and MUST NOT be read or written by generated code.

---

## 6. Image structure

### 6.1 Granularity

1. `tirc lower` produces one TVISA image for each native image a package builds: one per executable (app, extension, service, driver), one per framework, and one per plug-in image. A TVISA image and the native images finalized from it have the same image name.
2. Runtime-engine TIR (TIR.md section 6.5) is delivered as TIR and never appears in a TVISA image. Bridges to it do.
3. One TVISA image contains code for every architecture and every GPU family. It names no CPU target.

### 6.2 Logical contents

| Entity | Content |
| --- | --- |
| Functions | CPU functions, GPU functions and GPU entries, each with a signature record (section 8.1) and a body, or an import record |
| Data objects | Read-only data, relocated read-only data, writable data, zero-initialized data, thread-local data, and GPU constant data, each with alignment, contents and relocations (section 18) |
| Variant sets | Function bodies selected by architecture and features (section 16) |
| Dispatch sets | Variant sets whose selection waits for load time (section 16.3) |
| GPU entry records | Section 21.2 |
| Unwinding records | Landing-pad clauses and type tables (section 14) |
| Debug information | Section 20 |
| Pass-through records | The RIDL metadata blob, framework dependency records, bridge tables and runtime-module hashes, copied verbatim into the native image's `.ridlmeta`, `.tirdeps`, `.tirbridge` and `.tirrt` sections (TIR.md section 27.8) |
| Analysis summary | Trust classes and reason masks, sandbox regions and the indirect-call type table, copied from the TIR analysis; the ASM and GPU sections carry the assembly inventory and per-entry GPU capability declarations |

### 6.3 Linkage and symbols

| Linkage | Code | Meaning | Symbol |
| --- | --- | --- | --- |
| `local` | 0 | Visible only inside the image | None in the native image's dynamic symbol table; a name for debugging only |
| `image` | 1 | Package-linkage entity that the image defines | The internal `_TIR1` symbol (TIR.md section 22.9); not exported |
| `exported` | 2 | RIDL export | The RIDL symbol (KLF.md section 24.2), exported |
| `import` | 3 | Defined by another image | Resolved by the loader (section 18.4) |

Every entity has exactly one linkage. An `exported` function has convention `ridl` (section 9). An `import` has no body.

In text, defined `func` and `data` items use `local`, `image` or `exported`. Imports use the separate `import` item and carry import linkage in the binary records.

### 6.4 Index spaces

1. Functions: function imports in IMPORTS order, then defined functions in SIGNATURES order. CPU and GPU functions share the space.
2. Data objects: data imports in IMPORTS order, then defined data objects in DATA order.
3. Variant sets and dispatch sets: in VARIANTS order.
4. A symbol operand is a ULEB whose low two bits select the space (0 function, 1 data object, 2 variant set, 3 reserved) and whose remaining bits are the index.
GPU entry records refer to function-space indexes. A GPU entry's function record has target code 2; `GPUENTRY64` relocations use that function-space index.

### 6.5 Independent decodability

Every function body is self-contained: it references only the image's tables (strings, types, signatures, symbols, constants) and never another body. The FUNCINDEX section gives each body's offset and size, so the finalizer decodes, verifies and compiles bodies in parallel and in any order (section 27.3).

### 6.6 Image records

The IMAGE section contains exactly one record in this order:

| Field | Encoding |
| --- | --- |
| Image name | ULEB STRINGS index of a nonempty string |
| Kind | `u8`: 0 executable, 1 framework, 2 plug-in, 3 driver, 4 kernel |
| Package digest | 32 raw BLAKE3-256 bytes |
| `tirc` build identifier | ULEB STRINGS index of a nonempty string |
| TIR version | `u8` year, `u8` month, `u8` week, as TIR.md section 24.2 defines |
| Module hashes | Nonzero ULEB count followed by that many raw 32-byte BLAKE3-256 module hashes |
| Entry function | ULEB global function-space index plus one; zero means absent |
| Initializers | ULEB count, then records of ULEB global function-space index and `u16` little-endian priority |
| Finalizers | Same encoding as initializers |
| GPU capabilities | `u64` little-endian mask |
| Minimum Driver API | Driver images only: `u8` year and `u8` month, in DeviceKit's `YY.M` form |

Kind 0 has an entry function; the other kinds do not. Kinds 0–2 require the `app` or `system` profile, kind 3 requires `driver`, and kind 4 requires `kernel`. A driver image has a minimum Driver API; every other kind omits it. The TIR version year and month are 1–99 and 1–12, and the week is 1–5; the TIR version equals the TVISA header version (section 33.2). A Driver API year and month use the same respective ranges.

Function indexes use section 6.4's global function space, with imported functions first. The entry, initializer and finalizer indexes MUST resolve to defined CPU functions, never imports or GPU-only functions. The executable entry field stores index + 1 so zero is an unambiguous absent value. Initializers and finalizers are ordered by ascending `(priority, function index)`; finalizers are invoked in reverse order at unload. Each function index is less than 2^28.

Module hashes are unique and strictly ascending by their raw bytes. GPU capability bits use the optional-capability order in TIR.md section 17.18; bits 19–63 are zero. The mask equals the bitwise union of the capability masks of all GPU entry records, or zero when there are no GPU entries. The IMAGE payload is always canonically encoded; decoders reject a payload whose decoded record differs from its canonical re-encoding.

---

## 7. Types

### 7.1 Value types

TVISA values are scalars, vectors, code pointers and, in the GPU profile, GPU-only types. There are no aggregates, references, generic parameters, coroutine handles or frames: the lowering has replaced them with memory, leaves and explicit runtime calls.

| Type | Syntax | Profile | Notes |
| --- | --- | --- | --- |
| Integers | `i1 i8 i16 i32 i64 i128` | CPU, GPU (`i128` CPU only) | `i1` is a value type only; it is never loaded or stored (section 13.1) |
| Floats | `f16 bf16 f32 f64 f128` | CPU; GPU without `f128`, `f64` with capability `float64` | |
| Narrow floats | `f8e4m3 f8e5m2 f6e2m3 f6e3m2 f4e2m1 e8m0` | Vector elements and memory only | As TIR.md section 7.1 |
| Pointer | `ptr` | CPU | A 64-bit address |
| GPU pointer | `ptr<device>`, `ptr<constant>`, `ptr<threadgroup>`, `ptr<thread>`, `ptr<object_payload>`, `ptr<ray_payload>`, `ptr<hit_attribute>`, `ptr<tile>`, `ptr<node_record>` | GPU; `ptr<device>` and `ptr<constant>` also as CPU values | |
| Code pointer | `code` | CPU | A function value in register form (section 17.4). Distinct from `ptr` so that signing and authentication are explicit |
| Fixed vector | `vec<N x T>` | CPU, GPU (N ∈ {2, 3, 4} on GPUs) | N ∈ {2, 3, 4, 8, 16, 32, 64}; total size at most 512 bytes |
| Scalable vector | `svec<T>` | CPU | Section 15.2 |
| Masks | `vec<N x i1>`, `svec<i1>` | CPU, GPU | |
| Resource handles | `texture<...>`, `sampler`, `accel`, `vft<sig>`, `tensor<rank x T>` | GPU; CPU values of these types are 8-byte opaque words of type `i64` | As TIR.md section 7.8; `vft` names a TVISA signature (section 7.4) |
| Register-only GPU types | `rayquery`, `hitobject`, `coop<scope, M x N, T, use>` | GPU | As TIR.md section 8.6 |

### 7.2 Type tags

TVISA reuses TIR's type tags (TIR.md section 22.7) for the types it keeps, with the payloads TIR defines, and adds one.

| Tag | Type | Payload | TVISA restriction |
| --- | --- | --- | --- |
| 0x01 | `iN` | ULEB N | N ∈ {1, 8, 16, 32, 64, 128} |
| 0x02–0x0C | Float types | | |
| 0x10 | `ptr` / `ptr<space>` | `u8` space | |
| 0x14 | `code` | | New in TVISA |
| 0x20 | `vec<N x T>` | ULEB N, type index | |
| 0x21 | `svec<T>` | Type index | |
| 0x40–0x47 | GPU types | As TIR | `vft` (0x43) names a signature index instead of a function type; tag 0x48 is not used |

Tags 0x11–0x13, 0x22–0x28 and 0x30–0x33 do not occur in TVISA. An image that contains them is rejected with `TVISA-ENC-0003`.

### 7.3 Memory types

Memory accesses (section 13) name a memory type: any value type except `i1`, `code`, `svec<T>` and the register-only GPU types. A mask `vec<N x i1>` occupies N bytes in memory, each 0 or 1. Sizes and alignments are TIR.md section 8.2 and 8.3. `svec<T>` values reach memory only through the vector instructions of section 15.

### 7.4 Signatures

A signature is the lowered calling interface of a function: convention, flags, parameter items and result items (section 9.2). Signatures are interned in the TYPES section, part B, and referenced by index from function records, indirect calls, visible function tables and GPU entries. Two functions with the same signature index are call-compatible. The type tag used for control-flow integrity is carried separately (section 17.2), because two TIR function types with different `typeid`s can lower to one signature.

---

## 8. Functions, values and frames

### 8.1 Function records

SIGNATURES contains exactly `header.function_count` concatenated function records, with no count prefix. Each record is:

| Field | Encoding | Meaning |
| --- | --- | --- |
| Name | String index | Symbol or debug name |
| Linkage | `u8` | Section 6.3 |
| Signature | ULEB signature index | Section 7.4 |
| Target | `u8` | 0 CPU, 1 GPU function, 2 GPU entry |
| Attributes | ULEB bit set | Section 8.8 |
| Type tag | `u32` | Section 17.2. Present when attribute `address_taken` is set, otherwise absent |
| Variant | `u8` architecture (0 portable, 1 aarch64, 2 x64, 3 rv64, 4 wasm), ULEB feature count, feature string indices | Section 16; architecture codes follow TIR.md section 22.4 |
| Trust class | `u8` | 0 managed, 1 sandboxed, 2 unmanaged (TIR.md section 9.7) |
| Semantics | `u8` | 0 defined, 1 permissive (TIR.md section 10) |
| Personality | `u8` | 0 none, 1 `tir_personality` (section 14) |
| Alignment | `u8` log2 | At least 4 (16 bytes) |
| Placement | `u8` | 0 hot or normal code, 1 cold code, 2 startup code |
| Frame slots | ULEB count, then slot records | Section 8.4 |
| Debug scope | ULEB | One plus the function-scope index in DEBUG, or 0 when absent |

The variant field is always present. Architecture 255 denotes a function with no variant body and requires a zero feature count; codes 0 through 4 and their feature indexes have the meanings in section 16. Code 4 is the WebAssembly variant used by the `web` and `wasi` platform backends.

### 8.2 Bodies, blocks and values

1. A body is a list of blocks in reverse post-order of its control-flow graph. A block has typed parameters and ends in exactly one terminator (section 10). There are no phi nodes.
2. The first block's parameters are the function's incoming values: one value per leaf of each parameter item, in item order (section 9.2), followed by nothing else. A `byref`, `sret` or `cagg` item contributes one `ptr` value.
3. Values are numbered as in TIR (TIR.md section 22.5): first-block parameters (the function's incoming values) are counted once, then, for each subsequent block in order, its parameters and each instruction result. An instruction with several results (section 11.2) defines them in order, each as one value.
4. Every operand is defined before its use in a dominating position. Values are SSA: each is defined once.

### 8.3 Constants

Integer, floating-point and vector constants are instructions (`const.int`, `const.float`, `const.vec`, `const.zero`, `const.poison`). Large constants (vectors of more than 16 bytes, `f128`) are referenced by index into the CONSTANTS section. Strings and other constant data are data objects reached with `global.addr`.

### 8.4 Frame slots

A frame slot is a stack object whose size is known statically. Slot records:

| Field | Encoding | Meaning |
| --- | --- | --- |
| Size | ULEB | Bytes, < 2^31 |
| Alignment | `u8` log2 | At most 12 (4096 bytes) |
| Address space | `u8` | 0 CPU stack; in GPU code 4 `thread` or 3 `threadgroup` (threadgroup slots only in entries) |
| Flags | `u8` | Bit 0 `escapes`: the slot's address is stored to memory, passed to a call, or converted to an integer. Bit 1 `zeroinit`: zero-filled when its lifetime starts. Bit 2 `protected`: placed adjacent to the stack protector (section 17.6). Bit 3 `tagged`: memory-tagged (section 17.8). Bit 4 `scoped`: its lifetime is bracketed by `slot.start` and `slot.end`; otherwise it lives for the whole call |
| Debug variable | ULEB | One plus the variable index in DEBUG, or 0 when absent |

Rules:

1. `slot.addr n` yields the slot's address. A slot's address is valid only during its lifetime.
2. A `scoped` slot's lifetime starts at each `slot.start n` and ends at the matching `slot.end n`; on every path, starts and ends alternate, beginning with a start. Accessing a slot outside its lifetime is undefined behavior in permissive functions; the verifier rejects it in defined functions where it is statically visible and the lowering never produces it.
3. Slots whose lifetimes are disjoint MAY share storage. A `tagged` slot that shares storage is retagged at each `slot.start` (section 17.8).
4. `zeroinit` is how the lowering implements TIR.md section 10.5 for slots it could not prove initialized. The finalizer zero-fills such a slot at each start of its lifetime, and MAY omit the fill only where it proves that every byte is written before it is read.

### 8.5 Dynamic stack allocation

`stack.alloc.dyn %size, align` allocates `%size` bytes on the stack, aligned to `align` (a power of two, at most 4096), and yields its address; `stack.save` yields an opaque stack state and `stack.restore %s` releases every dynamic allocation made since that state. Dynamic allocations are released at function return. A function with dynamic allocations always has a frame pointer and probes each page it allocates (section 17.7).

### 8.6 Frame layout

The finalizer chooses the placement of frame slots, spill slots and saved registers, except for the following fixed points, which unwinders, profilers, the stack protector and audit tooling rely on.

| Architecture | Frame pointer points at | Frame record | Stack protector slot | Incoming stack arguments |
| --- | --- | --- | --- | --- |
| aarch64 | The frame record | `[x29]` saved x29, `[x29, #8]` saved x30 (signed) | `[x29, #-8]` | From the stack pointer at entry upward |
| x64 | The saved rbp | `[rbp]` saved rbp, `[rbp + 8]` return address | `[rbp - 8]` | From `[rbp + 16]` upward |
| rv64 | The canonical frame address (stack pointer at entry) | `[s0 - 8]` saved ra, `[s0 - 16]` saved s0 | `[s0 - 24]` | From `[s0]` upward |

Below the protector slot come, in this order: `protected` slots, other frame slots and saved callee-saved registers in an order the finalizer chooses, spill slots, the dynamic allocation area, and the outgoing argument area, which ends at the stack pointer. Every function maintains the frame pointer (section 5), including leaf functions, so that frame-pointer unwinding is always exact.

### 8.7 Leaf functions

A leaf function makes no calls of any kind, including `call.runtime`. On aarch64 and rv64, a leaf function that does not modify the link register and has no frame slots MAY omit its frame record; the unwinder then uses the function's unwind table (section 14.6). On x64 a leaf function in user space MAY place frame slots and spills in the 128-byte red zone below the stack pointer.

### 8.8 Function attributes

| Bit | Attribute | Meaning |
| --- | --- | --- |
| 0 | `address_taken` | The function's address is taken or it is reachable through a table; it has a landing pad and a type tag |
| 1 | `nounwind` | No exception leaves it |
| 2 | `noreturn` | Never returns |
| 3 | `cold` | Rarely executed; placed in cold code |
| 4 | `returns_twice` | Contains `setjmp` |
| 5 | `kernel_fp` | Kernel profile only: may use floating-point and vector registers (TIR.md section 31.1) |
| 6 | `no_fp_regs` | Uses no floating-point or vector registers, including for copies; set on every kernel function without `kernel_fp` |
| 7 | `sandboxed` | Trust class sandboxed; sandbox bounds checks are present and MUST be preserved |
| 8 | `has_asm` | Contains `asm` or `asm.goto` |
| 9 | `speculative_hardening` | Every conditional branch on a bounds check is followed by index masking (section 17.9) |
| 10 | `entry_point` | The image's entry function or an initializer or finalizer |
| 11 | `bridge` | An engine bridge or thunk (TIR.md section 6.5) |
| 12 | `interrupt_safe` | Kernel profile: callable with interrupts disabled; makes no call that can sleep |
| 13 | `naked_variant` | A variant body consisting of one `asm` instruction and a terminator; the finalizer emits no prologue or epilogue other than the landing pad (kernel and runtime use only) |

Bits 14 to 63 are zero.

---

## 9. Calling conventions

### 9.1 Conventions

| Code | Convention | Used for | Definition |
| --- | --- | --- | --- |
| 0 | `ridl` | Every function lowered from `abi(ridl)`: exports, imports, package-internal calls | Section 9.3 |
| 1 | `c` | `abi(c)`: `call.foreign`, callbacks into C, `.klffi` | Section 9.7 |
| 2 | `rtcc` | `libtirrt` entry points | Section 9.6 |
| 3 | `gpu` | Calls between GPU functions | Section 22.4 |
| 4 | `entry` | GPU entries | Section 22.5 |

### 9.2 Parameter and result items

A signature lists parameter items and result items. Each item is one of:

| Item | Code | Content | Meaning |
| --- | --- | --- | --- |
| `leaf T` | 0 | A value type | One scalar or vector value |
| `group` | 1 | Size, alignment, 2 to 4 members (offset, value type) | A small aggregate whose leaves travel in registers when they fit and by reference otherwise; offsets and types describe its memory layout |
| `byref` | 2 | Size, alignment, flags (bit 0 owned contents, bit 1 read only) | The address of caller-owned storage |
| `sret` | 3 | Size, alignment | The address of uninitialized caller storage the callee initializes (the indirect result) |
| `context` | 4 | | The RIDL context word (KLF.md section 22.2) |
| `cagg` | 5 | A C aggregate descriptor (section 9.7) | Convention `c` only |

Rules:

1. At most one `context` and one `sret` per signature; `sret` excludes result items.
2. A failing signature (flag `fails`) has an implicit error result: the call yields one extra trailing `ptr` value, zero on success (section 9.5).
3. A `group` member's value type is a scalar or a fixed vector of at most 16 bytes. Members do not overlap and lie within the group's size.
4. Result items are leaves and at most one group, flattened in order into result leaves (section 9.4).

Rationale: whether a small aggregate fits in registers depends on the architecture (x64 has six integer argument registers, aarch64 and rv64 have eight; rv64 flattens vectors). The lowering cannot know the architecture, so it describes the aggregate and the finalizer decides. The callee's body sees the leaves as values either way.

### 9.3 `ridl` on each architecture

Registers (KLF.md section 22.2 and TIR.md section 19.2):

| Architecture | Integer arguments | Float and vector arguments | Integer results | Float and vector results | Indirect result | Context | Error |
| --- | --- | --- | --- | --- | --- | --- | --- |
| aarch64 | x0–x7 | v0–v7 | x0–x3 | v0–v3 | x8 | x20 | x21 |
| x64 | rdi, rsi, rdx, rcx, r8, r9 | xmm0–xmm7 | rax, rdx, rcx, r8 | xmm0–xmm3 | rax | r13 | r12 |
| rv64 | a0–a7 | fa0–fa7 | a0–a3 | fa0–fa3 | t4 | s10 | s11 |

Leaf classes:

| Value type | aarch64 | x64 | rv64 |
| --- | --- | --- | --- |
| `i8`–`i64`, `ptr`, `ptr<device>`, `ptr<constant>`, `code`, handles | Integer, one register | Integer, one register | Integer, one register |
| `i128` | Two integer registers, low first | Same | Same |
| `f16`, `bf16`, `f32`, `f64` | Float, one register | Float, one register | Float, one register (NaN-boxed per the RISC-V psABI) |
| `f128` | Float, one register (q) | Float, one register (xmm) | Two integer registers, low first |
| `vec<N x T>` of at most 16 bytes | Float, one register | Float, one register | Expanded into its N lanes, each classified as its element type (vectors of `i1` as `i8` lanes) |
| `vec<N x T>` of more than 16 bytes | Never a leaf: the lowering passes it as `byref` | Same | Same |

Argument assignment. The finalizer processes the signature in this order, keeping one cursor per register class:

1. `sret`: the indirect result register.
2. `context`: the context register.
3. If the signature fails: the error register, which the caller sets to zero immediately before the call.
4. Each remaining item in order:
   1. `leaf` of one register: the next register of its class if one remains, otherwise the next stack slot.
   2. `leaf` of two registers (`i128`, and `f128` on rv64): the next two integer registers if two remain, otherwise a 16-byte stack slot aligned to 16; when one integer register remained, it stays available for later items.
   3. `group`: expand the members into leaves by the table. If the leaves number at most four and enough registers of every class remain, assign each leaf in member order to the next register of its class. Otherwise the caller stores the members into a caller-owned temporary of the group's size and alignment at their offsets, and passes the temporary's address as an integer leaf (rule 4.1). The callee loads the members from that address.
   4. `byref`: an integer leaf (rule 4.1).
5. Registers left unused by a group that went by reference remain available to later items.

Stack arguments occupy consecutive slots in parameter order, starting at the stack pointer at the call instruction on aarch64 and rv64 and at the stack pointer just after the pushed return address on x64. Each leaf takes an 8-byte slot, a 16-byte slot aligned to 16 for 16-byte leaves. The area is rounded up to 16 bytes.

Stack argument cleanup: the callee releases the stack argument area as it returns (callee-pop). On x64 the return is `ret imm16` with the area's size; on aarch64 and rv64 the epilogue adds the area's size to the stack pointer after authenticating or checking the return address (section 17.3). This is what makes `tail.call` possible for every `ridl` signature (TIR.md section 11.5): a tail call replaces the caller's incoming area with the callee's.

Rationale: KLF.md section 22.1 defines ridlcc as the platform base standard plus additions. Callee-pop is the one addition that the guaranteed tail calls of TIR require, and it matches LLVM's `swifttailcc`, so the server backend reuses an existing lowering. No RIDL 1.0 binaries have shipped, so the change breaks nothing (TIR.md section 19.2).

### 9.4 Results

1. Result leaves are classified by the table of section 9.3, with rv64 vector expansion.
2. If there are at most four leaves after expansion and they fit in the result registers of their classes, each leaf is returned in the next result register of its class.
3. Otherwise the caller passes the address of a temporary in the indirect result register, the callee stores the leaves at the group's offsets (single leaves at offset 0, consecutively and naturally aligned, when there is no group), and the caller loads them after the call. The call instruction still yields the leaves as values.
4. A signature with an `sret` item has no result leaves.

### 9.5 Failure and the error register

1. Before calling a failing function, the caller sets the error register to zero.
2. `return` in a failing function returns with the error register zero. `fail %e` returns with the error register holding `%e` (a pointer to a RIDL box, KLF.md section 22.5); the result registers are unspecified.
3. The call yields the error register's value as its trailing result. The lowering branches on it.
4. The error register is not preserved by failing calls. Across calls to non-failing functions it is callee-saved like any other register of the base standard.
5. A failing `tail.call` passes the callee's error register through to the caller's caller unchanged.

### 9.6 `rtcc`

`rtcc` is the convention of `libtirrt` entries (section 12). It is designed so that the frequent entries (retain, release, allocation fast paths) do not force spills in their callers.

| Architecture | Arguments | Results | Clobbered | Preserved |
| --- | --- | --- | --- | --- |
| aarch64 | x0–x3 | x0, x1 | The argument and result registers the entry uses, x16, x17, x30, flags | Every other general register and every SIMD and floating-point register in full |
| x64 | rdi, rsi, rdx, rcx | rax, rdx | The argument and result registers the entry uses, r10, r11, flags | Every other general register and every vector register in full, including upper halves |
| rv64 | a0–a3 | a0, a1 | The argument and result registers the entry uses, t0, t1, t3, ra | Every other general register and every floating-point and vector register in full, including `vl` and `vtype` |

1. Each entry declares how many argument and result registers it uses (section 12.2); registers beyond those are preserved.
2. Stack alignment and the shadow stack follow the base standard. Entries never unwind unless the entry table marks them `may_unwind`.
3. An entry that needs scratch state beyond the clobbered registers saves and restores it itself. `libtirrt` implements entries in TIR with the server backend's `preserve_all`-class lowering, in assembly variant bodies under the all-three rule, or both.

### 9.7 `c`

The `c` convention is AAPCS64, the System V AMD64 psABI, or the RISC-V LP64D psABI, with the hardening of section 17 on every function. Items are `leaf`, `byref`, `sret` and `cagg`.

A C aggregate passed by value is a `cagg` item: the caller supplies the address of the aggregate in its memory, and the finalizer moves the aggregate into registers, onto the stack, or into a temporary passed by reference, exactly as the psABI of the architecture classifies the C type from which the descriptor was made. The descriptor carries everything the three classifications read:

| Field | Meaning |
| --- | --- |
| Size, alignment | Of the C type |
| Members | Every scalar member at depth, arrays expanded, in offset order: offset, value type, and bit-field width or 0 |
| Flags | Bit 0 has a union (members may overlap); bit 1 has an unaligned member; bit 2 non-trivial for the purpose of calls (a C++ type passed by invisible reference); bit 3 has a flexible array member; bit 4 empty C++ record |

Members beyond offset 64 are omitted: no classification of the three psABIs reads them.

The same descriptor serves results: a `c` signature whose result is a C aggregate has an `sret` item and a descriptor, and the finalizer returns it in registers when the psABI says so.

Variadic `c` calls (`call.variadic`) name the number of fixed parameter items. Variadic arguments are passed after the default argument promotions, as the psABI requires: on x64 `al` holds an upper bound on the number of vector registers used; on rv64 floating-point variadic arguments travel in integer registers; on aarch64 variadic arguments follow the standard AAPCS64 rules. A function defined with the `c` convention and the `variadic` flag uses `va.start`, `va.arg`, `va.copy` and `va.end` on a 32-byte, 8-aligned frame slot that holds the architecture's `va_list` (32 bytes on aarch64, 24 on x64, 8 on rv64). RIDL-mode variadics (KLF.md section 29.5) are ordinary pointer arithmetic after lowering and never use these instructions.

### 9.8 Callee-saved registers and vector state

| Architecture | Callee-saved |
| --- | --- |
| aarch64 | x19–x29 (x20 is the context register, x21 the error register under section 9.5), low 64 bits of v8–v15. SVE `z` and `p` registers and SME state are caller-saved; the finalizer emits `smstop` before any call made in streaming mode and restores the mode after it |
| x64 | rbx, rbp, r12–r15. All vector registers are caller-saved. The finalizer emits `vzeroupper` before every call and return when the function may have dirtied the upper halves of ymm or zmm registers. Under APX, r16–r31 are caller-saved |
| rv64 | s0–s11 (s10 context, s11 error), fs0–fs11. Vector registers and `vl`, `vtype` are caller-saved |

### 9.9 Tail calls

1. `tail.call` to a `ridl` function is always performed as a tail call, for any signatures, provided the caller and callee are both `ridl` and have the same result items and failing flag.
2. `tail.call` to a `c` function requires the same result classification and a stack argument area no larger than the caller's incoming area; the verifier rejects others (`TVISA-VFY-0031`).
3. A tail call restores the caller's callee-saved registers, authenticates or checks the caller's return address (section 17.3), adjusts the stack argument area, and branches. On aarch64 the branch is `b` or `br x16`/`br x17` after the control-flow-integrity check; on x64 `jmp`; on rv64 `jr` through t1 with t2 holding the callee's label (section 17.2).
4. Mutual and self recursion through `tail.call` runs in constant stack space.

### 9.10 Calls between conventions

Calls between conventions happen only through thunks the lowering has made explicit: a thunk is an ordinary function of one convention that calls a function of another. The finalizer never converts conventions implicitly. A `code` value carries no convention; the signature at the call site does, and a nonforeign call's type tag check (section 17.2) ensures it matches the callee.

---

## 10. Control flow

### 10.1 Blocks and edges

Blocks and edges follow TIR.md sections 11.1 and 11.2: block parameters instead of phi nodes, critical edges allowed, successors repeated with different arguments allowed.

### 10.2 Terminators

| Instruction | Opcode | Semantics |
| --- | --- | --- |
| `br bbN(args)` | 0x1E0 | |
| `cond.br %c, bbT(args), bbF(args) [merge bbM]` | 0x1E1 | `merge` is required in GPU code and absent in CPU code |
| `switch.int %v, [n: bbN(args)]..., default bbD(args) [merge bbM]` | 0x1E2 | Cases sorted ascending, distinct |
| `br.indirect %addr, [bb...]` | 0x1E4 | CPU only; `%addr` from `block.addr` of the same function |
| `return %v...` | 0x1E5 | One operand per result leaf |
| `fail %e` | 0x1E6 | Failing functions only (section 9.5) |
| `tail.call callee(args)` | 0x1E7 | Section 9.9; callee direct, indirect (with signature and tag) or runtime |
| `unreachable [trap\|ub]` | 0x1E8 | `[trap]` traps with `TIR-RT-0001` |
| `trap code` | 0x1E9 | Section 17.10 |
| `invoke callee(args) to bbN(args) unwind bbU` | 0x1AA | Section 14 |
| `asm.goto ...` | 0x221 | Section 16.5 |

TIR's `switch.enum`, `try.call`, `coro.yield`, `coro.resume`, `coro.finish`, `throw`, `resume` and `deopt` do not occur in TVISA: the lowering has replaced them with the terminators above, explicit loads of tags and payloads, `call.runtime` and `tail.call`.

### 10.3 Structured control flow

GPU functions and entries keep the selection merges and loop declarations of TIR.md section 11.3, and satisfy S1 to S7 (`TVISA-CF-000N` for rule SN). GPU family backends rely on them for reconvergence (section 21.4). CPU code carries no structure annotations; CPU control flow may be irreducible.

### 10.4 Switches and jump tables

1. The finalizer and the server backend choose between compare trees, bit tests and jump tables.
2. A jump table lives in read-only data and holds 32-bit offsets from the table's start. The dispatch sequence bounds-checks the index before loading the offset; the check is never removed.
3. Jump-table targets and `br.indirect` targets carry landing pads: `bti j` on aarch64 and `endbr64` on x64. On rv64 the dispatch branches with `jr t2`, which Zicfilp treats as a software-guarded branch, and the targets carry no `lpad`.

### 10.5 Block placement

The finalizer lays out blocks in TVISA order, except that blocks reachable only through edges with a branch weight of zero, blocks ending in `trap` or `unreachable`, and landing pads are moved to the function's cold part. Out-of-line trap sequences (section 17.10) are shared per function and per trap code.

### 10.6 Metadata

TVISA uses the METADATA section record layout from TIR.md section 22.4. It
retains kinds 0–6 and 9, keyed by function index and instruction ordinal. Kind
7 (tbaa) and kind 8 (profile id) are invalid: type-based alias information is
expressed as alias scopes during lowering, and profiles have already been
applied. Every record refers to a defined function body, and its ordinal uses
the block/instruction order of CODE. Metadata never changes semantics in
defined functions; in permissive functions, violating range, nonnull, align or
noalias metadata is undefined behavior, as in TIR.

---

## 11. Instruction set

### 11.1 Principles

1. An instruction that TVISA shares with TIR has TIR's opcode, TIR's modifiers and TIR's semantics (TIR.md sections 12 to 17), restricted to TVISA's types. Nothing is renumbered.
2. Instructions TIR has and TVISA does not are invalid in TVISA (`TVISA-VFY-0002`). The lowering replaces each of them as section 11.4 states.
3. Instructions only TVISA has occupy the range 0x700 (section 11.5).
4. Where TIR yields a tuple, TVISA yields several results (section 11.2).
5. The encoding is TIR's (TIR.md section 22.5): ULEB opcode, ULEB modifier word, operand fields in the order of the textual syntax.

### 11.2 Multiple results

| Instruction | Results |
| --- | --- |
| `add.ovf`, `sub.ovf`, `mul.ovf` | Wrapped value `T`, overflow `i1` |
| `frexp` | Fraction `T`, exponent `i32` |
| `atomic.cmpxchg` | Old value `T`, success `i1` |
| `landingpad` | Exception record `ptr`, selector `i32` (section 14.3) |
| Calls (`call`, `call.indirect`, `call.runtime`, `call.variadic`) and `invoke` | One value per result leaf, then the error word `ptr` when the signature fails |
| `asm` | One value per output operand |
| Sparse-capable texture instructions with `[sparse]` | Value `T`, residency `i1` |
| `tex.gather` | Four values of the selected texture component scalar type |
| `tex.gather_compare` | Four `f32` comparison results |
| `tex.lod` | `vec<2 x f32>` |
| `tex.query size` | `i32` or `vec<2/3 x i32>` according to the texture dimension |
| `rq.get`, `rt.get`, or `rt.hitobject.get` of a matrix attribute | Twelve `f32` values in row-major order |
| `rt.get triangle_vertex_positions` | Nine `f32` values in vertex order (`v0.xyz`, `v1.xyz`, `v2.xyz`) |

### 11.3 Instructions kept from TIR

| TIR range | Kept instructions | Notes for TVISA |
| --- | --- | --- |
| 0x000 Constants | `const.int`, `const.float`, `const.bool`, `const.null`, `const.zero`, `const.poison`, `const.bytes`, `const.vec`, `func.ref`, `global.addr`, `tls.addr`, `block.addr`, `gpu.entry.ref`, `const.sampler` | `const.bytes` only for vector types. `func.ref @f` yields `code` and requires `@f` to be `address_taken`. `global.addr @d [+ n]` takes a data symbol and a constant byte addend. `gpu.entry.ref @e` yields `i64` (section 21.2). `const.sampler` is GPU-only and uses the fixed descriptor encoding of TIR.md section 22.5. `const.poison` only in permissive functions |
| 0x020 Integer | Every instruction | `icmp` also accepts two `code` operands with `eq` or `ne` |
| 0x060 Floating point and mathematics | Every instruction | `math.*` lower per section 12.4 |
| 0x0A0 Conversions | Every instruction | No conversion takes or yields `code` (section 11.5 has the code conversions) |
| 0x0C0 Vectors | Every instruction | Section 15 |
| 0x140 Memory | `raw.load`, `raw.store`, `ptr.offset`, `ptr.diff`, `mem.copy`, `mem.move`, `mem.set`, `prefetch` | Section 13 |
| 0x1A0 Calls | `call`, `call.indirect`, `call.variadic`, `invoke` | Section 11.6 |
| 0x1D0 Unwinding | `landingpad` | Section 14 |
| 0x1E0 Terminators | Section 10.2 | |
| 0x200 Atomics | `atomic.load`, `atomic.store`, `atomic.rmw`, `atomic.cmpxchg`, `fence` | Section 13.3 |
| 0x210 Variadics | `va.start`, `va.arg`, `va.copy`, `va.end` | `c` variadic functions only (section 9.7) |
| 0x220 Target | `asm`, `asm.goto`, `target.intrinsic`, `target.has`, `target.pagesize` | Section 16 |
| 0x230 Portable intrinsics | `intrinsic` | Section 12.5 |
| 0x240 Debug and hints | `debug.value`, `debug.addr`, `debug.label`, `assume`, `nop` | `assume` only in permissive functions |
| 0x270 Device I/O and kernel | Every instruction | Section 13.5 |
| 0x280 Platform interoperation | `foreign.addr`, `foreign.data`, `foreign.entry` | Platform modules only (TIR.md section 12.23) |
| 0x400–0x5C0 GPU | Every instruction | GPU profile only (section 21); field encodings are in TIR.md section 22.5 |

### 11.4 Instructions removed by the lowering

| TIR instructions | Replaced by |
| --- | --- |
| `const.string`, `const.aggregate`, `descriptor.ref`, `witness.ref` | Data objects reached with `global.addr`; string lengths as `const.int` |
| Aggregates (0x100 range) | Leaves, `raw.load`, `raw.store` and `ptr.offset` on memory laid out by TIR.md section 8 |
| `stack.alloc`, `stack.dealloc` | Frame slots with `slot.addr`, `slot.start`, `slot.end`; `stack.alloc.dyn`, `stack.save`, `stack.restore` |
| `field.addr`, `elem.addr`, `enum.payload.addr`, `addr.cast`, `addr.toptr`, `ptr.toaddr` | `ptr.offset` with constant or computed offsets, preceded by `trap.if` bounds checks where TIR checks |
| `enum.inject`, `enum.tag`, `enum.take`, `enum.project` | Stores and loads of tags and payloads; `trap.if` for failed preconditions |
| `load`, `load.borrow`, `store`, `copy.addr`, `move.addr`, `destroy.addr` | `raw.load`, `raw.store`, `mem.copy`, and calls of policy functions and descriptor entries |
| Heap and references (0x160 range) | `call.runtime` (section 12) |
| Ownership (0x180 range) | Explicit calls, `call.runtime`, nothing (for moves and borrows) |
| `closure.new`, `call.closure`, `closure.callback`, `call.witness`, `call.vtable` | Stores and loads of code and context words, `code.load`, `call.indirect` with a `context` item; callback thunks as ordinary `c` functions |
| `try.call` | `call` with a failing signature and a `cond.br` on the error word |
| `call.foreign` | `call` of a `c` import |
| Coroutines and async (0x1C0 range) | State-machine functions, frames in memory, `call.runtime` for frame allocation, `switch.int` on the resume index |
| `resume`, `throw`, `exception.payload`, `exception.typeid` | `call.runtime` and loads from the exception record (section 14) |
| `switch.enum` | Tag load and `switch.int` |
| `atomic.wait`, `atomic.notify` | Calls of LibTessera's `Sync.wait32`, `Sync.wait64` and `Sync.wake`, or of Tarwin's primitives in the kernel profile |
| `setjmp`, `longjmp` | `call.runtime` (section 14.5) |
| `expect`, `prof.count` | Branch-weight metadata; removed |
| `type.size`, `type.align`, `type.stride` | Constants, or loads from descriptors |
| `guard`, `deopt` | `cond.br` and `tail.call` to the fallback |

### 11.5 TVISA-only instructions (range 0x700)

| Opcode | Instruction | Result | Semantics |
| --- | --- | --- | --- |
| 0x700 | `slot.addr n` | `ptr` (`ptr<thread>` or `ptr<threadgroup>` in GPU code) | Address of frame slot n (section 8.4) |
| 0x701 | `slot.start n` | | Starts the lifetime of a `scoped` slot; applies `zeroinit` and `tagged` |
| 0x702 | `slot.end n` | | Ends it; a `tagged` slot is retagged (section 17.8) |
| 0x703 | `stack.alloc.dyn %size, align` | `ptr` | Section 8.5. A size of zero yields a unique non-null address |
| 0x704 | `stack.save` | `ptr` | Opaque stack state |
| 0x705 | `stack.restore %s` | | Releases dynamic allocations made since `%s` |
| 0x706 | `call.runtime id (args)` | Per entry | Calls runtime entry `id` (section 12) |
| 0x707 | `trap.if %c, code` | | If `%c` is true, traps with the 16-bit code; otherwise nothing |
| 0x708 | `code.load %addr, tag` | `code` | Loads a code pointer stored in storage form at `%addr` and converts it to register form (section 17.4). Authentication failure faults with `TIR-RT-0012` |
| 0x709 | `code.store %c, %addr, tag` | | Converts `%c` to storage form for `%addr` and stores it |
| 0x70A | `code.addr %c` | `ptr` | The entry address of `%c` without signature. Unmanaged |
| 0x70B | `code.fromaddr %p, tag` | `code` | A register-form code pointer for the entry address `%p`. System and kernel profiles only (`TVISA-VFY-0040`) |
| 0x70C | `thread.pointer` | `ptr` | The thread pointer, which addresses the thread control block (section 19) |
| 0x70D | `frame.address` | `ptr` | The current function's frame pointer |
| 0x70E | `return.address` | `ptr` | The current function's return address with any signature removed |
| 0x70F | `vscale.fixed` | `i64` | The `vscale` of the process; folded when the CPU target fixes it (section 15.2). Equivalent to `vec.vscale` and kept separately so GPU-free CPU code can query it without a vector type |

### 11.6 Calls

| Form | Syntax | Callee |
| --- | --- | --- |
| Direct | `call @f(args)` | A function, an import or a variant set; the signature is the callee's |
| Indirect | `call.indirect %c : sig, tag (args)` | A `code` value; a nonzero tag checks the callee's type tag (section 17.2), while zero marks a foreign call |
| Runtime | `call.runtime id (args)` | A runtime entry; the signature is the entry's (section 12) |
| Variadic | `call.variadic @f : sig, fixed n (args)` | A `c` function with the variadic flag |

`tail.call` and `invoke` take any of the first three forms; their modifier word's selector (bits 48–51) is 0 for direct, 1 for indirect and 2 for runtime. Call attributes `[cold]`, `[nounwind]`, `[noreturn]` use TIR's modifier bits. Arguments are one value per leaf of each parameter item (and one `ptr` per `byref`, `sret` or `cagg` item).

An indirect `c` call uses tag `0x00000000` only for a TIR `[foreign]` function value. Zero is a foreign-call marker, not a type tag: it requires the `c` convention, skips the TIR type-tag comparison and contributes TIR reason bit 3 (`foreign`). Every other indirect call uses a valid nonzero type tag. The same rule applies to indirect `tail.call` and `invoke`.

### 11.7 Modifier word

TVISA uses TIR's modifier word (TIR.md section 22.8) and assigns:

| Bit | Modifier | Meaning |
| --- | --- | --- |
| 52 | `unordered` | On `raw.load`, `raw.store`, `mem.copy` and `mem.move`: the access may race and has the unordered-atomic semantics of TIR.md section 15.4 rule 3. Backends MUST NOT introduce, duplicate or widen such accesses, and MUST perform an access of at most 8 naturally aligned bytes as one access. The lowering sets it on every raw access of a defined function |
| 53 | `ctime` | On `select`, comparisons and arithmetic produced from `ct.select` and `ct.eq` expansions: the instruction is lowered without branches and without data-dependent memory accesses |

Bits 54 to 63 are zero.

---

## 12. Runtime entry points

### 12.1 Model

Runtime entries are functions of `libtirrt` (TIR.md section 28.10) called with `call.runtime`. Each has a 16-bit identifier, a convention (`rtcc` or `ridl`), a signature, and flags. The finalizer binds identifiers to `libtirrt`'s exports through the image's runtime import table (section 18.4); there is one table per image, filled eagerly by the loader.

### 12.2 Entry table

All `rtcc` arguments and results are integer-class: floating-point values travel as their bit patterns. Flags: `N` noreturn, `U` may unwind, `T` returns twice, `P` pure (no side effects, may be removed if unused).

| Id | Name | Convention | Arguments → results | Flags | Semantics |
| --- | --- | --- | --- | --- | --- |
| 0x0001 | `heap.alloc` | rtcc | size, align, flags → ptr | | Raw allocation (TIR `heap.alloc.raw`); flags bit 0 uninitialized |
| 0x0002 | `heap.alloc.counted` | rtcc | size, align, descriptor → ptr | | Counted slot, strong count 1 |
| 0x0003 | `heap.alloc.unique` | rtcc | size, align, descriptor → ptr | | |
| 0x0004 | `heap.realloc` | rtcc | ptr, size, align → ptr | | |
| 0x0005 | `heap.free` | rtcc | ptr → | | Raw free; double free traps with `TIR-RT-0006` |
| 0x0006 | `ref.retain` | rtcc | ptr → ptr | | Strong count + 1 by address (interior pointers allowed); returns its argument |
| 0x0007 | `ref.release` | rtcc | ptr → | | Strong count − 1; destroys and frees at zero |
| 0x0008 | `ref.retain.n` | rtcc | ptr, n → ptr | | |
| 0x0009 | `ref.release.n` | rtcc | ptr, n → | | |
| 0x000A | `ref.isunique` | rtcc | ptr → i64 | P | 1 if the strong count is 1 |
| 0x000B | `weak.new` | rtcc | ptr → ptr | | |
| 0x000C | `weak.load` | rtcc | ptr → ptr | | Strong reference or null |
| 0x000D | `weak.release` | rtcc | ptr → | | |
| 0x000E | `uniq.free` | rtcc | ptr, descriptor → | | Destroys and frees a unique object |
| 0x000F | `slot.check` | rtcc | ptr → | | Traps with `TIR-RT-0004` if the slot was deleted |
| 0x0010 | `access.begin` | rtcc | ptr, kind, record → | | Dynamic exclusivity (KLF.md section 18.7); traps with `TIR-RT-0003` |
| 0x0011 | `access.end` | rtcc | record → | | |
| 0x0020 | `coro.alloc` | rtcc | size, align → ptr | | Coroutine and async frame |
| 0x0021 | `coro.free` | rtcc | ptr, size, align → | | |
| 0x0030 | `throw` | ridl | box, typeid → | N U | Starts two-phase unwinding |
| 0x0031 | `resume` | ridl | record → | N U | Continues unwinding from a cleanup pad |
| 0x0032 | `exception.take` | rtcc | record → box | | Ends handling: returns the owned box and frees the record |
| 0x0033 | `exception.rethrow` | ridl | record → | N U | Rethrows the exception being handled |
| 0x0040 | `setjmp` | rtcc | buffer → i64 | T | Section 14.5 |
| 0x0041 | `longjmp` | rtcc | buffer, value → | N | |
| 0x0050 | `mem.copy` | rtcc | dst, src, n → | | Out-of-line copy; overlapping ranges behave as `mem.move` |
| 0x0051 | `mem.move` | rtcc | dst, src, n → | | |
| 0x0052 | `mem.set` | rtcc | dst, byte, n → | | |
| 0x0053 | `mem.copy.unordered` | rtcc | dst, src, n → | | Copy with the `unordered` access semantics |
| 0x0060–0x0065 | `i128.sdiv`, `i128.udiv`, `i128.srem`, `i128.urem`, `i128.sdiv.wrap`, `i128.srem.wrap` | rtcc | a.lo, a.hi, b.lo, b.hi → r.lo, r.hi | P | Division by zero traps with `TIR-RT-0010`. MIN / −1 and MIN rem −1 trap with `TIR-RT-0002` in the first four and wrap in the `.wrap` forms (TIR `sdiv`/`srem` modes) |
| 0x0070–0x0082 | `f128.add`, `f128.sub`, `f128.mul`, `f128.div`, `f128.sqrt`, `f128.fma`, `f128.cmp`, `f128.frem`, `f128.min`, `f128.max`, `f128.minnum`, `f128.maxnum`, `f128.round`, `f128.roundeven`, `f128.floor`, `f128.ceil`, `f128.trunc`, `f128.ldexp`, `f128.frexp` | ridl | Operands as two integer leaves each, low first; rounding mode as a trailing `i32` where meaningful → result | P | IEEE 754-2019 binary128. `f128.cmp` returns the unordered/less/equal/greater code 3/0/1/2. Instructions with `[strict]` use the `.strict` forms at id + 0x0100, which also accumulate exception flags |
| 0x0088–0x0097 | `f128.from_i64`, `from_u64`, `from_i128`, `from_u128`, `to_i64`, `to_u64`, `to_i128`, `to_u128`, `from_f32`, `from_f64`, `to_f32`, `to_f64`, `from_f16`, `to_f16`, `from_bf16`, `to_bf16` | ridl | | P | Conversions; the `to_` integer forms take the mode of `fptosi`/`fptoui` as a trailing `i32` (0 sat, 1 trap) |
| 0x00A0 | `atomic128` | rtcc | op, ptr, a.lo, a.hi, b.lo, b.hi (order and op packed in `op`) → r.lo, r.hi, flag | | 16-byte atomics through the address-hashed lock table on rv64 without Zacas (TIR.md section 15.5) |
| 0x00A8 | `bridge.invoke` | ridl | function handle, argument area, result area → | U | Engine bridge into the interpreter or JIT (`tir_rt_invoke`) |
| 0x00B0 | `stack.overflow` | rtcc | → | N | Reports `TIR-RT-0021`; called by `stack.alloc.dyn` when an allocation would cross the stack limit (section 17.7) |
| 0x0800–0x08FF | Mathematics | ridl | Floating-point leaves → result | P | Section 12.4 |
| 0xF000–0xFFFF | Development entries | rtcc | Per `TVISA-RUNTIME-DEV.md` | | Sanitizer and instrumentation hooks; rejected in pipeline builds (`TVISA-VFY-0041`) |

Identifiers not listed are reserved. A TVISA version adds entries; it never renumbers or changes one.

### 12.3 Binding and versioning

1. Each image's runtime import table has one slot per entry the image uses. The loader fills the slots from the `libtirrt` of the device's train before any code of the image runs; the table is then read-only.
2. `libtirrt` of train T implements every entry of every TVISA version in T's window.
3. `call.runtime` lowers to a call through the slot (section 18.4). The finalizer and the server backend MAY inline the fast path of `ref.retain`, `ref.release` and `heap.alloc` when `TVISA-RUNTIME-INLINE.md` for the image's TVISA version publishes one; an inlined fast path is a normative instruction sequence like those of section 17 and falls back to the entry.

### 12.4 Mathematics

1. `math.*` instructions on `f32`, `f64` and `f128` lower to calls of runtime entries 0x0800 + 4 × k + t, where k is the instruction's position in the `math.*` part of TIR.md section 12.3 (`math.sin` is 0, `math.rsqrt` is 24) and t is 0 for `f32`, 1 for `f64` and 2 for `f128`. Instructions with `[approx]` use 0x0880 + 4 × k + t. Value t = 3 is reserved.
2. `f16` and `bf16` operands are converted to `f32`, computed with the `f32` entry, and rounded back; this meets the accuracy of TIR.md section 12.3 because every `f32` entry is correctly rounded.
3. Every CPU tier, including the interpreter, calls the same entries, so mathematical results are bitwise identical across tiers and architectures.
4. A backend MAY inline a mathematical function only with a sequence published in `TVISA-RUNTIME-INLINE.md` that produces identical results.
5. GPU mathematics is section 21.6.

### 12.5 Portable intrinsics

`intrinsic <id>` (TIR.md section 12.18) lowers to hardware instructions when the CPU target has them and otherwise to the sequences in `TVISA-RUNTIME-INLINE.md`:

| Id | aarch64 | x64 | rv64 |
| --- | --- | --- | --- |
| 0 `crc32c` | `crc32c{b,h,w,x}` (FEAT_CRC32) | `crc32` (SSE4.2) | Zbc `clmul` folding; table otherwise |
| 1 `crc32` | `crc32{b,h,w,x}` | `pclmulqdq` folding | Zbc folding |
| 2–6 AES | `aese`, `aesmc`, `aesd`, `aesimc` (FEAT_AES) | `aesenc`, `aesenclast`, `aesdec`, `aesdeclast`, `aesimc` | Zvkned when present; constant-time bitsliced software otherwise |
| 7 `sha1.block` | FEAT_SHA1 | SHA-NI | Software |
| 8 `sha256.block` | FEAT_SHA256 | SHA-NI | Zvknha |
| 9 `sha512.block` | FEAT_SHA512 | SHA512 extension when present | Zvknhb |
| 10 `keccak.f1600` | FEAT_SHA3 (`eor3`, `rax1`, `xar`, `bcax`) | Software | Software |
| 11 `spin.hint` | `yield` | `pause` | `pause` (Zihintpause) |
| 12 `counter.read` | `mrs CNTVCT_EL0` | `rdtsc` on invariant-TSC CPUs; otherwise the LibTessera clock | `rdtime` |
| 13 `counter.frequency` | `mrs CNTFRQ_EL0` | Calibrated value from the thread control block's LibTessera state | Device-tree timebase from LibTessera |
| 14 `debug.break` | `brk #0xF000` | `int3` | `ebreak` (never listed in the trap-site table, section 17.10) |
| 15 `ct.select` | `csel` | `cmov` | Zicond `czero.eqz`, `czero.nez`, `or` |
| 16 `ct.eq` | Loop of `eor`/`orr` with no early exit | Same with `xor`/`or` | Same |

Software sequences of intrinsics 2 to 10 MUST be constant time.

---

## 13. Memory, atomics and device I/O

### 13.1 Loads and stores

1. `raw.load T, %p [align n] [volatile] [nontemporal] [unordered]` and `raw.store` access `sizeof(T)` bytes. `T` is a memory type (section 7.3).
2. `align n` states the true alignment. Absent, the alignment is 1. A backend MUST produce correct code for any true alignment the instruction states, including misaligned scalars and vectors, on every architecture.
3. `volatile` accesses are performed exactly once, in program order relative to other volatile accesses, with the stated width, and are never merged, split or elided. A naturally aligned volatile access of at most 8 bytes is one machine access.
4. `nontemporal` maps to `ldnp`/`stnp` on aarch64, `movnti`/`movntdq`/`vmovntdq` (stores) and `movntdqa` (loads from write-combining memory) on x64, and the `ntl.all` hint on rv64 (Zihintntl). It is a hint and never changes results.
5. `i1` is never loaded or stored: the lowering loads `i8` and converts with `icmp ne` or `trunc`, and stores `zext` results.

### 13.2 Bulk memory

1. `mem.copy`, `mem.move` and `mem.set` with constant sizes up to 256 bytes are expanded inline; larger or variable sizes MAY be expanded inline (for example with `cpyp`/`cpym`/`cpye` and `setp`/`setm`/`sete` on aarch64 with FEAT_MOPS, `rep movsb`/`rep stosb` on x64 with ERMS and FSRM, vector loops with RVV on rv64) or call entries 0x0050 to 0x0053.
2. In functions with `no_fp_regs`, expansions use only general registers or the entries' `rtcc`, which preserves vector registers.
3. `[unordered]` bulk operations copy each naturally aligned 8-byte word that lies inside both ranges with one access.

### 13.3 Atomics

TVISA uses the ordering and scope rules in TIR.md section 15.6. Atomic order bits 20–22 use 0 for omitted/default and codes 1–5 for relaxed, acquire, release, acq_rel and seq_cst; failure-order bits 23–25 are legal only on compare-exchange, where 0 selects the default derived from success. Explicit failure orders are relaxed, acquire or seq_cst and cannot be stronger than or incompatible with success. Scope bits 26–28 use 0 for the CPU/GPU default and 1–5 for thread, subgroup, threadgroup, device and system; values 6–7 are invalid. Loads reject release/acq_rel, stores reject acquire/acq_rel, CPU code rejects subgroup/threadgroup/device scope, and GPU atomics require a `device` or `threadgroup` pointer. The atomic modifier word carries only order, failure order, scope, `weak` on compare-exchange, and the RMW operation selector; all other modifier bits are zero.

Atomic values follow TIR.md section 15.6: a trivial fixed-layout value of 1, 2, 4, 8 or 16 bytes. Integer and floating-point RMW operations require a scalar integer or floating-point value respectively; `xchg` accepts any atomic value.

One mapping per architecture is normative, because native code, finalized code, JIT code and the interpreter of one process share memory and their atomics must interoperate.

| Operation | aarch64 (FEAT_LSE, FEAT_LRCPC) | x64 | rv64 |
| --- | --- | --- | --- |
| Load relaxed | `ldr` | `mov` | `l{b,h,w,d}` |
| Load acquire | `ldapr` | `mov` | `l{b,h,w,d}; fence r,rw` |
| Load seq_cst | `ldar` | `mov` | `fence rw,rw; l{b,h,w,d}; fence r,rw` |
| Store relaxed | `str` | `mov` | `s{b,h,w,d}` |
| Store release | `stlr` | `mov` | `fence rw,w; s{b,h,w,d}` |
| Store seq_cst | `stlr` | `xchg` | `fence rw,w; s{b,h,w,d}` |
| Read-modify-write | `ldadd`, `ldclr`, `ldeor`, `ldset`, `ldsmax`, `ldsmin`, `ldumax`, `ldumin`, `swp` with `a`, `l` or `al` suffix per order; others as a `cas` loop | `lock xadd`, `xchg`; others as a `lock cmpxchg` loop | `amo<op>.{w,d}` with `.aq`, `.rl` or `.aqrl` per order; 8- and 16-bit and the rest as `lr`/`sc` loops (or Zabha `amo<op>.{b,h}`) |
| Compare and exchange | `cas{,a,l,al}` | `lock cmpxchg` | Zacas `amocas.{w,d}`; `lr`/`sc` loop otherwise |
| 16-byte | `caspal`; FEAT_LSE128 `ldclrp`, `ldsetp`, `swpp` when present; 16-byte loads and stores with `ldp`/`stp` under FEAT_LSE2 | `lock cmpxchg16b`; aligned 16-byte loads and stores with `vmovdqa` on CPUs whose vendor guarantees their atomicity, which the CPU target records | Zacas `amocas.q`; runtime entry 0x00A0 otherwise |
| Fence acquire | `dmb ishld` | none (compiler barrier) | `fence r,rw` |
| Fence release | `dmb ish` | none (compiler barrier) | `fence rw,w` |
| Fence acq_rel, seq_cst | `dmb ish` | `mfence` | `fence rw,rw` |

The rv64 column is the mapping of the RISC-V psABI's atomics ABI (its Table A.6 convention). Scope `thread` is a compiler barrier only; every other CPU scope is the system scope. An atomic whose alignment is not proven is preceded by an alignment test that traps with `TIR-RT-0007`.

### 13.4 Floating-point environment

Instructions with `[rm=…]` set the rounding mode, execute, and restore it (`FPCR.RMode` on aarch64, `MXCSR.RC` or EVEX embedded rounding on x64, the instruction's `rm` field on rv64). `[strict]` instructions accumulate exception flags into the thread's flags, which `fp.flags.read` returns: `FPSR` on aarch64, `MXCSR` on x64, `fflags` on rv64. Code without `[strict]` neither reads nor depends on the flags. The default environment is round-to-nearest-even with all exceptions masked and subnormals preserved (`FPCR.FZ` and `FZ16` clear, `MXCSR.FTZ` and `DAZ` clear).

### 13.5 Device I/O

| Instruction | aarch64 | x64 | rv64 |
| --- | --- | --- | --- |
| `mmio.load` relaxed | One `ldr{b,h,,}` with a base-plus-immediate address and no writeback | One `mov` | One `l{bu,hu,wu,d}` |
| `mmio.load` acquire | `ldar{b,h,}` | `mov` | Load, then `fence i,ir` |
| `mmio.store` relaxed | One `str{b,h,,}` with no writeback | One `mov` | One `s{b,h,w,d}` |
| `mmio.store` release | `stlr{b,h,}` | `mov` | `fence rw,o`, then the store |
| `fence.io read` | `dmb oshld` | `lfence` | `fence i,ir` |
| `fence.io write` | `dmb oshst` | `sfence` | `fence ow,ow` |
| `fence.io full` | `dsb osh` | `mfence` | `fence iorw,iorw` |

Device accesses never use load or store pairs, writeback addressing, vector registers or compressed rv64 encodings, so that every access carries a complete instruction syndrome a hypervisor can emulate (Tarwin.md hypervisor). `kernel.percpu.addr @g` lowers to `mrs xN, TPIDR_EL1` plus the variable's offset on aarch64, a `gs:`-relative address on x64, and the per-CPU base convention of Tarwin.md on rv64.

---

## 14. Exceptions and unwinding

### 14.1 Model

TVISA keeps TIR's two-phase, table-based model (TIR.md section 14). In TVISA, an `invoke` names its unwind block; an unwind block begins with `landingpad`; throwing and resuming are runtime calls. The finalizer and the server backend produce unwind tables (section 14.6) and language-specific data areas (section 14.4) that `tir_personality` reads.

### 14.2 `invoke`

`invoke callee(args) to bbN(args) unwind bbU` calls as `call` does. On normal return control continues in `bbN`, whose parameters receive the call's results first and then the listed arguments. If an exception propagates out of the callee, control enters `bbU` during the cleanup phase when its landing pad matches or is a cleanup. A call that may unwind and is not an `invoke` propagates exceptions to the caller.

### 14.3 Landing pads and the exception record

`landingpad [clauses]` is the first instruction of an unwind block. Clauses are TIR's: `catch.type <typeid>` (64-bit), `catch.filter @pred`, `catch.all`, `cleanup`. It yields the exception record and the selector: the index of the matching clause, or −1 for a cleanup.

Exception record layout (allocated by the runtime, owned by the unwinder until `exception.take`):

| Offset | Field |
| --- | --- |
| 0 | `ptr` the owned RIDL box of the exception (KLF.md section 21.6) |
| 8 | `u64` the payload type's `typeid` |
| 16 | 48 bytes private to `libtirrt` (the unwinder header) |

`exception.payload` lowers to a load of the box and an offset by the box header (16 bytes rounded up to the payload's alignment). `exception.typeid` lowers to a load at offset 8. A handler that keeps the exception calls `exception.take` (entry 0x0032); a cleanup pad ends with `call.runtime resume` (entry 0x0031) and `unreachable`.

### 14.4 Language-specific data area

Each function with landing pads, or with a terminate code, has an LSDA in the native image's read-only data. Its layout:

| Field | Encoding |
| --- | --- |
| Header | `u8` version (1), `u8` terminate code kind (0 none, 1 boundary, 2 nounwind), `u16` reserved, ULEB call-site count, ULEB action count, ULEB type count |
| Call-site table | Per entry: ULEB start offset from the function's entry, ULEB length, ULEB landing-pad offset (0 for none), ULEB first action + 1 (0 for no action, a cleanup-free call) |
| Action table | Per action: SLEB clause kind and index (positive: type table index + 1 for `catch.type`; negative: −(filter index + 1) for `catch.filter`; 0 for `catch.all`), SLEB offset to the next action or 0 |
| Type table | Per entry: `u64` typeid |
| Filter table | Per entry: `u32` image-relative offset of the filter function |

Call sites are sorted by start offset and do not overlap. A cleanup is represented by a landing pad with no action. A call site not covered by any entry is a call that may unwind with no landing pad in this frame.

`tir_personality` behaves as follows:

1. Search phase: for each action of the call site in order, `catch.type` matches when the typeids are equal, `catch.filter` when the filter returns true, `catch.all` always. A frame whose terminate code kind is 1 or 2 and that has no matching action ends the search: the unwinder traps with `TIR-RT-0020` (kind 1, the frame of an `exported` function or a `c` callback) or `TIR-RT-0024` (kind 2, a `nounwind` frame) before running any cleanup.
2. Cleanup phase: frames are unwound up to the handler; each landing pad on the way runs with the selector of its matching clause or −1.

The lowering gives terminate kind 1 to every `exported` function and every `c` function reachable from outside the package, and kind 2 to every `nounwind` function that contains a call that may unwind.

### 14.5 Non-local jumps

`setjmp` and `longjmp` are runtime entries 0x0040 and 0x0041. The buffer is 256 bytes aligned to 16 (the C `jmp_buf` of `tir64`). `setjmp` saves the callee-saved registers, the stack pointer, the frame pointer, the return address (signed with the IB key and the stack pointer as modifier on aarch64), the shadow-stack pointer, and the floating-point control register. `longjmp` restores them, unwinds the shadow stack to the saved pointer (`incsspq` on x64, `gcspopm` loops on aarch64 with GCS, writing `ssp` on rv64), and returns `value` (1 if `value` is 0) from `setjmp`. A function that calls `setjmp` has `returns_twice`; the finalizer keeps every value live across the call in memory.

### 14.6 Unwind tables

1. Every function, leaf or not, has an unwind entry. Unwind information is DWARF Call Frame Information in the native image's `.eh_frame`, indexed by a sorted binary-search table in `.eh_frame_hdr`.
2. The CIE of aarch64 functions records return-address signing, and `DW_CFA_AARCH64_negate_ra_state` tracks it at every `paciasp` and `autiasp`.
3. Frame-pointer unwinding is always valid at call sites; CFI is exact at every instruction so that asynchronous unwinding (profilers, crash reports) works in prologues and epilogues.
4. When the cleanup phase transfers control to a landing pad, the unwinder pops the shadow stack by the number of frames removed before resuming.
5. JIT code registers equivalent tables with the runtime (TIR.md section 28.5). GPU code and the kernel profile have no unwind tables for exceptions; the kernel keeps CFI for backtraces (Tarwin.md).

---

## 15. Vectors and masks

### 15.1 Fixed vectors

1. Fixed vectors are legalized per target: wider vectors are split into the widest native registers, narrower ones are widened with the extra lanes unused, and `vec<3 x T>` is computed in four lanes. Unused lanes never trap, never raise floating-point flags observable through `[strict]` and never reach memory.
2. Masks (`vec<N x i1>`) are represented as element-sized lane masks on Advanced SIMD and AVX2, as predicate registers under SVE2, as `k` registers under AVX-512 and AVX10, and in `v0` under RVV.
3. Lane 0 is at the lowest address and in the least significant bits of a register on every architecture.

### 15.2 Scalable vectors

1. `svec<T>` has `vscale × (128 / bits(T))` lanes (TIR.md section 16.2).
2. Each image records its `svec` mode: `agnostic`, when its code reads the vector length at run time (aarch64 targets with `sve2`, every rv64 target), or `fixed(v)`, when `vscale` is folded into its code (aarch64 targets without `sve2`: v = 1; `x64.v3`: v = 2; `x64.v4` and later: v = 4). Agnostic code reads the length with `cntb`/`cntw` and `vsetvli`.
3. The process's `vscale` is decided before any code runs, from the executable's image. For `fixed(v)` it is v; on aarch64 CPUs with SVE2, Tarwin then sets the process's SVE vector length to 128 × v bits, so agnostic images in the same process see the same `vscale`. For `agnostic` it is the hardware's: VL / 128 at the vector length Tarwin grants on aarch64, VLEN / 128 on rv64.
4. The loader maps an image only if it is agnostic or fixed to the process's `vscale`. Otherwise it maps that image's build for a target with a matching mode, or refuses with `TVISA-RT-0002`. Every image of a package is built for one target, and system frameworks are installed for every mode the device's targets use, so a refusal indicates a damaged installation.
5. On aarch64 without SVE2 `svec` lowers to Advanced SIMD with explicit masking; on x64 to AVX2 or AVX-512 registers of 128 × `vscale` bits.
6. The interpreter and the JIT tiers use the process's `vscale`.
7. `svec` loops use predication: SVE2 `whilelo` predicates, RVV `vl`, AVX-512 `k` masks, AVX2 masked loads and stores (`vpmaskmov`, `vmaskmov`) with blends.

### 15.3 Ordered reductions

`vec.reduce fadd` and `fmul` without `reassoc` are performed in lane order: `fadda` under SVE2, `vfredosum` under RVV, a sequential chain of scalar operations elsewhere. With `reassoc` the backend chooses the order.

### 15.4 Narrow floats

Conversions among `f8e4m3`, `f8e5m2`, `f6e2m3`, `f6e3m2`, `f4e2m1`, `e8m0` and wider types use hardware where the target has it (FEAT_FP8 on aarch64, AVX10.2 conversions on x64, and the RVV OFP8 conversion extension on rv64 when the CPU target has it) and the exact software sequences of `TVISA-RUNTIME-INLINE.md` elsewhere. Results are identical either way.

---

## 16. Variant bodies, target intrinsics and inline assembly

### 16.1 Variant sets

The VARIANTS section carries TIR's variant sets (TIR.md section 20) unchanged: set name, members with architecture, features and function index. Architecture codes are 0 portable, 1 aarch64, 2 x64, 3 rv64 and 4 wasm. For a CPU target, the finalizer and the server backend apply TIR.md section 20.2 rule 4: among the members of the target's architecture whose features the target guarantees, the one with the most features, declaration order breaking ties; otherwise the portable member. Members not selected are not compiled.

### 16.2 Load-time dispatch

When some member needs features the CPU target only may have, the set becomes a dispatch set:

1. The image holds one dispatch slot per set in relocated read-only data, with a relocation listing the candidate members in order of preference and their features.
2. The loader evaluates the candidates against the CPU's features once, writes the chosen member's address into the slot (in storage form on aarch64, section 17.4), and the slot becomes read-only with the rest of relocated data.
3. Calls to a dispatch set are calls through its slot, using the import call sequence of section 18.4; taking its address loads the slot.

### 16.3 `target.has` and `target.pagesize`

`target.has "feature"` folds to a constant when the CPU target guarantees or excludes the feature, and otherwise loads a byte from the image's feature table, which the loader fills from the CPU's features before any code runs and then makes read-only. `target.pagesize` folds to 16384 on aarch64 and 4096 on x64 and rv64.

### 16.4 Target intrinsics

1. `target.intrinsic "name"(args) -> T` names an entry of the target intrinsic registry, `TVISA-INTRINSICS.md`, of the image's TVISA version.
2. Each registry entry gives: the name (the LLVM target intrinsic name, for example `llvm.aarch64.neon.tbl1` or `llvm.x86.avx512.vpdpbusd.512`), the architecture, the required features, the TVISA signature, and the machine instruction or instructions it denotes, with a reference to the architecture manual.
3. The registry covers: Advanced SIMD, SVE, SVE2, SME and SME2, MTE, CRC, cryptography and FEAT_FP8 on aarch64; SSE through SSE4.2, AVX, AVX2, FMA, F16C, BMI, AVX-512 (every subset), AVX10, AMX, VAES, VPCLMULQDQ, GFNI, SHA and APX-related intrinsics on x64; RVV 1.0 and its ratified extensions (Zvbb, Zvbc, Zvkn, Zvks, Zvfh, Zvfbfwma and later) and the scalar extensions on rv64.
4. Both the finalizer and the server backend implement every registry entry. An entry exists only when both do (section 34.1).
5. An intrinsic whose required features the CPU target does not guarantee is allowed only in a variant body that requires those features; the finalizer rejects others with `TVISA-ARCH-0002`.

### 16.5 Inline assembly

1. The ASM section carries each assembly instruction's template and constraint string exactly as TIR has them (TIR.md section 20.4): LLVM syntax for the architecture.
2. The finalizer contains an integrated assembler for each architecture that accepts every instruction of every feature in section 30.1, the directives `.byte`, `.hword`, `.word`, `.quad`, `.p2align`, `.inst`, local numeric labels, and LLVM's operand modifiers for the architecture.
3. Supported constraints: aarch64 `r`, `w`, `x`, `y`, `Upa`, `Upl`, `I`, `J`, `K`, `L`, `M`, `N`, `Q`, `m`, `i`, `n`; x64 `r`, `q`, `Q`, `a`, `b`, `c`, `d`, `S`, `D`, `x`, `v`, `Yk`, `k`, `m`, `i`, `n`, `e`, `Z`, `@cc<cond>`; rv64 `r`, `f`, `vr`, `vm`, `I`, `J`, `K`, `A`, `m`, `i`, `n`. Every architecture: `=` output, `+` read-write, `&` early clobber, tied operands by number, and the clobbers `memory`, `cc` and register names. Constraint strings use LLVM's comma-separated output, input and clobber order. An entry may concatenate supported constraint codes or use `|` alternatives; when alternatives are used, every entry has the same number of alternatives. A tied input uses a decimal output index. Each output constraint corresponds to one `asm` result; `+` also consumes one explicit input. Ordinary inputs consume one explicit input each; clobbers consume none. `asm.goto` has no outputs.
4. Templates MUST NOT read or write x18 on aarch64, `gp` or `tp` on rv64, or the frame pointer, and MUST leave the stack pointer, the shadow stack and the thread pointer as they found them; `[privileged]` templates in the kernel profile are exempt from the thread-pointer rule only.
5. The assembled bytes of a template are identical in the finalizer, the server backend and `tirc`'s integrated assembler; the conformance suite compares them (section 34.1).
6. The analysis summary lists every assembly region. Audit tooling (section 34.5) excludes assembly regions from sequence checks and reports them.

---

## 17. Control-flow integrity and hardening sequences

### 17.1 Overview

This section turns TIR.md section 27 into exact machine code. The sequences below are normative: in instruction choice, order and, where encodings are given, bytes. Registers written `Xc`, `Rc`, `Xa` are chosen by the register allocator; every other register is fixed. The sequences use only the scratch registers of section 5.1, which the allocator never assigns.

| Mechanism | `app`, `system`, `driver` | `kernel` | GPU |
| --- | --- | --- | --- |
| Type tags and landing pads (17.2) | Every `address_taken` function | Same, supervisor forms | Not applicable; `gpu.call.vft` is typed and bounded |
| Return-address protection (17.3) | PAC, plus GCS when present; CET shadow stack; Zicfiss | PAC with kernel keys; supervisor shadow stack; supervisor Zicfiss | Not applicable |
| Code-pointer signing (17.4) | aarch64 | aarch64, kernel keys | |
| Stack protector (17.6) | Section 17.6 rule | Every function with an `escapes` slot | |
| Stack probes (17.7) | Yes | Yes | |
| Stack memory tagging (17.8) | `tagged` slots on `+mte` targets | Tarwin.md's decision | |
| Speculative hardening (17.9) | `speculative_hardening` and `sandboxed` functions | Tarwin.md's mitigations | |
| Traps (17.10) | Yes | Yes | Section 21.7 |

### 17.2 Forward-edge control-flow integrity

Type tag. The tag of a TIR function type is computed from its `typeid` (TIR.md section 7.9): t = the low 32 bits of the `typeid`; while t is forbidden, t = (t + 1) mod 2^32. A value t is forbidden when any of the following holds:

| Condition | Reason |
| --- | --- |
| (t & 0xFFFFF) = 0 | A Zicfilp label of 0 accepts any caller |
| (t & 0xFFF) = 0x017 | The word would decode as an rv64 `lpad` |
| t = 0xFA1E0FF3 or (2^32 − t) mod 2^32 = 0xFA1E0FF3 | The tag word or the call site's immediate would contain `endbr64` |
| t ∈ {0xD503241F, 0xD503245F, 0xD503249F, 0xD50324DF, 0xD503233F, 0xD503237F} | The word would decode as an aarch64 `bti` or `pacixsp` landing pad |

`tirc` computes tags in its analysis stage, so TIR's control-flow-integrity types (TIR.md section 21.3) and TVISA's tags agree. `tag16` below is t & 0xFFFF; `label` is t & 0xFFFFF.

The tag value zero is reserved as the foreign-call marker of section 11.6 and is never a function type tag. It is permitted only on an indirect call whose signature uses the `c` convention; every code-pointer load, store or conversion and every other indirect call uses a valid nonzero function type tag.

Callee layout. An `address_taken` function with entry address E (aligned to 16) is laid out as:

| Architecture | Bytes before E | At E |
| --- | --- | --- |
| aarch64 | E − 4: the tag as a little-endian word | `bti c`, then the prologue |
| x64 | E − 5: `B8`; E − 4: the tag, little-endian (together: `mov eax, imm32`) | `endbr64` (`F3 0F 1E FA`), then the prologue |
| rv64 | E − 4: the tag as a little-endian word | `lpad label`, then the prologue |

Padding between functions is `udf #0` words on aarch64, `CC` bytes on x64 and zero words (illegal instructions) on rv64.

Indirect call. `call.indirect %c : sig, tag (args)` with `%c` in a register (`Xc` is never x16 or x17; `Rc` is never r10 or r11; `Rc` is never t0–t3, ra or t4 on rv64):

aarch64 (`%c` in register form, section 17.4):

```
mov    x16, Xc
xpaci  x16
ldur   w16, [x16, #-4]
movz   w17, #tag16
movk   w17, #(t >> 16), lsl #16
cmp    w16, w17
b.eq   1f
brk    #0x000C                  // TIR-RT-0012
1:
movz   x17, #tag16
blraa  Xc, x17
```

x64:

```
mov    r11, Rc
mov    r10d, (2^32 − t)         ; 41 BA imm32
add    r10d, dword ptr [r11-4]  ; 45 03 53 FC
je     1f                       ; 74 07
ud1    eax, dword ptr [rax+12]  ; 0F B9 80 0C 00 00 00   TIR-RT-0012
1:
call   r11                      ; 41 FF D3
```

rv64:

```
mv     t1, Rc
lw     t0, -4(t1)
lui    t3, %hi(t)
addiw  t3, t3, %lo(t)
beq    t0, t3, 1f
addi   t0, zero, 12             # TIR-RT-0012
ebreak
1:
lui    t2, label
jalr   ra, 0(t1)
```

`%hi(t)` is ((t + 0x800) >> 12) & 0xFFFFF and `%lo(t)` is t − (%hi(t) << 12) as a signed 12-bit value.

Indirect tail call. The check is the same. Then, on aarch64, `mov x16, Xc` (the signed value), the epilogue of section 17.3 up to but excluding the return, `movz x17, #tag16` and `braa x16, x17`; on x64 the epilogue and `jmp r11` (`41 FF E3`); on rv64 the epilogue, `lui t2, label` and `jr t1`.

Direct calls (`bl`, `call rel32`, `jal` or `auipc ra`/`jalr ra`) need no check. Calls to imports, dispatch sets and runtime entries use section 18.4.

### 17.3 Backward-edge integrity

Prologue of a function that saves its return address (every non-leaf function, and every leaf that keeps a frame record):

| aarch64 | x64 | rv64 |
| --- | --- | --- |
| `bti c` (if `address_taken`) | `endbr64` (if `address_taken`) | `lpad label` (if `address_taken`) |
| `paciasp` | `push rbp` | `sspush ra` |
| `stp x29, x30, [sp, #-16]!` | `mov rbp, rsp` | `addi sp, sp, -16` |
| `mov x29, sp` | | `sd ra, 8(sp)` |
| | | `sd s0, 0(sp)` |
| | | `addi s0, sp, 16` |

Epilogue, after the frame has been deallocated down to the frame record:

| aarch64 | x64 | rv64 |
| --- | --- | --- |
| `ldp x29, x30, [sp], #16` | `pop rbp` | `ld ra, 8(sp)` |
| `autiasp` | `ret` or `ret imm16` (stack arguments) | `ld s0, 0(sp)` |
| `add sp, sp, #n` (stack arguments, n > 0) | | `addi sp, sp, 16` |
| `ret` | | `sspopchk ra` |
| | | `addi sp, sp, n` (stack arguments, n > 0) |
| | | `ret` |

1. On aarch64 with FEAT_GCS the hardware Guarded Control Stack is enabled by Tarwin for every process; `bl` pushes and `ret` checks without further instructions. Without FEAT_FPAC, a failed `autiasp` yields a non-canonical address and `ret` faults; Tarwin reports both as `TIR-RT-0013`.
2. On x64 the CET shadow stack checks `ret` in hardware.
3. A leaf function without a frame record (section 8.7) has no `paciasp`, `sspush` or `sspopchk`.
4. `naked_variant` bodies have only the landing pad.
5. Large frames are allocated after the prologue with the probing of section 17.7.

### 17.4 Code pointers

On aarch64 every code pointer is signed with the IA key. Two forms exist:

| Form | Where | Discriminator |
| --- | --- | --- |
| Register form | SSA values of type `code`, function arguments and results, spills | `tag16`, zero-extended |
| Storage form | Code pointers in memory: vtables, witness tables, closure code words, function values in data structures, import and dispatch slots | (A & 0x0000FFFFFFFFFFFF) \| (tag16 << 48), where A is the address of the 8-byte slot |

Sequences (aarch64). `Xd` is the result register, `Xa` the storage address:

`func.ref @f`:

```
adrp   x16, f
add    x16, x16, :lo12:f
movz   x17, #tag16
pacia  x16, x17
mov    Xd, x16
```

`code.load %a, tag`:

```
ldr    x16, [Xa]
mov    x17, Xa
movk   x17, #tag16, lsl #48
autia  x16, x17
mov    x17, x16                 // check: omitted on targets with FEAT_FPAC
xpaci  x17                      //
cmp    x16, x17                 //
b.eq   1f                       //
brk    #0x000C                  //
1:                              //
movz   x17, #tag16
pacia  x16, x17
mov    Xd, x16
```

`code.store %c, %a, tag`:

```
mov    x16, Xc
movz   x17, #tag16
autia  x16, x17
mov    x17, x16                 // check: omitted on targets with FEAT_FPAC
xpaci  x17                      //
cmp    x16, x17                 //
b.eq   1f                       //
brk    #0x000C                  //
1:                              //
mov    x17, Xa
movk   x17, #tag16, lsl #48
pacia  x16, x17
str    x16, [Xa]
```

`code.addr %c`: `mov x16, Xc`, `xpaci x16`, `mov Xd, x16`. `code.fromaddr %p, tag`: `mov x16, Xp`, `movz x17, #tag16`, `pacia x16, x17`, `mov Xd, x16`. `return.address`: `ldr x16, [x29, #8]`, `xpaci x16`, `mov Xd, x16`.

On x64 and rv64, `code` values are plain entry addresses: `func.ref` is a PC-relative address computation, `code.load` and `code.store` are 64-bit loads and stores, `code.addr` and `code.fromaddr` are moves.

Data relocations of kind CODE64 and IMPORTCODE64 (section 18.3) produce storage-form pointers: the loader signs each with the slot's address and the relocation's tag before the containing data becomes read-only.

### 17.5 Kernel forms

In the kernel profile the same sequences run at EL1, in supervisor mode and in ring 0: `paciasp`, `pacia` and `autia` use the kernel's keys (Tarwin.md), `bti` landing pads are enforced on kernel text, and supervisor shadow stacks (x64 CET supervisor shadow stacks, rv64 Zicfiss with `sse` enabled for S-mode) back returns. `fs` and `tpidr_el0` are not used; the kernel's thread pointer and per-CPU conventions are Tarwin.md's.

### 17.6 Stack protector

A function has a stack protector when it has a `protected` slot, or a dynamic allocation, or when its trust class is `unmanaged` or `sandboxed` and it has an `escapes` slot. In the kernel profile, every function with an `escapes` slot has one. The value is the thread control block's word at offset 8 (section 19).

| | aarch64 | x64 | rv64 |
| --- | --- | --- | --- |
| Store, after the prologue | `mrs x16, tpidr_el0` / `ldr x16, [x16, #8]` / `stur x16, [x29, #-8]` | `mov r11, qword ptr fs:[8]` (`64 4C 8B 1C 25 08 00 00 00`) / `mov qword ptr [rbp-8], r11` (`4C 89 5D F8`) | `ld t0, 8(tp)` / `sd t0, -24(s0)` |
| Check, before the epilogue and before every tail call | `mrs x16, tpidr_el0` / `ldr x16, [x16, #8]` / `ldur x17, [x29, #-8]` / `cmp x16, x17` / `b.eq 1f` / `brk #0x000E` / `1:` | `mov r11, qword ptr [rbp-8]` (`4C 8B 5D F8`) / `xor r11, qword ptr fs:[8]` (`64 4C 33 1C 25 08 00 00 00`) / `je 1f` (`74 07`) / `ud1 eax, dword ptr [rax+14]` (`0F B9 80 0E 00 00 00`) / `1:` | `ld t0, 8(tp)` / `ld t1, -24(s0)` / `beq t0, t1, 1f` / `addi t0, zero, 14` / `ebreak` / `1:` |

A failed check traps with `TIR-RT-0014`. Kernel forms read the value from the location Tarwin.md defines.

### 17.7 Stack probes

1. The probe interval P is 16384 bytes on aarch64 and 4096 bytes on x64 and rv64. Tarwin.md guarantees a guard region of at least 64 KiB below every stack.
2. A function whose fixed frame exceeds P allocates it in steps of at most P bytes, writing to the new stack pointer after each step: `str xzr, [sp]` (aarch64), `mov qword ptr [rsp], 0` (x64), `sd zero, 0(sp)` (rv64). Frames larger than 16 × P use a loop with the same per-step write.
3. `stack.alloc.dyn` first compares the new stack pointer against the stack limit at offset 16 of the thread control block and calls runtime entry `stack.overflow` (0x00B0) when it would cross it; it then allocates in steps of at most P bytes with the same writes.
4. The outgoing argument area and spill area are part of the fixed frame for this purpose.

### 17.8 Stack memory tagging

On CPU targets with the `mte` feature, `tagged` slots are tagged as follows. The sequence is not byte-exact; these properties are normative:

1. Each invocation draws one random tag base with a single `irg` from the stack pointer, held in a callee-saved register for the function's duration.
2. Each tagged slot's address is formed with `addg` from the base, with a tag offset distinct from every other tagged slot of the function and different from the stack pointer's tag.
3. At each start of a slot's lifetime its granules are tagged with `stg`, `st2g`, or, for `zeroinit` slots, `stzg`/`stz2g`, which also zero them.
4. At each end of a slot's lifetime, and on every exit from the function including unwinding through a landing pad, its granules are restored to the stack pointer's tag.
5. Tagged slots are aligned to 16 bytes and their sizes rounded up to 16.

### 17.9 Speculative hardening

In functions with `speculative_hardening` or `sandboxed`, every bounds check that guards a memory access (a `trap.if` or conditional branch on an unsigned comparison of an index with a limit, which the lowering marks with the `ctime` modifier on the comparison) is followed on the in-bounds path by index masking:

| aarch64 | x64 | rv64 |
| --- | --- | --- |
| `cmp Xi, Xn` / `b.hs trap` / `csel Xi, Xi, xzr, lo` / `csdb` | `xor r10d, r10d` / `cmp Ri, Rn` / `jae trap` / `cmovae Ri, r10` | `bgeu Ri, Rn, trap` / `sltu t0, Ri, Rn` / `czero.eqz Ri, Ri, t0` |

### 17.10 Traps

A trap with 16-bit code c (TIR.md section 34.6) is:

| Architecture | Encoding |
| --- | --- |
| aarch64 | `brk #c` |
| x64 | `ud1 eax, dword ptr [rax + c]`: `0F B9 80` followed by c as a 32-bit little-endian value |
| rv64 | `addi t0, zero, c` when c < 2048, otherwise `lui t0, %hi(c)` and `addiw t0, t0, %lo(c)`; then `ebreak` |

1. `trap.if` and `trap` sites MAY be inline or shared out-of-line blocks; the sequences of sections 17.2 and 17.6 are inline as written.
2. The native image's `.tirtrap` section lists every trap site: a `u32` image-relative offset of the `brk`, `ud1` or `ebreak` and the `u16` code, sorted by offset. The table is authoritative: Tarwin reports an exception at a listed site as the TIR trap with that code, and treats an unlisted `brk`, `int3`, `ud1` or `ebreak` as a breakpoint or an invalid instruction.
3. Overflow, division and conversion checks of TIR's trapping modes lower to the native flags or comparisons followed by `trap.if`.

### 17.11 General requirements

| TIR.md section 27.1 requirement | TVISA realization |
| --- | --- |
| Position-independent code at randomized addresses | Section 18.1; no text relocations |
| Never writable and executable | The finalizer writes code into a Memory object it then seals; images map code `Read \| Execute` (TIR.md section 28.2) |
| Non-executable stacks, probed frames | Section 17.7 |
| Frame pointers | Section 8.6 |
| Zero-initialization | `zeroinit` slots (section 8.4) |

### 17.12 Audit properties

Audit tooling (section 34.5) checks, for every native image outside recorded assembly regions:

1. Every indirect call or indirect tail call is the sequence of section 17.2 or a call through an import, dispatch or runtime slot (section 18.4).
2. Every function reachable from a relocation of kind CODE64, from a `func.ref`, or from an export begins with the landing pad and is preceded by its tag.
3. Every function that saves its return address uses the prologue and epilogue of section 17.3.
4. Every function the rule of section 17.6 covers stores and checks the protector.
5. Every `.tirtrap` entry points at a trap instruction, and every trap instruction emitted for a TIR trap is listed.
6. No page is writable and executable; no code section has relocations.

---

## 18. Code model, relocations and linking

### 18.1 Code model

1. User-space images are position independent with a small code model: all code and data of one image lie within a 2 GiB span. An image whose code and data would exceed it is rejected with `TVISA-FIN-0010`.
2. aarch64 reaches data with `adrp` plus `add` or a load; direct calls are `bl`. If an image's code exceeds the `bl` range of 128 MiB, the finalizer inserts veneers `adrp x16, f` / `add x16, x16, :lo12:f` / `br x16` between functions, and every function reached through a veneer begins with `bti c`.
3. x64 reaches data and functions with RIP-relative addressing and `call rel32`.
4. rv64 reaches data with `auipc` pairs and calls with `auipc ra` and `jalr ra`, relaxed to `jal ra` when in range. `gp`-relative relaxation is never used.
5. The kernel code model is Tarwin.md's (`tirc build --code-model kernel`).

### 18.2 Native image layout

The finalizer is also the image's static linker. It produces, in this order: `.text.startup`, `.text`, `.text.cold`, `.rodata`, `.eh_frame_hdr`, `.eh_frame`, `.tirlsda`, `.tirtrap`, `.data.rel.ro` (including `.tirimport`, `.tirfeat` and dispatch slots), `.data`, `.bss`, `.tdata`, `.tbss`, then the non-loaded sections `.tirgpu`, `.ridlmeta`, `.tirdeps`, `.tirbridge`, `.tirrt`, the build identifier and the signature, inside the platform image format of Tarwin.md. Within a section, functions and data objects appear in TVISA order. Every choice is deterministic (section 29).

### 18.3 Data objects and relocations

A data object in the DATA section has: kind (0 read-only, 1 relocated read-only, 2 writable, 3 zero-initialized, 4 thread-local initialized, 5 thread-local zero-initialized, 6 GPU constant data), an independent per-CPU flag, alignment, size, contents (absent for kinds 3 and 5) and relocations. Each relocation is (offset, kind, symbol, addend):

Textual data-kind spellings are `readonly`, `relro`, `writable`, `zero`, `tls`, `tls_zero` and `gpu_constant`, corresponding to binary kinds 0 through 6 in order. The optional `percpu` marker follows the data kind and sets the independent per-CPU flag; it is valid only in the kernel profile and cannot be combined with `tls`, `tls_zero` or `gpu_constant`. The linkage is optional in text and defaults to `local`; this permits the short form shown in Appendix A.1's example. Text `align` values are positive powers of two in bytes. Kinds `zero` and `tls_zero` use the `zero size` initializer; all other kinds use `bytes` or the aggregate initializer. A `bytes` initializer decodes Kira string escapes and stores the UTF-8 encoding of the decoded text.

For a textual `bytes` initializer, Kira string escapes are the ordinary
escapes in TIR section 23.1; the decoded string is encoded as UTF-8. This is
distinct from the byte-specific `const.bytes` operand in TIR section 12.1.

The aggregate initializer begins with its total byte size. Each `dataelem` gives a byte offset and a quoted sequence of hexadecimal byte pairs; bytes not covered by an element are zero. Elements are nonempty, ordered by increasing offset, do not overlap, and lie within the declared size. Hex pairs are in increasing byte-address order. This form represents arbitrary object bytes, including bytes that are not valid UTF-8.

| Kind | Name | Width | Value |
| --- | --- | --- | --- |
| 0 | ABS64 | 8 | Address of the symbol plus addend |
| 1 | CODE64 | 8 | Code pointer to a defined function, in storage form on aarch64 (tag from the function's record) |
| 2 | REL32 | 4 | Address of the symbol plus addend minus the address of the field |
| 3 | GPUENTRY64 | 8 | The process-unique identifier of a GPU entry (section 21.2) |
| 4 | TPOFF64 | 8 | Offset of a thread-local object from the thread pointer |
| 5 | IMPORT64 | 8 | Address of an imported data object |
| 6 | IMPORTCODE64 | 8 | Code pointer to an imported function, in storage form on aarch64 |
| 7 | DISPATCH64 | 8 | Code pointer to the selected member of a dispatch set |

A relocation in a read-only object (kind 0) is allowed only for REL32, which the finalizer resolves itself; objects that need load-time values are kind 1, 2 or 4.

In binary DATA, the section starts with a ULEB object count. Each object is: ULEB name string index, `u8` linkage (0 local, 1 image, 2 exported), `u8` data kind with bits 0–2 holding the kind (0 through 6 above), bit 3 the per-CPU flag, and bits 4–7 zero, ULEB alignment, ULEB size, ULEB debug index, initializer bytes, ULEB relocation count, then relocations. The per-CPU flag is valid only in the kernel profile and cannot accompany kinds 4–6. The initializer is exactly `size` raw bytes for kinds 0, 1, 2, 4 and 6; kinds 3 and 5 have no initializer bytes. A debug index of zero means no data debug record; nonzero values are the one-based record index in DEBUG. The name string must be nonempty and unique in DATA.

Each binary relocation is ULEB offset, `u8` kind (0 ABS64, 1 CODE64, 2 REL32, 3 GPUENTRY64, 4 TPOFF64, 5 IMPORT64, 6 IMPORTCODE64, 7 DISPATCH64), ULEB symbol operand in section 6.4's tagged index space, SLEB addend, `u8` flags, and an optional `u32` little-endian tag when flags bit 0 is set. Other flag bits are zero. Only CODE64 and IMPORTCODE64 may carry a tag; when present it must equal the target function's type tag. Every relocation field is in bounds and does not overlap another field. A zero-initialized object may have relocations, but its allocated storage is zero-filled before relocation.

In text, a relocation addend is zero when omitted. Its optional signed integer is written as `+magnitude` or `-magnitude` and must fit in `i64`; binary stores that value as SLEB.

### 18.4 Imports and slots

1. Imports bind eagerly: the loader resolves every import of an image, checks it (RIDL link check, TIR.md section 34.6 `TIR-RT-0200`), and fills the image's slots before any code of the image runs. `.data.rel.ro` is then read-only. There is no lazy binding and no procedure linkage table.
2. Each function import, dispatch set and runtime entry the image uses has one 8-byte slot; each data import has one 8-byte slot.
3. Calls through a slot:

| aarch64 | x64 | rv64 |
| --- | --- | --- |
| `adrp x17, slot` | `call qword ptr [rip + slot]` (`FF 15` rel32) | `auipc t1, %pcrel_hi(slot)` |
| `add x17, x17, :lo12:slot` | | `ld t1, %pcrel_lo(slot)(t1)` |
| `ldr x16, [x17]` | | `lui t2, label` |
| `movk x17, #tag16, lsl #48` | | `jalr ra, 0(t1)` |
| `blraa x16, x17` | | |

Tail calls through a slot end with `braa x16, x17`, `jmp qword ptr [rip + slot]` (`FF 25` rel32) and `jr t1`. Exported functions and runtime entries are `address_taken`, so they carry landing pads and tags; the call through a slot needs no tag comparison because the loader checked the binding and the slot is read-only.

4. Data imports are reached by loading the slot (plain, unsigned) and adding the offset.

### 18.5 Load-time relocations

The finalizer resolves every reference within the image. The loader applies only: rebasing of ABS64 values; signing of CODE64 values (aarch64); binding of IMPORT64, IMPORTCODE64, runtime and function import slots; TPOFF64 values for images that are not the executable; DISPATCH64 selection; and the feature table. Their encoding in the image is Tarwin.md's.

---

## 19. Thread pointer, thread control block and thread-local storage

### 19.1 Thread control block

The thread pointer of every user-space thread addresses its 64-byte thread control block (TCB), aligned to 64:

| Offset | Field | Set by |
| --- | --- | --- |
| 0 | `ptr` the TCB's own address | LibTessera at thread creation |
| 8 | `u64` stack protector value | LibTessera: one random value per process, copied into every thread's TCB |
| 16 | `ptr` stack limit: the lowest address the thread's current stack may use | LibTessera; updated by stack switches |
| 24 | `ptr` `libtirrt` thread state | `libtirrt` |
| 32 | `ptr` LibTessera thread state | LibTessera |
| 40 | `ptr` shadow-stack base of the current stack | LibTessera |
| 48, 56 | Reserved, zero | |

No other code reads or writes the TCB except through `thread.pointer` in `libtirrt` and LibTessera.

### 19.2 Thread-local storage

1. TLS is static only. Every image's TLS block lives at a fixed offset from the thread pointer, the same in every thread.
2. aarch64 and rv64: blocks lie above the TCB. The executable's block starts at TP + 64 rounded up to its alignment; frameworks follow in load order.
3. x64: blocks lie below the TCB. The executable's block ends at TP rounded down to its alignment; frameworks precede it in load order.
4. The loader reserves a static TLS surplus of 64 KiB per thread for images loaded after process start. When such an image loads, the loader copies its TLS template into every existing thread's surplus before the image's code runs. An image whose TLS does not fit is refused with `TVISA-RT-0003`.
5. `tls.addr @g` in the executable uses its offset as a constant; in other images it adds the image's TLS offset, read from a relocated read-only word (TPOFF64), to the thread pointer.

### 19.3 Thread creation

LibTessera creates threads (TIR.md section 15.1), allocates the TCB and TLS area, initializes TLS from the templates, sets the TCB fields above and the architecture's thread-pointer register, and only then runs code compiled from TVISA on the thread.

---

## 20. Debug information and symbolication

### 20.1 The DEBUG section

TVISA's DEBUG section uses the shared eight-table encoding of TIR.md section 33.1, with locations keyed by TVISA instruction ordinal and variables tied to TVISA values (`debug.value`) or addresses (`debug.addr`, frame slots). The lowering carries locations, inlined-at chains and variables through every transformation it performs. Its final DATA table indexes TVISA DATA objects in DATA order. A function record's nonzero debug-scope field names a function-scope record whose function index matches that record's position after imported functions. A frame slot's nonzero debug-variable field names a variable record whose scope belongs to that function.

Variable and label records are zero-based in their order within DEBUG. `debug.value` and `debug.addr` refer to a variable record; `debug.label` refers to a label record.

TVISA text uses the same `!var("name", type, "scope")` and
`!label("name", "scope")` operands defined in TIR.md section 12.19. The
variable type uses the TIR type grammar because it describes the source
variable and may be wider than the lowered TVISA value type. A trailing
`!loc(...)` may follow either debug attachment.

The DATA section's debug index is zero when an object has no data debug record; otherwise it is one plus that object's ordinal among data-object records in DEBUG. The ordinal follows DATA order. A DEBUG section with no data debug records uses zero for every DATA object.

### 20.2 Server-built images

The server backend emits DWARF 5 for every native image and moves it to the symbol bundle (TIR.md section 33.2). Line tables and variable locations are complete at `-O0` and best effort above it, as in LLVM.

### 20.3 Finalized images

1. A device-finalized image carries unwind tables, the trap-site table and a function-start table, but no DWARF.
2. Its native image header records the TVISA image hash, the CPU target and feature set, and the finalizer build identifier.
3. Symbolication re-finalizes: the pipeline, or a developer with the same finalizer build, finalizes the same TVISA image for the same target, checks that the build identifier is equal, and emits DWARF from that run. The finalizer's code output is identical with and without debug-information output (section 29.1).
4. Devices in developer mode MAY keep DWARF for finalized images.

### 20.4 GPU code

GPU family backends emit line tables where the family's tools support them (section 22.7). `gpu.printf` is compiled only in development builds.

---

## 21. The GPU profile

### 21.1 Scope

GPU functions and entries in a TVISA image carry TIR's GPU model (TIR.md section 17) after the lowering: the GPU instructions of the ranges 0x400 to 0x5C0, the scalar and vector instructions of section 11.3 restricted by TIR.md section 17.15, frame slots in the `thread` and `threadgroup` spaces, and structured control flow. Everything TIR verified about GPU code (rules V15 and V16, uniformity) holds in TVISA, and the TVISA verifier re-checks the structural part (section 26.3).

Additional GPU rules in TVISA:

1. Every direct call between GPU functions is inlined by the lowering, except calls marked `noinline` by the producer for size, which family backends compile with the `gpu` convention (section 22.4). The GPU call graph is acyclic, so inlining terminates.
2. Generic, aggregate and ownership constructs are gone, as on CPUs. Register-only types (`rayquery`, `hitobject`, `coop`) remain as values and in `thread` slots.
3. `gpu.spec_const` remains an instruction so that family backends can fold it at pipeline creation (section 21.8).
4. GPU synchronization instructions retain TIR's defaults and scope semantics: `gpu.barrier` without an explicit scope uses threadgroup scope, and `gpu.memfence` without an explicit scope uses device scope (TIR.md sections 15.3 and 17.9).

### 21.2 GPU entry records

The GPU section has one record per entry:

| Field | Encoding |
| --- | --- |
| Function | ULEB function-space index; kinds 0–12 reference target 2, kind 13 references target 1 |
| Kind | `u8`: 0 compute, 1 object, 2 mesh, 3 vertex, 4 fragment, 5 tile, 6 raygen, 7 intersection, 8 anyhit, 9 closesthit, 10 miss, 11 callable, 12 node, 13 visible (a `target(gpu)` function the producer exported for visible function tables; its record carries only the function and its TIR function type's `typeid`) |
| Threads | 3 × ULEB, or 0 0 0 for `threads(dynamic)` |
| Root | ULEB size, `u8` alignment log2, `u64` typeid, `u8` space (1 device, 2 constant), or a zero size when absent |
| Immediate | ULEB size (≤ 256), `u64` typeid |
| Stage input | ULEB count; per field: `u8` semantic (section 21.3), ULEB semantic index, ULEB location, value type index, `u8` interpolation flags: bit 0 flat, bit 1 perspective, bit 2 noperspective, bit 3 centroid, bit 4 sample, bit 5 invariant, bit 6 per-primitive, ULEB byte offset in the stage struct |
| Stage output | Same encoding |
| Payload | ULEB object payload size, ULEB ray payload size, ULEB hit attribute size, ULEB callable data size |
| Mesh | ULEB max vertices, ULEB max primitives, `u8` topology (0 points, 1 lines, 2 triangles) |
| Threadgroup memory | ULEB static bytes |
| Capabilities | ULEB bit set, bit i the i-th optional capability of TIR.md section 17.18 in its listed order |
| Subgroup size | `u8` log2, or 0 for any |
| Floating point | `u8`: bit 0 fast, bit 1 IEEE denormals |
| Specialization constants | ULEB count; per constant: ULEB unsigned 32-bit id, value type index, default as a CONSTANTS index |
| Node | For kind 12: `u8` launch mode (0 broadcast, 1 coalescing, 2 thread), 3 × ULEB maximum dispatch grid, ULEB input record size, ULEB output count, then per output a string-table index for its name, `u8` kind (0 node, 1 mesh), ULEB record size and ULEB maximum records |
| Ray tracing | For kinds 6 to 11: `u8` maximum recursion depth (≤ 31) |
| Fragment | `u8` flags: bit 0 `early_fragment_tests`, bit 1 writes depth, bits 2–3 depth mode (0 none, 1 any, 2 greater, 3 less), bit 4 discards or demotes, bit 5 reads attachments |

The loader gives each image's entries consecutive process-unique identifiers starting at a base registered with `libtirrt`'s GPU entry registry; GPUENTRY64 relocations and `gpu.entry.ref` resolve to base + record index. Ember looks entries up by identifier (Ember.md).

For a non-visible entry, its `entry` signature parameter items are ordered as
the root (when present), immediate data (when present), then the entry's stage
parameters in their TIR order. The root is one `leaf` whose type is
`ptr<device>` or `ptr<constant>` matching the root record's `space`. Immediate
data occupies one signature parameter item; a `group` expands to its member
values, and `leaf` or `byref` contributes one value. The first block expands
these items in signature order as section 8.2 specifies. This order lets the
verifier recover the root and immediate roles for uniformity analysis from the
entry record without adding role indexes to the binary format.

The possible target set of `gpu.call.vft` is every `visible` record in the
image whose function has the instruction's encoded signature index. A verifier
checks every target's body using the caller entry's stage and capability set;
the runtime table may contain a subset of this set. These edges participate in
the GPU call-graph acyclicity check.

### 21.3 Stage semantics

| Code | Semantic | Code | Semantic |
| --- | --- | --- | --- |
| 0 | User varying (location) | 9 | `@clip_distance(n)` |
| 1 | `@position` | 10 | `@cull_distance(n)` |
| 2 | `@color(n)` | 11 | `@point_size` |
| 3 | `@color(n)` with `@index(1)` | 12 | `@layer` |
| 4 | `@depth(any)` | 13 | `@viewport` |
| 5 | `@depth(greater)` | 14 | `@primitive_id` |
| 6 | `@depth(less)` | 15 | `@view_index` |
| 7 | `@stencil` | 16 | `@cull` |
| 8 | `@sample_mask` | 17 | `@shading_rate` (output) |

The semantic index is `n` for codes 2, 3, 9 and 10 (`color(n)`, `color_index1(n)`, `clip_distance(n)` and `cull_distance(n)`) and zero for every other code. Stages link by semantic code, semantic index and location. Vertex or mesh outputs and fragment inputs match when those three values and the value type are equal; an input with no matching output reads zero. This is what lets Ember link precompiled stage binaries without compiling (section 21.9).

### 21.4 Control flow and reconvergence

1. Family backends MUST implement maximal reconvergence (TIR.md section 17.8): invocations that diverge at a selection or loop header reconverge at its merge block, and subgroup, quad and derivative operations act on exactly the invocations the structured control flow makes active.
2. Backends realize this with the family's mechanism (execution masks and scalar branches on AMD, convergence barriers on NVIDIA, the SIMD control-flow stack on Intel, predication on Adreno, Mali and PowerVR, execution masks on Apple), never by assuming that hardware reconverges where the source does not say so.
3. Loops whose exit condition is non-uniform keep all invocations of the subgroup in the loop until each has exited; operations after the merge see the reconverged set.

### 21.5 Memory and resources

1. GPU pointers are 64-bit virtual addresses of the process's GPU address space (DeviceKit.md). `ptr.offset`, `raw.load` and `raw.store` with an address space, and the atomics of section 13.3 with scopes, are the memory instructions.
2. Resource handles have one format on every family:

| Handle | Bits 0–31 | Bits 32–63 |
| --- | --- | --- |
| `texture` | Index into the process's texture descriptor heap | Zero |
| `sampler` | Index into the process's sampler descriptor heap | Zero |
| `accel` | GPU virtual address of the acceleration structure | |
| `vft` | GPU virtual address of the function table | |
| `tensor` | GPU virtual address of the tensor descriptor (section 22.6) | |

Index 0 of each heap is the null descriptor and address 0 is the null object (TIR.md section 17.4 rule 2). Ember owns the heaps; their descriptor formats are the family's hardware formats, written by Ember from the family layer that ships with each backend (section 22.6).

3. A handle used with a non-uniform value is legal in every stage. On families that need uniform descriptors, the backend emits a loop over the distinct handle values of the active invocations.
4. Out-of-bounds texture and buffer accesses through handles follow Ember.md's robustness rules; out-of-bounds accesses through raw GPU pointers fault the dispatch (TIR.md section 17.4 rule 3).

### 21.6 Floating point and mathematics

1. Accuracy is TIR.md section 17.16. Each family backend lowers `math.*` with the family's transcendental hardware plus Tessera-owned correction sequences, and proves its bounds against the reference executor (TIR.md section 28.11) over the conformance corpus and exhaustively for `f16`.
2. `fp(precise)` code MUST NOT contract, reassociate or approximate beyond the bounds. `fmuladd` is fused or unfused per family; each family's choice is fixed in its ABI annex and never varies by optimization level.
3. Subnormal `f32` behavior follows the entry's floating-point mode; a backend sets the family's denormal control accordingly.

### 21.7 Traps on GPUs

`trap`, `trap.if` and `unreachable [trap]` in GPU code terminate the dispatch or draw. The backend uses the family's trap instruction where it has one, and otherwise writes the code and the entry identifier into the dispatch's fault record and ends every invocation. Ember reports the code to the process (Ember.md). Integer division by zero and the other trapping modes of TIR behave as on CPUs: the backend emits the checks.

### 21.8 Specialization constants

Specialization constant IDs are unsigned 32-bit TIR integer literals, as in TIR.md section 17.18. Their values use the integer, floating-point, and fixed-vector types and literal rules in that section. Defaults are encoded as same-typed CONSTANTS records (section 24.4). Pipeline binaries are compiled with default values. When Ember creates a pipeline with other values, the finalizer's GPU path compiles the entry again from TVISA with the constants folded; the result is cached under the code cache key of TIR.md section 28.8, which includes the constants.

### 21.9 Separate compilation and linking

1. Each entry is compiled alone. Stage binaries carry interface descriptors (semantic code and index, location, type, interpolation) and resource-usage metadata.
2. Ember creates graphics and ray tracing pipelines by linking stage binaries: matching interfaces, assigning family-level parameter slots, and building shader binding tables. Linking never recompiles a shader body and never requires information beyond the stage binaries and Ember's pipeline state.
3. Families whose hardware requires cross-stage compilation (for example, merged geometry stages) receive stage binaries in the family's split form and a small Ember-side prologue or epilogue generated from the descriptors; the family's ABI annex defines them.

---

## 22. GPU family backends

### 22.1 Role

A GPU family backend compiles TVISA GPU code to one family's instruction set. Backends are Tessera-owned Rust crates (TIR.md section 35.1, layer 4). The same crate, at the same build, runs in the pipeline and in the compile service on devices. No driver contains a shader compiler; drivers execute the binaries backends produce (DeviceKit.md).

### 22.2 Structure

Every backend is built from shared stages and a family part:

| Stage | Work | Shared |
| --- | --- | --- |
| 1 Import | Decode and verify the entry and its callees; fold specialization constants | Yes |
| 2 Divergence | Uniformity analysis on TVISA, recomputed (it is not trusted from the image) | Yes |
| 3 Lowering | Address spaces; texture and sampler operations; resource handle fetches; ray queries and ray tracing (hardware or software traversal); cooperative matrices (hardware or emulation); subgroup and quad operations; mesh and object emulation where the family lacks them; work graphs | Yes, with family hooks |
| 4 Optimization | SSA optimization, scalarization of uniform values, memory access coalescing, loop optimization | Yes |
| 5 Selection | Family instructions | Family |
| 6 Scheduling and allocation | Latency scheduling, register allocation targeting the family's occupancy model, spilling to scratch | Family |
| 7 Encoding | Machine code, metadata, line tables | Family |

### 22.3 Determinism and parity

1. For the same TVISA GPU code, family, revision, specialization constants and backend build, output is bit-identical in the pipeline and on the device.
2. Device compilation uses the same optimization level as the pipeline. A binary finalized on a device is therefore the binary the pipeline would have built.

### 22.4 The `gpu` convention

A non-inlined GPU function call (a `noinline` function or a `gpu.call.vft` target) uses the family's call ABI, fixed in its ABI annex: argument and result registers, the scratch-memory stack, callee-saved registers, and the return mechanism. Visible function table entries are the family's callable-function addresses; Ember fills tables from compiled functions (Ember.md).

### 22.5 The entry convention

Where an entry receives its root pointer, immediate data, stage inputs and builtins is fixed per family in its ABI annex. On every family:

1. The root pointer is a 64-bit GPU address available to every invocation as a uniform value.
2. Immediate data (at most 256 bytes) is uniform and readable without a memory dependency on the root.
3. Heap bases for textures and samplers are available without loading the root.
4. Builtins come from hardware registers where the family provides them and are otherwise computed by the backend.

### 22.6 Family layer and descriptors

Each backend ships with a family layer used by Ember: descriptor encoders that write texture, sampler and tensor descriptors in the family's hardware format; acceleration-structure builders where the family has hardware traversal (otherwise Tessera's software format); shader binding table and visible function table encoders; and the stage-linking routines of section 21.9. The family layer and the backend are one versioned unit; the descriptor formats they agree on are internal to it.

The tensor descriptor format, shared by all families, is:

| Offset | Field |
| --- | --- |
| 0 | `u64` GPU address of element 0 |
| 8 | `u8` rank (1, 2, 3, 4, or 8), `u8` element type (a TVISA type tag), `u16` flags (bit 0 read only), `u32` reserved |
| 16 | 8 × `u64` extents |
| 80 | 8 × `i64` strides in elements |

### 22.7 GPU binaries (`.tirgpu`)

The native image's `.tirgpu` section holds every entry's binaries:

| Part | Content |
| --- | --- |
| Header | Magic `54 47 50 55` ("TGPU"), `u8` year, `u8` month, `u8` week, `u8` revision (the TVISA version), `u32` entry count, `u32` binary count |
| Entry table | Per entry: entry index, a copy of the entry record of section 21.2 |
| Binary table | Per binary: entry index, `u32` family identifier (section 31.1), `u32` revision (0 for family baseline), `u64` specialization constant hash (0 for defaults), offset and size of the code, offset and size of metadata, offset and size of line tables, BLAKE3-256 of the code |
| Metadata | Per binary: register counts per class, scratch bytes per invocation, threadgroup memory bytes, subgroup size, interface descriptors, resource-usage flags, the family-specific fields of the ABI annex |
| Code and line tables | Raw bytes, each aligned to 256 |

The section is covered by the image's build identifier and signature. Binaries finalized on the device are stored in the code cache, not in the image.

### 22.8 Revisions and robustness

1. A device reports its GPU's family and revision (DeviceKit.md). When the pipeline did not build the family or does not know the revision, the device compiles from TVISA.
2. For an unknown revision of a known family, the backend compiles for the newest revision it knows that is not newer than the device's, and otherwise for the family baseline. It never emits instructions the family baseline lacks unless the device reports them.
3. A backend MUST compile every valid TVISA GPU function whose capabilities the family advertises. If it fails internally, the compile service retries the same input with the backend's conservative mode (no optimization beyond what correctness requires); the conservative mode is required to succeed and is tested as such (section 34.1). Pipeline creation fails only if both fail, with `TVISA-GPU-0010`, which is a backend defect.
4. A GPU whose family has no backend is not supported; its driver does not register it with Ember.

### 22.9 Family notes

| Family identifiers | Instruction set | Notes |
| --- | --- | --- |
| `amd.gfx103`, `amd.gfx110`, `amd.gfx115`, `amd.gfx120` | AMD RDNA 2, 3, 3.5 and 4 | Wave32 by default and wave64 where the backend's cost model prefers it; scalar unit for uniform values; NGG for vertex, mesh and object; ray intersection instructions for traversal; WMMA for cooperative matrices from `gfx110`, FP8 forms from `gfx120`; software cooperative matrices on `gfx103` |
| `nvidia.sm75`, `sm80`, `sm86`, `sm89`, `sm90`, `sm100`, `sm120` | NVIDIA SASS of each generation | Convergence barriers for reconvergence; tensor-core MMA for cooperative matrices; RT cores where present, software traversal on revisions without them; mesh and object stages in hardware |
| `intel.xe_hpg`, `intel.xe_lpg`, `intel.xe2`, `intel.xe3` | Intel Xe GPU ISA | SIMD8/16/32 dispatch chosen per entry; XMX matrix engines where present; hardware ray tracing through the ray tracing unit |
| `qcom.a7xx`, `qcom.a8xx` | Qualcomm Adreno | Wave64/128 per entry; mesh and object emulated where absent; hardware ray queries on revisions with them |
| `arm.valhall_csf`, `arm.gen5` | Arm Mali Valhall with the command stream frontend, and the 5th Gen Arm GPU architecture | Warp size 16; ray tracing in hardware on revisions with it; cooperative matrices through the arithmetic units |
| `img.d`, `img.e` | Imagination PowerVR D-Series and E-Series | Tile-based; tile shading and framebuffer fetch native |
| `apple.g13`, `apple.g14`, `apple.g15`, `apple.g16` | Apple AGX | SIMD width 32; tile shading, imageblocks and raster order groups native; hardware ray tracing and mesh shading from `g15` |
| `tessera.tvgpu` | The TVISA GPU profile itself | Section 23 |

Each family has a normative ABI annex, `TVISA-GPU-ABI-<family>.md`, versioned with TVISA, fixing the entry and call conventions, register usage, scratch layout, descriptor formats, binary metadata fields and stage-split rules for that family.

---

## 23. The paravirtual GPU

### 23.1 Purpose

`tessera.tvgpu` is the GPU of Tessera development virtual machines (QEMU and the Tarwin hypervisor, Tarwin.md). It lets Ember, the GPU model and every application run in a virtual machine with the same semantics as on hardware, and it is the second conformance target after the reference executor.

### 23.2 Instruction set

The `tvgpu` family's instruction set is the TVISA GPU profile. Its backend performs stages 1 to 4 of section 22.2 (including specialization-constant folding and software lowering of features the host lacks) and emits a `tvgpu` binary: a canonical, self-contained TVISA GPU module holding the entry, its inlined callees, its constants and its record. The guest's driver passes `tvgpu` binaries to the host unchanged.

### 23.3 The host translator

The host side of the paravirtual device (in the virtual machine monitor) verifies each `tvgpu` binary with the TVISA verifier and translates it to the host's GPU interface:

| Host | Target |
| --- | --- |
| macOS | Metal Shading Language or Metal IR, with argument buffers tier 2 and GPU addresses |
| Linux, Windows | SPIR-V for Vulkan 1.3 with buffer device addresses, descriptor indexing, maximal reconvergence, and the ray tracing and mesh extensions when the host GPU has them |
| Any host, conformance mode | The GPU reference executor |

Requirements:

1. The translation preserves TVISA semantics exactly, within the floating-point bounds of TIR.md section 17.16. Where the host API cannot express a construct (for example 64-bit atomics on a host without them), the translator emulates it or the device does not advertise the capability.
2. The device advertises as capabilities the intersection of the host GPU's features and what the translator implements, plus everything the guest-side backend lowers in software.
3. Resource handles keep the format of section 21.5: the host maps heap indices to its own descriptors and guest GPU addresses to host buffer addresses.
4. Translated code is cached by the host keyed by the binary's hash and the host GPU.

### 23.4 Transport

The device's register interface, memory sharing, command submission and fences are DeviceKit.md's (paravirtual devices) and Ember.md's (command encoding). This section defines only the code the device executes.

---

## 24. Binary format (`.tvisa`)

### 24.1 Conventions

The conventions of TIR.md section 22.1 apply: fixed-width little-endian header and section table, minimal ULEB128 and SLEB128 in payloads, strings as a ULEB byte length followed by UTF-8, one canonical encoding per image.

### 24.2 Header (64 bytes)

| Offset | Field | Value |
| --- | --- | --- |
| 0 | Magic | `54 56 49 53` ("TVIS") |
| 4 | `u8` version year | 1–99 (e.g. 26) |
| 5 | `u8` version month | 1–12 |
| 6 | `u8` version week | 0 when absent, otherwise 1–5 |
| 7 | `u8` encoding revision | 0 |
| 8 | `u32` flags | Bit 0 has GPU code; 1 has assembly; 2 has variant sets; 3 has dispatch sets; 4 has unmanaged functions; 5 has sandboxed functions; 6 canonical (MUST be 1 in delivery units); 7 has `c` functions; others 0 |

Header summary bits 0–5 and 7 MUST exactly match the image contents: bit 0 is set when a defined function has target code 1 or 2; bit 1 when ASM has at least one record; bits 2 and 3 when VARIANTS has at least one kind-0 variant set or kind-1 dispatch set, respectively; bits 4 and 5 when a defined function has trust class 2 (unmanaged) or 1 (sandboxed), respectively; and bit 7 when an imported or defined function references a TYPES signature whose convention is `c` (code 1). Unreferenced `c` signatures do not set bit 7. Bit 6 is the canonical marker and is governed separately by the delivery-unit rule.
| 12 | `u8` profile | 0 `app`, 1 `system`, 2 `driver`, 3 `kernel` |
| 13 | `u8[3]` | Zero |
| 16 | `u32` section count | |
| 20 | `u32` function count | Defined functions |
| 24 | `u8[32]` content hash | BLAKE3-256 of every byte from offset 64 to the end of the file |
| 56 | `u64` file size | |

### 24.3 Section table

Entries of 32 bytes follow the header, sorted by kind, each kind at most once: `u32` kind, `u32` flags (bit 0 zstd-compressed; MUST be 0 in canonical images), `u64` offset, `u64` stored size, `u64` uncompressed size.

### 24.4 Sections

| Kind | Section | Content |
| --- | --- | --- |
| 1 | STRINGS | ULEB count, then strings. Index 0 is the empty string |
| 2 | IMAGE | Section 6.6 |
| 3 | IMPORTS | ULEB count; per import: `u8` kind (0 function, 1 data), `u8` module kind (0 RIDL module, 1 package framework, 2 platform library), ULEB module-path or library-name string index, 16-byte ABI UUID (nonzero for RIDL modules, zero for package frameworks and platform libraries), `u8` flags (bit 0 weak; only platform libraries may set it), ULEB symbol string index, then for functions ULEB signature index and `u32` type tag, or for data ULEB size and ULEB alignment |
| 4 | TYPES | Part A: ULEB value-type count, then tagged value types (section 7.2). Part B: ULEB signature count, then signatures. Part C: ULEB C aggregate count, then descriptors (section 9.7). The parts and records are encoded below |
| 5 | CONSTANTS | Concatenated type-directed records through EOF. Each record is a ULEB value-type index followed by the value payload defined below. The section is absent when there are no pooled constants |
| 6 | DATA | ULEB count; data objects and relocations encoded as specified in section 18.3 |
| 7 | SIGNATURES | Exactly `header.function_count` concatenated function records (section 8.1) |
| 8 | FUNCINDEX | Per defined function, 16 bytes: `u64` offset of its body in CODE, `u32` body size, `u32` zero |
| 9 | CODE | Function bodies (section 24.5) |
| 10 | VARIANTS | ULEB count; per record ULEB name string index, `u8` kind (0 variant set, 1 dispatch set), ULEB member count, then members as `u8` architecture (0 portable, 1 aarch64, 2 x64, 3 rv64, 4 wasm), ULEB feature count, feature string indexes and ULEB function index |
| 11 | GPU | Entry records (section 21.2) |
| 12 | UNWIND | ULEB record count; per function with landing pads or a terminate kind: ULEB global function index, `u8` terminate kind (0 none, 1 boundary, 2 nounwind), ULEB type-id count followed by that many `u64` typeids, ULEB filter count followed by that many global function indexes |
| 13 | METADATA | The section 10.6 subset of the TIR.md section 22.4 encoding |
| 14 | ASM | Templates and constraint strings |
| 15 | DEBUG | Section 20.1 |
| 16 | RIDLMETA | The RIDL metadata blob, verbatim |
| 17 | DEPS | Framework dependency records, verbatim |
| 18 | BRIDGE | Engine bridge and thunk tables, verbatim |
| 19 | RTMODS | Hashes of the runtime TIR modules of the delivery unit, verbatim |
| 20 | ANALYSIS | The analysis summary (section 6.2) |
| 21 | RUNTIME | Optional; when present, a nonzero ULEB count followed by that many `u16` runtime entry identifiers in strictly increasing order. The section is absent when no runtime entries are used |

There is no CUSTOM section; an unknown kind is rejected with `TVISA-ENC-0004`.

CONSTANTS records contain an integer, floating-point scalar, or fixed vector thereof. An integer payload is `ceil(bit_width / 8)` bytes containing the integer's low-order bits in little-endian order; a floating-point payload is the same number of bytes containing its raw format bits. Bits above a non-byte-aligned scalar width are zero. A fixed-vector payload contains one scalar payload per lane in lane order, without inter-lane padding. Narrow floats occur only as vector lanes. The ULEB type index determines the payload length, so record indexes are their zero-based order in the section.

TYPES part A records begin with a `u8` type tag and use that tag's payload in section 7.2 and TIR.md section 22.7, except TVISA-forbidden tags in section 7.2. Type indexes are zero-based and each value-type record may reference only earlier value-type indexes. The `vft` tag's payload is a signature index. Part B records are, in order: `u8` convention; ULEB flags (bit 0 `fails`, bit 1 `variadic`, bit 2 `noreturn`); when variadic, ULEB fixed-parameter count; ULEB parameter-item count and that many parameter items; ULEB result-item count and that many result items. Each item starts with its `u8` code from section 9.2. A leaf carries a ULEB type index; a group carries ULEB size, ULEB byte alignment, ULEB member count, then each member's ULEB byte offset and ULEB type index; a byref carries ULEB size, ULEB byte alignment and `u8` flags (bit 0 owned contents, bit 1 read only); an sret carries ULEB size and ULEB byte alignment; context has no payload; cagg carries a ULEB index into part C. Result items use only leaf and group. Part C records are ULEB size, ULEB byte alignment, `u8` flags (bits 0–4 from section 9.7), ULEB member count, then each member's ULEB byte offset, ULEB type index and ULEB bit-field width; width zero means the member is not a bit-field. Signature type indexes and C aggregate indexes are zero-based indexes into their respective parts.

ASM starts with a ULEB record count. Each record contains a ULEB STRINGS index for the assembly template and a ULEB STRINGS index for its constraint string. Records contain the distinct pairs referenced by CODE, ordered by first reference in function, block and instruction order. Both indexes are checked; the section ends after the final record.

ANALYSIS is required in every image. It starts with a ULEB defined-function count equal to `header.function_count`; each record in SIGNATURES order is a `u8` trust class (0 managed, 1 sandboxed, 2 unmanaged) and `u8` TIR reason mask (bits 0–7 from TIR.md section 21.4). A ULEB sandbox-region count follows; each record is a ULEB linear-memory index, ULEB DATA base-object index and ULEB DATA current-size-object index. There is exactly one record per linear memory; indices start at zero and are contiguous. Every base and current-size DATA object index is distinct across all records and refers to a defined, local, 8-byte writable or zero-initialized, non-per-CPU DATA object. A ULEB indirect-call type count follows; each record is a ULEB signature index and little-endian `u32` type tag. These distinct pairs are ordered by signature index then type tag and are exactly the CPU indirect-call pairs in CODE (`call.indirect`, `tail.call` with selector 1, and `invoke` with selector 1). Tag zero appears only for the foreign-call marker of section 11.6 and requires a `c` signature; every other pair carries a valid function type tag. A ULEB assembly-region count follows; each record is a ULEB global function index, ULEB zero-based instruction ordinal in flattened block order, ULEB ASM record index, `u8` architecture (1 aarch64, 2 x64, 3 rv64) and `u8` effects (bit 0 sideeffect, bit 1 memory, bit 2 pure, bit 3 privileged; other bits zero). Records are ordered by function index and instruction ordinal and list every `asm` and `asm.goto` instruction. `asm.goto` has effects zero. The section ends after the last record.

The ANALYSIS sandbox-region table has exactly one record for each linear memory; indices start at zero and are contiguous.

### 24.5 Function bodies

A body is: ULEB function-space index; ULEB block count; ULEB value count (the total number of values the body defines, for preallocation); then blocks. The value count includes the first block's parameters once, then later block parameters and all instruction results. The function-space index includes imports before defined functions, so a body for the n-th defined function has index `function_import_count + n`, matching its position in FUNCINDEX and SIGNATURES. A body has at least one block. A block is: ULEB parameter count and parameter type indices; `u8` block flags (bit 0 loop header, bit 1 cold); for loop headers, ULEB merge block and ULEB continue block; ULEB instruction count; instructions.

The `asm` opcode 0x220 carries a ULEB ASM record index, a counted list of input value deltas and a result type index. TVISA result types are non-unit and produce one SSA result. The `asm.goto` opcode 0x221 carries a ULEB ASM record index, a counted list of input value deltas, the normal successor's block index, a ULEB label count and that many label block indexes. These successors carry no block arguments, so each destination block has zero parameters; all are control-flow edges.

FUNCINDEX body ranges partition CODE exactly: they are nonempty, do not overlap, and leave no gaps or trailing bytes. In a canonical image, their offsets follow defined-function order.

An instruction is encoded as TIR.md section 22.5 defines, with these operand fields added:

| Field | Encoding |
| --- | --- |
| Slot | ULEB slot index |
| Signature | ULEB signature index |
| Type tag | `u32` little-endian; zero only for the foreign indirect-call marker of section 11.6, otherwise a valid function type tag |
| Runtime entry | ULEB identifier |
| Callee | Per the selector of section 11.6: a symbol; or a value, a signature and a tag; or a runtime identifier |
| Clause list | ULEB count; per clause: `u8` kind (0 `catch.type`, 1 `catch.filter`, 2 `catch.all`, 3 `cleanup`); kinds 0 and 1 carry a ULEB index into the current function's UNWIND type-id or filter table, respectively |

UNWIND records are unique and sorted by global function index (imports precede defined functions); every record names a defined function. A record's type-id and filter indexes are local to that function. The section is absent if it has no records. A function with a `landingpad` has a record even when its terminate kind and both tables are empty.

`global.addr @d [+ n]` stores its symbol followed by a sign-and-magnitude byte addend; an omitted addend is encoded as zero. This keeps the instruction length independent of the following opcode while preserving the textual default.

`kernel.percpu.addr`, `foreign.addr`, `foreign.data` and `foreign.entry` each store one tagged symbol index from section 24.4. `kernel.percpu.addr` uses a defined data index and requires its DATA record's per-CPU flag; it returns `ptr`. `foreign.addr` uses an imported platform-function index and returns `code`; `foreign.data` uses an imported platform-data index and returns `ptr`; `foreign.entry` uses a defined CPU-function index with the C calling convention and returns `code`. A weak platform import may resolve to null, which these `foreign.*` instructions preserve.

For `const.vec`, the type index determines the lane count and width. Vectors with storage size at most 16 bytes carry raw scalar lane payloads without inter-lane padding; larger vectors carry a CONSTANTS index. Every optional `align n` operand is encoded as a ULEB byte alignment, with zero for an omitted clause. Vector selectors, mask-bit operand placement, and implicit result types follow TIR.md section 22.5: selector 0 chooses the vector-of-pointers address form, selector 1 chooses base plus offsets and scale, and modifier bit 32 adds each instruction's optional mask operands. Atomic operation selectors and the `weak` modifier follow the same section. Raw and MMIO memory operands, alignment fields, prefetch hints, and I/O fence selectors also follow TIR.md section 22.5.

`target.intrinsic` stores its name as a STRINGS index, its counted value arguments, then its result type index. `target.has` stores its feature as a STRINGS index, and `target.pagesize` has no operands. Portable `intrinsic` stores its ID followed by a counted value list whose count matches section 12.18. `debug.value` and `debug.addr` store a value delta followed by a ULEB DEBUG variable index; `debug.label` stores a ULEB DEBUG label index. `assume` stores one value delta. `nop` has no operands and retains TIR opcode 0x245; the removed `expect` opcode 0x244 is not reassigned.

The 0x400–0x452 GPU builtins, control, derivative, quad and subgroup encodings follow TIR.md section 22.5. `gpu.call.vft` derives its signature index from the visible-function-table operand's type and stores that index on the wire before the counted argument list. Its verifier requires that the stored signature match the table type. The subgroup reduction operator and cluster size are explicit ULEB fields after the value operand. Object/mesh, tile imageblock and work-graph operands and results use the encodings in that section; `node.output.allocate` stores thread/group mode in selector bits.

An instruction with several results advances the value numbering by its result count.

### 24.6 Canonical encoding

`tirc lower` emits only canonical images:

1. Sections in kind order, uncompressed, contiguous, without padding.
2. STRINGS, value types, signatures, descriptors and constants in order of first reference during a traversal of the other sections in kind order.
3. Functions in SIGNATURES in the order of the TIR modules' declarations; blocks in reverse post-order. Within a block, visit distinct successors in TIR block order; start each disconnected component at its first block in TIR block order.
4. Minimal LEB encodings.

A delivered image whose re-encoding differs from its bytes is rejected with `TVISA-ENC-0002`. Compression happens only in delivery archives (TIR.md section 29.6).

### 24.7 Limits

| Item | Limit |
| --- | --- |
| Values per function | < 2^32 |
| Blocks per function | < 2^24 |
| Instructions per block | < 2^24 |
| Frame slots per function | < 2^20 |
| Fixed frame size | < 2^31 bytes |
| Parameter items per signature | < 2^16 |
| Functions per image | < 2^28 |
| Data objects per image | < 2^28 |
| Code and data of one image (finalized) | < 2^31 bytes |

---

## 25. Textual format (`.tvs`)

### 25.1 Correspondence

The textual format is a lossless rendering of the binary format: `tirc dis --tvisa` followed by `tirc asm --tvisa` yields identical bytes for a canonical image. The grammar is Appendix A.1. Comments run from `;` to the end of the line.

Quoted strings in `.tvs` use the ordinary Kira string escapes defined by TIR
section 23.1. A TVISA `const.bytes` operand uses TIR section 12.1's
byte-specific rules.

An import names its module kind explicitly: `ridl` is a RIDL module, `package`
is a package framework, and `platform` is a platform library. The following
string is the module path or library name and MUST be nonempty. `abi` is a
lowercase, hyphenated 36-character UUID string. RIDL imports require a nonzero
UUID; package and platform imports require the all-zero UUID. `weak`, written
after a platform library name, applies only to that import record; it is
forbidden on RIDL and package imports. The convention of a function import is
the convention of its referenced signature. A platform function import MUST
reference a `c` signature. Imported data alignment is a positive power of two.

Every `.tvs` image has exactly one `metadata image` record. Its fields are
comma-separated and appear in this canonical order:

```text
metadata image {
    package_digest: "<64 lowercase hexadecimal digits>",
    tirc_build: "<nonempty build identifier>",
    module_hashes: ["<64 lowercase hexadecimal digits>", ...],
    entry_function: none | "@<function symbol>",
    initializers: [["@<function symbol>", <u16 priority>], ...],
    finalizers: [["@<function symbol>", <u16 priority>], ...],
    gpu_capabilities: [<optional capability name>, ...],
    minimum_driver_api: none | "YY.M"
}
```

The module hash array is nonempty, unique and sorted by decoded hash bytes.
Capability names use TIR.md section 17.18's bit order. Function strings name
the exact `@`-prefixed TVISA symbols; array order is the corresponding IMAGE
record order after names are resolved to function indexes. The entry string is
present only for executable images. `minimum_driver_api` is present only for
driver images. The TIR version is the `.tvs` header version, which is also the
TVISA header version as section 33.2 requires. A disassembler emits all eight
fields, including `none` and empty-array values.

GPU entry records spell the fields of section 21.2 by their lower-snake-case names. A non-visible entry has every field listed below, including empty or zero-valued fields. The `visible` kind has only `typeid`; its function and kind are already in the record header. Record fields are one per line. Arrays use `[]`, objects use `{ name: value, ... }`, and nested object members are comma-separated. Types use the textual type syntax; type indices are assigned when the image is encoded. Integers and hexadecimal values use TIR's textual literal syntax.

Interpolation is a flag set: any listed qualifiers may be combined, and zero means no interpolation qualifier. The text array order does not affect its encoded mask. An image with no runtime calls omits `metadata runtime`. Otherwise it has exactly one such item, whose 16-bit identifiers are strictly increasing and equal the set used by `call.runtime`; it encodes as section RUNTIME.

An absent root and immediate encode as zero size and zero type identifier; their other encoded fields are zero. `root: none` and `immediate: none` are their textual spellings. `payload: none` encodes four zero sizes. `mesh: none` encodes zero limits and points topology; a mesh entry with a `mesh` object MUST have nonzero limits. `node: none`, `ray_tracing: none` and `fragment: none` mean that the corresponding stage-specific record is absent. A non-threaded stage writes `[0, 0, 0]` for `threads`; on a threaded stage that value means `threads(dynamic)`. A threaded stage with a fixed size writes three positive counts. A visible entry's `typeid` is a 64-bit hexadecimal integer.

A present root or immediate object has nonzero size; the `none` spelling is the only textual form for an absent one. A root's `align_log2` is at most 63. A payload object has at least one nonzero size; otherwise write `none`. A stage-input or stage-output array does not repeat a `(semantic, semantic_index, location)` tuple. The binary semantic index is `n` for `color(n)`, `color_index1(n)`, `clip_distance(n)` and `cull_distance(n)`, and zero for other semantics.

Every record field and every nested object has exactly the members shown below. Fields not applicable to the entry kind use their `none`, empty-array or zero form. `stage_input` and `stage_output` use the semantic names in section 21.3 (`user`, `position`, `color(n)`, `color_index1(n)`, `depth(any)`, `depth(greater)`, `depth(less)`, `stencil`, `sample_mask`, `clip_distance(n)`, `cull_distance(n)`, `point_size`, `layer`, `viewport`, `primitive_id`, `view_index`, `cull`, `shading_rate`). `color(n)` is semantic code 2; `color_index1(n)` is code 3, the `@color(n)` with `@index(1)` in TIR. Interpolation names are `flat`, `perspective`, `noperspective`, `centroid`, `sample`, `invariant`, and `per_primitive`. Capability names are exactly the optional capabilities listed in TIR.md section 17.18. A GPU entry `capabilities` array is a bit-set rendering: names are unique and appear in that section's listed order, matching section 21.2 bit order. The order of every other array is preserved.

| Field | Text value |
| --- | --- |
| `threads` | `[x, y, z]`; `[0, 0, 0]` means dynamic threads |
| `root` | `none`, or `{ size: n, align_log2: n, typeid: hex, space: device \| constant }` |
| `immediate` | `none`, or `{ size: n, typeid: hex }` |
| `stage_input`, `stage_output` | Arrays of `{ semantic: s, location: n, type: T, interpolation: [name, ...], offset: n }`; semantic names follow section 21.3 (`user`, `position`, `color(n)`, `depth(any)`, and so on) |
| `payload` | `none` when all sizes are zero, or `{ object: n, ray: n, hit_attribute: n, callable: n }` |
| `mesh` | `none`, or `{ max_vertices: n, max_primitives: n, topology: points \| lines \| triangles }` |
| `threadgroup_memory` | A byte count |
| `capabilities` | An array of unique optional capability names in TIR section 17.18 bit order |
| `subgroup_size` | `any`, or the requested invocation count (4 through 128, a power of two) |
| `floating_point` | An array containing `fast` and/or `ieee_denormals`; an empty array means precise IEEE defaults |
| `specialization_constants` | An array of `{ id: n, type: T, default: value }`; `id` is an unsigned 32-bit TIR integer literal |
| `node` | `none`, or `{ launch: mode, max_dispatch_grid: [x, y, z], input_record_size: n, outputs: [{ name: string, kind: node \| mesh, record_size: n, max_records: n }, ...] }` |
| `ray_tracing` | `none`, or `{ max_recursion_depth: n }` |
| `fragment` | `none`, or `{ early_fragment_tests: bool, writes_depth: none \| any \| greater \| less, discards_or_demotes: bool, reads_attachments: bool }` |
| `typeid` | The TIR function type identifier in a `visible` record |

Each node output has a nonempty unique name, a `kind` of `node` or `mesh`, a
positive record size and a positive maximum-record count. A mesh output targets
a mesh node and requires `work_graphs.mesh`; other outputs require only
`work_graphs`. The output order assigns the zero-based indexes consumed by
`node.output.allocate`.

### 25.2 Example (informative)

```
tvisa 26.11.1 profile app image "Demo" kind executable

metadata image {
    package_digest: "0000000000000000000000000000000000000000000000000000000000000000",
    tirc_build: "tirc-26.11.1-example",
    module_hashes: ["0000000000000000000000000000000000000000000000000000000000000000"],
    entry_function: "@Demo.sortBy",
    initializers: [],
    finalizers: [],
    gpu_capabilities: [],
    minimum_driver_api: none
}

sig $cmp = ridl (leaf i64, leaf i64) -> (leaf i32)
sig $sort = ridl (byref 16 8 [owned], leaf code, context) -> ()
sig $hash = ridl (leaf ptr) -> (leaf i64)

import function @Foundation.String.hash ridl "Foundation" abi "3d0f5c2a-91b4-4e7e-8c1d-6a2f9b0e4c71" sig $hash tag 0x5A31C07D

data @msg readonly align 1 bytes "overflow\00"

func image @Demo.sortBy sig $sort tag 0x3C9A41F2 attrs(address_taken) trust managed semantics defined
    slots { 0: size 64 align 16 [escapes, zeroinit, scoped] } {
bb0(%buf: ptr, %cmp: code, %ctx: ptr):
  %len = raw.load i64, %buf + 8 [align 8 unordered]
  %two = const.int i64, 2
  %small = icmp ult %len, %two
  cond.br %small, bb3(), bb1()
bb1():
  slot.start 0
  %tmp = slot.addr 0
  %a = raw.load i64, %tmp [align 8 unordered]
  %b = raw.load i64, %tmp + 8 [align 8 unordered]
  %r = call.indirect %cmp : $cmp, 0x7E21D0A4 (%a, %b)
  %zero = const.int i32, 0
  %lt = icmp slt %r, %zero
  slot.end 0
  cond.br %lt, bb2(), bb3()
bb2():
  %limit = const.int i64, 4096
  %big = icmp ugt %len, %limit
  trap.if %big, 0x0005                          ; TIR-RT-0005
  %flags = const.int i64, 0
  %p = call.runtime 0x0001 (%len, %two, %flags) ; heap.alloc
  br bb3()
bb3():
  return
}

variants @Crc.update {
    portable: @Crc.update.generic
    aarch64: @Crc.update.a64
    x64: @Crc.update.x64
    rv64: @Crc.update.rv64
}
```

The `%buf + 8` form is shorthand for a `ptr.offset` folded into the access; the binary format encodes the `ptr.offset` instruction.

---

## 26. Verification

### 26.1 Purpose and limits

The TVISA verifier establishes that an image is well formed and that finalizing it produces code satisfying section 17. It does not re-establish TIR's ownership and memory-safety guarantees: `tirc` proves those on TIR before lowering (TIR.md section 9.8), and the delivery unit's signature carries them to the device (TIR.md section 29.4). A device finalizes only TVISA images from delivery units whose signature verifies.

### 26.2 Complexity

Verification is linear in the image size, except for dominator computation, which uses an algorithm that is near-linear in practice (semi-NCA). Function bodies are verified independently and in parallel.

### 26.3 Rules

| Code | Rule |
| --- | --- |
| `TVISA-VFY-0001` | Structure: block count, block order is a reverse post-order, exactly one terminator per block at its end, block arguments match parameters in number and type |
| `TVISA-VFY-0002` | Every instruction belongs to TVISA (section 11) |
| `TVISA-VFY-0003` | Operand and result types match the instruction's definition |
| `TVISA-VFY-0004` | Every operand's definition dominates its use |
| `TVISA-VFY-0005` | Call arguments and results match the callee's signature leaf by leaf |
| `TVISA-VFY-0006` | Function imports resolve their signature; platform function imports use a `c` signature; parameter and result items obey section 9.2 |
| `TVISA-VFY-0007` | Slot operands name existing slots; `slot.start` and `slot.end` alternate on every path for `scoped` slots; no slot access outside a lifetime is statically visible in a defined function |
| `TVISA-VFY-0008` | Slot records obey section 8.4 |
| `TVISA-VFY-0009` | Data objects and relocations obey section 18.3 |
| `TVISA-VFY-0010` | Symbol operands name existing entities of the right space |
| `TVISA-VFY-0011` | `address_taken` functions have a tag; tags are not forbidden values (section 17.2); `func.ref` names an `address_taken` function; exported functions are `address_taken` |
| `TVISA-VFY-0012` | Variant sets: TIR.md section 20.2 rules 1 to 3 and the all-three rule of TIR.md section 20.3 |
| `TVISA-VFY-0014` | Profile rules: kernel functions without `kernel_fp` use no floating-point or vector types; no `call.runtime` of allocation entries in the kernel profile; GPU instructions only in GPU functions |
| `TVISA-VFY-0015` | Permissive-only constructs (`const.poison`, `assume`, `unreachable [ub]`, `[poison]` modes, `freeze`) only in permissive functions |
| `TVISA-VFY-0016` | `landingpad` is the first instruction of a block reached only by unwind edges; every unwind edge targets such a block; clause indices are valid |
| `TVISA-VFY-0017` | Trap codes are 16-bit; `unreachable` and `trap` are terminators |
| `TVISA-VFY-0021` | GPU restrictions: TIR.md section 17.15 rules 1 to 8 as they apply to TVISA |
| `TVISA-VFY-0022` | GPU entry records agree with their functions (kinds, stage fields, capabilities used) |
| `TVISA-VFY-0030` | `tail.call` to a `ridl` function: same results and failing flag |
| `TVISA-VFY-0031` | `tail.call` to a `c` function: section 9.9 rule 2 |
| `TVISA-VFY-0040` | `code.fromaddr` only in the system and kernel profiles |
| `TVISA-VFY-0041` | Development runtime entries (0xF000–0xFFFF) only in development images |
| `TVISA-VFY-0042` | Runtime entries exist in the image's TVISA version and are called with their signatures |
| `TVISA-VFY-0050` | The ANALYSIS section agrees with what the verifier recomputes: trust classes from instruction content, the assembly inventory, the runtime entry list |
| `TVISA-CF-000N` | GPU structured control flow rule SN (TIR.md section 11.3) |
| `TVISA-ARCH-0001` | `asm` or `asm.goto` outside a variant body of its architecture |
| `TVISA-ARCH-0002` | A target intrinsic of another architecture, or one whose features the variant does not require |
| `TVISA-ARCH-0003` | An inline-assembly constraint is invalid for its architecture, operand counts or output form |

### 26.4 Where verification runs

1. `tirc lower` verifies every image it emits; a failure is a `tirc` defect.
2. The pipeline verifies every TVISA image at publication.
3. The compile service verifies every image before finalizing it, every time, and the GPU path verifies the GPU functions it compiles.
4. The `tvgpu` host translator verifies each `tvgpu` binary.

---

## 27. The finalizer

### 27.1 Placement

The finalizer is one crate, `tir-finalize`, built from the same source into: the compile service on every device (TIR.md section 28.2), `tirc build` for development images at `-O0` and `-O1` and `tirc finalize` on developer hosts, and the pipeline's re-finalization for symbolication (section 20.3).

### 27.2 Inputs and outputs

| Input | Content |
| --- | --- |
| TVISA image | A verified, canonical image |
| CPU target | A target name and the exact feature set of section 30 |
| GPU family | Optional: family, revision, and the entries and specialization constants to compile |
| Options | Debug-information output on or off; conservative mode |

| Output | Content |
| --- | --- |
| Native image | Section 18.2, signed with the compile service's device key (TIR.md section 28.6) |
| GPU binaries | Stored in the code cache with their metadata (section 22.7 layout) |

### 27.3 Stages

| Stage | Work |
| --- | --- |
| 1 Decode and verify | Header, tables, then bodies in parallel (section 26) |
| 2 Select | Variant selection, dispatch sets, folding of `target.has`, `target.pagesize` and fixed `vscale` |
| 3 Legalize | Types and operations the target lacks: `i128`, `f128`, narrow floats, vector widths, masks |
| 4 Optimize | Target-dependent: e-graph rewriting with constant folding, global value numbering and loop-invariant code motion on the legalized form; addressing-mode folding; branch folding; nothing that crosses a function boundary |
| 5 Lower | Instruction selection; calling conventions (section 9); parameter groups; runtime calls; hardening sequences (section 17) |
| 6 Allocate | Register allocation with live-range splitting and rematerialization; frame layout (section 8.6) |
| 7 Emit | Machine code, block layout, branch relaxation, constant pools |
| 8 Link | Section layout, intra-image relocation, veneers, import and runtime slots, data relocations, TLS layout |
| 9 Tables | Unwind tables, LSDAs, trap-site table, function-start table, feature table |
| 10 Image | Image assembly, build identifier, signature |

Stages 1 to 7 run per function in parallel. Stages 8 to 10 are sequential and deterministic.

### 27.4 Resource use

1. Peak memory MUST NOT exceed 64 MiB plus 16 times the size of the largest function body being compiled per worker thread.
2. The compile service uses at most as many worker threads as the device policy grants (Tarwin.md), at background priority unless a launch or a pipeline creation waits.
3. Finalization is restartable: an interrupted finalization leaves nothing in the code cache.

### 27.5 Throughput and quality

1. On each train's reference devices (section 34.3), the finalizer sustains at least 2 MiB of TVISA per second per performance core at its default level.
2. Finalized code meets TIR.md section 34.4: geometric-mean runtime at most 1.20× the server backend's for the same target.

### 27.6 Failure

1. A finalizer internal error is retried once in conservative mode, which performs no optimization in stage 4 and uses a simple register allocator; conservative mode is required to succeed on every verified image.
2. If CPU finalization still fails, the device keeps using the pipeline's native image, which always exists for the baseline target (TIR.md section 30.4), and reports `TVISA-FIN-0001` with the image hash.
3. GPU failures follow section 22.8.

---

## 28. The server backend

### 28.1 Translation to LLVM

The server backend translates each TVISA function to LLVM IR for the Tessera LLVM fork and compiles it with the target's code generator.

| TVISA | LLVM IR |
| --- | --- |
| Value types | `iN`, `half`, `bfloat`, `float`, `double`, `fp128`, `ptr`, `<N x T>`, `<vscale x N x T>` (with `vscale` set per section 15.2 for fixed targets), `ptr` for `code` |
| Narrow floats | Integer storage with explicit conversion sequences |
| `ridl` | The fork's `ridlcc`, `swifttailcc`-based: callee-pop, `swiftself` for the context, `swifterror` for the error; parameter groups resolved by the translator per section 9.3 |
| `c` | The target's C calling convention, with `cagg` items classified by the fork's port of Clang's ABI lowering for AAPCS64, System V AMD64 and RISC-V LP64D |
| `rtcc` | The fork's `tirrtcc`, a `preserve_all`-class convention with the register sets of section 9.6 |
| Accesses with `unordered` | `load atomic unordered` and `store atomic unordered` for naturally aligned accesses of at most 8 bytes; larger or misaligned accesses become plain accesses tagged `!tir.unordered`, which the fork's passes never duplicate, introduce or widen |
| Alias scopes | `!alias.scope` and `!noalias` |
| Branch weights, loop hints, range, nonnull, align | The matching LLVM metadata (in permissive functions only for range, nonnull and align) |
| Hardening | The fork's passes emit the sequences of section 17 exactly |

### 28.2 Exactness

The translation gives every TVISA instruction exactly its TVISA semantics. Where an LLVM instruction has undefined behavior or poison on inputs that TVISA defines (division by zero, shift amounts at least the width, `fptosi` out of range, signed overflow in non-poison modes), the translator emits the explicit check, mask or saturating intrinsic that TVISA's semantics require. Defined functions produce LLVM IR without `nsw`, `nuw`, `exact`, `inbounds`, `noundef`, `nonnull` or `dereferenceable` unless TVISA proves the fact by the instruction's own semantics.

### 28.3 Optimization

At the pipeline's release level the server backend runs LLVM's target-dependent IR pipeline (loop strength reduction, loop and SLP vectorization with the target's cost model, interleaved-access lowering, CodeGenPrepare) and the full `-O3` code generator. It MAY run any other LLVM transformation that preserves the semantics of the translation. It never inlines across images.

### 28.4 Kernel and driver builds

Kernel code is built only by the server backend, with Tarwin.md's code model. Functions with `no_fp_regs` are compiled with `noimplicitfloat` and with floating-point and vector target features disabled. Driver images are built like application images.

### 28.5 Output

The server backend writes native images with the same image writer as the finalizer (stages 8 to 10 of section 27.3), so section layout, slots, tables and relocations are identical in form. It emits DWARF 5 into the symbol bundle.

---

## 29. Determinism, caching and parity

### 29.1 Determinism

1. Finalizer output is bit-identical for identical (TVISA image bytes, CPU target and feature set, GPU family and revision, specialization constants, finalizer build), on any host, with any number of threads, with or without debug-information output.
2. Server backend output is bit-identical for identical (TVISA image bytes, CPU target, options, `tirc` build).
3. No output embeds a time, a path outside the package, a host name or a random value. Values that must be random at run time (stack protector, memory tags) are drawn at run time.

### 29.2 Caching

The pipeline caches TVISA images and native images under TIR.md section 30.5's keys extended with the TVISA image hash. The device code cache uses TIR.md section 28.8's keys; the input hash there is the TVISA image hash.

### 29.3 Parity

1. TIR.md section 34.3 applies: results are bitwise identical across the interpreter, the JIT tiers, the server backend and the finalizer, on all three architectures, except where fast-math flags, `[approx]`, GPU `fmuladd` or `fp(fast)` permit differences within bounds.
2. On targets without native `f16` or `bf16` arithmetic, `fadd`, `fsub`, `fmul`, `fdiv` and `fsqrt` are computed in `f32` and rounded once to the narrow type, which is exact for these operations; `fma` and `fmuladd` use the exact sequences of `TVISA-RUNTIME-INLINE.md`.
3. Variant set members are compared by the conformance suite (TIR.md section 34.1, variant equivalence).
4. The interpreter and every native tier use the atomics mapping of section 13.3 and the mathematics entries of section 12.4.

---

## 30. CPU targets and features

### 30.1 Feature names

Feature names are used in variant attributes (TIR.md section 20.2 rule 3), in `target.has`, in target intrinsic requirements and in target definitions. Each names one architectural feature.

| Architecture | Features |
| --- | --- |
| aarch64 | `crc`, `lse`, `lse2`, `lse128`, `rdm`, `rcpc`, `rcpc2`, `rcpc3`, `jscvt`, `fcma`, `fp16`, `fhm`, `dotprod`, `i8mm`, `bf16`, `frintts`, `flagm`, `flagm2`, `sb`, `ssbs`, `bti`, `pauth`, `fpac`, `gcs`, `mte` (FEAT_MTE2), `mte3`, `rng`, `aes`, `pmull`, `sha1`, `sha2`, `sha3`, `sha512`, `mops`, `hbc`, `cssc`, `wfxt`, `ecv`, `sve2`, `sve2p1`, `sve_aes`, `sve_bitperm`, `sve_sha3`, `sme`, `sme2`, `sme2p1`, `sme_f64f64`, `sme_i16i64`, `fp8`, `fp8dot2`, `fp8dot4`, `fp8fma` |
| x64 | `cx16`, `lahf_sahf`, `sse3`, `ssse3`, `sse4_1`, `sse4_2`, `popcnt`, `lzcnt`, `movbe`, `bmi1`, `bmi2`, `fma`, `f16c`, `avx`, `avx2`, `xsave`, `shstk`, `ibt`, `aes`, `pclmul`, `vaes`, `vpclmulqdq`, `gfni`, `sha`, `sha512`, `rdrand`, `rdseed`, `erms`, `fsrm`, `avx_vnni`, `avxifma`, `avx_ne_convert`, `avx_vnni_int8`, `avx512f`, `avx512bw`, `avx512cd`, `avx512dq`, `avx512vl`, `avx512ifma`, `avx512vbmi`, `avx512vbmi2`, `avx512vnni`, `avx512bitalg`, `avx512vpopcntdq`, `avx512bf16`, `avx512fp16`, `avx10_1`, `avx10_2`, `amx_tile`, `amx_int8`, `amx_bf16`, `amx_fp16`, `amx_complex`, `apx_f`, `movdiri`, `movdir64b`, `serialize`, `atomic16_vector` (vendor-guaranteed atomic aligned 16-byte vector loads and stores) |
| rv64 | `v`, `zba`, `zbb`, `zbs`, `zbc`, `zbkb`, `zicond`, `zfa`, `zfh`, `zfhmin`, `zvfh`, `zvfhmin`, `zfbfmin`, `zvfbfmin`, `zvfbfwma`, `zvbb`, `zvbc`, `zvkg`, `zvkned`, `zvknha`, `zvknhb`, `zvksed`, `zvksh`, `zvkt`, `zacas`, `zabha`, `zawrs`, `zicboz`, `zicbom`, `zicbop`, `zihintntl`, `zihintpause`, `zimop`, `zcmop`, `zcb`, `zicfiss`, `zicfilp`, `zkr`, `supm` |
| wasm | `mutable-globals`, `sign-ext`, `nontrapping-fptoint`, `bulk-memory`, `multivalue`, `reference-types`, `simd128`, `relaxed-simd`, `tail-call`, `exception-handling`, `extended-const`, `memory64`, `atomics` |

A TVISA version adds features; it never removes or renames one.

### 30.2 Targets

A CPU target is a named feature set. The pipeline builds every target of section 30.3's matrix; the device chooses the best target its CPU satisfies.

| Target | Features | `svec` mode |
| --- | --- | --- |
| `aarch64.base` | Armv8.5-A and every feature it makes mandatory (`crc`, `lse`, `lse2`, `rdm`, `rcpc`, `rcpc2`, `jscvt`, `fcma`, `dotprod`, `frintts`, `flagm`, `flagm2`, `sb`, `ssbs`, `bti`, `pauth`), plus `fp16` | `fixed(1)` |
| `aarch64.v86` | `aarch64.base` plus `bf16`, `i8mm`, `fhm`, `aes`, `pmull`, `sha1`, `sha2`, `sha3`, `sha512` | `fixed(1)` |
| `aarch64.v9` | `aarch64.v86` plus `sve2` | `agnostic` |
| `x64.v3` | x86-64-v3 (`avx`, `avx2`, `bmi1`, `bmi2`, `fma`, `f16c`, `lzcnt`, `movbe`, `popcnt`, `sse4_2`, `xsave`) plus `shstk`, `ibt`, `aes`, `pclmul`, `vaes`, `vpclmulqdq`, `rdrand`, `rdseed`, `erms` | `fixed(2)` |
| `x64.v4` | `x64.v3` plus `avx512f`, `avx512bw`, `avx512cd`, `avx512dq`, `avx512vl` | `fixed(4)` |
| `x64.avx10_2` | `x64.v4` plus `avx10_1`, `avx10_2` and every AVX-512 subset AVX10.2 includes | `fixed(4)` |
| `x64.apx` | `x64.avx10_2` plus `apx_f` | `fixed(4)` |
| `rv64.base` | RVA23U64 as ratified (including `v`, `zba`, `zbb`, `zbs`, `zicond`, `zfa`, `zfhmin`, `zvfhmin`, `zvbb`, `zvkt`, `zawrs`, `zicboz`, `zicbom`, `zicbop`, `zihintntl`, `zihintpause`, `zimop`, `zcmop`, `zcb`, `supm`) plus `zicfiss`, `zicfilp` | `agnostic` |
| `rv64.zvk` | `rv64.base` plus `zbc`, `zvbc`, `zvkg`, `zvkned`, `zvknhb`, `zacas`, `zfh`, `zvfh` | `agnostic` |

Modifiers append features to any target of their architecture:

| Modifier | Adds | Effect |
| --- | --- | --- |
| `+mte` | `mte`, `mte3` | Stack tagging (section 17.8) |
| `+fpac` | `fpac` | Omits the authentication checks of section 17.4 |
| `+gcs` | `gcs` | Records that the Guarded Control Stack is in use |
| `+sme2` | `sme`, `sme2` | Target intrinsics for SME2 in variant bodies |
| `+amx` | `amx_tile`, `amx_int8`, `amx_bf16` | Target intrinsics for AMX in variant bodies |

A tuned target adds a tuning model to a target, written `target:model` (for example `aarch64.v9+mte+fpac:cortex-x4`, `x64.avx10_2:zen5`). A tuning model changes scheduling, instruction choice among equivalent sequences, and loop and alignment heuristics, never the feature set. The tuning models of a TVISA version are listed in `TVISA-TUNING.md`.

### 30.3 Target matrix

1. Baselines `aarch64.base`, `x64.v3` and `rv64.base` are always built.
2. At publication the pipeline also builds `aarch64.v9`, `aarch64.v86`, `x64.v4` and `x64.avx10_2`, each with the modifiers that change code (`+mte`, `+fpac`), for app and system packages.
3. Tuned targets are built on demand (TIR.md section 30.3).
4. System frameworks are built and installed on each device for every target the device's executables can use, so the rule of section 15.2 always finds a matching framework image.
5. Platform targets build the baselines and optional targets that TIR.md section 32.5 lists for each platform (section 32.2).

### 30.4 Device matching

A device's CPU target is the most specific target whose features its CPU has, preferring in order: more features, then a tuning model matching the CPU, then declaration order in this section. The loader refuses an image whose target features the CPU lacks with `TVISA-RT-0004`. On heterogeneous CPUs (big and little cores), only features present on every core count; Tarwin.md guarantees that a thread never migrates to a core lacking a feature the process's images use.

---

## 31. GPU families

### 31.1 Family identifiers

| Identifier | Code | Family | Representative hardware |
| --- | --- | --- | --- |
| `tessera.tvgpu` | 0x0001 | Tessera paravirtual GPU | Development virtual machines |
| `amd.gfx103` | 0x0100 | AMD RDNA 2 | Radeon RX 6000, Radeon 680M |
| `amd.gfx110` | 0x0101 | AMD RDNA 3 | Radeon RX 7000, Radeon 780M |
| `amd.gfx115` | 0x0102 | AMD RDNA 3.5 | Radeon 890M, Radeon 8060S |
| `amd.gfx120` | 0x0103 | AMD RDNA 4 | Radeon RX 9000 |
| `nvidia.sm75` | 0x0200 | NVIDIA Turing | GeForce RTX 20 and GTX 16 |
| `nvidia.sm80` | 0x0201 | NVIDIA Ampere (data center) | A100 |
| `nvidia.sm86` | 0x0202 | NVIDIA Ampere | GeForce RTX 30 |
| `nvidia.sm89` | 0x0203 | NVIDIA Ada Lovelace | GeForce RTX 40 |
| `nvidia.sm90` | 0x0204 | NVIDIA Hopper | H100 |
| `nvidia.sm100` | 0x0205 | NVIDIA Blackwell (data center) | B200 |
| `nvidia.sm120` | 0x0206 | NVIDIA Blackwell | GeForce RTX 50 |
| `intel.xe_hpg` | 0x0300 | Intel Xe-HPG | Arc A-series |
| `intel.xe_lpg` | 0x0301 | Intel Xe-LPG | Core Ultra Series 1 graphics |
| `intel.xe2` | 0x0302 | Intel Xe2 | Arc B-series, Core Ultra 200V graphics |
| `intel.xe3` | 0x0303 | Intel Xe3 | Core Ultra Series 3 graphics |
| `qcom.a7xx` | 0x0400 | Qualcomm Adreno 7xx | Adreno 730, 740, 750, X1 |
| `qcom.a8xx` | 0x0401 | Qualcomm Adreno 8xx | Adreno 830 and later |
| `arm.valhall_csf` | 0x0500 | Arm Mali Valhall with the command stream frontend | Mali-G710, G610, G715, G615, Immortalis-G715 |
| `arm.gen5` | 0x0501 | 5th Gen Arm GPU architecture | Mali-G720, G620, Immortalis-G720, G925 and later |
| `img.d` | 0x0600 | Imagination PowerVR D-Series | DXT and later D-Series cores |
| `img.e` | 0x0601 | Imagination PowerVR E-Series | E-Series |
| `apple.g13` | 0x0700 | Apple AGX G13 | M1 family |
| `apple.g14` | 0x0701 | Apple AGX G14 | M2 family |
| `apple.g15` | 0x0702 | Apple AGX G15 | M3 family |
| `apple.g16` | 0x0703 | Apple AGX G16 | M4 family |

Families whose devices Tarwin does not yet support are specified so that support can be added without a new TVISA version. A TVISA version adds families; it never renumbers one.

### 31.2 Revisions

A revision is a `u32` that identifies a GPU within its family closely enough to choose instructions and scheduling (for example a GFX IP version, an SM minor version, an Adreno or Mali product identifier). Each family's ABI annex defines its revision encoding, lists the revisions the backend knows, and gives each revision's capability set. Revision 0 is the family baseline: the instruction set every revision of the family has.

### 31.3 Reporting

A GPU driver reports its GPU's family code and revision to DeviceKit when it registers the GPU (DeviceKit.md). The device's delivery requests carry them (TIR.md section 30.4), and the code cache keys on them (TIR.md section 28.8).

### 31.4 Capabilities

Every family provides TIR's baseline GPU capabilities (TIR.md section 17.18) in hardware or through its backend: for example, cooperative matrices are emulated on families without matrix units, ray tracing uses software traversal on revisions without ray tracing hardware, and object and mesh stages are emulated with compute on families without them. Optional capabilities are reported per revision by the ABI annex and by the driver; Ember advertises the intersection.

### 31.5 Hosted translation targets

The GPU section of a platform image (TIR.md section 32.11) holds, in place of family binaries, code for the platform's Ember hosted backends (Ember.md section 30), identified by these codes in the GPU binary container (section 22.7):

| Identifier | Code | Content | Backend |
| --- | --- | --- | --- |
| `host.msl` | 0x0800 | Metal Shading Language 4.0 source, and a Metal library when the build host has Apple's Metal toolchain | Metal |
| `host.dxil` | 0x0801 | DXIL for Shader Model 6.8, validated and signed by the DXIL validator | Direct3D 12 |
| `host.spirv` | 0x0802 | SPIR-V 1.6 for the Vulkan environment of Ember.md section 30.1 | Vulkan |
| `host.wgsl` | 0x0803 | WGSL | WebGPU |

The translators share the host translator's front end (section 23.3); their rules are Ember.md section 30.3. Each entry's record lists the translation targets that hold it, and an entry beyond a backend's capability floor is absent for that backend.

---

## 32. Platform targets

### 32.1 Scope and backends

1. For the platform targets of TIR.md section 32, `tirc` lowers TIR to a TVISA image as for Tessera and compiles it into the platform's image format: with the server backend for release builds (`-O2`, `-O3`) and on every platform, and with the finalizer for development builds (`-O0`, `-O1`) on every platform except `web` and `wasi`, whose code only the server backend generates.
2. TVISA images are never delivered to platforms. On platforms TVISA is an internal stage of `tirc`, and the device-side components (the compile service, finalized-image signing, the code cache) do not exist.
3. Everything in this document applies to platform targets except what this section replaces: the `c` convention (section 9.7, replaced by section 32.4), the hardening sequences for code pointers, return addresses and stack tagging (sections 17.3, 17.4 and 17.8, replaced by section 32.5), unwind tables (section 14.6, replaced by section 32.7), the code model, image layout, relocations and imports (section 18, replaced by sections 32.9 and 32.10), and the thread control block and thread-local storage (section 19, replaced by section 32.6). WebAssembly code generation is section 32.11.

### 32.2 Platform CPU targets

| Target | Features | `svec` mode |
| --- | --- | --- |
| `aarch64.v80` | Armv8.0-A (floating point and Advanced SIMD) plus `crc` | `fixed(1)` |
| `aarch64.v82` | Armv8.2-A and its mandatory features (`crc`, `lse`, `rdm`) plus `rcpc`, `dotprod`, `fp16`, `aes`, `pmull`, `sha1`, `sha2` | `fixed(1)` |
| `aarch64.apple1` | Armv8.4-A and its mandatory features (`crc`, `lse`, `lse2`, `rdm`, `rcpc`, `rcpc2`, `jscvt`, `fcma`, `dotprod`, `flagm`) plus `fp16`, `fhm`, `aes`, `pmull`, `sha1`, `sha2`, `sha3`, `sha512`, `frintts`, `flagm2`, `sb`, `ssbs`: the Apple M1 and A14 generation | `fixed(1)` |
| `x64.v2` | x86-64-v2: `cx16`, `lahf_sahf`, `popcnt`, `sse3`, `sse4_1`, `sse4_2`, `ssse3` | `fixed(1)` |
| `x64.v3h` | x86-64-v3 (`avx`, `avx2`, `bmi1`, `bmi2`, `fma`, `f16c`, `lzcnt`, `movbe`, `popcnt`, `sse4_2`, `xsave` and `x64.v2`) plus `aes`, `pclmul` | `fixed(2)` |
| `x64.v4h` | `x64.v3h` plus `avx512f`, `avx512bw`, `avx512cd`, `avx512dq`, `avx512vl` | `fixed(4)` |
| `rv64.rva23` | RVA23U64 as ratified: the features of `rv64.base` without `zicfiss` and `zicfilp` | `agnostic` |
| `wasm.base` | `mutable-globals`, `sign-ext`, `nontrapping-fptoint`, `bulk-memory`, `multivalue`, `reference-types`, `simd128`, `tail-call`, `exception-handling` (with `exnref`), `extended-const`; one 32-bit memory | `fixed(1)` |
| `wasm.m64` | `wasm.base` with `memory64`: one 64-bit memory | `fixed(1)` |

1. Modifiers for `wasm` targets: `+threads` adds `atomics` and a shared memory; `+relaxed-simd` adds `relaxed-simd`, used only for instructions whose TIR semantics allow implementation-defined results (fast-math flags and `[approx]`).
2. The Tessera targets of section 30.2 may serve as optional targets on platforms of the same architecture, as TIR.md section 32.5 lists, because platform hardening uses no instruction outside the baseline (section 32.5).
3. TIR.md section 32.5 fixes which baseline and optional targets each platform builds.
4. At start, `libtirrt` reads the CPU's features (`getauxval` with `AT_HWCAP` and `AT_HWCAP2` on `linux` and `android`, `sysctlbyname` with the `hw.optional.arm` keys on Apple platforms, `IsProcessorFeaturePresent` and `cpuid` on `windows`, `cpuid` on x64 everywhere), refuses to run an image whose baseline the CPU lacks (`TVISA-RT-0004`, reported as TIR.md section 32.8 describes), and fills the dispatch slots of section 32.10 with the most specific clone or variant whose features the CPU has, by the rule of section 30.4.

### 32.3 Image contents per platform

A platform image holds the code of the baseline target, the clones of each optional target (TIR.md section 32.5 item 2) in a separate `.text.<target>` section, the dispatch slots that select among them, and the platform's GPU code (section 31.5). The `.tirfeat` table and the function-start table of Appendix A.2 are kept; the finalized-image header record is not.

### 32.4 Calling conventions on platforms

1. `ridl` keeps section 9.3's register assignment, callee-popped stack arguments and tail-call guarantee on every platform of an architecture, with these differences: on `windows` (x64 and aarch64), no function uses memory below its stack pointer (there is no red zone); on `windows` x64, a `c` function that calls a `ridl` function treats rdi, rsi and xmm6 to xmm15 as clobbered, and saves and restores them as the Microsoft convention's nonvolatile registers. `ridl` on `wasm` is section 32.11.
2. The `c` convention is the platform's:

| Platform | Architecture | `c` |
| --- | --- | --- |
| `macos`, `ios` | aarch64 | Apple's arm64 ABI: AAPCS64, except that variadic arguments go on the stack in 8-byte slots (16 bytes for 16-byte-aligned types), arguments narrower than 32 bits are extended to 32 bits by the caller according to their type's signedness, stack arguments are packed at their natural alignment rather than in 8-byte slots, x18 is reserved, and `long double` is `double` |
| `windows` | x64 | The Microsoft x64 convention: the first four arguments in rcx, rdx, r8 and r9 or xmm0 to xmm3 by position; a 32-byte home area allocated by the caller; aggregates of 1, 2, 4 or 8 bytes in a register, others by reference to a caller-made copy; results of at most 8 bytes in rax or xmm0, others through a hidden pointer; nonvolatile rbx, rbp, rdi, rsi, r12 to r15 and xmm6 to xmm15; variadic floating-point arguments also copied to the matching integer register |
| `windows` | aarch64 | The Microsoft ARM64 convention: AAPCS64 with x18 reserved for the thread environment block; variadic functions receive every argument in x0 to x7 and then on the stack, never in vector registers, and homogeneous floating-point aggregates lose their special treatment in variadic calls |
| `linux`, `android` | aarch64, x64, rv64 | AAPCS64, the System V AMD64 psABI and the RISC-V LP64D psABI, as section 9.7 |
| `web`, `wasi` | wasm | The WebAssembly C ABI as Clang implements it for `wasm32` (under `wasm.base`) and `wasm64` (under `wasm.m64`): scalars as Wasm values, aggregates other than single-scalar aggregates by reference to a caller-made copy, aggregate results through a hidden pointer. Under `wasm.base`, pointers are passed as `i32` after the range check of section 32.11 |

3. `cagg` descriptors (section 9.7) carry everything the Microsoft x64 classification reads (size), the Microsoft ARM64 and Apple classifications (those of AAPCS64), and the WebAssembly classification (member count and kind).
4. `call.variadic` in platform mode follows the table's variadic rules.

### 32.5 Hardening sequences on platforms

1. Forward edge. The type tag precedes every address-taken entry and the tag comparison of section 17.2 precedes every nonforeign TIR indirect call on every platform. Landing pads and the call instruction are:

| Architecture | Landing pad | Internal call after the tag check |
| --- | --- | --- |
| aarch64 | `bti c` (`hint #34`) | `blr x16` (no `blraa`, because code pointers are not signed on platforms) |
| x64 | `endbr64` | `call r11` |
| rv64 | `lpad label` as section 17.2 for TIR entries; foreign entries are thunks beginning with `lpad 0` | `jalr ra, 0(t1)` with t2 holding the label, as section 17.2 |
| wasm | None: a function value is a table index | `call_indirect` with the function's Wasm type, after comparing the tag stored for that table index in the `__tir_tags` array |

2. Calls through `[foreign]` values (TIR.md section 32.6) are represented in TVISA by an indirect call with tag zero (section 11.6) and skip the tag comparison. On `windows` x64 they load the target into rax and execute `call qword ptr [rip + __guard_dispatch_icall_fptr]`; on `windows` aarch64 they load the target into x15 and call through `__guard_check_icall_fptr` before `blr x15`. Such calls are never tail calls on `windows`.
3. Backward edge on aarch64: functions that save the link register begin with `paciasp` (`hint #25`) and authenticate with `autiasp` (`hint #29`) before `ret`, on every platform. No other pointer-authentication instruction is emitted, and the Guarded Control Stack is not used.
4. Backward edge on x64: code is shadow-stack compatible: no return address is modified, and stack switching for coroutines never occurs because TIR's coroutines are lowered to frames in memory. Images are marked `IMAGE_DLLCHARACTERISTICS_EX_CET_COMPAT` on `windows` and with `GNU_PROPERTY_X86_FEATURE_1_IBT` and `GNU_PROPERTY_X86_FEATURE_1_SHSTK` on `linux` and `android`.
5. Backward edge on rv64: `sspush ra` after the prologue's return-address save and `sspopchk ra` before return, in their Zimop encodings, which execute as no-ops unless the kernel enables shadow stacks. Images carry the RISC-V feature property with the unlabeled-landing-pad and shadow-stack bits.
6. aarch64 images for `linux` and `android` carry `GNU_PROPERTY_AARCH64_FEATURE_1_BTI` and `GNU_PROPERTY_AARCH64_FEATURE_1_PAC`.
7. Code pointers in memory (section 17.4) are plain addresses on aarch64 platforms and table indices widened to 64 bits on `wasm`. CODE64 relocations produce plain values.
8. Stack protector (section 17.6): the guard value is the global `__tir_stack_guard` in `libtirrt`, set from the platform's random source before any other initializer and read-only afterwards. It is loaded PC-relative when `libtirrt` is linked into the image, and through the image's import slot otherwise.
9. Stack probes (section 17.7) are unchanged; they touch every page in order, which also satisfies Windows' requirement that stack pages be committed in order. Stack memory tagging (section 17.8) is not used. Speculative hardening (section 17.9) is unchanged.
10. `windows` images are linked with Control Flow Guard (`IMAGE_DLLCHARACTERISTICS_GUARD_CF` and a guard function table listing every address-taken function and every foreign entry), `DYNAMICBASE`, `HIGHENTROPYVA` and `NXCOMPAT`.

### 32.6 Thread-local storage and the thread control block

1. On platforms the thread control block of section 19.1 is the 64-byte thread-local object `__tir_tcb` defined by `libtirrt`. Its stack-limit, `libtirrt`-state and shadow-stack-base fields keep their offsets; the stack-protector field is unused (section 32.5 item 8) and the LibTessera field is zero. `thread.pointer`, used only by `libtirrt`, yields the object's address.
2. Thread-local storage uses the platform's model:

| Platform | Model |
| --- | --- |
| `macos`, `ios` | Thread-local variable descriptors (`__DATA,__thread_vars`); an access calls the descriptor's thunk once per function and reuses the address |
| `windows` | Implicit TLS: the image's slot index in `_tls_index`, the thread's TLS array at `gs:[0x58]` on x64 and `[x18, #0x58]` on aarch64 |
| `linux` | Local-exec in executables; TLS descriptors in shared images (aarch64, x64, rv64) |
| `android` | As `linux`; the TLS segment of an executable is aligned to at least 64 bytes so that Bionic's reserved thread slots are skipped |
| `web`, `wasi` | The global `__tls_base`, set by each thread's start function to a block allocated from the heap and initialized from the image's TLS template |

3. Section 19.2's static TLS surplus is Tessera's; platform loaders allocate TLS for images loaded after start.

### 32.7 Unwind tables and exceptions

| Platform | Tables | Personality and language data |
| --- | --- | --- |
| `macos`, `ios` | `__TEXT,__unwind_info` with compact encodings where they describe the frame, `__TEXT,__eh_frame` otherwise | `tir_personality` in the compact entry or the CIE; the LSDA (section 14.4) in `__TEXT,__tirlsda` |
| `windows` | `.pdata` and `.xdata`: version 1 unwind information on x64, packed or full unwind data on aarch64 | The language-specific handler `tir_personality_seh`, with the LSDA as handler data; two-phase unwinding through `RtlUnwindEx` |
| `linux`, `android` | `.eh_frame` and `.eh_frame_hdr` | `tir_personality`; the LSDA in `.tirlsda` |
| `web`, `wasi` | None: WebAssembly exception handling | Exceptions are thrown with the tag `__tir_exception`, whose payload is the 64-bit address of the exception record; landing pads are `try_table` blocks with `catch_ref` and `catch_all_ref`; resumption is `throw_ref` |

1. TIR exceptions carry the exception class `TESSTIR\0` on unwinders that use exception classes. A foreign exception class reaching a frame whose landing pad has no foreign personality terminates the process with `TIR-RT-0020` (TIR.md section 32.6 item 9).
2. `setjmp` and `longjmp` on `wasm` are lowered to WebAssembly exception handling with the tag `__tir_longjmp`.

### 32.8 Traps

1. Trap encodings and the `.tirtrap` table of section 17.10 are unchanged on aarch64, x64 and rv64, except on `windows` aarch64, where `brk` immediates from 0xF000 to 0xFFFF are reserved by the system: codes in that range are emitted as `movz w17, #code` followed by `brk #0xEFFF`, and the handler reads the code from w17.
2. On `wasm`, a trap is `i32.const code`, `global.set $__tir_trap_code`, `unreachable`.
3. TIR.md section 32.8 item 7 describes delivery and reporting.

### 32.9 Image formats

TVISA's sections (section 18.2) map to the platform formats:

| Content | Mach-O (segment, section) | PE32+ | ELF64 | Wasm |
| --- | --- | --- | --- | --- |
| Code | `__TEXT`, `__text`; clones in `__text_<target>` | `.text`; clones in `.text$<n>` | `.text`; clones in `.text.<target>` | Code section |
| Read-only data | `__TEXT`, `__const` | `.rdata` | `.rodata` | Active data segment |
| Unwind | `__TEXT`, `__unwind_info` and `__eh_frame` | `.pdata`, `.xdata` | `.eh_frame`, `.eh_frame_hdr` | — |
| `.tirlsda` | `__TEXT`, `__tirlsda` | `.tirlsda` | `.tirlsda` | — |
| `.tirtrap` | `__TEXT`, `__tirtrap` | `.tirtrap` | `.tirtrap` | Custom section `tir.trap` |
| Relocated read-only data and import slots | `__DATA_CONST`, `__const` and `__got` | `.rdata` (import address table) | `.data.rel.ro` and `.got`, inside `PT_GNU_RELRO` | Globals and data |
| Dispatch slots | `__DATA`, `__tirdisp` | `.tirdisp` | `.tirdisp` | Table entries set by the start function |
| Data, zero-initialized data | `__DATA`, `__data` and `__bss` | `.data`, `.bss` | `.data`, `.bss` | Data segments |
| TLS template | `__DATA`, `__thread_data` and `__thread_bss` | `.tls` | `.tdata`, `.tbss` | Passive data segment |
| `.tirgpu` | `__TIR`, `__tirgpu` | `.tirgpu` | `.tirgpu` | Custom section `tir.gpu` |
| `.ridlmeta` | `__TIR`, `__ridlmeta` | `.ridlmd` | `.ridlmeta` | Custom section `ridl.meta` |
| `.tirdeps` | `__TIR`, `__tirdeps` | `.tirdeps` | `.tirdeps` | Custom section `tir.deps` |
| `.tirbridge` | `__TIR`, `__tirbridge` | `.tirbrg` | `.tirbridge` | Custom section `tir.bridge` |
| `.tirrt` | `__TIR`, `__tirrt` | `.tirrt` | `.tirrt` | Custom section `tir.rt` |
| Build identifier | `LC_UUID` (first 16 bytes) and `__TIR`, `__tirbuild` (32 bytes) | Debug directory CodeView record (GUID from the first 16 bytes) and `.tirbld` | `.note.gnu.build-id` (first 20 bytes) and `.note.tir.build` | Custom section `tir.build` |
| Signature | `LC_CODE_SIGNATURE` | Security directory (Authenticode) | None | None |

The `__TIR` segment is mapped read-only. PE section names are at most 8 bytes. The `tir.build` section, and its equivalents, also record the TIR version and the `tirc` build identifier.

### 32.10 Imports, relocations, dispatch and entry

1. Imports bind eagerly and their slots are read-only before any TIR code runs: on ELF, `DT_FLAGS` with `DF_BIND_NOW`, `DT_FLAGS_1` with `DF_1_NOW`, and the GOT inside `PT_GNU_RELRO`, with no PLT; on Mach-O, chained fixups that bind every import at launch into `__DATA_CONST,__got`, with no lazy stubs; on PE, the import address table, which the loader binds at load (delay loading is not used); on Wasm, imported functions are called by index and imported data through globals. Calls through slots use section 18.4's sequences with the substitutions of section 32.5.
2. Relocations: ABS64 becomes a relative relocation (ELF `R_*_RELATIVE`, a Mach-O rebase, a PE `IMAGE_REL_BASED_DIR64` base relocation) or, on Wasm, a value resolved at link time; CODE64 a plain address or, on Wasm, a table index; TPOFF64 an access in the model of section 32.6; IMPORT64 and IMPORTCODE64 slot bindings; DISPATCH64 a dispatch slot.
3. Dispatch slots live in their own page-aligned section. `libtirrt`'s first initializer fills them (section 32.2 item 4) and then makes the section read-only with `mprotect`, `VirtualProtect` or `vm_protect`. That initializer runs before every other: through `.preinit_array` in ELF executables and the first `DT_INIT_ARRAY` entry of ELF shared images; the first `__mod_init_func` entry on Mach-O; the image entry point on PE, which is `libtirrt`'s and calls every other initializer afterwards; the Wasm start function.
4. Entry points:

| Platform | Executable entry |
| --- | --- |
| `macos`, `ios` | `LC_MAIN` naming `libtirrt`'s `main`, which runs the package's entry |
| `windows` | `libtirrt`'s `tir_rt_entry`, for the console or Windows subsystem as the bundle declares |
| `linux` | `libtirrt`'s `_start`, which calls glibc's `__libc_start_main` with the package's entry |
| `android` | None: the application shell loads the shared image, whose `JNI_OnLoad` is `libtirrt`'s |
| `web` | The export `tir_main`, called by the loader |
| `wasi` | The export `wasi:cli/run` |

5. Code model: section 18.1's small model on every platform; an image above 2 GiB of code and data is rejected with `TVISA-FIN-0010`.

### 32.11 WebAssembly code generation

1. Types: TIR's 64-bit integers and pointers are `i64`; `i1` to `i32` are `i32`; `f32` and `f64` are themselves; `f16` and `bf16` values are stored as `i32` and their arithmetic is computed in `f32` and rounded, which is exact for every correctly rounded operation of those formats; `f128` uses runtime entries; vectors of at most 16 bytes and `svec` are `v128`; wider vectors are split.
2. Memory under `wasm.base`: every load and store at address `a` with access size `s` checks, as one unsigned comparison, that `a − 65536 ≤ M − 65536 − s`, where M is the memory size in bytes, and traps with `TIR-RT-0033` otherwise; the access then uses `i32.wrap_i64(a)` with the instruction's constant offset. Single-threaded builds keep `M − 65536` in a global updated after each `memory.grow`; `+threads` builds compute it from `memory.size`. A check made redundant by a dominating check of the same base over a covering range MAY be omitted.
3. Memory under `wasm.m64`: every access checks `a ≥ 65536` and traps with `TIR-RT-0033` otherwise; the engine's bounds check covers the upper end, and the host reports the engine's out-of-bounds trap as `TIR-RT-0033`.
4. The first 64 KiB of memory are never allocated, so null and near-null accesses trap as they fault on native platforms.
5. Frames: values stay in Wasm locals; address-taken frame slots live on a stack in linear memory addressed by the global `__stack_pointer`. A function with such slots checks `__stack_pointer − frame size ≥ __stack_limit` in its prologue and traps with `TIR-RT-0021` otherwise.
6. `ridl` on `wasm`: parameter groups become Wasm parameters in order; the context is an additional first `i64` parameter; a failing function returns its error word as an additional `i64` result; up to four result leaves are returned as Wasm multiple results, larger results through the indirect-result pointer. Tail calls are `return_call` and `return_call_indirect`.
7. Function values are 64-bit table indices. A function whose address is taken has one entry in the module's table; `__tir_tags` holds each entry's type tag (section 32.5).
8. Control flow is rebuilt from TVISA's structured-control-flow annotations; irreducible regions become a loop around a `br_table` dispatch.
9. Atomics: with `+threads`, TVISA atomics become Wasm atomic instructions, which are sequentially consistent and therefore satisfy every TIR ordering, and `wait` and `notify` implement wait on address; without `+threads` a program has one thread and atomics become ordinary operations.
10. Floating point follows WebAssembly, which is IEEE 754 with unspecified NaN payloads, as TIR allows (TIR.md section 5.5); `fneg`, `fabs` and `fcopysign` are Wasm's bitwise operations.
11. Globals the runtime uses: `__stack_pointer`, `__stack_limit`, `__tls_base`, `__tir_trap_code`, and, in single-threaded builds, the memory bound of item 2.

### 32.12 Development builds

The finalizer, running inside `tirc`, produces development images for `macos`, `ios`, `windows`, `linux` and `android` on aarch64, x64 and rv64 with the platform rules of this section, at least as fast as it finalizes for Tessera (section 27.5). Release images come from the server backend. The two produce the same observable behavior (section 29.3).

---

## 33. Versioning

### 33.1 Version identifiers

Tessera versions are written `YY.M` or `YY.M.W`: YY is the year within the century (1–99, so 26 is 2026), M the month (1–12), and W, optional, the week within the month (1–5). Versions order by year, then month, then week, with an absent week before week 1.

| Artifact | Form | Example |
| --- | --- | --- |
| TIR and TVISA versions | `YY.M.W`: the train that shipped on the W-th Thursday of month M | 26.11.1 |
| DeviceKit Driver API versions | `YY.M` (DeviceKit.md) | 26.10 |

Trains ship every Thursday. W is the ordinal of the train's Thursday within its month: a month has four or five trains (December 2026 has 26.12.1 to 26.12.5, the last on December 31), and a year has 52 or 53. A window of 26 trains is about six months and one of 52 trains about twelve.

In binary, a version is three bytes: year, month, and week (0 when absent), as in the header of section 24.2.

### 33.2 One version per train

Every train has exactly one TVISA version, with the same identifier as the train's TIR version. `tirc` of train T emits TVISA of version T only (TIR.md section 24.9).

### 33.3 Immutability

A released TVISA version never changes: its encoding, instruction semantics, conventions, hardening sequences, runtime entry table, intrinsic registry, CPU targets, feature names and GPU family codes are frozen. A correction is a new version.

### 33.4 Window

The finalizer and the GPU backends of train T accept TVISA images of versions within the window of TIR.md section 24.4 for the image's profile: the 26 trains ending at T for `app` and `system`, the 52 trains ending at T for `driver`. Older images are rejected with `TVISA-VER-0001` and newer ones with `TVISA-VER-0002`. Kernel TVISA never leaves the pipeline.

### 33.5 What a newer finalizer guarantees

The finalizer of train T finalizes an image of version V < T with V's semantics exactly, including V's hardening sequences, runtime entry identifiers and intrinsic definitions, and links it against `libtirrt` of train T, which implements every entry of every version in the window (section 12.3).

### 33.6 Change log

Every version has an entry in `TVISA-CHANGES.md`, in the format of TIR.md section 24.8, with the additional headings `Runtime entries`, `Intrinsics`, `Targets and features`, `GPU families` and `Hardening sequences`.

---

## 34. Conformance and diagnostics

### 34.1 Suites

| Suite | Content |
| --- | --- |
| Encoding | Binary and text round trips; canonical re-encoding; malformed inputs (fuzzed) rejected without crashing |
| Verifier | A positive and a negative case for every rule of section 26.3 |
| Conventions | For every architecture and convention: assembly-level callers and callees for every item kind, groups that fit and groups that spill, `i128` with one register left, rv64 vector expansion, results in registers and indirect, failing calls, context, callee-pop with stack arguments, tail calls between every pair of signatures, `cagg` against Clang's classification of the same C types; run across the finalizer, the server backend and engine bridges |
| Hardening | Every sequence of section 17 byte for byte; audit (section 34.5) of every image of the corpus; fault injection that corrupts tags, signed pointers, return addresses, shadow stacks and protector values and expects the specified trap codes |
| Atomics | Litmus tests for every pattern of the C++20 model on every architecture, with the two sides of each test compiled by different tiers (interpreter, JIT, finalizer, server backend) |
| Runtime | Every entry of section 12.2, called from both backends; entry binding and versioning across the window |
| Intrinsics | Every registry entry in both backends, against the architecture's reference semantics, on hardware or an instruction-accurate emulator |
| Assembly | Byte equality of every template of the corpus between `tirc`'s assembler, the finalizer and the server backend |
| Variants | Member equivalence (TIR.md section 34.1) |
| GPU | Every construct on every family backend against the reference executor, in default and conservative modes; stage linking without recompilation; unknown-revision fallback; `tvgpu` on Metal, Vulkan and the reference executor |
| Determinism | Two finalizations of the corpus on different hosts, thread counts and debug options are bit-identical; server builds are bit-identical |
| Parity | TIR.md section 34.1, semantics parity, with finalized TVISA as a tier |
| Platforms | For every platform and architecture of TIR.md section 32.2: the convention suite against the platform's C compiler, every sequence of section 32.5 byte for byte and audited, unwinding through every table format of section 32.7, trap delivery, image layout of section 32.9, dispatch and binding of section 32.10, and WebAssembly code generation of section 32.11 against Clang's `wasm32` and `wasm64` output |
| Performance | Section 34.4 |

### 34.2 Parity rules

TIR.md section 34.3 applies unchanged.

### 34.3 Reference devices

For each train, `TVISA-REFERENCE-DEVICES.md` lists the reference devices on which throughput and quality gates are measured: for each architecture, the slowest device of the high-end class Tessera supports on that train, and one mid-range device.

### 34.4 Performance gates

| Gate | Requirement |
| --- | --- |
| Finalizer throughput | ≥ 2 MiB of TVISA per second per performance core on each reference device |
| Finalized code | Geometric-mean runtime ≤ 1.20× the server backend's for the same target on the benchmark corpus |
| Server backend | Geometric-mean runtime ≤ 1.03× the same source compiled directly by the same LLVM (TIR.md section 34.4) |
| TVISA size | ≤ 1.5× the size of the aarch64 native code finalized from it, on the corpus |
| GPU compile time on device | ≤ 1.5× the pipeline's compile time for the same entry on the same family, normalized for CPU speed |

### 34.5 Audit

`tirc audit <image>` checks the properties of section 17.12 on a native image and reports `TVISA-AUD-NNNN` diagnostics. The pipeline audits every native image before signing it. The compile service audits finalized images in development builds of the system and a sample of finalized images in release builds, reporting failures as finalizer defects.

### 34.6 Diagnostics

Codes have the form `TVISA-<AREA>-<NNNN>` and render as TIR's (TIR.md section 34.5).

| Code | Meaning |
| --- | --- |
| `TVISA-ENC-0001` | Malformed encoding |
| `TVISA-ENC-0002` | Non-canonical image in a delivery unit |
| `TVISA-ENC-0003` | Type tag not allowed in TVISA |
| `TVISA-ENC-0004` | Unknown section kind |
| `TVISA-VER-0001` | Version older than the window |
| `TVISA-VER-0002` | Version newer than the finalizer's train |
| `TVISA-VFY-NNNN` | Section 26.3 |
| `TVISA-CF-000N` | GPU structured control-flow rule SN |
| `TVISA-ARCH-0001`, `TVISA-ARCH-0002` | Section 26.3 |
| `TVISA-GPU-0001` | Entry uses a capability the family revision lacks |
| `TVISA-GPU-0010` | Family backend failed in default and conservative modes (backend defect) |
| `TVISA-FIN-0001` | CPU finalization failed in default and conservative modes (finalizer defect) |
| `TVISA-FIN-0010` | Image exceeds the code model (section 18.1) |
| `TVISA-RT-0002` | Image `svec` mode does not match the process (section 15.2) |
| `TVISA-RT-0003` | Static TLS surplus exhausted (section 19.2) |
| `TVISA-RT-0004` | CPU lacks a feature of the image's target (section 30.4) |
| `TVISA-AUD-0001` | Indirect branch without a specified sequence |
| `TVISA-AUD-0002` | Indirect target without landing pad or tag |
| `TVISA-AUD-0003` | Prologue or epilogue deviates from section 17.3 |
| `TVISA-AUD-0004` | Missing or deviating stack protector |
| `TVISA-AUD-0005` | Trap-site table inconsistent |
| `TVISA-AUD-0006` | Writable and executable mapping, or relocation in code |
| `TVISA-AUD-0007` | Platform hardening sequence deviates from section 32.5 |
| `TVISA-PLAT-0001` | Call not representable on the platform (for example a tail call through a `[foreign]` value on Windows) |
| `TVISA-PLAT-0002` | WebAssembly image exceeds an implementation limit of the target (functions, locals, table or memory size) |

Runtime trap code added to TIR's table by this document: `TIR-RT-0014`, stack protector check failed (section 17.6).

---

## 35. Implementation plan and crate layout

### 35.1 Crates

TVISA's crates sit in TIR's layers (TIR.md section 35.1):

| Layer | Crates |
| --- | --- |
| 0 | `tir-target` gains the CPU target, feature, tuning and GPU family tables of sections 30 and 31 |
| 1 | `tvisa-format` (binary, text, canonical encoding) |
| 2 | `tvisa-verify` |
| 4 | `tir-tvisa` (lowering from TIR), `tvisa-isa-aarch64`, `tvisa-isa-x64`, `tvisa-isa-rv64` (instruction definitions, encoders, integrated assemblers), `tvisa-image` (the image writer shared by both backends: layout, slots, relocations, unwind tables, LSDAs, trap tables), `tvisa-harden` (the sequences of section 17, used by the finalizer and generated into the LLVM fork's tests), `tir-llvm` (server backend), `tvisa-gpu-common` (stages 1 to 4 of section 22.2), the GPU family backends of TIR.md section 35.1, `tvisa-platform` (the conventions, sequences, unwind formats, image mappings and dispatch of section 32) and `tvisa-wasm` (section 32.11) |
| 5 | `tir-finalize` (CPU finalizer on Tessera's Cranelift fork) |
| 7 | `tvisa-audit` |

### 35.2 Milestones

| Milestone | Content | Exit test | TIR milestone |
| --- | --- | --- | --- |
| V0 | Formats, verifier | Encoding and verifier suites | T3 |
| V1 | Lowering; server backend for aarch64; `ridl` with callee-pop and groups | Convention suite on aarch64; parity with the interpreter | T3 |
| V2 | Image writer; hardening sequences in the LLVM fork; audit | Hardening suite on aarch64 | T3 |
| V3 | x64 and rv64 in the server backend; variant and dispatch sets; TLS | Convention, hardening and parity suites on three architectures | T5 |
| V4 | Finalizer on three architectures; runtime entries; unwinding | Parity with finalized TVISA; determinism suite | T8 |
| V5 | Compile service integration; code cache; re-finalization for symbolication | Runtime suite; throughput gate | T8 |
| V6 | GPU profile; `tvgpu` backend and host translator | GPU suite on `tvgpu` and the reference executor | T9 |
| V7 | GPU family backends, one family at a time | GPU suite per family, including conservative mode | T10 |
| V8 | Tuned targets; performance gates | Section 34.4 | T12 |
| V9 | Platform conventions, sequences, unwind tables and image mappings for `linux`, `android`, `macos`, `ios` and `windows`; hosted translation targets | Platform rows of section 34.1 | T13, T14 |
| V10 | WebAssembly code generation for `wasm.base` and `wasm.m64` | Platform rows of section 34.1 for `web` and `wasi` | T15 |

### 35.3 Working rules

TIR.md section 35.3 applies. A change to any normative sequence of section 17 updates `tvisa-harden`, the LLVM fork and the audit tool in the same commit.

---

## 36. Appendix A: grammars and container layouts

Notation: `{ x }` repetition, `[ x ]` option, `|` alternative, quoted terminals. Newlines separate items and instructions.

### A.1 Textual TVISA

```
image       = header { item }
header      = "tvisa" version "profile" profile "image" string "kind" kind
version     = digits "." digits "." digits
profile     = "app" | "system" | "driver" | "kernel"
kind        = "executable" | "framework" | "plugin" | "driver" | "kernel"
item        = sigdef | import | data | func | variants | dispatch | gpuentry | metadata

sigdef      = "sig" "$" ident "=" conv [ "fails" ] [ "variadic" digits ] [ "noreturn" ]
              "(" [ pitem { "," pitem } ] ")" "->" "(" [ ritem { "," ritem } ] ")"
conv        = "ridl" | "c" | "rtcc" | "gpu" | "entry"
pitem       = "leaf" type
            | "group" size align "{" member { "," member } "}"
            | "byref" size align [ "[" byrefflag { "," byrefflag } "]" ]
            | "sret" size align
            | "context"
            | "cagg" cdesc
ritem       = "leaf" type | "group" size align "{" member { "," member } "}"
member      = offset ":" type
cdesc       = size align "{" { offset ":" type [ ":" digits ] } "}" [ "[" caggflag { "," caggflag } "]" ]
byrefflag   = "owned" | "readonly"
caggflag    = "union" | "unaligned" | "nontrivial" | "flexible" | "empty"

import      = "import" ( "function" sym import_module "sig" sigref "tag" hex
                       | "data" sym import_module size align )
import_module = "ridl" string "abi" uuid
              | "package" string "abi" uuid
              | "platform" string [ "weak" ] "abi" uuid
uuid        = string                         (lowercase 8-4-4-4-12 hexadecimal UUID)
data        = "data" sym [ linkage ] datakind [ "percpu" ] "align" align ( "bytes" string | "zero" size | "{" "size" size { dataelem } "}" )
              { "reloc" offset relockind sym [ ( "+" | "-" ) integer ] [ "tag" hex ] }
dataelem    = offset ":" "hex" string
relockind   = "ABS64" | "CODE64" | "REL32" | "GPUENTRY64" | "TPOFF64" | "IMPORT64" | "IMPORTCODE64" | "DISPATCH64"
offset      = integer
size        = integer
align       = integer
integer     = digits | binary | octal | hex
digits      = decimal_digit { decimal_digit | "_" }
binary      = ( "0b" | "0B" ) binary_digit { binary_digit | "_" }
octal       = ( "0o" | "0O" ) octal_digit { octal_digit | "_" }
hex         = ( "0x" | "0X" ) hex_digit { hex_digit | "_" }
datakind    = "readonly" | "relro" | "writable" | "zero" | "tls" | "tls_zero" | "gpu_constant"
func        = "func" linkage sym "sig" sigref [ "tag" hex ] [ "attrs" "(" attr { "," attr } ")" ]
              [ "variant" "(" arch { "," feature } ")" ] "trust" trust "semantics" ( "defined" | "permissive" )
              [ "personality" "tir" ] [ "slots" "{" { slotdef } "}" ] "{" { block } "}"
slotdef     = digits ":" "size" digits "align" digits [ space ] [ "[" slotflag { "," slotflag } "]" ]
block       = label [ "(" [ param { "," param } ] ")" ] [ "loop" "(" "merge" label "," "continue" label ")" ] ":"
              { instruction } terminator
param       = value ":" type
instruction = [ value { "," value } "=" ] opname { modifier } [ operands ] [ "!loc" "(" loc ")" ]
callee      = sym | value ":" sigref "," hex | "rt" hex
variants    = "variants" sym "{" { arch [ "[" feature { "," feature } "]" ] ":" sym } "}"
dispatch    = "dispatch" sym "{" { arch [ "[" feature { "," feature } "]" ] ":" sym } "}"
gpuentry    = "entry" entrykind sym "record" "{" { entryfield } "}"
metadata    = runtime_metadata | image_metadata
runtime_metadata = "metadata" "runtime" "[" [ hex { "," hex } ] "]"
image_metadata = "metadata" "image" "{" image_field { "," image_field } "}"
image_field = "package_digest" ":" string
            | "tirc_build" ":" string
            | "module_hashes" ":" "[" string { "," string } "]"
            | "entry_function" ":" ( "none" | string )
            | ( "initializers" | "finalizers" ) ":" "[" [ "[" string "," digits "]" { "," "[" string "," digits "]" } ] "]"
            | "gpu_capabilities" ":" "[" [ feature { "," feature } ] "]"
            | "minimum_driver_api" ":" ( "none" | string )
entryfield  = entry_field_name ":" entryvalue newline
            | "typeid" ":" hex newline                         (visible entries only)
entry_field_name = "threads" | "root" | "immediate" | "stage_input" | "stage_output"
                 | "payload" | "mesh" | "threadgroup_memory" | "capabilities"
                 | "subgroup_size" | "floating_point" | "specialization_constants"
                 | "node" | "ray_tracing" | "fragment"
entryvalue  = scalar | "none" | "any" | boolean | "[" [ entryvalue { "," entryvalue } ] "]"
            | "{" [ ident ":" entryvalue { "," ident ":" entryvalue } ] "}"
scalar      = literal | type | ident [ "(" ( ident | integer ) ")" ]
boolean     = "true" | "false"
entrykind   = "compute" | "object" | "mesh" | "vertex" | "fragment" | "tile"
            | "raygen" | "intersection" | "anyhit" | "closesthit" | "miss" | "callable"
            | "node" | "visible"

type        = "i" integer | "f16" | "bf16" | "f32" | "f64" | "f128"
            | narrowfloat | "ptr" [ "<" space ">" ] | "code" | "vec" "<" integer "x" type ">"
            | "svec" "<" type ">" | gputype
linkage     = "local" | "image" | "exported"
trust       = "managed" | "sandboxed" | "unmanaged"
arch        = "portable" | "aarch64" | "x64" | "rv64"
sym         = "@" qualified
sigref      = "$" ident
value       = "%" ident
label       = "bb" digits
```

Instruction operands follow TIR's textual syntax (TIR.md Appendix A.1) for shared instructions and section 11.5 for TVISA-only instructions. `%p + n` in an address position denotes a `ptr.offset` by the constant n.

For an indirect call, the `hex` after the signature is a valid nonzero function type tag, or `0x00000000` for a foreign call with a `c` signature (section 11.6). Zero is not accepted by code-pointer conversion, load or store instructions.

The field names, object members, array meanings, requiredness, and value
constraints are defined in section 21.2. Non-visible entries spell every
section 21.2 field exactly once; visible entries spell only typeid. Record
fields and object members may be written in any order. Each top-level record
field occupies one line; arrays use brackets, objects use comma-separated
named members, and scalar type and literal syntax follows this grammar and
TIR.md Appendix A.1.

For example:

    entry compute @Demo.blur record {
      threads: [8, 8, 1]
      root: none
      immediate: none
      stage_input: []
      stage_output: []
      payload: none
      mesh: none
      threadgroup_memory: 0
      capabilities: []
      subgroup_size: any
      floating_point: []
      specialization_constants: [{id: 1, type: i32, default: 1}]
      node: none
      ray_tracing: none
      fragment: none
    }

The scalar literal production is TIR.md Appendix A.1's literal; type is the
TVISA type grammar above. The example record assumes that the containing image
defines the matching compute function with no root or immediate-data parameter.

### A.2 Native image tables

`.tirtrap`:

| Field | Encoding |
| --- | --- |
| Header | `u32` entry count |
| Entry | `u32` image-relative offset of the trap instruction, `u16` code, `u16` zero |

Entries are sorted by offset.

`.tirimport` (inside `.data.rel.ro`):

| Part | Content |
| --- | --- |
| Header | `u32` function-import count, `u32` data-import count, `u32` runtime-entry count, `u32` dispatch-set count |
| Slots | 8 bytes each, in the order: function imports, runtime entries, dispatch sets, data imports |
| Descriptors | Per slot (outside the loaded data, in the image's loader records): module, symbol or runtime identifier, ABI UUID, tag |

`.tirfeat`: one byte per feature named by any `target.has` that was not folded, in the order the loader records list them; 1 when the CPU has the feature.

Function-start table: `u32` count, then `u32` image-relative entry offsets of every function in ascending order, used for backtraces of finalized images without DWARF.

Image header record for finalized images (in the platform image's notes): TVISA image hash (32 bytes), CPU target name, feature bit set, finalizer build identifier (32 bytes), `svec` mode.

---

## 37. Appendix B: decision log

| Id | Decision | Reason |
| --- | --- | --- |
| TV-001 | TVISA is a delivered format, received by devices, and the only input to every native code generator | One semantics for every build path; on-device finalization without LLVM |
| TV-002 | TVISA reuses TIR's opcodes, modifiers and type tags for everything it shares, and adds a range at 0x700 | Lowering and backends share decoders; no second numbering to keep consistent |
| TV-003 | No aggregates, ownership, generics or coroutines in TVISA | Everything target-independent happens in `tirc`; the finalizer does only target work |
| TV-004 | Parameter groups are decided per architecture by the finalizer | Register availability differs by architecture; TVISA stays architecture-neutral |
| TV-005 | `ridl` stack arguments are callee-popped | Guaranteed tail calls for every RIDL signature (TIR.md section 11.5); matches LLVM `swifttailcc`; no RIDL 1.0 binaries exist |
| TV-006 | `rtcc` preserves almost every register | Retain, release and allocation fast paths do not force spills |
| TV-007 | `c` aggregates travel as descriptors classified by the finalizer | C psABI classification needs the architecture; the descriptor keeps TVISA neutral |
| TV-008 | Hardening sequences are normative instruction by instruction | Mechanical audit of every native image |
| TV-009 | Type tags avoid encodings that form landing pads or `lpad` words | A tag word must never itself be a valid indirect target |
| TV-010 | aarch64 code pointers have a register form and an address-diversified storage form | Spilled and stored pointers stay signed; stored pointers cannot be swapped between slots |
| TV-011 | Imports bind eagerly into read-only slots; no PLT, no lazy binding | Simpler loader, no writable code-pointer tables after start |
| TV-012 | x18 on aarch64 is never used | Reserved for the platform, as on other modern aarch64 platforms |
| TV-013 | The TCB is 64 bytes at the thread pointer on every architecture | One layout for the protector, the stack limit and runtime state |
| TV-014 | Static TLS only, with a 64 KiB surplus for late-loaded images | Fast TLS access everywhere; no TLS descriptors or `__tls_get_addr` |
| TV-015 | Mathematics on CPUs goes through shared correctly rounded runtime entries | Bitwise parity across tiers and architectures |
| TV-016 | One atomics mapping per architecture for every tier | Code from different tiers shares memory in one process |
| TV-017 | The process `vscale` comes from the executable's image; images record `fixed` or `agnostic` | Scalable vectors are consistent across every image and tier of a process |
| TV-018 | Trap codes are embedded in the trap instruction where possible and listed in `.tirtrap` | Exact trap reporting; breakpoints remain distinguishable |
| TV-019 | Finalized images carry no DWARF; symbolication re-finalizes deterministically | Saves device storage without losing symbolication |
| TV-020 | GPU resource handles have one format on every family | Single-source CPU and GPU code stores handles without knowing the family |
| TV-021 | GPU stages compile separately and link by semantic and location | Pipeline creation never compiles |
| TV-022 | GPU backends retry in a conservative mode that must succeed | On-device GPU compilation is robust |
| TV-023 | The paravirtual GPU executes TVISA GPU code translated on the host | Development virtual machines run Ember with hardware semantics |
| TV-024 | Versions are `YY.M.W`, W the Thursday's ordinal in its month (1–5) | A strict weekly cadence; versions still read as dates, and months with five Thursdays get a fifth train |
| TV-025 | CPU targets are feature sets plus modifiers, tuned targets add a tuning model | Feature sets decide correctness; tuning only performance |
| TV-026 | `x64.v3` includes AES, PCLMUL, VAES, VPCLMULQDQ, RDRAND, RDSEED and ERMS | Every CPU with CET has them |
| TV-027 | Kernel code is lowered through TVISA but never delivered as TVISA | Nothing can finalize code before the kernel boots |
| TV-028 | Platform targets are compiled from the same TVISA images, with the platform's own conventions, unwind tables, TLS and image formats | One lowering for every platform; platform loaders and tools work unchanged |
| TV-029 | Platform baselines (`aarch64.v80`, `aarch64.v82`, `aarch64.apple1`, `x64.v2`, `x64.v3h`, `x64.v4h`, `rv64.rva23`) are separate from Tessera's, with clones for optional targets | Platforms run on hardware Tessera does not require; clones recover performance on newer CPUs |
| TV-030 | On platforms, code pointers are not signed and return addresses use hint-space instructions only | Platform operating systems manage pointer-authentication keys themselves; hint-space code is correct on every CPU |
| TV-031 | WebAssembly is generated from TVISA by the server backend only | The finalizer targets machine code; browsers and WASI runtimes compile WebAssembly themselves |
| TV-032 | Zero is reserved as the foreign indirect-call marker and is valid only with a `c` signature | The lowered signature omits TIR's `[foreign]` qualifier; the marker preserves tag-check and trust distinctions without changing instruction layout |

