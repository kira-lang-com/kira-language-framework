use super::*;

pub(super) fn source(index: usize, package_count: usize) -> DefinitionSource {
    DefinitionSource::new(
        format!("app/source-{index:05}.klfd"),
        source_text(index, None, package_count),
    )
}

pub(super) fn source_text(
    source_index: usize,
    changed_function: Option<usize>,
    package_count: usize,
) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(FUNCTIONS_PER_SOURCE * 384);
    for offset in 0..FUNCTIONS_PER_SOURCE {
        let function = source_index * FUNCTIONS_PER_SOURCE + offset;
        let increment = if changed_function == Some(function) {
            2
        } else {
            1
        };
        let common_targets: [usize; COMMON_PACKAGE_COUNT] =
            std::array::from_fn(|index| common_target(index, package_count));
        let target_a = call_target_a(function);
        let target_scatter = call_target_scatter(function);
        writeln!(text, "function layer_{function:05}(depth: Int) -> Int {{")
            .expect("write to a string");
        if function == 0 {
            writeln!(text, "if depth == 0 {{ return {increment} }}").expect("write to a string");
        } else {
            writeln!(text, "if depth == 0 {{ return 1 }}").expect("write to a string");
        }
        writeln!(
            text,
            "if depth == 1 {{ return layer_{target_a:05}(depth - 1) + {increment} }}"
        )
        .expect("write to a string");
        for (depth, target) in common_targets.into_iter().enumerate() {
            writeln!(
                text,
                "if depth == {} {{ return layer_{target:05}(depth - 1) + {increment} }}",
                depth + 2,
            )
            .expect("write to a string");
        }
        writeln!(
            text,
            "if depth == 6 {{ return layer_{target_scatter:05}(depth - 1) + {increment} }}"
        )
        .expect("write to a string");
        writeln!(
            text,
            "return layer_{:05}(depth - 1) + {increment}",
            common_target(0, package_count),
        )
        .expect("write to a string");
        writeln!(text, "}}").expect("write to a string");
    }
    text
}

pub(super) fn package_name(index: usize) -> String {
    format!("package-{index:05}")
}

pub(super) fn indexed_graph(sources: &[DefinitionSource], package_count: usize) -> DefinitionGraph {
    let mut graph = DefinitionGraph::with_capacity(package_count, sources.len());
    for index in 0..package_count {
        let start = index * sources.len() / package_count;
        let end = (index + 1) * sources.len() / package_count;
        let package = DefinitionPackage::from_sources(sources[start..end].to_vec());
        let mut dependency_indices = Vec::with_capacity(8);
        for source_index in start..end {
            for offset in 0..FUNCTIONS_PER_SOURCE {
                let function = source_index * FUNCTIONS_PER_SOURCE + offset;
                let targets = [
                    call_target_a(function),
                    common_target(0, package_count),
                    common_target(1, package_count),
                    common_target(2, package_count),
                    common_target(3, package_count),
                    call_target_scatter(function),
                ];
                for target in targets {
                    let target_source = target / FUNCTIONS_PER_SOURCE;
                    let target_package = target_source * package_count / sources.len();
                    if target_package != index && !dependency_indices.contains(&target_package) {
                        dependency_indices.push(target_package);
                    }
                }
            }
        }
        dependency_indices.sort_unstable();
        let dependencies = dependency_indices.into_iter().map(package_name).collect();
        graph
            .add_package(
                package_name(index),
                format!("Module_{index:05}"),
                dependencies,
                &package,
            )
            .expect("unique package, module, and source identities");
    }
    assert_eq!(graph.source_count(), sources.len());
    graph
}

pub(super) fn call_target_a(function: usize) -> usize {
    (function + 1) % FUNCTION_COUNT
}

pub(super) fn call_target_scatter(function: usize) -> usize {
    (function * 13_007 + 31) % FUNCTION_COUNT
}

pub(super) fn common_target(index: usize, package_count: usize) -> usize {
    let package = index % package_count.min(COMMON_PACKAGE_COUNT);
    let first_source = package * SOURCE_COUNT / package_count;
    first_source * FUNCTIONS_PER_SOURCE
}

pub(super) fn call_path(mut function: usize, mut depth: usize, package_count: usize) -> Vec<usize> {
    let mut path = Vec::with_capacity(depth + 1);
    while depth > 0 {
        path.push(function);
        function = match depth {
            1 => call_target_a(function),
            2..=5 => common_target(depth - 2, package_count),
            6 => call_target_scatter(function),
            _ => common_target(0, package_count),
        };
        depth -= 1;
    }
    path.push(function);
    path
}

pub(super) fn query_delta(after: QueryStats, before: QueryStats) -> QueryStats {
    QueryStats {
        executions: after.executions - before.executions,
        cache_hits: after.cache_hits - before.cache_hits,
        invalidated: after.invalidated - before.invalidated,
        invalidation_edges_visited: after.invalidation_edges_visited
            - before.invalidation_edges_visited,
    }
}

pub(super) fn report(
    layout: &str,
    packages: usize,
    sources: usize,
    phase: &str,
    wall_us: u128,
    peak_bytes: usize,
    stats: QueryStats,
    filesystem_ops: usize,
    vm_executions: usize,
) {
    let backend_initializations = usize::from(phase.ends_with("vm-lower"));
    let compile_passes = usize::from(phase.ends_with("vm-lower"));
    println!(
        "{layout},{packages},{sources},{FUNCTION_COUNT},{phase},{wall_us},{peak_bytes},{},{},{},{},{filesystem_ops},0,{backend_initializations},{compile_passes},0,{vm_executions}",
        stats.executions, stats.cache_hits, stats.invalidated, stats.invalidation_edges_visited,
    );
}
