use super::*;

impl Engine {
    pub(super) fn raw_input_risk(extra_reasons: Vec<String>) -> RiskAssessment {
        let mut reasons = vec!["raw input is not bound to a scene element".to_string()];
        reasons.extend(extra_reasons);
        RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons,
        }
    }

    pub(super) fn raw_point_risk(&self, x: f64, y: f64) -> RiskAssessment {
        let mut reasons = Vec::new();
        let point = (x, y);
        if self
            .cached_window_rect
            .map(|w| !point_in_bbox(point, w))
            .unwrap_or(false)
        {
            reasons.push("raw point is outside the target window".to_string());
        } else {
            let menubar = self.cached_menubar_root.as_deref();
            let hits_visible_node = self.scene_graph().nodes.values().any(|node| {
                !matches!(node.role, Role::Window | Role::MenuBar | Role::Toolbar)
                    && node_visible_or_menu(node, self.cached_window_rect, menubar)
                    && node.bbox.map(|b| point_in_bbox(point, b)).unwrap_or(false)
            });
            if !hits_visible_node {
                reasons.push(
                    "raw point is not inside any visible scene element; possible backdrop or blank area"
                        .to_string(),
                );
            }
        }
        Self::raw_input_risk(reasons)
    }

    pub(super) fn ensure_point_in_target_window(
        &self,
        x: f64,
        y: f64,
        operation: &str,
    ) -> dunst_core::Result<()> {
        if off_target_raw_allowed() {
            return Ok(());
        }
        let window = self.current_window_bounds();
        if point_in_bbox((x, y), window) {
            return Ok(());
        }
        Err(VisualOpsError::Execution(format!(
            "{operation} point ({x:.1},{y:.1}) is outside the target window {} {:?}; attach the intended window or set DUNST_MCP_ALLOW_OFF_TARGET_RAW=1",
            self.target.window_id,
            window
        )))
    }

    pub(super) fn ensure_region_in_target_window(
        &self,
        region: Bbox,
        operation: &str,
    ) -> dunst_core::Result<()> {
        if off_target_raw_allowed() {
            return Ok(());
        }
        let window = self.current_window_bounds();
        if rect_intersection_area(region, window) > 0.0
            && region.x >= window.x
            && region.y >= window.y
            && region.x + region.w <= window.x + window.w
            && region.y + region.h <= window.y + window.h
        {
            return Ok(());
        }
        Err(VisualOpsError::Execution(format!(
            "{operation} region {:?} is outside the target window {} {:?}; pass target-window screen coordinates or set DUNST_MCP_ALLOW_OFF_TARGET_RAW=1",
            region,
            self.target.window_id,
            window
        )))
    }

    /// Return a pending-approval audit entry when a raw input has not been
    /// explicitly approved. Raw inputs are nameable by synthetic target ids such
    /// as `screen@x,y:click` and `keyboard@hotkey:cmd+l`.
    pub(super) fn gate_raw_input(
        &mut self,
        target_id: &str,
        action: SemanticAction,
        argument: Option<String>,
        reasoning: Option<&str>,
        risk: RiskAssessment,
    ) -> Option<AuditEntry> {
        if self.approvals.contains(target_id) {
            return None;
        }
        self.pending_gate_ids.insert(target_id.to_string());
        Some(self.push_entry(AuditEntry {
            ts_ms: dunst_core::now_ms(),
            target_id: target_id.to_string(),
            action,
            argument,
            risk,
            reasoning: reasoning.map(str::to_owned),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        }))
    }

    /// Record a raw input attempt. The attempt is always written to the trace; on
    /// platform failure the entry is `Failed` and the error is surfaced to the
    /// caller. Mirrors [`act`](Self::act)'s re-perceive (`refresh` + `diff_since`).
    #[cfg(target_os = "macos")]
    pub(super) fn audit_raw_input(
        &mut self,
        target_id: String,
        action: SemanticAction,
        argument: Option<String>,
        reasoning: Option<&str>,
        risk: RiskAssessment,
        outcome: dunst_core::Result<()>,
    ) -> dunst_core::Result<AuditEntry> {
        let ts_ms = dunst_core::now_ms();
        let user_active_blocked = outcome
            .as_ref()
            .err()
            .map(|e| e.to_string().contains("user-active guard blocked"))
            .unwrap_or(false);
        let result = if outcome.is_ok() {
            ActionResult::Success
        } else {
            ActionResult::Failed
        };
        let graph_diff = if result == ActionResult::Success {
            self.approvals.remove(&target_id);
            self.pending_gate_ids.remove(&target_id);
            let _ = self.refresh();
            self.diff_since()
        } else if user_active_blocked {
            GraphDiff::default()
        } else {
            self.approvals.remove(&target_id);
            self.pending_gate_ids.remove(&target_id);
            let _ = self.refresh();
            self.diff_since()
        };
        let entry = self.push_entry(AuditEntry {
            ts_ms,
            target_id,
            action,
            argument,
            risk,
            reasoning: reasoning.map(str::to_owned),
            result,
            graph_diff,
        });
        outcome.map(|()| entry)
    }
}
