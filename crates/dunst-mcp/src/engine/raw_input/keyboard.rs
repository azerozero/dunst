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
            None => Err(VisualOpsError::Execution(format!(
                "no menu {name:?} found in the menubar"
            ))),
        }
    }

    /// Press a named key (e.g. `"Return"`/`"Enter"` to submit a typed URL).
    /// Raw keyboard input is high-risk because it is not tied to a scene element.
    #[cfg(target_os = "macos")]
    pub fn press_key(&mut self, key: &str) -> dunst_core::Result<AuditEntry> {
        if !is_press_key_name(key) {
            return Err(VisualOpsError::Execution(format!(
                "unsupported key {key:?}; expected return|enter, tab, escape, space, delete, up/down/left/right, pageup/pagedown, home/end"
            )));
        }
        let target_id = format!("keyboard@press:{key}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(key.to_string()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::press_key(self.target.pid, self.target.window_id, key)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(key.to_string()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn press_key(&mut self, _key: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
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
        let target_id = "keyboard@type_keys".to_string();
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
        Err(VisualOpsError::Execution(
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
            let can_scroll = self
                .affordance_graph()
                .affordances
                .get(id)
                .map(|affordance| affordance.actions.contains(&SemanticAction::Scroll))
                .unwrap_or(false);
            if !can_scroll {
                return Err(VisualOpsError::ActionUnavailable {
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
            SemanticAction::Type,
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
            SemanticAction::Type,
            Some(format!("scroll {direction} x{count}")),
            Some("background web scroll (Page/Home/End keys, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
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
        Err(VisualOpsError::Execution(
            "scroll requires a macOS backend".into(),
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
            SemanticAction::Type,
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
            SemanticAction::Type,
            Some(format!("zoom {direction}")),
            Some("background zoom (Cmd =/-/0, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn zoom(&mut self, _direction: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
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
            return Err(VisualOpsError::Execution(message));
        }
        let (flags, keycode) = parse_combo(combo)
            .ok_or_else(|| VisualOpsError::Execution(format!("unrecognised hotkey {combo:?}")))?;
        let target_id = format!("keyboard@hotkey:{combo}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
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
            SemanticAction::Type,
            Some(combo.to_string()),
            Some("background hotkey (modifier combo, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn hotkey(&mut self, _combo: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "hotkey requires a macOS backend".into(),
        ))
    }
}
