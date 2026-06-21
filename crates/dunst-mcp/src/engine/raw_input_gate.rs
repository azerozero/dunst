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
            let visibility = self.target_visibility();
            if !visibility.covered_by.is_empty() {
                reasons.push(format!(
                    "target window is covered by {:?}; verify OCR/screenshot came from the target before using visible coordinates",
                    visibility
                        .covered_by
                        .iter()
                        .map(|window| window.window_id)
                        .collect::<Vec<_>>()
                ));
            }
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
        if self.consume_raw_approval(target_id) || self.approvals.contains(target_id) {
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

    pub(super) fn approve_raw_input(&mut self, target_id: &str) {
        let expires_at = Instant::now() + Duration::from_secs(30);
        for policy in raw_approval_policy(target_id) {
            self.raw_approvals.insert(
                policy.key,
                RawApprovalGrant {
                    remaining: policy.grant_events,
                    expires_at,
                },
            );
        }
    }

    fn consume_raw_approval(&mut self, target_id: &str) -> bool {
        let now = Instant::now();
        self.raw_approvals
            .retain(|_, grant| grant.remaining > 0 && grant.expires_at > now);

        for policy in raw_approval_policy(target_id) {
            let key = policy.key;
            let Some(grant) = self.raw_approvals.get_mut(&key) else {
                continue;
            };
            if grant.remaining < policy.cost_events || grant.expires_at <= now {
                continue;
            }
            grant.remaining -= policy.cost_events;
            let consumed = RawApprovalGrant {
                remaining: policy.cost_events,
                expires_at: grant.expires_at,
            };
            if grant.remaining == 0 {
                self.raw_approvals.remove(&key);
            }
            self.raw_approval_inflight
                .insert(target_id.to_string(), consumed);
            return true;
        }
        false
    }

    fn restore_inflight_raw_approval(&mut self, target_id: &str) {
        let Some(grant) = self.raw_approval_inflight.remove(target_id) else {
            return;
        };
        if grant.expires_at <= Instant::now() {
            return;
        }
        for policy in raw_approval_policy(target_id) {
            self.raw_approvals
                .entry(policy.key)
                .and_modify(|existing| {
                    existing.remaining += grant.remaining;
                    existing.expires_at = existing.expires_at.max(grant.expires_at);
                })
                .or_insert_with(|| grant.clone());
        }
    }

    pub(super) fn clear_inflight_raw_approval(&mut self, target_id: &str) {
        self.raw_approval_inflight.remove(target_id);
    }

    #[cfg(test)]
    pub(super) fn raw_approval_available_for_test(&mut self, target_id: &str) -> bool {
        let now = Instant::now();
        self.raw_approvals
            .retain(|_, grant| grant.remaining > 0 && grant.expires_at > now);
        raw_approval_policy(target_id).into_iter().any(|policy| {
            self.raw_approvals
                .get(&policy.key)
                .is_some_and(|grant| grant.remaining >= policy.cost_events)
        })
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
            self.clear_inflight_raw_approval(&target_id);
            self.approvals.remove(&target_id);
            self.pending_gate_ids.remove(&target_id);
            let _ = self.refresh();
            self.diff_since()
        } else if user_active_blocked {
            self.restore_inflight_raw_approval(&target_id);
            GraphDiff::default()
        } else {
            self.clear_inflight_raw_approval(&target_id);
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

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct RawApprovalKey {
    scope: RawApprovalScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum RawApprovalScope {
    Exact(String),
    KeyPress(String),
    ScrollDirection(String),
    Hotkey(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawApprovalPolicy {
    key: RawApprovalKey,
    grant_events: usize,
    cost_events: usize,
}

pub(super) fn is_synthetic_approval_target_id(target_id: &str) -> bool {
    target_id.starts_with("keyboard@")
        || target_id.starts_with("screen@")
        || target_id.starts_with("file@")
        || target_id.starts_with("hover-reveal@")
}

pub(super) fn raw_press_key_target_id(key: &str, repeat: usize) -> String {
    format!("keyboard@press:{key}:{}", repeat.clamp(1, 20))
}

pub(super) fn raw_type_keys_target_id(text: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("keyboard@type_keys:{:016x}:{}", hash, text.chars().count())
}

fn raw_approval_policy(target_id: &str) -> Vec<RawApprovalPolicy> {
    if let Some(rest) = target_id.strip_prefix("keyboard@press:") {
        let (key, cost_events) = parse_key_with_count(rest);
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::KeyPress(key.to_string()),
            },
            grant_events: 10,
            cost_events,
        }];
    }
    if let Some(rest) = target_id.strip_prefix("keyboard@scroll:") {
        let mut parts = rest.split(':');
        let direction = parts.next().unwrap_or("down");
        let cost_events = parts
            .next()
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 20);
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::ScrollDirection(direction.to_string()),
            },
            grant_events: 5,
            cost_events,
        }];
    }
    if target_id.starts_with("keyboard@hotkey:") {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::Hotkey(target_id.to_string()),
            },
            grant_events: 2,
            cost_events: 1,
        }];
    }
    if target_id.starts_with("keyboard@type_keys:") {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::Exact(target_id.to_string()),
            },
            grant_events: 1,
            cost_events: 1,
        }];
    }
    vec![RawApprovalPolicy {
        key: RawApprovalKey {
            scope: RawApprovalScope::Exact(target_id.to_string()),
        },
        grant_events: 1,
        cost_events: 1,
    }]
}

fn parse_key_with_count(rest: &str) -> (&str, usize) {
    match rest.rsplit_once(':') {
        Some((key, count)) if !key.is_empty() => {
            (key, count.parse::<usize>().unwrap_or(1).clamp(1, 20))
        }
        _ => (rest, 1),
    }
}
