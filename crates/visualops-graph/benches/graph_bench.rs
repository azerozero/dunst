//! Optional perf baseline (G8). Build/run with:
//!
//! ```sh
//! cargo bench -p visualops-graph --features bench
//! ```
//!
//! Gated behind the `bench` feature so `cargo test` never pulls criterion.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use visualops_core::mock::MockPerceptor;
use visualops_core::{Perceptor, Target};
use visualops_graph::{build_scene_graph, derive_affordances, RiskEngine};

fn pipeline(c: &mut Criterion) {
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = Target { pid: 1363, window_id: 105 };
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    let engine = RiskEngine::new();

    c.bench_function("build_scene_graph", |b| {
        b.iter(|| build_scene_graph(black_box(roots.clone()), window.clone(), 1_000))
    });

    let graph = build_scene_graph(roots.clone(), window.clone(), 1_000);

    c.bench_function("derive_affordances", |b| {
        b.iter(|| derive_affordances(black_box(&graph), &engine))
    });

    c.bench_function("assess_all_nodes", |b| {
        b.iter(|| {
            for node in graph.nodes.values() {
                black_box(engine.assess(node));
            }
        })
    });
}

criterion_group!(benches, pipeline);
criterion_main!(benches);
