//! Platform backend boundary for Dunst MCP.
//!
//! The current implemented backend is macOS, but callers should branch on
//! grouped capabilities instead of scattering `target_os` checks across MCP
//! dispatch. OS-specific FFI stays behind this crate.
//!
//! macOS platform backend: the **real** [`Perceptor`] (AX tree walk) and
//! [`ActionExecutor`] (perform AX action / set value / CGEvent).
//!
//! This is the only crate that touches macOS FFI. See `docs/WP-A-platform.md`
//! for the full spec, the AX attribute list, and done-criteria.

use dunst_core::{
    ActionExecutor, Perceptor, RawAxNode, Result, SceneNode, SemanticAction, Target, WindowRef,
};

mod app_control;
mod capabilities;
mod clipboard;
mod file_chooser;

pub use app_control::{close_app, launch_app};
pub use capabilities::{
    AppCapabilities, ClipboardCapabilities, InputCapabilities, PerceptionCapabilities,
    PlatformCapabilities, PlatformKind, WindowCapabilities,
};
pub use clipboard::{
    paste_replace_field_foreground, paste_text_background, read_clipboard_bytes,
    write_clipboard_bytes,
};
#[cfg(target_os = "macos")]
pub use file_chooser::select_file_osascript_lines;
pub use file_chooser::{
    borrow_target_frontmost, click_menu_path, restore_frontmost_pid, select_file,
};

/// Returns the [`PlatformKind`] this crate was compiled for.
pub fn platform_kind() -> PlatformKind {
    capabilities::current_platform_kind()
}

/// Returns the current platform's grouped [`PlatformCapabilities`], probed at runtime.
pub fn platform_capabilities() -> PlatformCapabilities {
    capabilities::current_platform_capabilities()
}

/// AX-backed perception + action for macOS.
#[derive(Debug, Default)]
pub struct MacosBackend {
    _private: (),
}

impl MacosBackend {
    /// Creates a new macOS backend handle.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// Post a background click at a screen point to a macOS process without moving
/// the visible cursor.
///
/// # Errors
///
/// Returns an error if the user-active guard blocks the click (recent operator
/// input), the CoreGraphics event source cannot be created, the current cursor
/// position cannot be read, a mouse-down/up event cannot be created, or the
/// cursor cannot be restored to its saved position afterwards.
#[cfg(target_os = "macos")]
pub fn click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::click_at_point(pid, x, y)
}

/// Post a real-cursor left click at a screen point. Native popups (a
/// `<select>` menu, an open/save panel) live in separate windows — often
/// separate processes — that the PID-targeted click paths never reach; this
/// path briefly warps the cursor, posts a global HID click so it lands on
/// whatever window is under the point, then restores the cursor.
///
/// # Errors
///
/// Returns an error if the user-active guard blocks the click, the CoreGraphics
/// event source cannot be created, the current cursor position cannot be read,
/// warping the cursor to the target point fails, a left mouse-down/up event
/// cannot be created, or the cursor cannot be restored afterwards.
#[cfg(target_os = "macos")]
pub fn click_at_point_cursor(x: f64, y: f64) -> Result<()> {
    macos::click_at_point_cursor(x, y)
}

/// Post a real-cursor right-click at a screen point. Context menus on macOS
/// position from the real cursor, so this path briefly warps and restores it.
///
/// # Errors
///
/// Returns an error if the user-active guard blocks the click, the CoreGraphics
/// event source cannot be created, the current cursor position cannot be read,
/// warping the cursor to the target point fails, a right mouse-down/up event
/// cannot be created, or the cursor cannot be restored afterwards.
#[cfg(target_os = "macos")]
pub fn right_click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::right_click_at_point(pid, x, y)
}

/// Post a named keyboard key to a macOS window without touching the mouse.
///
/// # Errors
///
/// Returns an error if `key` is not a recognized key name, if the SkyLight
/// keyboard backend is unavailable, if the user-active guard blocks the press,
/// if a key event cannot be created, or if the SkyLight post is rejected.
#[cfg(target_os = "macos")]
pub fn press_key(pid: i32, window_id: u32, key: &str) -> Result<()> {
    macos::press_key(pid, window_id, key)
}

/// Time-multiplex the single OS cursor for a synthetic hover on a non-CDP
/// surface: save the current position, warp to `(x, y)`, and post a hover.
/// Returns the saved position to restore with [`cursor_restore`]. The hardware
/// mouse intentionally stays coupled because decoupling prevents web hover
/// events from reaching the window under the warped cursor. Keep the borrow
/// brief and always restore.
///
/// # Errors
///
/// Returns an error if the user-active guard blocks the borrow, if the
/// CoreGraphics event source cannot be created, if the current cursor position
/// cannot be read, if warping the cursor to the point fails, or if the hover
/// mouse-moved event cannot be created (in which case the cursor is restored
/// before returning).
#[cfg(target_os = "macos")]
pub fn cursor_borrow_to(x: f64, y: f64) -> Result<(f64, f64)> {
    macos::cursor_borrow_to(x, y)
}

/// Move an already-borrowed cursor without re-running the user-idle guard.
/// Callers must first use [`cursor_borrow_to`] and must restore with
/// [`cursor_restore`].
///
/// # Errors
///
/// Returns an error if the CoreGraphics event source cannot be created, if
/// warping the cursor to the point fails, or if the mouse-moved event cannot be
/// created.
#[cfg(target_os = "macos")]
pub fn cursor_borrow_move_to(x: f64, y: f64) -> Result<()> {
    macos::cursor_borrow_move_to(x, y)
}

/// End a [`cursor_borrow_to`]: release mouse buttons defensively, warp the
/// cursor back to `(x, y)`, and re-couple the hardware mouse so the user
/// controls it again.
///
/// # Errors
///
/// Returns an error if warping the cursor back to `(x, y)` fails. The defensive
/// button release and hardware re-coupling are best-effort and never surface an
/// error.
#[cfg(target_os = "macos")]
pub fn cursor_restore(x: f64, y: f64) -> Result<()> {
    macos::cursor_restore(x, y)
}

/// Unstick the OS cursor after driving a backgrounded window: drives a menu-bar
/// focus cycle (open + close the Apple menu) so the window server re-evaluates
/// the cursor shape. Workaround for the macOS stuck-cursor bug. macOS-only.
///
/// # Errors
///
/// Returns an error if the CoreGraphics event source cannot be created, if the
/// current cursor position cannot be read, or if either mouse-down/up event of
/// the Apple-menu click cannot be created.
#[cfg(target_os = "macos")]
pub fn unstick_cursor() -> Result<()> {
    macos::unstick_cursor()
}

/// Idle-gated cursor unstick for AUTOMATIC recovery: same maneuver as
/// `unstick_cursor`, but returns the user-active-guard error while the operator
/// is active, so callers can wrap it in the idle retry loop and never fight a
/// user who resumed control. Use `unstick_cursor` for operator-requested,
/// immediate recovery. macOS-only.
///
/// # Errors
///
/// Returns the user-active-guard error while the operator is active; otherwise
/// returns an error if the CoreGraphics event source cannot be created, if the
/// current cursor position cannot be read, or if either mouse-down/up event of
/// the Apple-menu click cannot be created.
#[cfg(target_os = "macos")]
pub fn unstick_cursor_if_idle() -> Result<()> {
    macos::unstick_cursor_if_idle()
}

/// Non-macOS stub.
///
/// # Errors
///
/// Always returns an error: cursor recovery requires a macOS backend.
#[cfg(not(target_os = "macos"))]
pub fn unstick_cursor_if_idle() -> Result<()> {
    Err(dunst_core::DunstError::Execution(
        "unstick_cursor_if_idle requires a macOS backend".into(),
    ))
}

/// Whether the current process has macOS Accessibility permission.
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    macos::accessibility_trusted()
}

/// Whether the current process has macOS Screen Recording permission.
#[cfg(target_os = "macos")]
pub fn screen_capture_trusted() -> bool {
    macos::screen_capture_trusted()
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
///
/// # Errors
///
/// Returns an error if Accessibility permission is not granted, if the AX
/// application element cannot be created for `pid`, if the requested window id
/// cannot be resolved (e.g. the window was closed), or if writing the AXSize or
/// AXPosition attribute is rejected by the app.
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
///
/// # Errors
///
/// Always returns an error: moving or resizing a window requires a macOS backend.
#[cfg(not(target_os = "macos"))]
pub fn set_window_frame(
    _pid: i32,
    _window_id: u32,
    _x: f64,
    _y: f64,
    _width: Option<f64>,
    _height: Option<f64>,
) -> Result<()> {
    Err(dunst_core::DunstError::Execution(
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
///
/// # Errors
///
/// Returns an error if the SkyLight backend is unavailable (a zero window id or
/// the mouse-post SPI missing), if the user-active guard blocks the hover, if
/// the hover event cannot be created, or if the SkyLight post is rejected.
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

/// Post a wheel-scroll event to a backgrounded web window at a concrete screen
/// point. `delta_y` follows CoreGraphics convention: positive scrolls up,
/// negative scrolls down.
///
/// # Errors
///
/// Returns an error if the SkyLight backend is unavailable (a zero window id or
/// the mouse-post SPI missing), if the user-active guard blocks the scroll, if
/// the CoreGraphics event source or scroll event cannot be created, or if the
/// SkyLight post is rejected.
#[cfg(target_os = "macos")]
pub fn scroll_web_background(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
    delta_y: i32,
) -> Result<()> {
    macos::scroll_web_background(pid, window_id, x, y, origin_x, origin_y, delta_y)
}

/// Borrow the real OS cursor, move it to `(x, y)`, post a global wheel-scroll
/// event, and restore the cursor. This is a generic fallback for visible native,
/// web, Electron, and canvas surfaces that only respond to real pointer wheel
/// input. `delta_y` follows CoreGraphics convention: positive scrolls up,
/// negative scrolls down.
///
/// # Errors
///
/// Returns an error if the user-active guard blocks the scroll, if the
/// CoreGraphics event source cannot be created, if the current cursor position
/// cannot be read, if warping the cursor to the point fails, if the pre-scroll
/// hover or wheel-scroll event cannot be created, or if the cursor cannot be
/// restored afterwards.
#[cfg(target_os = "macos")]
pub fn scroll_at_point(x: f64, y: f64, delta_y: i32) -> Result<()> {
    macos::scroll_at_point(x, y, delta_y)
}

/// Type `text` into the focused element of a **backgrounded** window's (web)
/// content via SkyLight — trusted (auth-signed), no cursor, no foreground. The
/// caller should first focus the field (e.g. a [`click_web_background`] on it).
/// Fails if SkyLight is unavailable or any expected key event cannot be created
/// and posted.
///
/// # Errors
///
/// Returns an error if the SkyLight backend is unavailable, if `window_id` is
/// zero, if the user-active guard blocks the input, or if any per-character or
/// Return key event cannot be created and posted. Multi-line `text` is routed
/// through a clipboard paste that fails if the clipboard cannot be read/written
/// or restored.
#[cfg(target_os = "macos")]
pub fn type_text_background(pid: i32, window_id: u32, text: &str) -> Result<()> {
    macos::type_text_background(pid, window_id, text)
}

/// Replace the text of the FOCUSED field in `pid` by setting the app's
/// `AXFocusedUIElement` value directly (AX select-all-replace, with a keyboard
/// fallback). Robust against the erratic cursor of raw clear-by-keystroke
/// (End/Backspace), even for sparse-AX web inputs absent from the scene graph.
/// Focus the field first (e.g. a click on it).
///
/// # Errors
///
/// Returns an error if the AX application element cannot be created for `pid`,
/// if no element currently holds keyboard focus (`AXFocusedUIElement` is
/// absent), or if the foreground select-all-and-paste replacement fails.
#[cfg(target_os = "macos")]
pub fn set_focused_field_text(pid: i32, window_id: u32, text: &str) -> Result<()> {
    macos::set_focused_field_text(pid, window_id, text)
}

/// Post a named keycode (down+up) with optional modifier `flags` (CGEventFlags
/// bits: Shift 0x20000, Control 0x40000, Alternate 0x80000, Command 0x100000) to
/// a **backgrounded** window's (web) content via the SkyLight auth-signed keyboard
/// path — for scrolling (Page/Home/End), zoom (Cmd =/-/0), and hotkeys (Cmd+L,
/// Cmd+T, …). Fails if SkyLight is unavailable or any expected key event cannot
/// be created and posted.
///
/// # Errors
///
/// Returns an error if the SkyLight backend is unavailable, if the user-active
/// guard blocks the key, if a key event cannot be created, or if the SkyLight
/// post is rejected.
#[cfg(target_os = "macos")]
pub fn key_web_background(pid: i32, window_id: u32, keycode: u16, flags: u64) -> Result<()> {
    macos::key_web_background(pid, window_id, keycode, flags)
}

/// AXRaise the exact window identified by its CoreGraphics `window_id`, making
/// it the app's key window (window-scoped via `_AXUIElementGetWindow`, robust
/// to duplicate/volatile titles). Use before a menu-bar command so it targets
/// the attached window and not whichever window of the app is currently key.
///
/// # Errors
///
/// Returns an error if the AX application element cannot be created for `pid`,
/// if the requested window id cannot be resolved (e.g. the window was closed),
/// or if the AXRaise action is rejected by the app.
#[cfg(target_os = "macos")]
pub fn raise_window_by_id(pid: i32, window_id: u32) -> Result<()> {
    macos::raise_window_by_id(pid, window_id)
}

/// Non-macOS stub.
///
/// # Errors
///
/// Always returns an error: raising a window requires a macOS backend.
#[cfg(not(target_os = "macos"))]
pub fn raise_window_by_id(_pid: i32, _window_id: u32) -> Result<()> {
    Err(dunst_core::DunstError::Execution(
        "raise_window_by_id requires a macOS backend".into(),
    ))
}

/// Fingerprint of the current global cursor image; `None` if unreadable. The
/// same pointer shape hashes identically, so comparing the fingerprint before
/// and after a borrowed-cursor gesture (at the same resting point) tells
/// whether the pointer was left stuck in a shape it did not have before.
#[cfg(target_os = "macos")]
pub fn cursor_shape_fingerprint() -> Option<u64> {
    macos::cursor_shape_fingerprint()
}

/// Non-macOS stub.
#[cfg(not(target_os = "macos"))]
pub fn cursor_shape_fingerprint() -> Option<u64> {
    None
}

/// Hit-test the AX element under a global screen point and return a shallow raw
/// snapshot. This is the AX-side primitive for region analysis by sampling a
/// spaced grid of points; macOS does not expose a direct "subtree by rectangle"
/// API.
///
/// # Errors
///
/// Returns an error if Accessibility permission is not granted, if the AX
/// application element cannot be created for `pid`, or if the AX hit-test at
/// `(x, y)` fails or resolves no element.
#[cfg(target_os = "macos")]
pub fn element_at_point(pid: i32, x: f64, y: f64) -> Result<RawAxNode> {
    macos::element_at_point(pid, x, y)
}

/// Non-macOS stub.
///
/// # Errors
///
/// Always returns an error: AX hit-testing requires a macOS backend.
#[cfg(not(target_os = "macos"))]
pub fn element_at_point(_pid: i32, _x: f64, _y: f64) -> Result<RawAxNode> {
    Err(dunst_core::DunstError::Perception(
        "element_at_point requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
mod macos {
    use dunst_core::{DunstError, RawAxNode, Result, SceneNode, SemanticAction, Target, WindowRef};

    pub fn capture(_target: &Target) -> Result<Vec<RawAxNode>> {
        Err(DunstError::Perception(
            "macOS accessibility backend is only available on macOS".into(),
        ))
    }

    pub fn window_ref(target: &Target) -> Result<WindowRef> {
        Err(DunstError::Perception(format!(
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
        Err(DunstError::Execution(
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
