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
                if self.page_ax_is_sparse() || self.browser_content_point_without_ax(point) {
                    reasons.push(
                        "raw point is inside the target window, but the page exposes no AX content element at that point; use OCR/shape verification because the browser AX tree is sparse"
                            .to_string(),
                    );
                } else {
                    reasons.push(
                        "raw point is not inside any visible scene element; possible backdrop or blank area"
                            .to_string(),
                    );
                }
            }
        }
        Self::raw_input_risk(reasons)
    }

    fn page_ax_is_sparse(&self) -> bool {
        let window = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        self.scene_graph()
            .nodes
            .values()
            .filter(|node| node_visible_or_menu(node, window, menubar))
            .filter(|node| {
                !matches!(
                    node.role,
                    Role::Window | Role::MenuBar | Role::Menu | Role::MenuItem | Role::Toolbar
                )
            })
            .filter(|node| {
                node.bbox
                    .is_some_and(|bbox| window.map(|w| bbox_intersects(bbox, w)).unwrap_or(true))
            })
            .count()
            <= 2
    }

    fn browser_content_point_without_ax(&self, point: (f64, f64)) -> bool {
        let Some(window) = self.cached_window_rect else {
            return false;
        };
        browser_app_name(&self.window.app_name)
            && point_in_bbox(point, window)
            && point.1 >= window.y + 80.0
    }

    #[cfg(test)]
    pub(super) fn ocr_point_risk(&self, hit: &OcrTextHit) -> RiskAssessment {
        self.ocr_point_risk_at(hit, hit.center, (0.0, 0.0))
    }

    pub(super) fn ocr_point_risk_at(
        &self,
        hit: &OcrTextHit,
        click_point: (f64, f64),
        offset: (f64, f64),
    ) -> RiskAssessment {
        let (x, y) = click_point;
        let mut reasons = vec![
            "raw input is bound to a verified OCR text hit, not a hand-picked screen point"
                .to_string(),
            format!(
                "OCR text {:?}, confidence {:.2}, bbox {:?}",
                hit.text, hit.confidence, hit.bbox
            ),
        ];
        if offset.0.abs() > f64::EPSILON || offset.1.abs() > f64::EPSILON {
            reasons.push(format!(
                "click point is offset {:+.1},{:+.1} from the OCR bbox centre; use this for adjacent form fields only after visual verification",
                offset.0, offset.1
            ));
        }
        let point = (x, y);
        if self
            .cached_window_rect
            .map(|w| !point_in_bbox(point, w))
            .unwrap_or(false)
        {
            reasons.push("OCR click point is outside the target window".to_string());
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
        }
        RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons,
        }
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
        Err(DunstError::Execution(format!(
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
        Err(DunstError::Execution(format!(
            "{operation} region {:?} is outside the target window {} {:?}; pass target-window screen coordinates or set DUNST_MCP_ALLOW_OFF_TARGET_RAW=1",
            region,
            self.target.window_id,
            window
        )))
    }

    pub(super) fn ensure_real_cursor_scroll_point_visible(
        &self,
        x: f64,
        y: f64,
    ) -> dunst_core::Result<()> {
        self.ensure_real_cursor_point_visible(x, y, "scroll_at borrow_cursor=true")
    }

    pub(super) fn ensure_real_cursor_point_visible(
        &self,
        x: f64,
        y: f64,
        operation: &str,
    ) -> dunst_core::Result<()> {
        if off_target_raw_allowed() {
            return Ok(());
        }
        let blockers: Vec<_> = self
            .target_visibility()
            .covered_by
            .into_iter()
            .filter(|window| point_in_bbox((x, y), window.bounds))
            .collect();
        if blockers.is_empty() {
            return Ok(());
        }
        Err(DunstError::Execution(format!(
            "{operation} requires the target window to be visible under ({x:.1},{y:.1}), but that point is covered by {:?}; use expose_target_window/arrange_windows or a background path when available",
            blockers
                .iter()
                .map(|window| window.window_id)
                .collect::<Vec<_>>()
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
        if self.batch_context_allows_mutation() {
            return None;
        }
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
            caller: None,
        }))
    }

    pub(super) fn approve_raw_input(&mut self, target_id: &str) {
        let expires_at = Instant::now() + Duration::from_secs(RAW_APPROVAL_TTL_SECS);
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

    pub(super) fn validate_synthetic_raw_approval(
        &self,
        target_id: &str,
    ) -> dunst_core::Result<()> {
        if let Some(rest) = target_id.strip_prefix("screen@") {
            let Some((x, y, action)) = parse_point_action(rest) else {
                return Err(DunstError::Execution(format!(
                    "{target_id} is not a valid screen raw-input target"
                )));
            };
            if !matches!(action, "click" | "right-click" | "double-click") {
                return Err(DunstError::Execution(format!(
                    "{target_id} uses unsupported raw screen action {action:?}"
                )));
            }
            self.ensure_point_in_target_window(x, y, "approve")?;
            return Ok(());
        }

        if let Some(rest) = target_id.strip_prefix("keyboard@press:") {
            let (key, _) = parse_key_with_count(rest);
            if !is_press_key_name(key) {
                return Err(DunstError::Execution(format!(
                    "{target_id} uses unsupported key {key:?}"
                )));
            }
            return Ok(());
        }

        if target_id.starts_with("keyboard@type_keys:") {
            return validate_type_keys_target_id(target_id);
        }
        if target_id.starts_with("keyboard@paste_text:") {
            return validate_paste_text_target_id(target_id);
        }
        if target_id.starts_with("keyboard@set_field_text:") {
            return validate_set_field_text_target_id(target_id);
        }

        if let Some(rest) = target_id.strip_prefix("keyboard@scroll:") {
            return validate_scroll_target(rest, false, self);
        }
        if let Some(rest) = target_id.strip_prefix("wheel@scroll:") {
            return validate_scroll_target(rest, true, self);
        }
        if let Some(rest) = target_id.strip_prefix("cursor@scroll:") {
            return validate_scroll_target(rest, true, self);
        }

        if let Some(combo) = target_id.strip_prefix("keyboard@hotkey:") {
            if let Some(message) = layout_sensitive_hotkey_message(combo) {
                return Err(DunstError::Execution(message));
            }
            if parse_combo(combo).is_none() {
                return Err(DunstError::Execution(format!(
                    "{target_id} uses unsupported hotkey combo {combo:?}"
                )));
            }
            return Ok(());
        }
        if let Some(rest) = target_id.strip_prefix("ocr@") {
            let Some((_hit_id, action)) = rest.rsplit_once(':') else {
                return Err(DunstError::Execution(format!(
                    "{target_id} is not a valid OCR raw-input target"
                )));
            };
            if !is_ocr_raw_action(action) {
                return Err(DunstError::Execution(format!(
                    "{target_id} uses unsupported OCR raw action {action:?}"
                )));
            }
            return Ok(());
        }
        if target_id.starts_with("file@") || target_id.starts_with("hover-reveal@") {
            return Ok(());
        }
        if target_id.starts_with("batch@selections:") {
            if valid_batch_selection_target_id(target_id).is_some() {
                return Ok(());
            }
            return Err(DunstError::Execution(format!(
                "{target_id} is not a valid batch selection approval target"
            )));
        }

        Err(DunstError::Execution(format!(
            "{target_id} is not a recognised synthetic raw-input target"
        )))
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

    pub(super) fn clear_raw_approval(&mut self, target_id: &str) {
        self.raw_approval_inflight.remove(target_id);
        for policy in raw_approval_policy(target_id) {
            self.raw_approvals.remove(&policy.key);
        }
        self.pending_gate_ids.remove(target_id);
        self.approvals.remove(target_id);
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
            caller: None,
        });
        outcome.map(|()| entry)
    }
}

const RAW_APPROVAL_TTL_SECS: u64 = 120;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct RawApprovalKey {
    scope: RawApprovalScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum RawApprovalScope {
    Exact(String),
    KeyPress(String),
    OcrClick(String),
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
        || target_id.starts_with("cursor@")
        || target_id.starts_with("ocr@")
        || target_id.starts_with("wheel@")
        || target_id.starts_with("screen@")
        || target_id.starts_with("file@")
        || target_id.starts_with("hover-reveal@")
        || target_id.starts_with("batch@selections:")
}

pub(super) fn raw_press_key_target_id(key: &str, repeat: usize) -> String {
    format!("keyboard@press:{key}:{}", repeat.clamp(1, 20))
}

pub(super) fn raw_type_keys_target_id(text: &str) -> String {
    raw_text_payload_target_id("type_keys", text)
}

pub(super) fn raw_paste_text_target_id(text: &str) -> String {
    raw_text_payload_target_id("paste_text", text)
}

pub(super) fn raw_set_field_text_target_id(text: &str) -> String {
    raw_text_payload_target_id("set_field_text", text)
}

pub(super) fn raw_apply_selections_target_id(hash: &str, count: usize) -> String {
    format!(
        "batch@selections:{hash}:{}",
        count.clamp(1, MAX_BATCH_PICKS)
    )
}

fn raw_text_payload_target_id(action: &str, text: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("keyboard@{action}:{:016x}:{}", hash, text.chars().count())
}

fn raw_approval_policy(target_id: &str) -> Vec<RawApprovalPolicy> {
    if let Some(count) = valid_batch_selection_target_id(target_id) {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::Exact(target_id.to_string()),
            },
            grant_events: count,
            cost_events: 1,
        }];
    }
    if let Some(rest) = target_id.strip_prefix("keyboard@press:") {
        let (key, cost_events) = parse_key_with_count(rest);
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::KeyPress(key.to_string()),
            },
            grant_events: cost_events.max(10),
            cost_events,
        }];
    }
    if let Some(rest) = target_id.strip_prefix("keyboard@scroll:") {
        return scroll_direction_policy(rest);
    }
    if let Some(rest) = target_id.strip_prefix("wheel@scroll:") {
        return scroll_direction_policy(rest);
    }
    if let Some(rest) = target_id.strip_prefix("cursor@scroll:") {
        return scroll_direction_policy(rest);
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
    if target_id.starts_with("keyboard@paste_text:") {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::Exact(target_id.to_string()),
            },
            grant_events: 1,
            cost_events: 1,
        }];
    }
    if target_id.starts_with("keyboard@set_field_text:") {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::Exact(target_id.to_string()),
            },
            grant_events: 1,
            cost_events: 1,
        }];
    }
    if let Some(scope) = ocr_click_approval_scope(target_id) {
        return vec![RawApprovalPolicy {
            key: RawApprovalKey {
                scope: RawApprovalScope::OcrClick(scope),
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

fn scroll_direction_policy(rest: &str) -> Vec<RawApprovalPolicy> {
    let direction = rest.split(':').next().unwrap_or("down").to_string();
    // A scroll is one operator gesture regardless of page count or exact point.
    // Approving "scroll <dir>" therefore grants a batch of same-direction scrolls
    // — at any point, any page count — within the TTL, so repeated point-scrolls
    // (scroll_at at successive coordinates) don't re-arm the approval gate on every
    // call. Each scroll costs exactly one event; the grant covers a working batch.
    vec![RawApprovalPolicy {
        key: RawApprovalKey {
            scope: RawApprovalScope::ScrollDirection(direction),
        },
        grant_events: 8,
        cost_events: 1,
    }]
}

fn ocr_click_approval_scope(target_id: &str) -> Option<String> {
    let rest = target_id.strip_prefix("ocr@")?;
    let (hit_id, action) = rest.rsplit_once(':')?;
    (!hit_id.is_empty() && !action.is_empty()).then(|| {
        format!(
            "{}:{}",
            action,
            stable_ocr_hit_slug(hit_id).to_ascii_lowercase()
        )
    })
}

fn is_ocr_raw_action(action: &str) -> bool {
    action == "click" || action == "dismiss_modal" || action.starts_with("click-offset@")
}

fn stable_ocr_hit_slug(hit_id: &str) -> String {
    let Some(rest) = hit_id.strip_prefix("ocr_text_") else {
        return hit_id.to_string();
    };
    let Some((prefix, slug)) = rest.split_once('_') else {
        return rest.to_string();
    };
    if prefix.chars().all(|ch| ch.is_ascii_digit()) && !slug.is_empty() {
        slug.to_string()
    } else {
        rest.to_string()
    }
}

fn browser_app_name(app_name: &str) -> bool {
    let app = app_name.to_ascii_lowercase();
    ["firefox", "chrome", "chromium", "safari", "edge", "arc"]
        .iter()
        .any(|needle| app.contains(needle))
}

fn parse_key_with_count(rest: &str) -> (&str, usize) {
    match rest.rsplit_once(':') {
        Some((key, count)) if !key.is_empty() => {
            (key, count.parse::<usize>().unwrap_or(1).clamp(1, 20))
        }
        _ => (rest, 1),
    }
}

fn parse_point_action(rest: &str) -> Option<(f64, f64, &str)> {
    let (point, action) = rest.rsplit_once(':')?;
    let (x, y) = parse_point(point)?;
    Some((x, y, action))
}

fn parse_point(point: &str) -> Option<(f64, f64)> {
    let (x, y) = point.split_once(',')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn validate_type_keys_target_id(target_id: &str) -> dunst_core::Result<()> {
    validate_hashed_text_target_id(target_id, "keyboard@type_keys:", "type_keys")
}

fn validate_paste_text_target_id(target_id: &str) -> dunst_core::Result<()> {
    validate_hashed_text_target_id(target_id, "keyboard@paste_text:", "paste_text")
}

fn validate_set_field_text_target_id(target_id: &str) -> dunst_core::Result<()> {
    validate_hashed_text_target_id(target_id, "keyboard@set_field_text:", "set_field_text")
}

fn validate_hashed_text_target_id(
    target_id: &str,
    prefix: &str,
    label: &str,
) -> dunst_core::Result<()> {
    let rest = target_id
        .strip_prefix(prefix)
        .ok_or_else(|| DunstError::Execution(format!("{target_id} is not a {label} target")))?;
    let (hash, count) = rest.rsplit_once(':').ok_or_else(|| {
        DunstError::Execution(format!("{target_id} is not a valid {label} target"))
    })?;
    let hash_is_hex = hash.len() == 16 && hash.chars().all(|ch| ch.is_ascii_hexdigit());
    let count = count.parse::<usize>().ok();
    if hash_is_hex && count.is_some_and(|count| count > 0) {
        Ok(())
    } else {
        Err(DunstError::Execution(format!(
            "{target_id} is not a valid {label} target"
        )))
    }
}

fn validate_scroll_target(
    rest: &str,
    expects_point: bool,
    engine: &Engine,
) -> dunst_core::Result<()> {
    let parts: Vec<_> = rest.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(DunstError::Execution(format!(
            "scroll target {rest:?} must be direction:count[:x,y]"
        )));
    }
    if !matches!(parts[0], "up" | "down" | "top" | "bottom") {
        return Err(DunstError::Execution(format!(
            "scroll target {rest:?} has unsupported direction {:?}",
            parts[0]
        )));
    }
    let Some(count) = parts[1].parse::<usize>().ok() else {
        return Err(DunstError::Execution(format!(
            "scroll target {rest:?} has invalid count {:?}",
            parts[1]
        )));
    };
    if !(1..=20).contains(&count) {
        return Err(DunstError::Execution(format!(
            "scroll target {rest:?} count must be in 1..=20"
        )));
    }
    if expects_point {
        let Some(point) = parts.get(2).and_then(|raw| parse_point(raw)) else {
            return Err(DunstError::Execution(format!(
                "scroll target {rest:?} must include x,y"
            )));
        };
        engine.ensure_point_in_target_window(point.0, point.1, "approve")?;
    } else if parts.len() == 3 {
        return Err(DunstError::Execution(format!(
            "keyboard scroll target {rest:?} must not include x,y"
        )));
    }
    Ok(())
}
