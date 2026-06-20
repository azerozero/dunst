use std::{env, process};

use core_graphics::{
    event::CGEvent,
    event_source::{CGEventSource, CGEventSourceStateID},
};
use dunst_core::{
    ActionExecutor, Perceptor, RawAxNode, Role, SceneNode, SemanticAction, Source, Target,
};
use dunst_platform::MacosBackend;

fn main() {
    if let Err(err) = run() {
        eprintln!("noforeground_check failed: {err}");
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
    let raw = find_target(&roots).ok_or_else(|| usage("no AX target with bbox found"))?;
    let scene = scene_node(raw);
    let bbox = scene
        .bbox
        .ok_or_else(|| usage("selected AX target has no bbox"))?;
    let drop = format!(
        "{:.1},{:.1}",
        bbox.x + bbox.w / 2.0 + 12.0,
        bbox.y + bbox.h / 2.0
    );

    let before = cursor_location()?;
    backend.perform(&target, &scene, SemanticAction::Hover, None)?;
    let after_hover = cursor_location()?;
    backend.perform(&target, &scene, SemanticAction::Drag, Some(&drop))?;
    let after_drag = cursor_location()?;

    println!(
        "target role={} label={:?} bbox={:?}",
        scene.ax_role, scene.label, scene.bbox
    );
    println!("cursor_before=({:.1},{:.1})", before.x, before.y);
    println!(
        "cursor_after_hover=({:.1},{:.1})",
        after_hover.x, after_hover.y
    );
    println!(
        "cursor_after_drag=({:.1},{:.1})",
        after_drag.x, after_drag.y
    );
    println!(
        "cursor_delta_after_drag=({:.1},{:.1})",
        after_drag.x - before.x,
        after_drag.y - before.y
    );
    Ok(())
}

fn cursor_location() -> dunst_core::Result<core_graphics::geometry::CGPoint> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|err| usage(&format!("create cursor event source: {err:?}")))?;
    let event = CGEvent::new(source).map_err(|err| usage(&format!("read cursor: {err:?}")))?;
    Ok(event.location())
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
        id: "noforeground_check_target".into(),
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
        path: Vec::new(),
        parent: None,
        children: Vec::new(),
    }
}

fn usage(message: &str) -> dunst_core::VisualOpsError {
    dunst_core::VisualOpsError::Perception(format!(
        "{message}; usage: cargo run -p dunst-platform --example noforeground_check -- <pid> <window_id>"
    ))
}
