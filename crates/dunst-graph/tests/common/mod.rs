//! Shared fixtures for the `dunst-graph` integration tests.
//!
//! Each file under `tests/` compiles as its own crate, so this module is linked
//! into several test binaries that each exercise only a subset of it. The
//! module-level `allow(dead_code)` keeps the helpers that are unused *in one
//! binary* from tripping the `-D warnings` gate — `expect(dead_code)` would be
//! wrong here because other binaries do use them.
#![allow(dead_code)]

use dunst_core::mock::MockPerceptor;
use dunst_core::{Perceptor, RawAxNode, SceneGraph, Target};
use dunst_graph::build_scene_graph;

/// The [`Target`] addressing the captured Notes window in `fixtures/notes.json`.
pub fn notes_target() -> Target {
    Target {
        pid: 1363,
        window_id: 105,
    }
}

/// Flatten the device-free Notes fixture into a [`SceneGraph`] captured at
/// `now_ms`.
pub fn notes_graph(now_ms: u64) -> SceneGraph {
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = notes_target();
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    build_scene_graph(roots, window, now_ms)
}

/// `RawAxNode` builder for synthetic trees, with an optional developer-assigned
/// `ax_identifier`. The 3-argument builders in individual test files delegate
/// here with `ax_identifier = None`.
pub fn raw_node(
    role: &str,
    label: Option<&str>,
    ax_identifier: Option<&str>,
    children: Vec<RawAxNode>,
) -> RawAxNode {
    RawAxNode {
        ax_role: role.to_string(),
        label: label.map(str::to_string),
        help: None,
        value: None,
        ax_identifier: ax_identifier.map(str::to_string),
        cmd_char: None,
        cmd_modifiers: None,
        cmd_virtual_key: None,
        ax_actions: Vec::new(),
        frame: None,
        enabled: true,
        focused: false,
        children,
    }
}
