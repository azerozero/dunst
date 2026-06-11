use std::{env, process, time::Instant};

use dunst_core::{
    ActionExecutor, Perceptor, RawAxNode, Role, SceneNode, SemanticAction, Source, Target,
};
use dunst_platform::MacosBackend;

fn main() {
    if let Err(err) = run() {
        eprintln!("action_latency failed: {err}");
        process::exit(1);
    }
}

fn run() -> dunst_core::Result<()> {
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
    let target = Target { pid, window_id };
    let roots = backend.capture(&target)?;
    let raw = find_target(&roots).ok_or_else(|| usage("no measurable AX target found"))?;
    let scene = scene_node(raw);

    env::set_var("VO_ACTION_DISABLE_CACHE", "1");
    let started = Instant::now();
    backend.perform(&target, &scene, SemanticAction::Focus, None)?;
    let fallback_ms = started.elapsed().as_secs_f64() * 1_000.0;
    env::remove_var("VO_ACTION_DISABLE_CACHE");

    let started = Instant::now();
    backend.perform(&target, &scene, SemanticAction::Focus, None)?;
    let cached_ms = started.elapsed().as_secs_f64() * 1_000.0;

    eprintln!(
        "target role={} label={:?} bbox={:?}",
        scene.ax_role, scene.label, scene.bbox
    );
    eprintln!("fallback focus in {fallback_ms:.3} ms");
    eprintln!("cached focus in {cached_ms:.3} ms");
    Ok(())
}

fn find_target(nodes: &[RawAxNode]) -> Option<&RawAxNode> {
    nodes
        .iter()
        .find_map(|node| find_role(node, "AXTextArea"))
        .or_else(|| nodes.iter().find_map(|node| find_role(node, "AXWindow")))
}

fn find_role<'a>(node: &'a RawAxNode, role: &str) -> Option<&'a RawAxNode> {
    if node.ax_role == role && node.frame.is_some() {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_role(child, role))
}

fn scene_node(raw: &RawAxNode) -> SceneNode {
    SceneNode {
        id: "action_latency_target".into(),
        role: Role::Unknown,
        ax_role: raw.ax_role.clone(),
        label: raw.label.clone(),
        help: raw.help.clone(),
        value: raw.value.clone(),
        bbox: raw.frame,
        confidence: 1.0,
        source: Source::Accessibility,
        enabled: raw.enabled,
        focused: raw.focused,
        ax_actions: raw.ax_actions.clone(),
        ax_identifier: raw.ax_identifier.clone(),
        last_seen_ms: 0,
        parent: None,
        children: Vec::new(),
    }
}

fn usage(message: &str) -> dunst_core::VisualOpsError {
    dunst_core::VisualOpsError::Perception(format!(
        "{message}; usage: cargo run -p dunst-platform --example action_latency -- <pid> <window_id>"
    ))
}
