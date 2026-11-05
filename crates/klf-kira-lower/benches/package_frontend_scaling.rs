#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionGraph, DefinitionPackage, DefinitionSource, QueryStats, Session,
};

#[path = "package_frontend_scaling/support.rs"]
mod support;
use support::*;

const SOURCE_COUNT: usize = 10_000;
const FUNCTIONS_PER_SOURCE: usize = 2;
const FUNCTION_COUNT: usize = SOURCE_COUNT * FUNCTIONS_PER_SOURCE;
const COMMON_PACKAGE_COUNT: usize = 4;
const CALL_DEPTH: u64 = 64;

struct MeasuringAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: System receives the layout requested by the caller.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: pointer and layout are passed back unchanged from System.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointer and old layout came from this allocator.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

fn record_allocation(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

fn reset_peak() -> usize {
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
    baseline
}

fn peak_delta(baseline: usize) -> usize {
    PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(baseline)
}

fn target() -> klf_target::Target {
    "aarch64-apple-darwin".parse().expect("benchmark target")
}

fn main() {
    println!(
        "layout,packages,sources,functions,phase,wall_us,peak_alloc_delta,query_executions,cache_hits,invalidated,edges_visited,filesystem_ops,compiler_processes,backend_initializations,compile_passes,link_operations,vm_executions"
    );
    let mut monolithic_work = None;
    for count in [1, 10, 100, 1_000, 10_000] {
        let sources = (0..SOURCE_COUNT)
            .map(|index| source(index, count))
            .collect::<Vec<_>>();
        let layout = if count == 1 {
            "monolithic"
        } else {
            "unified-split"
        };
        let work = run_layout(layout, count, &sources);
        if let Some(expected) = monolithic_work {
            assert_eq!(
                work, expected,
                "package boundaries changed the generated KIR work"
            );
        } else {
            monolithic_work = Some(work);
        }
    }
    eprintln!("validated identical KIR work for all five package counts");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct KirWork {
    functions: usize,
    blocks: usize,
    instructions: usize,
}

fn kir_work(module: &klf_kir::Module) -> KirWork {
    KirWork {
        functions: module.functions.len(),
        blocks: module
            .functions
            .iter()
            .map(|function| function.blocks.len())
            .sum(),
        instructions: module
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .map(|block| block.instructions.len())
            .sum(),
    }
}

fn run_layout(layout: &str, count: usize, sources: &[DefinitionSource]) -> KirWork {
    let baseline = reset_peak();
    let start = Instant::now();
    let mut graph = indexed_graph(sources, count);
    report(
        layout,
        count,
        sources.len(),
        "graph-index-cold",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    graph
        .resolve_dependencies()
        .expect("all indexed package dependencies resolve");
    report(
        layout,
        count,
        sources.len(),
        "dependency-index-cold",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );
    if count > 1 {
        let common_count = count.min(COMMON_PACKAGE_COUNT);
        for index in 0..count {
            let dependent = graph
                .package_id(&package_name(index))
                .expect("package index");
            let next_dependency = graph
                .package_id(&package_name((index + 1) % count))
                .expect("cyclic dependency index");
            let dependencies = graph
                .dependencies(dependent)
                .expect("resolved package dependencies");
            assert!(dependencies.contains(&next_dependency));
            for common in 0..common_count {
                if index != common {
                    let common_dependency = graph
                        .package_id(&package_name(common))
                        .expect("common package index");
                    assert!(dependencies.contains(&common_dependency));
                }
            }
        }
    }
    let baseline = reset_peak();
    let start = Instant::now();
    let resolved = graph
        .packages()
        .iter()
        .map(|package| {
            graph
                .dependencies(package.id)
                .expect("resolved index")
                .len()
        })
        .sum::<usize>();
    std::hint::black_box(resolved);
    if count > 1 {
        assert!(
            resolved >= count,
            "package dependency graph lost a cycle edge"
        );
    } else {
        assert_eq!(resolved, 0);
    }
    assert_eq!(peak_delta(baseline), 0, "warm dependency lookup allocated");
    report(
        layout,
        count,
        sources.len(),
        "dependency-index-warm",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let mut session = Session::new(target());
    report(
        layout,
        count,
        sources.len(),
        "compiler-init",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let unit = graph
        .register_sources(&mut session)
        .expect("unified source graph has sources");
    report(
        layout,
        count,
        sources.len(),
        "source-registration-cold",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let before = session.query_stats();
    let baseline = reset_peak();
    let start = Instant::now();
    let parsed = KiraDefinition
        .parse(&mut session, unit.clone())
        .expect("generated Kira sources parse");
    let cold_parse = query_delta(session.query_stats(), before);
    assert_eq!(cold_parse.executions, sources.len() as u64 + 1);
    assert_eq!(cold_parse.cache_hits, 0);
    std::hint::black_box(parsed);
    report(
        layout,
        count,
        sources.len(),
        "cold-parse",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        cold_parse,
        0,
        0,
    );

    let before = session.query_stats();
    let baseline = reset_peak();
    let start = Instant::now();
    let parsed = KiraDefinition
        .parse(&mut session, unit.clone())
        .expect("cached Kira source parses");
    let elaborated = KiraDefinition
        .elaborate(&mut session, unit.clone(), &parsed)
        .expect("generated Kira functions type-check");
    let check = query_delta(session.query_stats(), before);
    assert_eq!(
        check.executions, 1,
        "check should elaborate one unified unit"
    );
    assert_eq!(check.cache_hits, 1, "check should reuse the parse result");
    report(
        layout,
        count,
        sources.len(),
        "check-after-parse",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        check,
        0,
        0,
    );

    let before = session.query_stats();
    let baseline = reset_peak();
    let start = Instant::now();
    let module = KiraDefinition
        .lower_shared(
            &mut session,
            unit.clone(),
            &elaborated,
            "PackageBenchmark",
            target(),
        )
        .expect("generated Kira functions lower to KIR");
    let build = query_delta(session.query_stats(), before);
    assert_eq!(
        build.executions,
        FUNCTION_COUNT as u64 + 1,
        "build should lower one unified module and one query per function"
    );
    assert_eq!(build.cache_hits, 0);
    let work = kir_work(&module);
    std::hint::black_box(&module);
    report(
        layout,
        count,
        sources.len(),
        "build-after-check",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        build,
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let vm_program =
        klf_vm::lower_module(&module).expect("generated KIR lowers to working VM code");
    report(
        layout,
        count,
        sources.len(),
        "vm-lower",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let entry = vm_program
        .function_index(&format!("layer_{:05}", FUNCTION_COUNT - 1))
        .expect("generated entry exists");
    let baseline = reset_peak();
    let start = Instant::now();
    let outcome = klf_vm::run(&vm_program, entry, &[CALL_DEPTH]);
    assert_eq!(
        outcome,
        klf_vm::Outcome::Return(Some(CALL_DEPTH + 1)),
        "layered Kira functions should execute calls across cyclic package dependencies"
    );
    report(
        layout,
        count,
        sources.len(),
        "vm-run",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        1,
    );

    let before = session.query_stats();
    let baseline = reset_peak();
    let start = Instant::now();
    let registered = graph
        .register_sources(&mut session)
        .expect("no-op source registration");
    assert_eq!(registered, unit);
    let parsed = KiraDefinition
        .parse(&mut session, registered.clone())
        .expect("no-op parse");
    let elaborated = KiraDefinition
        .elaborate(&mut session, registered.clone(), &parsed)
        .expect("no-op check");
    let no_op_module = KiraDefinition
        .lower_shared(
            &mut session,
            registered,
            &elaborated,
            "PackageBenchmark",
            target(),
        )
        .expect("no-op build");
    assert!(std::sync::Arc::ptr_eq(&module, &no_op_module));
    std::hint::black_box(no_op_module);
    let noop = query_delta(session.query_stats(), before);
    assert_eq!(noop.executions, 0, "no-op build executed compiler queries");
    assert_eq!(
        noop.cache_hits, 3,
        "no-op build should reuse all phase results"
    );
    assert!(
        peak_delta(baseline) < 1_024,
        "no-op build copied cached KIR"
    );
    report(
        layout,
        count,
        sources.len(),
        "no-op-build",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        noop,
        0,
        0,
    );

    let runtime_path = call_path(FUNCTION_COUNT - 1, CALL_DEPTH as usize, count);
    let changed_function = runtime_path[runtime_path.len() / 2];
    let changed_source = changed_function / FUNCTIONS_PER_SOURCE;
    let package_index = changed_source * count / SOURCE_COUNT;
    graph
        .replace_source(
            &package_name(package_index),
            &format!("app/source-{changed_source:05}.klfd"),
            source_text(changed_source, Some(changed_function), count),
        )
        .expect("indexed source identity");
    let before = session.query_stats();
    let baseline = reset_peak();
    let start = Instant::now();
    let edited_unit = graph
        .register_sources(&mut session)
        .expect("edited source registration");
    assert_eq!(
        edited_unit, unit,
        "editing content must preserve the unified unit"
    );
    let parsed = KiraDefinition
        .parse(&mut session, edited_unit.clone())
        .expect("edited Kira source parses");
    let elaborated = KiraDefinition
        .elaborate(&mut session, edited_unit.clone(), &parsed)
        .expect("edited Kira program type-checks");
    let edited_module = KiraDefinition
        .lower_shared(
            &mut session,
            edited_unit,
            &elaborated,
            "PackageBenchmark",
            target(),
        )
        .expect("edited Kira program lowers");
    let changed_name = format!("layer_{changed_function:05}");
    let reused_functions = module
        .functions
        .iter()
        .zip(&edited_module.functions)
        .filter(|(old, new)| {
            old.name.as_str().starts_with("layer_")
                && old.name.as_str() != changed_name
                && std::sync::Arc::ptr_eq(old, new)
        })
        .count();
    assert_eq!(
        reused_functions,
        FUNCTION_COUNT - 1,
        "one body edit should reuse every other function's KIR"
    );
    assert!(
        module
            .functions
            .iter()
            .zip(&edited_module.functions)
            .any(
                |(old, new)| old.name.as_str() == changed_name && !std::sync::Arc::ptr_eq(old, new)
            )
    );
    let incremental = query_delta(session.query_stats(), before);
    assert_eq!(
        incremental.executions, 5,
        "one source edit should parse one file, refresh unified queries, and lower one changed function"
    );
    assert_eq!(
        incremental.cache_hits,
        sources.len() as u64 - 1 + FUNCTION_COUNT as u64
    );
    let incremental_wall_us = start.elapsed().as_micros();
    let incremental_peak = peak_delta(baseline);
    report(
        layout,
        count,
        sources.len(),
        "incremental-one-source",
        incremental_wall_us,
        incremental_peak,
        incremental,
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let edited_program =
        klf_vm::lower_module(&edited_module).expect("edited KIR lowers to working VM code");
    let edited_entry = edited_program
        .function_index(&format!("layer_{:05}", FUNCTION_COUNT - 1))
        .expect("edited entry exists");
    report(
        layout,
        count,
        sources.len(),
        "incremental-vm-lower",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        0,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let edited_outcome = klf_vm::run(&edited_program, edited_entry, &[CALL_DEPTH]);
    assert_ne!(
        edited_outcome, outcome,
        "edited dependency must affect the call chain"
    );
    report(
        layout,
        count,
        sources.len(),
        "incremental-vm-run",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        QueryStats::default(),
        0,
        1,
    );
    work
}
