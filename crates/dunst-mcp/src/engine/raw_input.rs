use super::*;

mod keyboard;
pub(super) use keyboard::page_scroll_target_id;

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
            return Err(DunstError::Execution(
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
            Ok(entry) => {
                self.clear_inflight_raw_approval(&target_id);
                self.approvals.remove(&target_id);
                self.pending_gate_ids.remove(&target_id);
                Ok(entry)
            }
            Err(err) => {
                let _ = self.audit_raw_input(
                    target_id,
                    SemanticAction::Click,
                    argument,
                    reasoning.or(Some("reveal hover-only control and click it")),
                    risk,
                    Err(err),
                );
                Err(DunstError::Execution(
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

        Err(DunstError::Execution(format!(
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
                Err(DunstError::Execution(
                    "right-click requires the SkyLight backend".into(),
                ))
            }
        })
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
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
        Err(DunstError::Execution(
            "reveal_hover_click requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn right_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "right_click_at requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn double_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "double_click_at requires a macOS backend".into(),
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
        Err(DunstError::Execution(
            "hover_at requires a macOS backend".into(),
        ))
    }
}
