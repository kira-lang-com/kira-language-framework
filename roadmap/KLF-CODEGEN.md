# KLF-CODEGEN: the optimizing native code generator

Status: design, 2026-11-21. Nothing here is built yet. Implementation lives in `languages/klf-swift/` (pure Swift, one
SwiftPM package). This document is the contract the six implementation pieces (section 14) code against; where it and
`TIR.md` / `TVISA.md` disagree on semantics, those two win and this file is corrected.

Goal, in the requester's words: code as good as LLVM's, produced much faster than LLVM produces it, at `-O0` to `-O3`.

---

## 1. Summary

1. The generator is the TVISA finalizer of `TVISA.md` section 27, stages 2 to 7 and 9, written new. It reads an
   in-memory `TVISAProgram` and writes the same `ARM64Object` / `X64Object` values today's back ends write, so image
   writing and linking (A12) do not change.
2. It has one internal IR (**NIR**, section 4) used from the TVISA reader to instruction selection, and one machine IR
   (**MIR**, section 8) used from selection to bytes. Both are flat arrays of integers; no class instances, no
   dictionaries and no strings on the per-instruction path.
3. Four levels share one code path and differ only in which passes run (section 6), which selector tier is used
   (section 9) and which register allocator is used (section 10).
4. Functions are compiled in parallel, one task per function, and joined in function-index order, so output is
   bit-identical whatever the thread count (TIR.md section 26.8).
5. It is split into six pieces with disjoint files: core IR and driver, scalar optimizer, loop and memory optimizer,
   machine layer and register allocation, arm64 back end, x86-64 back end.

## 2. Targets

The run-speed and compile-speed benchmark against LLVM (clang and swiftc at `-O0` to `-O3`) is a separate piece
(`Benchmarks/codegen/` in `languages/klf-swift`, run with `python3 Benchmarks/codegen/run.py`). Sections 2.1 and 2.2
give the targets as ratios; section 2.3 gives the first measured figures and the absolute targets that follow from
them. Section 13 says how each target is gated.

### 2.1 Speed of the generated code (geometric mean over the benchmark's programs, same machine)

| Level | Compared with | Target (our run time / theirs) | First gate |
| --- | --- | --- | --- |
| `-O0` | clang `-O0`, swiftc `-Onone` | at most 0.70 | at most 1.00 |
| `-O1` | clang `-O1` | at most 1.05 | at most 1.25 |
| `-O2` | clang `-O2`, swiftc `-O` | at most 1.00 | at most 1.15 |
| `-O3` | clang `-O3`, swiftc `-O` (whole module) | at most 1.00 | at most 1.10 |

No single program may be slower than 1.5 times LLVM's at `-O2` or `-O3`; one that is counts as a bug with an owner.
`-O0` beats LLVM's `-O0` because values stay in registers within a block (section 10.1) while LLVM's `-O0` keeps
every value in memory.

### 2.2 Speed of the generator (wall time of stages 2 to 9, from `TVISAProgram` to objects, one core)

| Level | Compared with LLVM's back end at the same level | Design budget, TVISA instructions per second per core |
| --- | --- | --- |
| `-O0` | at least 20 times faster | 3,000,000 |
| `-O1` | at least 10 times faster | 1,200,000 |
| `-O2` | at least 5 times faster | 400,000 |
| `-O3` | at least 3 times faster | 150,000 |

Other limits: peak memory per worker at most 64 MiB plus 16 times the largest function body (TVISA section 27.4);
time grows no faster than n log n in function size at every level (section 12.3 lists the caps that guarantee it);
throughput scales to at least 0.85 × core count on 8 cores.

### 2.3 Measured starting point and absolute targets

First benchmark run: arm64 macOS, Apple M4 Pro VM, Swift 6.4, Apple clang 21, KLF's classic generator at main on
2026-11-21. Fastest of 3 compiles and 5 runs, 11 kernels. swiftc has no `-O1` or `-O3`, so its four levels are
`-Onone`, `-Osize`, `-O`, `-Ounchecked`; clang compiles a line-for-line C twin. x86-64 Linux figures are added when
the runner has them.

Where the classic generator stands (geometric mean, KLF / other; above 1 means KLF is behind):

| Level | Run vs swiftc | Run vs clang | Code bytes vs swiftc | Code bytes vs clang | Whole compile vs swiftc | Whole compile vs clang |
| --- | --- | --- | --- | --- | --- | --- |
| `-O0` | 0.23 | 1.25 | 1.07 | 4.58 | 2.27 | 9.02 |
| `-O1` | 3.48 | 4.03 | 2.97 | 9.29 | 2.08 | 8.86 |
| `-O2` | 2.42 | 2.71 | 2.35 | 6.42 | 2.20 | 9.23 |
| `-O3` | 2.54 | 2.83 | 2.62 | 4.91 | 2.33 | 8.91 |

Today `-O1` runs like `-O0` and `-O3` like `-O2`: there are two tiers, and the new generator must make four.

Per kernel at `-O2`. The target is the faster of swiftc `-O` and clang `-O2`; the first gate is 1.15 times that;
no kernel may stay above 1.5 times.

| Kernel | KLF today, ms | swiftc `-O`, ms | clang `-O2`, ms | Target, ms | KLF today, bytes | swiftc, bytes | clang, bytes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| bitmix | 459 | 435 | 442 | 435 | 592 | 368 | 132 |
| closures | 779 | 72 | 69 | 69 | 2504 | 680 | 268 |
| collatz | 391 | 300 | 199 | 199 | 928 | 560 | 144 |
| fib | 823 | 545 | 465 | 465 | 800 | 276 | 136 |
| gcd | 278 | 279 | 255 | 255 | 912 | 420 | 120 |
| mandelbrot | 624 | 154 | 123 | 123 | 1552 | 628 | 220 |
| matmul | 1709 | 210 | 207 | 207 | 3784 | 1620 | 600 |
| nbody | 1051 | 192 | 140 | 140 | 6460 | 1948 | 816 |
| quicksort | 945 | 660 | 652 | 652 | 3256 | 1616 | 576 |
| sieve | 673 | 364 | 380 | 364 | 2568 | 828 | 420 |
| structs | 495 | 319 | 310 | 310 | 960 | 588 | 184 |

What the gaps say about the work:

1. closures (11 times): no inlining through closure values. That is TIR-level work (A7, section 3.2); the generator's
   part is cheap `try.call.closure` sequences.
2. matmul (8 times): bounds checks kept and no vectorization. `checks` and `indvars` remove the checks; loop
   vectorization is not in this design's pass lists and is the first addition if matmul misses its gate.
3. nbody, mandelbrot (4 to 7 times), and the `-O0` weak spots (mandelbrot 1.9, nbody 2.3, sieve 3.6 times clang
   `-O0`): floating point and array access. Selection rules and allocation of float registers (sections 9, 10).
4. fib, collatz, quicksort, sieve, structs (1.3 to 1.9 times): general code quality, the core of sections 6 to 12.

Other absolute targets:

- Code size at `-O2`: at most 1.25 times clang's bytes per kernel (today 6.4 times), and at most swiftc's.
- `-O0` run time: at most 0.70 of clang `-O0` (today 1.25); already 0.23 of swiftc `-Onone`.
- Generator time: a whole KLF compile is 370 to 500 ms per kernel against 135 to 305 ms for swiftc and 40 to 66 ms
  for clang, but about 390 ms of that is fixed front-end start cost (a one-line `print(1)` takes 390 ms). Optimizer,
  generator and link together are 10 to 35 ms per kernel today. The generator is therefore measured on its own
  (`klf build --codegen-report`, `klf bench-codegen`), with the target that TIR optimization, code generation and
  link stay under 10 ms per kernel at `-O0` and under 35 ms at `-O3`, so the better code costs no more time than the
  classic path. The 2.2 ratios against LLVM's back end need LLVM's back end timed alone (`clang -ftime-report`),
  which the benchmark does not report yet.
- The front-end start cost is outside this generator. Until it is fixed, no back end can make the whole `klf build`
  faster than clang; it is listed in section 18.

## 3. Pipeline

```
TIR module
  │  A7  TIROptimize        target-independent, whole module: specialize, inline, devirtualize, SROA, ARC, vectorize
  │  A9  TIRLowering        ownership to calls, layout, coroutine split, signatures  ->  TVISAProgram
  ▼
TVISAProgram ─────────────── new code starts here ───────────────
  1 Read       TVISA function -> NIR (SSA kept; one pass; types become machine classes)
  2 Select     variant choice, fold `target.has` / page size / fixed vscale        (TVISA 27.3 stage 2)
  3 Legalize   i128, f16/f128, narrow ints, vector widths, masks, wide aggregates   (stage 3)
  4 Optimize   NIR passes by level (section 6)                                      (stage 4)
  5 Lower      instruction selection NIR -> MIR, calling conventions, hardening     (stage 5)
  6 Allocate   registers, spill slots, frame layout                                 (stage 6)
  7 Emit       block order, peepholes, branch relaxation, constant pools, bytes     (stage 7)
  8 Tables     unwind rows, trap sites, line rows, function starts                  (stage 9)
  ▼
ARM64Object / X64Object  ->  A12 ImageLinker (unchanged)  ->  Mach-O / ELF / Tarwin image
```

Stages 1 to 8 run per function with no shared mutable state. The only whole-image work is the join: concatenating
code, rebasing relocations and sorting tables, done sequentially in function-index order.

### 3.1 Why it reads TVISA and not TIR

1. `TVISA.md` section 27 defines the finalizer as TVISA in, image out, and the same generator must later run on
   devices where only TVISA exists.
2. A9 already turns ownership, enums, closures, typed failure, coroutines and layout into loads, stores, calls and
   offsets. Redoing that in a second lowering would double the surface every new TIR form touches.
3. The cost of the extra hop is small because the reader takes the `TVISAProgram` in memory; nothing is encoded to
   `.tvisa` bytes and decoded again on the build path.

Consequence: what A9 emits is the generator's input contract. Section 5 lists what the generator needs A9 to keep or
add (facts that today are dropped at the boundary).

### 3.2 Division of optimization work

| Kind of optimization | Where | Why there |
| --- | --- | --- |
| Needs types, ownership, generics or the whole module: specialization, inlining, devirtualization, SROA, ARC pairing, stack promotion of classes, closure specialization, loop interchange, loop vectorization | TIR, A7 (kept, section 11) | The facts exist only in TIR |
| Needs only values and memory: constants, value numbering, range facts, redundant loads and stores, branch folding, loop-invariant motion, induction variables, unrolling, check removal | NIR, new (section 6) | Lowering creates most of this work (address arithmetic, leaf loads, error-word tests, bounds checks), so it must be cleaned after lowering; the flat IR makes it cheap |
| Needs the target: addressing modes, flag reuse, compare-and-branch, conditional select, multiply-add, immediates, load-operate folding, vector idioms | Selection and MIR peepholes, new (sections 9, 12) | Per-target rules |

## 4. NIR, the IR the generator works on

### 4.1 Shape

NIR is SSA with block parameters (as TIR and TVISA have), one function at a time.

| Thing | Representation |
| --- | --- |
| Value | `UInt32` index. The instruction that defines result 0 has the same index as its value; extra results (`add.ovf`, calls with several results) are `project` instructions that follow it |
| Instruction | Row in parallel arrays: `opcode: UInt16`, `type: UInt8`, `flags: UInt8`, `a, b, c: UInt32` (operands or immediates), `block: UInt32`. Instructions with more than three operands (calls, block arguments, `switch`) store a range into one shared `operandPool: [UInt32]` |
| Block | Row: first and last instruction, parameter range, successor range, predecessor range, loop depth, frequency class (`hot`, `normal`, `cold`) |
| Order | Instructions of a block are a doubly linked list through `next`/`previous` arrays, so passes insert and remove in constant time; a compaction pass renumbers into layout order before selection |
| Type | One byte: `i8 i16 i32 i64 i128 f16 f32 f64 f128 ptr`, plus `v128` / `v256` / `v512` / `mask` with a lane byte in `flags`. Aggregates do not exist: TVISA already split them into leaves |
| Constant | `iconst`/`fconst` instructions; 64-bit payload in `a`,`b`. Selection folds them into immediates; they are never given a register unless a use needs one |
| Use lists | Not stored. A pass that needs them asks for `UseCounts` or `UseLists`, built in one sweep and dropped after. Value replacement is a `forward: [UInt32]` table resolved lazily, so "replace all uses" is constant time |
| Side tables | Dense arrays indexed by value or block; bit sets for liveness. No `Dictionary` or `Set` keyed by value anywhere in a pass's inner loop |
| Memory | Each function owns one `NIRFunction` struct whose arrays are reserved once from the TVISA body size (× 1.5) and reused by the worker for the next function |

### 4.2 Instructions

NIR opcodes are TVISA's instruction set (`TVISA.md` section 11) minus what stage 2 and 3 remove, with these changes:

1. Calls carry a resolved `CallSite` record (convention, items with their classes, failing flag, context flag, tail
   flag, callee kind: defined / import slot / runtime entry / indirect with type tag), built once by the reader.
2. Memory operations carry an **alias class** byte (section 4.4) and the access size.
3. Checks are explicit: `trap.if cond, code` stays one instruction through the optimizer, so a pass removes a check
   by proving `cond` false and nothing else needs to know about trap blocks. Selection moves the trap out of line.
4. Runtime entries used on hot paths (`ref.retain`, `ref.release`, `ref.isunique`, `heap.alloc.*`) stay calls but
   carry an effect summary (reads / writes / may-free by alias class), so value numbering and load elimination see
   through them.
5. Selection-only opcodes may be introduced by the last NIR pass (`addr base, index, scale, displacement`,
   `cmp.br`, `select.cmp`); passes before it never see them.

### 4.3 Analyses (computed on demand, cached per function, invalidated by a pass's declared effect)

Reverse post-order; dominator tree (Cooper, Harvey, Kennedy: no extra memory, fast on the small graphs functions
have); natural loops and loop depth; block frequency estimate (static: loop depth, `cold` marks, trap and failure
edges are cold, `expect` hints from TIR kept by A9); liveness (bit sets per block, only for the allocator and the
verifier); integer range facts (section 6.2).

### 4.4 Alias classes

Two accesses can touch the same memory only if their classes can overlap. Classes are assigned by the reader from what
TVISA says about the address:

| Class | Source | Can overlap |
| --- | --- | --- |
| `slot(n)` | `slot.addr` / static `stack.alloc` whose address never escapes | only itself |
| `global(g)` | `global.addr` | itself, `unknown` |
| `object.header` | reference-count and descriptor words (offset 0 to 15 of a `ref`) | itself, `unknown` |
| `object.field(k)` | load or store at a constant offset `k` from a `ref` payload | same `k`, `unknown` |
| `object.tail` | address derived from a tail allocation (section 5.2) | itself, `unknown` |
| `frame` | coroutine frame fields | same offset, `unknown` |
| `unknown` | anything else | everything except non-escaping slots |

A `borrowmut` or `uniq` parameter gets its own class for the function (TIR.md section 26.4: the verifier proved it).
A9 passes those marks on signature items (section 5.1).

### 4.5 Text form and verifier

NIR has a text form (`.nir`), a parser and a printer, used only by tests and `klf build --emit nir`. The NIR verifier
(types, dominance, block arguments, call-site shape) runs after every pass in debug builds and under
`KLF_VERIFY_NIR=1`; never in release builds. A NIR interpreter in the test target runs any function on given inputs,
so every pass is tested as "result before equals result after" on generated programs (section 13).

### 4.6 Row conventions (as built in `Sources/NativeIR`)

1. An opcode's number is its TVISA binary opcode for every instruction NIR shares with TVISA. NIR-only: `parameter` 0x800, `project` 0x801, and the selection-only forms from 0x810.
2. Each row has a type, the TIR/TVISA modifier word unchanged (TIR.md section 22.8), and three 32-bit fields `a`, `b`, `c`. Comparison predicates are the modifier selector (bits 48 to 51), numbered as `ComparisonPredicate`. Types include `i1` for conditions and overflow bits.
3. Block parameters are `parameter` rows at the start of the block (`a` is the position); the entry block's are the function's incoming values. Results after the first are `project` rows (`a` the instruction, `b` the result index).
4. Plain operations keep value operands in `a`, `b`, `c`. Constants: `a` low and `b` high 32 bits. `raw.load`: address `a`; `raw.store`: value `a`, address `b`; both keep an access word in `c` (low 6 bits log2 alignment, upper 26 bits alias class id, 0 unknown). `trap.if`: condition `a`, code `b`.
5. A target record in the operand pool is `[block, n, arguments]`. `br`: `b` is the pool offset of one record. `cond.br`: condition `a`, `b` the offset of the taken record then the not-taken record. `switch.int`: value `a`, `b` the offset of the default record then `keyLow, keyHigh, record` per case, `c` the case count. `return`, `call`, `call.runtime`: `b` the pool offset of the values, `c` their count; `call.indirect` adds the callee in `a` and the signature index and type tag after the arguments.
6. `NIRShape` (one value per opcode) is the single description of these layouts; operand visiting, the text form and the verifier all read it. An opcode with no shape yet makes the verifier fail, which sends the function to the classic generator.

## 5. Input contract with A9 (TIR to TVISA lowering)

### 5.1 Facts A9 must keep (small additions to `TVISAProgram`, in memory; the `.tvisa` encoding gets them in a later version)

| Fact | Where it rides | Used for |
| --- | --- | --- |
| `noalias` / `readonly` on pointer items from `borrowmut`, `borrow`, `uniq` | signature item flags | alias classes |
| Access kind of a `raw.load`/`raw.store`: slot, object field, header, tail, frame | 3 bits in the modifier word | alias classes |
| Branch hints: `expect`, cold blocks, failure edges | block flag (already exists for cold) | layout, spill weights |
| Loop marks the vectorizer made (vector loop, remainder loop, already unrolled) | block flag on the header | do not unroll or re-vectorize |
| Non-null and range of results: allocation results, `ref` loads, array counts ≥ 0 | result flags on the instruction | check removal |

Until A9 provides a fact, the reader assumes the conservative answer (`unknown` class, no range). The generator is
correct with none of them; they only make the code better.

### 5.2 The new TIR forms (main sequence 3597)

The generator must compile all five. They reach it as TVISA, in these shapes:

| TIR form | TVISA shape the generator receives | What the generator must support |
| --- | --- | --- |
| `try.call.indirect %fn : fnT (args) normal bbN, error bbE` | `call.indirect` with a failing signature and type tag; trailing error-word result; `cond.br` on it to the error edge block | Failing flag on **indirect** calls in both back ends: zero the error register before the call, define the error word from it after (x21 on arm64, r12 on x86-64 as today's direct `try.call`). Error edge is cold |
| `try.call.closure %c(args) normal bbN, error bbE` | as above, plus a `context` item carrying the closure context | Context register and error register live across the same indirect call; both conventions |
| `try.await %f normal bbN, error bbE` | after `CoroutineSplitter`: poll call, pending return, then the result load and an error-word test from the child frame | Nothing new in selection; the optimizer must treat the frame's error field as class `frame` and keep the suspend state store before the pending return |
| `heap.alloc.counted.tail T, E, %n` | size arithmetic with overflow and negative-count `trap.if` (code 2), then the counted allocation runtime call | Range facts: result non-null, `%n ≥ 0` after the check; the multiply-with-overflow selects to one instruction plus a flag branch |
| `ref.tail.addr %r, E` | `ptr.offset %r, constant` (header + payload size rounded to `E`'s alignment) | Folds into addressing modes; accesses through it get class `object.tail`, so tail stores do not kill field loads |

At the time of writing A9 does not lower these five yet (only `try.call` is). That lowering is A9's; the generator's
tests for these shapes use hand-written TVISA until it lands, then add TIR-to-native tests through the whole path.

## 6. Optimization levels

Passes named here are NIR passes unless marked. "Scalar" passes are piece G2's, "loop/memory" passes are G3's.

### 6.1 Pass lists

| Step | `-O0` | `-O1` | `-O2` | `-O3` |
| --- | --- | --- | --- | --- |
| Read, select variants, legalize | yes | yes | yes | yes |
| `simplify` (constants, identities, dead code, in one forward sweep while reading) | no | yes | yes | yes |
| `sccp` (constants and unreachable code through branches) | no | no | yes | yes |
| `gvn` (dominator-scoped value numbering, with loads by alias class) | no | block-local | yes | yes, 2 rounds |
| `ranges` + `checks` (range facts, remove proved `trap.if`, overflow checks, null checks) | no | no | yes | yes |
| `branches` (fold, thread jumps through known conditions, merge blocks, if-conversion to `select`) | no | fold and merge only | yes | yes |
| `memory` (redundant load, dead store, store-to-load forwarding across blocks) | no | no | yes | yes |
| `licm` (hoist; creates preheaders; hoists loads when the loop has no clobber of the class) | no | no | yes | yes |
| `indvars` (strength reduction, exit-test rewrite, remove derived inductions) | no | no | yes | yes |
| `unroll` (full for small constant trips; partial by 2 or 4 for small bodies) | no | no | full only | full and partial |
| `unswitch` (hoist invariant branches out of loops by cloning) | no | no | no | yes |
| `tails` (duplicate small join blocks into predecessors; hot paths only) | no | no | no | yes |
| `arc.fast` (inline retain / release / uniqueness fast paths, section 6.4) | no | no | yes | yes |
| `late` (addressing forms, compare-with-branch fusion marks, `select` forming, sink to uses, final dead code) | no | yes | yes | yes |
| Selector tier (section 9) | direct | rules | rules | rules |
| Register allocator (section 10) | local | linear scan | splitting | splitting, more rounds |
| Block placement | TVISA order, cold last | TVISA order, cold last | by frequency chains | by frequency chains, loop alignment |
| MIR peepholes (section 12.1) | no | yes | yes | yes |

`-Os` is `-O2` without `unroll`, `unswitch`, `tails`, loop alignment and `arc.fast`.

The A7 TIR pipeline in front keeps its own level mapping (`PassPipeline.standard(level:)`), with one change: at `-O0`
it runs nothing but required specialization, as now.

### 6.2 How the scalar passes are built (G2)

1. **One rewrite table.** `simplify`, `gvn` and `late` share a rule table `rewrite(opcode, operands) -> value?`:
   constant folding, algebraic identities, strength reduction of multiply/divide/remainder by constants, compare
   canonicalization, extension and truncation chains, boolean simplification, select folding. Rules are plain Swift
   `switch` statements over the opcode; no pattern language and no build-time generator. Every rule respects TVISA
   modes: nothing that can trap is folded into something that cannot unless the trap is proved impossible, and no
   floating-point rewrite changes a result bit unless fast-math flags allow it.
2. **`gvn`** walks the dominator tree with a scoped hash table (open addressing over a flat array, keys are
   `(opcode, type, flags, a, b, c)`). Loads are numbered by `(address value, class, size, memory version)`, where the
   memory version of a class is bumped by any store or call that may write it.
3. **`sccp`** is the standard sparse conditional constant propagation over the SSA graph with a three-level lattice.
4. **`ranges`** keeps an interval `[lo, hi]` and known-zero bits per integer value, refined on dominated branches
   (`i < n` in the taken block). One forward pass plus a bounded re-visit of loop headers (at most 2 rounds).
   `checks` then deletes `trap.if` whose condition the facts decide, turns checked arithmetic into wrapping
   arithmetic when no overflow is possible, and merges equal checks along a dominating path.
5. **`branches`** threads a jump when the branch condition is known on an incoming edge, converts a diamond of at
   most 2 cheap, non-trapping instructions per side into `select`, and turns dense `switch.int` into a jump table
   and sparse ones into a balanced compare tree (decided here, emitted by selection).

### 6.3 How the loop and memory passes are built (G3)

1. Every loop pass first calls `canonicalize(loop)`: one preheader, one latch, dedicated exits. It is the only
   place that edits the loop's edges, so passes do not each re-implement it.
2. **`memory`** uses the alias classes of 4.4. Within a class it tracks, per block, the last stored value per
   `(address, size)`; across blocks it merges by adding block parameters (the same shape A7's stack-slot promotion
   uses). Non-escaping slots are promoted to values entirely here, which catches the slots lowering itself creates
   (call temporaries, enum slots, `sret` buffers that are only read back).
3. **`licm`** hoists pure instructions, and loads whose class has no store or clobbering call in the loop. A
   division or checked operation is hoisted only if it runs on every iteration (it dominates the latch) or is
   proved not to trap.
4. **`indvars`** finds `phi = start + step·k` inductions, rewrites `base + i·stride` addresses to a pointer
   induction when that removes the multiply and frees a register, rewrites the exit test to the remaining induction,
   and computes trip counts for `unroll`. It uses range facts to prove no wrap.
5. **`unroll`**: full when trip count ≤ 16 and body ≤ 24 instructions at `-O2` (≤ 32 and ≤ 64 at `-O3`); partial by 2
   or 4 at `-O3` for bodies ≤ 16 instructions with a computable trip count, with a scalar remainder. Loops marked by
   A7 as vector or remainder loops are not touched.
6. **`unswitch`** and **`tails`** have a growth cap of 25 % of the function per pass and act only on blocks whose
   frequency class is `hot`.
7. Loop vectorization, SLP and interchange stay in A7 on TIR. G3 only cleans what they leave (legalized vector
   loops get `licm`, `indvars` and address folding like scalar ones).

### 6.4 Inline reference-count fast paths (`arc.fast`, G3)

`ref.retain` and `ref.release` calls to `klfrt` dominate object-heavy code. At `-O2` and up each becomes: load the
count word, test the "immortal or needs slow path" bits, atomic add/sub inline, and a cold call to the runtime only
when the count reaches zero or the bits are set. The exact word layout is libtirrt's (A13); the generator reads it
from one constants file owned by G3 (`RefCountLayout.swift`) that mirrors libtirrt, with a test that fails if they
drift. If the layout is not frozen when G3 starts, the pass ships disabled and the runtime calls stay.

## 7. Legalization (G4)

Runs on NIR right after the reader, driven by a `TargetCapabilities` value the back end supplies (which types have
registers, which operations are native per type).

| Input | Result |
| --- | --- |
| `i128` arithmetic, compares, shifts | Pairs of `i64` with carry forms; multiply and divide call libtirrt helpers |
| `i8`/`i16` arithmetic | Done in `i32` with the value form of the target (section 8.3), re-extended only where a use observes the high bits |
| `f16` | Converted to `f32` at loads/stores (native on arm64 with FP16; helper calls otherwise); `f128` always helper calls |
| Vectors wider than the target register | Split into halves; narrower ones widened where lanes allow |
| `mask` | Lane-sized masks on NEON/SSE/AVX2; `k` registers on AVX-512 |
| Scalable `svec` | Fixed at vscale 1 (128-bit), as A10 and A11 agreed |
| Operations with no instruction (`frem`, `ctpop` without POPCNT, `fma` without FMA) | Expansion or helper call, per the capability table |

## 8. MIR, the machine IR (G4)

### 8.1 Shape

Flat arrays again: `opcode: UInt16` (target's), up to four operand slots of `UInt32`, a flags byte. An operand is a
virtual register, a physical register, an immediate index, a block, a frame-slot index or a symbol index, tagged in
the top 3 bits. MIR stays in SSA with block parameters until allocation; the allocator's move resolver turns edge
arguments into parallel moves.

### 8.2 What the target tells the machine layer

Each back end implements one protocol, `TargetBackend` (static dispatch through a generic parameter; no existential
in inner loops):

| Member | Content |
| --- | --- |
| `registers` | Classes (integer, float/vector, mask), allocation order, caller-saved and callee-saved sets, reserved registers |
| `describe(opcode)` | Per MIR opcode: operand roles (use, def, use-def, fixed register, clobbers), whether it is a move, a call, a branch |
| `capabilities` | For legalization (section 7) |
| `select(...)` | Section 9 |
| `callingConvention(...)` | Argument and result locations for a `CallSite` |
| `frame(...)` | Prologue, epilogue, hardening, unwind rows for the final frame layout |
| `encode(...)` | Bytes and relocations for one MIR instruction; size bounds for relaxation |
| `peephole(...)` | Section 12.1 |

### 8.3 Value form

Integers narrower than the register keep **undefined high bits** in registers (LLVM's convention), not the
zero-extended form A10 uses today. An extension is inserted only where a use reads the full register (compare,
divide, right shift, conversion, call argument per the convention, address arithmetic), and `gvn` removes repeats.
This removes most `ubfx`/`movzx` in current output.

## 9. Instruction selection

| Tier | Level | Method |
| --- | --- | --- |
| Direct | `-O0` | One fixed MIR expansion per NIR instruction, emitted in a single forward sweep that is fused with the local allocator (section 10.1). Immediates fold when they fit; nothing else is combined |
| Rules | `-O1`+ | Blocks are walked backward; for each root (instruction with a side effect or more than one use) the selector matches the largest tree of single-use operands a rule accepts, then visits the leaves. Rules are Swift `switch` code, most specific first |

Rule families every back end must cover (the arm64 and x86-64 lists are in sections 15 and 16):

1. Addressing: base + index × scale + displacement, PC-relative globals, pre/post-indexed forms.
2. Compare feeding a branch, a `select` or a `trap.if` uses the flags directly; a flag-setting arithmetic
   instruction replaces a following compare with zero.
3. Overflow-checked add/sub/mul: one flag-setting instruction and one branch to the function's shared trap block.
4. Immediates in every form the instruction set allows; constants that do not fit are built in the fewest
   instructions or loaded from the constant pool.
5. Multiply-add, shifted and extended operands, bit-field extract/insert, rotate, byte swap, count-zero forms.
6. `select` as conditional move/select; small diamonds left by `branches` as conditional compare chains.
7. Division by constant arrives already rewritten (G2); division by zero and overflow checks select as in 3.
8. Calls: direct, import slot (TVISA section 18.4: no stubs), runtime entry, indirect with the type-tag check
   (section 17.2), tail calls, failing calls and context calls in any combination (section 5.2).
9. Vector operations to native lanes; no vector goes through a frame object unless legalization says so.

Selection also performs calling-convention lowering (arguments to their registers or stack slots as fixed-register
operands), so the allocator sees ordinary constraints and no special call logic.

## 10. Register allocation (G4)

All three allocators share liveness, the move resolver (parallel moves with cycle breaking through a scratch
register or an exchange), spill-slot coloring and the `TargetBackend` descriptions.

### 10.1 `-O0`: local allocator, fused with direct selection

One forward sweep per block. A value gets a free register when defined; when none is free the furthest-next-use
value in the block is stored to its slot. At a block end every live value is stored to its home slot; block
parameters are slots. No liveness analysis, no intervals: cost is one pass over the code. Values with a debug
variable also keep their slot up to date at every statement boundary, so debuggers always find them.

### 10.2 `-O1`: linear scan on SSA

Intervals with holes over a linearized block order (Wimmer and Franz), one split at the first use after a spill,
register hints from moves and fixed-register operands, caller-saved registers used for intervals that cross no call.
One pass, no iteration.

### 10.3 `-O2`, `-O3`: splitting allocator

A priority-queue allocator over live-range bundles (the design of LLVM's greedy allocator and regalloc2):

1. Build live ranges per virtual register; merge ranges joined by moves and block arguments into bundles when they
   do not overlap (this is coalescing).
2. Queue bundles by size; assign each the hinted register if free, else any free one.
3. On conflict, evict cheaper bundles (spill weight = uses and defs weighted by 10^loop-depth and frequency class),
   or split the bundle: around calls, at loop boundaries, at cold-block edges, then per use.
4. Spill what cannot be split further; constants, addresses of slots and globals are **rematerialized** at the
   use instead of reloaded.
5. Both caller-saved and callee-saved registers are in the pool. A callee-saved register costs its save/restore,
   charged once; a range crossing calls prefers one, a range crossing none prefers caller-saved. This replaces the
   callee-saved-only pools of A10 and A11.
6. `-O3` allows a second eviction round and split-at-every-loop-level; `-O2` caps evictions per bundle at 4.
7. After assignment: redundant-move removal, spill-slot sharing by interference, and shrink-wrapping of
   callee-save stores into the blocks that need them when the entry path is hot and does not.

Conservative mode (TVISA section 27.6): on an internal error at any level the function is recompiled with the `-O0`
tier, which must succeed on every verified function.

## 11. What happens to today's code

| Current code | Owner | Verdict |
| --- | --- | --- |
| `TIROptimize` (27 passes) | A7 | **Kept**, and stays the home of everything that needs TIR facts (3.2). Its loop-invariant motion, common-subexpression, constant folding and induction-variable passes stay for now because inlining and vectorization feed on them; once G2/G3 meet the `-O2` gate, A7 may drop the late repeats (`late:` list in `PassPipeline.standard`) |
| `TIRLowering` (TIR to TVISA, `CoroutineSplitter`, witness tables) | A9 | **Kept**. Adds the facts of 5.1 and the lowering of the forms in 5.2 |
| `TVISA` (program model, text, binary) | A9 | **Kept**, read only |
| `TIRCodegenARM64`: `ARM64Instruction`, `ARM64Assembler` (encoders) | A10 | **Reused** by import; G5 adds missing encodings in its own module, not there |
| `TIRCodegenARM64`: `ARM64Object`, `ARM64Unwind`, `ARM64Hardening`, `ARM64CallingConvention` | A10 | **Reused**: the new back end fills the same `ARM64Object` and calls the same unwind encoders and type-tag function, so objects from both generators link together |
| `TIRCodegenARM64`: `ARM64FunctionLowering`, `ARM64TVISAInput`, `ARM64Finalizer` | A10 | **Replaced** when the arm64 gate passes (section 13.4); left untouched until then and kept afterwards as `--codegen classic` for one release |
| `TIRCodegenX64`: `X64Assembler`, `X64Register`, `X64Object`, `X64Unwind` | A11 | **Reused**, same terms |
| `TIRCodegenX64`: `X64FunctionCompiler`, `X64RegisterAllocator`, `X64Scalarize`, `X64TVISAReader`, `X64Codegen`, `X64Layout` | A11 | **Replaced** at the x86-64 gate, same terms |
| `TIRImage` (objects, linker, Mach-O / ELF / Tarwin writers, debug line tables) | A12 | **Left alone**. No format or API change is needed |
| `KLFDriver/NativeBuilder` | A14 | **Edited once**, by G1: a `codeGenerator` option (`classic`, `optimizing`; `klf build --codegen`) chooses which finalizer fills the objects; default stays `classic` until the gates pass |
| `TIRInterpreter` | A8 | **Left alone**; used as the reference in differential tests |
| A10's arm64 emulator in `Tests/TIRCodegenARM64Tests` | A10 | **Reused** by G5's tests (moved to a shared test-support target by G1's package change) |

A10 and A11 keep fixing bugs in the classic path while the new one is built; they add no new optimizations there.

## 12. Emission (G4 framework, G5/G6 rules)

### 12.1 MIR peepholes (after allocation)

Remove moves to self and dead moves; merge adjacent loads/stores into pairs; fold a spill reload into the using
instruction where the target allows a memory operand; invert a branch over a jump; thread jumps to jumps; drop a
compare when the previous instruction already set the flags; replace `mov 0` forms with the cheapest zeroing form.

### 12.2 Layout

1. Blocks are chained by edge frequency (hottest successor falls through), cold blocks and trap stubs go to the
   end of the function, loop headers of hot innermost loops are aligned at `-O3` (16 bytes x86-64; 4 bytes is
   already implicit on arm64).
2. Branch relaxation: one forward size estimate, then a fix-up pass that grows out-of-range branches; arm64
   functions over 1 MiB get veneers instead of today's diagnostic.
3. Constants that are not immediates go to a per-function literal island placed after the function (arm64) or to
   read-only data with PC-relative loads (x86-64).

### 12.3 Caps that keep compile time near linear

| Pass | Cap |
| --- | --- |
| `gvn` | Scoped table; no fixpoint; `-O3` second round only on functions that changed |
| `ranges` | At most 2 visits per loop header |
| `memory` | At most 64 tracked addresses per class per block; beyond that the oldest is forgotten |
| `licm`, `indvars`, `unroll`, `unswitch` | Skipped for loops over 4096 instructions; total growth ≤ 2× function size |
| Splitting allocator | Evictions per bundle capped (4 at `-O2`, 16 at `-O3`); functions over 50,000 instructions use linear scan |
| Any level | A function over 200,000 NIR instructions is compiled at `-O1` and reported in the timing output |

## 13. Testing and gates

### 13.1 Correctness

1. **Differential fuzzing.** A11's TIR program generator feeds four executions: TIR interpreter, classic back end,
   new generator at each level, and the NIR interpreter before and after each pass. All must agree bit for bit.
   10,000 seeds per level per architecture before a gate; 400 on every request.
2. **Per-pass tests.** Each pass has `.nir` input and expected-output files and a property test through the NIR
   interpreter.
3. **Encodings.** Every new encoding is checked against LLVM's assembler output (as A10 and A11 do).
4. **Corpus.** The Swift corpus (`languages/swift/Tests/Corpus`) must produce identical output at every level on
   both architectures.
5. **Determinism.** The same input compiled with 1 and with 8 workers gives identical bytes.

### 13.2 Performance

The benchmark piece owns the programs and the LLVM reference numbers. G1 adds a `klf bench-codegen` command that
prints, per level: generator time split by stage, TVISA instructions per second, code size, and (when the benchmark
directory is present) run-time ratios against its stored LLVM numbers.

### 13.3 Regression rule

A request that makes the geometric mean run time worse by more than 1 %, or generator throughput worse by more than
3 %, at any level, says so in its description and why; otherwise it does not land.

### 13.4 Gates for switching the default

| Gate | Condition | Effect |
| --- | --- | --- |
| A | Fuzzing and corpus pass at `-O0` and `-O1` on one architecture; `-O0` compile-speed target met | `optimizing` becomes the default for `-O0` and `-O1` on that architecture |
| B | Same at `-O2` / `-O3`; run time meets the "first gate" column of 2.1 | Default for all levels on that architecture |
| C | Gate B on both architectures for one week of the nightly sweep | Classic finalizers are deleted; A10 and A11 pieces end |

## 14. Implementation pieces

Six pieces, each with its own module directories. A piece edits only its own files. The public types named in
section 14.2 are the interfaces between pieces; G1 lands them first as compiling stubs (one request, same day), and
every other piece starts from that request.

### 14.1 Pieces and file ownership (paths under `languages/klf-swift/`)

| Piece | Owns | Content |
| --- | --- | --- |
| **G1 Core and driver** | `Sources/NativeIR/`, `Sources/NativeCodegen/`, `Tests/NativeIRTests/`, `Tests/NativeCodegenTests/`, `Tests/NativeTestSupport/`, the new-target lines of `Package.swift`, the `codeGenerator` switch in `Sources/KLFDriver/NativeBuilder.swift` and `KLFCommandLine.swift` | NIR data structures, builder, text form, verifier, analyses (4.3), TVISA reader with alias classes and call sites (stages 1 and 2), pass manager and level tables (6.1), per-function parallel driver and deterministic join, conservative-mode retry, timing output, `klf bench-codegen`, NIR interpreter, the differential fuzz harness |
| **G2 Scalar optimizer** | `Sources/NativeOptimizeScalar/`, `Tests/NativeOptimizeScalarTests/` | Rewrite table, `simplify`, `sccp`, `gvn`, `ranges`, `checks`, `branches`, `late` (6.2) |
| **G3 Loop and memory optimizer** | `Sources/NativeOptimizeLoops/`, `Tests/NativeOptimizeLoopsTests/` | `canonicalize`, `memory`, `licm`, `indvars`, `unroll`, `unswitch`, `tails`, `arc.fast` (6.3, 6.4) |
| **G4 Machine layer** | `Sources/NativeMachine/`, `Tests/NativeMachineTests/` | Legalization (7), MIR (8), `TargetBackend` protocol, selector driver for both tiers (9), the three allocators and move resolver (10), frame layout, peephole driver, block placement, branch relaxation, literal islands, table collection (12). Ships a tiny fake target for its own tests |
| **G5 arm64 back end** | `Sources/NativeARM64/`, `Tests/NativeARM64Tests/` | `TargetBackend` for arm64: registers, selection rules, conventions (ridl, c on Darwin and Linux), hardening (PAC, BTI, type tags, stack protector), encoders it lacks, peepholes, unwind rows, filling `ARM64Object` (15) |
| **G6 x86-64 back end** | `Sources/NativeX64/`, `Tests/NativeX64Tests/` | The same for x86-64 with the feature levels of TVISA section 30 (baseline, AVX2, AVX-512), filling `X64Object` (16) |

Module dependencies: `NativeIR` ← `NativeOptimizeScalar`, `NativeOptimizeLoops`, `NativeMachine`;
`NativeMachine` ← `NativeARM64` (+ `TIRCodegenARM64` for encoders and object types), `NativeX64` (+ `TIRCodegenX64`);
`NativeCodegen` imports all of them and is the only module `KLFDriver` imports.

Shared files and who may touch them: `Package.swift` — G1 only, in its first request (all seven targets and their
test targets declared at once, so no later piece needs it; A14 is told before it lands). `NativeBuilder.swift` — G1
only. Existing `TIRCodegenARM64` / `TIRCodegenX64` files — nobody in G1 to G6; a missing `public` on a reused type
is asked of A10 / A11.

### 14.2 Interfaces fixed by this design

```swift
// NativeIR (G1)
public struct NIRFunction: ~Copyable { /* arrays of section 4.1 */ }
public struct NIRBuilder { /* append, insert before/after, replace(value, with:), remove */ }
public protocol NIRPass { static var name: StaticString { get }
                          static func run(_ function: inout NIRFunction, _ context: inout PassContext) -> PassEffect }
public struct PassContext { /* level, target capabilities, cached analyses, scratch arena, diagnostics */ }
public struct PassEffect: OptionSet { /* changedInstructions, changedBlocks, changedMemory */ }

// NativeMachine (G4)
public protocol TargetBackend { /* section 8.2 */ }
public struct MachineFunction: ~Copyable { /* MIR arrays, frame, unwind rows, trap sites, line rows */ }
public enum MachinePipeline<Target: TargetBackend> {
    public static func compile(_ function: consuming NIRFunction, level: CodegenLevel,
                               target: borrowing Target) throws(CodegenError) -> EmittedFunction }
public struct EmittedFunction: Sendable { /* bytes, relocations, symbols, unwind, traps, lines */ }

// NativeCodegen (G1). Synchronous, like `NativeBuilder`; it runs its own worker threads inside.
public struct NativeCodeGenerator: Sendable {
    public init(level: CodegenLevel, workers: Int? = nil)
    public func finalizeARM64(_ program: TVISAProgram, platform: ARM64Platform, debug: ARM64DebugInput?)
        -> (object: ARM64Object, diagnostics: DiagnosticBag)
    public func finalizeX64(_ program: TVISAProgram, debug: DebugLineInput?)
        -> (object: X64Object, diagnostics: DiagnosticBag) }
```

`CodegenLevel` (`O0`, `O1`, `O2`, `O3`, `Os`) is NativeIR's own level type, so the generator does not depend on
`TIROptimize`; `NativeBuilder` maps `OptimizationLevel` to it by name. The stubs of all of these are on main.

G2 and G3 expose only `enum ScalarPasses` / `enum LoopPasses` with one `NIRPass` type per pass name in 6.1; the level
tables in `NativeCodegen` name them. Parallelism is one worker thread per core over function indices; no locks, no shared caches.

### 14.3 Order of work

All six start together from G1's stub request. Until G1's reader lands, G2 and G3 work from `.nir` text; until a
back end lands, G4 tests on its fake target; G5 and G6 test selection and encoding on hand-built NIR. First
end-to-end target: `-O0` on x86-64 Linux (runs directly on the build VM), then arm64, then `-O1`, then `-O2`/`-O3`.

## 15. arm64 notes (G5)

1. Registers: x0–x17 caller-saved pool (x16/x17 only as scratch for veneers and import calls), x19–x28 callee-saved
   minus x20 (context) and x21 (error) as TVISA section 9 fixes them, x18 reserved, x29/x30 frame record.
   v0–v7 and v16–v31 caller-saved; v8–v15 callee-saved low halves, so a 128-bit value live across a call is spilled
   or split, never kept there.
2. Rules beyond section 9: `madd`/`msub`/`smull`/`umulh`, `csel`/`csinc`/`csinv`/`cneg`/`cset`, `ccmp` chains,
   `cbz`/`cbnz`/`tbz`/`tbnz`, `ubfx`/`sbfx`/`bfi`, shifted and extended register operands, `adds`/`subs` flag reuse,
   logical immediates, `movz`/`movn`/`movk` sequences of minimal length, `ldp`/`stp` pairing (including callee
   saves and spill slots), post-indexed loads in pointer-induction loops, `fmadd`/`fnmsub`, `fcsel`, `fcvtzs` with
   saturation, NEON lanes for all `v128` operations, `ld1`/`st1`, `addv`/`umaxv` reductions, `tbl`.
3. Hardening is unchanged from A10's decisions: `paciasp`/`retaa`, `bti c`, type-tag word and check, optional signed
   code pointers, hosted stack protector. The frame code is new but must produce the same unwind row shapes
   `ARM64Unwind` encodes (compact encoding where the frame is standard).
4. Darwin and Linux `c` conventions both supported from the start (`ARM64Platform`).

## 16. x86-64 notes (G6)

1. Registers: rax, rcx, rdx, rsi, rdi, r8–r11 caller-saved; rbx, r14, r15 callee-saved; r12 (error) and r13
   (context) are reserved, as A11 has them; rbp frame pointer kept (unwinding and profilers), rsp. xmm0–xmm15 all
   caller-saved.
2. Two-address forms are handled as use-def operands with a move hint; the allocator's coalescing removes the move
   when the input dies there. Three-operand forms (`lea`, VEX with AVX, `imul r, r/m, imm`, BMI `andn`/`shlx`) are
   chosen when they save the move.
3. Rules beyond section 9: `lea` for add/shift/scale combinations, memory operands on arithmetic and compares
   (load folding when the load has one use and no store intervenes), `test` for and-with-zero compares, `cmov`,
   `setcc`, `bt`, `imul` by constant vs `lea` chains, `mulx`/`adcx` for wide multiply when BMI2, `popcnt`/`lzcnt`/
   `tzcnt` by feature, division by fixed registers rdx:rax as constraints, shifts by cl as a constraint,
   `movzx`/`movsx` only where 8.3 requires, zeroing with `xor`, `cmpxchg`/`lock` forms for atomics, SSE2 baseline
   and AVX2 / AVX-512 lanes by feature, `vzeroupper` before calls when 256-bit registers were used.
4. Fixed vectors live in registers (today's `X64Scalarize` path is not carried over); only operations the feature
   level lacks are expanded by legalization.
5. Linux ELF only for now, as A12 links x86-64 only as ELF (TIR section 32.2).

## 17. Decisions taken here (and the alternative not taken)

| Decision | Alternative | Reason |
| --- | --- | --- |
| Read TVISA, not TIR | A second TIR lowering inside the generator | 3.1 |
| One new low-level optimizer on NIR; A7 stays | Grow A7 to LLVM's scalar pass set on TIR | Lowering creates the work; TIR's model (strings, enums with payloads, per-pass module copies) cannot reach the speed target |
| Hand-written Swift rules for rewriting and selection | A rule language with a generator (ISLE, TableGen style) | No build-time tool to maintain; Swift `switch` compiles to jump tables; rules stay debuggable |
| Simple rewrite + GVN, no e-graph | E-graph rewriting, as TVISA 27.3 stage 4 names | E-graphs cost several times more compile time for small gains at this level; the spec's stage list is met in effect (folding, value numbering, loop-invariant motion on the legalized form). To be recorded in TVISA's decision log when accepted |
| Three allocators | One allocator with a quality knob | The `-O0` and `-O1` speed targets need algorithms with no splitting machinery at all |
| Undefined high bits for narrow integers | Always zero-extended (A10 today) | Removes most extension instructions |
| Caller-saved registers in the pool | Callee-saved only (A10, A11 today) | Leaf and call-free ranges stop paying saves; needed to match LLVM |
| Reuse A10/A11 encoders and object types | New encoders | They are checked against LLVM already; objects from both generators stay linkable together during the switch |
| Classic back ends kept until gate C | Replace in place | The corpus and every other piece keep working while the new path grows |
| No link-time work in the generator | Cross-function passes on NIR | TVISA 27.3 forbids crossing function boundaries in the finalizer; whole-module work is A7's, before lowering |

## 18. Open items owned by other pieces

1. A9: lowering of the five forms in 5.2; the facts of 5.1.
2. A13 (libtirrt): freeze the reference-count word layout for 6.4.
3. Benchmark piece: x86-64 Linux figures, and LLVM's back-end time alone, for section 2.3.
4. A7: `-O3` currently runs the `-O2` TIR set; profile-guided and link-time work (TIR 26.3) remain there.
5. KLF front end: the fixed start cost of about 390 ms per `klf build` (section 2.3) hides every back-end gain in
   whole-compile comparisons.
