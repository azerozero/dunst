//! macOS platform backend: the **real** [`Perceptor`] (AX tree walk) and
//! [`ActionExecutor`] (perform AX action / set value / CGEvent).
//!
//! This is the only crate that touches macOS FFI. See `docs/WP-A-platform.md`
//! for the full spec, the AX attribute list, and done-criteria.

use dunst_core::{
    ActionExecutor, Perceptor, RawAxNode, Result, SceneNode, SemanticAction, Target, WindowRef,
};

/// AX-backed perception + action for macOS.
#[derive(Debug, Default)]
pub struct MacosBackend {
    _private: (),
}

impl MacosBackend {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// Post a background click at a screen point to a macOS process without moving
/// the visible cursor.
#[cfg(target_os = "macos")]
pub fn click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::click_at_point(pid, x, y)
}

/// Post a named keyboard key to a macOS window without touching the mouse.
#[cfg(target_os = "macos")]
pub fn press_key(pid: i32, window_id: u32, key: &str) -> Result<()> {
    macos::press_key(pid, window_id, key)
}

/// Trigger a real cursor hover at a screen point so non-web surfaces can reveal
/// hover state. This can move the visible cursor; web callers should prefer
/// [`hover_web_background`] when they need a cursorless probe.
#[cfg(target_os = "macos")]
pub fn hover_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::hover_at_point(pid, x, y)
}

/// Time-multiplex the single OS cursor for a synthetic hover on a non-CDP
/// surface: save the current position, **decouple the hardware mouse** (so the
/// user can't fight the warp), warp to `(x, y)` and post a hover. Returns the
/// saved position to restore with [`cursor_restore`]. The user's mouse is frozen
/// until restore — keep the borrow brief (~tens of ms).
#[cfg(target_os = "macos")]
pub fn cursor_borrow_to(x: f64, y: f64) -> Result<(f64, f64)> {
    macos::cursor_borrow_to(x, y)
}

/// End a [`cursor_borrow_to`]: warp the cursor back to `(x, y)` and **re-couple**
/// the hardware mouse so the user controls it again.
#[cfg(target_os = "macos")]
pub fn cursor_restore(x: f64, y: f64) -> Result<()> {
    macos::cursor_restore(x, y)
}

/// Whether the current process has macOS Accessibility permission.
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    macos::accessibility_trusted()
}

/// Make `window_id`'s app **AppKit-active without raising it or switching Spaces**
/// (SkyLight focus-without-raise, the recipe cua-driver ports from yabai). A
/// backgrounded web canvas (e.g. a chart) only paints when its window is active,
/// so this lets it render before a capture — without foregrounding. Returns
/// `false` if the private SkyLight SPIs don't resolve (best-effort, no-op
/// fallback).
#[cfg(target_os = "macos")]
pub fn focus_without_raise(window_id: u32) -> bool {
    macos::focus_without_raise(window_id)
}

/// Move/resize a target window by writing its AXPosition/AXSize attributes.
/// Coordinates are global macOS screen points. Passing `None` for width/height
/// preserves that dimension.
#[cfg(target_os = "macos")]
pub fn set_window_frame(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<()> {
    macos::set_window_frame(pid, window_id, x, y, width, height)
}

/// Non-macOS stub.
#[cfg(not(target_os = "macos"))]
pub fn set_window_frame(
    _pid: i32,
    _window_id: u32,
    _x: f64,
    _y: f64,
    _width: Option<f64>,
    _height: Option<f64>,
) -> Result<()> {
    Err(dunst_core::VisualOpsError::Execution(
        "set_window_frame requires a macOS backend".into(),
    ))
}

/// Click a **backgrounded / occluded** window's (web) content at a screen point
/// via SkyLight — trusted, no cursor move, no foreground. `window_origin` is the
/// window's top-left in screen points (for the window-local coordinate the gate
/// needs). Returns `false` if SkyLight is unavailable so the caller can fall back
/// to a cursor click.
#[cfg(target_os = "macos")]
pub fn click_web_background(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
    button: u8,
) -> bool {
    macos::click_web_background(pid, window_id, x, y, origin_x, origin_y, button)
}

/// Post a background mouse-move (hover) to a web window via SkyLight without
/// moving the visible cursor. `window_origin` is the window's top-left in screen
/// points so the event can be stamped with the window-local coordinate.
#[cfg(target_os = "macos")]
pub fn hover_web_background(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
) -> Result<()> {
    macos::hover_web_background(pid, window_id, x, y, origin_x, origin_y)
}

/// Type `text` into the focused element of a **backgrounded** window's (web)
/// content via SkyLight — trusted (auth-signed), no cursor, no foreground. The
/// caller should first focus the field (e.g. a [`click_web_background`] on it).
/// Fails if SkyLight is unavailable or any expected key event cannot be created
/// and posted.
#[cfg(target_os = "macos")]
pub fn type_text_background(pid: i32, window_id: u32, text: &str) -> Result<()> {
    macos::type_text_background(pid, window_id, text)
}

/// Post a named keycode (down+up) with optional modifier `flags` (CGEventFlags
/// bits: Shift 0x20000, Control 0x40000, Alternate 0x80000, Command 0x100000) to
/// a **backgrounded** window's (web) content via the SkyLight auth-signed keyboard
/// path — for scrolling (Page/Home/End), zoom (Cmd =/-/0), and hotkeys (Cmd+L,
/// Cmd+T, …). Fails if SkyLight is unavailable or any expected key event cannot
/// be created and posted.
#[cfg(target_os = "macos")]
pub fn key_web_background(pid: i32, window_id: u32, keycode: u16, flags: u64) -> Result<()> {
    macos::key_web_background(pid, window_id, keycode, flags)
}

/// Hit-test the AX element under a global screen point and return a shallow raw
/// snapshot. This is the AX-side primitive for region analysis by sampling a
/// spaced grid of points; macOS does not expose a direct "subtree by rectangle"
/// API.
#[cfg(target_os = "macos")]
pub fn element_at_point(pid: i32, x: f64, y: f64) -> Result<RawAxNode> {
    macos::element_at_point(pid, x, y)
}

/// Non-macOS stub.
#[cfg(not(target_os = "macos"))]
pub fn element_at_point(_pid: i32, _x: f64, _y: f64) -> Result<RawAxNode> {
    Err(dunst_core::VisualOpsError::Perception(
        "element_at_point requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
mod macos {
    use dunst_core::{
        RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
    };

    pub fn capture(_target: &Target) -> Result<Vec<RawAxNode>> {
        Err(VisualOpsError::Perception(
            "macOS accessibility backend is only available on macOS".into(),
        ))
    }

    pub fn window_ref(target: &Target) -> Result<WindowRef> {
        Err(VisualOpsError::Perception(format!(
            "macOS accessibility backend is only available on macOS (pid={}, window_id={})",
            target.pid, target.window_id
        )))
    }

    pub fn perform(
        _target: &Target,
        _node: &SceneNode,
        _action: SemanticAction,
        _argument: Option<&str>,
    ) -> Result<()> {
        Err(VisualOpsError::Execution(
            "macOS accessibility backend is only available on macOS".into(),
        ))
    }
}

impl Perceptor for MacosBackend {
    fn capture(&self, target: &Target) -> Result<Vec<RawAxNode>> {
        macos::capture(target)
    }

    fn window_ref(&self, target: &Target) -> Result<WindowRef> {
        macos::window_ref(target)
    }
}

impl ActionExecutor for MacosBackend {
    fn perform(
        &self,
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()> {
        macos::perform(target, node, action, argument)
    }
}
