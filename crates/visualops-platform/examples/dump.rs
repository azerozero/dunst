use std::{env, process};

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
    let nodes = backend.capture(&Target { pid, window_id })?;
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(())
}

fn usage(message: &str) -> visualops_core::VisualOpsError {
    visualops_core::VisualOpsError::Perception(format!(
        "{message}; usage: cargo run -p visualops-platform --example dump -- <pid> <window_id>"
    ))
}
