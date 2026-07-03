//! Grouped, runtime-probed description of what the platform backend can do.
//!
//! Callers branch on these capability groups instead of scattering `target_os`
//! checks across MCP dispatch.

use serde::{Deserialize, Serialize};

/// Operating-system family the backend runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    /// Apple macOS.
    Macos,
    /// Linux.
    Linux,
    /// Microsoft Windows.
    Windows,
    /// An operating system the crate does not recognize.
    Unknown,
}

/// Grouped capabilities the current platform backend advertises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    /// Operating-system family this report describes.
    pub kind: PlatformKind,
    /// Synthetic-input and focus capabilities.
    pub input: InputCapabilities,
    /// Clipboard read/write capabilities.
    pub clipboard: ClipboardCapabilities,
    /// Perception (AX tree, screenshots, OCR, vision) capabilities.
    pub perception: PerceptionCapabilities,
    /// Window listing and manipulation capabilities.
    pub windows: WindowCapabilities,
    /// Application launch/list/close capabilities.
    pub apps: AppCapabilities,
}

/// Synthetic-input and focus capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCapabilities {
    /// Whether AX actions (press, set value, …) can drive other apps.
    pub accessibility_actions: bool,
    /// Whether pointer events can be posted to backgrounded windows.
    pub background_pointer: bool,
    /// Whether keyboard events can be posted to backgrounded windows.
    pub background_keyboard: bool,
    /// Whether modifier hotkeys can be posted to backgrounded windows.
    pub background_hotkeys: bool,
    /// Whether a window's app can be activated without raising it.
    pub focus_without_raise: bool,
    /// Whether the real OS cursor can be borrowed and restored.
    pub real_cursor_borrow: bool,
    /// Whether native menu-bar items can be driven.
    pub menu_bar: bool,
}

/// Clipboard read/write capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardCapabilities {
    /// Whether clipboard text can be read.
    pub text_read: bool,
    /// Whether clipboard text can be written.
    pub text_write: bool,
    /// Whether rich (non-plain-text) clipboard formats survive round-trips.
    pub rich_formats_preserved: bool,
}

/// Perception capabilities: accessibility tree and pixel-based sensing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerceptionCapabilities {
    /// Whether the accessibility tree can be walked.
    pub accessibility_tree: bool,
    /// Whether the screen can be captured to pixels.
    pub screenshots: bool,
    /// Whether captured pixels can be OCR'd.
    pub ocr: bool,
    /// Whether captured pixels can be analyzed for shapes.
    pub vision_shapes: bool,
    /// Whether captured pixels can be scanned for charts.
    pub chart_scan: bool,
}

/// Window listing and manipulation capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCapabilities {
    /// Whether on-screen windows can be enumerated.
    pub list: bool,
    /// Whether window visibility can be queried.
    pub visibility: bool,
    /// Whether windows can be moved and resized.
    pub move_resize: bool,
    /// Whether windows can be arranged (tiled/positioned in bulk).
    pub arrange: bool,
    /// Whether windows can be exposed (Exposé-style reveal).
    pub expose: bool,
}

/// Application launch/list/close capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppCapabilities {
    /// Whether running applications can be listed.
    pub list_running: bool,
    /// Whether launchable (installed) applications can be listed.
    pub list_launchable: bool,
    /// Whether metadata about an application can be queried.
    pub app_info: bool,
    /// Whether applications can be launched.
    pub launch: bool,
    /// Whether URLs can be opened in an application.
    pub open_url: bool,
    /// Whether applications can be asked to quit.
    pub close: bool,
    /// Whether the native file-chooser dialog can be driven.
    pub file_chooser: bool,
}

impl PlatformCapabilities {
    /// Reports whether any UI-mutating capability (AX actions, background input,
    /// move/resize, or launch) is available.
    pub fn can_mutate_ui(&self) -> bool {
        self.input.accessibility_actions
            || self.input.background_pointer
            || self.input.background_keyboard
            || self.windows.move_resize
            || self.apps.launch
    }

    /// Reports whether any pixel-perception path (OCR, vision shapes, or chart
    /// scan) is available.
    pub fn can_use_ocr_or_cv(&self) -> bool {
        self.perception.ocr || self.perception.vision_shapes || self.perception.chart_scan
    }
}

pub fn current_platform_kind() -> PlatformKind {
    if cfg!(target_os = "macos") {
        PlatformKind::Macos
    } else if cfg!(target_os = "linux") {
        PlatformKind::Linux
    } else if cfg!(target_os = "windows") {
        PlatformKind::Windows
    } else {
        PlatformKind::Unknown
    }
}

pub fn current_platform_capabilities() -> PlatformCapabilities {
    match current_platform_kind() {
        PlatformKind::Macos => macos_runtime_capabilities(),
        kind => unsupported_capabilities(kind),
    }
}

/// Probe the live macOS TCC permissions and build capabilities from them, so the
/// report reflects what the process can actually do rather than a static
/// compile-time `true`. Without the Accessibility permission, AX read/actions
/// and synthetic input cannot reach other apps; without Screen Recording,
/// pixel-perception (screenshot/OCR/vision) returns blank frames.
#[cfg(target_os = "macos")]
fn macos_runtime_capabilities() -> PlatformCapabilities {
    macos_capabilities(
        crate::accessibility_trusted(),
        crate::screen_capture_trusted(),
    )
}

#[cfg(not(target_os = "macos"))]
fn macos_runtime_capabilities() -> PlatformCapabilities {
    // current_platform_kind() never reports Macos off-macOS, so this arm is
    // unreachable at runtime; keep it conservative for cross-compiled probes.
    macos_capabilities(false, false)
}

fn macos_capabilities(ax_trusted: bool, screen_trusted: bool) -> PlatformCapabilities {
    PlatformCapabilities {
        kind: PlatformKind::Macos,
        input: InputCapabilities {
            // All of these reach into other processes via AX or synthetic event
            // posting, which macOS gates behind the Accessibility permission.
            accessibility_actions: ax_trusted,
            background_pointer: ax_trusted,
            background_keyboard: ax_trusted,
            background_hotkeys: ax_trusted,
            // SkyLight focus-without-raise does not require Accessibility.
            focus_without_raise: true,
            real_cursor_borrow: ax_trusted,
            menu_bar: ax_trusted,
        },
        clipboard: ClipboardCapabilities {
            text_read: true,
            text_write: true,
            rich_formats_preserved: false,
        },
        perception: PerceptionCapabilities {
            accessibility_tree: ax_trusted,
            // Pixel perception needs the Screen Recording permission.
            screenshots: screen_trusted,
            ocr: screen_trusted,
            vision_shapes: screen_trusted,
            chart_scan: screen_trusted,
        },
        windows: WindowCapabilities {
            // CoreGraphics window listing/visibility needs no special permission.
            list: true,
            visibility: true,
            // Moving/resizing/arranging/exposing other apps' windows is driven
            // through AX.
            move_resize: ax_trusted,
            arrange: ax_trusted,
            expose: ax_trusted,
        },
        apps: AppCapabilities {
            list_running: true,
            list_launchable: true,
            app_info: true,
            launch: true,
            open_url: true,
            close: true,
            // Driving the native file chooser dialog goes through AX.
            file_chooser: ax_trusted,
        },
    }
}

fn unsupported_capabilities(kind: PlatformKind) -> PlatformCapabilities {
    PlatformCapabilities {
        kind,
        input: InputCapabilities {
            accessibility_actions: false,
            background_pointer: false,
            background_keyboard: false,
            background_hotkeys: false,
            focus_without_raise: false,
            real_cursor_borrow: false,
            menu_bar: false,
        },
        clipboard: ClipboardCapabilities {
            text_read: false,
            text_write: false,
            rich_formats_preserved: false,
        },
        perception: PerceptionCapabilities {
            accessibility_tree: false,
            screenshots: false,
            ocr: false,
            vision_shapes: false,
            chart_scan: false,
        },
        windows: WindowCapabilities {
            list: false,
            visibility: false,
            move_resize: false,
            arrange: false,
            expose: false,
        },
        apps: AppCapabilities {
            list_running: false,
            list_launchable: false,
            app_info: false,
            launch: false,
            open_url: false,
            close: false,
            file_chooser: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_match_current_platform() {
        let capabilities = current_platform_capabilities();
        assert_eq!(capabilities.kind, current_platform_kind());
    }

    #[test]
    fn unsupported_platforms_do_not_advertise_mutation_or_vision() {
        let capabilities = unsupported_capabilities(PlatformKind::Linux);
        assert!(!capabilities.can_mutate_ui());
        assert!(!capabilities.can_use_ocr_or_cv());
    }

    #[test]
    fn macos_groups_related_capabilities_by_call_type() {
        let capabilities = macos_capabilities(true, true);
        assert!(capabilities.input.focus_without_raise);
        assert!(capabilities.clipboard.text_read);
        assert!(!capabilities.clipboard.rich_formats_preserved);
        assert!(capabilities.perception.ocr);
        assert!(capabilities.perception.vision_shapes);
        assert!(capabilities.windows.move_resize);
        assert!(capabilities.apps.file_chooser);
    }

    #[test]
    fn macos_capabilities_reflect_missing_tcc_permissions() {
        // No Accessibility and no Screen Recording: the report must not over-claim
        // AX/input/perception, but permission-free surfaces stay available.
        let capabilities = macos_capabilities(false, false);

        assert!(!capabilities.input.accessibility_actions);
        assert!(!capabilities.input.background_pointer);
        assert!(!capabilities.input.menu_bar);
        assert!(!capabilities.perception.accessibility_tree);
        assert!(!capabilities.perception.screenshots);
        assert!(!capabilities.perception.ocr);
        assert!(!capabilities.windows.move_resize);
        assert!(!capabilities.apps.file_chooser);
        // No pixel perception without Screen Recording.
        assert!(!capabilities.can_use_ocr_or_cv());

        // Permission-free surfaces remain advertised (launching apps needs no TCC
        // grant, so can_mutate_ui stays true via that path).
        assert!(capabilities.input.focus_without_raise);
        assert!(capabilities.clipboard.text_read);
        assert!(capabilities.windows.list);
        assert!(capabilities.apps.launch);
    }

    #[test]
    fn macos_capabilities_separate_ax_and_screen_permissions() {
        // Accessibility granted but Screen Recording denied: input works, pixel
        // perception does not.
        let capabilities = macos_capabilities(true, false);
        assert!(capabilities.input.accessibility_actions);
        assert!(capabilities.perception.accessibility_tree);
        assert!(!capabilities.perception.screenshots);
        assert!(!capabilities.perception.ocr);
    }
}
