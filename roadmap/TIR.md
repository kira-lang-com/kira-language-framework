# TIR Specification 1.0

Date: 2026-11-01. TIR version: 26.11.1. Status: normative draft 1.0. Audience: every agent and engineer who implements a TIR producer, the TIR compiler, the device runtime, or the build and distribution pipeline.

Contents

1. Status, scope and rules for implementers
2. Terminology
3. Goals, non-goals and related work
4. System overview
5. Machine model and the tir64 target
6. Modules, profiles and engines
7. Type system
8. Data layout
9. Ownership model
10. Semantic modes: defined and permissive
11. Control flow
12. Instruction set
13. Coroutines and async
14. Unwinding
15. Concurrency and the memory model
16. Vectors and SIMD
17. GPU execution model
18. Generics
19. RIDL integration and RIDL 1.1
20. Architecture-specific code
21. Analysis data, capabilities and control-flow integrity
22. Binary format (`.tirb`)
23. Textual format (`.tir`)
24. Versioning and compatibility
25. Producers
26. `tirc`, the TIR compiler
27. Lowering and hardening
28. The device runtime
29. Packages and delivered artifacts
30. The build and distribution pipeline
31. Kernel and driver profiles
32. Platform targets
33. Debug information
34. Conformance and diagnostics
35. Implementation plan and crate layout
36. Appendix A: grammars and schemas
37. Appendix B: decision log

---

## 1. Status, scope and rules for implementers

This document is the single normative specification of TIR, the Tessera Intermediate Representation. It defines TIR's machine model, type system, data layout, ownership model, semantic modes, instruction set, coroutines, unwinding, memory model, vector and GPU execution models, generics, RIDL integration, architecture-specific code, analysis data, binary and textual formats, versioning, producer requirements, the TIR compiler `tirc`, lowering and hardening, the device runtime, packages, the build and distribution pipeline, the kernel and driver profiles, and the platform targets through which TIR programs run on macOS, iOS, Windows, Linux, Android, the web and WASI. An implementation that satisfies every MUST in this document is a conforming implementation of TIR version 26.11.1.

### 1.1 Normative language

- MUST, MUST NOT, SHOULD, SHOULD NOT and MAY carry their RFC 2119 meanings.
- Sections or paragraphs titled "Rationale" or "Example" are informative. Everything else is normative.
- Where prose and a table disagree, the table wins. Where a grammar in Appendix A and an example disagree, the grammar wins.
- Every numeric constant (opcode values, type tags, section kinds, offsets, magic numbers, limits, window lengths) is normative.

### 1.2 Document set

All specifications live in the Tessera repository and reference each other by file name.

| Document | Defines | Relationship to TIR |
| --- | --- | --- |
| `TIR.md` | This document | |
| `TVISA.md` | The Tessera Virtual ISA: the delivered virtual instruction set for CPUs and GPUs, its encoding, CPU and GPU profiles, the finalizer, per-architecture and per-GPU-family lowering | TIR lowers to TVISA (section 26.6). Everything after that boundary is TVISA.md's |
| `Ember.md` | The GPU API | Consumes GPU entry points, entry references and GPU sections of images (section 17) |
| `KSL.md` | The Kira Shading Language | A TIR producer through KLF; its GPU code obeys section 17 |
| `Tarwin.md` | The Tarwin kernel, its boot protocol, loader and image format | Compiled through TIR under the kernel profile (section 31.1); loads native images |
| `DeviceKit.md` | The driver model and Driver API | Drivers are TIR packages under the driver profile (section 31.2) |
| `TRFS.md` | The Tarwin File System | Stores packages, delivery units and the code cache; no TIR semantics depend on it |
| `LibTessera.md` | The system interface | Capability rules (section 21) consume its call and entitlement tables |
| `KLF.md` | The Kira Language Framework and RIDL | RIDL (KLF.md sections 19 to 26) is the ABI of every module boundary; KLF frontends are TIR producers (section 25.2) |

Precedence:

1. TIR replaces KIR, the KLF bytecode, the KLF VM, the KLF native backends and the KLF hybrid runtime. KLF.md sections 10 to 17 are superseded by this document. Where KLF.md and TIR.md disagree about anything below a KLF frontend, TIR.md wins.
2. RIDL as specified in KLF.md sections 19 to 26 remains normative. Section 19 of this document specifies the RIDL 1.1 additions that TIR requires. They are additive under KLF.md section 24.4.
3. TVISA.md is normative for everything after the TIR-to-TVISA boundary.
4. LibTessera.md is normative for every system call and entitlement name used in section 21.

### 1.3 Scope and stability

| Artifact | Role | Stability |
| --- | --- | --- |
| TIR (`.tir`, `.tirb`) | The format in which all Tessera code above the boot firmware is shipped for compilation | One version per weekly release train. A released version is frozen forever. `tirc` accepts the 26 most recent versions (52 for the driver profile) |
| `tir64` target | The neutral target producers compile for | Versioned with TIR |
| Package (`.tirpkg`) | The unit a developer submits | Versioned with TIR |
| Delivery unit (`.tirdu`) | What a device receives: native images, TVISA images, runtime TIR, resources | Versioned with TIR and TVISA |
| TVISA (`.tvisa`) | The delivered virtual ISA | TVISA.md |
| `tirc` | The only program that produces TVISA or native code from TIR | Implementation; behavior fixed by this document |
| Device runtime | Verifier, interpreter, JIT tiers, finalizer, code cache | System component; behavior fixed by this document |
| Platform image | An application or library for macOS, iOS, Windows, Linux, Android, the web or WASI, built by `tirc` (section 32) | Never expires; carries its own runtime |

### 1.4 Rules for implementing agents

1. Implement behavior exactly as written. Where the document is silent, choose the behavior that keeps every execution tier (interpreter, baseline JIT, optimizing JIT, ahead-of-time LLVM, ahead-of-time Cranelift-class, finalized TVISA) observably identical on every architecture, and record the choice in `docs/decisions.md` in the format of Appendix B.
2. Every CPU feature MUST work on every tier on aarch64, x64 and rv64. Every GPU feature MUST work on the GPU reference executor (section 28.11), on the paravirtual GPU backend, and on every GPU family backend that advertises the capability. On platform targets, every CPU feature MUST work on every tier the platform offers (section 32.8) and on every architecture section 32.2 lists for it.
3. No placeholder, stub, hardcoded success or host-rendered substitute counts as an implementation. A section is implemented when its conformance tests (section 34) pass on every tier and architecture it names.
4. `tirc`, the device runtime and the pipeline are written in Rust. Host builds use the Rust toolchain. Tessera builds of the same code go through TIR like everything else.
5. Rust files stay under 700 lines, generated files excepted. No Python in tracked files.
6. `tirc` MUST build against the latest stable LLVM release carried by the Tessera LLVM fork, which implements the `ridlcc` calling convention and the `tir64` target.
7. No component may produce executable code for Tessera from TIR except `tirc` and the device runtime, nor for platform targets except `tirc` and the in-process JIT of `libtirrt` (section 32.8). Third parties MAY read, analyze and transform TIR, and MAY write producers.

---

## 2. Terminology

| Term | Meaning |
| --- | --- |
| Producer | A program that emits TIR modules: a KLF frontend, a converter, or a third-party compiler |
| Converter | A producer whose input is another IR: LLVM bitcode, Wasm64, SPIR-V |
| `tirc` | The TIR compiler. Verifies, analyzes, optimizes and lowers TIR. Runs on developer machines, in the pipeline, and in its device subset on devices |
| Pipeline | The build and distribution pipeline (section 30): intake, verification, analysis, compilation of the target matrix, signing, delivery, expiry |
| Device runtime | The on-device system component made of the verifier, the interpreter, the JIT tiers, the finalizer and the code cache (section 28) |
| Train | One weekly release of the Tessera and Kira ecosystems. Trains ship every Thursday (section 24.1) |
| TIR version | The identifier of the train that released a TIR revision, written `YY.M.W` (section 24.1). The first version is 26.11.1 |
| Window | The set of TIR versions a `tirc` accepts: the version of its own train and the 25 before it (51 for the driver profile) |
| Module | The unit of TIR: one `.tirb` file |
| Package | A signed set of modules, resources and a manifest (`.tirpkg`) |
| Profile | The rule set a module is compiled under: `app`, `system`, `driver`, `kernel` |
| Engine | Where a function body executes: `native` (ahead-of-time), `runtime` (interpreter and JIT tiers), `any` (tirc decides) |
| Tier | One way of executing TIR on a device: interpreter, baseline JIT, optimizing JIT, finalized TVISA, native image |
| Native image | Machine code for one CPU target and the GPU binaries for one or more GPU families, in the platform image format (Tarwin.md; ELF64 on the Linux backend) |
| TVISA image | A module lowered to TVISA, finalized on the device |
| Finalizer | The device runtime component that turns a TVISA image into native code for the device's CPU and GPU |
| Delivery unit | The signed artifact a device receives for one package (`.tirdu`) |
| Symbol bundle | Debug information and the TIR-to-native mapping for one native image (`.tirsym`) |
| Canonical TIR | TIR in which all ownership operations are explicit (section 9.6). Only canonical TIR is shipped |
| Managed function | A function that contains no unmanaged instruction and whose producer supplied ownership information |
| Sandboxed function | A function whose every memory access is confined by construction to regions declared by its module (Wasm64 input, section 25.4) |
| Unmanaged function | Any other function |
| Defined function | A function whose every instruction has a defined result or a defined trap (section 10) |
| Permissive function | A function that may contain instructions with undefined behavior under stated conditions (section 10) |
| Capability declaration | The record of LibTessera calls, entitlements and system-framework requirements a module reaches (section 21) |
| GPU entry | A function executed by a GPU stage: compute, object, mesh, vertex, fragment, tile, ray tracing, node |
| Entry reference | A typed 64-bit value naming a GPU entry, usable from CPU code (section 17.17) |
| Address space | The memory a pointer refers to: `generic` on CPUs; `device`, `constant`, `threadgroup`, `thread`, `object_payload`, `ray_payload`, `hit_attribute`, `tile`, `node_record` on GPUs |
| Resource handle | An 8-byte opaque value naming a texture, sampler, acceleration structure, visible function table or tensor (section 17.4) |
| GPU pointer | A 64-bit address in the `device` or `constant` address space |
| Variant set | A function with per-architecture bodies (section 20) |
| CPU target | A named instruction-set baseline for one architecture, defined by TVISA.md (for example `aarch64.base`) |
| GPU family | A named GPU instruction-set family with a Tessera-owned backend, defined by TVISA.md (for example `amd.gfx110`) |
| Target matrix | The CPU targets and GPU families the pipeline builds for (section 30.3) |
| Platform | An operating environment TIR programs run on: `tessera`, `macos`, `ios`, `windows`, `linux`, `android`, `web` or `wasi` (section 32.2) |
| Platform target | A platform other than `tessera`, for which `tirc` builds platform images (section 32) |
| Portable module | A module that imports neither LibTessera nor any platform library and runs on every platform (section 32.3) |
| Platform module | A module that imports one platform's libraries (section 32.6) |
| Foreign function value | A function value of a type marked `[foreign]`, pointing to platform code or to a TIR function made callable by platform code (section 32.6) |

---

## 3. Goals, non-goals and related work

### 3.1 Goals

1. One portable format for all Tessera code above the boot firmware: apps, frameworks, services, shells, drivers and the Tarwin kernel. User-space code is delivered as native images or TVISA images. Kernel images are compiled through TIR and delivered native.
2. Lose nothing. Ownership, aliasing, layouts, generics, coroutines, unwinding and GPU semantics survive from the producer to the backend. A producer that cannot supply a piece of information marks it absent and never invents it.
3. Peak performance. A native image built by the pipeline MUST run within 3% of the same source compiled directly by the same LLVM release at the same optimization level for the same CPU (section 34.4). A finalized TVISA image MUST run within 20% of the pipeline's native image for the same target.
4. CPU and GPU in one format, with one type system and shared layouts, so that a producer can check at compile time that the data a CPU passes to a GPU entry has the entry's types.
5. Analyzable. Types, typed indirect calls, structured GPU control flow, capability declarations, the unmanaged manifest and the analysis report make every package reviewable without decompilation.
6. Open. Any party may write a producer. No construct is reserved to Kira. Every annotation a KLF frontend emits is available to every producer.
7. Complete on device. Unknown hardware is served by the finalizer. Dynamic languages are served by the interpreter and the JIT tiers. Development is served by hot module push.
8. Reproducible. Given the same package, `tirc` build and target, every output is bit-identical.
9. Modern without legacy. 64-bit only, little-endian only, three CPU architectures (aarch64, x64, rv64) and WebAssembly for the web and WASI, hardware control-flow integrity mandatory on Tessera, bindless GPUs only.
10. Everywhere. The same TIR runs on macOS, iOS, Windows, Linux, Android, the web and WASI through platform targets (section 32), with the same semantics and a bundled runtime, so a program is written once for every platform.

### 3.2 Non-goals

- TIR is not a source language. Nobody is expected to write TIR by hand outside tests.
- TIR is not a forever-stable format. Every train releases a version; a version is accepted for 26 trains (about six months).
- No component outside `tirc` and the device runtime produces executable code from TIR. Tessera devices execute only pipeline-signed images, development-signed images on development devices, and TIR verified by the device runtime.
- No tracing garbage collector, no reflection requirement, no runtime type registry.
- No 32-bit pointers, no big-endian targets, no x87, no IEEE-noncompliant CPU floating point. WebAssembly's 32-bit memories keep TIR's 64-bit pointers (section 32.5), and a platform `long double` in the x87 format is unreachable (section 32.4).
- No legacy GPU stages: no geometry shaders, no tessellation shaders, no transform feedback, no fixed-function vertex input. Converters lower those features of their inputs to mesh, compute and vertex-pulling code (section 25.5).
- No GPU assembly and no per-GPU-family code in 1.0.

### 3.3 Related work (informative)

| System | What TIR shares | Where TIR differs |
| --- | --- | --- |
| LLVM IR and bitcode | SSA form, metadata, the optimizer it is lowered into | TIR carries ownership, a defined mode, GPU code in the same format, a neutral target, and a versioned window. LLVM is one backend of `tirc` |
| Apple Bitcode | Store-side recompilation from IR | Bitcode was LLVM IR locked to one target and was abandoned. TIR has a neutral target, gives developers the exact delivered images, keeps symbol bundles, and builds reproducibly |
| SPIR-V | A portable GPU IR with structured control flow | TIR replaces it on Tessera. CPU and GPU share one type system. Resources are bindless handles and GPU pointers only |
| WebAssembly | Portability, openness, validation | Wasm is a sandbox with a host interface. TIR is a complete systems IR with ownership, RIDL and GPUs. Wasm64 is a TIR input |
| PTX | A virtual ISA finalized on the device | TVISA plays that role for CPUs and for GPUs from every vendor |
| Android ART | Interpreter, JIT and ahead-of-time tiers | TIR is statically typed, systems-level and ahead-of-time first |
| .NET CIL and ReadyToRun | Portable IR plus ahead-of-time images | TIR has no garbage collector and no reflection requirement |
| Swift SIL with OSSA | Ownership SSA | Inherited through KIR |
| Metal AIR, DXIL | GPU IR derived from LLVM | TIR adopts Metal-like restrictions and D3D12-style bindless resources, with GPU pointers everywhere |
| KIR | The direct ancestor | TIR is KIR made portable, versioned, GPU-capable, analyzable and open |

---

## 4. System overview

### 4.1 Roles

| Role | Runs where | Trust |
| --- | --- | --- |
| Producer | Developer machine, build service, or on device (dynamic TIR, section 28.7) | Untrusted |
| `tirc` | Developer machine, pipeline, device (finalizer and JIT subset) | Trusted |
| Pipeline | Tessera build infrastructure | Trusted |
| Device runtime | Every Tessera device | Trusted |
| Loader | Tarwin (Tarwin.md); LibTessera's loader on its other kernel backends; on platform targets, the platform's loader (section 32.10) | Trusted |

### 4.2 Flow

```
source ─▶ producer ─▶ TIR modules ─▶ tirc package ─▶ .tirpkg (developer-signed)
                                                        │
                              pipeline: intake ▶ verify ▶ analyze ▶ optimize ▶ lower
                                                        │
                    ┌───────────────────────────────────┼──────────────────────┐
             native images                       TVISA images            symbol bundles
       (per CPU target, GPU binaries            (CPU and GPU, for          (kept by the
         per GPU family)                        unknown hardware)           pipeline)
                    └─────────────── delivery unit (pipeline-signed) ──────────┘
                                                        │
        device: install ▶ native image for this CPU and GPU? ▶ load
                                     │ no
                                     ▼
                         finalize TVISA ▶ code cache ▶ load ▶ replace when native arrives
        runtime-engine functions and dynamic TIR ▶ verifier ▶ interpreter ▶ JIT tiers
```

### 4.3 Artifacts

| Artifact | Extension | Producer | Consumer | Signed by |
| --- | --- | --- | --- | --- |
| Textual TIR | `.tir` | Producers, `tirc dis` | Tests, humans, `tirc asm` | Not signed |
| Binary TIR module | `.tirb` | Producers, `tirc asm` | `tirc`, device runtime | Inside a package |
| Package | `.tirpkg` | `tirc package` | Pipeline, `tirc` | Developer |
| TVISA image | `.tvisa` | `tirc` | Finalizer | Pipeline |
| Native image | Platform image format | `tirc` | Loader | Pipeline |
| Profile data | `.tirprof` | Instrumented runs | `tirc` | Inside a package |
| Symbol bundle | `.tirsym` | `tirc` | Symbolicator, LLDB | Pipeline |
| Delivery unit | `.tirdu` | Pipeline | Installer | Pipeline |
| Platform image | Mach-O, PE32+, ELF64 or Wasm | `tirc` | The platform's loader | The developer, with the platform's identity (section 32.10) |

### 4.4 Trust boundaries

1. Producers are untrusted. `tirc` re-verifies every module it reads: encoding, types, ownership, profile rules, capabilities and GPU restrictions. Nothing a producer claims is accepted without a check, except where this document names a claim as unverifiable and records it in the analysis report.
2. The developer signature authenticates who submitted a package. It does not make the package trusted.
3. The pipeline signature authenticates delivered images. A device executes: native and TVISA images signed by the pipeline; images signed with a development certificate on a device in developer mode (section 30.12); and TIR that the device runtime has verified (runtime-engine functions shipped in a delivery unit, and dynamic TIR created by an entitled process).
4. The device runtime and the loader are trusted system components. The JIT and the finalizer run out of process (section 28.2), so an application process never holds memory that is both writable and executable.

---

## 5. Machine model and the tir64 target

### 5.1 Fixed properties

Every TIR program, on every architecture and every GPU, sees these properties. Producers MAY rely on them.

| Property | Value |
| --- | --- |
| Byte | 8 bits |
| Endianness | Little-endian in every address space |
| Pointer width | 64 bits in every address space |
| Integers | Two's complement. Signedness is a property of operations, not of types |
| Floating point | IEEE 754-2019. Round to nearest, ties to even, unless an instruction states otherwise |
| Null | The address 0. No object is ever placed at address 0 |
| Maximum object size | 2^62 bytes on CPUs; GPU limits are reported by Ember |
| Atomicity | Naturally aligned accesses of 1, 2, 4, 8 and 16 bytes have atomic forms on every CPU target; 16-byte atomics on GPUs are capability `atomic128` |

### 5.2 The tir64 target

Producers that compile languages with target-dependent front ends (C, C++, Rust, Zig) MUST compile for the neutral target `tir64-unknown-tessera`. Under this target no architecture is known; every architecture-dependent choice is made either by the fixed properties above, by variant sets (section 20), or by runtime feature queries.

LLVM data layout of the target (in the Tessera LLVM fork):

```
e-m:e-p:64:64-i64:64-i128:128-f128:128-n32:64-S128
```

C data model:

| C type | Size (bytes) | Alignment | Notes |
| --- | --- | --- | --- |
| `char` | 1 | 1 | Unsigned |
| `signed char`, `unsigned char` | 1 | 1 | |
| `short` | 2 | 2 | |
| `int` | 4 | 4 | |
| `long`, `long long` | 8 | 8 | LP64 |
| pointers, `size_t`, `ptrdiff_t`, `intptr_t` | 8 | 8 | `size_t` is `unsigned long` |
| `wchar_t` | 4 | 4 | Unsigned |
| `char16_t`, `char32_t` | 2, 4 | 2, 4 | |
| `_Float16` | 2 | 2 | |
| `__bf16` | 2 | 2 | |
| `float`, `double` | 4, 8 | 4, 8 | |
| `long double` | 16 | 16 | IEEE binary128 |
| `__int128` | 16 | 16 | |
| `_BitInt(N)` | Section 8.2 | Section 8.2 | 1 ≤ N ≤ 16384 |
| `bool` | 1 | 1 | |
| `max_align_t` | 16 | 16 | |

Predefined macros under `tir64-unknown-tessera`: `__tessera__` = 1, `__tir64__` = 1, `__TIR_VERSION__` = YY × 10000 + M × 100 + W of the emitted TIR version (261101 for 26.11.1), `__LP64__` = 1, `__CHAR_UNSIGNED__` = 1, `__BYTE_ORDER__` = `__ORDER_LITTLE_ENDIAN__`, `__LITTLE_ENDIAN__` = 1, `__STDC_IEC_559__` = 1, `__FLT_EVAL_METHOD__` = 0, the `__SIZEOF_*__` family matching the table, and `__SIZEOF_LONG_DOUBLE__` = 16. The target MUST NOT define `__x86_64__`, `__aarch64__`, `__arm__`, `__riscv`, `__linux__`, `__unix__`, `__APPLE__`, `_WIN32` or any other architecture or operating-system macro.

Language toolchains:

| Toolchain | Target name | Requirements |
| --- | --- | --- |
| Clang (Tessera fork) | `tir64-unknown-tessera` | The macros above. Default calling convention `ridlcc` for externally visible functions. Variadic functions follow KLF.md section 29.5. The attribute `__attribute__((tir_variant("aarch64"\|"x64"\|"rv64", features...)))` marks a variant body; inline assembly and `<arm_neon.h>`, `<immintrin.h>`, `<riscv_vector.h>` intrinsics are accepted only inside variant bodies of the matching architecture |
| rustc (Tessera fork) | `tir64-unknown-tessera` | `target_arch = "tir64"`, `target_os = "tessera"`, `target_pointer_width = "64"`, `target_endian = "little"`, no `target_family`. The attribute `#[tir::variant(arch = "aarch64", features = [...])]` marks a variant body and `#[tir::variants(aarch64 = path, x64 = path, rv64 = path)]` declares a variant set. `core::arch::{aarch64, x86_64, riscv64}` items are usable only inside variant bodies of the matching architecture. The standard library is the Tessera standard library built on LibTessera |
| Zig | `tir64-tessera` | The same rules expressed through Zig's target model; arch-specific code through `@tirVariant` |
| KLF frontends | — | Emit TIR directly (section 25.2); no target triple is involved |

Each platform target has its own toolchain target with the platform's operating-system macros and C data model (section 32.4).

### 5.3 CPU architectures and baselines

TIR is lowered to exactly three architectures. TVISA.md lists the exact features of every CPU target; this table fixes the baselines.

| Architecture | Final triple | Baseline (mandatory) | Used when present |
| --- | --- | --- | --- |
| aarch64 | `aarch64-unknown-tessera` | Armv8.5-A with FEAT_PAuth, FEAT_BTI, FEAT_LSE, FEAT_FP16, FEAT_DotProd, Advanced SIMD | FEAT_MTE2 and FEAT_MTE3, FEAT_GCS, SVE2, SME2, FEAT_LSE128, Armv9.x features |
| x64 | `x86_64-unknown-tessera` | x86-64-v3 (AVX2, BMI1, BMI2, FMA, F16C, MOVBE) with CET shadow stacks and indirect branch tracking, SMEP, SMAP | x86-64-v4 (AVX-512), AVX10, AMX, APX |
| rv64 | `riscv64-unknown-tessera` | RVA23U64 (including V) with Zicfiss and Zicfilp | Zvfh, Zvbb, Zvkn, Zvbc, later profile extensions |

A device whose CPU lacks a mandatory baseline feature is not a Tessera device. Platform targets (section 32) lower to these architectures with the platform baselines of TVISA.md section 32.2, and to WebAssembly (`wasm`) for the `web` and `wasi` platforms.

### 5.4 Runtime machine facts

Page size, cache line size, core topology, vector length and optional features are not constants of TIR. They are obtained with `target.pagesize` and `target.has` (section 12.17) or through LibTessera's `System.info`. `tirc` folds them when the CPU target fixes them (aarch64 Tessera uses only 16 KiB pages; x64 and rv64 use 4 KiB pages) and otherwise lowers them to load-time constants.

### 5.5 Floating point

1. CPU floating point is strict IEEE 754-2019. Subnormals are preserved. NaN payloads are not specified, but `fneg`, `fabs` and `fcopysign` operate on the sign bit only and preserve payloads.
2. An instruction MAY carry an explicit rounding mode `[rm=rne|rtz|rup|rdn|rna]` and the flag `[strict]`, which makes floating-point exceptions observable through `fp.flags` regions (section 12.3). Without these, the rounding mode is round-to-nearest-even and exception flags are not observable.
3. Fast-math flags are per instruction (section 10.4).
4. GPU floating point follows section 17.16.

---

## 6. Modules, profiles and engines

### 6.1 Module contents

| Component | Content |
| --- | --- |
| Header | Module name, TIR version, profile, platform (section 32.3), default semantic mode, producer record |
| Producer record | Producer name and version; the source languages of the module from the registry in 6.2; for the driver profile, the source-reproducibility record (section 31.2) |
| Imports | RIDL modules with the ABI UUID required of each (KLF.md section 24.3), package-internal modules, and platform libraries (section 32.6) |
| Types | Nominal type declarations with layout and policy |
| Globals | Global and thread-local variables with initializers |
| Functions | Declarations and definitions |
| Variant sets | Per-architecture bodies (section 20) |
| GPU entries | Entry declarations and bodies (section 17) |
| Generic records | Traits, conformances, vtables, witness tables |
| RIDL records | Export records, `@Abi` stamps, the canonical `.ridl` hash |
| Capability declaration | Computed and embedded by `tirc package` (section 21.1) |
| Analysis | Unmanaged manifest, sandbox regions, indirect-call type table (section 21) |
| Metadata | Alias scopes, branch weights, loop hints, value ranges |
| Debug | Section 33 |
| Profile | Optional profile-guided optimization data (section 26.5) |

A module's name is a RIDL identifier path `Name { "." Name }` (KLF.md section 24.1). Names are unique within a package. A module that exports anything is a RIDL module and its exported surface is described by a `.ridl` file emitted by `tirc ridl emit`.

### 6.2 Source-language registry

| Tag | Language | Producer |
| --- | --- | --- |
| `kira` | Kira | KLF |
| `kavascript` | JavaScript and TypeScript through KavaScript | KLF |
| `ksl` | Kira Shading Language | KLF |
| `karc-cpp` | C++ in KArc's C++ mode | KLF |
| `karc-arc` | C++ in KArc's ARC mode | KLF |
| `c` | C | KLF or Clang |
| `cpp` | C++ through Clang | Clang via the LLVM converter |
| `rust` | Rust | rustc via the LLVM converter |
| `zig` | Zig | Zig via the LLVM converter |
| `khlsl` | HLSL through KHLSL | KLF |
| `kmsl` | Metal Shading Language through KMSL | KLF |
| `wasm64` | WebAssembly with 64-bit memories | Wasm64 converter |
| `spirv` | SPIR-V | SPIR-V converter |
| `llvm` | Any other LLVM-based language | LLVM converter |

New tags are added by TIR versions. A producer MUST record every language that contributed code to the module.

### 6.3 Profiles

| Rule | `app` | `system` | `driver` | `kernel` |
| --- | --- | --- | --- | --- |
| Who | Third-party applications, their frameworks, extensions and tools | Tessera frameworks, services, shells | DeviceKit drivers | Tarwin |
| Entitlements | Public entitlements | All entitlements | DeviceKit entitlements only | Not applicable |
| Import private system RIDL modules | No | Yes | DeviceKit modules only | No RIDL imports from user space |
| LibTessera imports | In `tessera` modules | Yes | Yes | No |
| `runtime` engine | Yes | Yes | No | No |
| Dynamic TIR (section 28.7) | With `tessera.tir.dynamic` | With `tessera.tir.dynamic` | No | No |
| GPU entries | Yes | Yes | Yes | No |
| Variant sets, `asm` | Yes, all-three rule (section 20.3) | Yes, all-three rule | Yes, all-three rule | Yes, all-three rule; privileged instructions allowed |
| Unwinding | Yes | Yes | Yes | No |
| `mmio.*`, `fence.io` | No | No | Yes | Yes |
| `kernel.*` instructions | No | No | No | Yes |
| Floating point and vectors | Yes | Yes | Yes | Only inside functions marked `kernel.fp` (section 31.1) |
| Delivery | On Tessera, native and TVISA images and runtime TIR; on other platforms, platform images (section 32.10) | Native and TVISA images, runtime TIR | Native and TVISA images | Native kernel images only |
| Source languages | Any | Any | `kira`, `rust`, `zig`, `c`, `cpp`, `karc-cpp`, `karc-arc` | Any producer of `kira` or `rust` |
| License | Any | GPL-3.0 with the Tessera linking exception | `GPL-3.0-only` or `GPL-3.0-or-later` | GPL-3.0 with the Tessera linking exception |
| Platforms | Any (section 32.2) | Any; `app` rules outside Tessera (section 32.3) | `tessera` | `tessera` |
| TIR window | 26 trains | 26 trains | 52 trains | Current train only |

A package has one profile. Every module of a package has its package's profile.

### 6.4 Linkage

| Linkage | Visibility | Symbol |
| --- | --- | --- |
| `private` | This module | None after lowering |
| `module` | This module, across functions | Module-local |
| `package` | Every module of the package | TIR internal symbol (section 22.9) |
| `exported` | Every module of every package, through RIDL | RIDL symbol (KLF.md section 24.2) |

Exported functions use the RIDL convention and RIDL-representable types (rule V12). Package-linkage functions may use any ABI, including `abi(c)`, because both sides are compiled by the same `tirc` invocation.

### 6.5 Engines

Every CPU function has an engine attribute.

| Engine | Meaning |
| --- | --- |
| `native` | Compiled ahead of time into native and TVISA images |
| `runtime` | Shipped as TIR in the delivery unit and executed by the device runtime's interpreter and JIT tiers (section 28) |
| `any` | `tirc` decides. The default decision is `native`; `tirc build --engine-default runtime` changes it |

Rules:

1. A function that contains `asm`, `target.intrinsic` or a device I/O instruction, or that is a variant body, is forced `native`.
2. The driver and kernel profiles allow `native` only.
3. GPU code has no engine attribute; it is always compiled by a GPU backend.
4. Calls between engines are ordinary calls in one address space. Values are never copied or marshaled. `tirc` generates, into the native image, one bridge per distinct signature crossing from native to runtime code (it calls `tir_rt_invoke`) and one thunk per distinct signature crossing from runtime to native code (registered with the runtime at load). `try.call`, unwinding, coroutine suspension and `await` propagate across bridges and thunks.
5. Native code that calls a `runtime` function calls it through the function's ordinary symbol. Taking its address yields the bridge.
6. On platform targets, `runtime` functions execute on the interpreter and JIT tiers of the runtime bundled with the application, where the platform offers them (section 32.8); on platforms without a JIT they run on the interpreter.

---

## 7. Type system

### 7.1 Scalar types

| Type | Syntax | Notes |
| --- | --- | --- |
| Integers | `i1 i8 i16 i32 i64 i128` and `iN` for 1 ≤ N ≤ 16384 | Two's complement. `iN` with N not in {1, 8, 16, 32, 64, 128} is legalized by `tirc` |
| IEEE floats | `f16 bf16 f32 f64 f128` | `f128` is IEEE binary128 |
| Narrow floats | `f8e4m3 f8e5m2 f6e2m3 f6e3m2 f4e2m1 e8m0` | OCP formats. Storage, conversion, vector and cooperative-matrix element types only. `f6*` and `f4e2m1` exist only as elements of vectors, arrays and cooperative matrices, where they are packed |
| Unit | `unit` | The type of nothing. Size 0 |
| Never | `never` | The result type of a call that does not return |

### 7.2 Pointer and address types

| Type | Syntax | Notes |
| --- | --- | --- |
| Raw pointer | `ptr` | A 64-bit address in the `generic` address space. CPU code only |
| GPU pointer | `ptr<device>`, `ptr<constant>`, `ptr<threadgroup>`, `ptr<thread>`, `ptr<object_payload>`, `ptr<ray_payload>`, `ptr<hit_attribute>`, `ptr<tile>`, `ptr<node_record>` | GPU code only (section 17.3). `ptr<device>` and `ptr<constant>` values may cross between CPU and GPU as 64-bit integers |
| Address | `*T`, `*T<space>` | Address of storage holding a `T`. An SSA operand type only; never stored |

`ptr` is trivial. Converting between `ptr` and `ptr<device>` is `bitcast` and is permitted only in CPU code (section 17.3).

### 7.3 Vector types

| Type | Syntax | Notes |
| --- | --- | --- |
| Fixed vector | `vec<N x T>` | N ∈ {2, 3, 4, 8, 16, 32, 64}. T is an integer type up to `i64`, `i1`, a float type, a narrow float type, or `ptr` |
| Scalable vector | `svec<T>` | `vscale × (128 / bits(T))` lanes. Section 16.2 |
| Masks | `vec<N x i1>`, `svec<i1>` | |

### 7.4 Aggregate types

| Type | Syntax | Notes |
| --- | --- | --- |
| Tuple | `(T, U, ...)` | Laid out as an anonymous struct under the module's default layout rule |
| Array | `[N x T]` | N < 2^48 |
| Nominal | `%Name` or `%Name<T, ...>` | Declared with a layout (section 8) and a policy record (section 9.2). Kinds: `struct`, `enum`, `explicit` |

### 7.5 Reference types

| Type | Syntax | Meaning |
| --- | --- | --- |
| Counted reference | `ref T` | Strong reference to a counted heap object |
| Unique reference | `uniq T` | Sole owner of a heap object |
| Weak reference | `weak T` | Weak reference to a counted heap object; loading yields `%Optional<ref T>` |

### 7.6 Function, closure, coroutine and frame types

```
fn([context] [conv] T, ...) -> R [fails E] [async] [abi(ridl | c)] [foreign]
```

- `conv` is `owned`, `borrow` or `borrowmut`, omitted for trivial types. `context` marks the single pointer parameter passed in the RIDL context register.
- `fails E`: the function has an owned error result delivered on the error edge of `try.call`.
- `async`: calling the function does not run it; it returns a frame (section 13.4).
- `abi(ridl)` is the default. `abi(c)` is the platform C calling convention of the final architecture: AAPCS64, System V x86-64 or RISC-V LP64D on Tessera, and the conventions of TVISA.md section 32.4 on platform targets. `abi(c)` functions MUST NOT have `exported` linkage.
- `[foreign]` (only with `abi(c)`): a value of this type points to platform code, or to a TIR function made callable by platform code (section 32.6). Calls through it do not check TIR type tags.

| Type | Syntax | Meaning |
| --- | --- | --- |
| Function | `fn(...) -> R` | A code pointer with a static signature |
| Closure | `closure (A, B) -> R` | RIDL closure: `code` and `ctx` words (KLF.md section 21.8), owned |
| Borrowed closure | `borrow closure (A, B) -> R` | Non-escaping closure |
| Coroutine handle | `coro<Y, U, R>` | Owned handle to a suspended coroutine that yields `Y`, is resumed with `U` and returns `R` (section 13.2) |
| Frame | `frame @f` | The frame of the coroutine or async function `@f`. Address-only; its size is known after lowering (`coro.frame.size`) |

### 7.7 Generic parameters

`$T` names a generic parameter of the enclosing function or type. A parameter has bounds (traits) and, for associated types, the bounds of the trait that declares them. Generic values are handled through descriptors and witness tables exactly as RIDL defines (KLF.md section 23). Section 18 defines specialization.

### 7.8 GPU types

| Type | Syntax | Meaning |
| --- | --- | --- |
| Texture | `texture<dim, T, access [, ms]>` | `dim` ∈ {`1d`, `2d`, `3d`, `cube`, `1darray`, `2darray`, `cubearray`}; `T` the component type of reads (`f32`, `f16`, `i32`, `u32` written `i32` with unsigned operations, `vec<4 x ...>`); `access` ∈ {`sample`, `read`, `write`, `readwrite`}; `ms` marks multisampled |
| Sampler | `sampler` | |
| Acceleration structure | `accel` | Top-level acceleration structure |
| Visible function table | `vft<fn(...) -> R>` | Table of GPU functions callable indirectly |
| Tensor | `tensor<rank x T>` | Shape and strides read from the handle; rank ∈ {1, 2, 3, 4, 8} |
| Ray query | `rayquery` | Register-only |
| Hit object | `hitobject` | Register-only |
| Cooperative matrix | `coop<scope, M x N, T, use>` | `scope` ∈ {`subgroup`, `threadgroup`}; `use` ∈ {`a`, `b`, `acc`}. Register-only |
| Entry reference | `entry<kind, root R, immediate I>` | A typed 64-bit reference to a GPU entry (section 17.17) |

Texture, sampler, acceleration structure, visible function table and tensor values are resource handles: 8-byte opaque values that can be stored in `device` and `constant` memory.

### 7.9 Type identity and type identifiers

Two non-nominal types are identical when their structure is identical. Two nominal types are identical when their declaring module, name and generic arguments are identical.

Every type has a canonical string: its textual form (section 23) with every nominal name fully qualified by its module path and every generic argument substituted. Its `typeid` is the first 8 bytes, read little-endian, of the BLAKE3-256 hash of the canonical string. Type identifiers are used for control-flow integrity (section 21.3) and exception matching (section 14.3).

### 7.10 Loadable and address-only types

A type is loadable if its layout is known statically and its relocate policy is `bitwise`. Otherwise it is address-only: values of it exist only in memory and are manipulated through `*T`. Generic parameter types, `svec<T>`, `frame @f` and nominal types with `relocate: fn` are address-only. Register-only GPU types are loadable but have no memory representation outside `thread` storage.

---

## 8. Data layout

### 8.1 Layout rules

| Rule | Applies to | Definition |
| --- | --- | --- |
| `ridl` | Default for every nominal type and tuple | KLF.md section 21 |
| `c` | Types produced from C, C++, Zig and Rust `repr(C)` | Section 8.4 |
| `packed(n)` | Explicit packing, n ∈ {1, 2, 4, 8} | Fields in order; alignment of each field min(natural, n) |
| `explicit` | Producer-computed layouts (C++ classes with virtual bases, SPIR-V explicit offsets) | Size, alignment and field offsets supplied; `tirc` verifies that fields do not overlap except in unions and that every offset is a multiple of the field's alignment unless the type is packed. It applies only to field-bearing `struct` and `explicit` (union) declarations; `enum` declarations use `ridl`, `c` or `packed(n)` because this version has no per-variant payload-offset syntax |

### 8.2 Scalars

| Type | Size | Alignment |
| --- | --- | --- |
| `i1` | 1 (value 0 or 1) | 1 |
| `i8`, `f8e4m3`, `f8e5m2`, `e8m0` | 1 | 1 |
| `i16`, `f16`, `bf16` | 2 | 2 |
| `i32`, `f32` | 4 | 4 |
| `i64`, `f64`, `ptr`, every `ptr<space>`, resource handles, entry references | 8 | 8 |
| `i128`, `f128` | 16 | 16 |
| `iN` | The smallest power of two ≥ ⌈N/8⌉ bytes | min(size, 16) |

Packed narrow floats: in an array, vector or cooperative matrix, `f4e2m1` occupies 4 bits (two per byte, lower index in the low nibble) and `f6e2m3`, `f6e3m2` occupy 6 bits (four per three bytes, lower index in the low bits). An array of packed elements has alignment 1 and size ⌈count × bits / 8⌉.

### 8.3 Vectors

For `vec<N x T>` with T of size s: alignment = min(16, s × p) where p is the smallest power of two ≥ N; size = N × s rounded up to the alignment. `vec<3 x f32>` therefore has size 16 and alignment 16; producers that need a 12-byte layout use `[3 x f32]`. `vec<N x i1>` is stored as N bytes. `svec<T>` has no static size and is address-only; it cannot be a field of an aggregate.

### 8.4 The tir64 C layout rule

1. Non-bit-field members are placed in declaration order, each at the next offset that is a multiple of its alignment. The aggregate's alignment is the maximum member alignment (at least 1). Its size is the end of the last member rounded up to its alignment.
2. A bit-field of declared type `T` with width w is allocated in a storage unit of `sizeof(T)` bytes, from the least significant bit upward, in declaration order. If the field would cross a boundary of a storage unit of its declared type, it starts at the next such boundary.
3. A zero-width bit-field moves the next bit-field to the next boundary of its declared type.
4. Unnamed bit-fields do not affect the aggregate's alignment. Named bit-fields contribute the alignment of their declared type.
5. Unions have the size of their largest member rounded up to the maximum member alignment.
6. A flexible array member has size 0 and contributes its element alignment.
7. `__attribute__((packed))` is `packed(1)`; `__attribute__((aligned(n)))` raises alignment to n.
8. C enumerations have type `int` unless an enumerator does not fit, in which case they have the smallest of `unsigned int`, `long`, `unsigned long` that fits all enumerators; C23 fixed underlying types are honored.

### 8.5 GPU memory

GPU buffers use the same layout rules as CPU memory. There is no std140, std430 or other GPU-specific layout. A struct declared once has the same layout on the CPU and on every GPU, which is what makes single-source sharing (section 17.17) exact. Converters from SPIR-V express explicit offsets with `explicit` layouts.

### 8.6 Register-only types

`rayquery`, `hitobject` and `coop<...>` have no layout. Values of these types may exist as SSA values and in `stack.alloc` slots of the `thread` address space. They MUST NOT be stored in any other address space, captured, or passed across a `call` boundary except to `module`-linkage GPU functions that `tirc` inlines.

---

## 9. Ownership model

TIR's ownership model is KIR's (KLF.md section 11), restated here completely because KLF.md sections 10 to 17 are superseded.

### 9.1 Value kinds

| Kind | Obligation |
| --- | --- |
| trivial | None. May be used any number of times |
| owned | Consumed exactly once on every path from its definition to a function exit |
| borrowed | Produced by `borrow.begin`, `load.borrow` or a `borrow` parameter. Never consumed. Every use is dominated by the begin and post-dominated by the matching end. A borrowed value MUST NOT be stored, returned, captured by an escaping closure, or live across a coroutine suspension unless the coroutine's frame is itself borrowed for the same scope (section 13.3) |
| weak | An owned value of type `weak T` |

A type whose copy and destroy policies are both `trivial` produces trivial values. Every other type produces owned values, or borrowed values inside borrow scopes.

### 9.2 Policy record

Every nominal type carries a policy record.

| Field | Values | Meaning |
| --- | --- | --- |
| `copy` | `trivial`, `implicit(@f)`, `explicit(@f)`, `none` | How a second owner comes to exist. `implicit`: a producer's ownership pass may insert copies. `explicit`: only a source-level copy. `none`: never |
| `destroy` | `trivial`, `fn(@f)` | What runs when an owned value dies |
| `relocate` | `bitwise`, `fn(@f)` | How a value moves between addresses. `fn` makes the type address-only |
| `exclusivity` | `static`, `dynamic`, `none` | Default enforcement of `access.begin [modify]` on storage of this type |
| `pairing` | `counted`, `none` | `counted` promises copy and destroy are a retain and release pair on shared state; the optimizer may cancel and reorder pairs (section 26.4) |
| `lifetime` | `eager`, `scoped` | `scoped`: destroys happen at the end of the lexical scope recorded by the producer (C++ destructors, Kira `Drop`). Destroy timing of `scoped` types is observable and MUST NOT change under optimization |

Policy functions have the signatures `copy(dst: ptr, src: ptr)`, `relocate(dst: ptr, src: ptr)`, `destroy(obj: ptr)`; for generic types each takes the type's descriptor as a final `ptr` argument.

Default policies, which producers MAY request with `policy default` and `tirc` derives from the declared memory model of the type:

| Memory model | Aggregates | Pointer-like |
| --- | --- | --- |
| `unmanaged` | Copy trivial, destroy trivial | `ptr`, trivial |
| `counted` | Memberwise: implicit copy if any member is non-trivial | `ref T`: copy implicit(retain), destroy release, pairing counted |
| `affine` | Memberwise: copy `none` unless the type is marked copyable, then `explicit` | `uniq T`: copy none, destroy fn |

### 9.3 Conventions

| Convention | Caller | Callee |
| --- | --- | --- |
| `owned` | Gives up the value | Must consume it |
| `borrow` | Keeps ownership; guarantees the value outlives the call | Receives a borrowed value |
| `borrowmut` | Holds exclusive access to the storage for the call | Receives `*T` with modify access |

Results are owned or trivial.

### 9.4 Borrow scopes and exclusivity

`borrow.begin %v` opens a scope in which `%v` must not be consumed; `borrow.end` closes it. Scopes are properly nested and closed on every path.

`access.begin [read | modify] [static | dynamic | unchecked] %addr` brackets an access. Under `static`, `tirc` proves no two overlapping accesses to the same storage conflict (two modifies, or a modify and a read). Under `dynamic`, the runtime checks through `tir_rt_access_begin` and traps with `TIR-RT-0003`. Under `unchecked`, nothing is enforced and the function is unmanaged.

### 9.5 Unmanaged escape

`into.raw`, `from.raw`, `raw.load`, `raw.store`, `ptr.offset`, `ptr.diff`, `ptr.toaddr`, `addr.toptr`, `inttoptr`, `heap.alloc.raw`, `heap.realloc.raw`, `heap.free.raw`, `mem.copy`, `mem.move`, `mem.set` on `ptr` operands, `access.begin [unchecked]`, `call.foreign`, `call.variadic`, `va.*`, `setjmp`, `longjmp`, `asm`, `asm.goto`, `target.intrinsic`, `mmio.*` and `kernel.*` are the unmanaged instructions.

In GPU code, pointer arithmetic and accesses through GPU pointers are not unmanaged instructions: GPU code is confined to its process's GPU address space (section 17.4), so a GPU function is managed unless it uses `inttoptr`.

### 9.6 Canonical form

Only canonical TIR is shipped. In canonical TIR:

1. Every copy is an explicit `copy`, `load [copy]` or `copy.addr`.
2. Every owned value that dies is explicitly consumed: by a consuming use or by `destroy`, `destroy.addr`, `store [assign]` or `load [take]` paths.
3. Address-only values exist only in memory and are manipulated with the `.addr` forms.
4. Rules V4 to V8 hold.

Producers either emit canonical TIR or emit raw TIR and run `tirc canonicalize`, which applies KLF.md section 11.6's five mandatory passes (definite initialization, copy and move resolution, borrow checking, destroy insertion, address-only lowering) and reports through the diagnostic keys listed there. `tirc` rejects raw TIR inside a package (`TIR-OWN-0001`).

### 9.7 Trust classes

Every function is in exactly one trust class, computed by `tirc` and recorded in the analysis section.

| Class | Condition |
| --- | --- |
| managed | No unmanaged instruction, and the producer record states that ownership information is supplied (every KLF producer of managed languages) |
| sandboxed | Produced by the Wasm64 converter; every memory access goes through the module's declared sandbox regions (section 25.4) |
| unmanaged | Every other function. All LLVM-converter output is unmanaged |

A module is memory-safe when every function is managed or sandboxed and every import resolves to a RIDL module whose exported functions are all managed or sandboxed, LibTessera and the TIR runtime excepted.

### 9.8 Verifier rules

| Rule | Statement |
| --- | --- |
| V1 | SSA: every value is defined once and dominates its uses. Blocks appear in a reverse post-order of the control-flow graph. Block arguments match parameters in count and type |
| V2 | Every instruction is well typed per sections 12, 16 and 17 |
| V3 | Every block ends in exactly one terminator |
| V4 | Every owned value is consumed exactly once on every path to `return`, `fail`, `resume`, `tail.call`, `deopt`, `coro.yield`'s destroy path, or `unreachable`. Paths ending in `trap` are exempt |
| V5 | No borrowed value is an operand of a consuming instruction, of `store`, of `return`, or of an escaping capture |
| V6 | Every `borrow.begin` and `load.borrow` has exactly one matching `borrow.end` on every path, properly nested |
| V7 | `static` accesses do not conflict |
| V8 | Address-only types never appear as loadable SSA values |
| V9 | A function with engine `runtime` or `any` contains no `asm`, `target.intrinsic` or device I/O instruction |
| V10 | `try.call` is used if and only if the callee type has `fails`; `invoke` is used only in functions without `nounwind` |
| V11 | Generic parameters appear only in generic functions and types; every operation needing a size or witness has the descriptor available |
| V12 | Exported functions use only RIDL-representable types (KLF.md section 20, extended by section 19.2) |
| V13 | Every instruction and attribute is permitted by the module's profile (6.3) |
| V14 | Defined functions contain no permissive-only form (section 10.2) |
| V15 | GPU code obeys section 17.15 |
| V16 | Functions that require structured control flow satisfy rules S1 to S7 (section 11.3) |
| V17 | Coroutine and async functions obey section 13.7 |
| V18 | Variant sets obey section 20.2 |
| V19 | Every indirect call site's function type equals the type of every function whose address can reach it through a value of that type; `bitcast` between distinct function types is forbidden, and so is `bitcast` between a function type and `ptr`, except for `[foreign]` function types (section 32.6) |
| V20 | The analysis section matches what `tirc` computes (section 21.6) |
| V21 | A module's imports, instructions and function types agree with its platform (section 32.3) |

---

## 10. Semantic modes: defined and permissive

### 10.1 Modes

Every function has a semantic mode, written `semantics(defined)` or `semantics(permissive)`; a module declares a default. The module header flag `defined-only` is set when every SIGNATURES record's effective mode (its override or the MODULE default) is `defined`.

- A defined function has no undefined behavior: every instruction produces a defined result or a defined trap.
- A permissive function may contain the forms of 10.2, each of which has undefined behavior under a stated condition. Undefined behavior in a permissive function permits `tirc` to assume the condition never occurs.

Producers MUST emit defined functions whenever their source language has no undefined behavior for the code in question. KLF frontends for Kira, KavaScript and KSL emit only defined functions. The LLVM converter emits defined functions when the function contains none of the forms below, and permissive functions otherwise.

### 10.2 Permissive-only forms

| Form | Undefined when |
| --- | --- |
| Arithmetic mode `[poison]` on `add`, `sub`, `mul`, `shl`, `neg` | Signed (`s`) or unsigned (`u`) overflow occurs: the result is poison |
| `[exact]` on `sdiv`, `udiv`, `lshr`, `ashr` | A nonzero remainder or shifted-out bit: poison |
| `[poison]` on `fptosi`, `fptoui` | The value is out of range or NaN: poison |
| `[inbounds]` on `ptr.offset` | The result leaves the allocated object: poison |
| Fast-math flags `nnan`, `ninf` | An operand or result is NaN or infinite: poison |
| `const.poison` | Always poison |
| `assume %c` | `%c` is false |
| Alias metadata `!noalias`, `!alias.scope`, `!tbaa` | The assertion is violated |
| Parameter and result attributes `nonnull`, `align(n)`, `dereferenceable(n)`, `noundef`, `range(lo, hi)` | The attribute is violated |
| `unreachable [ub]` | Executed |
| `!range`, `!nonnull`, `!align` on loads | The loaded value violates them |
| Non-atomic data race on `raw.load` or `raw.store` | Section 15.4 |

### 10.3 Poison

1. Poison propagates through every instruction whose result depends on a poison operand, except `select` (only the chosen operand matters), `freeze`, and the logical forms stated in section 12.
2. Branching on poison, using poison as an address, as a divisor, as a shift amount in `[trap]` mode, or passing it to a `noundef` parameter is undefined behavior.
3. Memory never holds poison. Storing a poison value stores an unspecified but fixed value of the type.
4. `freeze %v` returns `%v` if it is not poison and an unspecified but fixed value of the type otherwise.
5. TIR has no `undef`. Converters replace `undef` with `const.zero`.

### 10.4 Fast-math flags

Floating-point instructions accept the flags `nnan`, `ninf`, `nsz`, `arcp`, `contract`, `afn`, `reassoc`, and `fast` (all of them). `nnan` and `ninf` are permissive-only. The others are allowed in defined functions: they permit results that differ from strict evaluation within the bounds of section 12.3 and section 17.16, they never produce undefined behavior, and instructions carrying them are exempt from bitwise tier parity (section 34.3).

### 10.5 Uninitialized storage

1. Definite initialization (V4 to V8 through canonicalization) makes reads of uninitialized storage impossible in managed functions.
2. `tirc` MUST zero-initialize every `stack.alloc` slot whose full initialization before every read it cannot prove.
3. `heap.alloc.raw` returns zeroed memory unless it carries `[uninit]`, which is permissive-only. The runtime allocator MAY zero memory when it is freed instead of when it is allocated.
4. Reading uninitialized memory in an unmanaged function yields an unspecified but fixed value, never data from another allocation.

---

## 11. Control flow

### 11.1 Blocks

A function body is a list of basic blocks in a reverse post-order of the control-flow graph. A block has parameters and ends in exactly one terminator. There are no phi nodes: values flow along edges as block arguments. The first block's parameters are the function's parameters.

### 11.2 Edges

Critical edges are allowed. A terminator's successor list may name the same block more than once with different arguments.

### 11.3 Structured control flow

A selection header is a block terminated by `cond.br` or `switch.int` carrying `merge bbM`. A loop header is a block declared `bbH: loop(merge bbM, continue bbC)`. A construct is the set of blocks dominated by its header and not dominated by its merge.

| Rule | Statement |
| --- | --- |
| S1 | A loop header dominates its continue block. The continue block's terminator contains the only back edge to the header |
| S2 | Every edge leaving a loop's construct targets the loop's merge block, the merge or continue block of an enclosing loop, or a terminator that exits the function (`return`, `fail`, `trap`, `unreachable`, `frag.demote`-then-`return`) |
| S3 | Every edge leaving a selection construct targets its merge block, or an exit permitted by S2 for an enclosing loop |
| S4 | Constructs are properly nested: if two constructs share a block, one contains the other |
| S5 | A block is the merge block of at most one construct |
| S6 | No edge enters a construct except through its header |
| S7 | `br.indirect`, `invoke`, `landingpad`, `resume`, `throw`, `setjmp` and `longjmp` do not appear |

Structured control flow is REQUIRED in GPU code and in every function marked `structured`. On CPUs, merge and loop annotations are optional hints; `tirc` verifies them when present. Under S1 to S6 control flow is reducible. GPU code relies on these rules for reconvergence (section 17.8).

### 11.4 Irreducible control flow

CPU code may contain irreducible control flow. GPU code may not.

### 11.5 Guaranteed tail calls

`tail.call @f(args)` is a terminator that transfers control to `@f`, releasing the caller's frame, and returns `@f`'s result to the caller's caller. The callee's ABI and result type MUST equal the caller's; argument stack space MUST NOT exceed the caller's incoming argument space for `abi(c)`. Under `abi(ridl)` the convention guarantees tail calls for every signature. Wasm tail calls, Rust explicit tail calls and KavaScript proper tail calls lower to `tail.call`.

### 11.6 Indirect branches

`br.indirect %addr, [bb...]` lists every possible target. It is CPU only. `block.addr` values flow only to `br.indirect` of the same function.

---

## 12. Instruction set

Notation: `T` a type, `%v` a value, `@f` a symbol, `bbN` a block. Unless stated, results are trivial for trivial types and owned otherwise. Every instruction may carry `!loc(file:line:col)` and metadata (section 22.5). Integer operands of an instruction have the same type unless stated. Vector forms of scalar instructions are listed in section 16.

The order in which instructions appear in the tables of this section, and in sections 13.8, 14.5, 15.6, 16.3 and 17, is normative: binary opcodes are assigned in that order within each range (section 22.6).

### 12.1 Constants (range 0x000)

| Instruction | Result | Semantics |
| --- | --- | --- |
| `const.int T, n` | `T` | Integer n, written signed or unsigned, in range of T |
| `const.float T, x` | `T` | x as a hexadecimal float, an exactly representable decimal, `inf`, `-inf`, or `nan(payload)` |
| `const.bool b` | `i1` | |
| `const.null [space]` | `ptr` or `ptr<space>` | Address 0 |
| `const.zero T` | `T` | Zero value of any trivial type, including vectors, resource handles (the null handle) and entry references |
| `const.poison T` | `T` | Permissive only |
| `const.string "text"` | `(ptr, i64)` | Address and byte length of immutable UTF-8 data in the image |
| `const.bytes T, b"..."` | `T` | A trivial `T` built from the given little-endian bytes |
| `const.vec T, [c, ...]` | `T` | Vector constant |
| `const.aggregate T(c, ...)` | `T` | Trivial aggregate constant; for an `explicit` union, the first operand is the zero-based member index and the second is its value |
| `func.ref @f [<args>]` | function type | Function value, with generic arguments if `@f` is generic |
| `global.addr @g` | `ptr` | |
| `tls.addr @g` | `ptr` | Address of the calling thread's instance |
| `descriptor.ref T` | `ptr` | RIDL type descriptor of `T`; `T` may be a generic parameter |
| `witness.ref T, Trait` | `ptr` | Witness table of `T`'s conformance to `Trait` |
| `block.addr bbN` | `ptr` | For `br.indirect` |
| `gpu.entry.ref @e` | `entry<...>` | Section 17.17 |
| `const.sampler { field: value, ... }` | `sampler` | Inline sampler, GPU code only (section 17.6) |

For `const.bytes T, b"..."`, unescaped Unicode scalar values are encoded as
UTF-8. The escapes `\n`, `\r`, `\t`, `\0`, `\\` and `\"` produce
their corresponding character bytes; `\xNN` emits the single byte given by
two hexadecimal digits, and `\u{N...}` emits the UTF-8 encoding of the named
Unicode scalar value. Unknown, malformed, or incomplete escapes are invalid.

### 12.2 Integer operations (range 0x020)

Modes: `[wrap]`; `[trap s|u]` traps with `TIR-RT-0002` on signed or unsigned overflow; `[sat s|u]` saturates; `[poison s|u]` (permissive only) produces poison.

| Instruction | Result | Semantics |
| --- | --- | --- |
| `add [mode] %a, %b` | `T` | |
| `sub [mode] %a, %b` | `T` | |
| `mul [mode] %a, %b` | `T` | |
| `add.ovf s\|u %a, %b` | `(T, i1)` | Wrapped result and overflow flag |
| `sub.ovf s\|u %a, %b` | `(T, i1)` | |
| `mul.ovf s\|u %a, %b` | `(T, i1)` | |
| `mulh s\|u %a, %b` | `T` | High half of the double-width product |
| `sdiv [trap\|wrap] [exact] %a, %b` | `T` | Truncating. Division by zero traps with `TIR-RT-0010` in every mode. MIN / -1 traps under `[trap]` (default) and yields MIN under `[wrap]` |
| `udiv [exact] %a, %b` | `T` | Division by zero traps with `TIR-RT-0010` |
| `srem [trap\|wrap] %a, %b` | `T` | Sign of the dividend. MIN rem -1 is 0 under `[wrap]`, traps under `[trap]` |
| `urem %a, %b` | `T` | |
| `and %a, %b` | `T` | |
| `or [disjoint] %a, %b` | `T` | `[disjoint]` (permissive) asserts no common set bit |
| `xor %a, %b` | `T` | |
| `not %a` | `T` | |
| `neg [wrap\|trap\|poison] %a` | `T` | |
| `abs [wrap\|trap\|poison] %a` | `T` | |
| `shl [mode] %a, %n` | `T` | Under `[wrap]` the amount is reduced modulo the width; under `[trap s\|u]` an amount ≥ width or an overflow traps; `[poison]` is permissive |
| `lshr [wrap\|trap] [exact] %a, %n` | `T` | |
| `ashr [wrap\|trap] [exact] %a, %n` | `T` | |
| `rotl %a, %n` | `T` | Amount modulo width |
| `rotr %a, %n` | `T` | |
| `fshl %a, %b, %n` | `T` | Funnel shift left of the concatenation `a:b`, amount modulo width |
| `fshr %a, %b, %n` | `T` | |
| `smin %a, %b` | `T` | |
| `smax %a, %b` | `T` | |
| `umin %a, %b` | `T` | |
| `umax %a, %b` | `T` | |
| `icmp pred %a, %b` | `i1` | `eq ne slt sle sgt sge ult ule ugt uge`. Also accepts `ptr` operands (unsigned order) |
| `select %c, %a, %b` | `T` | `%c` is `i1`, or a mask of the same lane count as a vector `T` |
| `ctlz [poison] %a` | `T` | Width when `%a` is 0, unless `[poison]` |
| `cttz [poison] %a` | `T` | |
| `ctpop %a` | `T` | |
| `bswap %a` | `T` | Width a multiple of 16 |
| `bitreverse %a` | `T` | |
| `clmul %a, %b` | `T` | Low half of the carry-less product; `T` is `i32` or `i64` |
| `clmulh %a, %b` | `T` | High half |
| `freeze %a` | `T` | Section 10.3 |

### 12.3 Floating point and mathematics (range 0x060)

Every instruction accepts fast-math flags (section 10.4), `[rm=...]` and `[strict]` (section 5.5) where meaningful.

| Instruction | Result | Semantics |
| --- | --- | --- |
| `fadd %a, %b` | `T` | |
| `fsub %a, %b` | `T` | |
| `fmul %a, %b` | `T` | |
| `fdiv %a, %b` | `T` | |
| `frem %a, %b` | `T` | IEEE `fmod` semantics (result has the dividend's sign) |
| `fneg %a` | `T` | Sign bit only |
| `fabs %a` | `T` | Sign bit only |
| `fsqrt %a` | `T` | Correctly rounded |
| `fma %a, %b, %c` | `T` | Fused, single rounding |
| `fmuladd %a, %b, %c` | `T` | Fused on every CPU target (all baselines have FMA); on GPUs per section 17.16 |
| `fmin %a, %b` | `T` | IEEE 754-2019 `minimum` |
| `fmax %a, %b` | `T` | IEEE 754-2019 `maximum` |
| `fminnum %a, %b` | `T` | IEEE 754-2019 `minimumNumber` |
| `fmaxnum %a, %b` | `T` | IEEE 754-2019 `maximumNumber` |
| `fcopysign %a, %b` | `T` | |
| `ffloor %a` | `T` | |
| `fceil %a` | `T` | |
| `ftrunc %a` | `T` | |
| `fround %a` | `T` | Ties away from zero |
| `froundeven %a` | `T` | Ties to even |
| `fcmp pred %a, %b` | `i1` | `oeq one olt ole ogt oge ord uno ueq une ult ule ugt uge true false` |
| `fclass %a` | `i32` | Bit mask: 0 sNaN, 1 qNaN, 2 −∞, 3 −normal, 4 −subnormal, 5 −0, 6 +0, 7 +subnormal, 8 +normal, 9 +∞ |
| `frexp %a` | `(T, i32)` | |
| `ldexp %a, %e` | `T` | `%e` is `i32` |
| `fp.flags.read` | `i32` | Bits 0 invalid, 1 divide-by-zero, 2 overflow, 3 underflow, 4 inexact, accumulated by `[strict]` instructions of this thread since the last clear |
| `fp.flags.clear` | `unit` | |
| `math.sin %a` | `T` | |
| `math.cos %a` | `T` | |
| `math.tan %a` | `T` | |
| `math.asin %a` | `T` | |
| `math.acos %a` | `T` | |
| `math.atan %a` | `T` | |
| `math.atan2 %y, %x` | `T` | |
| `math.sinh %a` | `T` | |
| `math.cosh %a` | `T` | |
| `math.tanh %a` | `T` | |
| `math.asinh %a` | `T` | |
| `math.acosh %a` | `T` | |
| `math.atanh %a` | `T` | |
| `math.exp %a` | `T` | |
| `math.exp2 %a` | `T` | |
| `math.exp10 %a` | `T` | |
| `math.expm1 %a` | `T` | |
| `math.log %a` | `T` | |
| `math.log2 %a` | `T` | |
| `math.log10 %a` | `T` | |
| `math.log1p %a` | `T` | |
| `math.pow %x, %y` | `T` | Special cases per IEEE 754-2019 section 9.2.1 |
| `math.cbrt %a` | `T` | |
| `math.hypot %a, %b` | `T` | |
| `math.rsqrt %a` | `T` | |

CPU accuracy of `math.*`:

| Type | Default | With `[approx]` |
| --- | --- | --- |
| `f32`, `f64` | Correctly rounded (≤ 0.5 ULP) in every rounding mode named by `[rm]` | ≤ 4 ULP, IEEE special values exact |
| `f16`, `bf16` | ≤ 1 ULP | ≤ 2 ULP |
| `f128` | ≤ 1 ULP | ≤ 4 ULP |

Domain errors yield NaN; no global state (there is no `errno`). `[approx]` does not create undefined behavior and is allowed in defined functions; instructions carrying it are exempt from bitwise tier parity. GPU accuracy is section 17.16.

### 12.4 Conversions (range 0x0A0)

| Instruction | Semantics |
| --- | --- |
| `trunc %v to T` | Integer narrowing |
| `zext %v to T` | |
| `sext %v to T` | |
| `fptrunc %v to T [rm]` | Default round to nearest even |
| `fpext %v to T` | Exact |
| `sitofp %v to T [rm]` | |
| `uitofp %v to T [rm]` | |
| `fptosi [sat\|trap\|poison] %v to T` | Mode required. `[sat]`: NaN → 0, saturating; `[trap]`: out of range or NaN traps with `TIR-RT-0011`; `[poison]` permissive |
| `fptoui [sat\|trap\|poison] %v to T` | |
| `ptrtoint %v to i64` | |
| `inttoptr %v to ptr \| ptr<device> \| ptr<constant>` | Unmanaged |
| `bitcast %v to T` | Same size, both trivial, neither a function type (V19) |
| `fcvt %v to T [rm] [sat\|satfinite\|nonsat]` | Any float type to any float type, including narrow floats. The omitted mode is `sat`. `sat` replaces NaN with positive zero and clamps the input to [0, 1] before rounding to `T`. `satfinite` rounds to `T`, clamps infinities and finite overflow to the largest finite magnitude of `T` while preserving the sign when `T` supports one, and preserves NaN when `T` encodes it; otherwise NaN becomes the largest finite value. `nonsat` performs the ordinary rounded conversion using `T`'s defined overflow and special-value encodings |

### 12.5 Vectors (range 0x0C0)

Section 16.3.

### 12.6 Aggregates (range 0x100)

| Instruction | Semantics |
| --- | --- |
| `struct.new %T(fields)` | Builds a loadable struct; consumes owned fields |
| `struct.extract %v, i` | Trivial field of any struct, or field of a borrowed struct (yields borrowed) |
| `struct.insert %v, i, %f` | Trivial struct with field i replaced |
| `struct.destructure %v` | Consumes, yields all fields |
| `tuple.new (values)` | |
| `tuple.extract %v, i` | As `struct.extract` |
| `tuple.destructure %v` | |
| `array.new [N x T](values)` | |
| `array.extract %v, %i` | Trivial element; index out of range traps with `TIR-RT-0005` |
| `array.insert %v, %i, %e` | Trivial array |
| `enum.new %E, variant(payload)` | |
| `enum.tag %v` | Variant index as the enum's tag type |
| `enum.take %v, variant` | Consumes, yields payload; precondition tag = variant, else traps with `TIR-RT-0001` |
| `enum.project %b, variant` | Borrowed payload of a borrowed enum; same precondition |

### 12.7 Storage and addresses (range 0x120)

| Instruction | Semantics |
| --- | --- |
| `stack.alloc T [<space>] [count %n]` | Yields `*T` in the selected address space. If `<space>` is omitted, CPU and `target(cpu, gpu)` functions use generic space and GPU-only functions use `thread`. `count %n` reserves `%n` contiguous values of `T`; omitted count means one. Dynamic size for `$T` and `svec<T>` comes from the descriptor or `vscale`. GPU and kernel counts must be statically known, nonnegative, and less than 2^62; a binary static count is a `const.int` result. Section 10.5 governs initialization |
| `stack.dealloc %a` | LIFO order |
| `field.addr %a, i` | |
| `elem.addr %a, %index` | Index out of range of a fixed array traps with `TIR-RT-0005` unless `[unchecked]` (unmanaged) |
| `enum.payload.addr %a, variant` | |
| `enum.inject %a, variant` | Writes the tag after the payload was initialized |
| `addr.cast %a to *U` | Layout-compatible types only |
| `addr.toptr %a` | Yields `ptr` (or `ptr<space>`); unmanaged on CPUs |
| `ptr.toaddr %p as *T` | Yields an address view `*T` (or `*T<space>`) of a pointer; unmanaged on CPUs. In GPU code this is how typed storage behind a GPU pointer is reached |

`field.addr`, `elem.addr` and `enum.payload.addr` preserve the input address's
address space and return an address to the selected field, element or payload.
An enum variant's payload type is `unit` for no fields, the field type for one
field, and a tuple of its fields in declaration order for multiple fields.

### 12.8 Memory (range 0x140)

| Instruction | Semantics |
| --- | --- |
| `load [trivial\|copy\|take] %a` | `take` leaves the storage uninitialized |
| `load.borrow %a` | Borrows the stored value in place; ended by `borrow.end` |
| `store [trivial\|init\|assign] %v to %a` | `assign` destroys the old value first |
| `copy.addr %src to [init\|assign] %dst` | Through the type's policy or descriptor |
| `move.addr %src to [init\|assign] %dst` | Relocates; source becomes uninitialized |
| `destroy.addr %a` | |
| `raw.load T, %p [align n] [volatile] [nontemporal]` | Unmanaged access to trivial `T`. Misaligned access is defined when `align` states the true alignment; `align` greater than the true alignment is permissive-only |
| `raw.store %v, %p [align n] [volatile] [nontemporal]` | |
| `ptr.offset [inbounds] %p, %bytes` | Byte arithmetic; wraps modulo 2^64 without `[inbounds]` |
| `ptr.diff %p, %q` | `i64` byte difference |
| `mem.copy %dst, %src, %n [volatile] [nooverlap]` | Overlapping ranges behave as `mem.move`; `[nooverlap]` (permissive) asserts disjointness |
| `mem.move %dst, %src, %n [volatile]` | |
| `mem.set %dst, %byte, %n [volatile]` | |
| `prefetch %p [read\|write] [locality 0..3] [data\|instruction]` | No semantic effect |

### 12.9 Heap and references (range 0x160)

The heap is the TIR runtime allocator (section 28.10): spans, size classes, slot metadata with strong and weak counts, counting by address, poison and quarantine on raw deletion (the semantics of KLF.md sections 18.1 to 18.6, carried over unchanged). In the kernel profile, these instructions lower to the kernel allocator named by Tarwin.md.

| Instruction | Semantics |
| --- | --- |
| `heap.alloc.counted T` | Owned `ref T`, strong count 1, uninitialized payload |
| `heap.alloc.unique T` | Owned `uniq T` |
| `ref.addr %r` | Payload address inside a borrow of the reference |
| `uniq.addr %u` | |
| `heap.alloc.raw %size, %align [uninit]` | Unmanaged; zeroed unless `[uninit]` (permissive) |
| `heap.realloc.raw %p, %size, %align` | Unmanaged; contents preserved up to the smaller size; new bytes zeroed |
| `heap.free.raw %p` | Unmanaged; freeing a slot whose strong count is already zero traps with `TIR-RT-0006` |
| `weak.new %r` | Owned `weak T` from a borrowed `ref T` |
| `weak.load %w` | `%Optional<ref T>` |
| `ref.unowned %r` | `ptr` without ownership |
| `ref.isunique %r` | `i1`, for copy-on-write |

### 12.10 Ownership (range 0x180)

`copy %v`, `move %v`, `destroy %v`, `borrow.begin %v`, `borrow.end %b`, `into.raw %v`, `from.raw T, %p`, `access.begin [read|modify] [static|dynamic|unchecked] %a`, `access.end %acc`, in that order. Semantics: section 9.

### 12.11 Calls (range 0x1A0)

| Instruction | Semantics |
| --- | --- |
| `call @f<args>(args)` | Direct call. The callee's ABI comes from its declaration |
| `call.indirect %fn : fnT (args)` | Call through a function value of the stated type. Control-flow integrity checked (section 21.3); for `[foreign]` types, section 32.6 applies |
| `closure.new %code, %ctx` | Builds an owned closure from a code pointer and an owned context box (or null) |
| `call.closure %c(args)` | Context travels in the context register |
| `closure.callback i, fnT` | Yields an `abi(c)` function whose parameter i is a user-data pointer to a closure record; calling it calls the closure |
| `call.witness T, Trait.method(args)` | Through T's witness table; resolved statically when T is concrete |
| `call.vtable %obj, @Vtable, slot(args)` | Calls the selected slot through the named declared vtable; `%obj` is passed as the first argument, followed by `args`; control-flow integrity checked |
| `try.call callee(args) normal bbN, error bbE` | Terminator. For `fails` callees. `bbN` receives the result and `bbE` the owned error; a `unit` or `never` value contributes no block parameter |
| `call.foreign @sym(args)` | Platform C convention to a symbol declared by a `.klffi` import (KLF.md section 26) or by a platform import (section 32.6). System, driver and kernel profiles, and platform modules |
| `call.variadic callee(fixed; variadic)` | C variadics: RIDL mode per KLF.md section 29.5, platform mode per the architecture's C ABI |
| `invoke callee(args) to bbN unwind bbU` | Terminator. A call that may unwind (section 14). `bbN` receives the result, with no parameter for `unit` or `never`; `bbU` has no parameters and begins with `landingpad` |

Direct generic callees of `call` and `tail.call` supply one explicit type
argument per generic parameter, in declaration order. Direct generic callees
of `try.call` and `invoke` infer type arguments by structurally matching
parameter types to the actual arguments. Every generic parameter must be
inferred, and repeated occurrences must agree; otherwise the call is invalid.

Call attributes: `[cold]`, `[nounwind]`, `[noreturn]`, `[inline]`, `[noinline]`, `[builtin]`.

`call.vtable` uses the table named by `@Vtable` directly; it does not load a
table pointer from `%obj`. The selected slot's function type includes `%obj` as
its first parameter, followed by the explicit arguments. The verifier checks
that signature and the indirect-call type, but TIR does not encode or prove a
table-to-object identity relation; the producer is responsible for selecting a
table valid for `%obj`.

### 12.12 Coroutines and async (range 0x1C0)

Section 13.8.

### 12.13 Unwinding (range 0x1D0)

Section 14.5.

### 12.14 Terminators (range 0x1E0)

| Instruction | Semantics |
| --- | --- |
| `br bbN(args)` | |
| `cond.br %c, bbT(args), bbF(args) [merge bbM]` | |
| `switch.int %v, [n: bbN(args)]..., default bbD(args) [merge bbM]` | |
| `switch.enum %v, [variant: bbN]..., [default bbD]` | Consumes an owned enum; each target receives the payload as block parameters. The default is mandatory unless all variants are listed |
| `br.indirect %addr, [bb...]` | CPU only |
| `return %v` | |
| `fail %e` | Error edge of a `fails` function |
| `tail.call callee(args)` | Section 11.5 |
| `unreachable [trap\|ub]` | `[trap]` (default) traps with `TIR-RT-0001`; `[ub]` is permissive |
| `trap code` | Traps with the given 16-bit code (section 34.6) |

### 12.15 Atomics (range 0x200)

Section 15.6.

### 12.16 Variadics and non-local jumps (range 0x210)

| Instruction | Semantics |
| --- | --- |
| `va.start %list` | |
| `va.arg T, %list` | |
| `va.copy %dst, %src` | |
| `va.end %list` | |
| `setjmp %buf` | Yields `i32`; the function MUST be `returns_twice` |
| `longjmp %buf, %v` | Jumping over a frame that holds owned values with non-trivial destroy is a producer-level error for managed languages and is permitted for C |

All are unmanaged and permissive-only in combination with optimization assumptions as stated in C.

### 12.17 Target queries and architecture-specific instructions (range 0x220)

| Instruction | Semantics |
| --- | --- |
| `asm "template", "constraints"(args) -> T [sideeffect] [memory] [pure] [privileged]` | Inline assembly. Only in a variant body of the template's architecture (section 20). Constraint syntax is LLVM's for that architecture. `[privileged]` is allowed only in the kernel profile |
| `asm.goto "template", "constraints"(args) normal bbN, labels [bb...]` | Terminator. Inline assembly that may jump to the listed blocks |
| `target.intrinsic "name"(args) -> T` | A target intrinsic from the registry of TVISA.md for the variant body's architecture (NEON, SVE2, SME2, MTE, SSE through AVX-512 and AMX, RVV and the scalar extensions) |
| `target.has "feature"` | `i1`: whether the executing CPU has an optional feature from TVISA.md's feature list. Folded when the CPU target decides it, otherwise a load-time constant |
| `target.pagesize` | `i64` page size of the executing system |

### 12.18 Portable intrinsics (range 0x230)

`intrinsic <id>(args) -> T` is the only instruction of the range. Each backend lowers an intrinsic to hardware where present and to an equivalent instruction sequence otherwise; results are identical either way.

| Id | Name | Signature | Semantics |
| --- | --- | --- | --- |
| 0 | `crc32c` | `(i32, iK) -> i32`, K ∈ {8, 16, 32, 64} | CRC-32C (Castagnoli) accumulate, reflected |
| 1 | `crc32` | `(i32, iK) -> i32` | CRC-32 (IEEE 802.3) accumulate, reflected |
| 2 | `aes.enc` | `(vec<16 x i8>, vec<16 x i8>) -> vec<16 x i8>` | ShiftRows, SubBytes, MixColumns, then XOR round key |
| 3 | `aes.enclast` | same | ShiftRows, SubBytes, XOR round key |
| 4 | `aes.dec` | same | InvShiftRows, InvSubBytes, InvMixColumns, XOR round key |
| 5 | `aes.declast` | same | InvShiftRows, InvSubBytes, XOR round key |
| 6 | `aes.imc` | `(vec<16 x i8>) -> vec<16 x i8>` | InvMixColumns |
| 7 | `sha1.block` | `(*[5 x i32], ptr) -> unit` | Compresses one 64-byte block into the state |
| 8 | `sha256.block` | `(*[8 x i32], ptr) -> unit` | |
| 9 | `sha512.block` | `(*[8 x i64], ptr) -> unit` | Compresses one 128-byte block |
| 10 | `keccak.f1600` | `(*[25 x i64]) -> unit` | Keccak-f[1600] permutation |
| 11 | `spin.hint` | `() -> unit` | Spin-loop hint |
| 12 | `counter.read` | `() -> i64` | Constant-rate monotonic counter |
| 13 | `counter.frequency` | `() -> i64` | Ticks per second of `counter.read` |
| 14 | `debug.break` | `() -> unit` | Breakpoint; a no-op unless a debugger is attached on a development or debug image |
| 15 | `ct.select` | `(i1, T, T) -> T` | Constant-time select: lowered without branches and never converted into one |
| 16 | `ct.eq` | `(ptr, ptr, i64) -> i1` | Constant-time comparison of two byte ranges |

### 12.19 Debug and hints (range 0x240)

| Instruction | Semantics |
| --- | --- |
| `debug.value %v, !var(name, type, scope)` | |
| `debug.addr %a, !var(...)` | |
| `debug.label !label(name, scope)` | |
| `assume %c` | Permissive only |
| `expect %v, n [probability p]` | Branch-weight hint |
| `nop` | |
| `prof.count id` | Increments profile counter `id` in instrumented builds; removed otherwise |

Textual debug operands use `debug.value %v, !var("name", type, "scope")`,
`debug.addr %a, !var("name", type, "scope")` and
`debug.label !label("name", "scope")`. Names and scopes are nonempty strings
with Kira string escapes; `type` is a TIR type. The scope string identifies a
source scope and is resolved to a DEBUG scope record during lowering. Each
debug instruction requires its matching attachment. A trailing `!loc(...)`
may follow the debug attachment.

### 12.20 Generic support (range 0x250)

`type.size T`, `type.align T`, `type.stride T` yield `i64` and read the descriptor when `T` is generic.

### 12.21 Speculation (range 0x260)

| Instruction | Semantics |
| --- | --- |
| `guard %c, deopt @fallback(state...)` | If `%c` is true, continue. Otherwise transfer to `@fallback` with the listed state as arguments (owned values are moved) and return its result, exactly as `tail.call @fallback(state...)` would |
| `deopt @fallback(state...)` | Terminator: unconditional transfer as above |

The semantics are those of an ordinary conditional tail call, so every tier executes them identically. The guards exist so the optimizing JIT and ahead-of-time tiers can compile the guarded path speculatively and reconstruct the fallback's arguments from metadata when a guard fails (section 28.5). KavaScript emits them for type-specialized code.

### 12.22 Device I/O and kernel (range 0x270)

| Instruction | Profiles | Semantics |
| --- | --- | --- |
| `mmio.load T, %p [order relaxed\|acquire]` | driver, kernel | Single access of exactly `sizeof(T)` bytes (1, 2, 4 or 8) to device memory; never merged, split, duplicated or elided |
| `mmio.store %v, %p [order relaxed\|release]` | driver, kernel | |
| `fence.io [read\|write\|full]` | driver, kernel | Orders device-memory accesses with normal-memory accesses and DMA visibility |
| `kernel.percpu.addr @g` | kernel | Address of the executing CPU's instance of a per-CPU variable |

### 12.23 Platform interoperation (range 0x280)

Only in platform modules (section 32.3).

| Instruction | Semantics |
| --- | --- |
| `foreign.addr @import` | A `[foreign]` function value for an imported platform function; null when a weak import is absent |
| `foreign.data @import` | `ptr` to an imported platform data object; null when a weak import is absent |
| `foreign.entry @f` | A `[foreign]` function value through which platform code may call the `abi(c)` function `@f` (section 32.6 item 6) |

---

## 13. Coroutines and async

### 13.1 Overview

TIR has native coroutines. Two shapes are built on them:

1. Coroutines in the general sense (generators, resumable functions), with explicit yield and resume.
2. Async functions, whose frames are poll-based futures that cross RIDL boundaries as `Any Future<R>` (section 19.3). The model is executor-agnostic: Seyoul, KavaScript's event loop and any other runtime drive futures by polling them.

`tirc` splits coroutines into state machines with liveness-based frame layout. The interpreter executes them directly. Both produce identical behavior.

### 13.2 Coroutine functions

A coroutine function carries the attribute `coro(yield: Y, resume: U)` and has a result type `R`.

- `coro.start @f(args) [heap | inline %slot]` creates a coroutine instance and yields an owned `coro<Y, U, R>`. The body does not run until the first resume. `[heap]` (default) allocates the frame from the runtime allocator. `[inline %slot]` places it in caller-provided storage of type `*frame @f`, which MUST outlive the handle.
- Inside the body, `coro.yield %y, resume bbR, destroy bbD` suspends. When resumed with a value `u`, control continues at `bbR(u)`. When destroyed while suspended, control continues at `bbD`, which MUST destroy every live owned value and end in `coro.finish`.
- `return %r` completes the coroutine with `r`.
- `coro.resume %h, %u, yielded bbY, done bbDone` resumes a suspended coroutine through the address of its handle. If the body yields `y`, control continues at `bbY(y)`; if it returns `r`, at `bbDone(r)`. Unwinding out of the body propagates out of `coro.resume` as from an `invoke` when the resuming function is not `nounwind`.
- `coro.destroy %h` consumes the handle. A suspended coroutine runs its destroy path first; a completed one only frees its frame.
- Resuming a completed coroutine traps with `TIR-RT-0022`.

### 13.3 Frames and borrows

1. A borrowed value may be live across a suspension point only if it is a `borrow` parameter of the coroutine function. Such a handle is borrow-dependent: it MUST be destroyed before the caller's matching `borrow.end` of every argument it borrowed (rule V17).
2. `access.begin [static]` scopes MUST NOT span a suspension point.
3. `coro.frame.size @f` and `coro.frame.align @f` yield the frame's size and alignment as `i64`. They are constants after lowering and are used to size inline frames.

### 13.4 Async functions

A function whose type is `async` is an async function.

1. Calling an async function with `call` does not run it. The result is an owned value of type `frame @f`, initialized with the arguments. `frame @f` conforms to the RIDL trait `Async.Future<R>` (section 19.3), whose `poll` method `tirc` generates.
2. `await %f` is allowed only in async functions. `%f` is an owned future: a concrete frame, a value of a generic type bounded by `Future<T>`, or an `Any Future<T>`. `await` consumes `%f` and repeats: poll `%f` with the current waker; on `Ready(v)`, destroy `%f` and produce `v`; on `Pending`, suspend the enclosing async function so that its own `poll` returns `Pending`, and poll `%f` again when the enclosing function is polled again.
3. `async.waker` yields the borrowed waker of the current poll, for code that implements leaf futures inline.
4. Destroying a pending frame runs the cleanups of the live values at its current suspension point, in reverse order of initialization. Destroy timing of `scoped` types is preserved. This is cancellation.
5. Async functions cannot use `coro.yield`. Async streams are library types built on futures.
6. An async function exported through RIDL is exported as a function returning `Any Future<R>` (section 19.3); `tirc` boxes the frame.

### 13.5 Profiles and engines

Coroutines and async functions are allowed in every profile except in GPU code. In the kernel profile, frames MUST be `inline` or allocated from the kernel allocator. Coroutines may be `native` or `runtime`; resuming across engines goes through bridges (section 6.5).

### 13.6 Lowering

`tirc` lowers every coroutine to a frame type plus a resume function with a state index (switched resume). Frame layout reuses slots for values whose live ranges do not overlap across suspension points. A coroutine that is started and completed within one function and does not escape SHOULD be inlined and its frame placed on the stack.

### 13.7 Rules (V17)

1. `coro.yield` appears only in coroutine functions; `await` and `async.waker` only in async functions.
2. Section 13.3 holds.
3. `setjmp`, `longjmp`, `va.*` and `br.indirect` do not appear in coroutine or async functions.
4. A destroy path ends in `coro.finish` and consumes every owned value live at its suspension point.

### 13.8 Instructions (range 0x1C0)

| Instruction | Semantics |
| --- | --- |
| `coro.start @f(args) [heap\|inline %slot]` | Section 13.2 |
| `coro.yield %y, resume bbR, destroy bbD` | Terminator |
| `coro.resume %h, %u, yielded bbY, done bbDone` | Terminator |
| `coro.finish` | Terminator ending a destroy path |
| `coro.destroy %h` | |
| `coro.done %h` | `i1` |
| `coro.frame.size @f` | `i64` |
| `coro.frame.align @f` | `i64` |
| `await %f` | Section 13.4 |
| `async.waker` | Borrowed `%Async.Waker` |

---

## 14. Unwinding

### 14.1 Model

Unwinding is two-phase and table-based. Native code uses the platform's zero-cost tables with the TIR personality `tir_personality`. The interpreter walks its own frames. JIT code registers unwind tables with the runtime when it is installed. All three interleave correctly on one stack.

### 14.2 Exception objects

An exception is an owned RIDL box (KLF.md section 21.6) together with the `typeid` of its payload type. `throw %box as T` starts unwinding.

### 14.3 Landing pads

An unwind block begins with `landingpad` and a list of clauses evaluated in order during the search phase:

| Clause | Matches |
| --- | --- |
| `catch.type T` | An exception whose `typeid` equals `T`'s |
| `catch.filter @pred` | When `@pred(box: ptr, typeid: i64) -> i1` returns true. Filters MUST be `nounwind` and MUST NOT suspend. C++ class hierarchies and foreign exception models match through filters |
| `catch.all` | Every exception |
| `cleanup` | Never matches; the pad runs during the cleanup phase and ends in `resume` |

`landingpad` yields `(ptr, i32)`: the owned box and the index of the matching clause, or −1 for a cleanup.

### 14.4 Rules

1. Unwinding never crosses the frame of a function with `exported` linkage. An exception reaching one traps with `TIR-RT-0020`.
2. Unwinding never crosses a frame entered through `closure.callback` or any `abi(c)` function called from outside the package. It traps with `TIR-RT-0020`.
3. An exception reaching the frame of a `nounwind` function traps with `TIR-RT-0024`.
4. The kernel profile and GPU code never unwind.
5. Unwinding propagates across engine bridges and out of `coro.resume` and `poll`.

### 14.5 Instructions (range 0x1D0)

| Instruction | Semantics |
| --- | --- |
| `landingpad [clauses]` | First instruction of an unwind block |
| `resume %lp` | Terminator: continues unwinding |
| `throw %box as T` | Terminator |
| `exception.payload %box` | Borrowed payload address |
| `exception.typeid %box` | `i64` |

### 14.6 Foreign personalities

The LLVM converter maps personality functions as follows. Any other personality is rejected with `TIR-IMP-0010`.

| LLVM personality | TIR mapping |
| --- | --- |
| `__gxx_personality_v0` | `catch` clauses become `catch.filter` calls into the C++ runtime's type matcher, which ships as TIR in the C++ runtime framework; `cleanup` stays `cleanup` |
| `rust_eh_personality` | `catch.all` and `cleanup`; Rust's runtime decides panics versus foreign exceptions inside the catch block |
| `__gcc_personality_v0` | `cleanup` |

On platform targets the C++ personality also catches foreign exceptions at `invoke [foreign_unwind]` sites, and on Apple platforms the Objective-C personality does too (section 32.6 item 9).

---

## 15. Concurrency and the memory model

### 15.1 Threads

TIR has no thread instructions. Threads are created and joined through LibTessera (`Thread.spawn` and its relatives) or, in the kernel profile, through Tarwin. Thread-local storage is a `thread_local` global reached with `tls.addr`. On platform targets, threads are created through platform interfaces and attached to the runtime (section 32.8).

### 15.2 Memory model

CPU code follows the C++20 memory model (ISO/IEC 14882:2020 [intro.races] and [atomics.order]) with the orderings `relaxed`, `acquire`, `release`, `acq_rel` and `seq_cst`. There is no `consume`; converters map it to `acquire`. GPU code follows the same model extended with scopes.

### 15.3 Scopes

Every atomic instruction and fence carries a scope.

| Scope | Agents synchronized |
| --- | --- |
| `thread` | The executing thread only (a compiler barrier) |
| `subgroup` | Invocations of one GPU subgroup |
| `threadgroup` | Invocations of one GPU threadgroup |
| `device` | Every invocation on one GPU |
| `system` | Every CPU thread and every GPU invocation of the process |

The default scope is `system` on CPUs and `device` on GPUs. Two atomic operations synchronize only if each one's scope includes the other's agent. `system` scope on a GPU requires memory that Ember allocates coherent.

### 15.4 Data races

1. Storage reached only through managed instructions cannot race: exclusivity rules make conflicting accesses impossible or trapping.
2. In a permissive function, a data race involving a non-atomic `raw.load` or `raw.store` is undefined behavior.
3. In a defined function, non-atomic `raw.load` and `raw.store` behave as unordered atomic accesses: a racing read of a naturally aligned value of at most 8 bytes returns a value some write stored, larger or misaligned reads return, for each byte, a value some write stored to that byte, and no value appears out of thin air. `tirc` MUST NOT introduce or duplicate such accesses.

### 15.5 Atomic widths

| Size | aarch64 | x64 | rv64 |
| --- | --- | --- | --- |
| 1, 2, 4, 8 bytes | Lock-free | Lock-free | Lock-free |
| 16 bytes | Lock-free (FEAT_LSE128 or exclusive pairs) | Lock-free (`CMPXCHG16B`) | Lock-free with Zacas; otherwise the runtime's address-hashed lock table, reported by `target.has "atomic128.lockfree"` |

Atomics require natural alignment. A misaligned atomic traps with `TIR-RT-0007`.

### 15.6 Instructions (range 0x200)

| Instruction | Semantics |
| --- | --- |
| `atomic.load [order=O] [scope=S] T, %p` | |
| `atomic.store [order=O] [scope=S] %v, %p` | |
| `atomic.rmw op [order=O] [scope=S] %p, %v` | `op` ∈ `xchg add sub and nand or xor max min umax umin fadd fsub fmax fmin uinc_wrap udec_wrap usub_cond usub_sat`. Yields the old value |
| `atomic.cmpxchg [weak] [success_order=O] [failure_order=O] [scope=S] %p, %expected, %new` | Yields `(T, i1)` |
| `fence [order=O] [scope=S]` | |
| `atomic.wait %p, %expected, %timeout` | `T` is `i32` or `i64`; `%timeout` in nanoseconds, negative for none. Yields `i32`: 0 woken, 1 value differed, 2 timed out. Spurious wakeups are allowed |
| `atomic.notify %p, %count` | Wakes up to `%count` waiters; yields the number woken |

`atomic.wait` and `atomic.notify` lower to LibTessera's `Sync.wait32`, `Sync.wait64` and `Sync.wake` in user space and to Tarwin's primitives in the kernel profile. They are not allowed in GPU code.

`O` is `relaxed`, `acquire`, `release`, `acq_rel` or `seq_cst`. An omitted `order` or `success_order` means `seq_cst`. An omitted `failure_order` means the strongest failure order no stronger than the selected success order: `relaxed` for success `relaxed` or `release`, `acquire` for success `acquire` or `acq_rel`, and `seq_cst` for success `seq_cst`. A failure order may be only `relaxed`, `acquire` or `seq_cst`, and must not be stronger than the success order. Loads reject `release` and `acq_rel`; stores reject `acquire` and `acq_rel`; RMWs and compare-exchanges accept every success order; fences accept all five orders. An omitted scope is `system` on CPUs and `device` on GPUs. CPU code accepts only `thread` and `system`; GPU code accepts `thread`, `subgroup`, `threadgroup`, `device` and `system`. These modifiers use the bracketed `key=value` syntax before the value operands.

The value type `T` of `atomic.load`, `atomic.store`, `atomic.rmw` and `atomic.cmpxchg` has trivial copy and destroy policies and a fixed layout of 1, 2, 4, 8 or 16 bytes. A typed `addr<T>` pointer must name that same type. `atomic.cmpxchg`'s expected and new values both have type `T`. `xchg` accepts any such `T`; the integer RMW operations require a scalar integer `T`, and the floating-point RMW operations require a scalar floating-point `T`. `atomic.wait` points to `i32` or `i64`, its expected value has that same type, and its timeout has type `i64`; `atomic.notify` points to `i32` or `i64` and takes an `i32` count. Both wait and notify return `i32`.

---

## 16. Vectors and SIMD

### 16.1 Fixed vectors

Lane 0 is at the lowest address. The scalar instructions of sections 12.2 to 12.4 accept vector operands and apply lane-wise. A trapping mode traps if any active lane traps, reporting the lowest such lane. Comparisons yield masks. Elementwise instructions accept the suffix `[mask %m, passthru %p]`: inactive lanes take the passthrough value and never trap.

### 16.2 Scalable vectors

`svec<T>` has `vscale × (128 / bits(T))` lanes. `vscale` is a positive power of two chosen when the process starts, constant for the process's lifetime, and identical on every thread and in every tier of the process.

| Architecture | `vscale` |
| --- | --- |
| aarch64 with SVE2 | Vector length / 128 |
| aarch64 without SVE2 | 1 (Advanced SIMD) |
| x64, executable image built for x86-64-v3 | 2 (AVX2) |
| x64, executable image built for x86-64-v4 or later | 4 (AVX-512) |
| rv64 | VLEN / 128 |

The process's `vscale` is fixed before any code runs, from the executable's image; every other image of the process either reads the vector length at run time or was built for the same `vscale` (TVISA.md section 15.2).

`svec` values are address-only, cannot be fields of aggregates, cannot cross RIDL boundaries, and are not allowed in GPU code.

### 16.3 Instructions (range 0x0C0)

| Instruction | Semantics |
| --- | --- |
| `vec.splat %s to T` | Every lane set to `%s` |
| `vec.extract %v, %i` | Index out of range traps with `TIR-RT-0005` |
| `vec.insert %v, %i, %e` | Same |
| `vec.shuffle %a, %b, [indices]` | Fixed vectors only. Constant indices into the concatenation `a:b` |
| `vec.step T` | Lanes 0, 1, 2, ... |
| `vec.vscale` | `i64` |
| `vec.reduce op %v [mask %m]` | `op` ∈ `add mul and or xor smin smax umin umax fadd fmul fmin fmax fminnum fmaxnum`. Floating-point `fadd` and `fmul` reduce in lane order unless the instruction carries `reassoc` |
| `vec.load T, %p [align n] [mask %m, passthru %v]` | |
| `vec.store %v, %p [align n] [mask %m]` | |
| `vec.gather T, %ptrs [mask %m, passthru %v]` | `%ptrs` a vector of pointers, or `%base, %offsets, scale` |
| `vec.scatter %v, %ptrs [mask %m]` | `%ptrs` is a vector of pointers, or the address form is `%base, %offsets, scale`; lanes are stored in lane order and colliding lanes leave the highest lane's value |
| `vec.compress %v, %m, %passthru` | Active lanes packed toward lane 0 |
| `vec.expand %v, %m, %passthru` | Inverse of compress |
| `vec.dot kind %acc, %a, %b` | `kind` ∈ `sdot udot usdot` (`i8` groups of four into `i32` lanes) or `bfdot` (`bf16` pairs into `f32` lanes) |
| `vec.zip %a, %b [lo\|hi]` | Interleave |
| `vec.unzip %a, %b [even\|odd]` | De-interleave |
| `vec.reverse %v` | |
| `vec.slide %a, %b, %n [up\|down]` | Extract a window of the concatenation |
| `vec.concat %a, %b` | Fixed vectors only |
| `vec.split %v [lo\|hi]` | Fixed vectors only |
| `vec.mask.active %i, %n` | Mask with lane k active when `i + k < n` (unsigned) |
| `vec.mask.count %m` | `i64` number of active lanes |
| `vec.mask.first %m` | `i64` index of the first active lane, or the lane count |
| `vec.table %table, %indices` | Byte table lookup; out-of-range indices yield 0 |

### 16.4 Lowering requirements

1. A portable vector operation MUST lower to native SIMD whenever the target has an instruction of the needed width; wider vectors are split, narrower ones widened.
2. `svec` loops MUST lower to SVE2 and RVV with predication, and to masked AVX2 or AVX-512 on x64.
3. Server builds at `-O2` and above run loop and SLP vectorization.

### 16.5 Target intrinsics

Inside variant bodies, `target.intrinsic` reaches the full architecture-specific vector and matrix sets: Advanced SIMD, SVE2 and SME2 on aarch64; SSE through AVX-512, AVX10 and AMX on x64; RVV and its extensions on rv64. `svec<T>` values are passed directly to SVE2 and RVV intrinsics. CPU matrix units (SME2, AMX) are reachable only this way in TIR 1.0.

---

## 17. GPU execution model

### 17.1 Overview

GPU code is TIR. It lives in the same modules as CPU code, uses the same type system and layouts, and is compiled by Tessera-owned GPU backends, one per GPU family (TVISA.md). Drivers contain no shader compiler. The pipeline compiles GPU code for every GPU family in the target matrix; the device finalizes TVISA GPU code for any family it has a backend for (section 17.19). Ember (Ember.md) creates pipelines from GPU entries; nothing in this section depends on Ember's API.

The model is Metal-like in its restrictions and bindless in its resources: buffers are GPU pointers, every other resource is an 8-byte handle stored in memory, and there are no descriptor sets, binding slots or layout objects.

### 17.2 GPU functions and entries

- `func ... target(gpu)` declares a function callable only from GPU code.
- `func ... target(cpu, gpu)` declares a function compiled for both. It MUST satisfy the CPU rules and section 17.15.
- `entry kind @name(params) -> R attributes { body }` declares a GPU entry.

Entry parameters:

| Parameter | Count | Type | Meaning |
| --- | --- | --- | --- |
| `root %r: ptr<constant> %T` or `ptr<device> %T` | 0 or 1 | Pointer to a struct `%T` | The root argument: the entry's bindless argument structure |
| `immediate %i: %U` | 0 or 1 | Trivial struct of at most 256 bytes | Immediate data supplied per dispatch or draw |
| Stage input | 0 or 1 | Per kind (table below) | Interpolated varyings, mesh payload, ray payload, hit attributes, node record |

Builtins are read with `gpu.builtin` (section 17.5), never passed as parameters.

| Kind | Stage input | Result | Attributes |
| --- | --- | --- | --- |
| `compute` | None | `unit` | `threads(x, y, z)` or `threads(dynamic)` |
| `object` | `payload %p: ptr<object_payload> %P` (output) | `unit` | `threads(x, y, z)`, `payload_size(n)` ≤ 16384 |
| `mesh` | `payload %p: ptr<object_payload> %P` (optional) | `unit` | `threads(x, y, z)`, `mesh(max_vertices: V, max_primitives: P, topology: points \| lines \| triangles)`, V ≤ 256, P ≤ 512 |
| `vertex` | None (vertex pulling through GPU pointers) | Stage struct with a `@position` field | |
| `fragment` | Stage struct (varyings) | Stage struct of outputs | `early_fragment_tests` |
| `tile` | None | `unit` | `threads(x, y, 1)`; capability `tile` |
| `raygen` | None | `unit` | |
| `intersection` | None | `unit` | `attributes(%A)` ≤ 32 bytes |
| `anyhit`, `closesthit` | `payload %p: ptr<ray_payload> %P`, `attrs %a: ptr<hit_attribute> %A` | `unit` | |
| `miss` | `payload %p: ptr<ray_payload> %P` | `unit` | |
| `callable` | `data %d: ptr<ray_payload> %D` | `unit` | |
| `node` | `record %r: ptr<node_record> %R` | `unit` | `node(launch: mode, max_dispatch_grid: [x, y, z], input_record_size: n, outputs: [...])`; capability `work_graphs` |

The `node` attribute has exactly one `launch` (`broadcast`, `coalescing` or
`thread`), one positive three-dimensional `max_dispatch_grid`, one
`input_record_size`, and one `outputs` array. Each output is an object with a
nonempty unique `name`, `kind` (`node` or `mesh`), positive `record_size` and
positive `max_records`. Output order assigns zero-based indexes used by
`node.output.allocate`; an index outside the array traps. A `mesh` output
targets a mesh node and requires capability `work_graphs.mesh` in addition to
`work_graphs`.

Stage-struct field annotations: `@position`, `@color(n)` with optional `@index(k)` for dual-source blending, `@depth(any | greater | less)`, `@stencil`, `@sample_mask`, `@clip_distance(n)`, `@cull_distance(n)`, `@point_size`, `@layer`, `@viewport`, `@primitive_id`, `@view_index`, `@cull` (mesh per-primitive), `@per_primitive`, and the interpolation qualifiers `@flat`, `@perspective`, `@noperspective`, `@centroid`, `@sample`, `@invariant`.

### 17.3 Address spaces

| Space | Access | Notes |
| --- | --- | --- |
| `device` | Read and write | Global memory addressed by 64-bit GPU pointers |
| `constant` | Read only for the duration of a dispatch or draw | May be cached uniformly |
| `threadgroup` | Read and write, one threadgroup | Allocated with `stack.alloc T <threadgroup>` in the entry block of an entry; size limit reported per GPU |
| `thread` | Read and write, one invocation | `stack.alloc` in GPU code; static sizes only |
| `object_payload` | Written by an object threadgroup, read by its mesh threadgroups | |
| `ray_payload` | Read and write by the shaders of one ray | |
| `hit_attribute` | Written by intersection, read by any-hit and closest-hit | |
| `tile` | Imageblock memory of one tile | Capability `tile` |
| `node_record` | Work graph records | Capability `work_graphs` |

There is no generic address space in GPU code and no cast between spaces. CPU code may hold `ptr<device>` and `ptr<constant>` values, store them in memory, and convert them to and from `i64` and `ptr` with `bitcast`. Dereferencing such a value on the CPU is meaningful only for allocations Ember reports as mapped at the same address on CPU and GPU (shared virtual addressing).

### 17.4 Resources

Textures, samplers, acceleration structures, visible function tables and tensors are resource handles: 8-byte opaque values. Ember creates them; GPU code loads them from root structures or any other `device` or `constant` memory and uses them directly.

1. Any handle may be used with non-uniform values in any stage. Backends that need uniform descriptors generate the necessary loops.
2. Operations on the null handle (`const.zero`) read zero and discard writes.
3. A GPU access to an unmapped device address, or to memory outside the process's GPU address space, faults the dispatch. Ember reports the fault to the process; it never affects another process (DeviceKit.md).

### 17.5 Builtins

`gpu.builtin name : T`. Reading a builtin outside the stages listed is rejected by the verifier.

| Name | Type | Stages |
| --- | --- | --- |
| `thread_position_in_grid` | `vec<3 x i32>` | compute, object, mesh, tile |
| `thread_position_in_threadgroup` | `vec<3 x i32>` | compute, object, mesh, tile |
| `threadgroup_position_in_grid` | `vec<3 x i32>` | compute, object, mesh, tile |
| `threads_per_threadgroup` | `vec<3 x i32>` | compute, object, mesh, tile |
| `threadgroups_per_grid` | `vec<3 x i32>` | compute, object, mesh |
| `thread_index_in_threadgroup` | `i32` | compute, object, mesh, tile |
| `subgroup_index_in_threadgroup` | `i32` | compute, object, mesh, tile |
| `subgroups_per_threadgroup` | `i32` | compute, object, mesh, tile |
| `lane_index` | `i32` | all |
| `subgroup_size` | `i32` | all |
| `vertex_id`, `instance_id`, `base_vertex`, `base_instance`, `draw_index` | `i32` | vertex |
| `view_index` | `i32` | vertex, mesh, fragment |
| `position` | `vec<4 x f32>` | fragment |
| `front_facing` | `i1` | fragment |
| `sample_index`, `sample_mask_in` | `i32` | fragment |
| `barycentric_perspective`, `barycentric_linear` | `vec<3 x f32>` | fragment |
| `primitive_id`, `layer`, `viewport_index` | `i32` | fragment |
| `point_coord` | `vec<2 x f32>` | fragment |
| `helper_invocation` | `i1` | fragment |
| `shading_rate` | `vec<2 x i32>` | fragment; capability `shading_rate` |

### 17.6 Textures and samplers

The first operand of each texture operation is the texture handle. Sampling, gather, feedback and LOD operations then take a sampler handle and normalized floating-point coordinates; `tex.read`, `tex.write` and `tex.atomic` take integer texel coordinates. Array textures take a separate integer layer immediately after the coordinates. For cube and cube-array textures the coordinates have three components; the offset, when present, has two components for cube faces. Other offset widths equal the spatial coordinate width. Offset components are compile-time integer literals in [−8, 7].

| Instruction | Remaining operands after texture, sampler (when used), coordinates and optional array layer |
| --- | --- |
| `tex.sample` | Optional `offset [x, ...]`, optional `min_lod %m` |
| `tex.sample_level` | `lod %l`; optional offset and `min_lod %m` |
| `tex.sample_grad` | `grad %dx, %dy`; optional offset and `min_lod %m` |
| `tex.sample_bias` | `bias %b`; optional offset and `min_lod %m` |
| `tex.sample_compare` | `compare %reference`; optional offset and `min_lod %m` |
| `tex.sample_compare_level` | `compare %reference, lod %l`; optional offset and `min_lod %m` |
| `tex.gather` | Optional `component n` selector in 0–3 (default 0); optional offset and `min_lod %m` |
| `tex.gather_compare` | Required `f32` `%reference`; optional offset and `min_lod %m` |
| `tex.read` | Sample index for multisampled textures; absent otherwise |
| `tex.write` | Sample index for multisampled textures, then the component value; absent sample index otherwise |
| `tex.atomic` | Operation name, then the scalar value |
| `tex.query` | One of `width`, `height`, `depth`, `size`, `layers`, `levels`, `samples` |
| `tex.lod` | No additional operands |
| `tex.feedback` | Optional offset and `min_lod %m` |

`%l`, `%b`, `%dx`, `%dy`, `%reference` and `%m` are SSA values. `lod`, `bias`, `min_lod`, compare references and both gradients have type `f32`; gradients have the same vector shape as the coordinates. Comparison sampling requires a scalar `f32` texture component. Sampler operations, including `tex.lod` and `tex.feedback`, require a non-multisampled texture; multisampled textures are accessed with `tex.read` and `tex.write`, whose sample index is an integer. `tex.gather` returns four components of the selected channel; `tex.gather_compare` returns four `f32` comparison results. `tex.query` returns `i32` except `size`: a 1D size is `i32`, a 1D-array, 2D or cube size is `vec<2 x i32>`, and a 2D-array, 3D or cube-array size is `vec<3 x i32>`. `samples` returns the texture's sample count, which is one for a non-multisampled texture. `tex.lod` returns `vec<2 x f32>` containing the unclamped estimate and the estimate clamped to the sampler's LOD range. `tex.atomic` returns the old component value and is limited to scalar `i32` or `f32` texture components. Its operation is one of `xchg`, `add`, `sub`, `and`, `nand`, `or`, `xor`, `max`, `min`, `umax`, `umin`, `fadd`, `fsub`, `fmax` or `fmin`; floating-point operations require `f32`, and integer operations require `i32`.

`[sparse]` is a modifier on `tex.sample`, `tex.sample_level`, `tex.sample_grad`, `tex.sample_bias`, `tex.sample_compare`, `tex.sample_compare_level`, `tex.gather`, `tex.gather_compare`, `tex.read` and `tex.feedback`. It adds a final `i1` residency result and requires capability `sparse_residency`. `tex.sample`, `tex.sample_bias`, `tex.sample_compare`, `tex.gather`, `tex.gather_compare`, `tex.lod` and `tex.feedback` use implicit derivatives; they are allowed only in fragment code, and in compute code with capability `compute_derivatives`. `tex.feedback` additionally requires capability `sampler_feedback` and records the sampled mip usage while returning the sampled component.

`const.sampler` fields: `min`, `mag`, `mip` filters (`nearest`, `linear`); `address_u`, `address_v`, `address_w` (`clamp_to_edge`, `repeat`, `mirrored_repeat`, `clamp_to_border`, `mirror_clamp_to_edge`); `border` (`transparent_black`, `opaque_black`, `opaque_white`); `max_anisotropy` 1 to 16; `compare` (`never`, `less`, `equal`, `less_equal`, `greater`, `not_equal`, `greater_equal`, `always`); `lod_min`, `lod_max`, `lod_bias`; `reduction` (`weighted_average`, `min`, `max`); `normalized_coordinates`. Each field is optional and may occur once. Omitted fields default to nearest filters, clamp-to-edge addresses, transparent-black border, anisotropy 1, disabled comparison, `lod_min: 0.0`, `lod_max: inf`, `lod_bias: 0.0`, weighted-average reduction and normalized coordinates. LOD values are exactly representable `f32` literals. Sampler handles created by Ember carry the same state.

Texture formats are Ember's. The component type in `texture<...>` is what the shader reads and writes.

### 17.7 Derivatives and quads

`gpu.ddx`, `gpu.ddy` (`[coarse | fine]`) and `gpu.fwidth` compute screen-space derivatives across a 2×2 quad. Omitted derivative mode means `coarse`. Helper invocations participate. `quad.broadcast %value, %lane` broadcasts from lane 0–3; `%lane` has type `i32` and is dynamically uniform. `quad.swap [x | y | diagonal]` selects the horizontal, vertical or diagonal partner, defaulting to `x`. `quad.all` and `quad.any` reduce an `i1` across the quad. All require quad-uniform control flow (section 17.15).

### 17.8 Subgroups and reconvergence

1. A subgroup has between 4 and 128 invocations, a power of two fixed per pipeline. The entry attribute `subgroup_size(n)` requests an exact size (capability `subgroup_size_control`).
2. Maximal reconvergence: invocations that diverge at a structured selection or loop reconverge at its merge block. A subgroup operation acts on exactly the invocations that are active at that point under the structured control flow of section 11.3.
3. Operations: `elect`, `all`, `any`, `all_equal`, `broadcast` (a constant or dynamically uniform `i32` lane), `broadcast_first`, `ballot` (result `vec<4 x i32>`, a 128-bit mask), `inverse_ballot`, `ballot_bit_count`, `ballot_find_lsb`, `ballot_find_msb`, `shuffle`, `shuffle_xor`, `shuffle_up`, `shuffle_down`, `rotate`, `reduce`, `scan_inclusive`, `scan_exclusive`. The lane or offset operand of `broadcast`, `shuffle`, `shuffle_xor`, `shuffle_up`, `shuffle_down` and `rotate` has type `i32`. Reductions and scans take exactly one operator: `add.s`, `add.u`, `add.f`, `mul.s`, `mul.u`, `mul.f`, `min.s`, `min.u`, `min.f`, `max.s`, `max.u`, `max.f`, `and`, `or` or `xor`. Integer operators require an integer scalar; `.f` operators require a floating-point scalar. They may also take `[cluster n]`, where n is a power of two from 1 through the selected subgroup size. An omitted cluster covers the whole subgroup.

### 17.9 Barriers and memory

- `gpu.barrier [subgroup | threadgroup] [memory: device, threadgroup, texture, tile, object_payload]` is an execution barrier with the listed memory made available and visible within the scope. An omitted scope defaults to `threadgroup`; an omitted memory clause selects all five listed domains, including `tile` and its capability requirement.
- `gpu.memfence [scope] [memory: device, threadgroup, texture, tile, object_payload]` orders memory without an execution barrier. An omitted scope defaults to `device`, as for other GPU fences (section 15.3); an omitted memory clause selects all five listed domains.
- Atomics follow section 15 with scopes. `device` and `threadgroup` memory support atomics; textures support `tex.atomic`.
- A barrier MUST be executed in control flow that is uniform for its scope (section 17.15, rule 9).

### 17.10 Object and mesh stages

1. An object threadgroup writes its payload through its `payload` pointer and executes `object.dispatch_mesh %x, %y, %z` exactly once, in threadgroup-uniform control flow. Each launched mesh threadgroup reads the payload.
2. A mesh threadgroup executes `mesh.set_counts %vertices, %primitives` once, in uniform control flow, before writing outputs. It then writes `mesh.write.vertex %i, %v`, `mesh.write.primitive %i, %p` and `mesh.write.indices %i, %idx` (`vec<3 x i32>`, `vec<2 x i32>` or `i32` per topology). Indices out of the declared counts are discarded.

### 17.11 Ray tracing

Inline ray queries are available in every stage:

- `rq.init %q, %accel, %flags, %mask, %origin, %tmin, %direction, %tmax` initializes a `rayquery` held in a `thread` slot.
- `rq.proceed %q` returns `i1`: whether a candidate needs a decision.
- `rq.commit_triangle %q`, `rq.commit_procedural %q, %t`, `rq.abort %q`.
- `rq.get %q, attribute [candidate | committed]` reads intersection data: `kind`, `t`, `instance_id`, `instance_custom_index`, `geometry_index`, `primitive_index`, `barycentrics`, `front_face`, `object_ray_origin`, `object_ray_direction`, `object_to_world`, `world_to_object`. Integer attributes return `i32`, `t` returns `f32`, `barycentrics` returns `vec<2 x f32>`, `front_face` returns `i1`, ray origin/direction return `vec<3 x f32>`, and transforms return twelve `f32` results in row-major order. Omitted selection means `candidate`.

Ray tracing pipelines use the entry kinds `raygen`, `intersection`, `anyhit`, `closesthit`, `miss` and `callable`:

- `rt.trace %accel, %flags, %mask, %sbt_offset, %sbt_stride, %miss_index, %origin, %tmin, %direction, %tmax, %payload` traces a ray and invokes the hit or miss shaders.
- `rt.report_hit %t, %kind, %attrs` (intersection) yields `i1` acceptance; `rt.ignore_hit` and `rt.accept_and_end` (any-hit) are terminators.
- `rt.call_callable %index, %data`.
- `rt.get name` reads `launch_id`, `launch_size`, `world_ray_origin`, `world_ray_direction`, `object_ray_origin`, `object_ray_direction`, `ray_tmin`, `ray_tcurrent`, `ray_flags`, `instance_id`, `instance_custom_index`, `geometry_index`, `primitive_index`, `hit_kind`, `object_to_world`, `world_to_object`, and `triangle_vertex_positions` (capability `triangle_vertex_positions`). IDs and flags are `i32`; launch IDs and sizes are `vec<3 x i32>`; t values are `f32`; origins/directions are `vec<3 x f32>`; transforms return twelve `f32` results in row-major order; triangle positions return nine `f32` results in vertex order (`v0.xyz`, `v1.xyz`, `v2.xyz`).

Hit objects separate traversal from shading:

- `rt.hitobject.trace %accel, %flags, %mask, %sbt_offset, %sbt_stride, %miss_index, %origin, %tmin, %direction, %tmax` traverses without invoking hit shaders and yields a `hitobject`; its operands have the corresponding types of `rt.trace` without the payload. `rt.hitobject.from_query %q` builds one from a committed ray query; `rt.hitobject.miss` builds a miss.
- `rt.hitobject.invoke %h, %payload` runs the hit or miss shader the object refers to; `rt.hitobject.get %h, attribute` reads it.
- `rt.hitobject.get` uses the hit-attribute names and result types of `rq.get` without a candidate/committed selector.
- `rt.reorder %h [hint %k, bits n]` permits the GPU to reorder invocations for coherence. `%k` is an optional integer ordering key and `n` is its width, 1 through 64; if omitted, no key is supplied. Backends without hardware support implement it as no reordering; semantics are identical.

Trace recursion depth is bounded by the pipeline (Ember.md), at most 31.

### 17.12 Cooperative matrices and tensors

`coop<scope, M x N, T, use>` values are matrices distributed across the invocations of a subgroup or threadgroup. Elementwise instructions of sections 12.2 to 12.4 accept cooperative-matrix operands of identical shape lane-wise.

| Instruction | Semantics |
| --- | --- |
| `coop.fill T, %s` | Every element set to `%s` |
| `coop.load T, %p, %stride [row_major \| col_major]` or `coop.load T, %tensor, %coords` | From `device`, `threadgroup` or a tensor |
| `coop.store %m, %p, %stride [row_major \| col_major]` | |
| `coop.mma %a, %b, %c [unsigned] [saturate]` | `a × b + c` |
| `coop.convert %m to T` | Element type or use conversion |
| `coop.length T` | Elements held per invocation |
| `coop.extract %m, %i` | Per-invocation element |
| `coop.insert %m, %i, %e` | |
| `coop.reduce %m, op [row \| column \| quad2x2]` | Capability `coop.reduce` |
| `tensor.load %t, %coords` | Element of a tensor handle |
| `tensor.store %t, %coords, %v` | |
| `tensor.query %t, dim [extent \| stride]` | `i64` |

Baseline shapes, required on every GPU family (in hardware or in the backend): `(M, N, K) = (16, 16, 16)` at subgroup scope for `f16 × f16 + f32`, `f16 × f16 + f16`, `bf16 × bf16 + f32`, and `i8 × i8 + i32` (signed and unsigned). Optional: `f8e4m3` and `f8e5m2` inputs with `f32` or `f16` accumulators at `16 × 16 × 32` (capability `coop.fp8`); block-scaled `f4e2m1`, `f6e2m3`, `f6e3m2` and `f8` inputs with `e8m0` scales at `16 × 16 × 64` (capability `coop.blockscaled`). Every family publishes its additional shapes in TVISA.md; `tirc` rejects a shape the target families do not support.

`coop.fill` takes a cooperative-matrix type and an element value of its element type. `coop.load` produces the named type. Its pointer form takes a `ptr<device>` or `ptr<threadgroup>` and an element stride; the stride is the distance between adjacent rows for row-major storage or adjacent columns for column-major storage. Omitted layout is row-major. Its tensor form takes a tensor handle and coordinates whose rank matches the tensor rank; the tensor element type must match the matrix element type. `coop.store` takes a matrix, one of those pointers, and an element stride, with the same layout rule.

For `coop.mma`, `a`, `b` and `c` have uses `a`, `b` and `acc`; their scopes match; the inner dimensions match; and the result has `a`'s row count and `b`'s column count. The `unsigned` modifier treats integer input elements as unsigned; without it they are signed. `saturate` is valid for integer accumulator types and clamps the accumulation to the accumulator element width. Shape and type combinations supported by a target family are listed in TVISA.md.

`coop.convert` preserves scope and dimensions and converts each element to the target element type; its target use may differ. `coop.length` returns `i32`, the number of elements held by the current invocation. `coop.extract` reads and `coop.insert` replaces the invocation-local element selected by an integer index; `coop.insert` returns the updated matrix. An index is valid when it is less than `coop.length` for that matrix type.

`coop.reduce` uses one of the section 17.8 reduction operators and returns the same matrix type. `row` reduces each row and broadcasts its result across that row; `column` reduces each column and broadcasts its result across that column; `quad2x2` reduces each 2×2 group and broadcasts its result within that group. `quad2x2` requires subgroup scope and even row and column counts. This instruction requires capability `coop.reduce`.

Tensor coordinates are integer values: rank 1 uses an integer scalar; ranks 2, 3, 4 and 8 use `vec<rank x i32>`. These are the tensor ranks representable by the fixed-vector lane counts in section 7.3. `tensor.load` returns the tensor element at the coordinates; `tensor.store` writes a value of the tensor element type. `tensor.query` takes a zero-based dimension less than the tensor rank and `extent` or `stride` (default `extent`), and returns an `i64`; strides are measured in elements.

### 17.13 Tile shading

With capability `tile`, a `tile` entry runs over the tile being rendered and reaches its imageblock through `tile.imageblock.addr %position`, where `%position` is `vec<2 x i32>` (yielding `ptr<tile> %Block`). Fragment code reaches the same memory through `frag.imageblock.addr`. With capability `framebuffer_fetch`, `frag.load_attachment n` reads the current value of color attachment n at the fragment; `n` is an unsigned 32-bit index. With capability `raster_order`, `frag.raster_order.begin` and `frag.raster_order.end` bracket a critical section ordered by primitive submission per pixel.

### 17.14 Work graphs

With capability `work_graphs`, `node` entries form graphs that Ember launches.
Each entry's `node` attribute declares its launch mode, dispatch-grid limit,
input-record size and ordered outputs, as described in section 17.2. Output
indexes passed to `node.output.allocate` are zero-based indexes into that list.

| Instruction | Semantics |
| --- | --- |
| `node.input.addr %i` | Address of input record i |
| `node.input.count` | `i32` |
| `node.output.allocate %output, %count [thread \| group]` | Allocates output records for node output `%output`; omitted mode means `thread` |
| `node.output.addr %alloc, %i` | Address of an allocated record |
| `node.output.commit %alloc` | Makes records visible to the consumer node |
| `node.finished_cross_group` | `i1` for nodes that share input across threadgroups |

An output whose declared kind is `mesh` targets a mesh node and requires
capability `work_graphs.mesh`. Other node outputs require only `work_graphs`.

### 17.15 Restrictions (rule V15)

1. The call graph of GPU functions is acyclic. Indirect calls happen only through `gpu.call.vft %table, %index (args)` in compute and ray tracing stages, with a depth bound set by the pipeline. Its possible targets are all exported ordinary `target(gpu)` functions with the exact function type recorded by `%table`; each target participates in stage, capability and acyclic-call-graph verification.
2. No heap instruction, no `ref`, `uniq` or `weak` type, and no type whose policy is non-trivial. GPU values are trivial or borrowed resource handles.
3. No coroutine, async, unwinding, variadic, `setjmp`, `longjmp`, `asm`, `asm.goto`, `target.intrinsic`, `call.foreign`, closure, `atomic.wait`, `atomic.notify`, `fp.flags.*`, or portable intrinsic except `ct.select` and `ct.eq`.
4. Every generic is specialized before GPU lowering; `call.witness` must resolve statically.
5. No globals except `constant`-space globals with initializers and threadgroup storage allocated in an entry's entry block.
6. No integer wider than 64 bits and no `f128`. `f64` requires capability `float64`. Fixed vectors have 2, 3 or 4 lanes. `svec` is forbidden.
7. Control flow is structured (S1 to S7).
8. `stack.alloc` sizes are static. A supplied `count` must resolve to a nonnegative compile-time integer smaller than 2^62; in binary CODE it MUST be the result of `const.int`.
9. Uniformity: `tirc` runs a uniformity analysis in which builtins that differ between invocations, values loaded through divergent addresses, and their dependents are divergent. Barriers require threadgroup- or subgroup-uniform control flow for their scope; derivatives, implicit-derivative sampling and quad operations require quad-uniform control flow; `object.dispatch_mesh` and `mesh.set_counts` require threadgroup-uniform control flow. An instruction whose requirement the analysis cannot prove is rejected unless it carries `[unchecked_uniformity]`, which converters use when their input language guarantees uniformity without proof. A non-uniform execution of an unchecked instruction faults or times out the dispatch; it cannot affect another process.
10. Register-only types obey section 8.6.

### 17.16 Floating point on GPUs

Every GPU function has `fp(precise)` (default) or `fp(fast)`. `fp(fast)` implies the defined-mode fast-math flags `nsz arcp contract afn reassoc` on every floating-point instruction; it never implies `nnan` or `ninf`.

| Operation | `f32` precise | `f32` fast | `f16` |
| --- | --- | --- | --- |
| `fadd`, `fsub`, `fmul`, `fma` | Correctly rounded | Correctly rounded | Correctly rounded |
| `fdiv` | Correctly rounded | ≤ 2.5 ULP | ≤ 1 ULP |
| `fsqrt` | Correctly rounded | ≤ 1 ULP | Correctly rounded |
| `math.rsqrt` | ≤ 2 ULP | ≤ 2 ULP | ≤ 1 ULP |
| `math.exp`, `math.exp2`, `math.log`, `math.log2` | ≤ 3 ULP | ≤ 3 ULP, or absolute error ≤ 2^−21 for `log` near 1 | ≤ 2 ULP |
| `math.sin`, `math.cos`, `math.tan` | ≤ 4 ULP | Absolute error ≤ 2^−11 on [−π, π] | ≤ 2 ULP |
| `math.pow` | ≤ 16 ULP | Derived from `exp2(y × log2(x))` | ≤ 4 ULP |
| Other `math.*` | ≤ 4 ULP | ≤ 8 ULP | ≤ 4 ULP |
| `fmuladd` | Fused or unfused per family | Fused or unfused per family | Same |

Subnormal `f32` values may be flushed to sign-preserving zero on input and output unless the function carries `fp(ieee_denormals)`, which requires capability `float32.denorm_preserve`. `f16` and `f64` subnormals are preserved. In `fp(precise)`, NaN propagation and `fmin`/`fmax` follow IEEE 754-2019.

### 17.17 Single source and entry references

1. CPU and GPU code in one module share nominal types, constants and `target(cpu, gpu)` functions. A struct has one layout on the CPU and on every GPU (section 8.5).
2. `gpu.entry.ref @e` in CPU code yields a value of type `entry<kind, root R, immediate I>`, where `R` is the pointee type of the entry's root parameter and `I` its immediate type (`unit` when absent). The loader resolves the value to a process-unique 64-bit identifier of the entry in its image.
3. Across RIDL, `entry<...>` is the RIDL 1.1 type `GpuEntry<Kind, Root, Immediate>` (section 19.2). An API that dispatches or draws takes a `GpuEntry<K, R, I>` together with values typed by `R` and `I`, so a producer checks at compile time that the data supplied matches the entry. Kira and KSL do this; the mechanism is general and belongs to no API.
4. Each image's GPU section records, per entry: kind, root and immediate layouts and type identifiers, threadgroup size, threadgroup and payload memory, capabilities used, specialization constants, and a binary per built GPU family (section 29.5).

### 17.18 Capabilities and specialization

Baseline capabilities, which every Tessera GPU provides in hardware or through its backend: compute, vertex, fragment, object and mesh stages; bindless handles; 64-bit GPU pointers; 64-bit integer arithmetic; 64-bit integer atomics on `device` and `threadgroup`; `f32` atomic add, min and max; `f16` arithmetic and storage; 8- and 16-bit storage; integer dot products; every subgroup and quad operation; ray queries, ray tracing pipelines and hit objects; baseline cooperative-matrix shapes; 32-bit texture atomics; texture gather; cube arrays; multiview.

Optional capabilities: `float64`, `atomic128`, `float16.atomics`, `tile`, `framebuffer_fetch`, `raster_order`, `work_graphs`, `work_graphs.mesh`, `coop.fp8`, `coop.blockscaled`, `coop.reduce`, `compute_derivatives`, `float32.denorm_preserve`, `sparse_residency`, `sampler_feedback`, `subgroup_size_control`, `triangle_vertex_positions`, `shading_rate`, `printf`.

Each entry declares the optional capabilities it uses with `caps(...)`. `tirc` verifies the declaration against the code. Ember refuses to create a pipeline that uses a capability the device lacks, and the pipeline builds an entry only for families that support its capabilities.

`gpu.spec_const id, T, default` yields a value fixed when a pipeline is created. `id` is an unsigned 32-bit TIR integer literal. Scalar integer types have 1 through 64 bits; `i1` uses `true` or `false`, and other integer types use a signed or unsigned integer literal in range. Scalar floating-point types are `f16`, `bf16`, `f32` and `f64`; `f64` requires the `float64` capability. A fixed-vector type has 2, 3 or 4 numeric lanes accepted by the GPU type rules in sections 7.1 and 17.1. `default` is an untyped scalar literal, or a bracketed list with one scalar literal per lane. Each literal is in range or exactly representable in its declared scalar type. The pipeline precompiles entries with default values. A pipeline created with other values is compiled on the device from the TVISA GPU code (section 17.19).

### 17.19 Compilation and delivery

1. The pipeline compiles every GPU entry for every GPU family in the target matrix and stores the binaries in the native image's GPU section. Creating a pipeline on the device then links precompiled stage binaries without compiling.
2. The TVISA image carries the GPU code in TVISA's GPU profile. The device finalizer compiles it when the device's GPU family was not built, when the GPU is a revision the pipeline did not know, and for non-default specialization constants. On-device GPU compilation MUST be robust: it handles every TIR GPU construct that the family's backend handles server-side, with identical results.
3. Finalized GPU binaries are stored in the code cache (section 28.8).
4. The GPU families and their backends are defined in TVISA.md: AMD RDNA 2 and later, NVIDIA Turing and later, Intel Xe-HPG and later, Qualcomm Adreno 7xx and later, Arm Mali Valhall with the command stream frontend and the 5th Gen Arm GPU architecture and later, Imagination PowerVR D-Series and later, Apple AGX G13 and later, and the Tessera paravirtual GPU `tessera.tvgpu`. TVISA.md section 31 lists their identifiers.
5. The GPU reference executor (section 28.11) is the conformance oracle for every backend.

### 17.20 Instructions

Builtins and control (range 0x400):

| Instruction | Semantics |
| --- | --- |
| `gpu.builtin name : T` | Section 17.5 |
| `gpu.spec_const id, T, default` | Section 17.18 |
| `gpu.barrier [scope] [memory: ...]` | Section 17.9 |
| `gpu.memfence [scope] [memory: ...]` | |
| `gpu.call.vft %table, %index (args)` | Indirect call through a visible function table |
| `frag.demote` | The invocation becomes a helper: it stops writing outputs but keeps participating in derivatives |
| `frag.load_attachment n` | Capability `framebuffer_fetch` |
| `frag.raster_order.begin` | Capability `raster_order` |
| `frag.raster_order.end` | |
| `frag.imageblock.addr` | Capability `tile` |
| `gpu.printf "format", args` | Capability `printf`; development builds only; stripped by the pipeline in release delivery units |

Derivatives and quads (range 0x420): `gpu.ddx`, `gpu.ddy`, `gpu.fwidth`, `quad.broadcast`, `quad.swap`, `quad.all`, `quad.any`.

Subgroups (range 0x440): `subgroup.elect`, `subgroup.all`, `subgroup.any`, `subgroup.all_equal`, `subgroup.broadcast`, `subgroup.broadcast_first`, `subgroup.ballot`, `subgroup.inverse_ballot`, `subgroup.ballot_bit_count`, `subgroup.ballot_find_lsb`, `subgroup.ballot_find_msb`, `subgroup.shuffle`, `subgroup.shuffle_xor`, `subgroup.shuffle_up`, `subgroup.shuffle_down`, `subgroup.rotate`, `subgroup.reduce`, `subgroup.scan_inclusive`, `subgroup.scan_exclusive`.

Textures (range 0x480): `tex.sample`, `tex.sample_level`, `tex.sample_grad`, `tex.sample_bias`, `tex.sample_compare`, `tex.sample_compare_level`, `tex.gather`, `tex.gather_compare`, `tex.read`, `tex.write`, `tex.atomic`, `tex.query` (`width`, `height`, `depth`, `size`, `layers`, `levels`, `samples`), `tex.lod`, `tex.feedback` (capability `sampler_feedback`).

Object and mesh (range 0x4C0): `object.dispatch_mesh`, `mesh.set_counts`, `mesh.write.vertex`, `mesh.write.primitive`, `mesh.write.indices`.

Ray tracing (range 0x500): `rq.init`, `rq.proceed`, `rq.commit_triangle`, `rq.commit_procedural`, `rq.abort`, `rq.get`, `rt.trace`, `rt.report_hit`, `rt.ignore_hit`, `rt.accept_and_end`, `rt.call_callable`, `rt.get`, `rt.hitobject.trace`, `rt.hitobject.from_query`, `rt.hitobject.miss`, `rt.hitobject.invoke`, `rt.hitobject.get`, `rt.reorder`.

Cooperative matrices and tensors (range 0x560): `coop.fill`, `coop.load`, `coop.store`, `coop.mma`, `coop.convert`, `coop.length`, `coop.extract`, `coop.insert`, `coop.reduce`, `tensor.load`, `tensor.store`, `tensor.query`.

Tile (range 0x5A0): `tile.imageblock.addr`.

Work graphs (range 0x5C0): `node.input.addr`, `node.input.count`, `node.output.allocate`, `node.output.addr`, `node.output.commit`, `node.finished_cross_group`.

---

## 18. Generics

### 18.1 Shipped generics

TIR modules carry generic definitions. Producers MAY specialize before emitting TIR, but they are not required to: `tirc` specializes. Every generic function and type is compiled by one or both strategies:

1. Specialization. For every use with concrete arguments anywhere in the package, `tirc` clones the generic with arguments substituted, deduplicated by (generic, arguments). Specialization is whole-package: a generic defined in one module of a package and used in another is specialized at the use.
2. Polymorphic entry. One unspecialized body that receives, per type parameter, a descriptor and one witness table per bound, handling generic values by address (KLF.md section 23).

### 18.2 Rules

1. A producer requests `specialize`, `polymorphic` or `both` per generic. Exported generics are forced to `both`.
2. A generic that cannot be expressed polymorphically (a type parameter used as a compile-time constant in a type, a non-type generic parameter, or behavior specialized on the concrete type) is rejected at export with `TIR-GEN-0001` and may still be exported as named instances (`@Instance`, KLF.md section 23.5).
3. GPU code is always fully specialized.
4. `tirc` MAY call a polymorphic entry instead of specializing when a specialization would exceed the code-size budget of the optimization level. Results are identical.
5. Inside a polymorphic body, descriptors of nested instances are built with the nested type's builder once per activation (KLF.md section 23.4).
6. Each conformance infers one shared substitution for its trait's generic parameters by structurally matching every requirement function type against its witness function type. A trait generic binds to the corresponding witness type and MUST resolve consistently at every occurrence and across all methods. All other function-type structure, parameter context and convention, result and error types, async state, ABI and foreign status MUST match. A trait generic parameter unused by every method is unconstrained; the conformance's implementing type is not implicitly substituted for trait generic parameters.

### 18.3 Inlinable exports

A module MAY mark exported functions, including generic ones, `inlinable`. Their TIR bodies are carried in the module's RIDL records together with a BLAKE3-256 body hash, and the `.ridl` file carries `@Inlinable` (section 19.2).

1. A consumer package's `tirc` MAY inline or specialize an inlinable body.
2. The consumer's native and TVISA images then record a dependency (framework, ABI UUID, body hash) for every body they used.
3. The loader refuses an image whose recorded body hashes differ from the installed framework's, with `TIR-RT-0205`.
4. Before every train ships, the pipeline rebuilds every live package that depends on a body the train changes (section 30.7), so installed apps never reach that refusal through ordinary updates.

---

## 19. RIDL integration and RIDL 1.1

### 19.1 Use of RIDL

1. Every exported function and type follows RIDL: types and `.ridl` files (KLF.md section 20), data layout (section 21), calling convention (section 22), generics (section 23), symbols, metadata, versioning and linking (section 24).
2. `tirc` lowers `abi(ridl)` to `ridlcc` for each architecture.
3. The RIDL metadata blob of KLF.md section 24.3 is embedded in every native image.
4. `tirc abi freeze`, `tirc abi apply` and `tirc abi retire` replace the `klf abi` commands and operate on TIR modules with the same semantics.

### 19.2 RIDL 1.1 additions

TIR requires the following additions to RIDL. All are additive under KLF.md section 24.4, except the stack-cleanup rule, which changes a convention that no shipped binary uses.

| Addition | Definition |
| --- | --- |
| `F16`, `BF16` | 2-byte scalars; float-class leaves |
| `Vec<N, T>` | Fixed vectors, N ∈ {2, 3, 4, 8, 16}, T a RIDL integer or float scalar. Layout of section 8.3. On aarch64 and x64, a vector of total size ≤ 16 bytes is one float-class leaf occupying one SIMD register; larger vectors are passed by reference. On rv64, vectors are flattened into scalar leaves |
| `async function` | Section 19.3 |
| `GpuEntry<Kind, Root, Immediate>` | One 8-byte word. `Kind` is one of the marker types of RIDL module `Gpu`: `Compute`, `Object`, `Mesh`, `Vertex`, `Fragment`, `Tile`, `RayGeneration`, `Intersection`, `AnyHit`, `ClosestHit`, `Miss`, `Callable`, `Node`, `Visible`. `Root` and `Immediate` are RIDL types or `Void`; for `Visible` (a function callable through visible function tables, TVISA.md section 21.2 kind 13), `Root` is the function's type and `Immediate` is `Void` |
| `DevicePtr<T>`, `ConstantPtr<T>` | GPU pointers: one 8-byte word |
| `TextureHandle`, `SamplerHandle`, `AccelerationStructureHandle`, `FunctionTableHandle`, `TensorHandle` | Resource handles: one 8-byte word |
| `@Requires(entitlement: "name")` | Declares that calling the annotated function, or any method of the annotated type, requires the named entitlement (section 21.1) |
| `@Inlinable` | Marks an exported function whose TIR body is carried for consumers (section 18.3) |
| Callee-popped stack arguments | Under ridlcc the callee releases its stack argument area as it returns, as LLVM's `swifttailcc` does, so that every RIDL signature supports guaranteed tail calls (section 11.5). Register assignment is unchanged. This replaces the base standard's caller cleanup of KLF.md section 22.1 for stack arguments only; no RIDL 1.0 binaries have shipped. TVISA.md section 9.3 gives the per-architecture details |
| riscv64 register row | Table below |

ridlcc on rv64 (the row KLF.md section 22.2 lacks):

| Target | Integer arguments | Float arguments | Direct results | Indirect result | Context | Error |
| --- | --- | --- | --- | --- | --- | --- |
| rv64 | a0 to a7 | fa0 to fa7 | a0 to a3, fa0 to fa3 | t4 | s10 | s11 |

The base procedure call standard is the RISC-V LP64D psABI with the Zicfiss shadow stack and Zicfilp landing pads. The context register is callee-saved; the error register is zeroed by the caller before a call to a failing function and written by the callee.

### 19.3 Async across RIDL

RIDL module `Async` (part of the RIDL foundation shipped with every toolchain):

```
enum Poll<T> {
    Ready(T)
    Pending
}

struct WakerOps {
    let clone: RawPtr        // fn(data: RawPtr) -> Waker
    let wake: RawPtr         // fn(data: RawPtr), consumes
    let wakeByRef: RawPtr    // fn(data: RawPtr)
    let drop: RawPtr         // fn(data: RawPtr)
}

struct Waker {
    let data: RawPtr
    let ops: RawPtr          // static WakerOps
}

trait Future<T> {
    function poll(borrow mut self, waker: borrow Waker) -> Poll<T>
}
```

1. `async function f(params) -> R` in a `.ridl` file is a function returning `Any Future<R>`, annotated so binding generators present it natively (`async fn` in Rust, `async` in Kira).
2. The four `WakerOps` functions use `ridlcc`.
3. Parameters of exported async functions MUST be owned or trivial; a `borrow` or `borrow mut` parameter is rejected with `ridl.async-borrow`, because the returned future would outlive the borrow.
4. `poll` after `Ready` traps with `TIR-RT-0022`.

---

## 20. Architecture-specific code

### 20.1 Variant sets

A variant set gives one function several bodies:

```
variants @Crc.update {
    portable:       @Crc.update.generic
    aarch64:        @Crc.update.a64
    aarch64[sve2]:  @Crc.update.sve2
    x64:            @Crc.update.x64
    x64[avx512]:    @Crc.update.avx512
    rv64:           @Crc.update.rv64
}
```

Calls to `@Crc.update` and its address refer to the set. Variant bodies are ordinary functions with `variant(arch [, features...])` attributes.

### 20.2 Rules (V18)

1. Every member has the set's signature, ABI and engine `native`.
2. A variant body for architecture A may contain `asm`, `asm.goto` and `target.intrinsic` for A only. A portable body contains none.
3. Features named in brackets are TVISA.md feature names of the architecture.
4. Selection for a CPU target T: among the variants of T's architecture whose features T guarantees, choose the one with the most features (declaration order breaks ties); if none, the portable body. If some variant needs features T only may have, `tirc` emits a load-time dispatch that the loader resolves once per process, choosing the most specific variant whose features the CPU has.
5. All members of a set MUST be observably equivalent for every input. Conformance tests compare them (section 34.3); a mismatch is a producer bug.

### 20.3 The all-three rule

1. A variant set in which any variant contains `asm` or `asm.goto` MUST have a feature-free base variant for each of aarch64, x64 and rv64. A portable body does not substitute.
2. A variant set without assembly MUST have a portable body or a base variant for each of the three architectures.
3. `tirc package` and the pipeline reject violations with `TIR-ARCH-0003`.
4. A package that lists the `web` or `wasi` platform also needs, in every variant set it uses, a portable body or a `wasm` member, and a `wasm` member in every set with assembly (section 32.5).

### 20.4 Inline assembly

1. The template syntax and the constraint language are LLVM's for the architecture.
2. `[sideeffect]` prevents removal; `[memory]` declares memory reads and writes; `[pure]` declares neither, allowing common-subexpression elimination; `[privileged]` (kernel profile only) allows privileged instructions.
3. Labels are local to the template. Control leaves an `asm` only by falling through, and an `asm.goto` only to its listed blocks.
4. `tirc` assembles templates with its integrated assembler for each architecture. TVISA carries templates and constraints so the finalizer can assemble them (TVISA.md).
5. Functions containing assembly keep every hardening requirement of section 27. Indirect branches inside a template MUST target landing pads; this cannot be verified and is recorded in the analysis report.

TIR inline assembly is supported only in aarch64, x64 and rv64 variant bodies, whose constraint languages and assembler support are defined by TVISA.md section 16.5. A wasm variant MUST use portable operations or target intrinsics instead.

### 20.5 GPU code

GPU code has no variants and no assembly. Capability-dependent behavior uses `gpu.spec_const` or entry capabilities.

---

## 21. Analysis data, capabilities and control-flow integrity

### 21.1 Capability declarations

`tirc package` computes, for every module, its capability declaration:

1. The LibTessera calls reachable from the module: every function imported from the LibTessera RIDL module that the conservative call graph reaches. Indirect calls reach every address-taken function of a compatible type.
2. The system-framework functions reachable that carry `@Requires`.
3. The implied entitlements: those LibTessera.md assigns to the reached calls, plus those named by reached `@Requires`.
4. For modules of a platform other than `tessera`: the platform imports reached, by library and symbol (section 32.6 item 10). Such modules imply no entitlement.

Each RIDL function import carries the effective `@Requires` names from its
function and containing type, and the entitlement names assigned to that
LibTessera function by LibTessera.md. The verifier uses these sorted contract
metadata lists to recompute CAPS and does not trust the CAPS declarations. The
first list is empty when no `@Requires` applies; the second is empty except for
LibTessera functions. Only reached functions contribute references or
entitlements.

The conservative call graph starts at every defined non-private function, every
entry, every platform export, every inlinable function, and every module
initializer and finalizer. A reachable `foreign.entry` adds its defined
function as a root because platform code can call it. Traversal follows direct
calls (`call`, `try.call`, `invoke`, `call.foreign`, `call.variadic`,
`tail.call`, and `coro.start`), speculation fallbacks, and `catch.filter`
functions. Code in an unreachable private function does not contribute.

For CPU indirect calls, the conservative target set contains every function
whose address is taken in reachable code and whose function type exactly
matches the call site, plus compatible functions recorded in GENERICS vtables
and witness tables. A closure call uses the function references stored in its
reachable closure code. If a table dispatch cannot be narrowed statically, all
compatible GENERICS targets are included. A `gpu.call.vft` reaches every
exported ordinary `target(gpu)` function with the exact function type recorded
by the visible function table. A platform import is reached when reachable
code calls it or takes/references its address or data. Modules targeting
`tessera` have an empty reached-platform-import list.

It embeds the declaration in the module's CAPS section and checks:

| Check | Failure |
| --- | --- |
| Implied entitlements ⊆ the package's requested entitlements (manifest) | `TIR-CAP-0001` |
| Requested entitlements ⊆ the entitlements the profile allows | `TIR-CAP-0002` |
| An entitlement requested but never implied is reported as unused | Remark `TIR-CAP-0100` |

### 21.2 Dynamic TIR

A module created at run time (section 28.7) may import only LibTessera calls and framework functions within its creating process's declared set. The device runtime rejects it otherwise with `TIR-RT-0300`.

### 21.3 Control-flow integrity

1. Every indirect call (`call.indirect`, `call.closure`, `call.vtable`, `call.witness` through a runtime table, `gpu.call.vft`) carries its function type. Rule V19 forbids converting function values between types.
2. Every CPU function whose address is taken, and every CPU function reachable through a vtable, witness table or closure, has a landing pad and a 32-bit type tag derived from its function type's `typeid` by the rule of TVISA.md section 17.2. A GPU function reachable through a visible GPU function table has no CPU landing pad or type tag; `gpu.call.vft` enforces its function type through the typed, bounded table (TVISA.md section 17.1).
3. Every CPU indirect call checks the target's tag against the call site's type before transferring control. A mismatch traps with `TIR-RT-0012`. Section 27.2 fixes the instruction sequences.

### 21.4 Unmanaged manifest

For every function, the analysis section records its trust class (section 9.7) and a reason mask: bit 0 `asm`, bit 1 raw memory access, bit 2 `inttoptr`, bit 3 foreign or variadic call, bit 4 `setjmp`/`longjmp`, bit 5 unchecked access, bit 6 converter output without ownership information, bit 7 device I/O.

### 21.5 Sandbox regions

Sandboxed functions (Wasm64 input) declare their regions: for each linear memory, the private, mutable, non-thread-local, non-per-CPU `ptr` global holding its base and the private, mutable, non-thread-local, non-per-CPU `i64` global holding its current size. Every base and current-size global is distinct across the table. `tirc` proves that every access of a sandboxed function is bounds-checked against a declared region, and inserts the checks itself (section 25.4). Region indices cover every zero-based linear memory without gaps.

### 21.6 Analysis report

`tirc analyze` and `tirc package` produce a machine-readable report (`analysis.json`, canonical JSON per RFC 8785) for the package:

| Field | Content |
| --- | --- |
| `profile`, `languages`, `producers` | From the modules |
| `capabilities` | Per module and merged: LibTessera calls, `@Requires` functions, platform imports, implied and requested entitlements |
| `trust` | Counts and lists of managed, sandboxed and unmanaged functions with reason masks |
| `semantics` | Counts of defined and permissive functions |
| `assembly` | Every assembly region: function, architecture, effects |
| `variants` | Every variant set and its members |
| `dynamicCode` | Use of the dynamic TIR API, raw JIT entitlements, `runtime`-engine functions |
| `gpu` | Entries, kinds, capabilities |
| `ridl` | Exported and imported surfaces with ABI UUIDs |
| `strings` | Constant strings that parse as URLs or host names |
| `license`, `source` | From the manifest |

The report's hash is part of the provenance record (section 30.6). `tirc` recomputes every analysis section it reads; a mismatch with the embedded data fails rule V20.

The report is one JSON object with exactly the fields in the table above.
Object members are serialized in RFC 8785 order. Arrays are sorted by the
specified key, and string sets are unique and sorted by UTF-8 bytes.

| Field | JSON shape |
| --- | --- |
| `profile` | Manifest profile string |
| `languages` | Unique language tags from MODULE producers |
| `producers` | Module-sorted objects `{module, name, version, languages}` |
| `capabilities` | `{modules, merged}`; each module has `module`, `libtesseraCalls`, `requires`, `platformImports`, `impliedEntitlements`, `requestedEntitlements`; merged has those five list fields |
| `trust` | `managed`, `sandboxed`, and `unmanaged` objects, each `{count, functions}`; function records have `module` and `function`, and unmanaged records also have the numeric `reasonMask` |
| `semantics` | `{defined, permissive}` function counts |
| `assembly` | Records `{module, function, instruction, architecture, effects}` |
| `variants` | Records `{module, name, members}`; members have `architecture`, `features`, and `function` |
| `dynamicCode` | `{usesApi, apiFunctions, rawJitEntitlements, runtimeFunctions}` |
| `gpu` | Entry records `{module, function, kind, threads, capabilities}` |
| `ridl` | Module records `{module, exports, imports, abiStamps, canonicalHash, inlinableBodies}`; import records carry `module`, `abiUuid`, and named `entities`; export records carry `name`, `kind`, `entityIndex`, `abiTag`, and `abiUuid` |
| `strings` | Records `{module, value, kind}` where `kind` is `url` or `host` |
| `license`, `source` | The corresponding manifest value, or `null` when absent |

Within capabilities, import records are `{module, symbol, abiUuid}` for RIDL
calls and requirements, and `{library, symbol, kind, weak}` for platform
imports. Merged import records are unique and sorted by their canonical JSON
serialization. Entitlement lists are sorted unique strings. The merged
requested-entitlement list is the manifest list; per-module lists come from
CAPS.

Producer records sort by module name; each producer's language list is a
unique sorted set. Per-module capability reference lists preserve CAPS order.
Trust functions sort by module and function; assembly records sort by module,
function, and instruction; variant sets sort by module and name; GPU entries
sort by module and function; RIDL modules sort by module, imports by module,
and imported entities by kind and name. RIDL exports, stamps, and inlinable
bodies preserve their section order. Variant members preserve VARIANTS order.
String records sort by module and original value.

The RIDL export ABI tag is the lowercase hexadecimal encoding of the first
eight bytes of its UUID. A RIDL import UUID is rendered as lowercase
hexadecimal groups in the usual 8-4-4-4-12 form. The canonicalHash is the
lowercase hexadecimal SHA-256 of canonical RIDL text. Inlinable body hashes
are lowercase BLAKE3-256 hexadecimal strings. A dynamic TIR API use is present
when a module imports one or more of TIRRuntime's load, lookup, or unload
functions. A raw JIT entitlement is a requested entitlement whose name
contains jit. Runtime functions are those whose SIGNATURES engine is runtime.
URL and host string records retain the original constant value; host strings
are ASCII DNS names, and URL strings have an absolute scheme and authority
with an ASCII DNS name or IP address as host.
Every report integer is an unsigned JSON integer within the exact RFC 8785
range; a larger TIR u64 value is emitted as a decimal string.
The inlinableBodies records are `{function, blake3}`.

---

## 22. Binary format (`.tirb`)

### 22.1 Conventions

1. Header and section-table fields are fixed-width little-endian. Section payloads use ULEB128 and SLEB128 with minimal encodings only.
2. A string is a ULEB128 byte length followed by valid UTF-8. Strings are referenced by index into STRINGS, types by index into TYPES.
3. A module has exactly one canonical encoding (section 22.10). Packages contain only canonical encodings.

### 22.2 Header (64 bytes)

| Offset | Field | Value |
| --- | --- | --- |
| 0 | Magic | `54 49 52 42` ("TIRB") |
| 4 | `u8` version year | 1–99, e.g. 26 |
| 5 | `u8` version month | 1–12 |
| 6 | `u8` version week | 1–5 (0 only in non-train versions, never in TIR) |
| 7 | `u8` encoding revision | 0 |
| 8 | `u32` flags | Bit 0 defined-only, 1 has GPU code, 2 has unmanaged functions, 3 has assembly, 4 has `runtime`-engine functions, 5 has sandboxed functions, 6 canonical (MUST be 1 in packages); others 0 |
| 12 | `u8` profile | 0 `app`, 1 `system`, 2 `driver`, 3 `kernel` |

The verifier MUST require summary bits 0–5 to exactly match the records: bit 0 is set when every SIGNATURES record has effective semantics `defined` (its override or the MODULE default); bit 1 when a CODE function's SIGNATURES target is GPU or both; bit 2 when an ANALYSIS function has trust class 2 (unmanaged); bit 3 when ANALYSIS has at least one assembly-region record; bit 4 when a SIGNATURES record has engine `runtime`; and bit 5 when an ANALYSIS function has trust class 1 (sandboxed). Bit 6 is the canonical marker and is governed separately by the package rule.
| 13 | `u8` platform | 0 portable, 1 `tessera`, 2 `macos`, 3 `ios`, 4 `windows`, 5 `linux`, 6 `android`, 7 `web`, 8 `wasi` (section 32.2) |
| 14 | `u8[2]` | Zero |
| 16 | `u32` section count | |
| 20 | `u32` | Zero |
| 24 | `u8[32]` content hash | BLAKE3-256 of every byte from offset 64 to the end of the file |
| 56 | `u64` file size | |

### 22.3 Section table

Entries of 32 bytes follow the header, sorted by kind; each kind appears at most once except CUSTOM.

| Offset | Field |
| --- | --- |
| 0 | `u32` kind |
| 4 | `u32` flags (bit 0: zstd-compressed; MUST be 0 in canonical encodings) |
| 8 | `u64` offset from the start of the file |
| 16 | `u64` stored size |
| 24 | `u64` uncompressed size |

### 22.4 Sections

| Kind | Section | Content |
| --- | --- | --- |
| 1 | STRINGS | ULEB count, then strings. Index 0 is the empty string |
| 2 | MODULE | Name; producer name and version; language tags; default semantics (0 defined, 1 permissive); attribute pairs; initializer and finalizer lists (function index and 16-bit priority, run by the loader in ascending priority at load and in descending priority at unload); the driver profile's source-reproducibility hash |
| 3 | IMPORTS | Per import: kind (0 RIDL module, 1 package module, 2 platform library), module path or library name, ABI UUID (16 bytes, zero for package modules and platform libraries), flags (bit 0 weak, platform libraries only), then the imported entities: kind (function, global, type, witness, descriptor, platform data), name, platform symbol name (platform libraries only), type index, sorted `@Requires` names, sorted LibTessera-implied entitlement names |
| 4 | TYPES | Part A: counted nominal declarations (name, generic parameter names, kind). Part B: counted type entries in topological order (section 22.7). Part C: counted nominal definitions (layout rule, fields or variants, optional explicit offsets, policy record with function indices) |
| 5 | CONSTANTS | Counted, typed constants with an explicit value-form tag; scalar bits, zero/null, byte strings, recursively encoded aggregates, active enum variants and active union members, and sampler descriptors |
| 6 | GLOBALS | Counted records containing name, linkage, type, address space, storage flags, alignment and optional CONSTANTS initializer |
| 7 | SIGNATURES | Counted function and entry signatures with type, generic parameters and bounds, function modifiers, engine, semantics, target, variant and platform-export attributes |
| 8 | CODE | Counted defined-function bodies in SIGNATURES order, each with a bounded byte size, block count and blocks |
| 9 | VARIANTS | Counted variant sets; each has a unique name and ordered members containing architecture, sorted feature names and a SIGNATURES function index |
| 10 | GPU | Per entry: function index, kind, threads (0 for dynamic), root, immediate and stage-input type indices, payload and threadgroup sizes, mesh limits, capability set (bit i is the i-th optional capability of section 17.18), subgroup size, floating-point mode, specialization constants (id, type, default), and the node record for node entries |
| 11 | GENERICS | Trait requirements, parent traits, associated types, conformances and vtables; generic strategies are stored in SIGNATURES |
| 12 | RIDL | Export records (symbol, RIDL kind, ABI tag), `@Abi` stamps, inlinable bodies (function index and BLAKE3-256 body hash), the canonical `.ridl` hash |
| 13 | CAPS | Reached LibTessera calls, reached `@Requires` functions, reached platform imports, implied entitlements, requested entitlements |
| 14 | ANALYSIS | Per defined function: trust class (0 managed, 1 sandboxed, 2 unmanaged) and reason mask; sandbox regions; indirect-call type table; assembly-region inventory |
| 15 | METADATA | Alias domains and scopes; per-instruction attachments keyed by (function index, instruction ordinal): 0 branch weights, 1 loop hints, 2 range, 3 nonnull, 4 align, 5 noalias, 6 alias.scope, 7 tbaa, 8 profile id, 9 invariant load |
| 16 | ASM | Templates and constraint strings referenced by assembly instructions |
| 17 | DEBUG | Section 33 |
| 18 | PROFILE | Profile counters and value profiles (section 26.5) |
| 255 | CUSTOM | Producer-private; ignored by `tirc` and removed by `tirc package` |

RIDL (kind 12) starts with a ULEB export count. Each export is a STRINGS
symbol index, a `u8` kind, a ULEB entity index, and a 16-byte nonzero ABI UUID.
Kinds 0–7 are function, method, type descriptor, descriptor builder,
descriptor size, witness table, named instance, and global. Function,
method, and named-instance indexes address SIGNATURES; descriptor, builder,
and descriptor-size indexes address TYPES nominal declarations; witness
indexes address GENERICS conformances; global indexes address GLOBALS.
Exports are strictly ordered by symbol UTF-8 bytes. The symbol is unique and
nonempty. Its ABI tag is the first eight bytes of its ABI UUID.

A ULEB ABI-stamp count follows, then records of a STRINGS symbol index and a
16-byte nonzero ABI UUID. Stamps are strictly ordered by symbol and each
matches an export with the same UUID. A ULEB inlinable-body count follows,
then records of a SIGNATURES function index and a 32-byte BLAKE3-256 hash of
that function's canonical CODE body bytes. Records are strictly ordered by
function index; each function is exported, marked inlinable, and has a CODE
body whose hash matches. The section ends with the 32-byte SHA-256 of the
module's canonical RIDL text. A module with exported RIDL entities MUST have
this section; it is absent when the module has no exported RIDL entities,
stamps, or inlinable bodies. Every index is zero-based and in range.

CAPS is required. Its payload starts with five counted lists in this order:
reached LibTessera calls, reached `@Requires` functions, reached platform
imports, implied entitlements, and requested entitlements. Each of the first
three lists contains `(ULEB IMPORTS index, ULEB imported-entity index)` pairs;
the first two must refer to function entities in RIDL imports, and the third
must refer to entities in platform imports. Pairs are strictly ordered by
`(import index, entity index)`. Each entitlement list contains ULEB STRINGS
indexes; names are nonempty and strictly ordered by their UTF-8 bytes. All
indexes are zero-based. Repeated references and entitlement names are invalid.
The first three lists and implied-entitlement list MUST equal the conservative
call-graph computation in section 21.1.

METADATA starts with a ULEB domain count and that many STRINGS indexes, then a
ULEB scope count and records of `(ULEB domain index, ULEB scope-name index)`.
Domain names are nonempty and strictly ordered by UTF-8 bytes. Scope names are
nonempty; scope records are strictly ordered by `(domain index, scope-name
bytes)`. Every scope belongs to one domain. A ULEB attachment-record count
follows. Each record is `(ULEB SIGNATURES function index, ULEB instruction
ordinal, ULEB attachment count)` followed by that many attachments. The function
must have a CODE body. Ordinals start at zero and count instructions over blocks
in block order. Record keys are strictly ordered by function index and ordinal;
attachments are strictly ordered by kind, with no kind repeated for one
instruction. The section is absent when all three counts are zero, and no
unreferenced domain or scope is encoded.

Each attachment starts with a `u8` kind. Kind 0 branch weights is a ULEB count
of at least two followed by that many ULEB weights; the count equals the number
of CFG successors. Kind 1 loop hints stores ULEB unroll count, vectorize width
and interleave count (zero means absent), then a `u8` `mustprogress` flag (0 or
1); at least one hint is present. Kind 2 range stores a ULEB integer type index,
a ULEB interval count, and that many pairs of ULEB CONSTANTS indexes. Each
constant has the stated integer type; each interval is nonempty in unsigned
value order, and intervals are ordered and disjoint. Kind 3 nonnull has no
payload. Kind 4 align stores a ULEB power-of-two byte alignment. Kinds 5 noalias
and 6 alias.scope each store a nonzero ULEB count and that many strictly
increasing scope indexes. Kind 7 tbaa stores a nonempty STRINGS index naming
the access type tag. Kind 8 profile id stores a ULEB value below 2^32. Kind 9
invariant load has no payload. No other kind is valid. Scope references must
be in range, and every encoded domain and scope must be referenced.

The textual spellings are `!branch_weights(w, ...)`,
`!loop(unroll, vectorize, interleave, mustprogress)`,
`!range(lo, hi, ...)`, `!nonnull()`, `!align(n)`,
`!noalias("domain", "scope", ...)`,
`!alias.scope("domain", "scope", ...)`, `!tbaa("tag")`,
`!profile_id(n)` and `!invariant.load()`. Range bounds are unsigned integer
literals in the instruction result type. Alias attachments name one or more
domain/scope pairs; the binary format interns these pairs in the domain and
scope tables. `!loc("path":line:column)` belongs to DEBUG, not METADATA.

GPU starts with a ULEB entry count. It is absent when SIGNATURES has no entries;
otherwise it has exactly one record per entry, in increasing SIGNATURES function
index. A record is: ULEB function index; `u8` entry kind (1 compute, 2 object,
3 mesh, 4 vertex, 5 fragment, 6 tile, 7 raygen, 8 intersection, 9 anyhit,
10 closesthit, 11 miss, 12 callable, 13 node); three ULEB thread dimensions;
a ULEB interface count and that many interface records; a ULEB payload-size
reference; ULEB threadgroup-memory bytes; ULEB maximum vertices and maximum
primitives; `u8` mesh topology; ULEB capability mask; ULEB subgroup size; `u8`
floating-point mode; and a ULEB specialization-constant count followed by
`(ULEB id, ULEB type index, ULEB CONSTANTS index)` records. The entry kind must
match SIGNATURES. Thread dimensions are all zero for a dynamic compute entry or
an entry without a `threads` attribute; otherwise all three are positive.
Kind 13 appends a node record after the specialization constants: `u8` launch
mode (0 broadcast, 1 coalescing, 2 thread), three ULEB maximum dispatch-grid
dimensions, ULEB input-record size, ULEB output count, then per output a ULEB
STRINGS name index, `u8` kind (0 node, 1 mesh), ULEB record size and ULEB
maximum records. The record is present exactly for node entries. Output names
are nonempty and unique; grid dimensions, output sizes and maximum-record
counts are positive. Other entry kinds have no node record. A mesh output
requires capability bit 7 (`work_graphs.mesh`) as well as bit 6
(`work_graphs`).
Threadgroup-memory bytes are the exact storage required by the entry's
threadgroup `stack.alloc` instructions in its first block. In instruction order,
the running offset is aligned to each allocated type's TIR layout alignment and
advanced by its layout size times its static count (one when omitted); the final
size is rounded up to the greatest allocation alignment. The verifier checks
this value exactly. A supplied GPU `stack.alloc` count is statically known only
when it names a `const.int` result; its bit pattern is interpreted as an
unsigned integer.
Interface records for roles 0–2 are in function-parameter order; if present,
role 3 is last. Each contains a `u8` role (0 root, 1 immediate, 2 stage
parameter, 3 intersection-attributes type), a ULEB parameter index for roles
0–2, and a ULEB type index. Role 3 has no parameter index. Roles 0 and 1 occur
at most once; parameter indexes are unique and in range, and their type indexes
match SIGNATURES. Role 3 occurs exactly once for intersection entries and
nowhere else. The root type is a pointer to
a struct in the constant or device address space; the immediate type is a
struct. The stage-parameter records preserve the exact types and order of the
entry's payload, attribute, data, record, or stage-input parameters.

The payload-size reference is zero when absent and otherwise the size plus one;
it is present exactly for object entries and its value is at most 16384. Mesh
limits and topology are zero/zero/0 when absent; mesh entries require topology
1 points, 2 lines, or 3 triangles, maximum vertices at most 256, and maximum
primitives at most 512. Capability bits 0–18 follow the optional-capability
order in section 17.18; higher bits are zero. Subgroup size is zero when absent
or a power of two from 4 through 128. Floating-point mode is 0 absent, 1
precise, 2 fast, or 3 ieee_denormals, and equals the entry's SIGNATURES mode.
Specialization constants are ordered by
strictly increasing `id`; ids fit `u32`, types are valid, and each referenced
default constant has the same type. Thread dimensions, payload size, threadgroup
memory, mesh limits, and specialization defaults are checked against the entry
and its code by the verifier.

MODULE is, in order: the ULEB STRINGS index of the module name; a `u8` producer-present flag (0 absent, 1 present); when present, ULEB STRINGS indexes for producer name and version, a ULEB language count and that many language indexes; a `u8` default-semantics code (0 defined, 1 permissive); a ULEB attribute count and that many key/value STRINGS-index pairs; a ULEB initializer count and that many function-index ULEBs followed by `u16` little-endian priorities; a ULEB finalizer count and records of the same form; and, for the driver profile only, the raw 32-byte BLAKE3-256 source-reproducibility hash. The producer-present flag and all indexes are checked; the record ends immediately after its last field.

IMPORTS starts with a ULEB module count. Each record is a `u8` module kind (0 RIDL, 1 package, 2 platform library), a ULEB module-path or library-name STRINGS index, 16 ABI UUID bytes, a `u8` flags field, and a ULEB entity count. Each entity is a `u8` kind (0 function, 1 global, 2 type, 3 witness, 4 descriptor, 5 platform data), a ULEB name STRINGS index, a ULEB platform-symbol STRINGS index for platform-library imports only, and a ULEB type index. It then carries a ULEB count and that many STRINGS indexes for effective `@Requires` entitlement names, followed by a ULEB count and that many STRINGS indexes for LibTessera-implied entitlement names. Both lists contain nonempty names in strict byte-lexicographic order. They are empty except on RIDL function imports; the second list is empty for non-LibTessera modules. The RIDL producer copies the first list from the imported function's `@Requires` attributes and its containing type's attributes, and the second from the matching LibTessera.md function entry. RIDL UUIDs are nonzero; package and platform UUIDs are zero. Only platform imports may set flags bit 0 (`weak`), and all other flag bits are zero. Platform imports contain only functions and platform data.

ASM starts with a ULEB record count. Each record is a ULEB STRINGS index for the template followed by a ULEB STRINGS index for the constraint string. Records are the distinct template/constraint pairs used by CODE, ordered by their first reference while traversing functions, blocks and instructions in section order. Both indexes are checked, and the section ends after the last record.

ANALYSIS starts with a ULEB defined-function count equal to SIGNATURES. Each function in SIGNATURES order has a `u8` trust class (0 managed, 1 sandboxed, 2 unmanaged) and a `u8` reason mask (bits 0–7 from section 21.4). A ULEB sandbox-region count follows; each record is a ULEB linear-memory index, ULEB GLOBALS base index and ULEB GLOBALS current-size index. There is exactly one record per linear memory; indices start at zero and are contiguous. Every base and current-size global index is distinct across all records. A ULEB indirect-call type count follows, then strictly increasing little-endian `u64` typeids, one for every distinct function type used by an indirect call. A ULEB assembly-region count follows; each record is a ULEB SIGNATURES function index, ULEB zero-based instruction ordinal in flattened block order, ULEB ASM record index, `u8` architecture code (1 aarch64, 2 x64, 3 rv64) and `u8` effects (bit 0 sideeffect, bit 1 memory, bit 2 pure, bit 3 privileged; other bits zero). Assembly-region records are ordered by function and instruction ordinal. The section is required even when all counts are zero and ends after the last record.

TYPES is three consecutive parts. Part A starts with a ULEB declaration count. Each nominal declaration is a ULEB name STRINGS index, a ULEB generic-parameter count, then for each parameter a name STRINGS index, a ULEB bound count and that many bound-name STRINGS indexes, followed by a `u8` kind (0 `struct`, 1 `enum`, 2 `explicit`). Part B starts with a ULEB type-entry count, followed by that many entries, each beginning with its section 22.7 `u8` tag and the payload listed there. A type entry may refer only to earlier type entries; the function index in `frame @f` is the exception because it indexes SIGNATURES, not TYPES. The optional error type in a function entry is encoded as 0 for absent and as type-index + 1 when present. Part C starts with a ULEB definition count. Each definition is a declaration index, a layout code and its parameters, the declaration-kind-specific body, then the policy record. Layout codes are 0 `ridl`, 1 `c`, 2 `packed` followed by ULEB pack alignment, and 3 `explicit` followed by ULEB size and ULEB alignment. A `struct` or `explicit` body is a ULEB field count followed by field records; an `enum` body is a ULEB variant count followed by variant records. A field record is a name STRINGS index, type index, `u8` flags (bit 0: byte offset present; bit 1: bit offset and width present; all other bits zero), the optional ULEB byte offset, optional ULEB bit offset and width, then a ULEB annotation count and that many annotations. Each annotation is a name STRINGS index, a ULEB argument count and that many STRINGS indexes containing the canonical literal spellings from Appendix A.1. Bit offset and width occur together and require a byte offset. A variant record is a name STRINGS index, a ULEB payload count and that many type indexes. Definitions occur at most once and in increasing declaration-index order. An explicit layout requires every field to carry a byte offset; an `enum` cannot use the explicit layout rule because variant payload offsets are not represented.

The policy record is six `u8` fields in this order: copy (0 trivial, 1 implicit plus function-index ULEB, 2 explicit plus function-index ULEB, 3 none), destroy (0 trivial, 1 function plus function-index ULEB), relocate (0 bitwise, 1 function plus function-index ULEB), exclusivity (0 static, 1 dynamic, 2 none), pairing (0 counted, 1 none), and lifetime (0 eager, 1 scoped). Policy function indexes address SIGNATURES. Every string and type index is range-checked; declaration, generic-parameter, bound, field and variant names are nonempty. Annotation literal spellings are valid Appendix A.1 literals. The three part counts, every nested count, and each type-entry reference obey section 22.11 limits. The section ends after the last policy record.

CONSTANTS starts with a ULEB record count, less than 2^32. Each record is a ULEB type index and a `u8` value form: 0 scalar, 1 zero, 2 bytes, 3 string, 4 aggregate, 5 enum, 6 null pointer, 7 sampler, 8 union member. Form 0 is valid only for integer and float types. An integer is its unsigned N-bit pattern as a minimal ULEB, where N is the `iN` width; negative textual values are reduced modulo 2^N. A float is exactly `ceil(N/8)` little-endian raw-bit bytes, with unused high bits zero. Form 1 has no payload and is a zero value of a trivial type. Form 2 is a ULEB byte count followed by that many bytes; it is valid only for a trivial type and the byte count equals its section 8 size. Form 3 is a ULEB STRINGS index and requires type `(ptr, i64)`; the string table value is the UTF-8 payload. Form 4 is valid for tuples, arrays, fixed vectors and defined `struct` nominal types; it recursively encodes one value per element, lane or field in declaration order, with no count because the type determines the shape. Form 5 is valid for a defined enum nominal type; it carries a ULEB variant index and recursively encodes that variant's payload values in order. Form 6 has no payload and is valid only for a pointer type. Form 7 carries the fixed 23-byte sampler descriptor of section 22.5 and requires type `sampler`. Form 8 is valid for a defined `explicit` (union) nominal type; it carries a ULEB member index and recursively encodes one value of that member's type. Recursive values use the same form tags; every encoded type and string index is checked, and nesting is limited by section 22.11. The section ends after the last record.

GLOBALS starts with a ULEB record count, less than 2^32. Each record is a ULEB STRINGS name index, a `u8` linkage (0 private, 1 module, 2 package, 3 exported), a ULEB TYPES index, a `u8` address space (0 generic, 1 device, 2 constant, 3 threadgroup, 4 thread, 5 object_payload, 6 ray_payload, 7 hit_attribute, 8 tile, 9 node_record), a `u8` flags field (bit 0 `thread_local`, bit 1 `constant`, bit 2 `percpu`; all other bits zero), a ULEB alignment, and a ULEB initializer reference. Address space 0 is the default when text omits `<space>`. Alignment 0 means omitted; nonzero alignment must be a power of two. Initializer 0 means absent; otherwise it is CONSTANTS index + 1. Names must be nonempty and unique, and all indexes must be in range. Records preserve module declaration order. The section ends after the last record.

SIGNATURES starts with a ULEB function count, less than 2^32. Each record is a ULEB STRINGS name index, a `u8` linkage (0 private, 1 module, 2 package, 3 exported), a ULEB TYPES index whose entry is a function type, a ULEB generic-parameter count followed by named parameter records (name index, bound count, bound-name indexes), a ULEB function modifier word, and a `u8` extra-flags field. Function modifier bits 41–45 use section 22.8 for `cold`, `nounwind`, `noreturn`, `inline` and `noinline`; all other bits are zero. Extra flag bits 0–3 are `structured`, `kernel.fp`, `returns_twice` and `inlinable`. The modifier and extra-flag sets may not request both `inline` and `noinline`. The record then carries `u8` engine (0 native, 1 runtime, 2 any, 3 absent for GPU-only functions), `u8` semantics (0 inherit module default, 1 defined, 2 permissive), `u8` target (0 CPU, 1 GPU, 2 both), `u8` floating-point mode (0 absent, 1 precise, 2 fast, 3 ieee_denormals), and `u8` generic strategy (0 absent, 1 specialize, 2 polymorphic, 3 both). A `u8` coroutine-present flag (0 absent, 1 present) is followed, when present, by yield and resume type indexes. A ULEB instance-type count and its type indexes follow. A `u8` variant-present flag (0 absent, 1 present) is followed, when present, by architecture (1 aarch64, 2 x64, 3 rv64, 4 wasm), feature count and feature-name indexes. Entry kind is a `u8` (0 ordinary function, 1 compute, 2 object, 3 mesh, 4 vertex, 5 fragment, 6 tile, 7 raygen, 8 intersection, 9 anyhit, 10 closesthit, 11 miss, 12 callable, 13 node). The final field is a ULEB STRINGS index for the platform export name; empty means absent. CPU and both-target functions require an engine; GPU-only functions and entries omit it. Entries have a nonzero entry kind and GPU target; ordinary functions have entry kind 0. A platform export name must be nonempty and use a C-ABI function type. Function names and generic parameter names are nonempty and unique within the signature; bounds and feature names are nonempty. Every string, type and count is range-checked. A `frame @f` type index must address an existing SIGNATURES record. Records preserve module declaration order and the section ends after the last record.

VARIANTS starts with a ULEB set count. Each set contains a nonempty STRINGS name index, a nonzero ULEB member count and that many member records. A member is a u8 architecture (0 portable, 1 aarch64, 2 x64, 3 rv64, 4 wasm), a ULEB feature count and that many STRINGS indexes, then a ULEB SIGNATURES function index. Feature names are nonempty and strictly lexicographically ordered; portable members have no features. Set names, (architecture, feature-list) keys within a set, and function indexes across the section are unique. Every member function is CPU-only with engine native, and its SIGNATURES variant attribute matches the member architecture and feature list; portable members have no variant attribute. Sets and members preserve module declaration order. The section is absent when there are no variant sets and ends after the last member.

Feature names in a SIGNATURES variant attribute use the same strict lexicographic order as VARIANTS members.

CODE starts with a ULEB defined-function count, less than 2^32. Each record is a ULEB SIGNATURES function index followed by a ULEB body size and that many body bytes. The body size counts from the first byte after the size field through the end of the final block, including the block count. The bounded body starts with a ULEB block count in 1..2^24; the remainder is the concatenation of the blocks encoded by section 22.5. Function indexes are in strictly increasing SIGNATURES order. The section ends after the last body.

GENERICS is absent when the module has no trait definitions, conformances or
vtables. Otherwise it starts with a ULEB trait count, followed by trait records
in declaration order. A trait record contains a nonempty STRINGS index for its
canonical path without a leading `%`, a ULEB generic-parameter count and the
named parameter records used by SIGNATURES, a ULEB parent count and that many
STRINGS indexes for canonical parent-trait paths without a leading `%`, a ULEB
method-requirement count and that many pairs of a nonempty
method-name STRINGS index and a TYPES index whose entry is a function type,
then a ULEB associated-type count and that many associated-type records. Each
associated-type record is a nonempty STRINGS name index, a ULEB bound count and
that many STRINGS indexes for qualified trait paths without leading `%`.
Generic type indexes in a
method requirement use the trait's generic-parameter order. Trait names, method
names and associated-type names are unique in their respective scopes; parent
paths and bounds are unique. Trait records and their nested lists preserve text
declaration order.

A ULEB conformance count follows. Each conformance record contains a TYPES type
index, a nonempty STRINGS index for the canonical trait path without its
leading `%`, a ULEB count of associated-type bindings and that many pairs of
nonempty associated-type name STRINGS indexes and TYPES indexes, then a ULEB
witness count and that many pairs of nonempty method-name STRINGS indexes and
SIGNATURES function indexes. A conformance's (type index, trait path) pair is
unique. Associated-type names and witness method names are unique within their
lists; conformances and their nested lists preserve text declaration order.
When the corresponding trait definition is present, each list names exactly
its required associated types or methods. The witness function's function type
is checked according to section 18.2.

A ULEB vtable count follows. Each vtable record contains a nonempty STRINGS
name index, a ULEB slot count, then that many pairs of a ULEB slot number and a
SIGNATURES function index. Vtable names are unique, slot numbers are strictly
increasing, and function indexes are in range. The section ends after the last
record. Trait method type indexes and conformance type indexes are range-checked;
every count obeys section 22.11 limits. The generic strategy of each generic
function is the `generic strategy` byte in its SIGNATURES record.

### 22.5 Instruction encoding

Each function body contains its declared number of blocks in block-index order. Each block is a ULEB byte size followed by that many block bytes; the size excludes its own ULEB and includes all fields below. A block starts with a `u8` flags field (bit 0: cold; bit 1: loop header; all other bits zero), a ULEB parameter count, then each parameter's `u8` convention/context field and type index. Convention codes are 0 owned, 1 borrow, 2 borrowmut, 3 trivial, and 4 omitted/default; bit 7 marks context and bits 3–6 are zero. Block zero's parameter count and parameter records match the function type's parameters. A loop header then carries ULEB merge-block and continue-block indexes. The block ends with a ULEB instruction count in 1..2^24 and exactly that many instructions. Its final instruction MUST be a terminator, and no earlier instruction may be one. Every block index is the zero-based position in this list. Loop targets and block-target operands are checked against the function's block count. `cond.br` and `switch.int` always carry a ULEB selection-merge field after their ordinary successors; zero means no `merge` clause, otherwise the value is the merge block index plus one.

An instruction is: ULEB opcode, ULEB modifier word (section 22.8), ULEB result count, then its operand fields in the order of its textual syntax (Appendix A.1). The result count is the number of SSA values defined by the instruction, not the number of tuple elements; it is checked against the instruction's declared result shape, and a result of `unit` defines zero values. Field encodings:

| Field | Encoding |
| --- | --- |
| Value | ULEB of (current value number − operand value number). Values are numbered in order: first-block parameters (the function parameters), then for each subsequent block its parameters and each instruction result, in block order. First-block parameters are counted once. Rule V1 guarantees every operand precedes its use |
| Type | ULEB type index |
| Block target | ULEB block index, ULEB argument count, argument values |
| Symbol | ULEB index with the low two bits tagging the space: 0 function, 1 data object, 2 variant set or dispatch set, 3 reserved; the remaining bits are the index within that space |
| Immediate | Integer literal: `u8` sign (0 nonnegative, 1 negative), then minimal ULEB128 magnitude; zero uses sign 0. Floating-point immediates use a constant index |
| List | ULEB count, then elements |

Function symbol indexes enumerate imported function entities in IMPORTS module
and entity order, followed by SIGNATURES records in record order. Data symbol
indexes enumerate imported global and platform-data entities in IMPORTS module
and entity order, followed by GLOBALS records in record order.

For `const.int T, n`, the type is a type index followed by `ceil(N/8)` bytes containing the low N bits of n in little-endian order, where T is `iN`. A negative n is represented modulo 2^N. Unused high bits of the last byte are zero. The literal must fit T. This type-directed payload distinguishes signed and unsigned spellings without adding an encoding tag. Other integer immediates use the sign-and-magnitude encoding above; their instruction rule supplies the range.

`const.sampler` stores a fixed 23-byte descriptor, independent of field spelling order: bytes 0–2 are min, mag and mip filters (0 nearest, 1 linear); bytes 3–5 are U, V and W address modes (0 clamp-to-edge, 1 repeat, 2 mirrored-repeat, 3 clamp-to-border, 4 mirror-clamp-to-edge); byte 6 is border color (0 transparent black, 1 opaque black, 2 opaque white); byte 7 is anisotropy (1–16); byte 8 is compare (0–7 in the listed order, 255 disabled); byte 9 is reduction (0 weighted-average, 1 min, 2 max); byte 10 is normalized coordinates (0 false, 1 true); bytes 11–14, 15–18 and 19–22 are `lod_min`, `lod_max` and `lod_bias` as IEEE-754 binary32 bit patterns in little-endian order. The defaults in section 17.6 are materialized in this record.

The TIR 0x200 atomic range carries `atomic.load` as a type index and pointer value, `atomic.store` as value and pointer, `atomic.rmw` as pointer and value, `atomic.cmpxchg` as pointer, expected value and new value, and `fence` with no operands. `atomic.wait` carries pointer, expected value and timeout; `atomic.notify` carries pointer and count. `atomic.rmw` stores its operation in selector bits 48–51: 0 `xchg`, 1 `add`, 2 `sub`, 3 `and`, 4 `nand`, 5 `or`, 6 `xor`, 7 `max`, 8 `min`, 9 `umax`, 10 `umin`, 11 `fadd`, 12 `fsub`, 13 `fmax`, 14 `fmin`, 15 `uinc_wrap`, 16 `udec_wrap`, 17 `usub_cond`, 18 `usub_sat`; higher values are invalid. Other atomic instructions require selector 0. Bit 29 carries `weak` only on `atomic.cmpxchg`; order and scope use their fields in section 22.8. `atomic.load` and `atomic.rmw` return `T`, `atomic.cmpxchg` returns `(T, i1)`, `atomic.wait` and `atomic.notify` return `i32`, and store/fence have no result. `atomic.wait` and `atomic.notify` lower to runtime services and are not retained in TVISA.

GPU instructions in range 0x400 encode `gpu.builtin` as a STRINGS name index and type index; `gpu.spec_const` as a ULEB 32-bit ID, type index and CONSTANTS index; and `gpu.barrier`/`gpu.memfence` as a `u8` memory-domain mask after the modifier word. The mask bits, in order, are device, threadgroup, texture, tile and object-payload; omitted memory clauses use `0x1f`. Scope uses modifier bits 26–28 with section 22.8's codes; code 0 means the instruction-specific default. `gpu.call.vft` carries its two leading value operands, a signature index derived from the table type, then a counted argument list. `frag.demote`, `frag.raster_order.begin`, `frag.raster_order.end` and `frag.imageblock.addr` have no operands; `frag.load_attachment` carries a sign-and-magnitude unsigned 32-bit attachment index; `gpu.printf` carries a STRINGS format index and a counted value list.

The 0x420 range carries one value for `gpu.ddx`, `gpu.ddy`, `gpu.fwidth`, `quad.swap`, `quad.all` and `quad.any`, and two values for `quad.broadcast` (value then `i32` lane). `gpu.ddx` and `gpu.ddy` use modifier bit 36 for `fine`; zero means `coarse`. `quad.swap` uses selector bits 48–51 (0 x, 1 y, 2 diagonal; 3–15 are invalid). All return the input type except `quad.all` and `quad.any`, which return `i1`.

The 0x440 range assigns opcodes consecutively in section 17.20's order. `subgroup.elect` has no value operands; `subgroup.broadcast` has two; `subgroup.shuffle`, `shuffle_xor`, `shuffle_up`, `shuffle_down` and `rotate` have two; all other operations through `scan_exclusive` have one. `subgroup.reduce`, `scan_inclusive` and `scan_exclusive` then carry a ULEB operator code (0–14 in the order listed in section 17.8) and a ULEB cluster size (0 means omitted). All subgroup operations return the input type except the predicates and ballot-query instructions: `elect`, `all`, `any`, `all_equal`, `inverse_ballot` return `i1`; `ballot` returns `vec<4 x i32>`; and `ballot_bit_count`, `ballot_find_lsb`, `ballot_find_msb` return `i32`.

The 0x480 texture range assigns opcodes in section 17.20 order. Every texture instruction carries a ULEB value count and that many value deltas, followed by a ULEB offset-component count (zero when omitted) and one sign-and-magnitude integer per offset component. Offsets are limited to −8 through 7. `tex.atomic` then carries a ULEB operation code 0–14 in the order listed in section 17.6. `tex.gather` uses modifier selector bits 48–51 for its component (0–3, default 0); `tex.query` uses those bits for its field: 0 width, 1 height, 2 depth, 3 size, 4 layers, 5 levels, 6 samples; 7–15 are invalid. Other texture instructions require selector 0. The `sparse` modifier is bit 35 and adds a trailing `i1` result on the sparse-capable instructions listed in section 17.6. `tex.write` has no result; `tex.lod` returns `vec<2 x f32>`; query result shape follows the query and texture dimension; other results follow section 17.6.

The 0x560 range assigns consecutive opcodes in section 17.20 order. `coop.fill` carries a type index and one value; `coop.load` a type index and two values; `coop.store` three values; `coop.mma` three values; `coop.convert` one value followed by a target type index; `coop.length` a type index; `coop.extract` two values; `coop.insert` three values; `coop.reduce` one value followed by an operator code (0–14 in the order of section 17.8); `tensor.load` two values; `tensor.store` three values; and `tensor.query` one value followed by a sign-and-magnitude dimension immediate. `coop.load` and `coop.store` use selector bits 48–51: 0 row-major, 1 column-major; the default is row-major. Tensor-form `coop.load` requires selector 0. `coop.reduce` uses selector 0 row, 1 column, 2 quad2x2. `tensor.query` uses selector 0 extent or 1 stride. Other selectors are invalid. `coop.mma` uses bit 2 for unsigned integer inputs and bit 46 for saturation; both default off. Results are the declared type for `coop.fill`/`coop.load`, the accumulator type for `coop.mma`, the target type for `coop.convert`, `i32` for `coop.length`, the matrix element type for `coop.extract`, the input matrix type for `coop.insert`/`coop.reduce`, the tensor element type for `tensor.load`, `i64` for `tensor.query`, and none for stores.

The 0x4C0 range carries three values for `object.dispatch_mesh`, two for each mesh instruction, and no results. `tile.imageblock.addr` carries one value and returns `ptr<tile>`. The 0x5C0 range carries one value for `node.input.addr`, none for `node.input.count`, two for `node.output.allocate` and `node.output.addr`, one for `node.output.commit`, and none for `node.finished_cross_group`. `node.output.allocate` uses selector bits 48–51 (0 thread, 1 group); other values are invalid. Results are `ptr<node_record>` for input/output addresses and allocations, `i32` for `node.input.count`, and `i1` for `node.finished_cross_group`.

The 0x500 ray range assigns opcodes in section 17.20 order. `rq.init` carries eight values; `rq.proceed`, `rq.commit_triangle` and `rq.abort` one; `rq.commit_procedural` two; `rq.get` one followed by its ULEB attribute code; `rt.trace` eleven; `rt.report_hit` three; `rt.ignore_hit`, `rt.accept_and_end` and `rt.hitobject.miss` none; `rt.call_callable` two; `rt.get` its ULEB attribute code; `rt.hitobject.trace` ten; `rt.hitobject.from_query` one; `rt.hitobject.invoke` two; `rt.hitobject.get` one followed by its ULEB attribute code; `rt.reorder` a ULEB value count (one or two), those values, then a ULEB key width (zero when omitted). `rq.get` uses attribute codes 0–11 in its listed order and modifier selector 0 for candidate or 1 for committed. `rt.get` uses codes 0–16 in its listed order. `rt.hitobject.get` uses the `rq.get` codes. The two any-hit terminators retain terminator status in CODE.

The `fcvt` instruction uses selector bits 48–51 for its mode: 0 `sat` (also the default), 1 `satfinite`, 2 `nonsat`; higher values are invalid. Rounding uses bits 13–15 as for the other floating-point conversions.

`const.bool` stores its 0 or 1 literal as a sign-and-magnitude immediate. `const.null` always stores one `u8` address-space code after the modifier word; an omitted space is code 0 (generic). `const.bytes T, b"..."` stores a type index, a ULEB byte count and that many decoded bytes.

`icmp` stores its predicate in selector bits 48–51 using codes 0–9 in the order `eq`, `ne`, `slt`, `sle`, `sgt`, `sge`, `ult`, `ule`, `ugt`, `uge`. `fcmp` uses codes 0–15 in the order listed in section 12.3. All of these codes are valid for their respective instruction.

`const.vec` stores its type index followed by one scalar bit payload per lane in lane order, without inter-lane padding, when the vector's storage size is at most 16 bytes. Larger vectors store a CONSTANTS index instead. An `align n` operand is always a ULEB byte alignment; zero means the clause was omitted.

`const.aggregate` stores a type index and a CONSTANTS index whose type and aggregate value form match. `func.ref` stores a function symbol and a counted list of generic type indexes. `global.addr` and `tls.addr` store a data-object symbol; `descriptor.ref` stores a type index; `witness.ref` stores a type index and a STRINGS index naming the trait; `block.addr` stores a block index; `gpu.entry.ref` stores a function symbol. Each defines one result and requires zero modifiers.

The 0x0C0 vector range stores operands in textual order. `vec.splat` carries a value and type index; `vec.extract` two values; `vec.insert` three; `vec.shuffle` two values followed by a ULEB index count and that many unsigned sign-and-magnitude lane indices; `vec.step` a type index; and `vec.vscale` no operands. `vec.reduce` carries its vector and, when modifier bit 32 is set, its mask. `vec.load` carries a type index, pointer value, alignment, then an optional mask and passthrough when bit 32 is set. `vec.store` carries vector and pointer values, alignment, then an optional mask. `vec.gather` carries a result type and either one vector-of-pointers value or a base pointer and offsets vector plus ULEB scale; `vec.scatter` carries its value vector followed by the same address form. Bit 32 adds mask and passthrough to gather, and mask to store and scatter.

`vec.compress`, `vec.expand`, and `vec.dot` carry three values; `vec.zip`, `vec.unzip`, `vec.concat`, and `vec.table` carry two; `vec.reverse` and `vec.split` carry one; `vec.slide` carries three; `vec.mask.active` carries two; and `vec.mask.count` and `vec.mask.first` carry one. Selector bits 48–51 encode `vec.reduce` operators 0–14 in section 16.3 order; gather/scatter address forms (0 vector of pointers, 1 base plus offsets and scale); `vec.dot` kinds (0 `sdot`, 1 `udot`, 2 `usdot`, 3 `bfdot`); `vec.zip` (`0 lo`, `1 hi`); `vec.unzip` (`0 even`, `1 odd`); `vec.slide` (`0 up`, `1 down`); and `vec.split` (`0 lo`, `1 hi`). An omitted optional alternative selects 0. Every other vector instruction requires selector 0; out-of-range selectors are invalid.

`vec.splat`, `vec.step`, `vec.load`, and `vec.gather` return their encoded type. `vec.extract` and `vec.reduce` return the input element type. `vec.vscale`, `vec.mask.count`, and `vec.mask.first` return `i64`; `vec.mask.active` returns `svec<i1>`. `vec.shuffle` returns a fixed vector with the input element type and one lane per shuffle index. `vec.concat` returns a fixed vector with the common input element type and the sum of their lanes; `vec.split` returns a fixed vector with the input element type and half the input lane count. `vec.table` returns an `i8` vector with the index vector's shape. `vec.store` and `vec.scatter` have no result. The remaining vector instructions return the first value operand's type.

The retained 0x140 memory instructions carry `raw.load` as type, pointer and alignment; `raw.store` as value, pointer and alignment; `ptr.offset` and `ptr.diff` as two values; `mem.copy`, `mem.move` and `mem.set` as three values; and `prefetch` as one pointer value. `mmio.load` carries type and pointer; `mmio.store` carries value and pointer; `fence.io` has no value operands. Each raw/MMIO alignment is a ULEB byte count, with zero for an omitted clause. `prefetch` uses selector 0 for read and 1 for write (default 0), bits 0–1 for locality 0–3 (default 0), and bit 2 for instruction prefetch (zero means data); other selectors are invalid. `fence.io` uses selector 0 read, 1 write, 2 full; other selectors are invalid. MMIO order uses bits 20–22: 0 means omitted/default relaxed, load accepts 1 relaxed or 2 acquire, and store accepts 1 relaxed or 3 release. Other memory instructions require selector 0. Raw/MMIO loads return their encoded type; `ptr.offset` returns its input pointer type; `ptr.diff` returns `i64`; the other listed operations have no result. MMIO types have size 1, 2, 4 or 8 bytes. `kernel.percpu.addr`, `foreign.addr`, `foreign.data` and `foreign.entry` each carry one symbol index. `kernel.percpu.addr` returns `ptr`, `foreign.addr` and `foreign.entry` return `code`, and `foreign.data` returns `ptr`. The per-CPU symbol must be a defined data object carrying the TVISA per-CPU flag; `foreign.addr` and `foreign.data` must name platform imports, while `foreign.entry` names a defined C-ABI function.

`target.intrinsic` stores its name as a STRINGS index, its arguments as a counted list of value deltas, then its result type index. `target.has` stores its feature as a STRINGS index; `target.pagesize` has no operands. `intrinsic` stores its unsigned ID followed by a counted list of value deltas; the count must match the ID's signature in section 12.18. `debug.value` and `debug.addr` store a value delta followed by the zero-based index of the variable record in DEBUG; `debug.label` stores the zero-based index of its DEBUG label record. `assume` stores one value delta, and `nop` has no operands.

`expect` stores a value delta, the expected integer in sign-and-magnitude form, then a ULEB probability-present flag (0 absent, 1 present). When present, probability is four little-endian bytes containing an IEEE-754 binary32 value; it MUST be finite and in `[0, 1]`.

`guard` stores its condition value, a function symbol for the fallback, then a ULEB state count and that many state value deltas. `deopt` stores the same fallback symbol and state list without a condition. Both require zero result count and zero modifiers; `guard` continues the block on its true path, while `deopt` is a terminator.

`landingpad` stores a ULEB clause count followed by ordered clauses: `u8` kind 0 (`catch.type`) plus a type index, kind 1 (`catch.filter`) plus a function symbol, kind 2 (`catch.all`) with no payload, or kind 3 (`cleanup`) with no payload. It defines one tuple SSA value. `resume` stores one value and is a terminator; `throw` stores a value and type index and is a terminator; `exception.payload` and `exception.typeid` each store one value and define one result.

In the 0x210 range, `va.start` and `va.end` each carry one list value; `va.arg` carries a type index and one list value; `va.copy` carries destination and source list values; `setjmp` carries one buffer value and defines one result; `longjmp` carries buffer and value operands and is a terminator. These instructions require zero modifiers.

Call records use counted value lists. `call` stores a function symbol, counted generic type indexes and counted arguments; `call.indirect` stores the function value, function type index and arguments. `closure.new` stores code and context values; `call.closure` stores the closure value and arguments; `closure.callback` stores a ULEB parameter index and function type index. `call.witness` stores a type index, a STRINGS index for the qualified `Trait.method` name, and arguments. `call.vtable` stores the object value, a data-object vtable symbol, ULEB slot and arguments. `call.foreign` stores a function symbol and arguments. `call.variadic` stores a function symbol, function type index, fixed-argument count, variadic-argument count, then those two value lists. `try.call` and `invoke` store a function symbol, arguments and two block indexes. `try.call` supplies its normal result and owned error to the corresponding target parameters; `invoke` supplies its normal result, while its unwind target has no parameters and begins with `landingpad`. A `unit` or `never` result contributes no target parameter. `tail.call` uses the `call` fields and is a terminator. Ordinary calls define zero or one result as required by their function type; `closure.new` and `closure.callback` define one; `try.call`, `invoke` and `tail.call` define none. Call attributes use bits 41–45; `inline` and `noinline` are mutually exclusive. Modifier bit 46 means `foreign_unwind` only on `invoke` and `saturate` only on `fcvt`.

In the 0x1C0 coroutine range, `coro.start` stores a function symbol and counted arguments; selector 0 allocates on the heap and selector 1 is followed by the inline frame-slot value. `coro.yield` stores its yielded value and resume/destroy block indexes. `coro.resume` stores handle and resume values followed by yielded/done block indexes. These block targets have no explicit argument list; their coroutine-provided values correspond to the block parameters in section 13.2. `coro.finish` and `async.waker` have no operands; `coro.destroy`, `coro.done` and `await` store one value; `coro.frame.size` and `coro.frame.align` store a function symbol. Only `coro.start` uses selector bits 48–51, and selectors above 1 are invalid.

`asm` stores a ULEB index into ASM, a counted list of value deltas for its inputs, and a result type index. A `unit` result has no SSA result; every other result type has one. `asm.goto` stores a ULEB ASM index, a counted list of input value deltas, the normal successor's block index, a ULEB label count and that many label block indexes. These successors carry no block arguments, so every destination block has zero parameters. The normal successor and labels are all control-flow edges.

Instructions that produce no value do not advance the value numbering. A tuple result is one value.

`stack.alloc` uses selector bits 48–51: 0 omits `<space>` and selects generic space for CPU and `target(cpu, gpu)` functions or `thread` for GPU-only functions; 1–9 select `device`, `constant`, `threadgroup`, `thread`, `object_payload`, `ray_payload`, `hit_attribute`, `tile` and `node_record` respectively; 10 explicitly selects generic space; 11–15 are invalid. It stores its type index, a ULEB count-present flag (0 absent, 1 present), and, when present, one value delta for `count %n`. GPU and kernel count operands must refer to a `const.int` result whose unsigned value is below 2^62; an absent count means one. Address operations encode field and variant indexes as ULEB immediates and address casts as type indexes; their value operands use the value-delta encoding above.

In the 0x160 heap range, `heap.alloc.counted` and `heap.alloc.unique` carry a type index; `ref.addr`, `uniq.addr`, `weak.new`, `weak.load`, `ref.unowned` and `ref.isunique` carry one value; `heap.alloc.raw` carries size and alignment values; `heap.realloc.raw` carries pointer, size and alignment; and `heap.free.raw` carries one pointer. Only `heap.alloc.raw` accepts modifier bit 30 (`uninit`). All except `heap.free.raw` define one SSA value.

`access.begin` uses selector bits 48–51: codes 0–5 are the Cartesian product of access mode (read 0, modify 1) and checking mode (static 0, dynamic 1, unchecked 2), with the access-mode code as the low bit. Omitted options select read/static (code 0). Other instructions in the 0x180 ownership range require selector 0.

`br.indirect` stores its address value, a ULEB target count, and that many block indexes; these target blocks have no arguments. `switch.enum` stores its enum value, a ULEB case count, each strictly increasing variant index and target block index, then a ULEB default target encoded as zero when absent or block index plus one when present. Enum payloads arrive as the target block's parameters.

### 22.6 Opcode assignment

Within each range, opcodes are assigned consecutively starting at the range base, in the order the instructions appear in the tables of the section named.

| Range | Section |
| --- | --- |
| 0x000 | 12.1 Constants |
| 0x020 | 12.2 Integer |
| 0x060 | 12.3 Floating point and mathematics |
| 0x0A0 | 12.4 Conversions |
| 0x0C0 | 16.3 Vectors |
| 0x100 | 12.6 Aggregates |
| 0x120 | 12.7 Storage and addresses |
| 0x140 | 12.8 Memory |
| 0x160 | 12.9 Heap and references |
| 0x180 | 12.10 Ownership (order as listed) |
| 0x1A0 | 12.11 Calls |
| 0x1C0 | 13.8 Coroutines and async |
| 0x1D0 | 14.5 Unwinding |
| 0x1E0 | 12.14 Terminators |
| 0x200 | 15.6 Atomics |
| 0x210 | 12.16 Variadics and non-local jumps |
| 0x220 | 12.17 Target queries and architecture-specific |
| 0x230 | 12.18 Portable intrinsics |
| 0x240 | 12.19 Debug and hints |
| 0x250 | 12.20 Generic support (order as listed) |
| 0x260 | 12.21 Speculation |
| 0x270 | 12.22 Device I/O and kernel |
| 0x280 | 12.23 Platform interoperation |
| 0x400 | 17.20 Builtins and control |
| 0x420 | 17.20 Derivatives and quads |
| 0x440 | 17.20 Subgroups |
| 0x480 | 17.20 Textures |
| 0x4C0 | 17.20 Object and mesh |
| 0x500 | 17.20 Ray tracing |
| 0x560 | 17.20 Cooperative matrices and tensors |
| 0x5A0 | 17.20 Tile |
| 0x5C0 | 17.20 Work graphs |

Unassigned opcodes are reserved. A later TIR version adds instructions at the end of a range or in a new range; it never renumbers.

### 22.7 Type tags

| Tag | Type | Payload |
| --- | --- | --- |
| 0x01 | `iN` | ULEB N |
| 0x02 | `f16` | |
| 0x03 | `bf16` | |
| 0x04 | `f32` | |
| 0x05 | `f64` | |
| 0x06 | `f128` | |
| 0x07 | `f8e4m3` | |
| 0x08 | `f8e5m2` | |
| 0x09 | `f6e2m3` | |
| 0x0A | `f6e3m2` | |
| 0x0B | `f4e2m1` | |
| 0x0C | `e8m0` | |
| 0x10 | `ptr` / `ptr<space>` | `u8` space: 0 generic, 1 device, 2 constant, 3 threadgroup, 4 thread, 5 object_payload, 6 ray_payload, 7 hit_attribute, 8 tile, 9 node_record |
| 0x11 | `*T` | Type index, `u8` space |
| 0x12 | `unit` | |
| 0x13 | `never` | |
| 0x20 | `vec<N x T>` | ULEB N, type index |
| 0x21 | `svec<T>` | Type index |
| 0x22 | Tuple | ULEB count, type indices |
| 0x23 | Array | ULEB N, type index |
| 0x24 | Nominal | Nominal declaration index, ULEB argument count, type indices |
| 0x25 | Generic parameter | ULEB parameter index |
| 0x26 | `ref T` | Type index |
| 0x27 | `uniq T` | Type index |
| 0x28 | `weak T` | Type index |
| 0x30 | Function | ULEB parameter count; per parameter: `u8` convention (0 owned, 1 borrow, 2 borrowmut, 3 trivial) with bit 7 context, type index; result type; error type or 0; `u8` flags (bit 0 async, bit 1 abi(c), bit 2 foreign) |
| 0x31 | Closure | Function type index, `u8` borrowed |
| 0x32 | `coro<Y, U, R>` | Three type indices |
| 0x33 | `frame @f` | Function index |
| 0x40 | `texture` | `u8` dimension (0 1d, 1 2d, 2 3d, 3 cube, 4 1darray, 5 2darray, 6 cubearray), component type index, `u8` access (0 sample, 1 read, 2 write, 3 readwrite), `u8` multisampled |
| 0x41 | `sampler` | |
| 0x42 | `accel` | |
| 0x43 | `vft` | Function type index |
| 0x44 | `tensor` | ULEB rank, element type index |
| 0x45 | `rayquery` | |
| 0x46 | `hitobject` | |
| 0x47 | `coop` | `u8` scope (0 subgroup, 1 threadgroup), ULEB M, ULEB N, element type index, `u8` use (0 a, 1 b, 2 acc) |
| 0x48 | `entry` | `u8` kind, root type index, immediate type index |

### 22.8 Modifier word

The bracketed modifiers of every instruction and function attribute are encoded in one ULEB modifier word with fixed bit positions:

| Bits | Modifier |
| --- | --- |
| 0–1 | Overflow mode: 0 wrap, 1 trap, 2 sat, 3 poison |
| 2 | Unsigned (for modes that take `s` or `u`) |
| 3 | `exact` |
| 4 | `disjoint` |
| 5 | `inbounds` |
| 6–12 | Fast-math: `nnan`, `ninf`, `nsz`, `arcp`, `contract`, `afn`, `reassoc` |
| 13–15 | Rounding mode: 0 default, 1 rne, 2 rtz, 3 rup, 4 rdn, 5 rna |
| 16 | `strict` |
| 17 | `approx` |
| 18 | `volatile` |
| 19 | `nontemporal` |
| 20–22 | Memory order: 0 omitted/default, 1 relaxed, 2 acquire, 3 release, 4 acq_rel, 5 seq_cst |
| 23–25 | Failure order: 0 omitted/derived, 1 relaxed, 2 acquire, 5 seq_cst; other values invalid |
| 26–28 | Scope: 0 default, 1 thread, 2 subgroup, 3 threadgroup, 4 device, 5 system; 6–7 invalid |
| 29 | `weak` |
| 30 | `uninit` |
| 31 | `nooverlap` |
| 32 | Mask and passthrough present |
| 33 | `unchecked` |
| 34 | `unchecked_uniformity` |
| 35 | `sparse` |
| 36 | `fine` (derivatives) |
| 37 | `sideeffect` |
| 38 | `memory` |
| 39 | `pure` |
| 40 | `privileged` |
| 41 | `cold` |
| 42 | `nounwind` |
| 43 | `noreturn` |
| 44 | `inline` |
| 45 | `noinline` |
| 46 | `saturate` (`fcvt`); `foreign_unwind` (`invoke`) |
| 47 | `poison` on `ctlz`, `cttz`, `fptosi`, `fptoui` |
| 48–51 | Instruction-specific selector: the index of the chosen keyword among the alternatives the instruction's syntax lists in brackets, in grammar order (for example `load [trivial\|copy\|take]` → 0, 1, 2) |
| 52–63 | Zero |

### 22.9 Internal symbols

Package-linkage entities lower to symbols `_TIR1` followed by the first 8 bytes of the package digest in lowercase hexadecimal, the length-prefixed path components of KLF.md section 24.2, and a kind letter (`F` function, `G` global, `D` descriptor, `W` witness table, `V` vtable).

### 22.10 Canonical encoding

`tirc package` re-encodes every module canonically:

1. Sections in kind order, uncompressed, contiguous, no padding.
2. STRINGS and TYPES in order of first reference during a traversal of the other sections in kind order; types topologically sorted with first-reference order breaking ties.
3. Functions in SIGNATURES in declaration order as the producer emitted them; blocks in reverse post-order. Within a block, visit distinct successors in producer block order; start each disconnected component at its first block in producer block order.
4. Minimal LEB encodings.

A module inside a package whose re-encoding differs from its bytes is rejected with `TIR-ENC-0002`. Compression happens only in the package archive (section 29.6).

### 22.11 Limits

| Item | Limit |
| --- | --- |
| Values per function | < 2^32 |
| Blocks per function | < 2^24 |
| Instructions per block | < 2^24 |
| Types per module | < 2^32 |
| Type nesting depth | < 256 |
| Uncompressed section size | < 2^40 bytes |
| Function parameters | < 2^16 |

---

## 23. Textual format (`.tir`)

### 23.1 Correspondence

The textual format is a lossless rendering of the binary format: `tirc asm` followed by canonical re-encoding yields the same bytes as the original canonical module, and `tirc dis` of a canonical module followed by `tirc asm` yields identical bytes. The grammar is Appendix A.1. Comments run from `;` to the end of the line.

Quoted string terminals use Kira's ordinary string escapes: `\n`, `\t`,
`\r`, `\e`, `\0`, `\"` and `\\`. A backslash followed by LF or CRLF
continues the string; the line break and following whitespace are omitted.
Unknown escapes and a trailing backslash are invalid. The `b"..."` operand
of `const.bytes` uses the byte-specific escapes defined in section 12.1.

### 23.2 Example (informative)

```
module @Demo.Blur tir 26.11.1 profile app semantics defined
  producer "klf" "26.11.1" languages [kira, ksl]

import ridl @Gpu abi "6f1c2a9e-44d0-4b31-9a55-0c2f3e8d7a10"

type %BlurArgs = struct ridl {
    source: texture<2d, vec<4 x f32>, sample>,
    target: texture<2d, vec<4 x f32>, write>,
    radius: i32
  }
  policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none, lifetime: eager }

func module @Demo.Blur.weight(%x: f32, %sigma: f32) -> f32 target(cpu, gpu) {
bb0(%x: f32, %sigma: f32):
  %x2 = fmul %x, %x
  %s2 = fmul %sigma, %sigma
  %two = const.float f32, 2.0
  %d = fmul %two, %s2
  %q = fdiv %x2, %d
  %n = fneg %q
  %w = math.exp %n
  return %w
}

entry compute @Demo.Blur.horizontal(root %args: ptr<constant> %BlurArgs) -> unit
    threads(8, 8, 1) fp(fast) {
bb0(%args: ptr<constant> %BlurArgs):
  %a = ptr.toaddr %args as *%BlurArgs<constant>
  %srcA = field.addr %a, 0
  %src = load [trivial] %srcA
  %dstA = field.addr %a, 1
  %dst = load [trivial] %dstA
  %gid = gpu.builtin thread_position_in_grid : vec<3 x i32>
  %xy = vec.shuffle %gid, %gid, [0, 1]
  %smp = const.sampler { min: linear, mag: linear, address_u: clamp_to_edge, address_v: clamp_to_edge }
  %size = tex.query %src, size
  %uv = call @Demo.Blur.uv(%xy, %size)          ; a target(gpu) helper of this module
  %lod0 = const.float f32, 0.0
  %c = tex.sample_level %src, %smp, %uv, lod %lod0
  tex.write %dst, %xy, %c
  return
}

func exported @Demo.Blur.horizontalEntry() -> entry<compute, root %BlurArgs, immediate unit> engine(native) {
bb0:
  %e = gpu.entry.ref @Demo.Blur.horizontal
  return %e
}

func exported @Demo.Blur.load(%url: owned %String) -> %Image async {
bb0(%url: owned %String):
  %f = call @Net.fetch(%url)
  %bytes = await %f
  %img = call @Demo.Blur.decode(%bytes)
  return %img
}
```

### 23.3 Instruction metadata

The metadata names and value shapes are the ones listed after the section table
in 22.4. `!range` attaches to an integer-valued load; `!nonnull` and `!align`
attach to loads; `!noalias`, `!alias.scope` and `!tbaa` attach to memory
operations; `!invariant.load` attaches to a load. `!branch_weights` attaches to
a conditional branch or integer switch and has one weight per successor. `!loop`
attaches to a loop backedge terminator. Each metadata kind may appear at most
once on an instruction. Metadata violations have the semantics in 10.2.

---

## 24. Versioning and compatibility

### 24.1 Version identifiers

A TIR version is written `YY.M.W`: the year within the century (1–99), the month (1–12) and the week of the month (1–5) of the Thursday train that released it: W is the ordinal of that Thursday within its month, so a month has four or five trains and a year 52 or 53. Tessera's general version form makes the week optional (`YY.M`, used for DeviceKit's Driver API versions); TIR versions always carry it. In binary a version is a `u8` year, a `u8` month and a `u8` week. Versions order by year, month and week (TVISA.md section 33.1). The first version is 26.11.1.

### 24.2 One version per train

Every train releases exactly one TIR version. A version identical in content to its predecessor is still a distinct version.

### 24.3 Immutability

Once released, a version's encoding and semantics never change. A correction is a new version.

### 24.4 Window

A `tirc` built for train T accepts modules of version V when:

| Profile | Accepted versions |
| --- | --- |
| `app`, `system` | The 26 trains ending at T (T−25 to T) |
| `driver` | The 52 trains ending at T (T−51 to T) |
| `kernel` | T only |

Older modules are rejected with `TIR-VER-0001`, newer ones with `TIR-VER-0002`.

### 24.5 No automatic upgrade

`tirc` keeps one decoder and one semantic front stage per accepted version. A front stage maps a module of version V into `tirc`'s internal representation while preserving V's semantics exactly. `tirc` never rewrites a module into a newer version, and no producer may depend on it doing so.

### 24.6 Mixing versions

Every module of a package has the same TIR version (`TIR-PKG-0004`). Packages of different versions coexist on a device; they meet only at RIDL boundaries.

### 24.7 Expiry

A package's TIR version bounds its life. When the version leaves the window of the current train, the pipeline withdraws the package (section 30.8) and devices refuse to launch its delivery units with `TIR-RT-0400`. For the app and system profiles this is 26 trains, about six months; for drivers it is 52 trains, matching DeviceKit's twelve-month Driver API window (DeviceKit.md).

### 24.8 Change log

Every version has an entry in `TIR-CHANGES.md`:

```
## 26.11.2
Additions: <instructions, types, attributes, sections>
Removals: <with the last version that accepts them>
Semantic changes: <instruction, old meaning, new meaning>
Encoding changes: <section, field>
Producer notes: <what a producer must change to emit this version>
```

### 24.9 TVISA and trains

`tirc` built for train T emits TVISA of train T. The pipeline keeps the builds of every live package for every train within the window, and a device requests delivery units for its own train (section 30.4).

---

## 25. Producers

### 25.1 Requirements for every producer

1. Emit modules of exactly one TIR version, valid under sections 6 to 22, either canonical or made canonical with `tirc canonicalize`.
2. Record the producer and every source language truthfully (section 6.2).
3. Be deterministic: identical inputs and options produce identical modules.
4. Supply every piece of information the source language provides: ownership for managed languages, `semantics(defined)` where the source has no undefined behavior, alias and range metadata, debug information, structured-control-flow annotations for GPU code.
5. Never invent information: a function is not marked defined if its source construct maps to a permissive form, and no ownership is claimed that the source did not establish.
6. Third-party producers need no registration. A producer chooses its name string.

### 25.2 KLF

KLF frontends emit TIR directly. KLF.md section 8.11's `KirBuilder` becomes `TirBuilder`, with one method per TIR instruction and the same shape as before; KLF Kira definitions call it unchanged except for names. KIR instructions map one-to-one to TIR instructions of the same name, and every TIR addition is available to definitions.

| KLF artifact | TIR replacement |
| --- | --- |
| KIR | TIR |
| `.kir` | `.tir` |
| `.kirb` | `.tirb` |
| `.klbc` and the KLF VM | The interpreter tier executing TIR (section 28.4) |
| KLF Cranelift and LLVM backends | `tirc` |
| Hybrid runtime | Engines (section 6.5) and the device runtime |
| Hot module push of KIR text | Hot module push of `.tirb` (section 28.9) |
| `libklfrt` | `libtirrt` (section 28.10) |
| `.klbundle` | `.tirpkg` and `.tirdu` |
| `Session.evaluate` on the VM | The host TIR interpreter with no capabilities and the limits of KLF.md section 14.4 |
| `KLF-RT-NNNN` codes | `TIR-RT-NNNN` codes with the same numbers |

`klf build` compiles a definition to TIR and then with `tirc` to a native driver. Generated drivers emit TIR and link `tirc` as a library for native output. Compile-time evaluation (C++ `constexpr`, Kira `comptime`) runs on the host interpreter. KLF.md will be revised to reference this section; until then this table governs.

### 25.3 The LLVM bitcode converter

`tirc import llvm` reads LLVM bitcode (never textual IR) for `tir64-unknown-tessera`, produced by the Tessera LLVM fork or by any LLVM release the fork's bitcode reader accepts.

| LLVM | TIR |
| --- | --- |
| `iN`, `half`, `bfloat`, `float`, `double`, `fp128` | Same types |
| `x86_fp80`, `ppc_fp128` | Rejected, `TIR-IMP-0001` |
| `ptr` in address space 0 | `ptr` |
| Other address spaces | Rejected, `TIR-IMP-0002` |
| Fixed vectors, `<vscale x N x T>` with N × bits(T) = 128 | `vec<N x T>`, `svec<T>`; other scalable shapes rejected, `TIR-IMP-0003` |
| `undef` | `const.zero` |
| `poison` | `const.poison` |
| `nsw`, `nuw` | `[poison s]`, `[poison u]` |
| `exact`, `disjoint` | `[exact]`, `[disjoint]` |
| `getelementptr` with `inbounds` or `nusw` | `ptr.offset [inbounds]` after offset computation with the tir64 layout |
| Fast-math flags | Same flags |
| `freeze`, `select`, `phi` | `freeze`, `select`, block parameters |
| `load`, `store` (alignment, `volatile`, `!nontemporal`) | `raw.load`, `raw.store` |
| Atomics and `fence`; `syncscope("singlethread")` | Section 15.6 with scope `system`; scope `thread` |
| `call`, `invoke`, `landingpad`, `resume` | `call`, `invoke`, `landingpad` (section 14.6), `resume` |
| `musttail` | `tail.call` |
| `indirectbr`, `blockaddress` | `br.indirect`, `block.addr` |
| `switch`, `br`, `ret`, `unreachable` | `switch.int`, `br` or `cond.br`, `return`, `unreachable [ub]` |
| `llvm.memcpy`, `llvm.memmove`, `llvm.memset` | `mem.copy [nooverlap]`, `mem.move`, `mem.set` |
| `llvm.*.with.overflow`, saturating intrinsics | `add.ovf` family, `[sat]` modes |
| Bit-manipulation intrinsics, `abs`, `smin`, `smax`, `umin`, `umax` | Same instructions |
| Floating-point intrinsics (`sqrt`, `fma`, `fmuladd`, `minimum`, `maximum`, `minnum`, `maxnum`, rounding, `copysign`, math library functions) | Section 12.3 (`minnum` → `fminnum`, `maxnum` → `fmaxnum`) |
| `llvm.vector.reduce.*`, masked and vector-predicated intrinsics | Section 16.3 with masks |
| `llvm.assume`, `llvm.expect` | `assume`, `expect` |
| `llvm.trap`, `llvm.debugtrap` | `trap 0x0001`, `intrinsic 14` |
| `llvm.va_*` | `va.*` |
| `llvm.dbg.*` and debug records | Section 33 |
| `llvm.lifetime.*`, `llvm.invariant.*` | Dropped; stack slot lifetimes are recomputed |
| Inline assembly | Accepted only in functions carrying the `"tir-variant"` attribute, which become variant bodies; otherwise `TIR-IMP-0004` |
| Parameter and function attributes | Same; UB-capable attributes (section 10.2) make the function permissive |
| `!tbaa`, `!alias.scope`, `!noalias`, `!range`, `!nonnull`, `!align`, `!prof`, `!llvm.loop` | METADATA attachments |
| `@llvm.global_ctors`, `@llvm.global_dtors` | Module initializer and finalizer lists |
| `ifunc`, garbage-collection statepoints, Windows exception funclets, architecture intrinsics outside variant bodies | Rejected, `TIR-IMP-0005` |

Every converted function is unmanaged. It is defined when it contains no permissive form. Source languages come from the bitcode's debug compile units and the converter's `--language` option. C++ code links the C++ runtime framework (libc++ and libc++abi compiled through TIR). Rust code links the Tessera Rust standard library.

### 25.4 The Wasm64 converter

`tirc import wasm64` accepts WebAssembly modules with 64-bit memories.

| Feature | Status |
| --- | --- |
| memory64 | Required; 32-bit memories are rejected with `TIR-IMP-0020` |
| Multi-memory, bulk memory, multi-value, sign extension, non-trapping float-to-int, extended constant expressions, branch hinting | Accepted |
| Reference types (`funcref`), typed function references, tail calls | Accepted |
| Exception handling with `exnref` | Accepted, mapped to section 14 with the Wasm tag as `typeid` |
| Threads: shared memories, atomics, `wait`, `notify` | Accepted |
| SIMD128 | Accepted |
| Relaxed SIMD | Accepted with the deterministic semantics the relaxed-SIMD proposal defines for its deterministic profile |
| `externref`, garbage collection, `stringref`, the component model | Rejected, `TIR-IMP-0021` |

Mapping:

1. Each linear memory becomes a sandbox region: a reserved address range (LibTessera `Memory.reserve`) of the memory's maximum size, with globals holding base and current size. Growth commits pages.
2. Every load and store computes `address + offset`, checks it against the region's current size, traps with `TIR-RT-0030` when out of bounds, and masks the index on the checked path against speculative execution.
3. Tables become arrays of typed function references. `call_indirect` checks the Wasm signature and traps with `TIR-RT-0031` on mismatch, in addition to section 21.3's checks.
4. Function imports named `ridl:<Module>.<function>` become RIDL imports; imports named `tir:<symbol>` bind to package-linkage functions. WASI is not provided. Exports become RIDL exports or package functions.
5. Wasm traps map to TIR traps with matching codes.
6. Every function is defined and sandboxed.

### 25.5 GPU converters

The SPIR-V converter (`tirc import spirv`, also linked as a library into the Vulkan-on-Ember framework for use on the device) accepts SPIR-V 1.6 under the Vulkan 1.4 environment with the extensions Ember.md lists for its Vulkan layer.

| SPIR-V | TIR |
| --- | --- |
| Descriptor sets and bindings | Handle and pointer loads from a root structure whose layout the Vulkan layer supplies |
| Push constants | Immediate data (the layer reports at most 256 bytes) |
| Uniform and storage buffers, buffer device addresses | `ptr<constant>`, `ptr<device>` |
| Structured control flow, `OpPhi` | Section 11.3 annotations, block parameters |
| `OpUndef` | `const.zero` |
| Specialization constants | `gpu.spec_const` |
| Vertex input and input assembly state | Generated vertex-pulling code from the pipeline state the layer passes to the converter |
| Geometry stage | A mesh entry that runs the preceding vertex work per input primitive and the geometry body, writing primitives through mesh outputs |
| Tessellation stages | A compute pre-pass for control and patch-constant work writing to device memory, followed by a mesh entry for evaluation |
| Transform feedback | Device-memory appends with atomic counters in the last pre-rasterization stage |
| Instructions requiring uniformity | `[unchecked_uniformity]` |

KHLSL, KMSL and KSL are KLF languages that emit TIR GPU code directly. DXIL is not accepted; Direct3D content reaches Ember through layers that produce SPIR-V or through KHLSL source. GPU TIR generated at run time needs no entitlement (section 28.7).

### 25.6 Toolchain forks

The Tessera forks of LLVM and Clang, rustc and Zig implement the `tir64-unknown-tessera` target of section 5.2. Each train's toolchains emit that train's TIR version. The Tessera Rust standard library and the C++ runtime framework are built for the target and ship as system frameworks. The same forks implement the platform targets of section 32.4.

### 25.7 Driver-profile producers

Driver packages accept only the source languages of section 6.3 and are source-reproducible (section 31.2). A module whose producer record names any other language is rejected with `TIR-PROF-0010`.

---

## 26. `tirc`, the TIR compiler

### 26.1 Placement

`tirc` is one program. Its full form runs on developer hosts (macOS, Linux, Windows, Tessera) and in the pipeline. Its device subset (verifier, interpreter, JIT backends, finalizer) is built into the device runtime from the same code.

### 26.2 Stages

| Stage | Work | Rules |
| --- | --- | --- |
| 1 Decode | Version front stage (section 24.5) | Encoding |
| 2 Structure | SSA, block order, terminators | V1, V3 |
| 3 Types | Every instruction | V2 |
| 4 Ownership | Canonical form | V4 to V8 |
| 5 Profile and semantics | Includes the platform rules of section 32.3 | V13, V14, V21 |
| 6 GPU | Restrictions, structured control flow, uniformity | V15, V16 |
| 7 Coroutines and variants | | V17, V18 |
| 8 Link | Package-linkage resolution; RIDL imports against the target train's framework contracts; inlinable bodies | V12 |
| 9 Analysis | Capabilities, trust classes, control-flow-integrity types | V19, V20 |
| 10 Specialization | Section 18 | |
| 11 Optimization | Section 26.3 | |
| 12 Lowering | To TVISA, per section 26.6 | |
| 13 Backend | LLVM (pipeline release builds), Cranelift-class (development builds, JIT, finalizer), Tessera GPU backends | |
| 14 Image | Native image with GPU section, RIDL metadata, dependency records, bridges | Section 27.8 |
| 15 Symbols | Symbol bundle | Section 33 |
| 16 Signing | Pipeline only | Section 30.6 |

### 26.3 Optimization levels

| Level | Use | Content |
| --- | --- | --- |
| `-O0` | Debugging | Required lowering only |
| `-O1` | Fast development builds | Inlining of trivial functions, constant folding, dead code elimination, CFG simplification, retain and release pairing |
| `-O2` | Default | `-O1` plus promotion of stack slots, specialization of hot polymorphic calls, devirtualization of `call.vtable` and `call.witness`, scalar replacement of aggregates, redundant load elimination under access scopes, loop-invariant code motion, borrow-to-copy and copy-to-borrow conversion, destroy hoisting, uniqueness-check elision, coroutine frame optimization, loop and SLP vectorization, inlining by cost model |
| `-O3` | Pipeline release builds | `-O2` plus whole-package partitioned link-time optimization, profile-guided optimization when profiles are present, aggressive inlining and unrolling |
| `-Os` | Size | `-O2` with size-oriented inlining and no unrolling |

Observable behavior after optimization equals `-O0` behavior, except where fast-math flags or `[approx]` permit differences. Destroy timing of `scoped` types never changes.

### 26.4 Ownership-aware optimization

Ownership is translated into facts the backends use:

| TIR fact | LLVM lowering |
| --- | --- |
| `borrowmut` parameter | `noalias` |
| `borrow` parameter of a type without interior mutability | `noalias readonly` |
| Owned `uniq T` | `noalias` |
| `static` access scope | Scoped `!noalias` and `!alias.scope` metadata for the scope's accesses |
| `pairing: counted` | Retain and release pairing and motion before lowering (KLF.md section 14.3 rules) |
| Defined function | No poison flags, no UB-capable attributes |

Within a defined function these facts are proven by the verifier, never assumed, so their use cannot introduce undefined behavior.

### 26.5 Profile-guided optimization

`tirc build --instrument` inserts `prof.count` counters and value profiles. Profiles collected by running the instrumented build are stored as `.tirprof` files in the package and keyed by BLAKE3 hashes of function bodies; a function whose body hash changed ignores its stale profile. The pipeline uses profiles at `-O3`.

### 26.6 The TIR-to-TVISA boundary

Before TVISA, `tirc` has performed every target-independent step: verification, specialization, inlining, target-independent optimization at the requested level, coroutine splitting, ownership lowering into explicit calls and memory operations, layout computation, and conversion of engines into bridges. TVISA retains: variant sets (so the finalizer selects by architecture and features), control-flow-integrity types, the analysis summary, debug information, and GPU code in TVISA's GPU profile. Instruction selection, calling-convention lowering, register allocation, scheduling, hardening sequences and GPU-family code generation happen after the boundary and are specified by TVISA.md.

### 26.7 Command line

| Command | Effect |
| --- | --- |
| `tirc check <files>` | Decode and verify |
| `tirc asm <file.tir> -o <file.tirb>` | Text to binary |
| `tirc dis <file.tirb>` | Binary to text |
| `tirc canonicalize <files>` | Raw to canonical TIR |
| `tirc import llvm \| wasm64 \| spirv <input> -o <file.tirb>` | Converters |
| `tirc package <dir> --manifest <file> --sign <identity> -o <pkg.tirpkg>` | Section 29 |
| `tirc build <pkg> --target <cpu-target>[,<gpu-family>...] -O<level> [--instrument] [--sanitize ...] -o <out>` | Native images and delivery units for development |
| `tirc build <pkg> --platform <platform> [--arch <arch>,...] [--clone-targets <target>,...] [--with <pkg>...] [--sdk <path>] [--dynamic-tir] [--threads] [--simulator] [--bundle] [--sign <identity>] -O<level> -o <out>` | Platform images and bundles (section 32), with dependency packages linked in |
| `tirc sign <bundle-or-image> --identity <identity>` | Platform signatures (section 32.10) |
| `tirc notarize <bundle> --credentials <file>` | Apple notarization |
| `tirc lower <pkg> --tvisa -o <out>` | TVISA images |
| `tirc finalize <file.tvisa>` | Finalize for the host |
| `tirc analyze <pkg>` | Analysis report |
| `tirc ridl emit <module>` | `.ridl` of a module |
| `tirc abi freeze \| apply \| retire` | RIDL versioning |
| `tirc run <pkg> [--engine-default runtime]` | Build and run on the host, or interpret |
| `tirc push <host:port>` | Hot module push client |
| `tirc symbolicate <report> --symbols <dir>` | Crash symbolication |

`tirc` reports progress exactly as KLF.md section 5.2.1 specifies for client CLIs, through the same implementation.

### 26.8 Determinism and reproducibility

Given the same inputs, options, target and `tirc` build, every output is bit-identical. `tirc` embeds no timestamp and no absolute path, unless `--debug-paths=absolute` is given for development builds. Output ordering never depends on thread scheduling. Every image records the `tirc` build identifier.

### 26.9 Scale

`tirc` MUST compile packages of Unreal Engine 5's size. At `-O3`, peak memory per link-time-optimization partition MUST NOT exceed 1.5 times the peak memory of Clang's ThinLTO backend on the same code, and total wall-clock time MUST NOT exceed 2.0 times that of Clang at `-O3` with ThinLTO on the same machine.

---

## 27. Lowering and hardening

These requirements bind every tier that produces machine code: native images, finalized TVISA, and both JIT tiers. On platform targets, section 32.9 replaces sections 27.2 and 27.3 and states which of sections 27.4 and 27.5 apply.

### 27.1 General

1. Position-independent code, loaded at randomized addresses.
2. Memory is never writable and executable at once. Relocated data becomes read-only after loading.
3. Stacks are not executable. Frames larger than one page probe each page.
4. Frame pointers are maintained in every function (x29, rbp, s0) for profilers and debuggers.
5. Section 10.5's zero-initialization and every check written in TIR are preserved.

### 27.2 Forward-edge control-flow integrity

| Architecture | Callee | Call site |
| --- | --- | --- |
| aarch64 | Begins with `BTI c` (or `BTI jc`); the 32-bit type tag precedes the entry | Loads the tag at target − 4, compares, branches to the trap on mismatch, then calls with `BLRAA` when the pointer is signed |
| x64 | Begins with `ENDBR64`; the type tag is encoded in the preamble before the entry | Compares the tag, traps on mismatch, then calls; CET indirect branch tracking enforces the landing pad |
| rv64 | Begins with `lpad label`, label = the low 20 bits of the tag | Loads the label into t2 with `lui`, compares the full tag in software, traps on mismatch, then `jalr`; Zicfilp enforces the label in hardware |

On aarch64, every code pointer stored in memory (vtable and witness entries, closure code words, function values in data structures) is signed with the IA key and a discriminator mixing the storage address with the low 16 bits of the type tag, and authenticated when used. A tag mismatch traps with `TIR-RT-0012`.

### 27.3 Backward-edge integrity

| Architecture | Mechanism |
| --- | --- |
| aarch64 | Return addresses signed with `PACIASP` and authenticated with `AUTIASP` (FPAC faults when present); the Guarded Control Stack when FEAT_GCS is present |
| x64 | CET shadow stack |
| rv64 | Zicfiss shadow stack (`sspush`, `sspopchk`) |

A shadow-stack or authentication failure terminates the process with report `TIR-RT-0013`.

### 27.4 Memory tagging

On aarch64 CPUs with MTE, the runtime allocator tags every heap allocation with a random tag different from its neighbors' and retags freed memory. Address-taken stack slots of unmanaged functions are tagged. Tag checking is synchronous for system processes and development builds and asymmetric for applications. A tag fault terminates the process with report `TIR-RT-0032`. On CPUs without MTE these steps are absent.

### 27.5 Speculative execution

The interpreter, the JIT tiers and sandboxed code mask indices on bounds-checked paths. `ct.select` and `ct.eq` are lowered without data-dependent branches or memory accesses. Kernel mitigations are Tarwin.md's.

### 27.6 Sanitizers

`tirc build --sanitize address | thread | undefined | memory-tag` produces development builds with sanitizers. The pipeline rejects sanitized builds (`TIR-PIPE-0005`).

### 27.7 Calling conventions

`abi(ridl)` lowers to ridlcc (KLF.md section 22 and section 19.2). `abi(c)` lowers to AAPCS64, the System V AMD64 ABI, or the RISC-V LP64D psABI. RIDL-mode variadics follow KLF.md section 29.5. On platform targets `abi(c)` is the platform's convention (TVISA.md section 32.4).

### 27.8 Native image contents

A native image, in the platform image format (Tarwin.md; ELF64 with named sections on the Linux backend), contains the items below. Platform images (section 32.10) carry the same contents in the formats of TVISA.md section 32.9.

| Content | Notes |
| --- | --- |
| Code | Read and execute |
| Read-only data, relocated read-only data, data | |
| Thread-local storage template | |
| `.tirgpu` | GPU entry records and per-family GPU binaries (section 17.17) |
| `.ridlmeta` | RIDL metadata (KLF.md section 24.3) |
| `.tirdeps` | Framework dependencies: module, ABI UUID, inlinable body hashes |
| `.tirbridge` | Engine bridges and thunks table |
| `.tirrt` | Hashes of the runtime TIR modules in the delivery unit |
| Build identifier | BLAKE3-256 of the image without its signature |
| Signature | Pipeline or development signature (section 29.4) |

---

## 28. The device runtime

This section describes Tessera. On platform targets, section 32.8 describes the runtime each application carries.

### 28.1 Components

| Component | Runs in | Role |
| --- | --- | --- |
| `libtirrt` | Every process | Allocator, reference counting, traps, unwinder, coroutine support, bridges, GPU entry registry, the in-process verifier and interpreter, the dynamic TIR client |
| Compile service `tessera.tir` | A dedicated system process | Verifier, baseline JIT, optimizing JIT, finalizer, code cache |
| Loader | Tarwin; LibTessera's loader on the Linux backend | Loads native and finalized images, checks signatures, dependency records and expiry |

### 28.2 The compile service

1. Processes reach the compile service through `System.connectService("tessera.tir")` (LibTessera.md section 12).
2. The service verifies its input, generates code into a Memory object, and returns a handle the client maps with `Read | Execute` and without `Write`. A client process never holds writable code memory. On the Linux backend the Memory object is a sealed memfd.
3. Code generated for identical inputs is shared read-only between processes through the code cache.
4. The service runs with no network access and no access to application data.

### 28.3 Verification on the device

Runtime-engine TIR shipped in a delivery unit is verified when first loaded; the result is cached with the module hash and a device-key MAC. Dynamic TIR is verified every time it is loaded. Both the in-process verifier (defense in depth for the interpreter) and the compile service (before generating code) verify.

### 28.4 Interpreter

1. Tier 0 executes verified TIR with exactly the semantics of this document. Its internal predecoded form is private.
2. It supports every CPU instruction except those V9 excludes from `runtime` functions. `svec` uses the process's `vscale`. GPU code is never interpreted.
3. Interpreter frames are visible to the unwinder, debuggers, profilers and crash reports. Coroutines and async frames run natively in the interpreter.
4. It uses threaded dispatch over a register-based predecoded form, and MAY use superinstructions.

### 28.5 JIT tiers

1. The baseline JIT (tier 1) compiles a function quickly from TIR without optimization. The optimizing JIT (tier 2) applies Cranelift-class optimization.
2. Tier-up is driven by invocation and back-edge counters. Thresholds affect performance only, never results.
3. The optimizing JIT MAY compile only the success path of `guard` instructions. When a guard fails, the runtime reconstructs the fallback's arguments from the guard's metadata and continues in the fallback, whose own tier is chosen independently.
4. On-stack replacement enters optimized code at loop headers from the interpreter and leaves it on deoptimization.
5. JIT code is installed per process, obeys every requirement of section 27, and is invalidated when its module unloads.
6. JIT compilation of runtime-engine TIR shipped in a delivery unit needs no entitlement. Dynamic CPU TIR requires `tessera.tir.dynamic`.

### 28.6 Finalizer

1. The finalizer turns a TVISA image into native code for the device's CPU target and GPU family (TVISA.md).
2. It runs when a delivery unit carries CPU TVISA that the device policy chooses to finalize, when the device's GPU family was not built, when the GPU is a revision the pipeline did not know, and for non-default specialization constants.
3. It runs at background priority unless a launch or a pipeline creation waits on it.
4. For a given TVISA image, device target and finalizer build, the output is bit-identical.
5. Finalized images are signed with the compile service's device key; the loader accepts them alongside pipeline-signed images. When the pipeline later delivers a native image for the same package and target, it replaces the finalized one at the next launch.

### 28.7 Dynamic TIR

The system framework `TIRRuntime` (a RIDL module) exposes:

```
struct TirModule
enum TirError { Verification(String)  Capability(String)  Version  Entitlement }

extend TirModule {
    function load(bytes: borrow [U8]) -> Result<TirModule, TirError>
    function lookup(borrow self, symbol: borrow String, typeid: U64) -> Optional<RawPtr>
    function unload(self)
}
```

1. A dynamic module MUST be of the device train's TIR version.
2. Its imports are bounded by its creating process's capability declaration (section 21.2).
3. `lookup` returns a function value only if the function's `typeid` equals the requested one.
4. Modules containing CPU code require the entitlement `tessera.tir.dynamic` and execute on the interpreter and JIT tiers. Modules containing only GPU code require no entitlement; Ember compiles their entries through the finalizer's GPU path. The Vulkan-on-Ember framework and runtime shader generation use this path.
5. KavaScript generates TIR at run time for `eval` and dynamically loaded scripts and runs it through this API. KavaScript code compiled ahead of time is ordinary shipped TIR and needs no entitlement.

### 28.8 Code cache

1. Entries are keyed by BLAKE3-256 over the input hash, the device CPU target and feature set, the GPU family and revision, the finalizer or JIT build identifier, and specialization constants.
2. Entries live on a cache volume (TRFS.md) and carry a MAC under a device key held by the compile service.
3. Eviction is least-recently-used under storage pressure. Entries produced by a finalizer build of an earlier train are purged after a train update.

### 28.9 Hot module push

Hot module push follows KLF.md section 17.5 with these changes:

1. `PUSH_MODULE` carries a `.tirb` module of the device train's TIR version.
2. The device verifies it, compiles it through the JIT tiers, and replaces the module's `runtime`-engine functions with the two-version semantics of KLF.md section 17.5. A development build made with `--engine-default runtime` makes every eligible function replaceable.
3. Reason code 7 `CAPABILITY_EXCEEDED` is added: the module reaches capabilities beyond the running package's declaration.
4. Hot push is available only on devices in developer mode and on debug images.

### 28.10 `libtirrt`

`libtirrt` is linked into every process and carries:

1. The allocator, slot metadata, counting by address, raw deletion with poison and quarantine, and weak references, with the semantics of KLF.md sections 18.1 to 18.5. Spans are aligned to their own size, which is at least the system page size.
2. Memory tagging integration (section 27.4).
3. The cycle detector in development builds (KLF.md section 18.6) and the heap balance check at exit for processes made of managed modules (KLF.md section 15.6).
4. Dynamic exclusivity checks (KLF.md section 18.7).
5. Traps (section 34.6), the unwinder and `tir_personality`.
6. Coroutine frame allocation and the `Async` helpers.
7. Engine bridges (`tir_rt_invoke`) and thunk registration.
8. The GPU entry registry, which resolves entry references and gives Ember an entry's record and binaries.
9. The client side of dynamic TIR and of the compile service.

### 28.11 GPU reference executor

The GPU reference executor is a deterministic CPU implementation of every GPU construct of section 17 together with Ember.md's rasterization, blending and sampling rules, software ray traversal and cooperative-matrix arithmetic. In `fp(precise)` it computes every correctly-rounded operation exactly and every bounded operation with the most accurate result. It is the conformance oracle for every GPU backend and runs on developer hosts and devices.

---

## 29. Packages and delivered artifacts

### 29.1 Package kinds

| Kind | Profile | Entry |
| --- | --- | --- |
| `app` | `app` | Required |
| `extension` | `app` | Required |
| `tool` | `app` or `system` | Required |
| `framework` | `app` or `system` | None |
| `service` | `system` | Required |
| `runtime` | `system` | None (system runtime libraries) |
| `driver` | `driver` | Required |
| `kernel` | `kernel` | Required |

### 29.2 Layout

```
<Name>.tirpkg/
  manifest.json
  modules/<Module>.tirb
  resources/<path>
  profiles/<name>.tirprof
  analysis.json                 written by tirc package
  source.json                   driver profile
  signatures/developer.sig
```

A package is a directory or a single-file archive with the same tree (section 29.6).
Every package entry path is UTF-8 with `/` separators and is relative: it has
no leading slash, drive prefix, backslash or NUL, and no empty, `.` or `..`
component. A directory package entry resolves to a regular file inside the
package root.

### 29.3 Manifest

`manifest.json` is JSON, canonicalized with RFC 8785 when hashed.
Object member names are unique. Number values must be expressible as IEEE 754
binary64 values; represent integers requiring greater precision as JSON
strings.

| Field | Type | Meaning |
| --- | --- | --- |
| `format` | `"tirpkg"` | |
| `tir` | String | TIR version of every module, e.g. `"26.11.1"` |
| `id` | String | Reverse-DNS package identifier |
| `name` | String | Display name |
| `version` | String | Developer version |
| `build` | Integer | Required; strictly increasing per `id` |
| `kind` | String | Section 29.1 |
| `profile` | String | Section 6.3 |
| `platforms` | Array of strings | Platforms the package supports (section 32.3); default `["tessera"]` |
| `modules` | Array of `{name, path, blake3}` | |
| `entry` | `{module, symbol}` | Required for kinds that have an entry |
| `dependencies` | Array of `{module, abi}` | RIDL modules of other packages with the ABI UUID required |
| `entitlements` | Array of strings | Requested entitlements |
| `gpu` | `{capabilities: [...]}` | Optional GPU capabilities the package requires; devices lacking one cannot install it |
| `cpu` | `{features: [...]}` | Optional CPU features the package requires |
| `deviceKit` | String | Required Driver API version, e.g. `"26.10"` (driver profile) |
| `license` | SPDX expression | |
| `source` | Object | Required driver provenance, as in Appendix A.3 |
| `resources` | Array of `{path, blake3, size, kind}` | `kind` is `asset` (default) or `firmware` (section 31.2) |
| `profiles` | Array of `{path, blake3}` | |
| `bundle` | Object | Bundle metadata for platform targets (section 32.10, Appendix A.5) |
| `product` | Object | Product metadata defined outside this specification; carried and signed, never interpreted by `tirc` |

`platforms` contains unique names from section 32.3; `portable` is a module
target, not a package platform. Omitting the field means `tessera`; an
explicit empty array means that the package supports no platform.
The `id` is a reverse-DNS identifier with nonempty DNS labels containing only
ASCII letters, digits, and interior hyphens. Driver packages require
`GPL-3.0-only` or `GPL-3.0-or-later`, and `source` contains the canonical
Appendix A.3 source record. Every driver module's MODULE source hash equals
the BLAKE3-256 digest of that canonical `source.json`. Dependency module names
are nonempty and unique; each `abi` is a lowercase hyphenated UUID.

### 29.4 Hashes and signatures

1. File hashes are BLAKE3-256, written in lowercase hexadecimal.
2. The package digest is the BLAKE3-256 hash of the canonical manifest, which lists every file's hash.
3. `signatures/developer.sig` is a JSON object `{chain, ed25519, mldsa65}`. `chain` is an object with exactly `ed25519` and `mldsa65` members. Each member is a nonempty array of standard-base64 DER X.509 certificates ordered from leaf to root, including the Tessera developer certificate authority root. Each certificate's issuer Name DER encoding MUST equal the subject Name DER encoding of the next certificate; the root is self-issued, and both arrays end with the identical root certificate DER. The leaf subject Name DER encodings MUST be identical, identifying the same developer. The leaf certificates contain the public keys corresponding to the package's Ed25519 and ML-DSA-65 signatures. The signatures are over the message `"TIR package v1"`, a zero byte, and the digest; both MUST verify. Signature strings and certificate strings use canonical padded standard Base64.
4. The verifier MUST verify each certificate's signature over its complete DER-encoded `TBSCertificate` using the next certificate's public key. It MUST verify the root's self-signature using the root certificate's public key. The `TBSCertificate.signature` and outer `Certificate.signatureAlgorithm` encodings MUST be identical. Unknown signature algorithm identifiers or parameters outside this profile are rejected. The accepted profile is Ed25519, ML-DSA-65, ECDSA with SHA-256 or SHA-384 using P-256 or P-384 issuer keys, RSA PKCS#1 v1.5 with SHA-256, SHA-384 or SHA-512 using 2048–8192-bit issuer keys, or RSA-PSS with SHA-256, SHA-384 or SHA-512 using 2048–8192-bit issuer keys, MGF1 with the same digest, salt length equal to the digest output length, and trailer field 1. Ed25519, ML-DSA-65 and ECDSA parameters MUST be absent. RSA PKCS#1 v1.5 parameters MAY be absent or DER NULL. RSA-PSS parameters MUST identify the selected digest and matching MGF1 digest, and MUST set the salt length and trailer field as stated.
5. Delivery units are signed the same way by the pipeline over `"TIR delivery v1"`, a zero byte, and the delivery digest.
6. Development certificates are issued to Tessera Developer accounts with a validity of at most 182 days. Section 30.12 governs their use.
7. The `tirc package --sign` identity file is a JSON object with `chain` as above, plus `ed25519PrivateKey` and `mldsa65PrivateKey`, each a canonical padded standard-base64 PKCS#8 DER private key. The signing tool uses these keys for the corresponding signatures and copies `chain` into `developer.sig`. It MUST reject a chain with a certificate that is not currently valid or a developer leaf certificate whose validity exceeds 182 days.

### 29.5 Delivery units

A delivery unit (`.tirdu`) is built for one package, one train, one CPU target and a set of GPU families.

| Entry | Content |
| --- | --- |
| `manifest.json` | The package manifest plus `train`, `cpuTarget`, `gpuFamilies`, `cpuTvisa`, and the provenance hash |
| `images/<name>.img` | Native images: one per package, and one per framework for framework packages |
| `tvisa/<name>.tvisa` | TVISA image. GPU code is always included; CPU code is included per section 30.4 |
| `runtime/<Module>.tirb` | Runtime-engine TIR |
| `resources/<path>` | |
| `provenance.json` | Section 30.6 |
| `signatures/pipeline.sig` | |

The installer verifies the pipeline signature, the train window and the dependency records, then installs the files where Tarwin.md and TRFS.md place them.

### 29.6 Archive container

Single-file packages and delivery units use one container:

| Offset | Field |
| --- | --- |
| 0 | Magic `54 49 52 41` ("TIRA") |
| 4 | `u16` version (1) |
| 6 | `u16` flags (0) |
| 8 | `u32` entry count |
| 12 | `u32` zero |
| 16 | `u64` table offset |

The table, at the end of the file, lists per entry: path (ULEB length and UTF-8), `u64` offset, `u64` stored size, `u64` size, `u8` compression (0 none, 1 zstd), and the BLAKE3-256 hash of the uncompressed content. Entries are sorted by path. Hashes and signatures cover uncompressed content, so compression settings never affect them.

### 29.7 Symbol bundles

A symbol bundle (`.tirsym`) holds, for one image: DWARF 5, a map from native addresses to (module, function, instruction ordinal), and inlining records. It is keyed by the image's build identifier, is never delivered to devices, and is available to the package's developer.

---

## 30. The build and distribution pipeline

### 30.1 Stages

| Stage | Work |
| --- | --- |
| 1 Intake | Archive integrity, developer signature, certificate validity, consistency of kind and profile, window check |
| 2 Verification | `tirc check` on every module; canonical encoding |
| 3 Analysis | Capabilities against requested entitlements; profile rules; license and source for drivers; source reproducibility for drivers (section 31.2) |
| 4 Review | Automated policy and security analysis over the analysis report and the TIR; human review is outside this specification |
| 5 Build | The target matrix (section 30.3) |
| 6 Signing | Provenance and delivery-unit signatures |
| 7 Publication | Delivery units become available to devices |
| 8 Train rebuilds | Section 30.7 |
| 9 Expiry | Section 30.8 |

A package rejected at any stage receives the stage's diagnostics (`TIR-PIPE-NNNN` and the diagnostics of `tirc`).

### 30.2 Policy inputs

The pipeline applies policies expressed over the analysis report: entitlements, trust classes, assembly, dynamic code, GPU capabilities and strings. Policies are configuration of the pipeline, versioned with the train, and not part of TIR.

### 30.3 Target matrix

1. The CPU targets and GPU families are those of TVISA.md for the current train: a baseline per architecture and tuned targets for popular CPUs.
2. At publication the pipeline builds every CPU baseline and every GPU family the package's capabilities allow.
3. Tuned CPU targets are built on demand when the first device that matches one requests the package, then cached.

### 30.4 Device requests

A device requests a package with: package identifier and build, its train, its best-matching CPU target and feature set, and its GPU family and revision. The pipeline answers with a delivery unit built by the device train's `tirc`:

1. The native image for the best-matching tuned target if built, otherwise the baseline image, which always exists. A missing tuned build is queued.
2. GPU binaries for the device's family if it was built, and always the GPU TVISA code.
3. CPU TVISA when no built target uses features the device's CPU has, so the device may finalize a better image in the background.

### 30.5 Caching

Build outputs are content-addressed by BLAKE3-256 over the `tirc` build identifier, TIR version, target, options, module hash, and the hashes of the dependency contracts and inlinable bodies used. Link-time optimization is partitioned so that a change to one module rebuilds only affected partitions.

### 30.6 Signing and provenance

Every delivery unit carries `provenance.json`, a statement whose subjects are the hashes of the images, TVISA images and runtime modules, and whose fields are the package digest, the developer certificate fingerprint, the `tirc` build identifier, the target, the options, the analysis-report hash and the train. The pipeline signs it with its hybrid key. The pipeline publishes every `tirc` build, so anyone can rebuild a package and obtain bit-identical outputs.

### 30.7 Train rebuilds

Before each train ships, the pipeline rebuilds every live package for that train with that train's `tirc`, which also refreshes every dependency on changed framework contracts and inlinable bodies. Automated launch tests run each rebuilt app, and the package's declared test entry when present. A package that fails to rebuild or fails its tests is marked incompatible with the new train, devices on that train report it unavailable, and its developer is notified.

### 30.8 Expiry

When a package's TIR version leaves the window of the current train (section 24.7), the pipeline withdraws it: no new installations, no rebuilds, and devices refuse to launch installed copies once their own train's window excludes the version (`TIR-RT-0400`). Developers are notified four trains before withdrawal.

### 30.9 Developer access

A developer can download every delivery unit and symbol bundle of their packages, for every train in the window and every target, and can run the same `tirc` build locally. What users run is exactly what developers can test.

### 30.10 Crash reports

Crash reports sent from devices with the user's consent carry image build identifiers and addresses. The pipeline symbolicates them with symbol bundles and delivers them to the developer.

### 30.11 On-demand builds and fallbacks

A device never waits for an on-demand build: it installs the baseline image and replaces it when the tuned build arrives. A device whose GPU family was not built finalizes GPU code locally.

### 30.12 Development signing

1. `tirc build` signs images and delivery units with the developer's development certificate.
2. A device in developer mode accepts development-signed delivery units whose certificate is valid; validity is at most 182 days.
3. Images compiled on the device itself run in developer mode and receive no elevated entitlements.

---

## 31. Kernel and driver profiles

### 31.1 Kernel profile

The kernel profile compiles Tarwin (Tarwin.md).

1. The only source language is Kira, through KLF (Tarwin.md section 1.4). Code from any other producer is rejected in the kernel profile with `TIR-PROF-0012`.
2. No LibTessera import, `runtime` engine, dynamic TIR, unwinding (every function is `nounwind`), or GPU entry.
3. Floating-point and vector types and instructions appear only in functions marked `kernel.fp`. Tarwin brackets calls into such functions with its floating-point state handling. Outside them, `tirc` generates code that uses no floating-point or vector registers, including for `mem.copy` and similar lowerings; on x64 there is no red zone.
4. `stack.alloc` sizes are static. A supplied count must resolve to a nonnegative compile-time integer smaller than 2^62; in binary CODE it MUST be the result of `const.int`. Kernel stacks have fixed sizes and guard regions defined by Tarwin.md.
5. Heap instructions lower to the kernel allocator entry points Tarwin.md names.
6. Privileged instructions appear only in `[privileged]` assembly inside variant bodies, under the all-three rule.
7. Per-CPU variables are globals with the `percpu` flag, reached with `kernel.percpu.addr`, which lowers to TPIDR_EL1-relative addressing on aarch64, GS-relative addressing on x64, and the per-CPU base convention Tarwin.md fixes on rv64.
8. The code model and image layout are Tarwin.md's (`tirc build --code-model kernel`).
9. Control-flow integrity and shadow stacks apply in their supervisor forms: PAC with the kernel's keys and BTI on aarch64, supervisor shadow stacks and IBT on x64, Zicfiss and Zicfilp in supervisor mode on rv64. Kernel memory tagging is Tarwin.md's decision.
10. Kernel images are delivered native for each CPU target, built by the train's own `tirc`; there is no TVISA kernel image, because nothing can finalize code before the kernel boots.
11. Kernel modules MUST be of the current train's TIR version.

### 31.2 Driver profile

DeviceKit drivers are user-space processes (DeviceKit.md).

1. Imports are limited to LibTessera, the DeviceKit RIDL modules of the Driver API version named by the manifest's `deviceKit` field, and the frameworks DeviceKit.md lists.
2. Device registers are LibTessera Memory objects with a device cache policy that DeviceKit maps. They are accessed only with `mmio.load` and `mmio.store`, ordered with `fence.io`. DMA buffers come from DeviceKit with IOMMU mappings.
3. No `runtime` engine, dynamic TIR or JIT entitlement.
4. Source languages: `kira`, `rust`, `zig`, `c`, `cpp`, `karc-cpp`, `karc-arc`. Any other is rejected with `TIR-PROF-0010`.
5. The manifest's `license` is `GPL-3.0-only` or `GPL-3.0-or-later`; otherwise `TIR-PROF-0011`.
6. Source reproducibility: `source.json` (Appendix A.3) names the repository, the revision, the exact toolchain builds and the build command. The pipeline fetches the source, rebuilds every module with the locked toolchain in a hermetic environment, and requires bit-identical modules; a mismatch is rejected with `TIR-PIPE-0010`. Every module's MODULE section carries the BLAKE3-256 hash of the canonical `source.json`.
7. Firmware is carried as resources of kind `firmware`. Firmware may be proprietary. It is opaque to the pipeline, never executed by the CPU, and loaded into devices only through DeviceKit; vendor signatures on firmware are preserved.
8. Driver packages have a 52-train window, matching DeviceKit's twelve-month Driver API window. The rules for updates that would drop an installed driver's Driver API version are DeviceKit.md's.
9. Drivers use synchronous memory-tag checking where MTE is present.

---

## 32. Platform targets

### 32.1 Model

1. TIR code runs on Tessera and, through platform targets, on other operating systems: macOS, iOS and iPadOS, Windows, Linux, Android, web browsers and WASI runtimes. One TIR program has one semantics on all of them: the same instructions, types, ownership rules, memory model, traps and floating point. What differs is only what a program reaches through platform imports (section 32.6).
2. On a platform, a program is an ordinary application or library of that platform. `tirc` builds it on a developer machine or in the developer's continuous integration into the platform's image format, the developer signs it with the platform's identity, and it is distributed through the platform's own channels. Tessera's pipeline, delivery units, device runtime, compile service and expiry do not exist there.
3. Every platform application carries its own runtime: `libtirrt` built for the platform (section 32.8), and the frameworks it uses (Kira's standard library, Kira UI, Ember, Foundation, and any other package) linked into it. Nothing in a platform image depends on a Tessera train at run time.
4. LibTessera is Tessera's system interface and exists only where Tessera runs: on Tarwin and on its other kernel backends. The `linux` platform of this section is an ordinary Linux distribution, which is unrelated to LibTessera's Linux backend: a Tessera system running on the Linux kernel is the `tessera` platform.

### 32.2 Platforms

| Code | Platform | Minimum for TIR 26.11.1 | Architectures | Image format | Ember backends (Ember.md section 30.1) |
| --- | --- | --- | --- | --- | --- |
| 1 | `tessera` | — | aarch64, x64, rv64 | Tarwin.md section 7 | Native |
| 2 | `macos` | macOS 26 on Apple silicon | aarch64 | Mach-O | Metal |
| 3 | `ios` | iOS and iPadOS 26 on A14 Bionic and later, and the iOS Simulator on Apple silicon | aarch64 | Mach-O | Metal |
| 4 | `windows` | Windows 11 version 24H2 | x64, aarch64 | PE32+ | Direct3D 12, Vulkan |
| 5 | `linux` | Linux 5.15 with glibc 2.35 | x64, aarch64, rv64 | ELF64 | Vulkan |
| 6 | `android` | Android 13 (API level 33) | aarch64, x64 | ELF64 inside APK and AAB | Vulkan |
| 7 | `web` | Browsers implementing every feature of `wasm.base` (TVISA.md section 32.2) | wasm | Wasm module with a JavaScript loader | WebGPU |
| 8 | `wasi` | WASI 0.2 runtimes with the component model and every feature of `wasm.base` | wasm | Wasm component | None |

Code 0 is reserved for portable modules (section 32.3). Apple platforms are aarch64-only because Ember's Metal 4 backend requires Apple silicon.

### 32.3 Portable and platform modules

1. Every module's header records its platform (section 22.2): 0 portable, or one of the codes of section 32.2.
2. A portable module imports neither LibTessera nor any platform library, contains no `call.foreign`, no `foreign.*` instruction and no `[foreign]` function type, and has profile `app` or `system`. It runs unchanged on every platform, including Tessera.
3. A `tessera` module may import LibTessera and, as its profile allows, private system RIDL modules. Modules of the `driver` and `kernel` profiles are `tessera` modules.
4. A module of any other platform may import that platform's libraries (section 32.6), and may not import LibTessera or private system RIDL modules or use the instructions of section 12.22.
5. A package lists its platforms in the manifest field `platforms` (section 29.3), `["tessera"]` by default. It contains its portable modules and, for each listed platform, that platform's modules. Building for platform P compiles the portable modules and the modules of P. Packages of the `app` and `system` profiles may list any platform; on platforms other than `tessera`, a `system` package's modules follow the `app` profile's rules, so a framework that is a system framework on Tessera ships from the same package everywhere else. Packages of the `driver` and `kernel` profiles list only `tessera`. A module of an unlisted platform is rejected with `TIR-PLAT-0004`, a module whose imports contradict its platform with `TIR-PLAT-0001`, and a driver or kernel package that lists another platform with `TIR-PLAT-0003`.
6. A RIDL module implemented separately per platform (for example a module of Kira's standard library whose implementation calls libSystem on `macos` and Win32 on `windows`) MUST have the same canonical `.ridl` hash and ABI UUIDs on every platform, so that portable modules link to it unchanged; `tirc` checks this when it links (`TIR-PLAT-0002`).
7. Rule V21: the verifier enforces items 2 to 4 for each module.

### 32.4 Toolchain targets

C, C++, Rust and Zig code that calls a platform's interfaces is compiled for that platform's TIR target. Like `tir64-unknown-tessera` (section 5.2), each target is architecture-neutral: it defines no architecture macro (except `__wasm__` on the two WebAssembly platforms, which have one architecture), it uses the fixed properties of section 5.1, and architecture-specific code lives in variant sets. Unlike it, each target defines the platform's operating-system macros and C data model, so that platform headers compile unchanged.

| Platform | Target | Differences from section 5.2's C data model | Operating-system macros |
| --- | --- | --- | --- |
| `macos` | `tir64-apple-macos26.0` | `char` signed; `long double` is `double` (8 bytes) | `__APPLE__`, `__MACH__`, `TARGET_OS_MAC` = 1, `TARGET_OS_OSX` = 1, `__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__` = the deployment target |
| `ios` | `tir64-apple-ios26.0`, `tir64-apple-ios26.0-simulator` | As `macos` | `__APPLE__`, `__MACH__`, `TARGET_OS_IPHONE` = 1, `TARGET_OS_IOS` = 1, `TARGET_OS_SIMULATOR` as appropriate, `__ENVIRONMENT_IPHONE_OS_VERSION_MIN_REQUIRED__` |
| `windows` | `tir64-pc-windows-msvc` | LLP64: `long` 4 bytes; `wchar_t` 2 bytes; `char` signed; `long double` is `double`; Microsoft bit-field layout and `#pragma pack` semantics; `__LP64__` not defined, `__LLP64__` defined | `_WIN32`, `_WIN64`, and the macros Clang defines for `*-pc-windows-msvc` targets with Microsoft compatibility version 19.40, without architecture macros |
| `linux` | `tir64-unknown-linux-gnu` | None. `long double` is IEEE binary128 (see item 2) | `__linux__`, `__linux`, `__gnu_linux__`, `__unix__`, `__unix`, `__ELF__` |
| `android` | `tir64-linux-android33` (the number is the package's minimum API level, at least 33) | None | `__ANDROID__`, `__ANDROID_API__` = the minimum API level, `__linux__`, `__unix__`, `__ELF__` |
| `web` | `tir64-unknown-web` | `char` signed | `__wasm__` |
| `wasi` | `tir64-unknown-wasi` | `char` signed | `__wasm__`, `__wasi__` |

1. Every target defines `__tir64__`, `__TIR_VERSION__`, the byte-order macros, `__STDC_IEC_559__` and `__FLT_EVAL_METHOD__` = 0 as section 5.2 does. Plain `char` on `linux` and `android` is unsigned, as on aarch64 and rv64; this has no effect on calls because the C conventions pass the declared type's value.
2. On `linux` x64 the platform's own `long double` is the x87 80-bit format, which TIR does not have (section 3.2). C code compiled for `tir64-unknown-linux-gnu` uses binary128 internally; a platform import whose platform signature contains `long double` is rejected on that pairing with `TIR-PLAT-0010`, and is unavailable through a variant set elsewhere. On every other platform and architecture the platform's `long double` is `double` or binary128 and matches.
3. rustc fork: `target_arch = "tir64"`, `target_pointer_width = "64"`, `target_os` = `macos`, `ios`, `windows`, `linux`, `android`, `unknown` or `wasi`, `target_family` = `unix`, `windows` or `wasm`, `target_env` = `gnu` on `linux`, `msvc` on `windows`, `sim` for the iOS Simulator and empty otherwise. The standard library is Rust's own built for the target over the platform's interfaces (libSystem, Win32 and the Universal CRT, glibc, Bionic, the browser host, WASI), never LibTessera.
4. Zig: `tir64-macos`, `tir64-ios`, `tir64-windows-msvc`, `tir64-linux-gnu`, `tir64-linux-android`, `tir64-freestanding-web`, `tir64-wasi`.
5. KLF frontends emit portable modules unless a module declares platform imports; Kira's platform modules are ordinary Kira modules whose package marks them with the platform.

### 32.5 Architectures and CPU targets

1. Platform images are built for the platform baselines and optional targets that TVISA.md section 32.2 defines:

| Platform | Architecture | Baseline target | Optional targets built by default |
| --- | --- | --- | --- |
| `macos`, `ios` | aarch64 | `aarch64.apple1` | — |
| `windows` | x64 | `x64.v2` | `x64.v3h`, `x64.v4h` |
| `windows` | aarch64 | `aarch64.v82` | `aarch64.v86` |
| `linux` | x64 | `x64.v2` | `x64.v3h`, `x64.v4h` |
| `linux` | aarch64 | `aarch64.v80` | `aarch64.v82`, `aarch64.v86`, `aarch64.v9` |
| `linux` | rv64 | `rv64.rva23` | — |
| `android` | aarch64 | `aarch64.v82` | `aarch64.v86`, `aarch64.v9` |
| `android` | x64 | `x64.v2` | `x64.v3h` |
| `web`, `wasi` | wasm | `wasm.base` | `wasm.m64` (web only, as a second module) |

2. A platform image contains one copy of the program for the baseline and, for each optional target, clones of the functions that benefit: those the optimizer vectorized or specialized by target features, those marked hot by profile data, and every variant-set member that needs the target's features. `libtirrt` selects clones and variants once per process through dispatch slots (TVISA.md section 32.10). `tirc build --clone-targets` overrides the defaults.
3. Optional targets are chosen only for the instructions they enable. Platform hardening (section 32.9) never depends on them.
4. The `web` loader feature-detects memory64 and loads the `wasm.m64` module when present, the `wasm.base` module otherwise. Both have the same semantics: under `wasm.base`, pointers keep TIR's 64-bit representation in memory and in values, and every access checks that the address lies within the 32-bit memory (TVISA.md section 32.11).
5. The modifiers `+threads` (shared memory and atomics) and `+relaxed-simd` are build options for `wasm` targets (section 32.8 item 4).
6. The all-three rule (section 20.3) applies on every platform for the architectures of the package's platforms; a package that lists `web` or `wasi` additionally needs, for every variant set it uses, a portable body or a `wasm` member, and for every set with assembly a `wasm` member (`TIR-PLAT-0005`). Inline assembly in a `wasm` member is WebAssembly text accepted by LLVM's WebAssembly assembler.
7. Page size is a load-time constant (`target.pagesize`) except where it is fixed: 16 KiB on Apple platforms and 65,536 bytes (the Wasm page) on `web` and `wasi`. Images are aligned for the largest page size of their platform: 16 KiB on Apple and Android, 64 KiB on `linux` aarch64, 4 KiB on `linux` x64 and rv64, 64 KiB section alignment on Windows.

### 32.6 Platform imports, exports and foreign calls

1. A platform module imports platform libraries with IMPORTS entries of kind 2 (section 22.4): the library name, a flag word (bit 0 weak), and the imported functions and data, each with its platform symbol name and its TIR type. Functions have `abi(c)` types.

| Platform | Library names |
| --- | --- |
| `macos`, `ios` | Install names: `/usr/lib/libSystem.B.dylib`, `/usr/lib/libobjc.A.dylib`, `/System/Library/Frameworks/<Name>.framework/<Name>` |
| `windows` | DLL names, including API sets: `kernel32.dll`, `user32.dll`, `ucrtbase.dll`, `api-ms-win-core-synch-l1-2-0.dll` |
| `linux` | Sonames: `libc.so.6`, `libm.so.6`, `libdl.so.2`, `libwayland-client.so.0`. Graphics and windowing libraries the program loads optionally are reached through `dlopen` |
| `android` | Sonames of the NDK's stable libraries for the minimum API level: `libc.so`, `libm.so`, `libdl.so`, `liblog.so`, `libandroid.so`, `libvulkan.so`, `libaaudio.so`, `libnativewindow.so` |
| `web` | JavaScript module names supplied by the page or by framework packages (section 32.7) |
| `wasi` | WIT interface names with versions, for example `wasi:filesystem/types@0.2.0` |

2. Binding is eager: the platform loader resolves every import when the image loads, and the image's import slots are read-only afterwards (TVISA.md section 32.10). A weak import that the system lacks resolves to null. `tirc` checks every non-weak import against the platform's interface database for the package's minimum OS version and rejects absent or newer symbols with `TIR-PLAT-0006`: Apple's SDK text-based stubs from the SDK the developer supplies (`--sdk`), Windows metadata, glibc's version scripts at the minimum glibc (imports bind to the oldest symbol version that has the function), the NDK's stub libraries for the API level, and the WIT packages of the WASI version.
3. `call.foreign @import(args)` calls an imported function directly (section 12.11). `call.indirect` through a function value of a type marked `[foreign]` (section 7.6) calls platform code.
4. A `[foreign]` function value comes from `foreign.addr` (an imported function, null for an absent weak import), from `foreign.entry` (a TIR function made callable by platform code), or from `bitcast` of a `ptr` loaded from platform data (a COM vtable entry, a JNI function table entry, a callback a platform API returns). `bitcast` between `ptr` and `[foreign]` function types is permitted in both directions; rule V19 still forbids it for every other function type.
5. A call through a `[foreign]` value does not check a TIR type tag, because platform code has none. On Windows it goes through Control Flow Guard's dispatch (TVISA.md section 32.5); elsewhere the platform's own landing pads apply where the platform enables them.
6. `foreign.entry @f` requires `@f` to have `abi(c)`. The value addresses an entry with the platform's landing pad, recorded in the platform's control-flow tables (Windows Control Flow Guard's function table, the Wasm table), so platform code may call it. TIR code calling the same function directly or through a TIR function value still checks its type tag.
7. A function with the attribute `export.platform "symbol"` and `abi(c)` is exported under that platform symbol name: in the dynamic symbol table (ELF, Mach-O), the export directory (PE), or the export section (Wasm). Platform symbol names follow the platform's C convention, including the leading underscore on Apple platforms.
8. Only trivial values cross a foreign call: integers, floating-point values, `ptr`, `[foreign]` function values, vectors of at most 16 bytes, and C aggregates of trivial members passed with the `c` convention's aggregate rules. Owned, borrowed and reference-counted values never cross (`TIR-PLAT-0007`).
9. `call.foreign` and calls through `[foreign]` values are `[nounwind]` unless they are `invoke` with the `[foreign_unwind]` attribute, whose landing pad uses a foreign personality (section 14.6): C++ on every platform with a C++ runtime, and Objective-C on Apple platforms. A foreign exception reaching any other TIR frame terminates the process with `TIR-RT-0020`. A TIR exception never enters platform frames: `foreign.entry` functions are `nounwind`, and a TIR exception escaping one terminates with `TIR-RT-0020`.
10. Functions that call platform code are unmanaged with reason bit 3 (section 21.4). The CAPS section of a platform module lists the platform imports it reaches, and the analysis report lists them per function, so a reviewer sees exactly which platform interfaces a package uses.

### 32.7 Interoperation with platform runtimes

| Runtime | Platforms | Rules |
| --- | --- | --- |
| Objective-C | `macos`, `ios` | Messages are calls to `objc_msgSend` imported once per distinct method signature with that exact C signature (arm64 has no `_stret` or `_fpret` variants); `objc_msgSendSuper2` for messages to the superclass. Selectors and classes are resolved at image initialization with `sel_registerName` and `objc_lookUpClass` and kept in globals. Classes implemented in TIR are registered at initialization with `objc_allocateClassPair`, `class_addIvar` (one pointer-sized instance variable for the object's TIR state), `class_addMethod` (implementations are `foreign.entry` values with their type encodings) and `objc_registerClassPair`. Reference counting is explicit through `objc_retain`, `objc_release` and `objc_autorelease`, and autorelease pools through `objc_autoreleasePoolPush` and `objc_autoreleasePoolPop`. Blocks are laid out by the Clang block ABI, with `_NSConcreteGlobalBlock` and `_NSConcreteStackBlock` as data imports and `_Block_copy` and `_Block_release` as function imports. Objective-C exceptions are foreign exceptions. Interfaces available only to Swift are not reachable |
| COM and Windows Runtime | `windows` | Interfaces are vtables of `[foreign]` function values called with the `c` convention; `HRESULT` is `i32`; reference counting through `AddRef` and `Release` is explicit. Interfaces implemented in TIR are static vtables of `foreign.entry` values. The Windows Runtime is used through its ABI: activation factories from `combase.dll`, `HSTRING` functions, and COM interfaces |
| Java Native Interface | `android` | The application shell (section 32.10) loads the program's image and calls its exported `JNI_OnLoad`. Java calls TIR through methods registered with `RegisterNatives` whose implementations are `foreign.entry` values; TIR calls Java through the `JNIEnv` and `JavaVM` function tables. Local references are deleted explicitly; threads created outside Java attach with `AttachCurrentThread` before using JNI |
| JavaScript | `web` | Imports come from the JavaScript modules a library name designates. Arguments and results are WebAssembly numbers; JavaScript values are referenced by 32-bit handles in the handle table of `tir-host.mjs`, which every `web` build includes; strings cross as UTF-8 in linear memory. Exports are called by JavaScript with the same rules |
| WASI | `wasi` | Interfaces are imported by WIT name. `tirc` generates the canonical ABI lowering and lifting adapters, including the conversion of TIR's 64-bit pointers to the component's 32-bit memory addresses with the range check of TVISA.md section 32.11 |
| C and C++ runtimes | All except `tessera` | C producers use the platform's C library: libSystem, the Universal CRT (`ucrtbase.dll`), glibc, Bionic, and wasi-libc built for `tir64-unknown-web` and `tir64-unknown-wasi`. C++ producers use LLVM's libc++ and libc++abi built for the platform target and linked into the image |

### 32.8 The runtime on platforms

1. Every platform image links `libtirrt` built for its platform: statically into an executable, or, for an application made of several images (plugins, Android feature modules), as one shared image bundled with the application. It provides the items of section 28.10 except the compile-service client: the JIT, when present, runs in the process.

2. Engines and dynamic TIR:

| Platform | Interpreter | JIT tiers | Dynamic TIR |
| --- | --- | --- | --- |
| `macos` | Yes | Yes, with the hardened-runtime entitlement `com.apple.security.cs.allow-jit` | Yes |
| `ios` | Yes | No | Interpreted only |
| `windows` | Yes | Yes | Yes |
| `linux`, `android` | Yes | Yes | Yes |
| `web` | Yes | Yes, as WebAssembly modules compiled by the browser when the page's content security policy allows `wasm-unsafe-eval`; interpreter otherwise | Yes |
| `wasi` | Yes | No | Interpreted only |

Dynamic TIR (section 28.7) is enabled by the build option `--dynamic-tir` instead of an entitlement; a build without it rejects `TirModule.load` with `TIR-RT-0301`. Requesting the JIT where the table says no is rejected at build time with `TIR-PLAT-0011`.

3. JIT code memory is never writable and executable at once at any address: on `macos`, memory from `mmap` with `MAP_JIT`, toggled per thread with `pthread_jit_write_protect_np`; on `windows`, two views of one section object (`CreateFileMapping`, `MapViewOfFile3`), one read-write and one read-execute; on `linux` and `android`, two views of one `memfd`; on `web`, separate WebAssembly modules instantiated against the program's memory and table.

4. Threads are created by platform interfaces (by the Kira and Rust standard libraries, or by producers through platform imports). A thread calls `tir_rt_thread_attach` before running TIR code and `tir_rt_thread_detach` before it ends; the standard libraries do this. On `web`, threads require `+threads` and a cross-origin-isolated page (`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`); each worker instantiates the module with the shared memory. A `wasm` build without `+threads` has exactly one thread, and its atomics lower to ordinary operations.

5. The allocator obtains spans from `mmap` (Apple platforms, `linux`, `android`), `VirtualAlloc2` (`windows`) and `memory.grow` (`web`, `wasi`). Spans are aligned to their own size, at least 64 KiB on every platform. A `wasm.base` program addresses at most 4 GiB.

6. Memory tagging (section 27.4) is used on `android` when the application's manifest enables `memtagMode` and the CPU has MTE (synchronous in development builds, asynchronous otherwise), and on `linux` aarch64 when the CPU has MTE, through `prctl(PR_SET_TAGGED_ADDR_CTRL)`. Heap tagging uses variant bodies selected at load; stack tagging is not used on platforms.

7. Traps, faults and crashes:

| Platform | Delivery of TIR traps | Report |
| --- | --- | --- |
| `macos`, `ios` | `SIGTRAP` from `brk`; faults as `SIGSEGV` and `SIGBUS`; handlers on an alternate signal stack | The `TIR-RT` code and location go to `os_log`; the process then aborts so that the system crash report records it |
| `windows` | Vectored exception handler for breakpoint, illegal-instruction and access-violation exceptions | `OutputDebugString` and standard error, then `RaiseFailFastException` so Windows Error Reporting records it |
| `linux` | `SIGTRAP` (aarch64, rv64) and `SIGILL` (x64); faults as `SIGSEGV`, `SIGBUS` | Standard error and the journal, then `abort` |
| `android` | As `linux` | `liblog` at fatal priority, then `abort`, recorded in the tombstone |
| `web` | The trap code is stored in the global `__tir_trap_code` and `unreachable` executes | `tir-host.mjs` reads the code and reports it to the console and to the page's error handler |
| `wasi` | As `web` | The code is written to standard error by the host adapter when the runtime reports the trap |

8. Stack overflow is detected by guard pages and reported as `TIR-RT-0021` from the alternate signal stack, or after `SetThreadStackGuarantee` on Windows; on `web` and `wasi`, by the explicit stack-limit checks of TVISA.md section 32.11 and the engine's own stack limit.

9. Unwinding uses the platform's unwind tables (TVISA.md section 32.7). Hot module push (section 28.9) is available in development builds on every platform; on `ios` and `wasi` pushed functions run on the interpreter.

10. `libtirrt`'s messages go to the platform's log: `os_log` on Apple platforms, standard error and `OutputDebugString` on Windows, standard error on Linux, `liblog` on Android, the console on the web, standard error under WASI.

### 32.9 Hardening on platforms

TIR's type-tag check on every TIR indirect call (section 21.3) is software and applies on every platform. The protections that depend on the hardware and the operating system follow this table, which replaces sections 27.2 and 27.3 on platforms; TVISA.md section 32.5 gives the exact sequences.

| Protection | Apple platforms | `windows` | `linux`, `android` | `web`, `wasi` |
| --- | --- | --- | --- | --- |
| Landing pads | `bti c` (hint space) on aarch64 | `endbr64` on x64; `bti c` (hint space) on aarch64 | `bti c` (hint space) on aarch64, `endbr64` on x64, `lpad` on rv64 | Inherent: calls only reach table entries |
| Indirect calls to platform code | Unchecked (platform code) | Control Flow Guard dispatch | Unchecked | `call_indirect` signature check |
| Return addresses | `paciasp` and `autiasp` (hint space) | x64: shadow-stack compatible (`CETCOMPAT`); aarch64: `paciasp` and `autiasp` (hint space) | aarch64: `paciasp` and `autiasp` (hint space); x64: shadow-stack compatible, marked in the GNU property note; rv64: `sspush` and `sspopchk` (Zimop encodings) | Inherent: the call stack is not addressable |
| Code pointers in memory | Not signed | Not signed | Not signed | Table indices |
| Memory tagging | No | No | Item 6 of section 32.8 | No |
| Stack protector, zero-initialization, index masking | Yes | Yes | Yes | Yes |
| Address randomization | PIE | `DYNAMICBASE`, `HIGHENTROPYVA` | PIE | Not applicable |

Hint-space instructions execute as no-ops where the CPU or the operating system does not enable them, so one image is correct on every CPU of its baseline and protected wherever the system allows.

### 32.10 Images, bundles and signing

1. Image kinds: executables; shared libraries (`.dylib`, `.dll`, `.so`, Wasm modules); and static libraries (`.a`, `.lib`) that embed TIR code, with `libtirrt`, into applications built with the platform's own tools (Xcode, Visual Studio, Gradle), exposing `abi(c)` functions marked `export.platform`.

2. `tirc build --platform <p> --bundle` produces the platform's distributable form:

| Platform | Output |
| --- | --- |
| `macos` | `<Name>.app` (`Contents/MacOS`, `Contents/Info.plist`, `Contents/Resources`, `Contents/Frameworks` when shared images are used), or a single executable for tools |
| `ios` | `<Name>.app` with `Info.plist`, `embedded.mobileprovision` and the entitlements, and `<Name>.ipa` |
| `windows` | `<Name>.exe` with its resources (application manifest declaring UTF-8 as the active code page, long-path awareness and per-monitor DPI awareness v2; icon; version information), and optionally `<Name>.msix` |
| `linux` | The executable and its resource directory; for graphical applications a `.desktop` file and icons |
| `android` | `<Name>.apk` and `<Name>.aab` containing `lib/<abi>/lib<name>.so` for each architecture, the Kira application shell (`classes.dex`: an activity that loads the library, forwards lifecycle, input and surface events through JNI, and contains no other logic), and the binary manifest and resources `tirc` generates |
| `web` | `<name>.wasm`, `<name>.m64.wasm` when built, `<name>.mjs` (the loader), `tir-host.mjs`, framework JavaScript, and an `index.html` from the package's template |
| `wasi` | `<name>.wasm`, a component exporting `wasi:cli/run` for commands or the package's WIT world for libraries |

3. The manifest's `bundle` object (Appendix A.5) supplies what bundles need: the identifier (bundle identifier, Android application ID, Windows package identity), display name, version and build number, icon set, the minimum OS version (not below section 32.2's), permission declarations and usage descriptions, Apple entitlements, Android permissions and features, MSIX capabilities, URL schemes and document types. A bundle whose required fields are missing is refused with `TIR-PLAT-0021`.

4. Signing:

| Platform | Signature |
| --- | --- |
| `macos` | Apple code signature on every Mach-O image and on the bundle, with the developer's Developer ID or Apple Distribution certificate, the hardened runtime and the entitlements file; `tirc notarize` submits the result to Apple's notary service |
| `ios` | Apple code signature with the developer's Apple Development or Apple Distribution certificate and the provisioning profile |
| `windows` | Authenticode with SHA-256 digests and an RFC 3161 timestamp on every image and on the MSIX package |
| `android` | APK Signature Schemes v2 and v3 on APKs, and the upload key's JAR signature on AABs |
| `linux`, `web`, `wasi` | No platform signature; `tirc` can write a detached signature (Ed25519 and ML-DSA-65) over each image's build identifier for the developer's own distribution |

`tirc` implements every signature format itself and runs on any developer host (section 26.1), so Apple and Windows outputs can be produced and signed on Linux; the private keys come from files or hardware tokens the developer names.

5. Platform outputs are reproducible as section 26.8 requires, except for signature blocks and their timestamps.

### 32.11 GPU code on platforms

1. Platform images carry, in their GPU section, the hosted backend code of every GPU entry instead of GPU family binaries: Metal Shading Language (and a Metal library when `tirc` finds Apple's Metal toolchain on the build host) on Apple platforms; DXIL and SPIR-V on `windows`; SPIR-V on `linux` and `android`; WGSL on `web`. The translation and validation rules are Ember.md section 30.3, and the target codes TVISA.md section 31.5.
2. Ember is not available on `wasi`; GPU entries in a `wasi` build are rejected with `TIR-PLAT-0012`.
3. `tirc run --gpu-reference` runs GPU entries on the reference executor on every developer host.

### 32.12 Debug information and crash reports

| Platform | Debug format | Kept in | Matched by |
| --- | --- | --- | --- |
| `macos`, `ios` | DWARF 5 | A `.dSYM` bundle | `LC_UUID`, the first 16 bytes of the build identifier |
| `windows` | CodeView | A PDB 7.0 file | The debug directory's GUID (the first 16 bytes of the build identifier) and age 1 |
| `linux`, `android` | DWARF 5 | A separate ELF file with `.gnu_debuglink` | `.note.gnu.build-id`, the first 20 bytes of the build identifier |
| `web`, `wasi` | DWARF 5 | A separate Wasm file named by the `external_debug_info` custom section; optionally a source map | The `tir.build` custom section |

`tirc symbolicate` accepts Apple crash reports (`.ips`), Windows minidumps, Android tombstones, Linux core files, and browser and WASI stack traces, together with the debug files above and the symbol bundles of section 33.

### 32.13 Versions and platform minimums

1. `tirc` of train T builds platform images from TIR of the 26 trains ending at T, like Tessera builds.
2. Platform images never expire and depend on no train at run time. Each records the TIR version and the `tirc` build identifier in its `tir.build` section.
3. The minimums of section 32.2 belong to each TIR version. A TIR version raises a minimum only when the platform's vendor ends support for it or when an Ember hosted backend's floor (Ember.md section 30.1) requires it, and records the change under the heading `Platforms` in `TIR-CHANGES.md`. New platforms are added by TIR versions with new codes.

### 32.14 Conformance on platforms

| Suite | Content |
| --- | --- |
| Semantics parity | The parity suite of section 34.1 on every platform and architecture pair: `macos` aarch64; `ios` aarch64 on devices and in the Simulator; `windows` x64 and aarch64; `linux` x64, aarch64 and rv64; `android` aarch64 and x64; `web` with `wasm.base` and `wasm.m64` in the Chromium, Gecko and WebKit engines; `wasi` in Wasmtime and one other runtime. Output and traps are identical to Tessera's |
| C conventions | Calls and callbacks for every signature shape (aggregates of every classification, homogeneous floating-point aggregates, variadic calls, Microsoft bit-fields) against code compiled by the platform's C compiler (Clang, and MSVC on Windows) |
| Runtime interoperation | Objective-C messaging and class registration, blocks, COM and Windows Runtime calls and implementations, JNI in both directions, JavaScript imports and exports, WASI interfaces through the canonical ABI |
| Images | Validation with the platform's tools: `codesign --verify` and `dyld` loading on Apple platforms, `signtool verify` and loader checks on Windows, `readelf` checks and Android's 16 KiB alignment check, `wasm-tools validate` |
| Hardening | The table of section 32.9 audited per platform and architecture |
| Performance | Section 34.4's gates per platform and architecture, against the same source compiled by Clang for the platform at the same level; `wasm.base` and `wasm.m64` output against Clang's `wasm32` and `wasm64` output: ≤ 1.10× and ≤ 1.05× |

---

## 33. Debug information

### 33.1 The DEBUG section

| Content | Encoding |
| --- | --- |
| Files | Paths relative to the package root (or absolute under `--debug-paths=absolute`), optional BLAKE3 content hashes, embedded text for generated files |
| Compile units | Source language, producer |
| Scopes | Functions, lexical blocks, inlined-at chains |
| Locations | Per instruction `(file, line, column)`, delta-encoded and keyed by instruction ordinal |
| Variables | Name, source type, scope, and location through `debug.value` and `debug.addr` |
| Source types | A type graph sufficient to emit DWARF 5: base types with encodings, structs and classes with member names and offsets, enums and variants with names, pointers, references, typedefs, template parameters |
| Labels | `debug.label` targets |

Variable and label records are zero-based in their order within the DEBUG section. The corresponding debug instructions store those indices as ULEB values.

DEBUG is eight counted tables, in order: files, compile units, source types,
scopes, locations, variables, labels and DATA records. All ULEB indexes are
zero-based unless a field below explicitly uses zero as an absent sentinel.
Every referenced string is a nonempty STRINGS entry. A file is a path string
index, `u8` flags (bit 0 content hash, bit 1 generated source), then the
optional 32-byte BLAKE3 hash and optional generated-source UTF-8 byte length
and bytes. A compile unit is a source-language and producer string index.

Source types are indexed records with a `u8` tag: 0 base (name string index,
DWARF 5 `DW_ATE` encoding byte, nonzero bit width); 1 struct/class (name,
`u8` kind 0 struct or 1 class, size, byte alignment, member count, then
members of name string index, source-type index and byte offset); 2 enum (name,
underlying source-type index, variant count, then variant name and sign plus
ULEB128 magnitude); 3 pointer (pointee type); 4 reference (pointee type and
`u8` mutable flag); 5 typedef (name and target type); 6 template parameter
(name, `u8` kind 0 type or 1 value, and for kind 1 the value type); 7 array
(element type, `u8` count-present flag, and optional count); 8 qualified type
(target type and qualifier bits: const 1, volatile 2, restrict 4). Type
references may point forward or form cycles; every index must be in range.
Composite member offsets do not exceed the composite size, alignments are
positive powers of two, base encodings are nonzero DWARF 5 `DW_ATE` values,
and names carried by named source-type variants are nonempty. In TIR, DEBUG
DATA-record indexes address GLOBALS records in declaration order; when lowering
to TVISA, the compiler remaps them to the emitted DATA order.

A scope is `u8` kind, function index, compile-unit index and parent index
encoded as zero for absent or index plus one. Kind 0 is a function scope and
has no parent; kind 1 is a lexical scope and has an earlier parent in the same
function; kind 2 is an inlined scope and has an earlier parent followed by a
call-site file, one-based line and one-based column. A location is function
index, instruction-ordinal delta, file index, one-based line, one-based column
and scope index plus one (zero means absent). Ordinals reset at each function;
locations are strictly ordered by function and ordinal. Variables are name,
source-type and scope indexes. Labels are name, scope and source location.
The final DATA table contains DATA index, source-type index and source
location, ordered by DATA index; TVISA DATA records use zero for no debug
record or one plus this record's ordinal.

### 33.2 Native code

`tirc` emits DWARF 5 for native images and moves it into the symbol bundle. Images keep their build identifier and the unwind tables the runtime needs. The pipeline runs a symbol server from which developer tools fetch symbol bundles by build identifier; stock LLDB debugs native code with them. Platform images use their platform's debug formats (section 32.12).

### 33.3 Interpreter and JIT

JIT code is registered with debuggers through an in-memory object interface equivalent to the GDB JIT interface. Interpreter frames are presented to LLDB through the `tir-lldb` plugin as source-level frames. A debugger sees source-level frames in every tier.

### 33.4 GPU code

GPU binaries carry line tables where the family's backend supports them. `gpu.printf` (capability `printf`) is available in development builds. `tirc run --gpu-reference` runs GPU entries on the reference executor, where they can be stepped like CPU code.

### 33.5 Delivered TIR

Runtime TIR in release delivery units keeps line tables and drops variable information, so crash reports from interpreted code resolve to source lines.

---

## 34. Conformance and diagnostics

### 34.1 Suites

| Suite | Content |
| --- | --- |
| Encoding | Text and binary round trips; canonical re-encoding; fuzzed malformed inputs are rejected without crashing |
| Verifier | A positive and a negative case for each of V1 to V21, S1 to S7, each GPU restriction and the uniformity analysis |
| Semantics parity | Every CPU test program on the interpreter, both JIT tiers, LLVM at `-O0` and `-O3`, the Cranelift-class backend and finalized TVISA, on aarch64, x64 and rv64 (under emulation where hardware is absent): identical output and identical traps |
| Variant equivalence | Every variant set of the corpus, compared member against member |
| Converters | LLVM test-suite; Rust standard library and compiler UI tests through the rustc fork; Zig standard library tests; SQLite, Lua and zlib through Clang for tir64; the WebAssembly specification test suite with memory64 and the accepted proposals; the Vulkan conformance test suite through Vulkan-on-Ember |
| GPU | Every GPU construct against the reference executor on `tvgpu` and on every family backend; Ember's sample suite |
| Hardening | Every indirect call checked; every address-taken function with a landing pad and a tag; shadow stacks active; no writable and executable memory; zero-initialization of unproven slots |
| Reproducibility | Two independent builds of the corpus are bit-identical; finalizer output is deterministic |
| Runtime | Allocator, counting by interior pointer, quarantine, weak references, cycle detector, heap balance, dynamic TIR capability bounds, hot push reason codes |
| Pipeline | Intake rejections, windows, expiry, signatures, source reproducibility |
| Platforms | Section 32.14 |

### 34.2 KLF corpora

The KLF Kira test harness, `tests-kik`, and the Kira ecosystem packages (Kira UI, Ember, Foundation) MUST pass on TIR before KIR is deleted (milestone T4).

### 34.3 Parity rules

Results MUST be bitwise identical across tiers and architectures, except for instructions carrying fast-math flags, `[approx]`, GPU `fmuladd`, or `fp(fast)`, whose results MUST satisfy the bounds of sections 12.3 and 17.16, and except for values the semantics leave unspecified, which tests MUST NOT observe.

### 34.4 Performance gates

| Gate | Requirement |
| --- | --- |
| Native image against the same source compiled directly by the same LLVM at the same level for the same CPU | Geometric mean runtime ≤ 1.03× on the benchmark corpus (LLVM test-suite benchmarks, Rust benchmarks, Kira benchmarks), per CPU target |
| Finalized TVISA against the pipeline's native image | ≤ 1.20× |
| Wasm64 converter output against the same program compiled natively from C | ≤ 1.25× |
| `tirc` compile time and memory | Section 26.9 |
| Platform images | Section 32.14 |

### 34.5 Diagnostics

Codes have the form `TIR-<AREA>-<NNNN>`.

| Area | Meaning |
| --- | --- |
| `ENC` | Encoding |
| `VER` | Version and window |
| `VFY` | Verifier; `TIR-VFY-00NN` is rule VNN |
| `OWN` | Ownership and canonicalization |
| `CF` | Structured control flow; `TIR-CF-000N` is rule SN |
| `GPU` | GPU restrictions, uniformity, capabilities, shapes |
| `ARCH` | Variants and assembly |
| `CAP` | Capabilities |
| `GEN` | Generics |
| `IMP` | Converters |
| `PKG` | Packages |
| `PROF` | Profiles |
| `PIPE` | Pipeline |
| `RT` | Runtime traps and load errors |
| `JIT`, `FIN` | JIT tiers, finalizer |
| `PLAT` | Platform targets |

| Code | Meaning |
| --- | --- |
| `TIR-ENC-0001` | Malformed encoding |
| `TIR-ENC-0002` | Non-canonical module in a package |
| `TIR-VER-0001` | Version older than the window |
| `TIR-VER-0002` | Version newer than the compiler's train |
| `TIR-OWN-0001` | Raw TIR in a package |
| `TIR-OWN-0010` to `TIR-OWN-0015` | `own.use-after-move`, `own.borrow-escape`, `own.consume-while-borrowed`, `own.exclusivity`, `init.use-before-init`, `init.double-init` |
| `TIR-GPU-0001` | GPU restriction violated |
| `TIR-GPU-0002` | Uniformity requirement not proven |
| `TIR-GPU-0003` | Capability used but not declared |
| `TIR-GPU-0004` | Cooperative-matrix shape unsupported by the target families |
| `TIR-ARCH-0001` | Assembly outside a variant body |
| `TIR-ARCH-0002` | Target intrinsic of another architecture |
| `TIR-ARCH-0003` | All-three rule violated |
| `TIR-CAP-0001` | Implied entitlement not requested |
| `TIR-CAP-0002` | Entitlement not allowed in the profile |
| `TIR-CAP-0100` | Requested entitlement unused (remark) |
| `TIR-GEN-0001` | Generic cannot be exported polymorphically |
| `TIR-IMP-0001` to `TIR-IMP-0005` | LLVM converter rejections (section 25.3) |
| `TIR-IMP-0010` | Unsupported personality |
| `TIR-IMP-0020`, `TIR-IMP-0021` | Wasm64 converter rejections |
| `TIR-PKG-0001` | Invalid manifest |
| `TIR-PKG-0002` | File hash mismatch |
| `TIR-PKG-0003` | Invalid signature or certificate |
| `TIR-PKG-0004` | Modules of different TIR versions |
| `TIR-PKG-0005` | Kind and profile inconsistent |
| `TIR-PROF-0010` | Source language not allowed in the driver profile |
| `TIR-PROF-0011` | License not allowed in the driver profile |
| `TIR-PROF-0012` | Source language other than Kira in the kernel profile |
| `TIR-PROF-0013` | Driver source-reproducibility hash absent or malformed, or source hash present outside the driver profile |
| `TIR-PIPE-0001` | Intake failure |
| `TIR-PIPE-0005` | Sanitized build submitted |
| `TIR-PIPE-0010` | Driver source not reproducible |
| `TIR-PIPE-0020` | Incompatible with the train after rebuild |
| `TIR-PIPE-0030` | Withdrawn on expiry |
| `TIR-PLAT-0001` | Module's imports or instructions contradict its platform |
| `TIR-PLAT-0002` | A RIDL module differs between platforms |
| `TIR-PLAT-0003` | Driver or kernel package lists a platform other than `tessera` |
| `TIR-PLAT-0004` | Module of a platform the package does not list |
| `TIR-PLAT-0005` | Variant set lacks a member or portable body for an architecture of the package's platforms |
| `TIR-PLAT-0006` | Platform import absent from the platform's interfaces at the minimum OS version, or newer than it without being weak |
| `TIR-PLAT-0007` | Non-trivial value crosses a foreign call |
| `TIR-PLAT-0010` | Platform `long double` in the x87 format on `linux` x64 |
| `TIR-PLAT-0011` | Engine unavailable on the platform |
| `TIR-PLAT-0012` | GPU entries in a `wasi` build |
| `TIR-PLAT-0020` | Signing failed |
| `TIR-PLAT-0021` | Bundle metadata missing |

Diagnostics render as `severity[code]: message`, followed by the source line from debug information when present, a caret range, notes and fixes. `--diagnostics=json` emits one JSON object per line.

### 34.6 Runtime codes

`trap code` takes a 16-bit code. Codes 0 to 32767 are the codes below, reported as `TIR-RT-` followed by the code in four decimal digits. Codes 32768 to 65535 are producer-defined and reported through the producer's diagnostic registry (for KLF languages, the definition's registry).

| Code | Meaning |
| --- | --- |
| `TIR-RT-0001` | `unreachable` executed, or a variant precondition failed |
| `TIR-RT-0002` | Integer overflow in trap mode |
| `TIR-RT-0003` | Exclusivity violation |
| `TIR-RT-0004` | Use of a deleted object |
| `TIR-RT-0005` | Index out of range |
| `TIR-RT-0006` | Raw free of a slot that is already free |
| `TIR-RT-0007` | Misaligned atomic access |
| `TIR-RT-0010` | Division by zero |
| `TIR-RT-0011` | Invalid float-to-integer conversion in trap mode |
| `TIR-RT-0012` | Control-flow integrity violation |
| `TIR-RT-0013` | Return-address integrity violation |
| `TIR-RT-0014` | Stack protector check failed (TVISA.md section 17.6) |
| `TIR-RT-0020` | Exception reached a boundary it may not cross |
| `TIR-RT-0021` | Stack overflow |
| `TIR-RT-0022` | Coroutine or future resumed after completion |
| `TIR-RT-0024` | Exception reached a `nounwind` frame |
| `TIR-RT-0030` | Sandbox access out of bounds |
| `TIR-RT-0031` | Sandbox indirect-call signature mismatch |
| `TIR-RT-0032` | Memory tag check fault |
| `TIR-RT-0033` | WebAssembly access outside the program's memory or in its first 64 KiB |
| `TIR-RT-0100` | Heap imbalance at exit (development) |
| `TIR-RT-0101` | Reference cycle leak (development) |
| `TIR-RT-0200` | RIDL link check failed |
| `TIR-RT-0201` | RIDL import bound to a retired ABI (warning) |
| `TIR-RT-0202` | Two copies of a module with different current ABIs |
| `TIR-RT-0203`, `TIR-RT-0204` | Cross-process RIDL errors (KLF.md section 24.7) |
| `TIR-RT-0205` | Inlinable body hash mismatch |
| `TIR-RT-0300` | Dynamic TIR exceeds the process's capability declaration |
| `TIR-RT-0301` | Dynamic CPU TIR without `tessera.tir.dynamic` |
| `TIR-RT-0400` | Delivery unit expired |
| `TIR-RT-0401` | Image signature invalid |
| `TIR-RT-0402` | Image built for another train |

---

## 35. Implementation plan and crate layout

### 35.1 Crates

| Layer | Crates |
| --- | --- |
| 0 | `tir-core` (identifiers, interner, type identifiers, hashing), `tir-diag`, `tir-target` (CPU targets, GPU families, the tir64 model) |
| 1 | `tir-format` (binary, text, canonical encoding), `tir-types`, `tir-layout` |
| 2 | `tir-verify` (V and S rules), `tir-own` (canonicalization), `tir-uniformity`, `tir-analysis` (capabilities, trust classes, control-flow-integrity types) |
| 3 | `tir-generics`, `tir-coro`, `tir-opt` |
| 4 | `tir-platform` (platforms, interface databases, interoperation adapters), `tir-image-macho`, `tir-image-pe`, `tir-image-elf`, `tir-image-wasm`, `tir-tvisa` (lowering to TVISA), `tir-llvm`, `tir-cranelift`, `tir-gpu-amd`, `tir-gpu-nvidia`, `tir-gpu-intel`, `tir-gpu-adreno`, `tir-gpu-mali`, `tir-gpu-powervr`, `tir-gpu-agx`, `tir-gpu-tvgpu`, `tir-gpuref` |
| 5 | `tir-interp`, `tir-jit`, `tir-finalize`, `tir-rt` (`libtirrt`) |
| 6 | `tir-import-llvm`, `tir-import-wasm64`, `tir-import-spirv` |
| 7 | `tir-pkg` (packages, archives, signatures), `tir-bundle` (platform bundles, APK and AAB, MSIX, Apple code signatures, Authenticode, APK signatures), `tir-pipeline`, `tir-service` (compile service), `tir-push` |
| 8 | `tir-cli` (`tirc`), `tir-test` (conformance runner) |

No crate depends on a higher layer. `tir-interp`, `tir-rt` and every crate beneath them build for every Tessera CPU target and every platform target.

### 35.2 Milestones

| Milestone | Content | Exit test |
| --- | --- | --- |
| T0 | Binary and text formats, canonical encoding, structural verifier | Encoding suite |
| T1 | Types, layout, full verifier, canonicalization | Verifier suite |
| T2 | Interpreter and `libtirrt` (porting the KLF runtime) | Parity suite on the interpreter |
| T3 | TVISA lowering and the LLVM backend for aarch64; ridlcc | Interpreter and LLVM parity on aarch64; RIDL convention suite |
| T4 | KLF emits TIR; KIR, the KLF VM and KLF backends deleted | Section 34.2 |
| T5 | x64 and rv64; variant sets; hardening | Parity on three architectures; hardening suite |
| T6 | LLVM converter; tir64 forks of Clang, rustc and Zig; the Tessera Rust standard library through TIR | Converter corpora for LLVM, Rust, SQLite, Lua, zlib |
| T7 | Packages, signatures, analysis, pipeline intake, build and signing, delivery units, loader integration | Pipeline suite |
| T8 | Compile service, JIT tiers, finalizer, code cache, dynamic TIR, hot push | Runtime suite; finalizer determinism; KavaScript corpus |
| T9 | GPU model, uniformity analysis, reference executor, `tvgpu` backend | GPU suite on the reference executor and `tvgpu` |
| T10 | GPU family backends, one at a time | GPU suite per family |
| T11 | Wasm64 and SPIR-V converters; Vulkan-on-Ember | WebAssembly specification tests; Vulkan conformance tests |
| T12 | `-O3` link-time optimization, profile-guided optimization, performance and scale gates; kernel and driver profiles | Section 34.4; Tarwin builds through TIR; a DeviceKit driver passes source reproducibility |
| T13 | Platform targets `linux` and `android`: toolchain targets, ELF images, platform runtime, APK and AAB | Section 32.14 on both platforms |
| T14 | Platform targets `macos`, `ios` and `windows`: Mach-O and PE images, Objective-C and COM interoperation, Apple and Authenticode signing | Section 32.14 on the three platforms |
| T15 | Platform targets `web` and `wasi`: WebAssembly code generation, the JavaScript loader, WASI adapters | Section 32.14 on both platforms |

### 35.3 Working rules

One agent owns one crate at a time. A change in behavior updates this specification in the same commit. Every change is reviewed by at least one automated reviewer before merge.

---

## 36. Appendix A: grammars and schemas

Notation: `{ x }` repetition, `[ x ]` option, `|` alternative, quoted terminals. Newlines separate items and instructions. Comments run from `;` to the end of the line.

### A.1 Textual TIR

```
module      = header { item }
header      = "module" "@" path "tir" version "profile" profile [ "platform" platform ] [ "semantics" mode ]
              [ "producer" string string "languages" "[" ident { "," ident } "]" ] { modattr }
version     = digit [ digit ] "." digit [ digit ] "." digit
profile     = "app" | "system" | "driver" | "kernel"
platform    = "portable" | "tessera" | "macos" | "ios" | "windows" | "linux" | "android" | "web" | "wasi"
mode        = "defined" | "permissive"
modattr     = "attribute" string "=" string
            | ( "init" | "fini" ) "@" path "priority" int
            | "source" string                                  (driver profile: 64 lowercase hex digits for the source.json BLAKE3-256 hash)
path        = ident { "." ident }

item        = import | typedecl | global | func | entry | variants | trait | conformance | vtable

import      = "import" ( "ridl" "@" path "abi" string | "package" "@" path | "platform" string [ "weak" ] ) [ "{" { importent } "}" ]
importent   = ( "func" | "global" | "type" | "witness" | "descriptor" ) "@" path ":" type
            | ( "func" | "data" ) "@" path "symbol" string ":" type          (platform imports)

typedecl    = "type" "%" path [ tparams ] "=" kind layoutrule body "policy" ( "default" memmodel | "{" policy "}" )
tparams     = "<" "$" ident [ ":" bounds ] { "," "$" ident [ ":" bounds ] } ">"
bounds      = "%" path { "&" "%" path }
kind        = "struct" | "enum" | "explicit"
layoutrule  = "ridl" | "c" | "packed" "(" int ")" | "explicit" "(" "size" int "," "align" int ")"
body        = "{" [ member { "," member } ] "}"
member      = ident ":" type [ "@" int [ ":" int ":" int ] ] { annotation }      (byte offset, optionally bit offset and width)
            | ident [ "(" type { "," type } ")" ]                        (enum variant)
annotation  = "@" ident [ "(" [ literal { "," literal } ] ")" ]
memmodel    = "unmanaged" | "counted" | "affine" [ "copyable" ]
policy      = "copy:" copyp "," "destroy:" destroyp "," "relocate:" relocp ","
              "exclusivity:" ( "static" | "dynamic" | "none" ) "," "pairing:" ( "counted" | "none" ) ","
              "lifetime:" ( "eager" | "scoped" )
copyp       = "trivial" | ( "implicit" | "explicit" ) "(" "@" path ")" | "none"
destroyp    = "trivial" | "fn" "(" "@" path ")"
relocp      = "bitwise" | "fn" "(" "@" path ")"

global      = "global" linkage "@" path ":" type [ "<" space ">" ] [ "=" constant ] { gattr }
gattr       = "thread_local" | "constant" | "percpu" | "align" "(" int ")"

func        = "func" linkage "@" path [ tparams ] "(" [ param { "," param } ] ")" "->" type
              [ "fails" type ] { fattr } [ "{" { block } "}" ]
param       = "%" ident ":" [ "context" ] [ conv ] type
conv        = "owned" | "borrow" | "borrowmut"
linkage     = "private" | "module" | "package" | "exported"
fattr       = "engine" "(" ( "native" | "runtime" | "any" ) ")"
            | "semantics" "(" mode ")" | "abi" "(" ( "ridl" | "c" ) ")"
            | "target" "(" ( "cpu" | "gpu" ) [ "," ( "cpu" | "gpu" ) ] ")"
            | "async" | "coro" "(" "yield:" type "," "resume:" type ")"
            | "variant" "(" arch { "," ident } ")" | "structured" | "kernel.fp"
            | "fp" "(" ( "precise" | "fast" | "ieee_denormals" ) ")"
            | "inline" | "noinline" | "cold" | "noreturn" | "returns_twice" | "nounwind" | "inlinable"
            | "strategy" "(" ( "specialize" | "polymorphic" | "both" ) ")"
            | "instance" "(" type { "," type } ")"
            | "export.platform" string

entry       = "entry" entrykind "@" path "(" [ eparam { "," eparam } ] ")" [ "->" type ] { eattr }
              "{" { block } "}"
entrykind   = "compute" | "object" | "mesh" | "vertex" | "fragment" | "tile" | "raygen"
            | "intersection" | "anyhit" | "closesthit" | "miss" | "callable" | "node"
eparam      = ( "root" | "immediate" | "payload" | "attrs" | "data" | "record" | "input" )
              "%" ident ":" eparamtype
eparamtype  = "ptr" "<" space ">" type | type
eattr       = "threads" "(" ( int "," int "," int | "dynamic" ) ")"
            | "mesh" "(" "max_vertices:" int "," "max_primitives:" int "," "topology:"
                  ( "points" | "lines" | "triangles" ) ")"
            | "payload_size" "(" int ")" | "attributes" "(" type ")"
            | "caps" "(" ident { "," ident } ")" | "subgroup_size" "(" int ")"
            | "fp" "(" ( "precise" | "fast" | "ieee_denormals" ) ")" | "early_fragment_tests"
            | "node" "(" "launch:" ( "broadcast" | "coalescing" | "thread" )
                  "," "max_dispatch_grid:" "[" int "," int "," int "]"
                  "," "input_record_size:" int "," "outputs:" "[" [ node_output { "," node_output } ] "]" ")"
node_output = "{" "name:" string "," "kind:" ( "node" | "mesh" ) ","
                  "record_size:" int "," "max_records:" int "}"

variants    = "variants" "@" path "{" { varmember } "}"
varmember   = ( "portable" | arch [ "[" ident { "," ident } "]" ] ) ":" "@" path
arch        = "aarch64" | "x64" | "rv64" | "wasm"

trait       = "trait" "%" path [ tparams ] [ ":" bounds ] "{" { ( "type" ident [ ":" bounds ] | "method" ident ":" type ) } "}"
conformance = "conformance" type ":" "%" path "{" { ident "=" ( "@" path | type ) } "}"
vtable      = "vtable" "@" path "{" { int ":" "@" path } "}"

block       = label [ "(" [ param { "," param } ] ")" ]
              [ ":" "loop" "(" "merge" label "," "continue" label ")" ] ":" { inst }
label       = "bb" unsigned_integer | ident
inst        = [ "%" ident "=" ] opcode { modifier } [ operands ] { meta }
opcode      = ident { "." ident }
modifier    = "[" modword { ( "|" | " " | "," ) modword } "]"
modword     = ident [ "=" ident ] | ident int
operands    = operand { "," operand }
operand     = "%" ident | type | constant | "@" path [ "<" type { "," type } ">" ]
            | target | ident | "[" [ operand { "," operand } ] "]" | ident ":" operand
            | "(" [ operand { "," operand } ] ")" | "{" [ ident ":" literal { "," ident ":" literal } ] "}"
target      = label [ "(" [ "%" ident { "," "%" ident } ] ")" ]
meta        = "!" ident { "." ident } "(" [ literal { "," literal } ] ")"

type        = "i" unsigned_integer | "f16" | "bf16" | "f32" | "f64" | "f128"
            | "f8e4m3" | "f8e5m2" | "f6e2m3" | "f6e3m2" | "f4e2m1" | "e8m0"
            | "ptr" [ "<" space ">" ] | "*" type [ "<" space ">" ] | "unit" | "never"
            | "vec" "<" unsigned_integer "x" type ">" | "svec" "<" type ">"
            | "(" [ type { "," type } ] ")" | "[" unsigned_integer "x" type "]"
            | "%" path [ "<" type { "," type } ">" ] | "$" ident
            | ( "ref" | "uniq" | "weak" ) type
            | fntype | [ "borrow" ] "closure" fnsig
            | "coro" "<" type "," type "," type ">" | "frame" "@" path
            | "texture" "<" dim "," type "," access [ "," "ms" ] ">" | "sampler" | "accel"
            | "vft" "<" fntype ">" | "tensor" "<" unsigned_integer "x" type ">" | "rayquery" | "hitobject"
            | "coop" "<" ( "subgroup" | "threadgroup" ) "," unsigned_integer "x" unsigned_integer "," type "," ( "a" | "b" | "acc" ) ">"
            | "entry" "<" entrykind "," "root" type "," "immediate" type ">"
fntype      = "fn" fnsig
fnsig       = "(" [ ptype { "," ptype } ] ")" "->" type [ "fails" type ] [ "async" ] [ "abi" "(" ( "ridl" | "c" ) ")" ]
ptype       = [ "context" ] [ conv ] type
space       = "generic" | "device" | "constant" | "threadgroup" | "thread" | "object_payload"
            | "ray_payload" | "hit_attribute" | "tile" | "node_record"
dim         = "1d" | "2d" | "3d" | "cube" | "1darray" | "2darray" | "cubearray"
access      = "sample" | "read" | "write" | "readwrite"
int         = [ "+" | "-" ] unsigned_integer
unsigned_integer = digits | binary | octal | hex
digits      = decimal_digit { decimal_digit | "_" }
binary      = ( "0b" | "0B" ) binary_digit { binary_digit | "_" }
octal       = ( "0o" | "0O" ) octal_digit { octal_digit | "_" }
hex         = ( "0x" | "0X" ) hex_digit { hex_digit | "_" }

constant    = literal | type literal | "[" [ constant { "," constant } ] "]" | type "(" [ constant { "," constant } ] ")"
literal     = int | float | string | bytes | "true" | "false" | "null" | "inf" | "-inf" | "nan" "(" int ")"
bytes       = "b" string
```

The operand syntax of each instruction is as written in sections 12 to 17. The binary operand order of section 22.5 is the order of that syntax.

### A.2 Package manifest (example)

```json
{
  "build": 412,
  "dependencies": [
    { "abi": "a1b2c3d4-0000-4000-8000-000000000001", "module": "KiraUI" },
    { "abi": "6f1c2a9e-44d0-4b31-9a55-0c2f3e8d7a10", "module": "Ember" }
  ],
  "entitlements": ["tessera.gpu", "tessera.network.client"],
  "entry": { "module": "Notes.App", "symbol": "main" },
  "format": "tirpkg",
  "gpu": { "capabilities": [] },
  "id": "com.example.notes",
  "kind": "app",
  "license": "MIT",
  "modules": [
    { "blake3": "9f2c…", "name": "Notes.App", "path": "modules/Notes.App.tirb" },
    { "blake3": "41aa…", "name": "Notes.Sync", "path": "modules/Notes.Sync.tirb" }
  ],
  "name": "Notes",
  "product": { },
  "profile": "app",
  "resources": [ { "blake3": "77d0…", "kind": "asset", "path": "resources/Assets.car", "size": 1048576 } ],
  "tir": "26.11.1",
  "version": "3.2.0"
}
```

### A.3 `source.json` (driver profile)

```json
{
  "build": ["klf", "build", "drivers/xhci", "--profile", "release"],
  "environment": "hermetic-26.11.1",
  "repository": "https://example.org/xhci-driver",
  "revision": "c0ffee0123456789abcdef0123456789abcdef01",
  "toolchain": {
    "clang": "<build id>",
    "klf": "<build id>",
    "rustc": "<build id>",
    "tirc": "<build id>",
    "zig": "<build id>"
  }
}
```

Every toolchain component the build uses MUST be named; unused ones MAY be omitted.

### A.4 `provenance.json`

```json
{
  "analysis": "<blake3 of analysis.json>",
  "developerCertificate": "<fingerprint>",
  "options": ["-O3", "--lto=partitioned"],
  "package": "<package digest>",
  "subjects": [
    { "blake3": "…", "path": "images/Notes.img" },
    { "blake3": "…", "path": "tvisa/Notes.tvisa" }
  ],
  "target": { "cpu": "aarch64.base", "gpu": ["arm.gen5", "qcom.a7xx"] },
  "tirc": "<build id>",
  "train": "26.11.1"
}
```

### A.5 `bundle` (platform targets)

```json
{
  "identifier": "com.example.notes",
  "displayName": "Notes",
  "version": "3.2.0",
  "buildNumber": 412,
  "icons": "resources/Icons",
  "minimum": { "macos": "26.0", "ios": "26.0", "windows": "10.0.26100", "android": 33 },
  "apple": {
    "entitlements": { "com.apple.security.app-sandbox": true },
    "usageDescriptions": { "NSCameraUsageDescription": "Scan documents into notes." },
    "urlSchemes": ["notes"],
    "documentTypes": [ { "name": "Note", "extensions": ["note"], "role": "Editor" } ]
  },
  "android": {
    "applicationId": "com.example.notes",
    "targetSdk": 36,
    "permissions": ["android.permission.CAMERA"],
    "features": [ { "name": "android.hardware.vulkan.version", "required": true } ],
    "memtagMode": "async"
  },
  "windows": {
    "subsystem": "windows",
    "identity": { "name": "Example.Notes", "publisher": "CN=Example" },
    "capabilities": ["webcam"]
  },
  "web": { "template": "resources/web/index.html", "threads": true }
}
```

Every field above `apple` is required for every bundle; each platform's object is required when the package lists that platform and holds that platform's required fields. `minimum` may raise a platform's minimum of section 32.2, never lower it.

---

## 37. Appendix B: decision log

Format for `docs/decisions.md`: one entry per decision with id, date, decision, reason, sections affected.

| Id | Decision | Reason |
| --- | --- | --- |
| T-001 | TIR replaces KIR inside KLF; one IR runs from producer to backend | Two IRs would mean two verifiers, two optimizers and lossy translation |
| T-002 | TIR is open to producers and closed to consumers: only `tirc` and the device runtime produce executable code | Openness for languages, control of what runs |
| T-003 | Nothing is erased: ownership, generics, coroutines and GPU semantics are shipped; producers without the information mark it absent | Performance and analyzability come from that information |
| T-004 | Two semantic modes per function; memory never holds poison; no `undef` | LLVM inputs need undefined behavior for performance; Kira and KavaScript have none |
| T-005 | Fixed 64-bit little-endian machine model; neutral `tir64-unknown-tessera` target for C, C++, Rust and Zig | Bitcode locked to one architecture was why Apple Bitcode failed |
| T-006 | Three architectures with hardware control-flow integrity in their baselines; MTE used when present | Security is mandatory; MTE is absent from many supported CPUs |
| T-007 | Weekly frozen versions, a 26-train window (52 for drivers, current only for the kernel), no automatic upgrade; packages expire with their version | Tessera moves fast and breaks on purpose; the window bounds compiler complexity |
| T-008 | Devices receive native images for their CPU and GPU plus TVISA for unknown hardware and specialization | LLVM-level performance and support for new hardware at once |
| T-009 | Assembly only in variant bodies; any assembly requires variants for aarch64, x64 and rv64 | Every package runs on every architecture |
| T-010 | CPU and GPU code share one format and one layout; SPIR-V and GPU-specific layouts are gone; the GPU model is Metal-like and bindless | Single-source type checking; modern GPUs |
| T-011 | GPU compilers are Tessera-owned per family; drivers contain no shader compiler | One compiler per architecture instead of one per driver |
| T-012 | GPU code is precompiled server-side per family; on-device GPU compilation is robust | No shader compilation on devices in the common case, correctness in the rare one |
| T-013 | Native coroutines; async functions are poll-based futures crossing RIDL as `Any Future<R>` | Executor-agnostic and proven model |
| T-014 | Interpreter and JIT tiers are system components; JIT and finalizer run out of process; dynamic CPU TIR needs `tessera.tir.dynamic`, dynamic GPU TIR needs nothing | W^X without exceptions; Vulkan-on-Ember and runtime shaders |
| T-015 | Capability declarations are computed and checked against requested entitlements; `@Requires` extends the check to frameworks | Review without trusting the developer's claims |
| T-016 | CPU `math.*` is correctly rounded by default | Identical results on every tier and architecture |
| T-017 | BLAKE3-256 hashing and hybrid Ed25519 plus ML-DSA-65 signatures | Speed now, post-quantum safety for long-lived keys |
| T-018 | Drivers are GPLv3, written in Kira, Rust, Zig, C or C++, and source-reproducible | Open drivers that cannot fake their openness |
| T-019 | Rust reaches TIR through LLVM bitcode, not a new rustc backend | A new backend would be too heavy |
| T-020 | No geometry or tessellation stages; converters lower them to mesh and compute | Modern GPU pipelines only |
| T-021 | Frame pointers always; zero-initialization of unproven stack slots; zeroed heap allocations | Profiling and hardening by default |
| T-022 | Data races are undefined only in permissive functions; unordered semantics in defined functions | Defined functions keep their guarantee |
| T-023 | RIDL 1.1 adds `F16`, `BF16`, `Vec`, async functions, `GpuEntry`, GPU pointers and handles, `@Requires`, `@Inlinable`, and the rv64 register row | TIR needs each of them at module boundaries |
| T-024 | Wasm64 is accepted as sandboxed code; WasmGC and the component model are not in 1.0 | No tracing garbage collector |
| T-025 | TIR versions are written `YY.M.W`, W being the Thursday's ordinal in its month (1–5); the first is 26.11.1 | One version per Thursday train, every week of the year (TVISA.md decision TV-024) |
| T-026 | Pointer arithmetic on GPU pointers is not unmanaged | GPU code is confined to its process's GPU address space |
| T-027 | Cooperative matrices are GPU-only in 1.0; CPU matrix units are reached through target intrinsics | Portable CPU matrix semantics are not settled across SME2 and AMX |
| T-028 | TIR targets macOS, iOS, Windows, Linux, Android, the web and WASI as platform targets, with one semantics and a runtime bundled in each application | One format and one compiler for every platform; nothing depends on Tessera trains outside Tessera |
| T-029 | LibTessera is never a platform layer; platform modules call each platform's own interfaces through platform imports | LibTessera is Tessera's system interface; other platforms are reached directly, as their own applications are |
| T-030 | Portable modules are separate from platform modules, and per-platform RIDL implementations must be ABI-identical | Application code compiles once and links on every platform |
| T-031 | Platform toolchain targets are architecture-neutral but carry the platform's C data model | Platform headers compile unchanged while code stays architecture-neutral |
| T-032 | Platform hardening uses hint-space instructions and software tag checks | One image runs on every CPU of its baseline and is protected wherever the system enables protection |
| T-033 | WebAssembly keeps 64-bit pointers in 32-bit memories, with one range check per access | Every browser runs TIR with TIR's data layout; memory64 is used where available |
| T-034 | `tirc` implements every platform signature and bundle format itself | Apple, Windows and Android outputs are built and signed on any developer host |
| T-035 | `stack.alloc` records its address space; omitted space is generic on CPU and dual-target code, and `thread` on GPU-only code | Binary verification must distinguish per-thread values from entry-scoped threadgroup storage and preserve the address type |
| T-036 | Tensor ranks are 1, 2, 3, 4 or 8; rank 1 coordinates are integer scalars and the other ranks use matching fixed `i32` vectors | Fixed-vector lane counts and TVISA's value-type set cannot represent rank-5 through rank-7 coordinate values |
