#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use klf_services::{DefinitionArtifact, DefinitionUnit, QueryPhase, Session};

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

fn path(layout: &str, index: usize) -> String {
    if layout == "split" {
        format!("package-{index}/app/main.kira")
    } else {
        format!("monolith/app/module-{index}.kira")
    }
}

fn main() {
    println!(
        "layout,packages,sources,phase,wall_us,peak_alloc_delta,executions,cache_hits,invalidated,edges_visited,filesystem_ops,link_operations"
    );
    for count in [1, 10, 100, 1_000, 10_000] {
        run_layout("monolithic", 1, count);
        run_layout("split", count, count);
    }
}

fn run_layout(layout: &str, package_count: usize, source_count: usize) {
    let baseline = reset_peak();
    let start = Instant::now();
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("target"));
    let mut files = Vec::with_capacity(source_count);
    let mut units = Vec::with_capacity(source_count);
    for index in 0..source_count {
        let file = session.upsert_file(path(layout, index), "source");
        let unit = DefinitionUnit::new(file);
        session.register_unit(unit.clone());
        let query = session.begin_query("benchmark", &unit, QueryPhase::Parse);
        let artifact = DefinitionArtifact::new(&mut session, unit.clone(), index);
        session.cache_artifact("benchmark", QueryPhase::Parse, &artifact);
        session.finish_query(query);
        files.push(file);
        units.push(unit);
    }
    let graph = DefinitionUnit::from_files(files.clone()).expect("nonempty graph");
    session.register_unit(graph.clone());
    let graph_query = session.begin_derived_query("benchmark", &graph, QueryPhase::Elaborate);
    for unit in &units {
        assert!(
            session
                .cached_artifact("benchmark", unit, QueryPhase::Parse)
                .is_some()
        );
    }
    let artifact = DefinitionArtifact::new(&mut session, graph.clone(), source_count);
    session.cache_artifact("benchmark", QueryPhase::Elaborate, &artifact);
    session.finish_query(graph_query);
    let cold = session.query_stats();
    assert_eq!(cold.executions, source_count as u64 + 1);
    assert_eq!(cold.cache_hits, source_count as u64);
    report(
        layout,
        package_count,
        source_count,
        "cold",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        cold,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let before = session.query_stats();
    for unit in &units {
        assert!(
            session
                .cached_artifact("benchmark", unit, QueryPhase::Parse)
                .is_some()
        );
    }
    assert!(
        session
            .cached_artifact("benchmark", &graph, QueryPhase::Elaborate)
            .is_some()
    );
    let after = session.query_stats();
    assert_eq!(after.executions, before.executions);
    assert_eq!(
        after.cache_hits - before.cache_hits,
        source_count as u64 + 1
    );
    report(
        layout,
        package_count,
        source_count,
        "warm",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        after,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let before = session.query_stats();
    assert!(
        session
            .cached_artifact("benchmark", &graph, QueryPhase::Elaborate)
            .is_some()
    );
    let after = session.query_stats();
    assert_eq!(after.executions, before.executions);
    assert_eq!(after.cache_hits - before.cache_hits, 1);
    report(
        layout,
        package_count,
        source_count,
        "noop",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        after,
    );

    let baseline = reset_peak();
    let start = Instant::now();
    let before = session.query_stats();
    let changed_file = session.upsert_file(path(layout, source_count / 2), "edited");
    assert_eq!(changed_file, files[source_count / 2]);
    let invalidated = session.query_stats();
    assert_eq!(invalidated.invalidated - before.invalidated, 2);
    assert_eq!(
        invalidated.invalidation_edges_visited - before.invalidation_edges_visited,
        1
    );

    let query = session.begin_derived_query("benchmark", &graph, QueryPhase::Elaborate);
    for (index, unit) in units.iter().enumerate() {
        if session
            .cached_artifact("benchmark", unit, QueryPhase::Parse)
            .is_none()
        {
            let source_query = session.begin_query("benchmark", unit, QueryPhase::Parse);
            let artifact = DefinitionArtifact::new(&mut session, unit.clone(), index);
            session.cache_artifact("benchmark", QueryPhase::Parse, &artifact);
            session.finish_query(source_query);
        }
    }
    let artifact = DefinitionArtifact::new(&mut session, graph.clone(), source_count);
    session.cache_artifact("benchmark", QueryPhase::Elaborate, &artifact);
    session.finish_query(query);
    let incremental = session.query_stats();
    assert_eq!(incremental.executions - invalidated.executions, 2);
    assert_eq!(
        incremental.cache_hits - invalidated.cache_hits,
        source_count as u64 - 1
    );
    report(
        layout,
        package_count,
        source_count,
        "incremental-one-source",
        start.elapsed().as_micros(),
        peak_delta(baseline),
        incremental,
    );
}

fn report(
    layout: &str,
    packages: usize,
    sources: usize,
    phase: &str,
    wall_us: u128,
    peak_bytes: usize,
    stats: klf_services::QueryStats,
) {
    println!(
        "{layout},{packages},{sources},{phase},{wall_us},{peak_bytes},{},{},{},{},0,0",
        stats.executions, stats.cache_hits, stats.invalidated, stats.invalidation_edges_visited
    );
}
