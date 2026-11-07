# TIR 26.11.1 — encoding coverage and remaining T0 gates

Status: 2026-11-06. **T0 is not complete.** This is an implementation
inventory, not an amendment to normative `TIR.md` sections 12 and 22.
Distinguish three operations: binary *decoding* an opcode, assembling text
into CODE, and losslessly disassembling CODE into canonical text. A decoder
implementation or a successful binary round-trip does not prove all three.

## Section encoding

| Section kinds | Current builder status | Remaining gate |
| --- | --- | --- |
| 1–8 STRINGS through CODE | Modeled binary encoders present | Full per-record conformance and unrestricted CODE text | 
| 9 VARIANTS, 10 GPU, 11 GENERICS | Modeled binary encoders present | Complete positive/negative coverage, text representation |
| 12 RIDL | Newly wired typed encoder, cross-checked by decoder | Export/inlinable hash corpus, lossless text representation |
| 13 CAPS | **Always emitted**, including when empty | Reachability-driven capability computation, lossless nonempty text |
| 14 ANALYSIS | Typed encoder, derived unmanaged reasons and sorted/deduplicated indirect-call `typeid` inventory; canonical disassembly rejects inconsistent CODE/ANALYSIS | Full sandbox/assembly/closure/witness inference conformance |
| 15 METADATA | Newly wired typed encoder with attachment reference checks | Full attachment combinations and lossless text |
| 16 ASM | Modeled binary encoder present | Complete instruction-to-ASM cross-references, text |
| 17 DEBUG | Eight-table typed encoder/decoder wired into builder, with indexed reference checks and V2 verification | Full section-33 interoperability corpus and lossless textual form |
| 18 PROFILE | **Not wired** | The current spec names counters/value profiles but does not define the section-18 byte grammar; requires normative specification before a typed codec |
| 255 CUSTOM | Builder retains ordered arbitrary producer-private byte payloads | Additional canonical producer-policy conformance |

The text disassembler currently supports only its documented subset of
sections (including *empty* CAPS); it deliberately rejects sections whose
contents would otherwise be silently dropped.

## CODE text assembly/disassembly

| Opcode family | Current text CODE coverage | Remaining gate |
| --- | --- | --- |
| 0x000 constants | Scalars, bytes, strings and zero/null/poison supported subsets; `const.vec` for fixed `i1` masks (packed low-bit-first), `i8`/`i16`/`i32`/`i64` and exact IEEE `f16`/`bf16`/`f32`/`f64`/`f128` vectors with inline CODE for storage up to 16 bytes or CONSTANTS-backed CODE above that limit; `const.aggregate` for nonempty tuples, arrays, nongeneric trivial structs and explicit unions, including nested tuple/array literals and active union member indexes, backed by typed CONSTANTS; GPU-only `const.sampler` 23-byte descriptors with all specified defaults, fields and exact `f32` LOD encoding; `func.ref`, `global.addr`, `tls.addr`, `descriptor.ref`, `block.addr` with same-function provenance and cross-table V2 checks | Generic/managed nominal aggregates, nested nominal fields outside the supported trivial subset, packed narrow-float vector literals, witness/entry references and remaining forms |
| 0x020 integer | Multiple scalar/vector operations, supported modifiers | Full opcode/overflow/masking corpus |
| 0x060 float / math | Arithmetic, compare, scalar `fclass`, `frexp`, `ldexp`, FP status read/clear, and `math.*` with `[approx]` | Full fast-math/rounding/strict modes, vector forms of the special FP opcodes |
| 0x0A0 conversions | 13 opcode names, supported scalar/vector destinations | Full mode/type combinations and bitcast semantics |
| 0x0C0 vector | Splat, extract/insert, shuffle, step/vscale, all 15 reductions with optional masks and reassociation, masked/unmasked vector load/store with alignment, gather/scatter using pointer vectors or base+scaled integer offsets, compress/expand, all four dot selectors, zip/unzip, reverse/slide, concat/split, table, mask count/first; V2 text and binary verification and byte-identical CLI round-trips for the conformant subset. Raw `vec.mask.active` CODE/text round-trips but is rejected by V8 | Normative conflict: section 22.6 says `vec.mask.active` returns `svec<i1>` as SSA, while sections 7.10/16.2 and V8 say `svec<T>` is address-only and cannot be SSA. Must resolve the specification before claiming full `vec.mask.active` conformance. Other scalable-vector policies and modifier combinations remain |
| 0x100 aggregates | Trivial scalar tuple construction/extraction/destructuring and fixed-array construction/extraction/insertion; non-generic trivial `struct.new`, `struct.extract`, `struct.insert`, `struct.destructure`, `enum.new`, `enum.tag`, `enum.take`/`enum.project` (empty, scalar and trivial tuple payloads). Constructor/field payload types checked against TYPES; enum tag width follows C/RIDL/packed layout and variant count. V2/V4/V5 text/binary and CLI byte-identical conformance for supported forms | Nontrivial and generic fields, borrowed/owned payload lifetime synthesis, general ownership-sensitive aggregates |
| 0x120 storage/address | Basic integer/float/vector stack allocation; trivial `field.addr`/`elem.addr`, `enum.payload.addr`, payloadless `enum.inject`, `addr.toptr`, `ptr.toaddr`, and layout-compatible `addr.cast` (same address space), including V2/V20 and canonical binary/text round-trips | Broader nominal/managed address projections, proven payload initialization, full alias and bounds analysis, complete address-only lowering |
| 0x140 memory | Typed direct stack operations; supported `raw.load`/`raw.store` with alignment/flags, `ptr.offset`/`ptr.diff`, `mem.copy`/`mem.move`/`mem.set`, and `prefetch` | Full alias analysis, address projections, and remaining memory modes |
| 0x160 heap/references | Counted, unique and raw allocations; payload addresses, weak new/load, raw-free, unowned pointer and uniqueness query with typed CODE and V2/V4/V8/V14/V20 coverage | Generalized weak Optional lifecycle synthesis, broader ownership and policy canonicalization; `policy default counted/affine` still unsupported |
| 0x180 ownership | Copy/move/destroy, borrow and access scope subset | General ownership/aliasing canonicalization and opcode modes |
| 0x1A0–0x1E0 calls, async, unwind, CFG | Non-generic synchronous `call`, typed `call.indirect` with V19/V20 type IDs, `tail.call`, five binary-mapped call-site attributes; `br`, `cond.br`, `switch.int`, non-generic `switch.enum` with implicit payloads, `block.addr`/`br.indirect` restricted to direct same-function provenance, `return`, `fail`, `unreachable`, `trap` | Generics, other call forms, coroutine/unwind, full block-address forwarding, generic enum switches, CFG variations; `[builtin]` lacks a modifier-bit assignment |
| 0x200–0x280 atomics, varargs, targets, intrinsics, debug, generic, speculation, device, foreign | CPU `atomic.load`/`store`/`rmw`/`cmpxchg`/`fence`/`wait`/`notify`; `va.start`/`arg`/`copy`/`end`, `setjmp`, `longjmp`; target queries, portable intrinsics 11–14, `assume`, `expect`, `nop`, `prof.count`, MMIO/fence.io with typed V2/V14/V20/V21 checks | Full GPU atomics and foreign ABI calls, remaining opcodes; RMW selector codes 16–18 exceed the four-bit modifier field defined in section 22.8 |
| 0x400–0x5C0 GPU | GPU family binary decoder; `gpu.builtin` (0x400) implements the section 17.5 name/type map; `gpu.spec_const` (0x401) emits u32 ID, typed CONSTANTS-backed scalar defaults (i1–i64, f16/bf16/f32/f64) and 2–4 lane vector defaults, with V2/V15 and forged-record checks; `gpu.barrier`/`gpu.memfence` (0x402/0x403) encode scope bits 26–28, optional unchecked-uniformity bit 34 and the section 22.7 five-domain memory byte, with V2/V15 and byte-identical CLI CFG round-trips. Binary V2 rejects CPU placement, invalid scopes, result counts and zero masks | packed narrow-float specialization lanes, stage-specific entry/call graph, advanced GPU text CODE and device conformance; V15 text accepts `gpu.memfence` memory domains such as `constant` that cannot fit the normative five-domain CODE mask (currently fail closed in the assembler) |

The full exit criteria remain `TIR.md` section 35. The test count in
`IMPLEMENTED.md` reflects tests executed, not a coverage or completion
percentage. In particular, TIR does **not** replace KIR while T0/T1 and the
later T2–T4 migration gates are open.