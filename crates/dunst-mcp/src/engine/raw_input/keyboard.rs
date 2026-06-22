use super::*;

impl Engine {
    /// Open a menu-bar menu by name (e.g. "File"/"Fichier") — finds the menubar
    /// item and presses it (AX). Native menus; the items then appear in the graph.
    pub fn open_menu(&mut self, name: &str) -> dunst_core::Result<AuditEntry> {
        let id = self
            .scene_graph()
            .nodes
            .values()
            .find(|node| {
                node.ax_role.contains("Menu")
                    && node
                        .label
                        .as_deref()
                        .is_some_and(|label| label.eq_ignore_ascii_case(name.trim()))
            })
            .map(|node| node.id.clone());
        match id {
            Some(id) => self.click_element(&id, Some(&format!("open menu {name}"))),
            None => Err(DunstError::Execution(format!(
                "no menu {name:?} found in the menubar"
            ))),
        }
    }

    /// Press a named key (e.g. `"Return"`/`"Enter"` to submit a typed URL).
    /// Raw keyboard input is high-risk because it is not tied to a scene element.
    #[cfg(target_os = "macos")]
    pub fn press_key(&mut self, key: &str, repeat: usize) -> dunst_core::Result<AuditEntry> {
        if !is_press_key_name(key) {
            return Err(DunstError::Execution(format!(
                "unsupported key {key:?}; expected return|enter, tab, escape, space, delete, up/down/left/right, pageup/pagedown, home/end"
            )));
        }
        let repeat = repeat.clamp(1, 20);
        let argument = if repeat == 1 {
            key.to_string()
        } else {
            format!("{key} x{repeat}")
        };
        let target_id = raw_press_key_target_id(key, repeat);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::KeyPress,
            Some(argument.clone()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..repeat {
            outcome = retry_user_active_guard(|| {
                dunst_platform::press_key(self.target.pid, self.target.window_id, key)
            });
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::KeyPress,
            Some(argument),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn press_key(&mut self, _key: &str, _repeat: usize) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "press_key requires a macOS backend".into(),
        ))
    }

    /// Type `text` into the focused element via the **SkyLight auth-signed**
    /// keyboard path, so it reaches a backgrounded/occluded window's web content
    /// (trusted, no cursor, no foreground). First focus the field (e.g. click_at
    /// it). Raw keyboard input is high-risk because it is not tied to a scene
    /// element.
    #[cfg(target_os = "macos")]
    pub fn type_keys(&mut self, text: &str) -> dunst_core::Result<AuditEntry> {
        let target_id = raw_type_keys_target_id(text);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw keyboard text into focused element"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::type_text_background(self.target.pid, self.target.window_id, text)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw keyboard text into focused element (background web via SkyLight auth)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn type_keys(&mut self, _text: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "type_keys requires a macOS backend".into(),
        ))
    }

    /// Scroll the FOCUSED page in the background via auth-signed Page/Home/End keys
    /// (reaches web content, no cursor, no foreground). `direction` =
    /// up|down|top|bottom; `pages` = how many Page presses (down/up). Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn scroll(
        &mut self,
        direction: &str,
        pages: usize,
        focus_id: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        if let Some(id) = focus_id {
            if let Some(page_direction) = page_scroll_target_direction(id) {
                let window = self.current_window_bounds();
                return self.scroll_at(
                    window.x + window.w / 2.0,
                    window.y + window.h / 2.0,
                    page_direction.unwrap_or(direction),
                    pages,
                );
            }
            let can_scroll = self
                .affordance_graph()
                .affordances
                .get(id)
                .map(|affordance| affordance.actions.contains(&SemanticAction::Scroll))
                .unwrap_or(false);
            if !can_scroll {
                return Err(DunstError::ActionUnavailable {
                    id: id.to_string(),
                    action: format!("{:?}", SemanticAction::Scroll),
                });
            }
            return self.act(
                id,
                SemanticAction::Scroll,
                Some(&format!("{direction}:{pages}")),
                Some("direct AX scrollbar scroll"),
                None,
            );
        }

        self.scroll_with_background_keys(direction, pages)
    }

    #[cfg(target_os = "macos")]
    fn scroll_with_background_keys(
        &mut self,
        direction: &str,
        pages: usize,
    ) -> dunst_core::Result<AuditEntry> {
        // macOS virtual keycodes: PageDown=0x79, PageUp=0x74, Home=0x73, End=0x77.
        let (keycode, count) = match direction {
            "up" => (0x74_u16, pages.clamp(1, 20)),
            "top" => (0x73, 1),
            "bottom" => (0x77, 1),
            _ => (0x79, pages.clamp(1, 20)), // down (default)
        };
        let target_id = format!("keyboard@scroll:{direction}:{count}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Scroll,
            Some(format!("scroll {direction} x{count}")),
            Some("background web scroll"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..count {
            outcome = retry_user_active_guard(|| {
                dunst_platform::key_web_background(
                    self.target.pid,
                    self.target.window_id,
                    keycode,
                    0,
                )
            });
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(380));
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::Scroll,
            Some(format!("scroll {direction} x{count}")),
            Some("background web scroll (Page/Home/End keys, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Wheel-scroll at a concrete screen point in the target window. This is the
    /// fallback for web pages/cards that do not expose an AX scrollbar: the point
    /// chooses the scroll container, while the raw input gate still requires
    /// operator approval before mutating the page.
    #[cfg(target_os = "macos")]
    pub fn scroll_at(
        &mut self,
        x: f64,
        y: f64,
        direction: &str,
        pages: usize,
    ) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, "scroll_at")?;
        let direction = normalized_scroll_direction(direction);
        let count = pages.clamp(1, 20);
        if matches!(direction, "top" | "bottom") {
            return self.scroll_with_background_keys(direction, count);
        }
        let target_id = wheel_scroll_target_id(direction, count, x, y);
        let argument = format!("wheel scroll {direction} x{count} at {x:.1},{y:.1}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Scroll,
            Some(argument.clone()),
            Some("background wheel scroll at target point"),
            self.raw_point_risk(x, y),
        ) {
            return Ok(entry);
        }
        let (ox, oy) = dunst_vision::capture::window_bounds(self.target.window_id)
            .map(|(x, y, _, _)| (x, y))
            .unwrap_or((
                self.current_window_bounds().x,
                self.current_window_bounds().y,
            ));
        let delta = match direction {
            "up" => 720,
            _ => -720,
        };
        let mut outcome = Ok(());
        for _ in 0..count {
            outcome = retry_user_active_guard(|| {
                dunst_platform::scroll_web_background(
                    self.target.pid,
                    self.target.window_id,
                    x,
                    y,
                    ox,
                    oy,
                    delta,
                )
            });
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(160));
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::Scroll,
            Some(argument),
            Some("background wheel scroll at target point"),
            self.raw_point_risk(x, y),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scroll(
        &mut self,
        _direction: &str,
        _pages: usize,
        _focus_id: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "scroll requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scroll_at(
        &mut self,
        _x: f64,
        _y: f64,
        _direction: &str,
        _pages: usize,
    ) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "scroll_at requires a macOS backend".into(),
        ))
    }

    /// Zoom the focused page (browser/native) in the background: `in`/`out`/`reset`
    /// → Cmd+= / Cmd+- / Cmd+0, auth-signed (reaches web). Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn zoom(&mut self, direction: &str) -> dunst_core::Result<AuditEntry> {
        const CMD: u64 = 0x0010_0000;
        // keycodes: '=' 0x18, '-' 0x1B, '0' 0x1D.
        let keycode = match direction {
            "out" => 0x1B_u16,
            "reset" => 0x1D,
            _ => 0x18, // in (default)
        };
        let target_id = format!("keyboard@zoom:{direction}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Hotkey,
            Some(format!("zoom {direction}")),
            Some("background zoom"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::key_web_background(self.target.pid, self.target.window_id, keycode, CMD)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Hotkey,
            Some(format!("zoom {direction}")),
            Some("background zoom (Cmd =/-/0, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn zoom(&mut self, _direction: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "zoom requires a macOS backend".into(),
        ))
    }

    /// A keyboard shortcut in the background: modifiers (cmd|shift|opt|ctrl, `+`-
    /// separated) plus a key (a single character, or a name like enter/tab/escape/
    /// space/delete/left/right/up/down). E.g. "cmd+l" (focus omnibox), "cmd+t",
    /// "cmd+w". Auth-signed so it reaches web content. Layout-sensitive text
    /// selection shortcuts such as "cmd+a" are rejected; use `type_into` for
    /// field replacement. Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn hotkey(&mut self, combo: &str) -> dunst_core::Result<AuditEntry> {
        if let Some(message) = layout_sensitive_hotkey_message(combo) {
            return Err(DunstError::Execution(message));
        }
        let (flags, keycode) = parse_combo(combo)
            .ok_or_else(|| DunstError::Execution(format!("unrecognised hotkey {combo:?}")))?;
        let target_id = format!("keyboard@hotkey:{combo}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Hotkey,
            Some(combo.to_string()),
            Some("background hotkey"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::key_web_background(
                self.target.pid,
                self.target.window_id,
                keycode,
                flags,
            )
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Hotkey,
            Some(combo.to_string()),
            Some("background hotkey (modifier combo, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn hotkey(&mut self, _combo: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "hotkey requires a macOS backend".into(),
        ))
    }
}

pub(in crate::engine) fn page_scroll_target_id(direction: &str) -> String {
    format!("page@scroll:{}", normalized_scroll_direction(direction))
}

pub(super) fn page_scroll_target_direction(id: &str) -> Option<Option<&str>> {
    if id == "page@scroll" {
        return Some(None);
    }
    id.strip_prefix("page@scroll:")
        .map(|direction| Some(normalized_scroll_direction(direction)))
}

fn normalized_scroll_direction(direction: &str) -> &'static str {
    match direction.trim().to_ascii_lowercase().as_str() {
        "up" => "up",
        "top" => "top",
        "bottom" => "bottom",
        _ => "down",
    }
}

fn wheel_scroll_target_id(direction: &str, count: usize, x: f64, y: f64) -> String {
    format!(
        "wheel@scroll:{}:{}:{:.0},{:.0}",
        normalized_scroll_direction(direction),
        count.clamp(1, 20),
        x,
        y
    )
}
