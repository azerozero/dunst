//! WP-D4 — **pure-pipeline** perf baseline.
//!
//! Drives `MockPerceptor::notes_fixture()` through the full device-free pipeline
//! — `build_scene_graph` -> `derive_affordances` (which assesses risk per node) —
//! with **no AX / IO**. This is the baseline the platform batch-read work (WP-C)
//! compares its capture latency against: WP-C measures capture/AX cost, this
//! measures the pure CPU cost of turning a captured tree into an affordance graph.
//!
//! Build/run with:
//!
//! ```sh
//! cargo bench -p dunst-graph --features bench
//! ```
//!
//! Gated behind the `bench` feature so `cargo test` never pulls criterion.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dunst_core::mock::MockPerceptor;
use dunst_core::{Perceptor, Target};
use dunst_graph::{build_scene_graph, derive_affordances, RiskEngine};

fn pipeline(c: &mut Criterion) {
    // Capture once, outside the timed loop: replaying the fixture is not part of
    // the pure pipeline cost (and stands in for WP-C's AX capture).
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = Target {
        pid: 1363,
        window_id: 105,
    };
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    let engine = RiskEngine::new();
    let graph = build_scene_graph(roots.clone(), window.clone(), 1_000);

    // Namespaced under `pipeline/…` so these ids never collide with another
    // bench target's (e.g. `graph_bench`) same-named functions.
    let mut group = c.benchmark_group("pipeline");

    // Headline baseline: the full pure pipeline end to end.
    group.bench_function("full", |b| {
        b.iter(|| {
            let graph = build_scene_graph(black_box(roots.clone()), window.clone(), 1_000);
            black_box(derive_affordances(black_box(&graph), &engine))
        })
    });

    // Per-stage breakdown, to attribute the baseline.
    group.bench_function("build_scene_graph", |b| {
        b.iter(|| build_scene_graph(black_box(roots.clone()), window.clone(), 1_000))
    });

    group.bench_function("derive_affordances", |b| {
        b.iter(|| derive_affordances(black_box(&graph), &engine))
    });

    group.bench_function("assess_all_nodes", |b| {
        b.iter(|| {
            for node in graph.nodes.values() {
                black_box(engine.assess(node));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, pipeline);
criterion_main!(benches);
