# KLF Shader and KSL Specification v1.0

Date: 2026-10-04. Status: normative draft 1.0. Audience: every agent and engineer that implements any part of the KLF shader pipeline, a shader definition, or a shader backend.

Companion to [`KLF-RIDL.md`](KLF-RIDL.md). That document specifies the program pipeline (KIR, the VM, the native backends, RIDL). This document specifies the **shader pipeline**: KSIR (the KLF Shader IR), its validator and passes, its backends (MSL, SPIR-V, HLSL, GLSL, WGSL), the reflection contract with graphics hosts, the `ShaderLanguage` family that shader definitions extend, and the language profiles for KSL and an MSL frontend.

Contents

1. Status, scope and rules for implementers
2. Terminology
3. Goals, non-goals and related work
4. System overview
5. Shader definitions: the `ShaderLanguage` family
6. Kernel services for shader definitions
7. KSIR: module structure
8. KSIR types and layout
9. Address spaces, globals and the resource model
10. Entry points and the stage interface
11. Expressions
12. Statements and control flow
13. Functions and calls
14. Constants, overrides and constant evaluation
15. Execution semantics
16. Validation
17. Passes
18. Textual KSIR
19. Backends
20. Binding assignment
21. Reflection
22. Language profile: KSL
23. Language profile: MSL
24. Integration with hosted languages
25. Conformance and diagnostics
26. Implementation plan and crate layout
27. Appendix A: grammar of textual KSIR
28. Appendix B: correspondence with naga
29. Appendix C: built-in spelling per target
30. Appendix D: decision log

---

## 1. Status, scope and rules for implementers

### 1.1 Normative language

Section 1.1 of `KLF-RIDL.md` applies unchanged: MUST, SHOULD and MAY carry their RFC 2119 meanings; "Rationale" and "Example" paragraphs are informative; a table wins over prose; a grammar in Appendix A wins over an example; every numeric constant is normative.

### 1.2 Scope and stability

| Artifact | Role | Stability |
| --- | --- | --- |
| KSIR (in memory) | The kernel's shader IR. Every shader frontend produces it, every shader backend consumes it | Internal. Free to change between KLF releases |
| `.ksir` | Textual KSIR | Internal. Versioned per KLF release. Tests, dumps and humans only |
| `.ksirb` | Binary KSIR cache | Toolchains of the same KLF release only |
| KSLR reflection (`KSLR1`) and the `.resources` digest | What a graphics host binds against | Contract. Append-only record set (section 21) |
| Emitted MSL, SPIR-V, HLSL, GLSL, WGSL | Inputs to platform shader compilers and drivers | Valid for the target versions of section 19; textual form not stable |
| `ShaderLanguage` family | What a shader definition extends | Versioned with `klfkira`, additive-only after 1.0 |
| KSL profile (section 22) | Mapping of KSL onto KSIR | Normative |
| MSL profile (section 23) | Mapping of the accepted MSL subset onto KSIR | Normative for the subset it names |

### 1.3 Relation to `KLF-RIDL.md`

KSIR is not KIR. A shader never passes through KIR, the ownership passes, the generics engine, the bytecode, the VM, the native backends, the hybrid runtime or RIDL. The table states, for every part of `KLF-RIDL.md`, whether it applies to the shader pipeline.

| `KLF-RIDL.md` section | Applies to shaders |
| --- | --- |
| 1 Rules for implementers | Yes, all of 1.3, with rule 2 read as section 1.4 below |
| 3 Goals | Goals 1, 2, 3 and 4 apply. Goals 5 to 8 are about programs and do not |
| 4.2 The two times | Yes. A shader definition is compiled at toolchain-build time; KSL and MSL sources at user-compile time |
| 4.5 One definition per toolchain | Yes. A shader language is its own toolchain (section 4.3) |
| 5 `klf` tool and sysroot | Yes, with the shader pipeline of section 5.6 |
| 6 KLF Kira | Yes. Shader definitions are written in KLF Kira |
| 7 `Language` family | No. Replaced by `ShaderLanguage` (section 5) |
| 8.1 to 8.4, 8.6 to 8.9 Kernel services | Yes, unchanged |
| 8.5 Types, layouts and policies | No. Replaced by KSIR types (section 8) |
| 8.10 Compile-time evaluation | No for KIR evaluation. KSIR has its own constant evaluator (section 14.4) |
| 8.11 KIR builder | No. Replaced by the shader builder (section 6.3) |
| 9 Frontend pipeline | Yes, with `lower` returning a `ShaderModule` and step 4 of 9.1 replaced by section 4.4 |
| 10 to 18 KIR, ownership, VM, native, hybrid, runtime | No |
| 19 to 26 RIDL and `.klffi` | No. A shader's only contract is reflection (section 21) |
| 31 Conformance, diagnostics, security | Yes, extended by section 25 |
| 32 Crates and milestones | Yes, extended by section 26 |

### 1.4 Rules for implementing agents

1. Implement behavior exactly as written. Where this document is silent, choose the behavior that keeps every backend observably identical on the same KSIR module, and record the choice in `docs/decisions.md` in the format of `KLF-RIDL.md` Appendix B.
2. Every feature MUST work on every backend that can express it. A backend that cannot express a validated module MUST refuse it with a diagnostic naming the construct (section 19.1). It MUST NOT emit a substitute.
3. No placeholder, stub, hardcoded success or host-rendered substitute counts as an implementation. A section is implemented when its conformance tests (section 25) pass on every backend.
4. The kernel is written in Rust, with no `unsafe`, as the rest of the workspace. Rust files stay under 700 lines and `.klfd` files under 700 lines, generated files excepted.
5. No code from the pre-KLF Kira compiler repository (`kira`) is reused, per `KLF-RIDL.md` rule 1.3.6. Its KSL pipeline (`kira-ksl-*`, `kira-shader-*`, `kira-*-backend`) is the **oracle** for the KSL profile (section 25.3). Its behavior, comments and tests may be read; its source is not copied.
6. naga is the design reference for KSIR (Appendix B). It is not a dependency of any crate linked into the kernel. It MAY be a dependency of the test-only oracle crate (section 26.1).

---

## 2. Terminology

| Term | Meaning |
| --- | --- |
| Shader pipeline | Everything between shader source text and emitted target code, plus reflection |
| KSIR | KLF Shader IR: the arena-based module of sections 7 to 15 |
| Shader definition | A `.klfd` package whose single top declaration extends `ShaderLanguage` |
| Shader toolchain | Output of `klf build` for one shader definition: one driver (for example `ksl`) and its sysroot |
| Shader frontend | The compiled phases of a shader definition: source text to KSIR |
| Backend | Kernel code that turns a validated KSIR module into one target's output |
| Target | One of `msl`, `spirv`, `hlsl`, `glsl_430`, `wgsl` (section 19) |
| Stage | `vertex`, `fragment` or `compute`. `mesh`, `task` and the ray stages are reserved (section 10.1) |
| Entry point | A function a pipeline invokes for one stage |
| Shader | A named set of entry points sharing one resource interface and one reflection. A KSL `shader` declaration, or one MSL translation unit |
| Handle | A 32-bit index into one arena of a module or function |
| Expression | A KSIR value computation without side effects or control flow (section 11) |
| Statement | A KSIR operation with control flow or side effects (section 12) |
| Emit | The statement that marks where a range of expressions is evaluated (section 11.2) |
| Address space | Where a variable's storage lives: function, private, workgroup, uniform, storage, handle, immediate (section 9.1) |
| Global | A module-scope variable in some address space |
| Resource | A global in the uniform, storage or handle space. Hosts bind resources |
| Neutral binding | A resource's `(group, binding)` pair, independent of any target (section 9.3) |
| Target slot | Where a resource lands on one target: a Metal buffer index, an HLSL register, a GLSL binding (section 20) |
| Authored slot | A target slot written in source (MSL `[[buffer(3)]]`, KSL `@binding(2)`). It wins over assignment |
| Override | A scalar constant whose value may be supplied at pipeline creation (section 14.2). KSL `option`, MSL `[[function_constant]]`, SPIR-V specialization constant, WGSL `override` |
| Layout rule | The rule a struct's offsets were computed under: `std140`, `std430`, `scalar`, `msl` or `explicit` (section 8.4) |
| Reflection | The description a host binds against: entry points, interface, options, resources, target slots, layouts (section 21) |
| Uniformity | Whether a value or control-flow point is the same for every invocation of a quad, subgroup or workgroup (section 16.5) |
| Oracle | An existing implementation whose observable output a toolchain must match on a corpus: the pre-KLF `kira` KSL pipeline, Apple's `metal` compiler, `spirv-val`, naga |

---

## 3. Goals, non-goals and related work

### 3.1 Goals

1. **One shader IR, many frontends, many backends.** KSL, MSL and later frontends lower to KSIR; MSL, SPIR-V, HLSL, GLSL and WGSL are emitted from KSIR. Adding a frontend adds no backend work; adding a backend adds no frontend work. The pipelines `ksl -> KSIR -> MSL`, `ksl -> KSIR -> SPIR-V` and `msl -> KSIR -> SPIR-V` are all the same kernel code after the frontend.
2. **Decisions are made once.** Memory layout, binding slots, interface locations, sampling level, integer edge cases and overrides are decided in KSIR or in the kernel passes over it. A backend spells a decision; it never makes one. The reflection a host reads and the code a backend emits are computed from the same module and cannot disagree.
3. **Shader languages are definitions.** As in `KLF-RIDL.md` goal 2, changing KSL is an edit to KSL's `.klfd` files, never to Rust. The kernel owns KSIR, validation, passes, backends and reflection; the definition owns syntax and meaning.
4. **Validated before emitted.** Every module is validated (section 16) before any backend sees it. A backend may assume every rule of section 16 holds. A definition that produces an invalid module has a bug, reported as an internal error (section 25.5).
5. **Identical behavior across targets.** For a validated module, every backend produces code with the same observable results, within the floating-point latitude of section 15.3. Where the target languages disagree (integer division by zero, shifts, out-of-bounds access, sampling level), KSIR defines one behavior and backends polyfill.
6. **Readable and testable at the IR boundary.** `.ksir` round-trips, so a frontend is tested by its KSIR output and a backend by its input, separately.
7. **Host compatibility.** The reflection format and the generated file names that `ember`, `ui-foundation` and `kcoreanimation` read today keep working unchanged (sections 21 and 24).

### 3.2 Non-goals

- KSIR is not a general compute IR. No pointers into storage that outlive an expression, no pointer arithmetic, no recursion, no dynamic allocation, no function pointers.
- KSIR is not SSA. It is a statement tree over an expression DAG (section 7). Backends that need SSA (SPIR-V) build it.
- Optimization is not a goal of 1.0 beyond what correctness passes do. Platform shader compilers optimize.
- DXIL and Metal AIR bytecode are not emitted. HLSL and MSL source are, and the platform compilers (DXC, `metal`) produce bytecode.
- Mesh, task and ray-tracing stages, bindless binding arrays, cooperative matrices and subgroup operations are reserved, not specified (section 10.1, 26.2).
- The MSL frontend does not accept all of MSL. It accepts the subset of section 23 and refuses the rest with diagnostics.
- KSIR, `.ksir` and `.ksirb` are not interchange formats between KLF releases.

### 3.3 Related work (informative)

| System | What KLF shares | Where KLF differs |
| --- | --- | --- |
| naga (wgpu) | KSIR's core shape: arenas, expressions separated from statements, `Emit` ranges, structured control flow, explicit pointers and address spaces, explicit layouts, overrides, a validator producing per-expression analysis (Appendix B) | Spans on every node, a round-trippable text form, reflection as a first-class output, target slots carried in the IR, defined integer edge cases, no abstract types or value pointers in the module, frontends written as KLF definitions |
| Tint (Dawn) | WGSL and SPIR-V in, several targets out, through one IR | Tint's IR is SSA-like and WGSL-centric; KSIR is structured and frontend-neutral |
| SPIRV-Cross | SPIR-V to GLSL, HLSL, MSL, with reflection | KLF never round-trips through SPIR-V; each target is emitted from KSIR directly |
| Slang | One shading language, many targets, its own IR, reflection API | Slang is one language; KLF hosts many and keeps each in a definition |
| glslang, DXC | Mature frontends to SPIR-V | They are the platform compilers KLF's GLSL and HLSL output feeds, and possible oracles |
| MLIR SPIR-V dialect | SPIR-V modeled as an IR | KSIR is higher level and not tied to SPIR-V's form |
| Pre-KLF `kira` KSL pipeline | KSL itself, its binding rules, its reflection format, its corpus | Its backends walk the checked KSL tree directly ("Statements and expressions are not restated", `kira-shader-ir/src/lib.rs`). That works for one frontend and is exactly what a second frontend cannot use. KSIR restates the body |

---

## 4. System overview

### 4.1 Two pipelines in one kernel

```
                      program pipeline (KLF-RIDL.md)
  .kira .c .cpp ... -> definition -> KIR -> passes -> VM / LLVM / Cranelift / hybrid

                      shader pipeline (this document)
  .ksl   -> ksl  in --+                                        +--> ksl  out -> .ksl
  .metal -> msl  in --+                                        +--> msl  out -> .metal
  .hlsl  -> hlsl in --+-> KSIR -> validate -> passes(target) --+--> hlsl out -> .hlsl
  .glsl  -> glsl in --+                                        +--> glsl out -> .glsl
  .wgsl  -> wgsl in --+                                        +--> wgsl out -> .wgsl
                                                               +--> SPIR-V (kernel) -> .spv
                                                               \--> reflection -> KSLR1, .resources
```

A shader language is one definition with two sides. Its **in** side (`units`, `parse`, `elaborate`, `lower`) turns source into KSIR; its **out** side (`emit`, section 5.8) turns a KSIR unit into source. Any in reaches any out through KSIR, so converting a shader between two shader languages is one build. SPIR-V is binary and no language's source, so the kernel writes it.

The two pipelines share the KLF Kira frontend, the definition build machinery (`klf build`), the `Session` services of `KLF-RIDL.md` 8.1 to 8.4 and 8.6 to 8.9, the diagnostic renderer, the query cache and the driver shell. They share no IR and no pass.

### 4.2 Why not KIR

KIR is an ownership SSA for programs that run on a CPU, VM or both. A shader needs none of value kinds, policy records, borrow scopes, unwinding, engines or RIDL, and needs several things KIR does not have: address spaces as part of pointer types, image and sampler types, stage interfaces with built-ins and locations, binding slots, uniformity, structured-only control flow, and explicit per-address-space layout rules. Encoding those in KIR would grow every KIR pass to ignore them. A sibling IR keeps both small.

### 4.3 Shader toolchains

A shader language is a definition and produces its own toolchain, as `KLF-RIDL.md` 4.5 requires.

| Definition | Driver | In | Out | Section |
| --- | --- | --- | --- | --- |
| KSL | `ksl` | `ksl` | `ksl` | 22 |
| MSL | `kmsl` | `metal` | `msl` | 23 |
| HLSL | `khlsl` | `hlsl` (S12) | `hlsl` | 19.5 |
| GLSL | `kglsl` | `glsl` (S12) | `glsl_430` | 19.6 |
| WGSL | `kwgsl` | `wgsl` (S12) | `wgsl` | 19.7 |

MSL, HLSL, GLSL and WGSL are shader languages like any other. Their definitions live in `klf-kira` under `languages/<lang>/`, beside KSL's, and a `knvm` builds each as that language's own toolchain. KLF does not decide which of them exist: `klf build <package> --language <package>` links the others into a shader driver, and whoever builds it names them (section 5.9).

KSL is Kira's shader language and lives beside them in `klf-kira` (`languages/ksl/`). Every one of these five names the other four, so each driver's toolchain writes every target its language declares rather than only the one it was named for. A driver built with no `--language` holds its own definition alone and reports every other target as unsupported, which is true of that driver.

A program that uses shaders is built by two toolchains: its own (for example `kira`) and the shader toolchain. They meet through files and reflection (section 24), never through a linked call, exactly as two program toolchains meet only through RIDL.

### 4.4 Driver loop

The shader driver loop replaces `KLF-RIDL.md` 9.1 step 4:

1. Parse the command line, resolve inputs, create the `Session`.
2. `units(session, inputs)`.
3. For each unit, in parallel: query `parse`, `elaborate`, `lower`. `lower` returns a `ShaderModule`.
4. For each module: validate (section 16); on failure, report `KLF-SHD-9xxx` internal errors against the definition and stop.
5. Run the mandatory passes (section 17.1) for the requested target, then validate again in debug builds of the driver.
6. Write the unit for the requested target: the out of the language that names the target (section 5.8), or the kernel's SPIR-V writer. On refusal, report its diagnostic.
7. Compute reflection (section 21) from the same module and the same binding assignment.
8. Write outputs (section 24.2).

Steps 5 to 7 run once per target. Steps 1 to 4 run once per unit whatever the number of targets.

### 4.5 File types

| Extension | Content | Producer | Consumer |
| --- | --- | --- | --- |
| `.ksl` | KSL source | Humans, agents | `ksl` driver |
| `.metal` (input) | MSL source in the accepted subset | Humans, agents | `kmsl` driver |
| `.ksir` | Textual KSIR | Shader drivers (`--emit ksir`), humans | Tests, `klf ksir verify`, humans |
| `.ksirb` | Binary KSIR cache | Shader drivers | Shader drivers of the same KLF release |
| `<Shader>.metal` | Emitted MSL, every stage in one module | `--target msl` | Metal (`MTLDevice.makeLibrary`, `metal`) |
| `<Shader>.<stage>.spv` | Emitted SPIR-V words, one module per stage | `--target spirv` | Vulkan |
| `<Shader>.<stage>.hlsl` | Emitted HLSL, one per stage | `--target hlsl` | DXC, FXC |
| `<Shader>.<stage>.glsl` | Emitted GLSL 4.30, one per stage | `--target glsl_430` | OpenGL |
| `<Shader>.<stage>.wgsl` | Emitted WGSL, one per stage | `--target wgsl` | WebGPU |
| `<Shader>.ksl` | Emitted KSL, every stage in one shader | `--target ksl` | `ksl`, humans |
| `<Shader>.resources` | Compact resource digest (section 21.3) | Every shader build | Graphics hosts |
| `<Shader>.kslr` | `KSLR1` reflection document, one per target | `--emit reflection` | Graphics hosts, tools |

`<stage>` is `vert`, `frag` or `comp`. These are the names the existing hosts read (section 24.2).

### 4.6 What is shared and what is specialized

Shared by all shader toolchains of one KLF release: KSIR, the validator, the passes, the SPIR-V writer, every linked definition's in and out, reflection, binding assignment, the text format.

Specialized per shader toolchain: which in the driver runs by default (its own), its diagnostics, its driver flags, its default layout rules (section 5.3), its built-in function library (expressed as lowering, not as KSIR extensions).

The kernel keeps every per-target decision: passes, stage selection, binding assignment, capabilities, layouts and reflection. An out decides only spelling.

Contract with the outside world: reflection (section 21) and the emitted target code. Nothing else.

---

## 5. Shader definitions: the `ShaderLanguage` family

### 5.1 Why a second family

`Language` (`KLF-RIDL.md` 7.2) requires members that mean nothing for a shader language (`memory`, `exclusivity`, `convention`, `stdRoot`, `langItems`) and a `lower` that returns a `KirModule`. Reusing it would force shader definitions to declare fictitious values and the kernel to special-case them. `ShaderLanguage` is a sibling family with the same phase structure.

`KLF-RIDL.md` 7.5 rule 1 is amended: a package MUST contain exactly one declaration extending `Language` **or** `ShaderLanguage`, never both (`KLF-DEF-0001` otherwise).

### 5.2 The family (provided by the `KLF` module)

```
construct ShaderLanguage<Tree: Cacheable, Program: Cacheable> {
    @Required let name: String
    @Required let driver: String
    @Required let version: String
    @Required let extensions: [String]
    @Required let layouts: LayoutDefaults
    @Required let keys: [DiagnosticKey]

    let libraryRoot: String { return "" }
    let capabilities: [ShaderCapability] { return [] }
    let targets: [ShaderTarget] { return [.Msl, .Spirv, .Hlsl, .Glsl430, .Wgsl] }
    let flags: [DriverFlag] { return [] }
    let diagnostics: [DiagnosticSpec] { return [] }

    function units(session: Session, inputs: [SourceFile]) -> [Unit]
    function parse(session: Session, unit: Unit) -> Tree
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program
    function lower(session: Session, unit: Unit, program: Program) -> ShaderModule
    function emit(unit: ShaderUnit) -> Bool

    function compatDriver(arguments: [String]) -> Optional<DriverRequest> { return .None }
    function hover(session: Session, at: Position) -> Optional<String> { return .None }
    function complete(session: Session, at: Position) -> [Completion] { return [] }
    function definition(session: Session, at: Position) -> Optional<Span> { return .None }
}
```

Supporting declarations:

```
enum ShaderTarget { Msl Spirv Hlsl Glsl430 Wgsl Ksl }

enum LayoutRule { Std140 Std430 Scalar Msl Explicit }

struct LayoutDefaults {
    let uniform: LayoutRule
    let storage: LayoutRule
    let immediate: LayoutRule
}

struct DiagnosticKey {
    let key: String
    let code: Int
}

distinct ShaderUnit = Int   // one unit an out writes (section 5.8)

enum ShaderCapability {
    Float16 Float64 Int64 Int64Atomics ImageAtomics CubeArray Multisampled
    SampleShading ClipDistances DualSourceBlending StorageImageReadWrite
}
```

`DriverFlag`, `DiagnosticSpec`, `Unit`, `Completion` and `DriverRequest` are the declarations of `KLF-RIDL.md` 7.2 and 5.5, unchanged.

### 5.3 Meaning of the members

| Member | Meaning |
| --- | --- |
| `name` | Lowercase language identifier. Recorded as the `lang` of every module (section 7.1) and in reflection |
| `driver` | File name of the generated driver |
| `extensions` | Extensions the language claims. A directory input stands for every file under it with one of these |
| `layouts` | Layout rule the shader builder applies to a struct reached from each address space when the definition does not compute offsets itself (section 8.4). `Explicit` means the definition always supplies offsets and `structType` is refused for that space. KSL: `Explicit` for all three, because KSL packing is its own rule (section 22.5). MSL: `Msl` for all three |
| `libraryRoot` | Directory of shared shader modules shipped with the toolchain (for example a KSL module of common lighting functions), relative to the package. Empty means none. Library modules are source, resolved by the definition's import rules; nothing is precompiled |
| `targets` | Targets the driver accepts. A target outside this list is `KLF-DRV-0030` |
| `flags` | Extra driver flags, as `KLF-RIDL.md` 7.3 |
| `diagnostics` | The definition's diagnostic registry |
| `keys` | The definition's code for each kernel key of section 16.4. All nine keys MUST be mapped; a package that leaves one out does not build. A validation error with no key is `KLF-SHD-9002`, and a key the list does not map is `KLF-SHD-9003` |
| `capabilities` | The capabilities of section 16.6 the language can spell. Every module the definition lowers declares them; the driver narrows each to what the module uses |
| `units` | Groups input files into units. KSL and MSL return one unit per input file; imports are resolved inside `parse` |
| `parse`, `elaborate`, `lower` | The three phases, each an incremental query keyed by unit (`KLF-RIDL.md` 8.9). `lower` builds a `ShaderModule` through the shader builder (section 6.3). A definition with `extensions` declares all four of `units`, `parse`, `elaborate` and `lower`; one without declares none of them and has no in |
| `emit` | The out (section 5.8). Optional; a definition declares an in, an out, or both |
| `compatDriver`, `hover`, `complete`, `definition` | As `KLF-RIDL.md` 7.3 |

### 5.4 Example skeleton (informative)

```
import KLF

construct Ksl() extends ShaderLanguage<KslTree, KslProgram> {
    let name = "ksl"
    let driver = "ksl"
    let version = "1.0.0"
    let extensions = ["ksl"]
    // KSL packing differs from std140 and std430 (section 22.5), so the
    // definition computes every offset and declares structs explicitly.
    let layouts = LayoutDefaults { uniform = .Explicit, storage = .Explicit, immediate = .Explicit }

    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        return inputs.map({ file in return Unit.single(file) })
    }
    function parse(session: Session, unit: Unit) -> KslTree {
        return KslParser.run(session, unit)
    }
    function elaborate(session: Session, unit: Unit, tree: KslTree) -> KslProgram {
        return KslCheck.run(session, unit, tree)
    }
    function lower(session: Session, unit: Unit, program: KslProgram) -> ShaderModule {
        return KslLower.run(session, unit, program)
    }
}
```

### 5.5 Rules

1. A shader definition MUST produce a module that passes validation (section 16) for every well-formed input. A validation failure on a module lowered from a well-formed program is a definition bug, reported as `KLF-SHD-9xxx` naming the definition.
2. A shader definition MUST NOT decide target slots for resources that carry none in source. Assignment is the kernel's (section 20). It MAY record authored slots (section 9.4).
3. A shader definition MUST give every expression, statement, type, global, function and entry point a span (section 7.4). Generated nodes carry the span of the source construct they were generated for.
4. A shader definition MUST NOT perform I/O except through `Session`, and MUST be deterministic (`KLF-RIDL.md` 6.6).
5. A shader definition MUST NOT be written in the language it defines. This holds by construction.

### 5.6 `klf build` for a shader definition

`klf build <defdir>` detects which family the package's top declaration extends.

1. Load and check the definition with the KLF Kira frontend (unchanged).
2. Lower the definition to KIR, optimize, emit a host object (unchanged).
3. Link with `libklf_kernel.a`. The driver's `main` is the shader driver loop of section 4.4.
4. Copy the files under `libraryRoot` into the sysroot. There is no std to compile: shaders link nothing.
5. Assemble the sysroot and write `definition.lock`.

Sysroot layout:

```
<out>/
  bin/<driver>                   shader driver (host)
  share/<lang>/library/...       copy of libraryRoot, when not empty
  definition.lock
```

`definition.lock` records the KLF version, `klfkira` version, SHA-256 of every `.klfd` file and every library file, and the list of targets.

A shader definition has no VM path. The VM runs KIR, and a shader never becomes KIR (section 4.2); the native driver is the toolchain, and a host language reaches it by running it (section 24.1). The driver binary also holds the other shader languages it was built with (section 5.9).

### 5.7 Driver command line

Every shader driver accepts:

| Command | Effect |
| --- | --- |
| `<driver> build [paths] --target <t>[,<t>...] [--out <dir>]` | Compile every shader under `paths` for the listed targets (default: all of `targets`) and write the files of section 4.5 under `<dir>` (default `generated/shaders/`) |
| `<driver> build <file> --target <t> --emit source or reflection or ksir or digest` | Print one output for one file and one target to standard output |
| `<driver> check [paths]` | Frontend and validation only |
| `<driver> lsp` | Language server, specified by [`KLF-EDITOR.md`](KLF-EDITOR.md) (section 14 for shader behavior) |

Shader-pipeline flags common to every shader driver:

| Flag | Values | Default | Section |
| --- | --- | --- | --- |
| `--overrides` | `bake`, `keep` | `bake` | 17.3 |
| `--bounds` | `unchecked`, `restrict`, `read-zero-skip-write` | `unchecked` | 15.5 |
| `--integer-division` | `defined`, `native` | `defined` | 15.2 |
| `--msl-version` | `2.4`, `3.0`, `3.1`, `3.2` | `2.4` | 19.3 |
| `--spirv-version` | `1.0` to `1.6` | `1.3` | 19.4 |
| `--hlsl-model` | `5_0`, `5_1`, `6_0` to `6_8` | `6_0` | 19.5 |
| `--diagnostics` | `text`, `json` | `text` | `KLF-RIDL.md` 31.4 |

A shader driver writes outputs only when every requested target succeeded for every input. Outputs are bit-identical for identical inputs, flags and KLF release (`KLF-RIDL.md` 5.7).

### 5.8 Outs

A definition that declares `emit` is the out for the target its `name` names (`ksl`, `msl`, `hlsl`, `glsl` for `glsl_430`, `wgsl`). The driver calls `emit` once per unit (section 19.2 gives each target's units) with a `ShaderUnit`: the unit after the passes for that target and its validation, its `ModuleInfo`, the binding assignment for that target, and the target options. `emit` reads the unit through the reader services and writes the output through the writer services (section 6.7), and returns `true` when it wrote a whole output. It returns `false` after refusing at least one construct with `klf.shader.out.refuse`, and then nothing it wrote is used.

An out follows the backend contract of section 19.1. It reads nothing but the unit: no source, no tree, no other unit. It has no `Session`, so it cannot report through the definition's diagnostic registry; its refusals are the kernel key `shader.target-unsupported`, which the driver of the input language reports with that language's code.

### 5.9 The linked toolchain

`klf build <package>` of a shader definition produces one native toolchain binary holding that definition. Each `--language <package>` adds another, so the binary holds every out the build was told about and each compiled once. A `--language` naming the package being built is `KLF-DRV-0031`: the registry indexes definitions, and two of one name would answer for each other.

Asked for no `--language`, a shader driver holds one definition and reports every other target as unsupported. That is a statement about the driver, not a failure, and it is why `knvm`'s `toolchain.klfd` lists the outs for each shader language and `klf test` reads the `shaderLanguages` of each corpus manifest: a case naming `--target hlsl` should be answered by the HLSL out. The binary is linked from one object per definition, in which only three symbols are global, renamed by the definition's index `N`: `klffe_metadata_N`, `klffe_phase_N` and `klffe_init_N` (the definition's global prologue). A registry object joins them: `main` runs every `klffe_init_N`, `klffe_language_count` and `klffe_language_metadata(N)` list them, `klffe_select(N)` chooses the definition later calls reach, and `klffe_metadata` and `klffe_phase` answer for the chosen one. Definition 0 is the definition being built.

At startup the binary selects the definition whose `driver` equals the name it was run under, else definition 0. The in that runs is the selected definition's; each target reaches its out by selecting the out's definition around the `emit` call.

`klf dist` installs no shader language: a distribution is `klf`, and a user who never writes a shader should not carry the sources of four languages they will never read. A shader toolchain is installed by the `knvm` of the repository that holds it, beside that repository's other languages.

The objects of the linked definitions are cached under the KLF cache root, keyed by the KLF release, the target, the definition's index and the SHA-256 of its sources, so a build that links five definitions compiles only what changed. A definition that is not a shader language is built the same way, as a binary of one, and `--language` has nothing to link: `KLF-DRV-0031`.

---

## 6. Kernel services for shader definitions

### 6.1 Reused services

Shader definitions reach, unchanged: sources and spans (8.1), diagnostics (8.2), interning and arenas (8.3), scopes and name lookup (8.4), traits and the constraint solver (8.6, used by frontends with overloading such as MSL), speculation (8.7), instantiation (8.8, used by the MSL frontend for its template subset), queries and caching (8.9), `session.flag(name)`, and the lexer and Pratt helpers of 8.11.

### 6.2 Services that do not exist for shader definitions

`KirBuilder`, the KIR type services of 8.5, `session.evaluate` over KIR (8.10), file modes, memory models and conventions. Calling a KIR service from a shader definition is `KLF-DEF-0030` at toolchain-build time.

### 6.3 The shader builder

```
distinct ShaderModule = Int
distinct TypeHandle = Int
distinct ConstantHandle = Int
distinct OverrideHandle = Int
distinct GlobalHandle = Int
distinct FunctionHandle = Int
distinct ExprHandle = Int
distinct LocalHandle = Int
distinct BlockHandle = Int

extend Session {
    function shaderModule(name: String, origin: Span) -> ShaderModule
}

extend ShaderModule {
    // Types (section 8). Every call interns: equal types return the same handle.
    function scalarType(kind: ScalarKind, width: Int) -> TypeHandle
    function vectorType(size: Int, kind: ScalarKind, width: Int) -> TypeHandle
    function matrixType(columns: Int, rows: Int, width: Int) -> TypeHandle
    function atomicType(kind: ScalarKind, width: Int) -> TypeHandle
    function arrayType(base: TypeHandle, size: ArraySize, space: AddressSpaceKind) -> TypeHandle
    function structType(name: String, members: [MemberSpec], space: AddressSpaceKind, span: Span) -> TypeHandle
    function structTypeExplicit(name: String, members: [PlacedMember], size: Int, align: Int, span: Span) -> TypeHandle
    function imageType(dim: ImageDim, arrayed: Bool, class: ImageClass) -> TypeHandle
    function samplerType(comparison: Bool) -> TypeHandle
    function pointerType(base: TypeHandle, space: AddressSpace) -> TypeHandle
    function sizeOf(ty: TypeHandle) -> Int
    function alignOf(ty: TypeHandle) -> Int

    // Module scope (sections 9, 13, 14).
    function constant(name: String, ty: TypeHandle, init: ConstExpr, span: Span) -> ConstantHandle
    function override(name: String, id: Optional<Int>, ty: TypeHandle, init: Optional<ConstExpr>, span: Span) -> OverrideHandle
    function global(name: String, space: AddressSpace, ty: TypeHandle, binding: Optional<ResourceBinding>, span: Span) -> GlobalHandle
    function authoredSlot(global: GlobalHandle, slot: TargetSlot, span: Span)
    function addFunction(name: String, params: [ParamSpec], result: Optional<ResultSpec>, span: Span) -> FunctionBuilder
    function entryPoint(name: String, stage: Stage, params: [ParamSpec], result: Optional<ResultSpec>, span: Span) -> FunctionBuilder
    function shader(name: String, entries: [FunctionHandle], groups: [GroupSpec], span: Span)
    function finish() -> ShaderModule
}
```

`FunctionBuilder` builds one function body. It owns the function's expression arena and a stack of open blocks:

```
extend FunctionBuilder {
    function local(name: String, ty: TypeHandle, init: Optional<ExprHandle>, span: Span) -> LocalHandle

    // Expressions (section 11). Each returns its handle.
    function literal(value: Literal, span: Span) -> ExprHandle
    function argument(index: Int, span: Span) -> ExprHandle
    function globalRef(global: GlobalHandle, span: Span) -> ExprHandle
    function localRef(local: LocalHandle, span: Span) -> ExprHandle
    function constantRef(constant: ConstantHandle, span: Span) -> ExprHandle
    function overrideRef(override: OverrideHandle, span: Span) -> ExprHandle
    function zero(ty: TypeHandle, span: Span) -> ExprHandle
    function compose(ty: TypeHandle, parts: [ExprHandle], span: Span) -> ExprHandle
    function access(base: ExprHandle, index: ExprHandle, span: Span) -> ExprHandle
    function accessIndex(base: ExprHandle, index: Int, span: Span) -> ExprHandle
    function splat(size: Int, value: ExprHandle, span: Span) -> ExprHandle
    function swizzle(vector: ExprHandle, pattern: [Int], span: Span) -> ExprHandle
    function load(pointer: ExprHandle, span: Span) -> ExprHandle
    function unary(op: UnaryOp, value: ExprHandle, span: Span) -> ExprHandle
    function binary(op: BinaryOp, left: ExprHandle, right: ExprHandle, span: Span) -> ExprHandle
    function select(condition: ExprHandle, accept: ExprHandle, reject: ExprHandle, span: Span) -> ExprHandle
    function convert(value: ExprHandle, kind: ScalarKind, width: Optional<Int>, span: Span) -> ExprHandle
    function math(fun: MathFunction, args: [ExprHandle], span: Span) -> ExprHandle
    function relational(fun: RelationalFunction, value: ExprHandle, span: Span) -> ExprHandle
    function derivative(axis: DerivativeAxis, control: DerivativeControl, value: ExprHandle, span: Span) -> ExprHandle
    function imageSample(sample: SampleSpec, span: Span) -> ExprHandle
    function imageLoad(load: ImageLoadSpec, span: Span) -> ExprHandle
    function imageQuery(image: ExprHandle, query: ImageQuery, span: Span) -> ExprHandle
    function arrayLength(pointer: ExprHandle, span: Span) -> ExprHandle

    // Statements (section 12). Each appends to the innermost open block.
    function store(pointer: ExprHandle, value: ExprHandle, span: Span)
    function imageStore(store: ImageStoreSpec, span: Span)
    function call(callee: FunctionHandle, args: [ExprHandle], span: Span) -> Optional<ExprHandle>
    function atomic(pointer: ExprHandle, fun: AtomicFunction, value: ExprHandle, wantResult: Bool, span: Span) -> Optional<ExprHandle>
    function barrier(kind: BarrierKind, flags: BarrierFlags, span: Span)
    function beginIf(condition: ExprHandle, span: Span)
    function beginElse()
    function endIf()
    function beginLoop(span: Span)
    function beginContinuing()
    function endLoop(breakIf: Optional<ExprHandle>)
    function beginSwitch(selector: ExprHandle, span: Span)
    function beginCase(value: SwitchValue, fallThrough: Bool, span: Span)
    function endCase()
    function endSwitch()
    function breakLoop(span: Span)
    function continueLoop(span: Span)
    function returnValue(value: Optional<ExprHandle>, span: Span)
    function kill(span: Span)

    function finish() -> FunctionHandle
}
```

The record types `MemberSpec`, `PlacedMember`, `ParamSpec`, `ResultSpec`, `GroupSpec`, `SampleSpec`, `ImageLoadSpec`, `ImageStoreSpec` and the enums mirror sections 8 to 12 field for field.

### 6.4 Emit management

Definitions never write `Emit` statements. The builder maintains them, as naga's frontends do with their emitter:

1. The builder holds an open emit range per function.
2. An expression whose evaluation time is "when emitted" (section 11.2) extends the open range when created.
3. Before appending any statement, opening or closing a block, or switching between `accept` and `reject`, the builder closes the open range: if it is non-empty it appends `Emit(range)` to the current block. Then it starts a new empty range.
4. An expression created inside a block is in scope only inside that block (section 11.3). A definition that needs a value after the block ends stores it in a local.

The builder checks operand types eagerly and reports `KLF-SHD-9001` with the definition's name on misuse (wrong arity, operand type mismatch, handle from another function, expression used outside its scope). These are definition bugs.

### 6.5 Layout service

`structType` computes member offsets, member sizes and alignments, struct size and struct alignment under the rule `layouts` gives for `space` (section 8.4). A definition whose language fixes layout by its own rule (KSL packing, section 22.5; a future frontend reading `layout(offset = N)`) calls `structTypeExplicit`:

```
struct PlacedMember {
    let name: String
    let ty: TypeHandle
    let binding: Optional<BindingSpec>
    let offset: Int
    let size: Int
    let align: Int
}
```

The struct is then rule `explicit` and the validator checks it by SV3 only.

### 6.6 Service ABI

Services follow `KLF-RIDL.md` 8.12. Entry points are named `klf.shader.<area>.<name>` (for example `klf.shader.expr.binary`, `klf.shader.stmt.store`, `klf.shader.type.struct`) and listed in `kernel/services.manifest`. Builder calls are coarse enough that no service runs per character or token. Specs with arrays cross the boundary as bulk buffers, as `KLF-RIDL.md` 5.5 describes for strings.

The calling convention:

1. Handles are arena indices; -1 means none.
2. Types, bindings, address spaces and slots cross as textual KSIR fragments (section 18), parsed by the kernel. Enumerated selectors (operators, math functions, stages, group classes) cross as their KSIR spelling.
3. A list operand (the parts of a composition, the arguments of a call or a math function, the entries of a shader, the operands of an image operation) is pushed one word at a time with `klf.shader.push` before the service that consumes it.
4. `klf.shader.span(module, file, start, end)` sets the span the next declarations and statements carry; file -1 is the phase's own file.
5. An expression service takes the module and a function builder; builder -1 builds a global expression.
6. The first misuse is kept and later calls are ignored; `klf.shader.module.finish` then fails the phase with `KLF-SHD-9001` naming the service.

### 6.7 Reader and writer services

An out reads its `ShaderUnit` through four services that address every record of the unit by table, owner, index and field, and writes through three. Each answers a word; a text answer is a text id, read with the builtin `shaderText(unit, id) -> String` (over `klf.shader.read.text_len` and `klf.shader.read.text_copy`). A field the record lacks, and an absent operand, answers -1; the text id -1 reads as the empty string.

| Service | Answer |
| --- | --- |
| `klf.shader.read.count(unit, table, owner)` | How many records `table` holds under `owner` |
| `klf.shader.read.kind(unit, table, owner, index)` | The record's kind, as a text id |
| `klf.shader.read.int(unit, table, owner, index, field, item)` | An integer field. `item` indexes a list field and is 0 otherwise; item -1 answers the list's length |
| `klf.shader.read.text(unit, table, owner, index, field, item)` | A text field, as a text id, with `item` as for `int` |
| `klf.shader.out.write(unit, text)` | Appends `text` to the output |
| `klf.shader.out.refuse(unit, message, function, expression)` | Records a refusal at the span of `expression` in `function` (-1 for the function, -1, -1 for the unit) |
| `klf.shader.out.refuse_at(unit, message, table, owner, index)` | Records a refusal at the span of a record: of `types`, `constants`, `overrides`, `globals` or `functions` (owner -1), `locals` (owner the function), `statements` (owner the block) or `expressions` (owner the function, -1 for a global expression) |

A table, owner, index or field name the unit does not have, or an item past a list's end, is a misuse: the service answers -1 and the driver reports the first misuse as an internal error naming the definition (`KLF-SHD-9001`), whatever `emit` returns.

A **function number** counts the unit's functions in arena order, then its entry points: entry `e` is function `F + e`. A **block number** is unit-wide, in pre-order; block 0 of function `f` is `read.int(unit, "functions", -1, f, "body", 0)`. Kinds and text values use the KSIR spellings of section 18 (`f32`, `storage`, `binary`, `add`, `image_sample`, `exact`). A literal's `literal` text is its value with no suffix; its `scalar` gives the type.

| Table, owner | Kind | Integer fields | Text fields |
| --- | --- | --- | --- |
| `unit`, -1 (one record) | `graphics`, `compute`, or `library` for a module with no shader | `entries` (list of function numbers), `options` (list of overrides, in the shader's order), `debug` | `name`, `shader`, `target`, `lang`, `stage` (when the unit has one entry), `capabilities` (list), `files` (list: the source files spans name) |
| `groups`, -1 | the group's class (section 9.3) | `globals` (list) | `name` |
| `types`, -1 | `scalar`, `vector`, `matrix`, `atomic`, `pointer`, `array`, `struct`, `image`, `sampler` | `lanes`, `columns`, `rows`, `base`, `length` (absent for a runtime array), `length_override`, `stride`, `size`, `align`, `arrayed`, `multisampled`, `comparison`, `host_shared`, and for a scalar, vector, matrix or atomic its extent under each rule: `size_std140`, `align_std140`, `size_std430`, `align_std430`, `size_msl`, `align_msl` | `name`, `scalar`, `space`, `access` (words joined by spaces: `load store`), `dimension`, `class`, `format`, `rule` |
| `members`, type | `member` | `type`, `offset`, `size`, `align`, `location`, `blend_src` | `name`, `builtin`, `interpolation`, `sampling` |
| `constants`, -1 | `constant` | `type`, `init` (a global expression), `value` | `name` |
| `overrides`, -1 | `override` | `type`, `id`, `init`, `value` | `name` |
| `globals`, -1 | `global` | `type`, `init`, `group`, `binding`, `used` (by the unit's entries), `read`, `written`, `slot`, `slot_space`, `count_slot`, `argument`, `vertex_argument`, `paired_sampler`, `value` | `name`, `space`, `access`, `resource`, `slot_class`, `slot_name` |
| `functions`, -1 | `function` or `entry` | `result`, `body`, `arguments`, `locals`, `expressions` (counts), `workgroup` (items 0 to 2; -1 where an override sizes it), `workgroup_override` (items 0 to 2), `calls` (list of functions it calls), `uses` (list of globals it reaches, transitively), `may_kill`, `result_location`, `result_blend_src` | `name`, `stage`, `result_builtin`, `result_interpolation`, `result_sampling`, `early_depth_test` |
| `arguments`, function | `argument` | `type`, `location`, `blend_src` | `name`, `builtin`, `interpolation`, `sampling` |
| `locals`, function | `local` | `type`, `init` | `name` |
| `expressions`, function (-1 global) | the expression kinds of section 11.1 | `type` (resolved; interned if new), `shaped` (1 when the type is a shape no declared type names), `ref_count`, `bake`, `emits`, `line`, `bits` and `finite` (of a float literal), and the operands by their section 11.1 names (`base`, `index`, `value`, `left`, `right`, `expr`, `image`, `sampler`, `coordinate`, `array_index`, `offset`, `level_value`, `gradient_x`, `gradient_y`, `depth_ref`, `sample`, `condition`, `accept`, `reject`, `pointer`, `function`, `argument`, `global`, `local`, `constant`, `override`, `component`, `width`, `lanes`, `gather`, `items` (list)) | `op`, `fun`, `axis`, `control`, `literal`, `scalar`, `float` (the shortest text of a float literal's value as an `f64`), `pattern`, `level`, `query`, `named` |
| `statements`, block | the statement kinds of section 12.1 | `line`, `file` (an index into the unit's `files`), `first` and `last` (an `emit` range), `condition`, `accept`, `reject`, `body`, `continuing`, `break_if`, `selector`, `case_block` (list), `case_fall` (list), `case_ends` (list: 1 when the case's body cannot finish), `value`, `pointer`, `result`, `compare`, `image`, `coordinate`, `array_index`, `function`, `arguments` (list) | `fun`, `flags`, `case_value` (list: `3i32`, `3u32`, `default`) |
| `values`, -1 | `scalar` or `composite`: what the validator evaluated an initializer to | `type`, `items` (list of `values`), `bits`, `finite` | `literal`, `scalar`, `float` |

`bake` is 1 when the shared temporary rule of D-343 binds the expression to a temporary, and is set only on expressions an `emit` evaluates; `emits` is 1 for those. A `value` field names the record of `values` holding what the validator evaluated the initializer to, and is absent when it could not, so that an out writes a constant without evaluating it. `host_shared` is 1 for a struct a host writes (section 8.5). The slot fields come from the binding assignment for the unit's target (section 20): `slot` is the binding or register, `slot_space` the set or space, `slot_class` the HLSL register class, `slot_name` the GLSL name, `count_slot` the Metal length buffer index of section 20.3, `argument` and `vertex_argument` the Metal argument indices of section 20.6 (on the `msl` target only), `paired_sampler` the GLSL sampler a texture combines with (section 20.4), and `resource` the resource kind.

Names in the unit are the KSIR names. An out spells them for its target itself, by section 19.1 rule 6.

---

## 7. KSIR: module structure

### 7.1 Module

A KSIR module has:

| Field | Content |
| --- | --- |
| `name` | Module name, from the unit (for KSL, the file stem) |
| `lang` | The producing definition's `name` |
| `types` | Unique arena of types (section 8). Interning: one handle per distinct type |
| `constants` | Arena of named constants (section 14.1) |
| `overrides` | Arena of overrides (section 14.2) |
| `globals` | Arena of global variables (section 9.2) |
| `global_expressions` | Arena of constant and override expressions referenced by constants, overrides, array sizes, workgroup sizes and global initializers |
| `functions` | Arena of functions (section 13) |
| `entry_points` | Ordered list of entry points (section 10) |
| `shaders` | Ordered list of shader declarations (section 7.5) |
| `spans` | One span per entry of every arena above and per function-local arena entry |

A module that declares no shader is a library module: it validates, it prints as `.ksir`, and every backend emits nothing for it.

### 7.2 Arenas and handles

1. A handle is a 32-bit index into one arena. Handles are dense and start at 0. The null handle does not exist; optional references are optional.
2. **Topological order.** Every handle in an arena entry refers only to entries that precede it in the same arena, or to entries of an arena of an enclosing scope. In particular: a type refers only to earlier types; a global expression only to earlier global expressions; a function expression only to earlier expressions of the same function; a function calls only earlier functions. This makes cycles unrepresentable, so recursion is excluded by construction (section 13.3).
3. **Function-local arenas.** Each function owns its `expressions`, `locals` and `named_expressions` (a map from expression handle to a source name, used by backends to name temporaries).
4. **Determinism.** Arena order is a function of source order only (`KLF-RIDL.md` 9.4). Handles are never written to output except as `.ksir` indices.

### 7.3 Names

Types, constants, overrides, globals, functions, entry points and shaders carry a name. Locals, struct members and named expressions carry a source name when the source had one. Names are source names, unmangled. Backends rename to avoid target keywords and collisions with a deterministic scheme (section 19.1 rule 6) and record renames that reach the host (entry point names, resource names in GLSL) in reflection.

A module joined from several source files (KSL imports, MSL includes) has unique names per kind: the frontend qualifies imported names (`GlassCommon.specular` becomes `GlassCommon_specular`, as the oracle does).

### 7.4 Spans

Every arena entry carries a `Span` (`KLF-RIDL.md` 8.1). A generated entry (a polyfill, a zero-initializer, a marshalling temporary) carries the span of the construct it serves. Diagnostics from validation, passes and backends point at spans. Backends emit spans as line directives where the target supports them (`#line` in GLSL and HLSL, `OpLine` in SPIR-V when `--debug-info` is given); MSL and WGSL carry none.

### 7.5 Shader declarations

```
ShaderDecl {
    name: String
    kind: Graphics | Compute
    entries: [EntryPointIndex]
    groups: [GroupDecl]
    options: [OverrideHandle]
}
GroupDecl {
    name: String
    class: Frame | Pass | Material | Object | Draw | Dispatch | Custom
    globals: [GlobalHandle]
}
```

A shader is the unit of reflection and of output file naming. Its `groups` order defines neutral group indices (section 9.3); each group's `globals` order is the default binding order within the group. A graphics shader has at most one vertex and one fragment entry; a compute shader has exactly one compute entry. One module holds at most one shader in 1.0 (the KSL rule, `KSLS013`); the list form leaves room for more.

---

## 8. KSIR types and layout

### 8.1 Scalars

| Kind | Widths (bytes) | Notes |
| --- | --- | --- |
| `bool` | 1 (nominal) | No memory representation. Never in host-shareable memory or the stage interface (SV16) |
| `sint` | 4; 8 with capability `int64` | Two's complement |
| `uint` | 4; 8 with capability `int64` | |
| `float` | 4; 2 with capability `float16`; 8 with capability `float64` | IEEE 754 binary16, binary32, binary64 storage |

There are no abstract (untyped) numeric types in a module. A frontend resolves every literal to a concrete scalar before lowering. Text spelling: `bool`, `i32`, `u32`, `f32`, `f16`, `i64`, `u64`, `f64`.

### 8.2 Type table

| Type | Text | Constructible | Host-shareable | Notes |
| --- | --- | --- | --- | --- |
| Scalar | `f32` | Yes | Yes except `bool` | |
| Vector | `vec2<f32>` to `vec4<T>` | Yes | Yes except of `bool` | |
| Matrix | `mat<C>x<R><f32>` (`mat4x4<f32>`) | Yes | Yes | `C` columns of `R` rows, 2 to 4 each, float only, column-major |
| Atomic | `atomic<u32>`, `atomic<i32>` | No | Yes | Only in storage or workgroup space |
| Array | `array<T, N, stride S>`, `array<T, @o, stride S>`, `array<T, runtime, stride S>` | Fixed-size only | Yes when `T` is | `N >= 1` constant, or sized by override `@o`, or runtime-sized. Stride is explicit |
| Struct | `%Name` | When every member is | When every member is | Declared once with its layout rule, size and alignment, and for each member its type, optional binding, offset, size and alignment |
| Pointer | `ptr<space, T>` | No | No | Only as expression types and function parameters (section 13.2) |
| Image | `image<dim[, arrayed], class>` | No | No | Handle space only |
| Sampler | `sampler`, `sampler<comparison>` | No | No | Handle space only |

There is no "value pointer" type: a pointer to a vector component or matrix column is `ptr<space, f32>` or `ptr<space, vec4<f32>>` over the interned element type.

### 8.3 Images

| Field | Values |
| --- | --- |
| `dim` | `1d`, `2d`, `3d`, `cube` |
| `arrayed` | Whether the image has layers. `3d` cannot be arrayed. `cube` arrayed requires capability `cube-array` |
| `class` | `sampled<kind[, multisampled]>` where `kind` is `float`, `sint`, `uint`; `depth[<multisampled>]`; `storage<format, access>` |

Storage formats: `r8unorm`, `rg8unorm`, `rgba8unorm`, `rgba8snorm`, `rgba8uint`, `rgba8sint`, `bgra8unorm`, `r16float`, `rg16float`, `rgba16float`, `rgba16uint`, `rgba16sint`, `r32float`, `r32uint`, `r32sint`, `rg32float`, `rg32uint`, `rg32sint`, `rgba32float`, `rgba32uint`, `rgba32sint`, `rgb10a2unorm`, `rg11b10ufloat`. Storage access is a non-empty subset of `{load, store, atomic}`; `atomic` requires format `r32uint` or `r32sint`.

### 8.4 Layout rules

Every struct records the rule it was laid out under, its members' offsets, its size and its alignment. Every array records its stride. Nothing about memory layout is implied by a target or an address space.

Sizes and alignments by rule (`N` = scalar width, normally 4):

| Type | `std140` size / align | `std430` size / align | `scalar` size / align | `msl` size / align |
| --- | --- | --- | --- | --- |
| Scalar | N / N | N / N | N / N | N / N |
| `vec2` | 2N / 2N | 2N / 2N | 2N / N | 2N / 2N |
| `vec3` | 3N / 4N | 3N / 4N | 3N / N | 4N / 4N |
| `vec4` | 4N / 4N | 4N / 4N | 4N / N | 4N / 4N |
| `matCxR` | C x column stride / column align, column = array of `vecR` | C x round_up(size(vecR), align(vecR)) / align(vecR) | C x R x N / N | C x size(vecR) / align(vecR) |
| `array<T, n>` | n x stride, stride = round_up(size(T), max(align(T), 16)) / max(align(T), 16) | n x stride, stride = round_up(size(T), align(T)) / align(T) | n x stride, stride = size(T) rounded to align(T) / align(T) | n x stride, stride = round_up(size(T), align(T)) / align(T) |
| Struct | round_up(end, A) / A where A = round_up(max member align, 16) | round_up(end, A) / A where A = max member align | round_up(end, A) / A where A = max member align | round_up(end, A) / A where A = max member align |

`std140` and `std430` are GLSL's rules (OpenGL 4.6 section 7.6.2.2). `scalar` is `VK_EXT_scalar_block_layout`. `msl` is Metal's natural layout of non-packed types (Metal Shading Language Specification, section 2, "Alignment and size"). Members are placed in declaration order at the smallest offset not less than the end of the previous member that is a multiple of the member's alignment.

Under a named rule, a member's recorded size and alignment are its type's size and alignment under that rule. `explicit` structs carry offsets, member sizes and member alignments the frontend computed (section 6.5). A member's recorded size is at least its type's `scalar` size and may be larger (KSL records a 3-wide vector as 16 bytes); its recorded alignment is a multiple of its type's `scalar` alignment. The validator checks explicit structs by SV3 alone. The KSL profile lays out every host-shareable struct this way (section 22.5).

Recorded member sizes and alignments are what reflection reports (section 21.1). A backend realizes the offsets and the struct size; it does not need the member extents to agree with the target's own sizes.

A struct used in two address spaces with different rules is two KSIR types. A struct used only in the function, private or workgroup space, or in the stage interface, records rule `none`: its offsets exist for the text form but carry no meaning, and backends emit it in the target's natural form.

### 8.5 Host-shareable types

A type is host-shareable if it is a non-`bool` scalar, vector or matrix, an atomic, an array of a host-shareable type, or a struct of host-shareable members with a rule other than `none`. Only host-shareable types appear in the uniform, storage and immediate spaces (SV15).

---

## 9. Address spaces, globals and the resource model

### 9.1 Address spaces

| Space | Holds | Access | Global | Local | SPIR-V | MSL | HLSL | GLSL | WGSL |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `function` | Locals | load, store | No | Yes | `Function` | `thread` | local | local | `function` |
| `private` | Per-invocation module variables | load, store | Yes | No | `Private` | `thread`, threaded through as a hidden parameter | `static` | global | `private` |
| `workgroup` | Memory shared by a compute workgroup | load, store, atomic | Yes, compute only | No | `Workgroup` | `threadgroup` | `groupshared` | `shared` | `workgroup` |
| `uniform` | Uniform buffers | load | Yes | No | `Uniform` + `Block` | `constant` | `cbuffer` | `uniform` block | `uniform` |
| `storage<access>` | Storage buffers; `access` is a subset of `{load, store, atomic}` containing `load` | per `access` | Yes | No | `StorageBuffer` + `Block` | `device` (`const device` without `store`) | `StructuredBuffer` / `RWStructuredBuffer` | `buffer` block | `storage` |
| `handle` | Images and samplers | per image class | Yes | No | `UniformConstant` | `texture*`, `sampler` | `Texture*`, `SamplerState` | `sampler*`, `image*` | `handle` |
| `immediate` | Push constants | load | Yes, at most one per entry point | No | `PushConstant` | `constant` buffer at a reserved slot | root constants `cbuffer` | refused | refused |

A `GlobalVariable` expression in the `handle` space yields the handle value, not a pointer. In every other space it yields a pointer.

### 9.2 Global variables

```
GlobalVariable {
    name: String
    space: AddressSpace
    ty: TypeHandle
    binding: Optional<ResourceBinding>
    init: Optional<GlobalExpressionHandle>
}
```

1. `uniform`, `storage` and `handle` globals are **resources**. Each has a binding (SV13).
2. `private` globals may have an `init` that is a constant expression. Without one they are zero.
3. `workgroup` globals have no `init`. Their initial value is zero (section 15.7).
4. `immediate` globals have no binding and no `init`.
5. A resource belongs to exactly one group of its shader (section 7.5).

### 9.3 Neutral bindings

```
ResourceBinding { group: u32, binding: u32 }
```

`group` is the index of the resource's group in the shader's `groups`. `binding` is the resource's index within the group: its authored universal slot when it has one (section 9.4), otherwise its position in the group's `globals`. Two resources of one shader MUST NOT share a neutral binding (SV13; KSL reports it as `KSLS018` before lowering).

Neutral bindings are what WGSL and SPIR-V emit directly (`@group(g) @binding(b)`, `DescriptorSet g` `Binding b`). Every other target derives its slots from them by the deterministic assignment of section 20.

### 9.4 Authored slots

A frontend records a slot written in source with `authoredSlot`:

| Slot | Meaning | Source |
| --- | --- | --- |
| `universal(n)` | The resource's index within its group is `n` on every target | KSL `@binding(n)` |
| `msl.buffer(n)`, `msl.texture(n)`, `msl.sampler(n)` | Metal argument table index | MSL `[[buffer(n)]]`, `[[texture(n)]]`, `[[sampler(n)]]` |
| `hlsl(class, n, space)` | HLSL register `b`, `t`, `u` or `s` | A future HLSL frontend |
| `glsl(n)` | GLSL `layout(binding = n)` | A future GLSL frontend |

An authored slot wins over assignment for its target (section 20.1). Resources whose slots collide on one target are `shader.slot-collision` (section 20.5).

### 9.5 Runtime-sized arrays

A runtime-sized array appears only as the type of a `storage` global or as the last member of a struct that is the type of a `storage` global. Its length is read with `ArrayLength` on a pointer to it (section 11). Targets that cannot query a buffer's length (MSL) receive it from the host through a sizes buffer whose slot is assigned by section 20.3 and reported in reflection (`count` records, section 21.2).

### 9.6 Images and samplers

Images and samplers are separate resources in KSIR, as in KSL, MSL, HLSL, WGSL and SPIR-V. GLSL 4.30 without separate samplers combines them: its backend emits one `sampler2D` per (image, sampler) pair used by `ImageSample`, and reflection names the pairing (`paired_sampler`, section 20.4).

---

## 10. Entry points and the stage interface

### 10.1 Stages

| Stage | Status | Execution model |
| --- | --- | --- |
| `vertex` | Specified | Per vertex |
| `fragment` | Specified | Per fragment |
| `compute` | Specified | Per invocation in workgroups of `workgroup_size` |
| `task`, `mesh`, `ray-generation`, `any-hit`, `closest-hit`, `miss`, `intersection` | Reserved | Not part of 1.0 |

### 10.2 Entry points

```
EntryPoint {
    name: String
    stage: Stage
    function: Function
    workgroup_size: [WorkgroupExtent; 3]      compute only; each a positive u32 or an override expression
    early_depth_test: None | Force | Allow(GreaterEqual | LessEqual | Unchanged)   fragment only
}
```

An entry point's function is not in the `functions` arena and cannot be called (SV19). Entry point names are unique per module. Backends emit the name given; the KSL profile names entries `vertex_main`, `fragment_main` and `compute_main` as the oracle does.

### 10.3 Bindings on the interface

Every argument and the result of an entry point is either a value with a binding, or a struct whose every member has a binding. Nested structs are not part of an interface.

```
Binding = BuiltIn(BuiltIn) | Location { location: u32 or auto, interpolation: Optional<Interpolation>, sampling: Optional<Sampling>, blend_src: Optional<u32> }
Interpolation = perspective | linear | flat
Sampling = center | centroid | sample
```

`auto` is allowed only in a lowered module; the normalization pass (section 17.1 step 1) replaces it before validation. KSL leaves every location `auto`.

### 10.4 Built-ins

| Built-in | Type | Stage and direction |
| --- | --- | --- |
| `position` | `vec4<f32>` | vertex out; fragment in (window coordinates) |
| `vertex_index` | `u32` | vertex in |
| `instance_index` | `u32` | vertex in |
| `front_facing` | `bool` | fragment in |
| `frag_depth` | `f32` | fragment out |
| `sample_index` | `u32` | fragment in; requires capability `sample-shading` |
| `sample_mask` | `u32` | fragment in, fragment out |
| `global_invocation_id` | `vec3<u32>` | compute in |
| `local_invocation_id` | `vec3<u32>` | compute in |
| `local_invocation_index` | `u32` | compute in |
| `workgroup_id` | `vec3<u32>` | compute in |
| `num_workgroups` | `vec3<u32>` | compute in |
| `clip_distances` | `array<f32, N>` | vertex out; requires capability `clip-distances` |

`front_facing` is the one `bool` allowed in an interface. Appendix C gives each built-in's spelling per target. Base-vertex and base-instance inclusion in `vertex_index` and `instance_index` follow each graphics API and are the host's concern; KSIR records the built-in, not a normalization.

### 10.5 Locations and stage matching

1. Within one entry point, locations are unique per direction among `Location` bindings (SV17). `blend_src` is 0 or 1, only on fragment outputs at location 0, and requires capability `dual-source-blending`.
2. An integer or integer-vector `Location` binding MUST be `flat` (SV17). A float binding without interpolation is `perspective` with `center` sampling.
3. In a graphics shader, every fragment input location MUST be produced by a vertex output of the same location, the same type and the same interpolation and sampling (SV18). A vertex output no fragment input reads is allowed.
4. Normalization assigns `auto` locations in member declaration order, skipping built-ins, starting at 0, separately for each argument or result struct. This is the oracle's rule and is what KSL means.

### 10.6 Stage rules

1. A vertex entry MUST write the `position` built-in.
2. A fragment entry's `Location` outputs are color attachments by location.
3. A compute entry has no result (SV20). A compute entry's arguments are built-ins only.
4. `kill` occurs only in functions reachable only from fragment entries (SV21).
5. `workgroup` globals and barriers occur only in functions reachable only from compute entries (SV21).
6. Each `workgroup_size` extent is at least 1. Upper limits are device properties and are checked by the host from reflection, not by KSIR. The KSL profile additionally limits the product to 256.

---

## 11. Expressions

### 11.1 Expression kinds

| Kind | Operands | Result | Evaluated | Const |
| --- | --- | --- | --- | --- |
| `literal v` | A typed literal (`1.0f32`, `7u32`, `-3i32`, `true`) | Its scalar | Before execution | Yes |
| `constant c` | Constant handle | The constant's type | Before execution | Yes |
| `override o` | Override handle | The override's type | Before execution | Override |
| `zero T` | Constructible type | `T` with every component zero, `false` | Before execution | Yes |
| `compose T, [e...]` | Components or members in order; a vector may be composed from smaller vectors whose sizes sum to its size | `T` | When emitted | Yes |
| `access b, i` | `b` a vector, matrix, array or pointer to one of those, or to a runtime array; `i` an `i32` or `u32` | Element value, or pointer to element when `b` is a pointer | When emitted | Yes when `b`, `i` are |
| `access_index b, n` | As `access` with a constant index; also a struct member index | Element or member value, or pointer to it | When emitted | Yes |
| `splat n, e` | Scalar `e` | `vecn` of `e` | When emitted | Yes |
| `swizzle e, [c...]` | Vector `e`; 1 to 4 components from `x y z w` within its size | Vector of that size, or the scalar for one component | When emitted | Yes |
| `arg n` | Parameter index | Parameter type | On function entry | No |
| `global g` | Global handle | `ptr<space, T>`, or the handle value for `handle` space | Before execution | No |
| `local l` | Local handle | `ptr<function, T>` | On function entry | No |
| `load p` | Pointer to a constructible type or an atomic | The pointee, or the atomic's scalar | When emitted | No |
| `image_sample` | `image`, `sampler`, `coordinate`, optional `array_index`, optional constant `offset`, `level`, optional `depth_ref`, optional `gather` component | `vec4` of the image's kind, or `f32` for a depth compare | When emitted | No |
| `image_load` | `image`, `coordinate`, optional `array_index`, optional `sample`, optional `level` | `vec4` of the image's kind or format | When emitted | No |
| `image_query` | `image`; `size(level?)`, `levels`, `layers`, `samples` | `u32` or `vecN<u32>` | When emitted | No |
| `unary op, e` | `negate`, `not` (bool), `bit_not` (integer) | Operand type | When emitted | Yes |
| `binary op, l, r` | Section 11.4 | Section 11.4 | When emitted | Yes |
| `select c, a, r` | `c` a `bool` or bool vector of `a`'s size; `a`, `r` of one type | That type | When emitted | Yes |
| `derivative axis, control, e` | `axis` `x`, `y`, `width`; `control` `coarse`, `fine`, `none`; float scalar or vector `e` | Operand type | When emitted, fragment only | No |
| `relational f, e` | `all`, `any` over bool vectors; `is_nan`, `is_inf` over floats | `bool` or bool vector | When emitted | Yes |
| `math f, a[, b[, c]]` | Section 11.5 | Section 11.5 | When emitted | Yes |
| `convert e, kind[, width]` | Numeric conversion to `kind` at `width` (default: operand width) | Same shape, new scalar | When emitted | Yes |
| `bitcast e, T` | Same total size, scalar or vector of 32-bit components | `T` | When emitted | Yes |
| `call_result f` | Produced by a `call` statement | Callee result type | By the `call` statement | No |
| `atomic_result T` | Produced by an `atomic` statement | `T` | By the `atomic` statement | No |
| `array_length p` | Pointer to a runtime-sized array | `u32` | When emitted | No |

`level` of `image_sample` is one of `auto` (implicit, from derivatives; fragment only), `zero`, `exact(e)`, `bias(e)` (fragment only), `gradient(dx, dy)`. A depth compare uses `zero` or `auto` only.

### 11.2 Evaluation time

The rules are naga's:

- `literal`, `constant`, `override`, `zero` and `global` are evaluated before execution begins.
- `arg` and `local` are evaluated on function entry. `local` yields a pointer whose address does not change.
- `call_result` and `atomic_result` are evaluated when the statement that names them as its result executes.
- Every other expression is evaluated when the unique `emit` statement whose range covers it executes. Every such expression is covered by exactly one `emit` (SV7).

Only `load`, `image_sample`, `image_load`, `image_query`, `array_length` and `derivative` observe when they run. Backends MAY evaluate other expressions at any point after their operands, but MUST evaluate the time-sensitive ones exactly where they are emitted. A backend that does not inline an expression stores it in a temporary at its `emit`. Backends MUST bake an expression into a temporary when it is used more than once, or when it is time-sensitive and its use is not in the same statement as its `emit` (the "bake reference count" rule; the validator's `ModuleInfo` provides reference counts).

### 11.3 Scope

- `literal`, `constant`, `override`, `zero`, `global`, `arg` and `local` are in scope in the whole function.
- An expression covered by an `emit` is in scope in the expressions that follow it in the same `emit`, in the statements that follow the `emit` in its block, and in their nested blocks.
- The result of a `call` or `atomic` is in scope in the statements that follow it in its block and their nested blocks.

Using an expression out of scope is SV8. A value needed after its block ends goes through a local.

### 11.4 Binary operators

| Operators | Operands | Result |
| --- | --- | --- |
| `add`, `subtract` | Two numeric scalars or vectors of one type; or a vector and a scalar of its component type; or two matrices of one type | Operand type (vector for mixed) |
| `multiply` | As `add`; also matrix x scalar, matrix x matrix (`matKxR` x `matCxK` gives `matCxR`), matrix x vector (`matCxR` x `vecC` gives `vecR`), vector x matrix (`vecR` x `matCxR` gives `vecC`) | Per linear algebra, column-major |
| `divide`, `modulo` | Numeric scalars or vectors of one type, or vector and scalar | Operand type. Integer edge cases per section 15.2. `modulo` on floats truncates (`x - y * trunc(x / y)`) |
| `equal`, `not_equal` | Two scalars or vectors of one type, including `bool` | `bool` or bool vector |
| `less`, `less_equal`, `greater`, `greater_equal` | Two numeric scalars or vectors of one type | `bool` or bool vector |
| `and`, `inclusive_or`, `exclusive_or` | Two integer or two `bool` scalars or vectors of one type | Operand type. On `bool`, non-short-circuiting |
| `logical_and`, `logical_or` | Two `bool` scalars | `bool`. Both operands are evaluated; a frontend whose language short-circuits a side-effecting right operand lowers it to `if` |
| `shift_left`, `shift_right` | Integer scalar or vector; shift amount `u32` of the same shape | Left operand type. Section 15.2 |

There is no implicit conversion. Mixed vector-scalar forms are the only shape mismatch accepted; backends splat where the target needs it.

### 11.5 Math functions

| Group | Functions |
| --- | --- |
| Comparison | `abs`, `min`, `max`, `clamp`, `saturate`, `sign` |
| Trigonometric | `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `radians`, `degrees` |
| Rounding | `ceil`, `floor`, `round` (ties to even), `fract` (`x - floor(x)`), `trunc` |
| Exponential | `exp`, `exp2`, `log`, `log2`, `pow`, `sqrt`, `inverse_sqrt` |
| Geometric | `dot`, `cross` (`vec3`), `distance`, `length`, `normalize`, `face_forward`, `reflect`, `refract` |
| Interpolation | `mix`, `step`, `smoothstep`, `fma` |
| Matrix | `transpose`, `determinant` |
| Bits | `count_one_bits`, `count_leading_zeros`, `count_trailing_zeros`, `first_leading_bit`, `first_trailing_bit`, `reverse_bits`, `extract_bits`, `insert_bits` |
| Packing | `pack4x8snorm`, `pack4x8unorm`, `pack2x16snorm`, `pack2x16unorm`, `pack2x16float`, and the matching `unpack*` |

Argument and result types follow WGSL's built-in function table for the function of the same name. Functions whose result is undefined on some inputs in some target (for example `smoothstep` with `low >= high`, `pow` with a negative base, `inverse_sqrt(0)`) produce a target-defined value; they never trap.

---

## 12. Statements and control flow

### 12.1 Statement kinds

| Statement | Meaning |
| --- | --- |
| `emit { ... }` | Evaluates a contiguous range of expressions (section 11.2) |
| `block { ... }` | A nested block; opens a scope |
| `if c { ... } else { ... }` | `c` is a `bool` scalar |
| `switch s { case v: { ... } ... default: { ... } }` | `s` is an `i32` or `u32`; case values are constants of `s`'s type, unique; exactly one `default`; a case marked `fallthrough` has an empty body and is not the last case |
| `loop { body } continuing { ... } [break_if e]` | Runs `body`, then `continuing`, then evaluates `e` (when present) and leaves the loop if it is true; repeats otherwise |
| `break` | Leaves the innermost `loop` or `switch` |
| `continue` | Jumps to the innermost loop's `continuing` |
| `return [e]` | Leaves the function. `e` is present if and only if the function has a result |
| `kill` | Discards the fragment. Fragment only |
| `store p, v` | Writes `v` through pointer `p`. `p` is writable (section 9.1) and its pointee constructible |
| `image_store` | `image`, `coordinate`, optional `array_index`, `value`. The image is `storage` with `store` access |
| `atomic p, f, v[, compare] -> r?` | `f` is `add`, `subtract`, `and`, `or`, `xor`, `min`, `max`, `exchange`, or `compare_exchange` (with `compare`); `p` a pointer to an atomic in `storage<atomic>` or `workgroup` space; optional result |
| `image_atomic` | Atomic on an `r32uint`/`r32sint` storage image texel; requires capability `image-atomics` |
| `call f(args) -> r?` | Calls function `f`; optional result expression |
| `control_barrier flags` | Workgroup execution barrier with memory semantics `flags` in `{storage, workgroup, image}`. Compute only, uniform control flow |
| `memory_barrier flags` | Memory barrier without execution synchronization. Compute only |
| `workgroup_uniform_load p -> r` | Loads a `workgroup` pointer and broadcasts it as uniform, with the barriers that implies |

Reserved for later revisions: subgroup operations, ray queries, debug printf.

### 12.2 Structured control flow rules

1. Control flow is structured only. There is no `goto`, no labeled break, no unstructured branch. Every construct has one entry and one exit.
2. `break` occurs inside a `loop` body or a `switch` case; `continue` inside a `loop` body, not inside a `switch` that is inside no loop (SV10).
3. A `continuing` block contains no `return`, `kill`, `break` or `continue`, and no `loop` whose body does (SV10).
4. A function with a result returns on every path that reaches its end (SV11). A frontend may append `return zero T` where its language makes the end unreachable but cannot prove it.
5. A `while c { body }` in source lowers to `loop { emit c; if !c { break } body } continuing { }`. A `for` lowers its step into `continuing`.

---

## 13. Functions and calls

### 13.1 Function

```
Function {
    name: String
    parameters: [{ name: Optional<String>, ty: TypeHandle }]
    result: Optional<TypeHandle>
    locals: Arena<{ name: Optional<String>, ty: TypeHandle, init: Optional<ExprHandle> }>
    expressions: Arena<Expression>
    named_expressions: Map<ExprHandle, String>
    body: Block
}
```

A local's `init` is an expression of the same function that is in scope at function entry (a constant expression or `arg`), or absent, in which case the local starts at zero (section 15.7).

### 13.2 Parameters and results

| Parameter type | Allowed |
| --- | --- |
| Constructible type | Yes |
| Image or sampler handle | Yes. Backends that cannot pass handles (none of the 1.0 targets) inline the callee |
| `ptr<function, T>`, `ptr<private, T>` | Yes |
| `ptr<workgroup, T>` | Yes |
| `ptr<uniform or storage or immediate, T>` | No (SV19). Pass the value, or let the callee read the global |
| Runtime-sized array or atomic by value | No |

A result is a constructible type. A function may read and write any global of a resource or private space directly; it does not need it as a parameter. Backends whose target requires resources to be entry point arguments (MSL) thread them through hidden parameters of every function that uses them, transitively.

### 13.3 Calls

A call names an earlier function in the arena (section 7.2), passes arguments matching the parameter types, and names a `call_result` expression when the callee has a result. Recursion and mutual recursion are unrepresentable. A frontend whose language allows recursion rejects it before lowering.

---

## 14. Constants, overrides and constant evaluation

### 14.1 Constants

```
Constant { name: String, ty: TypeHandle, init: GlobalExpressionHandle }
```

`init` is a constant expression: built only from `literal`, `constant`, `zero`, `compose`, `access`, `access_index`, `splat`, `swizzle`, `unary`, `binary`, `select`, `relational`, `math`, `convert` and `bitcast` over constant expressions. The validator evaluates every constant (section 14.4); a constant whose evaluation fails is SV5.

### 14.2 Overrides

```
Override { name: String, id: Optional<u16>, type: bool | i32 | u32 | f32 | f16, init: Optional<GlobalExpressionHandle> }
```

An override is a scalar the host may set at pipeline creation. `init` is an override expression (a constant expression that may also reference earlier overrides) giving the default. An override with no `init` must be supplied by the host. `id` is the numeric identity a host uses to set it; without one the host sets it by name. Ids are unique per module.

Override expressions may size an array (`array<T, @o, stride S>`) and give a workgroup extent.

### 14.3 Mapping per target

| Target | `--overrides keep` | `--overrides bake` |
| --- | --- | --- |
| MSL | `constant T name [[function_constant(id)]]`, with the default applied through `is_function_constant_defined` | Literal values |
| SPIR-V | `OpSpecConstant*` with `SpecId` | Literal values |
| WGSL | `override name: T = default` with `@id` | Literal values |
| HLSL | Refused (`shader.override-unsupported`) | Literal values |
| GLSL 4.30 | Refused (`shader.override-unsupported`) | Literal values |

`bake` replaces each override by the host-supplied value or its default (section 17.3). `bake` is the default because it is what the oracle does with KSL options.

### 14.4 Constant evaluation

The kernel evaluates constant and override expressions with the semantics of section 15, exactly (integer edge cases included, independent of `--integer-division`). Float arithmetic uses IEEE 754 binary32 (or the expression's width) with round-to-nearest-even. A constant or baked override that evaluates to NaN or an infinity is `shader.const-not-finite`, because MSL and HLSL source cannot spell them as literals portably.

---

## 15. Execution semantics

### 15.1 Invocations

Each entry point runs once per invocation: per vertex, per fragment (including helper invocations), or per compute invocation. Invocations share nothing except resources, workgroup memory (within a workgroup) and the values of uniform built-ins.

### 15.2 Integers

1. `add`, `subtract`, `multiply`, `negate` wrap modulo 2^width.
2. With `--integer-division defined` (default): `x / 0` is `x`; `x % 0` is `0`; for signed `x`, `INT_MIN / -1` is `INT_MIN` and `INT_MIN % -1` is `0`. Division truncates toward zero; the remainder has the sign of the dividend. Backends whose target leaves these cases undefined emit a guard (`select` on the divisor) through the pass of section 17.1. WGSL's native semantics already match and need no guard.
3. With `--integer-division native`: the guards are omitted and the edge cases are whatever the target does. This is the oracle's behavior.
4. Shift amounts are taken modulo the operand's bit width. `shift_right` is arithmetic on `sint`, logical on `uint`.
5. `convert` from float to integer saturates to the integer range; NaN converts to 0. From integer to float rounds to nearest. Between integers of different widths it truncates or extends by the source signedness.

### 15.3 Floating point

Float operations follow the target's precision rules, which are weaker than IEEE 754: implementations may flush denormals, may not preserve the sign of zero, and may treat NaN and infinity as undefined (Metal compiles with fast math unless the host disables it). KSIR guarantees only what every target guarantees: storage is IEEE 754 at the type's width, and operations on finite, normal inputs with finite, normal results are accurate to the target's documented ULP bounds. Programs that need NaN or infinity semantics are outside what KSIR promises.

### 15.4 Booleans

`bool` has no memory representation. It never crosses into host-shareable memory or the interface (except `front_facing`). Composites of `bool` exist only in the function and private spaces.

### 15.5 Out-of-bounds access

The `--bounds` policy decides what an index out of range does, for `access` on arrays (including runtime-sized arrays), vectors and matrices with a non-constant index, and for `image_load` and `image_store` coordinates, array layers, samples and levels:

| Policy | Read | Write |
| --- | --- | --- |
| `unchecked` (default) | Target behavior (undefined in MSL and HLSL; robustness-dependent in Vulkan; clamped or zero in WebGPU) | Target behavior |
| `restrict` | Index clamped to the last valid element | Index clamped |
| `read-zero-skip-write` | Zero | Discarded |

A constant index out of range is a validation error regardless of policy (SV12). `unchecked` is the default for parity with the oracle; hosts exposed to untrusted shader input SHOULD use `restrict`.

### 15.6 Derivatives and uniformity

`derivative`, `image_sample` with `auto` or `bias` level, and barriers require uniform control flow (section 16.5). Their results in non-uniform control flow are undefined on every target.

### 15.7 Initial values

Locals without an initializer, private globals without one, and workgroup globals are zero at the start of their lifetime (function entry, invocation start, workgroup start). Backends whose target leaves memory uninitialized emit the zeroing: for workgroup memory, a cooperative zeroing loop and a workgroup barrier at the start of the compute entry, inserted by the pass of section 17.1 when the module has workgroup globals.

---

## 16. Validation

### 16.1 When

The driver validates every module after lowering and normalization, before any pass. Debug builds of a driver validate again after the passes. `klf ksir verify <file.ksir>` validates a text module.

### 16.2 Rules

| Rule | Statement |
| --- | --- |
| SV1 | Every handle is in range and refers only to earlier entries (section 7.2) |
| SV2 | Every type is well formed: vector and matrix sizes in range, matrix scalars float, atomic scalars 32-bit integers (64-bit with `int64-atomics`), array sizes positive, array strides at least the element size and multiples of its alignment |
| SV3 | Every struct's members are in offset order; each offset is a multiple of the member's recorded alignment; recorded extents (`offset` to `offset + size`) do not overlap; the struct's alignment is a multiple of 4 (or of its widest scalar); its size covers the last member's extent and is a multiple of its alignment; a runtime-sized array is only its last member |
| SV4 | Every struct with a layout rule other than `explicit` and `none` matches its rule exactly (section 8.4) |
| SV5 | Every constant and override expression evaluates (section 14.4) and has its declared type |
| SV6 | Every expression is well typed (section 11) |
| SV7 | Every expression that is evaluated when emitted is covered by exactly one `emit`, and an `emit` covers expressions in arena order |
| SV8 | Every expression is used only within its scope (section 11.3) |
| SV9 | `store` writes a pointer whose space and access allow it; `load` reads one that allows it; atomics only on atomics with `atomic` access |
| SV10 | `break`, `continue`, `continuing` follow section 12.2 |
| SV11 | A function with a result returns a value of its type on every path; a function without one never returns a value |
| SV12 | A constant index into a fixed-size array, vector or matrix is in range |
| SV13 | Every resource has a neutral binding, unique within its shader, and belongs to exactly one group; non-resources have none |
| SV14 | Every global's type is allowed in its space (atomics only in storage and workgroup; handles only in handle space; runtime arrays per section 9.5) |
| SV15 | Uniform, storage and immediate globals have host-shareable types |
| SV16 | No `bool` in host-shareable memory or the interface, except the `front_facing` built-in |
| SV17 | Interface bindings follow section 10.3 to 10.5: each leaf bound once, built-ins legal for the stage and direction with the right type, locations unique per direction, integers flat |
| SV18 | Vertex outputs and fragment inputs of one graphics shader match (section 10.5 rule 3) |
| SV19 | Entry points are not called; parameters follow section 13.2; call arguments match parameter types |
| SV20 | A compute entry has no result and declares a workgroup size; other stages declare none |
| SV21 | Stage-specific operations (`kill`, `derivative`, `auto` and `bias` sampling; `workgroup` access, barriers) occur only in functions reachable only from entries of the stages that allow them |
| SV22 | Every override id is unique; every override without a default is reported in reflection as required |
| SV23 | Image operations match the image class: `image_sample` on `sampled` and `depth` images only, with a comparison sampler exactly when `depth_ref` is present; `image_load` on any class but `depth` with a `sampler` it does not take; `image_store` on `storage` images with `store` access; coordinates of the dimension's size |
| SV24 | Capability use is declared: every construct requiring a capability (sections 8 to 12) appears only when the module's capability set contains it |

### 16.3 Module information

Validation produces `ModuleInfo`, which passes, backends and reflection read:

| Per | Information |
| --- | --- |
| Expression | Resolved type; reference count (uses by expressions and statements); uniformity (the expression whose non-uniformity it inherits, if any) |
| Function | Globals read and written, with access; whether it may `kill`; uniformity requirements; functions called |
| Entry point | Transitive globals read and written; capabilities used |
| Module | Constant values (evaluated); override dependency order |

Resource visibility in reflection (which stages read a resource) is the entry point's transitive global use, computed here rather than by walking source trees as the oracle does.

### 16.4 Diagnostics

The validator reports two kinds of failure:

1. **Definition bugs** (SV1 to SV11, SV13, SV14, SV19, SV20 when the source language should have rejected the program): `KLF-SHD-9xxx`, internal errors naming the definition, as `KLF-RIDL.md` 7.5 rule 4 does for KIR.
2. **User errors the kernel is the right place to find**: reported through keys that each definition maps to its own codes, as `KLF-RIDL.md` 31.4 does for ownership:

| Key | Meaning |
| --- | --- |
| `shader.uniformity` | A derivative, implicit-level sample or barrier in non-uniform control flow (section 16.5) |
| `shader.const-not-finite` | A constant or baked override is NaN or infinite |
| `shader.const-index` | A constant index out of range (SV12) |
| `shader.interface-mismatch` | Vertex outputs and fragment inputs disagree (SV18) |
| `shader.override-missing` | `--overrides bake` and an override has no default and no supplied value |
| `shader.override-unsupported` | `--overrides keep` for a target without specialization |
| `shader.target-unsupported` | A backend cannot express a construct (section 19.1) |
| `shader.capability` | A target does not support a capability the module uses |
| `shader.slot-collision` | Two resources land on one slot of one target (section 20.5) |

A definition MUST map every key. KSL maps them into its `KSLS` family (section 22.8).

### 16.5 Uniformity

The analysis is naga's: an expression is non-uniform if it depends on a built-in that varies per invocation (`position`, `vertex_index`, `instance_index`, `front_facing`, `sample_index`, `sample_mask`, `local_invocation_id`, `local_invocation_index`, `global_invocation_id`), on a `load` from a writable storage or workgroup pointer, on an `atomic_result`, on a non-uniform `arg`, or on a non-uniform operand. A point of control flow is non-uniform after a branch on a non-uniform condition, a `kill`, or a `return` reachable under a non-uniform condition.

An operation that requires uniform control flow (section 15.6) at a non-uniform point is reported as `shader.uniformity`:

- as an error when the requested target set contains `wgsl`, which rejects such programs;
- as a warning otherwise, because MSL, HLSL, GLSL and SPIR-V accept them with undefined results in the divergent lanes.

### 16.6 Capabilities

| Capability | Enables | MSL | SPIR-V | HLSL | GLSL 4.30 | WGSL |
| --- | --- | --- | --- | --- | --- | --- |
| `float16` | `f16` | Yes | `Float16` + storage extension | SM 6.2 | Refused | `enable f16` |
| `float64` | `f64` | Refused | `Float64` | Yes | Yes | Refused |
| `int64` | `i64`, `u64` | Yes (MSL 2.2+) | `Int64` | SM 6.0 | Refused | Refused |
| `int64-atomics` | 64-bit atomics | MSL 3.1, limited | `Int64Atomics` | SM 6.6 | Refused | Refused |
| `image-atomics` | `image_atomic` | MSL 3.1 | Yes | Yes | Yes | Refused |
| `cube-array` | Arrayed cube images | Yes | `ImageCubeArray` | Yes | Yes | Yes |
| `multisampled` | Multisampled sampled images | Yes | Yes | Yes | Yes | Yes |
| `sample-shading` | `sample_index`, `sample` sampling | Yes | `SampleRateShading` | Yes | Yes | Yes |
| `clip-distances` | `clip_distances` | Yes | `ClipDistance` | Yes | Yes | `enable clip_distances` |
| `dual-source-blending` | `blend_src` | Yes | Yes | Yes | Yes | `enable dual_source_blending` |
| `storage-image-read-write` | Storage images with both `load` and `store` | Yes (`access::read_write`) | Yes | Yes | Yes | `read_write` textures (format-limited) |

"Refused" means the backend reports `shader.capability`. A module's capability set starts as what its frontend declares (section 5.3); the validator rejects constructs outside it (SV24) so that a frontend opts in deliberately. After normalization the driver narrows the set to the capabilities the module uses, so a shader that uses no `f16` gets no `enable f16` and is not refused for `float16`. The passes then refuse, before any backend runs, a module whose set holds a capability the requested target marks "Refused", naming the capability and the target.

---

## 17. Passes

### 17.1 Mandatory pipeline

Run in this order by the driver for each target:

1. **Normalize** (before validation): assign `auto` locations (section 10.5 rule 4); resolve universal authored slots into neutral bindings (section 9.3).
2. **Validate** (section 16).
3. **Select stages**: for per-stage targets (SPIR-V, HLSL, GLSL, WGSL), produce one module per entry point; for MSL, keep every entry point.
4. **Overrides**: bake or keep (section 17.3).
5. **Bounds**: rewrite indexing per `--bounds` (section 15.5) with `select` and `min` against lengths and sizes. No-op for `unchecked`.
6. **Integer edge cases**: guard division, remainder and float-to-integer conversion per section 15.2, unless the target's native semantics already match (WGSL) or `--integer-division native`.
7. **Initial values**: insert zeroing for workgroup globals at compute entry when the target does not zero them (all but WGSL, whose implementations do).
8. **Compact**: remove types, constants, overrides, globals, functions and expressions not reachable from the remaining entry points. Arena order of what remains is preserved.
9. **Validate** (debug builds only).

Every rewrite gives new nodes the span of the construct they serve (section 7.4) and leaves the module valid.

### 17.2 Optional passes

| Pass | Effect | Default |
| --- | --- | --- |
| Constant folding | Replaces constant subexpressions by their values | On |
| Dead local elimination | Removes locals never loaded | On |
| Inlining | Inlines functions called once | Off in 1.0 |

Optional passes MUST NOT change observable results (section 15).

### 17.3 Overrides

`--overrides bake` takes values from `--override <name or id>=<value>` flags (repeatable) and otherwise from defaults. It replaces each `override` expression by a `literal`, re-evaluates override expressions, resolves override-sized arrays to fixed-size arrays and override workgroup extents to numbers, and removes the overrides. Reflection still lists them with their baked values (`option` records, section 21.2).

`--overrides keep` leaves overrides for the backend (section 14.3) and records in reflection that they are specializable.

---

## 18. Textual KSIR

### 18.1 Purpose and guarantees

`.ksir` is the human and test form of a module. A driver prints it with `--emit ksir`; `klf ksir verify <file>` parses and validates it; `klf ksir dump <file.ksirb>` prints a cached binary module.

1. **Round trip.** For every valid module `m`, `parse(print(m))` equals `m` (same arenas, same order, same spans), and `print(parse(t))` equals `t` for every canonically printed `t`.
2. **Arena order is text order.** Types, global expressions, constants, overrides, globals, functions and, inside a function, expressions appear in arena order. Expression `%n` is the `n`-th entry of its function's expression arena; `%gN` of the global expression arena.
3. **Floats print exactly.** A float literal prints as the shortest decimal that round-trips at its width, with its width suffix (`0.9f32`), or as `nan`/`inf` forms for bit patterns that are not finite (which only appear in `bitcast`-produced values, never in literals of a valid module).
4. **Spans** print as a trailing `!loc("<path>":<line>:<col>)` on the line of the node they belong to. `--debug-paths=absolute` controls paths as in `KLF-RIDL.md` 5.7.

### 18.2 Example (informative)

KSL source:

```ksl
type Camera {
    let view_projection: Float4x4
}

type VertexIn {
    @builtin(vertex_index)
    let vid: UInt
}

type VertexOut {
    @builtin(position)
    let clip: Float4
    let color: Float4
}

type FragmentOut {
    let color: Float4
}

shader Tri {
    group Frame {
        uniform camera: Camera
    }

    vertex {
        input VertexIn
        output VertexOut

        function entry(input: VertexIn) -> VertexOut {
            let out: VertexOut
            let x = Float(input.vid) - 1.0
            out.clip = mul(camera.view_projection, Float4(x, 0.0, 0.0, 1.0))
            out.color = Float4(1.0, 0.0, 0.0, 1.0)
            return out
        }
    }

    fragment {
        input VertexOut
        output FragmentOut

        function entry(input: VertexOut) -> FragmentOut {
            let out: FragmentOut
            out.color = input.color
            return out
        }
    }
}
```

Its KSIR after normalization (spans omitted):

```
ksir 1
module @Tri lang "ksl" capabilities()

type %Camera = struct explicit size 64 align 16 {
  view_projection: mat4x4<f32> offset 0 size 64 align 16
}
type %VertexIn = struct none {
  vid: u32 @builtin(vertex_index)
}
type %VertexOut = struct none {
  clip: vec4<f32> @builtin(position),
  color: vec4<f32> @location(0)
}
type %FragmentOut = struct none {
  color: vec4<f32> @location(0)
}

global @camera: %Camera space(uniform) binding(0, 0)

shader @Tri graphics {
  group @Frame class(frame) { @camera }
  entries { @vertex_main, @fragment_main }
}

entry vertex @vertex_main(%input: %VertexIn) -> %VertexOut {
  local $out: %VertexOut
  local $x: f32
  %0 = arg 0
  %1 = local $out
  %2 = local $x
  %3 = global @camera
  %4 = literal 1.0f32
  %5 = literal 0.0f32
  emit {
    %6 = access_index %0, 0
    %7 = convert %6, float
    %8 = binary subtract %7, %4
  }
  store %2, %8
  emit {
    %9 = access_index %1, 0
    %10 = access_index %3, 0
    %11 = load %10
    %12 = load %2
    %13 = compose vec4<f32>, [%12, %5, %5, %4]
    %14 = binary multiply %11, %13
  }
  store %9, %14
  emit {
    %15 = access_index %1, 1
    %16 = compose vec4<f32>, [%4, %5, %5, %4]
  }
  store %15, %16
  emit {
    %17 = load %1
  }
  return %17
}

entry fragment @fragment_main(%input: %VertexOut) -> %FragmentOut {
  local $out: %FragmentOut
  %0 = arg 0
  %1 = local $out
  emit {
    %2 = access_index %1, 0
    %3 = access_index %0, 1
  }
  store %2, %3
  emit {
    %4 = load %1
  }
  return %4
}
```

Note what the text says that the oracle's pipeline left implicit: the uniform's layout, the interface locations, where each value is evaluated, that `mul` is a column-major matrix-vector product, and that `let` is a variable (KSL rebinds `let`).

### 18.3 Statement and declaration forms

| Form | Text |
| --- | --- |
| Constant | `const @name: T = %gN` |
| Override | `override @name: T [id(N)] [= %gN]` |
| Global | `global @name: T space(<space>[, <access>]) [binding(G, B)] [slot(<target slot>) ...] [= %gN]` |
| Function | `func @name(%p: T, ...) [-> T] { locals, expressions, statements }` |
| Entry point | `entry <stage> @name(...) [-> T] [workgroup(X, Y, Z)] [early_depth(<mode>)] { ... }` |
| Call | `call @f(%a, %b) [-> %n]` |
| Atomic | `atomic <fun> %p, %v [, %cmp] [-> %n]` |
| Loop | `loop { ... } continuing { ... } [break_if %n]` |
| Switch | `switch %s { case 1i32: { ... } case 2i32 fallthrough: { } default: { ... } }` |

The full grammar is Appendix A.

---

## 19. Backends

A backend is the out of a shader definition (section 5.8), except SPIR-V, which the kernel writes. Sections 19.3 and 19.5 to 19.7 specify what the `msl`, `hlsl`, `glsl` and `wgsl` outs write, and section 22.9 the `ksl` out; this contract binds every one of them.

### 19.1 Common contract

1. **Input.** A backend receives a module that passed validation and the mandatory passes for its target, its `ModuleInfo`, the binding assignment of section 20, and the target options. It reads nothing else: not the source, not the frontend's trees.
2. **No decisions.** A backend never chooses a layout, a slot, a location, a sampling level, an integer edge-case behavior or an override value. Each is in the module or the assignment.
3. **Refuse, never substitute.** A construct the target cannot express is reported as `shader.target-unsupported` with the construct's span and a message naming the target, the construct and why. The backend emits nothing for that shader on that target.
4. **Layout realization.** For every host-shareable type the backend emits a declaration whose byte layout on the target equals the module's offsets, sizes and strides exactly: by choosing target types (for example `packed_float3` in MSL), by inserting padding members named `_pad<N>`, or by explicit decorations (`Offset`, `ArrayStride`, `MatrixStride` in SPIR-V). A layout it cannot reproduce is refused (rule 3).
5. **Temporaries.** Expressions are inlined or baked per section 11.2. Baked temporaries are named from `named_expressions` when present, otherwise `_e<N>`.
6. **Names.** An identifier that is a target keyword, a reserved word, a built-in function name of the target, or that starts with a reserved prefix (`gl_`, `__`, `_ksir_`, `SV_`) is renamed by appending `_`, repeated until unique. Generated identifiers start with `_ksir_`. Renames are deterministic. Renames visible to a host (entry points; GLSL resource names) are recorded in reflection.
7. **Determinism.** Output is a pure function of the input module and options.
8. **Validity.** Output MUST be accepted by the target's reference validator (section 25.2). Output that a driver rejects is a backend bug, never a user error.

### 19.2 Summary

| Target | Default version | Output | Units | Entry names | Resources |
| --- | --- | --- | --- | --- | --- |
| `msl` | MSL 2.4 | Text | One module, every stage | As in KSIR | Entry point parameters with `[[buffer]]`, `[[texture]]`, `[[sampler]]` |
| `spirv` | SPIR-V 1.3 (Vulkan 1.1) | Binary words | One module per stage | As in KSIR | `DescriptorSet` / `Binding` decorations |
| `hlsl` | Shader model 6.0 (DXC) | Text | One per stage | As in KSIR | `register(bN)`, `tN`, `uN`, `sN`, space 0 |
| `glsl_430` | GLSL 4.30 core | Text | One per stage | `main`; the KSIR name is recorded in reflection | `layout(binding = N)` |
| `wgsl` | WGSL (W3C CR) | Text | One per stage | As in KSIR | `@group(G) @binding(B)` |
| `ksl` | KSL as the oracle accepts it | Text | One module, every stage | As in KSIR | `@binding(n)` written only for a resource whose authored universal slot differs from its position |

### 19.3 MSL

1. `#include <metal_stdlib>` and `using namespace metal;` head every module.
2. Entry points are `vertex`, `fragment` and `kernel` functions. Resources are entry point parameters: `constant T& name [[buffer(n)]]` for uniforms, `device T* name [[buffer(n)]]` for storage (with `const device` when `store` is absent), `textureXd<T, access::...> name [[texture(n)]]`, `sampler name [[sampler(n)]]`. Functions that use resources, private globals or workgroup globals receive them as hidden reference parameters, transitively (section 13.2).
3. A runtime-sized array's element count arrives in its own `constant uint& <name>_count [[buffer(16 + k)]]`, where `k` is the length index section 20.3 assigns and reflection reports. `array_length` reads that parameter. The host binds the number of elements, not bytes. The base 16 is the oracle's. Because `k` continues the resource buffer counter, `16 + k` is always above every assigned resource index; an authored slot that lands on a count buffer is `shader.slot-collision`, and a count buffer past Metal's last buffer index (30) is `shader.target-unsupported`.
4. The vertex output and fragment input interfaces are separate structs: the output with `[[position]]` and `[[user(locN)]]` members, the input taken as `[[stage_in]]` with the same `[[user(locN)]]` names. Interpolation: `[[flat]]`, `[[center_no_perspective]]`, `[[centroid_perspective]]`, `[[sample_perspective]]` and so on. Fragment outputs carry `[[color(N)]]`; `frag_depth` is `[[depth(any)]]`.
5. Workgroup globals are `threadgroup` variables declared in the kernel function and passed down. Zeroing (section 15.7) precedes a `threadgroup_barrier(mem_flags::mem_threadgroup)`.
6. Sampling: `level` maps to `.sample(s, c)` (`auto`), `level(0)` (`zero`), `level(e)`, `bias(e)`, `gradient2d(dx, dy)` / `gradientcube`. Depth compare: `.sample_compare`. Gather: `.gather(s, c, offset, component::x)`. `image_load` is `.read`; `image_store` is `.write(value, coord)`.
7. Overrides follow section 14.3. `kill` is `discard_fragment()`. Barriers: `threadgroup_barrier(mem_flags::...)`.
8. Versions: features requiring MSL above `--msl-version` are refused with the version they need.
9. With `--debug-info`, the backend SHOULD emit `static_assert(sizeof(T) == size)` and `static_assert(offsetof(T, m) == offset)` for every host-shareable struct, so the Metal compiler checks rule 19.1.4.

### 19.4 SPIR-V

1. One module per entry point. Header version per `--spirv-version`; capability `Shader` plus those section 16.6 requires; `GLSL.std.450` extended instructions; memory model `Logical GLSL450`.
2. The interface is module-scope `Input` and `Output` variables, one per interface leaf, decorated `Location` or `BuiltIn`. The entry function takes and returns nothing; it loads the inputs, composes the argument structs, runs the body, and stores the result members into the outputs. Fragment entries carry execution mode `OriginUpperLeft`; compute entries `LocalSize` (or `LocalSizeId` for kept overrides, SPIR-V 1.2 and later).
3. `position` is `BuiltIn Position` as a vertex output and `BuiltIn FragCoord` as a fragment input.
4. Locals are `Function` variables declared in the first block of their function. Structured control flow uses `OpSelectionMerge` and `OpLoopMerge` with explicit merge and continue blocks.
5. A host-shareable struct used in a buffer is emitted with `Offset` on every member, `ColMajor` and `MatrixStride` on matrices, `ArrayStride` on arrays, and `Block` on the buffer's outer struct. Because `Offset` is illegal on a struct in the `Function` storage class, a struct used in both buffer and function contexts is emitted twice: decorated and undecorated, with member-wise copies between them on `load` and `store`.
6. Sampling combines image and sampler with `OpSampledImage`; `auto` and `bias` use `OpImageSampleImplicitLod`, other levels `OpImageSampleExplicitLod`; depth compare uses the `Dref` forms. Storage images declare their format and `NonReadable`/`NonWritable` per access, and use `OpImageRead`/`OpImageWrite`.
7. `OpName` is emitted for every named entity. `OpLine` and `OpSource` are emitted with `--debug-info`.
8. Output MUST pass `spirv-val --target-env vulkan1.1` (or the environment matching `--spirv-version`).

### 19.5 HLSL

1. One unit per entry point, compiled by the host with DXC for shader model `--hlsl-model` (FXC accepts the output when the model is `5_0` or `5_1`).
2. Uniforms are `cbuffer <name>_buffer : register(bN) { T <name>; }`. Read-only storage is `StructuredBuffer<T>`, writable storage `RWStructuredBuffer<T>`; images `Texture2D<T>`, `TextureCube<T>`, `RWTexture2D<T>`; samplers `SamplerState` and `SamplerComparisonState`.
3. Matrix semantics are KSIR's (column-major, element `(c, r)` at `c * stride + r * size(scalar)`). The backend spells a KSIR `matCxR` as the HLSL type and operand order that realize them with a `column_major` qualifier, as the oracle does; the spelling is transposed, the matrix is not.
4. Built-ins map to system values (Appendix C). A `Location` binding is `TEXCOORD<n>` between stages and `SV_Target<n>` as a fragment output.
5. Integer edge cases are guarded (section 15.2) because HLSL leaves them undefined.

### 19.6 GLSL 4.30

1. `#version 430 core`. The entry function is `main`.
2. Uniform blocks are `layout(std140, binding = N) uniform <name>_block { ... } <name>;`; storage blocks `layout(std430, binding = N) buffer`. GLSL 4.30 has no member `offset` qualifier, so the backend realizes the module's offsets with padding members within the std140 or std430 rule; a layout those rules cannot reach (an offset smaller than the rule's next offset) is refused.
3. Images and samplers are combined: one `layout(binding = U) uniform sampler2D <texture>;` per sampled texture, bound to the sampler the pairing of section 20.4 gives. A texture sampled with two different samplers is refused, because one combined uniform carries one sampler state. Storage images are `layout(<format>, binding = N) uniform image2D` with `readonly`/`writeonly` per access.
4. The interface uses `layout(location = n) in` and `out` variables and `gl_*` built-ins. Fragment shaders redeclare `layout(origin_upper_left) in vec4 gl_FragCoord;` so `position` keeps KSIR's top-left origin.
5. `immediate` space and kept overrides are refused.

### 19.7 WGSL

1. One unit per entry point. Resources are `@group(G) @binding(B) var<space> name: T`.
2. WGSL host-shareable layout is fixed by the language (similar to `std430` with `vec3` aligned to 16, uniform arrays strided to 16). The backend realizes the module's offsets with `@size` and `@align` member attributes; layouts those cannot reach are refused.
3. Integer division, remainder, shifts and float-to-integer conversion already follow section 15.2; no guards are emitted. Workgroup memory is zero-initialized by WebGPU implementations; no zeroing is emitted.
4. `image_sample` with level `zero` is `textureSampleLevel(t, s, c, 0.0)` (`0` for depth); `auto` is `textureSample` and requires uniform control flow (section 16.5).
5. `f16` requires `enable f16;`. `immediate` space is refused.

---

## 20. Binding assignment

### 20.1 Principle

Binding assignment turns neutral bindings into target slots, once per shader per target, before the backend runs. Reflection and the backend read the same assignment. The algorithm is the oracle's (`kira-shader-ir/src/lower.rs`), so hosts that bind KSL shaders today keep their slots.

Resources are visited in group order, then in each group's `globals` order. Counters run over the whole shader. A resource with an authored slot for the target takes it and does not advance a counter; a resource with a `universal(n)` slot takes `n` on every target, with its group index unchanged, exactly as the oracle honors a written `@binding(n)` "on every target rather than only where the default happens to be positional".

### 20.2 Per target

| Target | Uniform | Storage | Sampled or storage image | Sampler | Reported group |
| --- | --- | --- | --- | --- | --- |
| `spirv`, `wgsl` | `(group, binding)` neutral | neutral | neutral | neutral | Neutral group |
| `msl` | Next buffer index, starting at 1 | Next buffer index | Next texture index, from 0 | Next sampler index, from 0 | 0 |
| `hlsl` | Next `b` register, from 0 | Next `t` register, from 0; a writable buffer is declared on `u` with that same index | Next `t` register (writable images on `u` with that index) | Next `s` register, from 0 | 0 (space 0) |
| `glsl_430` | Next block binding, from 0 (shared by uniform and storage) | Next block binding | Next texture unit, from 0 | No unit; binding 0, bound through the pairing of 20.4; reflected by name | 0 |

Metal buffer index 0 is reserved for the vertex attribute stream, which is why resource buffers start at 1. GLSL resources also carry their emitted name in reflection, because a GL host may look them up by name.

### 20.3 Metal length buffers

After every resource is assigned, each `storage` resource whose type contains a runtime-sized array, in resource order, takes the next value `k` of the Metal buffer counter as its length index. Placing them last means adding a runtime array never renumbers another resource. Reflection reports `k` as a `count <resource> msl <k>` record; the emitted parameter sits at buffer index `16 + k` (section 19.3 rule 3), which is where a host binds the element count. Both the counter continuation and the base 16 are the oracle's, and hosts rely on both.

### 20.4 Texture and sampler pairing (GLSL)

For each sampled image, its paired sampler is: the sampler of the first `image_sample` that names it, in entry point order and then statement order; otherwise the first sampler declared after it in its group; otherwise the first sampler of its group. A texture no stage samples still gets a pairing, because a host binds every resource a group declares. Reflection reports the pairing on every target.

### 20.5 Collisions

Two resources that land on one slot of one target are `shader.slot-collision`, reported against both spans. With assignment alone this cannot happen; it arises only from authored slots.

### 20.6 Metal argument indices

The `msl` slot of 20.2 is what reflection reports. Emitted MSL declares a resource at its **argument index** instead, because that is where hosts bind it: the oracle's MSL backend declares every resource at its WGSL binding (`kira-msl-backend`, `resource_param`), and DS-007 keeps those slots. The assignment gives each resource two:

| Stage | Uniform or storage | Texture or sampler |
| --- | --- | --- |
| Fragment, compute | WGSL binding | WGSL binding |
| Vertex | WGSL binding + 1 | WGSL binding |

A resource with an authored `msl.buffer(n)`, `msl.texture(n)` or `msl.sampler(n)` slot takes `n` in every stage, so `msl -> KSIR -> MSL` keeps every Metal index (section 23.6). The vertex row's `+ 1` steps over buffer 0, the vertex attribute stream. Length buffers stay at `16 + k` (20.3).

---

## 21. Reflection

### 21.1 Content

Reflection for one shader and one target holds: the shader name and kind; the target; every override (`option`) with type, default or baked value, id, and whether it is specializable on this target; every stage with its entry name (after renames), input and output types and workgroup size; every interface field with type, built-in, interpolation and location; every resource with group name, group class, neutral group index, name, kind, type, access, stage visibility (from `ModuleInfo`, section 16.3), target slot, Metal length slot, and paired sampler; and the layout of every host-shareable struct a resource names, with each member's offset, alignment, size and stride.

For a `uniform` resource, the reported layout's alignment is `max(16, align)` and its size `round_up(end of last member, that alignment)`, where `end` is the last member's offset plus its size. This is the binding size every target accepts for a uniform buffer. It reproduces the oracle's uniform layouts byte for byte (section 22.5).

### 21.2 `KSLR1`

The serialized form is the oracle's `KSLR1` text document, unchanged: UTF-8, one record per line, space-separated fields, `-` for an absent optional field, first line `KSLR1`, records `shader`, `option`, `stage`, `threads`, `in`, `out`, `resource`, `bind`, `layout`, `field`, `count`, with the field order and meanings of the oracle's encoder (`kira-shader-ir/src/reflection.rs`). The record set is append-only and decoders reject unknown keywords, as the format's own compatibility rule states. KLF appends:

| Record | Fields | When |
| --- | --- | --- |
| `spec` | `<option name> <id or -> <keep or bake>` | For every override, after its `option` record |
| `rename` | `<kind> <ksir name> <emitted name>` | For every host-visible rename (section 19.1 rule 6) |
| `source` | `<lang>` | Once, after `shader`; the producing definition (`ksl`, `msl`) |

A `KSLR1` document produced by KLF for a KSL shader, with the appended records removed, MUST equal the oracle's document for the same shader and target (section 25.3).

### 21.3 `.resources` digest

The compact digest a graphics host parses at startup (`<Shader>.resources`) keeps the oracle's format exactly: one record per resource, `u|`, storage, texture and sampler (`m`) records, fields split on `:`, the WGSL binding index and the group last. It MUST be byte-identical to the oracle's for every KSL shader in the corpus.

---

## 22. Language profile: KSL

### 22.1 Definition constants

`name = "ksl"`, `driver = "ksl"`, `extensions = ["ksl"]`, `layouts` all `Explicit`, every target, one unit per file, `libraryRoot = "library"` (shared KSL modules shipped with the toolchain). Imports resolve as the oracle resolves them: `import A.B` names `A/B.ksl` relative to the importing file's directory, then under the library root, each path segment matched ignoring ASCII case.

### 22.2 The language

KSL is the language the oracle accepts, as documented in the `kira` docs site (`appendix/shaders`, `macros/shaders`) and exercised by its tests and the ecosystem corpus. In summary: files hold `import`, `type`, `enum`, `const` and `function` declarations and at most one `shader`; a shader holds `option`s, `group`s of `uniform`, `storage read`, `storage read_write`, `texture`, `texture read_write` and `sampler` resources, and `vertex`, `fragment` or `compute` stage blocks with `input`, `output`, `threads` and one `entry` function plus helpers. Types are `Bool`, `Int`, `UInt`, `Float`, `Float2` to `Float4`, `Int2` to `Int4`, `UInt2` to `UInt4`, `Float2x2` to `Float4x4`, `Texture2d`, `Texture2dUint`, `TextureCube`, `Depth2d`, `Sampler`, `SamplerComparison` (the oracle's spellings, which reflection records too), runtime arrays `[T]` in storage, and declared `type`s. Conversions are explicit constructor calls; integer literals have no default type; `*` never means matrix multiplication; `mul(a, b)` does.

The `ksl` definition lives in `klf-kira` under `languages/ksl/`, beside Kira's own (section 4.3), and owns the KSL reference documentation.

### 22.3 Mapping to KSIR

| KSL | KSIR |
| --- | --- |
| `Bool`, `Int`, `UInt`, `Float` | `bool`, `i32`, `u32`, `f32` |
| `FloatN`, `IntN`, `UIntN` | `vecN<f32>`, `vecN<i32>`, `vecN<u32>` |
| `FloatCxR` | `matCxR<f32>`, column-major |
| `type T { ... }` | One struct type `%T`. When reached from a uniform or storage resource it is `explicit` with KSL packing (22.5); otherwise `none`. Field annotations `@builtin(b)` and `@interpolate(flat or linear)` become member bindings; members without a built-in get `@location(auto)` |
| `enum E { ... }` | `i32` constants, one per variant, named `E_Variant`, folded as the oracle folds them (the oracle's enums are `Int`) |
| `const name: T = literal` | `const` with a constant expression |
| `option name: T = value` | `override` with default `value` and its position among the shader's options as id; listed in the shader's `options`. Baked by default (section 17.3), which is the oracle's folding |
| `group G { ... }` | `GroupDecl` with class from the group name (`Frame`, `Pass`, `Material`, `Object`, `Draw`, `Dispatch`, case-insensitive; otherwise `Custom`) |
| `uniform name: T` | Global, space `uniform`, type `%T` |
| `storage read name: [T]` | Global, space `storage<load>`, type `array<%T, runtime, stride S>` with `S` the KSL storage stride |
| `storage read_write name: [T]` | Global, space `storage<load, store>` (with `atomic` when an `atomicAdd` targets it) |
| `texture name: Texture2d` | Global, space `handle`, `image<2d, sampled<float>>` |
| `texture name: DepthTexture2d` | `image<2d, depth>` |
| `texture name: TextureCube` | `image<cube, sampled<float>>` |
| `texture read_write name: Texture2d` | `image<2d, storage<rgba8unorm, load store>>`; `storage<rgba8unorm, store>` when never loaded |
| `texture read_write name: Texture2dUint` | `image<2d, storage<r32uint, ...>>` |
| `sampler name: Sampler` / `ComparisonSampler` | `sampler` / `sampler<comparison>` |
| `@binding(n)` on a resource | Authored slot `universal(n)` |
| `vertex { input I output O function entry(...) }` | Entry point `vertex_main`, stage `vertex`, argument of type `%I`, result `%O` |
| `fragment { ... }` | Entry point `fragment_main` |
| `compute { input I threads(x, y, z) ... }` | Entry point `compute_main`, workgroup size `(x, y, z)`; an extent naming an option is an override expression |
| Helper function in a stage, top-level function | Function in the arena, qualified name for imports (`GlassCommon_specular`) |
| `let x = e`, `let x: T`, `let x: T = e` | Local `$x` of the checked type, `init` absent; a `store` of `e` at the `let`'s position. KSL rebinds `let`, so every `let` is a variable |
| `x = e`, `a.b = e`, `a.b.c[i] = e` | `store` through the place's pointer (`local`, `access_index`, `access`) |
| `if` / `else if` / `else` | Nested `if` |
| `while c { body }` | `loop` per section 12.2 rule 5 |
| `return [e]` | `return` |
| A name of a local or parameter | `load` of `local`, or `arg` |
| A resource name | `global`, then `load` for uniform and storage values |
| `a.field` | `access_index` by member index |
| `v.xyz` and other swizzles | `swizzle`; a one-component swizzle is `access_index` |
| `a[i]` | `access` |
| `array.count` | `array_length` on the storage global's pointer |
| `T(args)` constructing a vector, matrix or struct | `compose` (a single scalar argument to a vector type is `splat`) |
| `T(x)` converting a scalar or vector | `convert` |
| Operators | `unary` and `binary`; KSL's operand rules make every operation same-typed |
| Literals | `literal` at the type the checker resolved |

### 22.4 KSL built-in functions

| KSL | KSIR |
| --- | --- |
| `mul(a, b)` | `binary multiply a, b` (linear algebra, column-major) |
| `dot`, `cross`, `normalize`, `length` | `math` of the same name |
| `abs`, `floor`, `ceil`, `fract`, `sqrt`, `pow`, `exp`, `exp2`, `log`, `log2`, `sin`, `cos`, `tan`, `atan2` | `math` of the same name |
| `min`, `max`, `clamp`, `mix`, `step`, `smoothstep` | `math` of the same name |
| `sample(t, s, uv)` | `image_sample` with level `zero` in every stage (DS-009) |
| `load(t, coord)` | `image_load`; level 0 for a sampled image, none for a storage image |
| `store(t, coord, value)` | `image_store` |
| `atomicAdd(buffer, index, value)` | `atomic add` on `access(global buffer, index)` with result |

### 22.5 KSL packing

KSL lays out host-shareable structs by its own rule, which is the oracle's and which hosts pack against today. It is neither `std140` nor `std430`: a scalar after a `Float3` lands at offset 16, not 12. The definition computes it and declares structs with `structTypeExplicit`.

For each member in declaration order, with `(size, align)`:

| Member type | Size | Align |
| --- | --- | --- |
| Scalar | 4 | 4 |
| 2-wide vector | 8 | 8 |
| 3- or 4-wide vector | 16 | 16 |
| `FloatCx_` matrix | 16 x C | 16 |
| Nested struct `S` | `S`'s uniform size: `round_up(end(S), max(16, align(S)))` | `max(16, align(S))` |

`offset = round_up(previous end, align)`; `end = last offset + last size`.

The struct's KSIR alignment is its **storage alignment**: the maximum over members of 4, 8 or 16 for scalars, 2-wide and wider vectors and matrices, and of a nested struct's storage alignment. Its KSIR size is `round_up(end, storage alignment)`. That size is the stride of a storage array of it. Reflection rounds it up for uniform bindings (section 21.1).

Worked values, from the oracle's tests: a struct of 70 `Float`s has storage stride 280 and uniform size 288; `{ Float4, Float }` has storage stride 32; `{ Float3, Float }` places the `Float` at 16 and has uniform size 32.

A later KSL revision may move to `std140`/`std430` exactly; that would be a KSL language change (an edit to the definition and to the hosts' packing), not a kernel change.

### 22.6 Locations and entry names

Interface locations are assigned by section 10.5 rule 4 (declaration order, skipping built-ins), which is the oracle's rule. Entry names are `vertex_main`, `fragment_main` and `compute_main`.

### 22.7 Checks that stay in the definition

The definition keeps every check the oracle performs before lowering, with the oracle's codes: unknown types and names, duplicates, type mismatches, argument counts, members, built-in legality, assignability, stage shape, options, imports, resources, shader count, operators, missing returns, constants, binding collisions (`KSLP001` to `KSLP009`, `KSLS001` to `KSLS018`; `KSLS016` stays reserved for "a target produced nothing"). Compute stages that declare an `output` are rejected with `KSLS009` (DS-010).

### 22.8 Kernel diagnostic keys

| Key | KSL code |
| --- | --- |
| `shader.uniformity` | `KSLS019` |
| `shader.const-not-finite` | `KSLS017` |
| `shader.const-index` | `KSLS020` |
| `shader.interface-mismatch` | `KSLS009` |
| `shader.override-missing` | `KSLS010` |
| `shader.override-unsupported` | `KSLS021` |
| `shader.target-unsupported` | `KSLS016` |
| `shader.capability` | `KSLS016` |
| `shader.slot-collision` | `KSLS018` |

---

### 22.9 The KSL out

The `ksl` definition's out (section 5.8) writes one `.ksl` file per unit: the module's structs, constants and helper functions, then one `shader` holding its options, its groups and one stage block per entry point. `ksl` is a target (`--target ksl`) every toolchain accepts, never a default one. Its passes keep overrides, which become `option`s, and add no bounds checks, integer guards or workgroup zeroing, so that the file is the source the unit came from and not a lowered form of it.

A KSL build of the file gives a module that behaves as the unit does. Names of temporaries, locals and helpers and spans may differ, and so may the statements:

1. An expression the D-343 rule bakes is bound with `let _tN = ...`. A `select` becomes an `if` assigning `_tN`. A struct, matrix or array zero is a `let _tN: T` nothing writes.
2. Every local is declared at the top of its function, so that each assignment writes the one variable.
3. A loop whose body starts with the emits of a condition and `if c { break }`, never leaves elsewhere, and whose emits feed only `c`, is `while !c { ... }`: the form KSL lowers its own `while` to. Any other loop runs `while _loopN`. `break` clears that flag, `continue` sets `_skipN`, the `continuing` block and `break_if` run under `if _loopN`, and the statements after one that may leave are guarded by the flags of every loop and switch open around them.
4. A `switch` binds its selector to `_selectorN` and becomes a chain of `if`s; cases that fall through with an empty body join the next case's test with `||`. A `break` that is not the last statement of its case clears `_switchN`, which guards the rest of the case.
5. An entry whose arguments are not one struct takes a `_<Stage>Input` struct the out declares, holding the arguments that are not structs and the members of those that are, built-ins first and then by location. The entry's first statements copy it back into one variable per argument. A result that is not a struct is the `value` of a `_<Stage>Output` struct.
6. A uniform that is not a struct is the `value` of a struct the out declares.
7. A function that reaches a resource is written inside each stage block whose entry reaches it, where the shader's resources are in scope; the others are written once, before the shader.
8. An option takes the id of its position among the shader's options (D-339), whatever id the unit gave it.
9. An out name starts with `_`. A source name that starts with `_` is written with a `v` before it, one that is a KSL keyword with `_` after it, and a byte an identifier cannot hold is written as `_`.

The out refuses, as `shader.target-unsupported` naming the construct, what KSL cannot spell: among others pointers passed to functions, atomics other than `atomicAdd` on a storage array element, barriers, `kill`, workgroup variables, fixed-size arrays, storage images with a format KSL lacks, `f16` and 64-bit scalars, samples at a level other than zero or with an offset, a comparison or a gather, struct packing other than KSL's for a struct a host writes, options without a default, and input locations with a gap. Section 22.2 is the list of what KSL has; everything else is refused.

### 22.10 The text library

The MSL, HLSL, GLSL and WGSL outs share one writer, `klf-kira/languages/text`. It holds the reader calls, naming by section 19.1 rule 6, struct padding to a recorded layout, the text of constant values, the interface leaves of an entry, and the body writer with the temporary rule of D-343.

A package takes the library in through an `include` file at its root. Each line names a directory relative to the root (`#` starts a comment), and that directory's `.klfd` sources join the package. All of them share one namespace, so a package does not reuse a name the library defines.

The package defines `struct Dialect`, its own state, and in `extend TextOut` the functions the body writer calls where its target differs: `dialectExpression`, `dialectTemporary`, `dialectStatement`, `dialectNativeLoop`, `dialectCaseBreak`, `dialectCaseLabel`, `dialectKeepsArgument`, `dialectArgument`, `dialectCallArguments`, `dialectBakeAlways` and `dialectLineDirective`, with `dialectLiteral` and `dialectCompose` for constant values.

A type's text that the target cannot spell is carried as text starting with `!` and the reason. Where the module declares something of the type, the out refuses at the type; inside a body it writes the reason as a comment, as the unit is refused by then anyway.

The `hlsl`, `glsl` and `wgsl` packages hold only their outs until S12: their phases refuse every source with code 1.

## 23. Language profile: MSL

### 23.1 Position

The MSL definition is the second shader frontend and the proof that KSIR is frontend-neutral. Its purpose is to take Metal shaders that exist (engine shaders, ports, examples) to SPIR-V, HLSL, GLSL and WGSL, and to MSL again with a validated, normalized form. It accepts a defined subset of MSL and refuses the rest with diagnostics naming the construct; it never guesses.

The definition lives with the other shader languages in `klf-kira/languages/msl/`. `knvm binstall` builds the `kmsl` driver and `klf test languages/msl` runs its corpus (section 25.3).

### 23.2 Definition constants

`name = "msl"`, `driver = "kmsl"`, `extensions = ["metal"]`, `layouts` all `Msl`, every target, one unit per file, `libraryRoot = ""`, `capabilities` `float16`, `cube-array`, `multisampled`, `sample-shading`, `clip-distances` and `storage-image-read-write`. Diagnostic family `KMSL-E<NNNN>` and `KMSL-W<NNNN>`: the kernel's diagnostic channel carries one integer, so error N is N and warning N is 5000 + N. Definition codes are 1 to 11 (preprocessing and parsing), 101 to 126 (outside the subset) and 201 to 220 (checking); the kernel keys map to 230 to 237 and, for `shader.uniformity`, 5001. `klf-kira/languages/msl/docs/diagnostics.md` lists them all.

### 23.3 Accepted subset

| Area | Accepted |
| --- | --- |
| Preprocessing | `#include <metal_stdlib>` and `<simd/simd.h>` (recognized, not read); `#include "file"` of files in the subset; object-like `#define` of literals; `#if`, `#ifdef`, `#ifndef`, `#else`, `#endif` over defined names and integer expressions; `#pragma once`. The definition carries its own preprocessor |
| Namespaces | `using namespace metal;`; `metal::` qualification; user `namespace` blocks (names flattened with `_`) |
| Types | `bool`, `int`, `uint`, `float`, `half` (capability `float16`), `short`/`ushort` only as stage-in attribute types widened to 32-bit, vector types `floatN`, `halfN`, `intN`, `uintN`, `boolN`, `packed_floatN`, matrices `floatCxR`, `halfCxR`, fixed-size C arrays and `array<T, N>`, structs without methods, base classes or bit-fields, `atomic_int`, `atomic_uint` |
| Resources | `texture1d`, `texture2d`, `texture3d`, `texturecube`, `texture2d_array`, `texturecube_array`, `depth2d`, `depth2d_array`, `depthcube`, `texture2d_ms`, `depth2d_ms` with `access::sample`, `read`, `write`, `read_write`; `sampler` |
| Entry points | `vertex`, `fragment` and `kernel` functions with `[[buffer(n)]]`, `[[texture(n)]]`, `[[sampler(n)]]`, `[[stage_in]]`, the built-in attributes of Appendix C, `[[threads_per_threadgroup]]` (read from the workgroup-size overrides of section 23.5), `[[early_fragment_tests]]`; `[[max_total_threads_per_threadgroup]]` is accepted and changes nothing |
| Buffers | `constant T& name [[buffer(n)]]`, `constant T* name [[buffer(n)]]` (as a runtime array), `device T& name`, `device T* name`, `const device T*`; indexing them with `[]` |
| Interface | Stage-in structs with `[[attribute(n)]]` (vertex) or `[[user(name)]]`, `[[flat]]`, `[[center_perspective]]`, `[[center_no_perspective]]`, `[[centroid_*]]`, `[[sample_*]]`; return structs with `[[position]]`, `[[user(name)]]`, `[[color(n)]]`, `[[depth(any)]]`, `[[sample_mask]]`, `[[clip_distance]]`. A member of a vertex output or fragment input with no attribute passes between the stages under its name, as Apple's samples write it |
| Functions | Free functions, `inline`, `static`; parameters by value, `thread T&` and `threadgroup T&` references; no recursion |
| Templates | Function templates whose arguments are all deducible at each call, instantiated per distinct argument list through the instantiation service; no class templates, specializations or SFINAE |
| Statements | Declarations, expressions, `if`, `switch` (only a case with an empty body may fall through; any other is `KMSL-E0123`), `for`, `while`, `do`, `break`, `continue`, `return`, `discard_fragment()`; `threadgroup` variables declared in a kernel's body |
| Expressions | Arithmetic, comparison, logical (short-circuit lowered to `if`), bitwise, `?:`, `++`/`--`, compound assignment, swizzles, constructors, `static_cast` and C-style casts between numeric types, `as_type<T>` (`bitcast`) |
| Library | The `metal_stdlib` functions that have a KSIR counterpart (sections 11.5 and 12): math, geometric, relational, `select`, `dfdx`/`dfdy`/`fwidth`, texture `sample`, `sample_compare`, `gather`, `read`, `write`, `get_width`/`get_height`/`get_num_mip_levels`, `atomic_load_explicit`, `atomic_store_explicit`, `atomic_exchange_explicit`, `atomic_compare_exchange_weak_explicit` and `atomic_fetch_*_explicit` with `memory_order_relaxed`, `threadgroup_barrier`. A function with no KSIR counterpart is `KMSL-E0121` |
| Constants | `constant` program-scope variables with constant initializers; `constexpr` variables; `constant T name [[function_constant(n)]]` (override with id `n`, no default, `n` below 65533) |

### 23.4 Refused

Pointer arithmetic and pointer comparison; pointers stored in variables or struct members; `device` or `constant` pointers passed to non-entry functions unless the argument is an entry's resource or a parameter that is one (the function is then lowered once per resource it is called with, and reads the global); a function constant with a default value, or `is_function_constant_defined`; argument buffers and `[[id(n)]]`; `visible_function_table`, `intersection_function_table` and ray tracing; mesh and object shaders; `imageblock`, tile functions and raster order groups; `simdgroup` matrix and `simd_*` functions; `[[function_constant]]` used to gate the existence of a parameter; class templates; lambdas; `constexpr sampler` (refused in 1.0, DS-012); `printf`; recursion. Each has a `KMSL-E` code naming the construct.

### 23.5 Mapping

| MSL | KSIR |
| --- | --- |
| `[[buffer(n)]]` parameter `constant T&` | Global `uniform` of `T`; authored slot `msl.buffer(n)` |
| `device T*`, `const device T*` | Global `storage<...>` of `array<T, runtime, stride sizeof(T)>`, access from the qualifier and from whether it is written |
| `device T&` | Global `storage<...>` of `T` |
| `[[texture(n)]]`, `[[sampler(n)]]` | Global `handle`; authored slots `msl.texture(n)`, `msl.sampler(n)` |
| `threadgroup T x;` in a kernel | Global `workgroup` |
| `[[stage_in]]` struct | Entry argument struct; `[[attribute(n)]]` becomes `@location(n)`; `[[user(name)]]` members get locations in declaration order unless the name is `locN`, which gives `N` |
| `[[color(n)]]` | `@location(n)` |
| `[[function_constant(n)]]` | Override with id `n` and no default: a build supplies it with `--override name=value` (else `KMSL-E0231`) or keeps it with `--overrides keep` |
| A kernel's workgroup size | Three `u32` overrides `workgroup_size_x`, `_y`, `_z`, ids 65533 to 65535, default 1, which size the entry's workgroup; `[[threads_per_threadgroup]]` is the vector of the three. Metal sizes a dispatch at encode time; the overrides are where the other targets take it |
| Program-scope `constant` | `const`; one whose value reads a function constant is not declared but computed where it is read, as KSIR has no override without an id that a target can keep |
| `access::sample` and `access::read` textures | `sampled` images; a `read` passes level 0 |
| `access::write` and `access::read_write` textures | Storage images of `rgba8unorm` (`float`, `half`), `r32uint` or `r32sint` by texel type |
| `[[user(name)]]` and unattributed interface members | `@location`: `locN` gives `N`; any other name takes the lowest location no `locN` holds, in the order names first appear, so a vertex output and a fragment input of one name meet. Integer members are `@interpolate(flat)` |
| A `device` or `constant` pointer or reference parameter of a helper | No parameter: the helper is lowered once for each list of resources its calls pass, and reads them as globals |
| `thread T&` parameter | `ptr<function, T>`; an argument that is part of a variable goes through a local written back after the call |
| `threadgroup T&` parameter | `ptr<workgroup, T>` |
| Function template | One function per distinct deduced argument list, made only for the candidate overload resolution picks |
| `half` arithmetic | `f16` |
| Metal natural layout | Rule `msl`; `packed_floatN` members are `vecN<f32>` with size `4N` and align 4 inside an explicit struct |

### 23.6 Bindings from Metal slots

MSL has no groups. The frontend puts every resource of a shader in one group named `Default`, class `Custom`. Neutral bindings within it are assigned in this order: buffers by ascending `[[buffer]]` index, then textures by ascending `[[texture]]` index, then samplers by ascending `[[sampler]]` index. Every resource also records its authored Metal slot, so `msl -> KSIR -> MSL` keeps every Metal index. SPIR-V and WGSL output use set 0 and the neutral binding; reflection gives the host the mapping from Metal slot to `(set, binding)`.

A shader translation unit is one file. Its entry points form one shader named after the file stem; a file with both a `kernel` and graphics entries is refused (`KMSL-E0101`), as a KSIR shader is graphics or compute.

### 23.7 Semantic differences the frontend resolves

1. **Implicit conversions.** MSL converts implicitly between numeric types as C++ does. The frontend makes each conversion an explicit `convert` per C++'s usual arithmetic conversions, with MSL's rules for vectors: a scalar beside a vector is splat; a vector never converts to another vector implicitly; a literal is folded to the other operand's type; a float scalar that is not a literal beside an integer vector is refused. Overload resolution takes the candidate needing the fewest conversions, a non-template winning a tie.
2. **Integer edge cases.** MSL's integer division by zero is undefined; KSIR's is defined. A translated shader therefore gains defined behavior; this is a refinement and is allowed.
3. **Implicit-LOD sampling in divergent control flow.** Accepted by Metal; reported as a `shader.uniformity` warning, or an error when `wgsl` is requested.
4. **Fast math.** Metal's default fast math is the host's setting; KSIR promises nothing stronger (section 15.3).
5. **Rounding.** Metal's `round` rounds halves away from zero and KSIR's to even; the frontend writes `sign(x) * floor(abs(x) + 0.5)`. `rint` is KSIR `round`, `fmod` is KSIR `modulo` on floats.
6. **Atomics.** `atomic_load` is a fetch-or with 0, `atomic_store` an exchange whose result is dropped, and `atomic_compare_exchange_weak` writes the value found back to `expected` and returns whether it exchanged.
7. **Short-circuit and `?:`.** Lowered to `if` only when the side that may not run has effects; otherwise `logical_and`/`logical_or` and `select`.
8. **Missing return.** A non-`void` function whose end can be reached is `KMSL-E0214`, where C++ allows it.

### 23.8 Oracle

For acceptance: Apple's `metal` compiler (`xcrun -sdk macosx metal -std=metal3.0`), on macOS hosts. A program the definition accepts MUST be accepted by `metal`. For behavior: the compute parity suite (section 25.4) runs each corpus kernel on Metal from its source and on Vulkan from the translated SPIR-V and compares the output buffers.

---

## 24. Integration with hosted languages

### 24.1 The `ksl!` macro

Kira's `ksl!` is an ordinary `comptime macro` over `Ksl.compile(path, target)` (Kira docs, "The Shader Macro"). Kira's definition answers that call from files the shader toolchain wrote, not by running the shader pipeline itself: a Kira toolchain and a KSL toolchain are separate (section 4.3), and user code at compile time runs in a sandbox with no I/O (`KLF-RIDL.md` 31.5). `klf-kira` does this in `definitions/syntax/ksl-artifacts.klfd`: the definition (not the sandboxed macro) runs `ksl build <shader> --out generated/shaders` through the process services, then reads `generated/shaders/<Stem>.<suffix>` beside the package. A shader `ksl` rejects is `KLFK-E4038`, with `ksl`'s own diagnostics on the error output. When no `ksl` is on the path, what an earlier build left in `generated/shaders` is read; when nothing is there either, the call is `KLFK-E4039`. An empty shader is never synthesized.

### 24.2 The file contract

The `ksl` driver's `build` writes, for every shader `<Stem>` under the package's `Shaders/` directory, into `generated/shaders/`:

| File | Content |
| --- | --- |
| `<Stem>.metal` | MSL, every stage |
| `<Stem>.vert.wgsl`, `.frag.wgsl`, `.comp.wgsl` | WGSL per stage |
| `<Stem>.vert.glsl`, `.frag.glsl`, `.comp.glsl` | GLSL per stage |
| `<Stem>.vert.hlsl`, `.frag.hlsl`, `.comp.hlsl` | HLSL per stage |
| `<Stem>.vert.spv`, `.frag.spv`, `.comp.spv` | SPIR-V words as lowercase hexadecimal, eight characters per word, beginning `07230203` |
| `<Stem>.resources` | The digest of section 21.3 |

A stage a shader does not have produces no file; `klf-kira` reads an absent file as empty. The names, the hexadecimal SPIR-V text and the digest are the contract `klf-kira`, `ember`, `ui-foundation` and `kcoreanimation` read today. Changing any of them is a coordinated change to this section and to those readers.

### 24.3 Build ordering and caching

A host language build runs the shader driver before its own `parse`, over the shader paths its sources name (the oracle scans `name!("....ksl")` call sites; `klf-kira` reads whatever the shader driver wrote). The shader driver's outputs are inputs of the host build's queries (`KLF-RIDL.md` 8.9 `input`), so an edit to a `.ksl` file re-runs exactly the macro expansions that read its outputs.

---

## 25. Conformance and diagnostics

### 25.1 Kernel suites

| Suite | Content |
| --- | --- |
| KSIR text | Round trip of every `.ksir` fixture; printer canonical form |
| Validator | One positive and one negative `.ksir` fixture per rule SV1 to SV24 |
| Layout | Tables of expected size, alignment, offsets and strides for every type under each rule of section 8.4 |
| Passes | Golden before-and-after `.ksir` pairs for every mandatory pass |
| Binding assignment | Golden assignments for each target over fixtures covering every row of section 20.2, length buffers, pairing and authored slots |
| Backends | Every fixture emitted for every target and checked by the reference validators of 25.2 |
| Reflection | Golden `KSLR1` and `.resources` per fixture and target |
| Constant evaluation | Edge cases of section 15.2 and 15.5 evaluated at compile time, compared with the runtime results of 25.4 |

### 25.2 Reference validators

| Target | Validator | Required |
| --- | --- | --- |
| SPIR-V | `spirv-val` (SPIRV-Tools), environment per `--spirv-version` | Always |
| MSL | `xcrun -sdk macosx metal -c` | On macOS CI hosts |
| HLSL | `dxc -T <stage>_<model>` | On hosts with DXC |
| GLSL | `glslangValidator` with `--target-env opengl` | Always |
| WGSL | naga's WGSL frontend and validator, and Tint where available | Always |

A backend change is not complete until every fixture passes every validator available on the CI host matrix, and the matrix as a whole covers every validator.

### 25.3 The KSL corpus and oracle

| Corpus | Source |
| --- | --- |
| Oracle unit fixtures | The `.ksl` programs embedded in the tests of `kira-ksl-*`, `kira-shader-ir` and `kira-*-backend` |
| Ecosystem shaders | Every `.ksl` under `ember` (`Shaders/`, `examples/*/Shaders/`), `ui-foundation/Shaders`, `kcoreanimation/Shaders` (38 files at the date of this document) |

For each corpus shader and each target the conformance runner compares:

1. Accept or reject, and on reject the diagnostic code and line (the `KLF-RIDL.md` 31.1 corpus form).
2. `KSLR1`, with KLF's appended records removed: byte-identical.
3. `.resources`: byte-identical.
4. The emitted output: passes the reference validator. Byte identity with the oracle's output is not required.
5. Behavior: the render and compute parity of 25.4 where a GPU is available.

`klf test <definition>` runs the manifests under the package's `corpus/` for a shader definition too: each case is `<driver> build <sources> <arguments> --out <dir>`, and only `accept` and `diagnostics` are compared, as shader programs do not run on the host. A diagnostic with no source position (a target refusing a whole module) is recorded as `<code>:0`. The `msl` definition's corpus uses this form.

Known, intended differences from the oracle are listed in Appendix D (DS-009, DS-010, DS-011, DS-013, DS-016) and are excluded from comparison item by item, never wholesale.

### 25.4 Parity on hardware

- **Compute parity.** Each compute fixture runs on every API available on the host (Metal from MSL, Vulkan from SPIR-V, D3D12 from HLSL, OpenGL from GLSL, WebGPU from WGSL through Dawn or wgpu), over the same input buffers; output buffers MUST be identical for integer results and within 1 ULP per operation chain for float results.
- **Render parity.** Each graphics fixture renders a fixed scene per API; images MUST match within a per-channel tolerance of 1/255 outside rasterization-rule edges.

### 25.5 Diagnostics

Kernel codes add the area `SHD`: `KLF-SHD-<NNNN>`. `0xxx` are user-facing kernel diagnostics that have no definition key (command-line and input errors of the shader driver). `9xxx` are internal errors that blame a definition (invalid module, builder misuse), as `KLF-KIR-9xxx`. Keys of section 16.4 are mapped by definitions. Rendering and `--diagnostics=json` follow `KLF-RIDL.md` 31.4.

### 25.6 Trust

Shader definitions are trusted, as all definitions are. Shader source is untrusted input: the driver never executes it, and constant evaluation is bounded (`EvalLimits` defaults of `KLF-RIDL.md` 8.10 apply to the number of evaluated expressions).

---

## 26. Implementation plan and crate layout

### 26.1 Crates

| Layer (`KLF-RIDL.md` 32.1) | Crate | Content |
| --- | --- | --- |
| 1 | `klf-shader` | KSIR data model, arenas, handles, layout rules, typifier, text printer and parser, binary cache format |
| 2 | `klf-shader-valid` | Validator, `ModuleInfo`, uniformity, constant evaluator |
| 2 | `klf-shader-passes` | Normalization, stage selection, overrides, bounds, integer edge cases, initial values, compaction, folding |
| 3 | `klf-shader-bind` | Binding assignment (section 20) |
| 3 | `klf-shader-reflect` | Reflection, `KSLR1` encoder and decoder, `.resources` digest |
| 3 | `klf-shader-spirv` | The SPIR-V writer |
| 3 | `klf-shader-out` | The reader and writer services of section 6.7 over a pass-processed unit |
| Definitions | `klf-kira/languages/msl`, `hlsl`, `glsl`, `wgsl`, `ksl` | Every shader language, in KLF Kira (section 4.3) |
| 5 | `klf-services` (module `shader`) | Shader builder services, `ShaderLanguage` family support |
| 6 | `klf-driver` (module `shader`) | Shader driver loop, outputs |
| 7 | `klf-cli` | `klf ksir verify`, `klf ksir dump`, `klf build` for shader definitions |
| Test only | `klf-shader-oracle` | Conversion of KSIR to `naga::Module` for differential validation (DS-005); runs the oracle `kira` binary for corpus comparison. Never a dependency of a kernel crate |

No crate depends on a higher layer. `klf-shader` through `klf-shader-spirv` build for `wasm32-unknown-unknown`.

Every text target is written only by its out. SPIR-V, a binary format, is written by `klf-shader-spirv`. The text outs share `klf-kira/languages/text`, which each takes in through its `include` file (section 22.10).

### 26.2 Milestones

| Milestone | Content | Exit test |
| --- | --- | --- |
| S0 | `klf-shader`: data model, types, layout rules, text format | Text round trip and layout suites |
| S1 | `klf-shader-valid`: SV1 to SV24, `ModuleInfo`, uniformity, constant evaluation | Validator suite |
| S2 | Passes, binding assignment, reflection, and the SPIR-V and MSL backends over hand-written `.ksir` | Pass, binding and reflection suites; `spirv-val` and `metal` on every fixture |
| S3 | `ShaderLanguage` family, shader builder services, shader driver loop; the `ksl` definition | KSL corpus items 1 to 3 of 25.3 against the oracle |
| S4 | HLSL, GLSL and WGSL backends | KSL corpus item 4 on every target |
| S5 | `klf-kira` reads `ksl` output; the oracle's `kira shader build` is no longer needed by any package | Ecosystem packages build and run with shaders from `ksl` only |
| S6 | `kmsl`: MSL definition | MSL corpus accepted by `metal` and by `kmsl`; translated SPIR-V passes `spirv-val`; compute parity Metal against Vulkan |
| S7 | `--overrides keep`, `--bounds`, debug info, capabilities beyond 1.0 KSL | Override, bounds and capability fixtures on every target |
| S8 | Hardware parity suites in CI | 25.4 on the host matrix |
| S9 | Outs: the `emit` member, the reader and writer services, the linked toolchain binary, the shader languages moved to `klf-kira/languages/`, and the `ksl` out | Every corpus shader, from KSL and from MSL, converts to KSL, and the KSL builds again to a module that validates |
| S10 | The `msl`, `hlsl`, `glsl` and `wgsl` outs, ported from the Rust backends, which are then removed | Each out's output equals its Rust backend's, byte for byte, on the corpus and every fixture; the reference validators accept it |
| S11 | Every shader language's toolchain links the other shader languages | Six drivers, each converting between every pair of languages |
| S12 | The `hlsl`, `glsl` and `wgsl` ins | Every corpus shader written by an out reads back through the matching in |

### 26.3 Working rules

As `KLF-RIDL.md` 32.3. A change to KSIR's model updates this document, the text grammar, the validator fixtures and Appendix B in the same commit.

---

## 27. Appendix A: grammar of textual KSIR

Notation as `KLF-RIDL.md` Appendix A. Comments are `//` to end of line.

```
file        = "ksir" integer module { item }
module      = "module" "@" ident "lang" string "capabilities" "(" [ ident { "," ident } ] ")"
item        = typedecl | gexpr | constdecl | overridedecl | globaldecl | shaderdecl | func | entry

typedecl    = "type" "%" ident "=" "struct" rule [ "size" integer "align" integer ] "{" [ member { "," member } ] "}" [ loc ]
rule        = "std140" | "std430" | "scalar" | "msl" | "explicit" | "none"
member      = ident ":" type [ "offset" integer "size" integer "align" integer ] [ binding ]
type        = scalar | "vec" digit "<" scalar ">" | "mat" digit "x" digit "<" scalar ">"
            | "atomic" "<" scalar ">" | "array" "<" type "," arraysize "," "stride" integer ">"
            | "%" ident | "ptr" "<" space "," type ">" | image | "sampler" [ "<" "comparison" ">" ]
scalar      = "bool" | "i32" | "u32" | "f32" | "f16" | "i64" | "u64" | "f64"
arraysize   = integer | "@" ident | "runtime"
image       = "image" "<" dim [ "," "arrayed" ] "," iclass ">"
dim         = "1d" | "2d" | "3d" | "cube"
iclass      = "sampled" "<" ( "float" | "sint" | "uint" ) [ "," "multisampled" ] ">"
            | "depth" [ "<" "multisampled" ">" ]
            | "storage" "<" ident "," access ">"
space       = "function" | "private" | "workgroup" | "uniform" | "storage" "<" access ">" | "handle" | "immediate"
access      = ident { ident }                                   (subset of load, store, atomic)
binding     = "@builtin" "(" ident ")" | "@location" "(" ( integer | "auto" ) ")" { "@" ident [ "(" ident ")" ] }

gexpr       = "%g" integer "=" expr [ loc ]
constdecl   = "const" "@" ident ":" type "=" "%g" integer [ loc ]
overridedecl= "override" "@" ident ":" type [ "id" "(" integer ")" ] [ "=" "%g" integer ] [ loc ]
globaldecl  = "global" "@" ident ":" type "space" "(" space ")" [ "binding" "(" integer "," integer ")" ]
              { "slot" "(" slot ")" } [ "=" "%g" integer ] [ loc ]
slot        = "universal" integer | "msl." ( "buffer" | "texture" | "sampler" ) integer
            | "hlsl" ident integer integer | "glsl" integer
shaderdecl  = "shader" "@" ident ( "graphics" | "compute" ) "{" { group } "entries" "{" ref { "," ref } "}"
              [ "options" "{" ref { "," ref } "}" ] "}"
group       = "group" "@" ident "class" "(" ident ")" "{" [ ref { "," ref } ] "}"
ref         = "@" ident

func        = "func" "@" ident "(" [ param { "," param } ] ")" [ "->" type [ binding ] ] body
entry       = "entry" stage "@" ident "(" [ param { "," param } ] ")" [ "->" type [ binding ] ]
              [ "workgroup" "(" extent "," extent "," extent ")" ] [ "early_depth" "(" ident ")" ] body
stage       = "vertex" | "fragment" | "compute"
extent      = integer | "%g" integer
param       = "%" ident ":" type [ binding ]
body        = "{" { local } { line } "}"
local       = "local" "$" ident ":" type [ "=" "%" integer ] [ loc ]
line        = "%" integer "=" expr [ loc ]                       (only before-execution and on-entry kinds)
            | stmt
stmt        = "emit" "{" { "%" integer "=" expr [ loc ] } "}"
            | "block" "{" { line } "}"
            | "if" "%" integer "{" { line } "}" [ "else" "{" { line } "}" ] [ loc ]
            | "switch" "%" integer "{" { case } "}" [ loc ]
            | "loop" "{" { line } "}" "continuing" "{" { line } "}" [ "break_if" "%" integer ] [ loc ]
            | "break" [ loc ] | "continue" [ loc ] | "return" [ "%" integer ] [ loc ] | "kill" [ loc ]
            | "store" "%" integer "," "%" integer [ loc ]
            | "image_store" operands [ loc ]
            | "atomic" ident "%" integer "," "%" integer [ "," "%" integer ] [ "->" "%" integer ] [ loc ]
            | "call" "@" ident "(" [ "%" integer { "," "%" integer } ] ")" [ "->" "%" integer ] [ loc ]
            | ( "control_barrier" | "memory_barrier" ) "(" ident { "," ident } ")" [ loc ]
            | "workgroup_uniform_load" "%" integer "->" "%" integer [ loc ]
case        = ( "case" literal | "default" ) [ "fallthrough" ] ":" "{" { line } "}"
expr        = opcode operands
operands    = operand { "," operand }
operand     = "%" integer | "%g" integer | "@" ident | "$" ident | literal | type | ident
            | "[" [ operand { "," operand } ] "]" | ident "(" [ operands ] ")"
literal     = float ( "f16" | "f32" | "f64" ) | integer ( "i32" | "u32" | "i64" | "u64" ) | "true" | "false"
loc         = "!loc" "(" string ":" integer ":" integer ")"
```

Expression opcodes and their operand order are the rows of section 11.1, spelled as there (`literal`, `constant`, `override`, `zero`, `compose`, `access`, `access_index`, `splat`, `swizzle`, `arg`, `global`, `local`, `load`, `image_sample`, `image_load`, `image_query`, `unary`, `binary`, `select`, `derivative`, `relational`, `math`, `convert`, `bitcast`, `array_length`). Optional named operands of image operations are written `name(value)`: `image_sample %t, %s, %c, level(zero), depth_ref(%d)`.

---

## 28. Appendix B: correspondence with naga

| naga | KSIR | Change and reason |
| --- | --- | --- |
| `Module` with `types`, `constants`, `overrides`, `global_variables`, `global_expressions`, `functions`, `entry_points` | Same | Kept |
| `special_types`, `diagnostic_filters`, `doc_comments` | Absent | Predeclared result types (`modf`, `frexp`, atomic compare-exchange) are ordinary interned structs; diagnostics filtering is the definition's; doc comments are not emitted |
| — | `shaders` (section 7.5) | Added: the reflection unit with groups and classes, which naga leaves to its users |
| `Span` side tables, optional | Span on every node, mandatory | Diagnostics from validation and backends must point into source (`KLF-RIDL.md` 8.2) |
| `ScalarKind::AbstractInt`, `AbstractFloat` | Absent | Frontends concretize; a module never holds untyped values (DS-003) |
| `TypeInner::ValuePointer` | Absent | Pointers to components point at interned element types (DS-004) |
| `TypeInner::Struct { members, span }` | Struct with rule, size, alignment, and per-member offset, size, alignment | Layout rule recorded; KSL packing needs member extents (section 22.5) |
| `Array { stride }` | Same | Kept |
| `BindingArray`, `CooperativeMatrix`, `AccelerationStructure`, `RayQuery` | Reserved | Not in 1.0 |
| `AddressSpace` incl. `Handle`, `Immediate` | Same, minus task and ray payload spaces | Reserved stages |
| `ResourceBinding { group, binding }` | Same, plus authored target slots (section 9.4) | MSL input keeps its Metal indices; KSL `@binding` applies to all targets |
| `Binding::Location { location, interpolation, sampling, blend_src, per_primitive }` | Same, with `location` allowed `auto` before normalization, without `per_primitive` | KSL never writes locations |
| `Expression` | Same set minus abstract, ray, subgroup, cooperative kinds; `As` split into `convert` and `bitcast` | Clearer text and validation |
| `Statement` including `Emit` ranges | Same; `Emit` managed by the builder (section 6.4) | Definitions never write `Emit` |
| `Statement::Call` with `CallResult` | Same | Kept |
| `Loop { body, continuing, break_if }` | Same | Kept |
| `SwitchCase { fall_through }` | Fall-through only on empty, non-last cases | Every target can express it |
| `Override` and pipeline-constant processing | Same, with `bake` and `keep` modes per target | HLSL and GLSL cannot keep |
| `valid::Validator` and `ModuleInfo` | Same role, numbered rules SV1 to SV24, keys for user errors | Matches `KLF-RIDL.md`'s V-rule style and definition-owned codes |
| Uniformity analysis | Same; error only when WGSL is a target | Other targets accept divergent derivatives |
| `BoundsCheckPolicy` | Same three policies | Kept |
| Integer division: naga backends polyfill WGSL semantics | Defined by KSIR (section 15.2), with a `native` escape | Goal 5 |
| `proc::Layouter` (WGSL rules) | Four named rules plus `explicit` | Several source languages fix layouts |
| `back::msl::Options::per_entry_point_map` | Binding assignment (section 20) plus authored slots | Assignment is a kernel decision, recorded in reflection |
| Frontends: WGSL, GLSL, SPIR-V | Frontends are definitions: KSL, MSL | Kernel holds no frontend |
| Backends: SPIR-V, MSL, HLSL, GLSL, WGSL, DOT | SPIR-V, MSL, HLSL, GLSL 4.30, WGSL | DOT is replaced by `.ksir` |

---

## 29. Appendix C: built-in spelling per target

| KSIR | SPIR-V `BuiltIn` | MSL | HLSL | GLSL 4.30 | WGSL | KSL |
| --- | --- | --- | --- | --- | --- | --- |
| `position` (vertex out) | `Position` | `[[position]]` | `SV_Position` | `gl_Position` | `@builtin(position)` | `@builtin(position)` |
| `position` (fragment in) | `FragCoord` | `[[position]]` | `SV_Position` | `gl_FragCoord` (`origin_upper_left`) | `@builtin(position)` | `@builtin(position)`, `@builtin(frag_coord)` |
| `vertex_index` | `VertexIndex` | `[[vertex_id]]` | `SV_VertexID` | `gl_VertexID` | `@builtin(vertex_index)` | `@builtin(vertex_index)` |
| `instance_index` | `InstanceIndex` | `[[instance_id]]` | `SV_InstanceID` | `gl_InstanceID` | `@builtin(instance_index)` | `@builtin(instance_index)` |
| `front_facing` | `FrontFacing` | `[[front_facing]]` | `SV_IsFrontFace` | `gl_FrontFacing` | `@builtin(front_facing)` | `@builtin(front_facing)` |
| `frag_depth` | `FragDepth` | `[[depth(any)]]` | `SV_Depth` | `gl_FragDepth` | `@builtin(frag_depth)` | — |
| `sample_index` | `SampleId` | `[[sample_id]]` | `SV_SampleIndex` | `gl_SampleID` | `@builtin(sample_index)` | — |
| `sample_mask` | `SampleMask` | `[[sample_mask]]` | `SV_Coverage` | `gl_SampleMaskIn` / `gl_SampleMask` | `@builtin(sample_mask)` | — |
| `global_invocation_id` | `GlobalInvocationId` | `[[thread_position_in_grid]]` | `SV_DispatchThreadID` | `gl_GlobalInvocationID` | `@builtin(global_invocation_id)` | `@builtin(thread_id)` |
| `local_invocation_id` | `LocalInvocationId` | `[[thread_position_in_threadgroup]]` | `SV_GroupThreadID` | `gl_LocalInvocationID` | `@builtin(local_invocation_id)` | `@builtin(local_id)` |
| `local_invocation_index` | `LocalInvocationIndex` | `[[thread_index_in_threadgroup]]` | `SV_GroupIndex` | `gl_LocalInvocationIndex` | `@builtin(local_invocation_index)` | `@builtin(local_index)` |
| `workgroup_id` | `WorkgroupId` | `[[threadgroup_position_in_grid]]` | `SV_GroupID` | `gl_WorkGroupID` | `@builtin(workgroup_id)` | `@builtin(group_id)` |
| `num_workgroups` | `NumWorkgroups` | `[[threadgroups_per_grid]]` | Constant buffer supplied by the host | `gl_NumWorkGroups` | `@builtin(num_workgroups)` | — |
| `clip_distances` | `ClipDistance` | `[[clip_distance]]` | `SV_ClipDistance` | `gl_ClipDistance` | `@builtin(clip_distances)` | — |

Interpolation: `flat` is `Flat` / `[[flat]]` / `nointerpolation` / `flat` / `@interpolate(flat)`; `linear` is `NoPerspective` / `[[center_no_perspective]]` / `noperspective` / `noperspective` / `@interpolate(linear)`; `perspective` is the default everywhere.

---

## 30. Appendix D: decision log

Specification decisions, numbered `DS-` to keep them apart from `KLF-RIDL.md`'s `D-` series. Implementation decisions taken under rule 1.4.1 go to `docs/decisions.md` with the next `D-` number.

| Id | Decision | Reason |
| --- | --- | --- |
| DS-001 | Shaders get their own IR (KSIR), not a KIR dialect | KIR's ownership model, engines and RIDL do not apply; address spaces, images, interfaces, bindings, uniformity and layout rules do not exist in KIR. Two small IRs beat one large one (section 4.2) |
| DS-002 | KSIR follows naga's design: arenas, expressions separate from statements, `Emit` ranges, structured control flow, explicit pointers and address spaces, explicit layouts, overrides, a validator producing analysis | naga already solves many-frontends-to-many-backends for exactly these targets, and its structure is close to what the oracle's backends needed (Appendix B) |
| DS-003 | No abstract numeric types in a module | Every frontend can resolve literals; keeping them out removes a class of backend cases |
| DS-004 | No value-pointer type | With a unique type arena a pointer to a component costs nothing as an ordinary pointer |
| DS-005 | naga is a reference and a test oracle, never a kernel dependency | Kernel stays dependency-light and on its own release cadence; differential testing still gets an independent validator |
| DS-006 | Shader languages extend a sibling family, `ShaderLanguage`, and build their own toolchains | `Language`'s members are meaningless for shaders; one definition per toolchain (`KLF-RIDL.md` 4.5) is kept |
| DS-007 | Binding assignment is a kernel pass that reproduces the oracle's rules | Hosts binding KSL shaders today keep their slots; reflection and code cannot disagree |
| DS-008 | `KSLR1` and `.resources` stay byte-compatible; KLF only appends records | Existing hosts parse them; the format's own rule is append-only |
| DS-009 | KSL `sample` lowers to explicit level zero in every stage | The oracle's backends disagree (WGSL level 0 everywhere, SPIR-V and MSL implicit in fragments). KSL has no mips, so level zero is what every backend observably returned; making it explicit gives one semantics and no uniformity errors |
| DS-010 | A compute stage with an `output` is a KSL error | The oracle emitted it on four targets and dropped it on SPIR-V; an output nobody can read is a bug, and KSIR forbids compute results |
| DS-011 | Integer division, remainder, shifts and float-to-integer conversion are defined (WGSL's semantics) with a `native` escape | Goal 5. The oracle emitted target operators, which are undefined on edge cases in MSL, HLSL and SPIR-V |
| DS-012 | MSL `constexpr sampler` is refused in 1.0 | Other targets need a host-created sampler object; representing inline sampler state needs a reflection record and host support first |
| DS-013 | Locals, private and workgroup variables start at zero | Defined behavior on every target; the oracle left uninitialized `let` locals to the target |
| DS-014 | KSL keeps its own packing (section 22.5) instead of moving to std140/std430 | Hosts pack against it today; changing it is a language change made deliberately, not a side effect of the IR |
| DS-015 | A uniform binding's reported size is rounded to 16 bytes with alignment at least 16 | Accepted by every target, required by D3D, and reproduces the oracle's uniform layouts |
| DS-016 | GLSL refuses a texture sampled with two different samplers | One combined `sampler2D` carries one sampler state; the oracle silently paired the first |
| DS-017 | Shaders have no VM path: a shader definition builds a native driver only | The VM runs KIR and a shader never becomes KIR (section 4.2). A host language reaches shaders by running the driver (section 24.1), so a VM path would serve no caller |
| DS-018 | Superseded by DS-020. The MSL frontend was a definition in its own repository, `klf-msl` | Section 4 keeps frontends out of the kernel; a repository of its own let it ship and version apart |
| DS-019 | Text backends are the outs of shader definitions, written in KLF Kira; the kernel keeps every per-target decision and writes SPIR-V | A language's in and out then live in one definition, so any language KLF reads it can also write, KSL included, and converting a shader is one build. Keeping passes, bindings, layouts and reflection in the kernel leaves an out nothing to decide but spelling, so outs cannot disagree on what a shader means |
| DS-020 | The MSL, HLSL, GLSL and WGSL definitions live in `klf-kira/languages/`, beside KSL's, and each shader toolchain links the other four | A driver must be able to reach every out its language declares, and a `klf` distribution must not carry four languages' sources a user who writes no shader will never read. The linking is explicit so a build says which languages it needs, and the `knvm` that owns them says so once |
| DS-021 | A shader toolchain is one binary holding every definition it was built with, each a separately compiled object whose only global symbols are renamed per definition | Running the in of one definition and the out of another in one process needs no serialization of units and bindings between processes. Definitions' code is local to their objects, so renaming three entry symbols is all linking them needs |
