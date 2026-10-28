# Navigation VM benchmark

Target: maximum CPU duration of 5 ms, with no JIT. This target has not been met.

The benchmark runs the navigation app compiled with `kira`, on screen at
2560 × 1640 pixels. It records 180 frames after 30 warmup frames. The CPU
column measures elapsed time around the app frame handler and submission;
it can include graphics waits. The GPU column is residency, not active work.

## Workloads

Common environment:

```sh
KIRA_METAL_UNCAPPED=1
KIRA_METAL_ONSCREEN_BENCH=180
KIRA_METAL_WINDOW_STATE=1
KIRA_UI_SWIPE_SCRIPT='50,700,430,0,-460,23'
```

Navigation and scroll:

```sh
KIRA_UI_CLICK_SCRIPT='5@81,414;30@583,264'
```

Navigation, later toolbar interactions, and scroll:

```sh
KIRA_UI_CLICK_SCRIPT='5@81,414;30@583,264;90@1247,32;150@500,700'
```

Before **each** benchmark or profile, run from the KiraUI checkout:

```sh
set -e
./tools/stage-catalog.sh
cmp Resources/Default.kcui Examples/navigation-app/.kira-build/KiraUI.klbundle/resources/Resources/Default.kcui
shasum -a 256 Resources/Default.kcui Examples/navigation-app/.kira-build/KiraUI.klbundle/resources/Resources/Default.kcui
```

The catalog hash for these runs is
`ee065c25a21d83ac121de49743196f89166a3ba0aa155419261d70b1a093ce3d`.

## Intermediate measurements

All times below are CPU milliseconds from individual runs of navigation and
scroll. Compare the longer workload separately.

| VM change | p50 | p90 | max |
|---|---:|---:|---:|
| Register frame reuse | 3.036 | 98.109 | 143.545 |
| Floating operation predecode | 2.388 | 47.915 | 81.744 |
| Cached frame descriptors and scalar heap access | 1.868 | 40.590 | 72.804 |
| Scalar KIR locals in registers | 1.761 | 37.253 | 59.320 |
| Initial leaf inlining | 1.857 | 36.220 | 57.537 |
| Integer casts and direct scalar memory dispatch | 1.692 | 35.448 | 65.311 |
| Simplified straight-line helpers and fallthroughs | 1.614 | 29.863 | 43.565 |

Cranelift with speed optimization and the default register allocator measured
p50 0.564, p90 24.362, and max 44.962 on navigation and scroll. The longer
workload measured p50 12.224, p90 25.420, and max 42.094.

## Profiling

Instruments Time Profiler identified dispatch, register/frame traffic, and
scalar heap access as VM costs. Resolve both XML `frame` references and
`tagged-backtrace` references when aggregating exported samples.

The optional `klf-vm/execution-profile` Cargo feature records dispatch counts
by function and bytecode PC, then prints the hottest functions when the
machine is dropped. It is absent from normal builds. Its timings are not
benchmark results.

The longer workload before helper simplification executed 649,517,244
predecoded instructions. `createBatch` accounted for 69,240,438; its hottest
loop PCs ran 6,291,456 times. Inspection traced this to runtime allocation
zeroing emitted as word and byte loops. The frontend now lowers those fills,
and C image initialization, to KIR `mem.set` through the builder service.

## Bulk-fill lowering and helper passes

The full workload after bulk-fill lowering measured CPU p50 12.775,
p90 34.768, and max 92.573 ms on VM. Cranelift measured p50 9.191,
p90 24.051, and max 49.231 ms. These are separate uninstrumented runs.
The VM p90 ratio is 1.45; neither backend meets the 5 ms maximum.

The optional dispatch profile on the updated image recorded 491,664,577
instructions, down from 649,517,244. These profiles are frame driven and
animation sampling varies with execution speed, so the count reduction is
not a fixed-work speedup estimate. Allocation zeroing no longer leads the
profile; text batching, affine transforms, and layer descriptions do.

A Metal System Trace of the rebuilt Cranelift full workload recorded 75
next-drawable waits totaling 846.819 ms. Their p50 was 14.192 ms, p90
19.781 ms, and maximum 41.431 ms; 51 exceeded 5 ms. These are instrumented
wait intervals, not normal frame benchmark results. Drawable acquisition
occurs inside the timed handler, so compiler improvements alone cannot
remove this part of its elapsed time.

Local copy propagation now resets at every basic-block boundary and rewrites
branch conditions and edge sources before dead-register elimination. Liveness
is solved across successors, including backedges, while edge moves and
trapping operations remain intact. The full workload measured CPU p50
11.786, p90 31.510, and max 85.934 ms after this pass. The 48 scoped VM tests
pass, including loops, overwritten aliases, and comparison results on edges.

The short workload after control-flow optimization measured CPU p50 1.446,
p90 25.609, and max 37.022 ms. A subsequent full-workload dispatch profile
counted 430,105,230 instructions. Constant-plus-integer operations led with
66,065,234 dispatches, followed by 49,532,513 64-bit loads.

The decoder now combines a constant, address arithmetic, and a following
scalar load or store. This preserves intermediate register writes and leaves
all original PCs available for branch entry and metered execution. The full
workload measured CPU p50 11.026, p90 29.490, and max 83.909 ms. Direct
64-bit add/subtract/multiply/divide dispatch then measured p50 10.284,
p90 27.889, and max 83.408 ms. Both changes use static interpreter handlers.

The dispatch profiler also reports opcode counts and adjacent-PC count
estimates. Adjacent counts are the minimum of the two PC counters, not
recorded runtime transitions. They identify candidates for decoding passes.

Small fixed-size copies (1, 2, 4, or 8 bytes) lower to one scalar load and
store, preserving overlap by reading the entire value before writing it.
This also exposes temporary slots to promotion. The full workload measured
CPU p50 9.762, p90 26.002, and max 81.671 ms. All 51 scoped VM tests pass.
A separate per-frame trace locates its largest interaction cost at frame 91,
following the toolbar click at frame 90; tracing timings are diagnostic.

## Constant propagation and small call wrappers

Constant propagation now solves reachable blocks with a worklist, intersects
values at joins, widens loop backedges, and applies parallel edge moves from
the source state. Proven branches become unconditional while retaining an
integer comparison's result write. Unreachable blocks are removed. Constant
integer casts also fold without changing signed narrowing semantics.

The full workload with these changes measured CPU p50 10.078, p90 25.633,
and max 74.798 ms. A paired baseline measured p50 9.676, p90 26.058,
and max 83.129 ms. Small straight-line helpers may now contain ordinary or
indirect calls; indirect target registers are remapped, self-recursive
helpers and stack allocations remain excluded. That image measured CPU
p50 9.443, p90 25.130, and max 82.231 ms. Maximum latency still varies
substantially between runs. Twenty-two scoped parity and inlining tests pass.

A static ARM64 threaded interpreter was evaluated and removed after three
variants failed to improve the full workload. It generated no code at runtime.
The Rust interpreter remains the implementation used for these results.

Frame profiling can delimit samples with `KLF_VM_PROFILE_FRAME_BEGIN` and
`KLF_VM_PROFILE_FRAME_END`, naming entry functions in the image. For this app,
use `metalContextBeginFrameTiming` and `metalContextEndFrameTiming`. The feature
reports the eight slowest sampled frames and their ten leading functions.

The wrapper image's diagnostic profile recorded 379,877,568 dispatches.
Frame 91 executed 11,258,426; `ensureUniqueArrayData`, `quadIndices`,
`zeroedInts`, `ensureUniqueHeapBuffer`, and `metalCreateIndexBuffer` led that
frame. These counts identify array construction as the largest interaction
spike. Profile timings are excluded from normal benchmark comparisons.

## Proven array builders

The frontend recognizes an entire function that declares an empty array and
zero counter, appends zero once while the counter is below an integer
parameter, increments by one, and returns that array. With trivial elements,
it allocates once and emits `mem.set`. It preserves negative/empty counts and
the original `capacity = capacity * 2 + 4` growth result. The corpus case runs
on VM and Cranelift and checks values, capacity boundaries, subsequent
appends, copy-on-write, and rejected patterns.

The full workload with bulk zero arrays measured CPU p50 10.001, p90 25.102,
and max 57.447 ms on VM. Cranelift measured p50 15.488, p90 24.362,
and max 39.545 ms. Neither meets the 5 ms maximum.

For other pure builders, the frontend verifies scalar declarations and
appends whose values read only the counter and those declarations. It
preallocates the final capacity and writes directly, retaining checked
arithmetic and conversions. Calls, side effects, alternate counter steps,
and elements requiring cleanup keep ordinary lowering. This covers quad
indices. The diagnostic frame-91 count dropped from 11,258,426 to 6,273,269;
its leading functions are now `metalCreateIndexBuffer` and `quadIndices`.

Two normal full-workload runs measured p90 30.347 and 28.409, with maxima
65.551 and 73.753 ms. A saved bulk-zero baseline rerun measured p90 28.717
and max 69.110 ms. GPU residency also increased in these runs, so they do
not establish a wall-time gain for the general builder change. Dispatch
counts establish the removed work; more paired timing is needed.

## Further interpreter decoding

A fresh Time Profiler capture of the array-builder image sampled 3,053 ms
of main-thread CPU time. Dispatch attributed 662 ms of self time; frame
creation attributed 74 ms, and register/frame helpers remained prominent.
These are instrumented samples, including startup; inlined helper labels
are source attribution rather than proof of out-of-line calls.

Numeric `LoadK` values now resolve during decoding. The fast instruction
retains their exact integer/float bits and validates the destination.
Adjacent float multiply/add operations may share a dispatch, preserving
both register writes, separate rounding, original branch-entry PCs, and
metered instruction boundaries. This is not an FMA.

Branches without edge moves lower to direct decoded jumps when an edge is
fallthrough. Branches with moves and specialized numeric-loop plans retain
their behavior. Thirty-eight scoped decoding, parity, and inlining tests
pass. The full workload measured CPU p50 10.260, p90 25.285,
and maximum 66.540 ms with these decoding passes.

## Proven raw array copies

The frontend recognizes a borrowed scalar array's count, a fresh runtime
allocation of `count * elementWidth` bytes, and a zero counter followed by an
exact element-write loop. The source, count, and destination are immutable;
the loop must write the unchanged indexed element and advance by one.
Only matching runtime memory primitives and element widths qualify. The
loop becomes a guarded `mem.copy`, preserving the final counter and skipping
all source memory access for an empty array. Other strides and computed
values retain ordinary lowering. The corpus checks VM and Cranelift.

The direct branch decoder's short workload measured CPU p50 2.728, p90
18.387, and max 26.177 ms. The 5 ms maximum remains unmet.

With raw upload copies, the full workload measured CPU p50 9.316, p90
23.900, and max 43.881 ms on VM. The matching Cranelift frontend build
measured p50 12.315, p90 26.551, and max 67.905 ms, with GPU residency
p90 45.646 ms versus VM's 10.554 ms. These separate runs have substantial
presentation variance; they do not establish a native execution speed ratio.

The diagnostic profile reports 5,354,329 dispatches on frame 91, versus
6,273,269 before raw upload lowering. Index upload no longer appears in
that frame's ten leading functions. Quad construction still contributes
852,234 dispatches. Whole-run animation counts vary with frame elapsed time.

## Consecutive stack allocations

The decoder groups at least three consecutive small stack allocations into
one aligned arena reservation. Individual destinations retain their offsets
and all original interior PCs remain executable. Metered execution allocates
one original slot per step and retains its memory checks. Groups are bounded
to 64 KiB with power-of-two alignment up to 4 KiB; other allocations retain
the existing path. Eighteen scoped decoding tests pass, including alignment,
values, arena reuse, interior branch entry, and metering.

The full workload measured CPU p50 9.207, p90 23.199, and max 47.936 ms.
The p90 improved in this run, but the maximum increased; 5 ms remains unmet.

## Scalar struct construction

For a plain struct with at most sixteen scalar fields, no owned values, and
no layout padding, the frontend evaluates all fields into KIR values before
storing them into the destination. This preserves field evaluation order
and aliased inputs while eliminating the intermediate aggregate allocation
and copy. Padded layouts and nontrivial fields retain ordinary lowering.
The corpus checks aliasing, side-effect order, float fields, and padding on
both VM and Cranelift.

The full workload measured CPU p50 9.276, p90 22.631, and max 43.200 ms on
VM. Cranelift measured p50 11.755, p90 24.964, and max 53.526 ms. GPU
residency again differed substantially between runs. Neither meets 5 ms.

A fresh Instruments Time Profiler sample records 2,851 ms of main-thread
CPU, including startup. Leading self samples include interpreter dispatch
664 ms, register reads 99 ms, immediate handlers 76 ms, frame creation
70 ms, memory moves 66 ms, and heap access slow path 65 ms. Inlined helper
source attribution does not prove those helpers are out-of-line calls.

## Larger allocation-free helpers

The VM inliner now admits straight-line helpers up to 128 instructions.
It still rejects stack allocations, control flow, recursive direct calls,
and `noinline` declarations. Scalar constructor lowering makes transform
and color helpers eligible without extending any stack lifetime. Forty-one
scoped inlining, decoding, and parity tests pass.

The full workload measured CPU p50 9.543, p90 21.827, and max 43.069 ms.
This remains above the requested 5 ms maximum.

## Alternating heap accesses

A second cached live-allocation range avoids page and metadata lookups when
reads and writes alternate between two allocations. Every disposal path
clears both ranges. Five scoped runtime tests pass, including bounds,
release, raw free, native memory, arena reuse, and slot reuse.

The full workload measured CPU p50 9.486, p90 21.301, and max 48.006 ms.
The paired single-range runner on the same image measured p50 9.513,
p90 21.835, and max 53.779 ms. These remain individual runs with graphics
variance; neither meets the 5 ms limit.

## Pure construction loads

Within a scalar constructor whose expressions contain only scalar reads,
member reads, arithmetic, and conversions, repeated reads of the same local
field reuse one KIR value. Calls, conditional evaluation, arrays, and captured
cells disable this cache. The cache ends before any result stores. The
fixture checks repeated reads, mutation through calls, and both conditional
arms on VM and Cranelift. `caConcat` drops from 105 bytecode instructions to
69; no stack allocation is introduced.

The full workload measured CPU p50 9.666, p90 22.432, and max 42.804 ms on
VM, versus Cranelift p50 12.392, p90 24.296, and max 50.385 ms. Presentation
variance still dominates these individual runs.

## Guarded affine array builders

For eligible pure integer builders, the frontend proves every expression is
a nonnegative affine function of the counter, with Int or U32 values. It
computes a conservative iteration limit covering all intermediate results.
A single count guard selects arithmetic and conversions without per-element
range checks. Inputs beyond the limit execute the original checked loop.
The fixture checks empty and negative inputs, 4,096 quads, U32 boundaries,
and a successful fallback whose wide intermediate exceeds the fast bound.
Both VM and Cranelift pass.

The full workload measured CPU p50 9.433, p90 22.662, and max 44.954 ms on
VM, versus Cranelift p50 12.164, p90 26.173, and max 39.574 ms. This step
did not improve the VM's frame latency in that run. The 5 ms limit remains
unmet.

## Fixed builder buffer addresses

A proven pure builder never replaces its allocated output buffer. Its data
address is therefore loaded before the loop and reused by every append.
The loop counter starts at zero and advances only while below the original
nonnegative Int count; the append cursor is bounded by the checked final
length. Their increments cannot overflow, so both paths omit those checks.
The existing VM and Cranelift builder fixture passes.

The full workload measured CPU p50 9.469, p90 21.217, and max 42.900 ms on
VM; Cranelift measured p50 11.599, p90 26.156, and max 36.322 ms.
A thin-LTO, single-codegen-unit interpreter build measured p50 15.543,
p90 20.285, and max 48.314 ms. It improved p90 in that run while worsening
the median and maximum, so it is an experiment, not a verified broad win.

The diagnostic profiler can select functions with
`KLF_VM_PROFILE_FUNCTIONS=quadIndices,metalCreateIndexBuffer`. It reports
dispatches and entry-PC hits; the latter are not general call counts if a
backedge enters PC zero. Frame markers must survive inlining, and the
profiler now reports unavailable markers rather than implying frame data
was collected. Neither diagnostic timings nor dispatch counts replace the
normal on-screen benchmark.

The selected diagnostic profiles report three entry-PC hits for `quadIndices`
in each image. Dispatches fall from 1,278,351 before the guarded builder and
buffer hoist to 602,526 afterward. `metalCreateIndexBuffer` reports 165
dispatches and three entry-PC hits in both images, confirming bulk upload
copying remains intact. This comparison is function-specific; whole-run
animation counts are not fixed-work measurements.
