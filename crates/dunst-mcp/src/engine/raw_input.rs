use super::*;

impl Engine {
    // --- raw input tools ----------------------------------------------------

    /// Click at a raw **screen point** (P1 navigation: OCR a link with `read_text`,
    /// then click its bbox centre).
    ///
    /// Unlike [`click_element`](Self::click_element), this is not bound to an
    /// element or affordance. A raw click can land on anything under that point,
    /// so it is gated as a high-risk raw action and audited under
    /// `target_id = "screen@x,y"`.
    #[cfg(target_os = "macos")]
    pub fn click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.click_at_button(x, y, 0, "click")
    }

    /// Briefly raise the target window and borrow the real cursor to reveal
    /// hover-only controls, then click the first visible element matching
    /// `query` through AX and restore the user's previous frontmost app/cursor.
    #[cfg(target_os = "macos")]
    pub fn reveal_hover_click(
        &mut self,
        x: f64,
        y: f64,
        query: &str,
        settle_ms: u64,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, "reveal_hover_click")?;
        let query = query.trim();
        if query.is_empty() {
            return Err(VisualOpsError::Execution(
                "reveal_hover_click requires a non-empty query".into(),
            ));
        }
        let target_id = format!("hover-reveal@{x:.0},{y:.0}:{query}:click");
        let risk = Self::raw_input_risk(vec![
            "temporarily raises the target window".to_string(),
            "briefly borrows the real OS cursor".to_string(),
            "clicks a hover-revealed control".to_string(),
        ]);
        let argument = Some(format!("hover {x:.1},{y:.1}; click visible {query:?}"));
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Click,
            argument.clone(),
            reasoning.or(Some("reveal hover-only control and click it")),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        self.approvals.remove(&target_id);
        self.pending_gate_ids.remove(&target_id);

        match self.reveal_hover_click_outcome(x, y, query, settle_ms, reasoning) {
            Ok(entry) => Ok(entry),
            Err(err) => {
                let _ = self.audit_raw_input(
                    target_id,
                    SemanticAction::Click,
                    argument,
                    reasoning.or(Some("reveal hover-only control and click it")),
                    risk,
                    Err(err),
                );
                Err(VisualOpsError::Execution(
                    "reveal_hover_click failed; cursor/window restore was attempted".into(),
                ))
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn reveal_hover_click_outcome(
        &mut self,
        x: f64,
        y: f64,
        query: &str,
        settle_ms: u64,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let settle = settle_ms.clamp(50, 1_500);
        let _guard = BorrowedHoverUiGuard::start(&self.window, x, y)?;
        std::thread::sleep(Duration::from_millis(settle));
        self.refresh()?;

        let candidates: Vec<String> = self
            .find_element_filtered(query, true)
            .into_iter()
            .map(|n| n.id.clone())
            .collect();
        for id in candidates {
            if self
                .resolve_action_target(&id, &[SemanticAction::Click])
                .is_ok()
            {
                return self.click_element(
                    &id,
                    reasoning.or(Some("click hover-revealed control by AX id")),
                );
            }
        }

        Err(VisualOpsError::Execution(format!(
            "no visible clickable element found after hover reveal for query {query:?}"
        )))
    }

    /// Right-click at a raw screen point (context menus). Background web via SkyLight.
    #[cfg(target_os = "macos")]
    pub fn right_click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.click_at_button(x, y, 1, "right-click")
    }

    /// Double-click at a raw screen point — two quick clicks.
    #[cfg(target_os = "macos")]
    pub fn double_click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, "double-click")?;
        let target_id = format!("screen@{x},{y}:double-click");
        let risk = self.raw_point_risk(x, y);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Click,
            Some(format!("double-click {x},{y}")),
            Some("raw screen double-click"),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let mut outcome = self.raw_click_outcome(x, y, 0);
        std::thread::sleep(std::time::Duration::from_millis(90));
        if outcome.is_ok() {
            outcome = self.raw_click_outcome(x, y, 0);
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::Click,
            Some(format!("double-click {x},{y}")),
            Some("raw screen double-click"),
            risk,
            outcome,
        )
    }

    /// Shared raw click at a screen point. Prefers the SkyLight background path
    /// (reaches a backgrounded/occluded web target, trusted, no cursor move),
    /// falling back to a cursor click. Raw input is high-risk because it is not
    /// tied to a scene element.
    #[cfg(target_os = "macos")]
    pub(super) fn click_at_button(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        label: &str,
    ) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, label)?;
        let target_id = format!("screen@{x},{y}:{label}");
        let risk = self.raw_point_risk(x, y);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Click,
            Some(format!("{label} {x},{y}")),
            Some("raw screen click"),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let outcome = self.raw_click_outcome(x, y, button);
        self.audit_raw_input(
            target_id,
            SemanticAction::Click,
            Some(format!("{label} {x},{y}")),
            Some("raw screen click"),
            risk,
            outcome,
        )
    }

    #[cfg(target_os = "macos")]
    pub(super) fn raw_click_outcome(&self, x: f64, y: f64, button: u8) -> dunst_core::Result<()> {
        retry_user_active_guard(|| {
            let (ox, oy) = dunst_vision::capture::window_bounds(self.target.window_id)
                .map(|(x, y, _, _)| (x, y))
                .unwrap_or((0.0, 0.0));
            if dunst_platform::click_web_background(
                self.target.pid,
                self.target.window_id,
                x,
                y,
                ox,
                oy,
                button,
            ) {
                Ok(())
            } else if button == 0 {
                dunst_platform::click_at_point(self.target.pid, x, y)
            } else {
                Err(VisualOpsError::Execution(
                    "right-click requires the SkyLight backend".into(),
                ))
            }
        })
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "click_at requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn reveal_hover_click(
        &mut self,
        _x: f64,
        _y: f64,
        _query: &str,
        _settle_ms: u64,
        _reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "reveal_hover_click requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn right_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "right_click_at requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn double_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "double_click_at requires a macOS backend".into(),
        ))
    }

    /// Open a menu-bar menu by name (e.g. "File"/"Fichier") — finds the menubar
    /// item and presses it (AX). Native menus; the items then appear in the graph.
    pub fn open_menu(&mut self, name: &str) -> dunst_core::Result<AuditEntry> {
        let id = self
            .scene_graph()
            .nodes
            .values()
            .find(|n| {
                n.ax_role.contains("Menu")
                    && n.label
                        .as_deref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(name.trim()))
            })
            .map(|n| n.id.clone());
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
                .map(|a| a.actions.contains(&SemanticAction::Scroll))
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
        let (keycode, n) = match direction {
            "up" => (0x74_u16, pages.clamp(1, 20)),
            "top" => (0x73, 1),
            "bottom" => (0x77, 1),
            _ => (0x79, pages.clamp(1, 20)), // down (default)
        };
        let target_id = format!("keyboard@scroll:{direction}:{n}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(format!("scroll {direction} x{n}")),
            Some("background web scroll"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..n {
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
            Some(format!("scroll {direction} x{n}")),
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

    /// Background hover at a screen point so the target shows a hover state (e.g.
    /// a chart crosshair tooltip / value-at-cursor) without moving the visible
    /// cursor. A pure probe — no risk-gating, no audit, **no refresh** — so a
    /// following `read_text` reads the hovered result.
    ///
    /// Some web UIs only instantiate controls on a real OS-cursor hover. For
    /// those, `hover_at` can post successfully while AX stays unchanged. Before
    /// falling back to `read_at(..., borrow_cursor=true)`, confirm with
    /// `desktop_view` that the target window is actually visible/topmost under
    /// the point; borrowed-cursor OCR reads the composited display, not the
    /// background target capture.
    #[cfg(target_os = "macos")]
    pub fn hover_at(&self, x: f64, y: f64) -> dunst_core::Result<()> {
        self.ensure_point_in_target_window(x, y, "hover_at")?;
        self.hover_target_background(x, y)
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn hover_at(&self, _x: f64, _y: f64) -> dunst_core::Result<()> {
        Err(VisualOpsError::Execution(
            "hover_at requires a macOS backend".into(),
        ))
    }
}
