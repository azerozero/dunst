use std::{env, process, time::Instant};

use visualops_core::{Perceptor, Target};
use visualops_platform::MacosBackend;

fn main() {
    if let Err(err) = run() {
        eprintln!("dump failed: {err}");
        process::exit(1);
    }
}

fn run() -> visualops_core::Result<()> {
    let mut args = env::args().skip(1);
    let pid = args
        .next()
        .ok_or_else(|| usage("missing pid"))?
        .parse::<i32>()
        .map_err(|err| usage(&format!("invalid pid: {err}")))?;
    let window_id = args
        .next()
        .ok_or_else(|| usage("missing window_id"))?
        .parse::<u32>()
        .map_err(|err| usage(&format!("invalid window_id: {err}")))?;

    if args.next().is_some() {
        return Err(usage("too many arguments"));
    }

    let backend = MacosBackend::new();
    let started = Instant::now();
    let nodes = backend.capture(&Target { pid, window_id })?;
    if env::var_os("VO_DUMP_TIMING").is_some() {
        eprintln!(
            "captured {} nodes in {:.3} ms",
            count_nodes(&nodes),
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(())
}

fn count_nodes(nodes: &[visualops_core::RawAxNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + count_nodes(&node.children))
        .sum()
}

fn usage(message: &str) -> visualops_core::VisualOpsError {
    visualops_core::VisualOpsError::Perception(format!(
        "{message}; usage: cargo run -p visualops-platform --example dump -- <pid> <window_id>"
    ))
}
