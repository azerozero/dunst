use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    Macos,
    Linux,
    Windows,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    pub kind: PlatformKind,
    pub input: InputCapabilities,
    pub clipboard: ClipboardCapabilities,
    pub perception: PerceptionCapabilities,
    pub windows: WindowCapabilities,
    pub apps: AppCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCapabilities {
    pub accessibility_actions: bool,
    pub background_pointer: bool,
    pub background_keyboard: bool,
    pub background_hotkeys: bool,
    pub focus_without_raise: bool,
    pub real_cursor_borrow: bool,
    pub menu_bar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardCapabilities {
    pub text_read: bool,
    pub text_write: bool,
    pub rich_formats_preserved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerceptionCapabilities {
    pub accessibility_tree: bool,
    pub screenshots: bool,
    pub ocr: bool,
    pub vision_shapes: bool,
    pub chart_scan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCapabilities {
    pub list: bool,
    pub visibility: bool,
    pub move_resize: bool,
    pub arrange: bool,
    pub expose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppCapabilities {
    pub list_running: bool,
    pub list_launchable: bool,
    pub app_info: bool,
    pub launch: bool,
    pub open_url: bool,
    pub close: bool,
    pub file_chooser: bool,
}

impl PlatformCapabilities {
    pub fn can_mutate_ui(&self) -> bool {
        self.input.accessibility_actions
            || self.input.background_pointer
            || self.input.background_keyboard
            || self.windows.move_resize
            || self.apps.launch
    }

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
        PlatformKind::Macos => macos_capabilities(),
        kind => unsupported_capabilities(kind),
    }
}

fn macos_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        kind: PlatformKind::Macos,
        input: InputCapabilities {
            accessibility_actions: true,
            background_pointer: true,
            background_keyboard: true,
            background_hotkeys: true,
            focus_without_raise: true,
            real_cursor_borrow: true,
            menu_bar: true,
        },
        clipboard: ClipboardCapabilities {
            text_read: true,
            text_write: true,
            rich_formats_preserved: false,
        },
        perception: PerceptionCapabilities {
            accessibility_tree: true,
            screenshots: true,
            ocr: true,
            vision_shapes: true,
            chart_scan: true,
        },
        windows: WindowCapabilities {
            list: true,
            visibility: true,
            move_resize: true,
            arrange: true,
            expose: true,
        },
        apps: AppCapabilities {
            list_running: true,
            list_launchable: true,
            app_info: true,
            launch: true,
            open_url: true,
            close: true,
            file_chooser: true,
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
        let capabilities = macos_capabilities();
        assert!(capabilities.input.focus_without_raise);
        assert!(capabilities.clipboard.text_read);
        assert!(!capabilities.clipboard.rich_formats_preserved);
        assert!(capabilities.perception.ocr);
        assert!(capabilities.perception.vision_shapes);
        assert!(capabilities.windows.move_resize);
        assert!(capabilities.apps.file_chooser);
    }
}
