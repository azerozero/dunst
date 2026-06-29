use super::*;
use std::hash::{Hash, Hasher};

mod perception;

impl Engine {
    /// IDs whose affordance offers `action`. WP-J/J2: latent (off-screen /
    /// zero-bbox) nodes — e.g. collapsed-menu items — are omitted by default so
    /// the agent isn't handed phantom targets. The gated action path is
    /// unaffected: it resolves ids against the graph, not this listing.
    ///
    /// Ergonomic default over [`query_affordances_filtered`](Self::query_affordances_filtered);
    /// the MCP server calls the latter directly, so in the binary this wrapper is
    /// exercised only by callers/tests that want the filtered listing.
    // `expect` is scoped to non-test builds: these fns ARE used by the test module,
    // so a bare `#[expect(dead_code)]` would be "unfulfilled" under the test target.
    // In the binary they are genuinely dead — and clippy will flag this expectation
    // the moment a non-test caller appears (the point of `expect` over `allow`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "ergonomic unfiltered wrapper, exercised only by tests"
        )
    )]
    pub fn query_affordances(&self, action: SemanticAction) -> Vec<String> {
        self.query_affordances_filtered(action, false)
    }

    /// As [`query_affordances`](Self::query_affordances), but `include_latent`
    /// returns every id exposing `action`, latent ones included.
    pub fn query_affordances_filtered(
        &self,
        action: SemanticAction,
        include_latent: bool,
    ) -> Vec<String> {
        self.query_affordances_scoped(action, include_latent, "all")
    }

    pub fn query_affordances_scoped(
        &self,
        action: SemanticAction,
        include_latent: bool,
        scope: &str,
    ) -> Vec<String> {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        let mut ids: Vec<String> = self
            .affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .filter(|a| {
                include_latent
                    || g.get(&a.id)
                        .map(|n| node_visible_or_menu(n, window_rect, menubar))
                        .unwrap_or(false)
            })
            .filter(|a| {
                g.get(&a.id)
                    .map(|n| node_matches_scope(g, n, window_rect, menubar, scope))
                    .unwrap_or(false)
            })
            .map(|a| a.id.clone())
            .collect();
        if action == SemanticAction::Scroll && matches!(scope, "all" | "page" | "content") {
            ids.extend(["down", "up", "bottom", "top"].map(page_scroll_target_id));
        }
        ids
    }

    /// WP-J/J2: the affordance graph as JSON, latent nodes omitted unless
    /// `include_latent`. Shape matches [`AffordanceGraph`] (`{ "affordances": … }`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "compatibility wrapper for tests and internal callers; MCP uses the scoped variant"
        )
    )]
    pub fn affordances_view(&self, include_latent: bool) -> Value {
        self.affordances_view_scoped(include_latent, "all")
    }

    pub fn affordances_view_scoped(&self, include_latent: bool, scope: &str) -> Value {
        let ag = self.affordance_graph();
        if include_latent && scope == "all" {
            return serde_json::to_value(ag).unwrap_or(Value::Null);
        }
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        let mut map = serde_json::Map::new();
        for (id, aff) in &ag.affordances {
            if g.get(id).is_some_and(|n| {
                (include_latent || node_visible_or_menu(n, window_rect, menubar))
                    && node_matches_scope(g, n, window_rect, menubar, scope)
            }) {
                map.insert(id.clone(), serde_json::to_value(aff).unwrap_or(Value::Null));
            }
        }
        json!({ "affordances": Value::Object(map) })
    }

    /// Semantic targets with safe click zones and a UI epoch. This is the
    /// compact "act on this button" read path: it combines scene nodes,
    /// affordances, risk, browser tab state, and target visibility so agents do
    /// not have to stitch together several tools before avoiding raw coords.
    pub fn hit_targets(
        &self,
        include_latent: bool,
        scope: &str,
        limit: usize,
        previous_epoch: Option<&str>,
    ) -> HitTargetsResult {
        let limit = limit.clamp(1, 500);
        let g = self.scene_graph();
        let ag = self.affordance_graph();
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let window = self.current_window_bounds();
        let browser_tab = self
            .list_browser_tabs(None, true)
            .into_iter()
            .find(|tab| tab.selected);
        let target_visibility = self.target_visibility();
        let target = TargetState {
            pid: g.window.pid,
            window_id: g.window.window_id,
            app_name: g.window.app_name.clone(),
        };
        let ui_epoch = self.ui_epoch(window, browser_tab.clone(), target_visibility.clone(), ag);
        let state_changed = previous_epoch.is_some_and(|previous| previous != ui_epoch.fingerprint);
        let stale_reason = state_changed.then(|| {
            "ui_epoch changed; the window, selected tab, visibility, or actionable graph no longer matches the caller's previous view"
                .to_string()
        });
        let resume_hint = if state_changed {
            Some(
                "Discard cached coordinates and call get_hit_targets again before clicking, dragging, or typing."
                    .to_string(),
            )
        } else if !target_visibility.warnings.is_empty() {
            Some("Resolve target_visibility warnings before OCR, screenshot, or raw pointer actions.".to_string())
        } else {
            None
        };

        let mut targets = Vec::new();
        for (id, affordance) in &ag.affordances {
            let Some(node) = g.get(id) else { continue };
            if affordance.actions.is_empty() && affordance.drag_targets.is_empty() {
                continue;
            }
            if !include_latent && !node_visible_or_menu(node, window_rect, menubar) {
                continue;
            }
            if !node_matches_scope(g, node, window_rect, menubar, scope) {
                continue;
            }

            let action_modes = hit_action_modes(affordance);
            if action_modes.is_empty() {
                continue;
            }
            targets.push(HitTarget {
                id: node.id.clone(),
                source: source_name(node.source).to_string(),
                role: node.role.as_str(),
                label: node.label.clone().or_else(|| node.help.clone()),
                value: node.value.clone(),
                bbox: node.bbox,
                safe_click: node.bbox.and_then(safe_click_zone),
                confidence: node.confidence,
                action_modes,
                risk: affordance.risk.clone(),
            });
        }
        let mut supplemental_warnings = Vec::new();
        if scope != "browser_chrome" && scope != "chrome" {
            append_page_scroll_targets(&mut targets, window);
            self.append_ocr_hit_targets(&mut targets, &mut supplemental_warnings, limit);
            self.append_shape_hit_targets(&mut targets, &mut supplemental_warnings, limit);
        }
        targets.sort_by(hit_target_order);
        targets.truncate(limit);

        HitTargetsResult {
            target,
            title: g.window.title.clone(),
            window,
            browser_tab,
            target_visibility,
            ui_epoch,
            previous_epoch: previous_epoch.map(str::to_string),
            state_changed,
            stale_reason,
            resume_hint,
            supplemental_warnings,
            targets,
        }
    }

    pub fn current_ui_epoch_fingerprint(&self) -> String {
        let browser_tab = self
            .list_browser_tabs(None, true)
            .into_iter()
            .find(|tab| tab.selected);
        let target_visibility = self.target_visibility();
        self.ui_epoch(
            self.current_window_bounds(),
            browser_tab,
            target_visibility,
            self.affordance_graph(),
        )
        .fingerprint
    }

    /// OCR/vision fallback for the JSON-facing `find_element` read path. Action
    /// resolution remains AX-only because synthetic hit targets are not
    /// `SceneNode`s and must be driven through their advertised raw/OCR tools.
    pub fn find_element_hit_target_fallback(&self, query: &str, limit: usize) -> Vec<HitTarget> {
        let q = normalize_match(query);
        let mut targets: Vec<HitTarget> = self
            .hit_targets(false, "page", 500, None)
            .targets
            .into_iter()
            .filter(|target| matches!(target.source.as_str(), "ocr" | "vision"))
            .filter(|target| hit_target_matches_find_query(target, &q))
            .collect();
        targets.truncate(limit.clamp(1, 500));
        targets
    }

    fn append_ocr_hit_targets(
        &self,
        targets: &mut Vec<HitTarget>,
        warnings: &mut Vec<String>,
        limit: usize,
    ) {
        match self.extract_ocr_cards(false, true, limit.min(24)) {
            Ok(result) => {
                warnings.extend(result.warnings);
                for card in result.cards {
                    if bbox_duplicate_of_existing(card.bbox, targets) {
                        continue;
                    }
                    let risk = raw_ocr_click_risk(self.risk.assess_text(&card.lines.join(" ")));
                    targets.push(card_hit_target(card, risk));
                }
            }
            Err(err) => warnings.push(format!("OCR card targets unavailable: {err}")),
        }

        match self.read_text_detailed(None, false, true) {
            Ok(result) => {
                warnings.extend(result.warnings);
                append_ocr_form_field_targets(targets, &result.hits, &self.risk, limit.min(20));
                for (idx, hit) in result.hits.iter().enumerate().take(limit.min(80)) {
                    if hit.confidence < 0.45 || bbox_duplicate_of_existing(hit.bbox, targets) {
                        continue;
                    }
                    let risk = raw_ocr_click_risk(self.risk.assess_text(&hit.text));
                    targets.push(ocr_hit_target(idx, hit, risk));
                }
            }
            Err(err) => warnings.push(format!("OCR text targets unavailable: {err}")),
        }
    }

    fn append_shape_hit_targets(
        &self,
        targets: &mut Vec<HitTarget>,
        warnings: &mut Vec<String>,
        limit: usize,
    ) {
        match self.read_shapes() {
            Ok(shapes) => {
                for (idx, shape) in shapes.into_iter().enumerate().take(limit.min(40)) {
                    if shape.confidence < 0.35 || bbox_duplicate_of_existing(shape.bbox, targets) {
                        continue;
                    }
                    targets.push(shape_hit_target(idx, shape));
                }
            }
            Err(err) => warnings.push(format!("vision shape targets unavailable: {err}")),
        }
    }

    fn ui_epoch(
        &self,
        window: Bbox,
        browser_tab: Option<BrowserTab>,
        target_visibility: TargetVisibility,
        affordances: &AffordanceGraph,
    ) -> UiEpoch {
        let g = self.scene_graph();
        let fingerprint = self.ui_fingerprint(
            window,
            browser_tab.as_ref(),
            &target_visibility,
            affordances,
        );
        UiEpoch {
            fingerprint,
            captured_at_ms: g.captured_at_ms,
            target: TargetState {
                pid: g.window.pid,
                window_id: g.window.window_id,
                app_name: g.window.app_name.clone(),
            },
            title: g.window.title.clone(),
            window,
            browser_tab,
            target_visibility_status: target_visibility.status.clone(),
            visible_fraction: target_visibility.visible_fraction,
            covered_by: target_visibility
                .covered_by
                .iter()
                .map(|w| w.window_id)
                .collect(),
            warnings: target_visibility.warnings.clone(),
        }
    }

    fn ui_fingerprint(
        &self,
        window: Bbox,
        browser_tab: Option<&BrowserTab>,
        target_visibility: &TargetVisibility,
        affordances: &AffordanceGraph,
    ) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let g = self.scene_graph();
        g.window.pid.hash(&mut hasher);
        g.window.window_id.hash(&mut hasher);
        g.window.app_name.hash(&mut hasher);
        g.window.title.hash(&mut hasher);
        hash_bbox(window, &mut hasher);
        if let Some(tab) = browser_tab {
            tab.id.hash(&mut hasher);
            tab.title.hash(&mut hasher);
            tab.url.hash(&mut hasher);
            tab.selected.hash(&mut hasher);
            hash_optional_bbox(tab.bbox, &mut hasher);
        }
        target_visibility.status.hash(&mut hasher);
        rounded_i64(target_visibility.visible_fraction * 10_000.0).hash(&mut hasher);
        for window in &target_visibility.covered_by {
            window.window_id.hash(&mut hasher);
            window.title.hash(&mut hasher);
            hash_bbox(window.bounds, &mut hasher);
        }

        for (id, node) in &g.nodes {
            id.hash(&mut hasher);
            node.role.as_str().hash(&mut hasher);
            node.label.hash(&mut hasher);
            node.value.hash(&mut hasher);
            node.enabled.hash(&mut hasher);
            node.focused.hash(&mut hasher);
            hash_optional_bbox(node.bbox, &mut hasher);
            if let Some(affordance) = affordances.affordances.get(id) {
                affordance.actions.hash(&mut hasher);
                affordance.drag_targets.hash(&mut hasher);
            }
        }
        format!("{:016x}", hasher.finish())
    }

    /// WP-J/J1: the scene graph under a projection `view`, optionally limited to
    /// actionable nodes. `Full` without `actionable_only` is byte-for-byte the
    /// old `get_scene_graph` payload (the escape hatch).
    pub fn scene_graph_view(&self, view: SceneView, actionable_only: bool) -> Value {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        match view {
            SceneView::Full if !actionable_only => serde_json::to_value(g).unwrap_or(Value::Null),
            SceneView::Full => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if node_actionable(n, window_rect, menubar) {
                        map.insert(id.clone(), serde_json::to_value(n).unwrap_or(Value::Null));
                    }
                }
                json!({
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Compact => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if actionable_only && !node_actionable(n, window_rect, menubar) {
                        continue;
                    }
                    map.insert(id.clone(), compact_node(n));
                }
                json!({
                    "view": "compact",
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Summary => {
                let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
                let mut n_actionable = 0usize;
                for n in g.nodes.values() {
                    *counts.entry(n.role.as_str()).or_insert(0) += 1;
                    if node_actionable(n, window_rect, menubar) {
                        n_actionable += 1;
                    }
                }
                json!({
                    "view": "summary",
                    "n_nodes": g.nodes.len(),
                    "roots": g.roots,
                    "counts_by_role": counts,
                    "n_actionable": n_actionable,
                    "window": g.window,
                })
            }
        }
    }

    /// Browser-tab projection from the current AX graph. Firefox/Chrome expose
    /// visible tab-strip tabs as AXRadioButton nodes near the top of the window;
    /// using this avoids confusing a page/sidebar item named "ClaudeAI" with a
    /// real browser tab.
    pub fn list_browser_tabs(&self, query: Option<&str>, visible_only: bool) -> Vec<BrowserTab> {
        let q = query.map(normalize_match);
        let window_rect = self.cached_window_rect;
        let has_explicit_selection = self.scene_graph().nodes.values().any(|node| {
            node.role == Role::Radio
                && node.ax_role == "AXRadioButton"
                && looks_like_browser_tab(node, window_rect)
                && (!visible_only || node_on_screen(node, window_rect))
                && browser_tab_explicitly_selected(node)
        });
        let mut tabs = Vec::new();

        for node in self.scene_graph().nodes.values() {
            if node.role != Role::Radio || node.ax_role != "AXRadioButton" {
                continue;
            }
            if !looks_like_browser_tab(node, window_rect) {
                continue;
            }
            if visible_only && !node_on_screen(node, window_rect) {
                continue;
            }

            let title = browser_tab_title(self.scene_graph(), node);
            if title.is_empty() {
                continue;
            }
            if let Some(q) = q.as_deref() {
                let haystack = format!("{} {}", normalize_match(&node.id), normalize_match(&title));
                if !normalized_contains_query(&haystack, q) {
                    continue;
                }
            }

            let selected =
                browser_tab_selected(self.scene_graph(), node, &title, has_explicit_selection);
            tabs.push(BrowserTab {
                id: node.id.clone(),
                url: likely_url(&title),
                title,
                selected,
                bbox: node.bbox,
            });
        }

        tabs.sort_by(|a, b| {
            let ay = a.bbox.map(|b| b.y).unwrap_or(f64::MAX);
            let by = b.bbox.map(|b| b.y).unwrap_or(f64::MAX);
            ay.partial_cmp(&by)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ax = a.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    let bx = b.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        if tabs.is_empty() {
            if let Some(tab) =
                fallback_browser_tab_from_window_title(self.scene_graph(), q.as_deref())
            {
                tabs.push(tab);
            }
        }
        tabs
    }

    /// Lightweight orientation snapshot: window title, likely URL, visible text
    /// snippets and key visible action targets. Intended for "where am I?" checks
    /// without requesting a screenshot or full graph.
    pub fn page_state(&self, limit: usize) -> PageState {
        let limit = limit.clamp(1, 50);
        let g = self.scene_graph();
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let suppressed_repetitive_destructive = page_state_repetitive_destructive_keys(
            g,
            self.affordance_graph(),
            window_rect,
            menubar,
        );

        let mut visible_text = Vec::new();
        let mut key_elements = Vec::new();
        let mut url = None;
        let browser_tab = self
            .list_browser_tabs(None, true)
            .into_iter()
            .find(|tab| tab.selected);

        for node in g.nodes.values() {
            if !node_visible_or_menu(node, window_rect, menubar) {
                continue;
            }
            let chrome = page_state_chrome_node(g, node, window_rect, menubar);

            if url.is_none() {
                url = node
                    .value
                    .as_deref()
                    .or(node.label.as_deref())
                    .and_then(likely_url);
            }

            if !chrome
                && matches!(
                    node.role,
                    Role::StaticText | Role::TextField | Role::TextArea
                )
            {
                if let Some(text) = node
                    .label
                    .as_deref()
                    .or(node.value.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    push_unique_string(&mut visible_text, text, limit);
                }
            }

            if key_elements.len() < limit
                && !chrome
                && !page_state_suppressed_repetitive_destructive(
                    node,
                    &suppressed_repetitive_destructive,
                )
                && page_state_key_element_candidate(node, window_rect, menubar)
                && node.enabled
                && self
                    .affordance_graph()
                    .affordances
                    .get(&node.id)
                    .map(|a| !a.actions.is_empty())
                    .unwrap_or(false)
            {
                key_elements.push(KeyElement {
                    id: node.id.clone(),
                    role: node.role.as_str(),
                    label: node.label.clone(),
                    value: node.value.clone(),
                    bbox: node.bbox,
                });
            }

            if visible_text.len() >= limit && key_elements.len() >= limit && url.is_some() {
                break;
            }
        }
        if url.is_none() {
            url = browser_tab.as_ref().and_then(|tab| tab.url.clone());
        }
        if visible_text.is_empty() && is_browser_app_name(&g.window.app_name) {
            if let Ok(result) = self.read_text_detailed(None, false, true) {
                for hit in result.hits.iter().filter(|hit| hit.confidence >= 0.45) {
                    let text = hit.text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    push_unique_string(&mut visible_text, text, limit);
                    if visible_text.len() >= limit {
                        break;
                    }
                }
            }
        }

        PageState {
            target: TargetState {
                pid: g.window.pid,
                window_id: g.window.window_id,
                app_name: g.window.app_name.clone(),
            },
            title: g.window.title.clone(),
            url,
            browser_tab,
            target_visibility: self.target_visibility(),
            visible_text,
            key_elements,
        }
    }

    /// AX-only text extraction for LLM chats and document-like pages. This is
    /// lighter than `get_scene_graph full` and more reliable than OCR when the
    /// browser exposes response text through accessibility.
    pub fn text_snapshot(
        &self,
        query: Option<&str>,
        visible_only: bool,
        limit: usize,
    ) -> Vec<TextSnippet> {
        let limit = limit.clamp(1, 500);
        let q = query.map(normalize_match);
        let g = self.scene_graph();
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let mut snippets = Vec::new();

        for node in g.nodes.values() {
            if !matches!(
                node.role,
                Role::StaticText | Role::TextField | Role::TextArea
            ) {
                continue;
            }

            let (primary, secondary) = match node.role {
                Role::TextField | Role::TextArea => (node.value.as_deref(), node.label.as_deref()),
                _ => (node.label.as_deref(), node.value.as_deref()),
            };
            let text = primary
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| secondary.map(str::trim).filter(|s| !s.is_empty()));
            let Some(text) = text else {
                continue;
            };

            let visible = node_visible_or_menu(node, window_rect, menubar);
            if visible_only && !visible {
                continue;
            }
            if read_chrome_node(g, node, window_rect, menubar) {
                continue;
            }

            if let Some(q) = q.as_deref() {
                let haystack = format!(
                    "{} {} {}",
                    normalize_match(&node.id),
                    node.role.as_str(),
                    normalize_match(text)
                );
                if !normalized_contains_query(&haystack, q) {
                    continue;
                }
            }

            snippets.push(TextSnippet {
                id: node.id.clone(),
                role: node.role.as_str(),
                text: text.to_string(),
                visible,
                bbox: node.bbox,
            });
        }

        snippets.sort_by(|a, b| {
            let avis = if a.visible { 0 } else { 1 };
            let bvis = if b.visible { 0 } else { 1 };
            avis.cmp(&bvis)
                .then_with(|| {
                    let ay = a.bbox.map(|b| b.y).unwrap_or(f64::MAX);
                    let by = b.bbox.map(|b| b.y).unwrap_or(f64::MAX);
                    ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    let ax = a.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    let bx = b.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        snippets.truncate(limit);
        snippets
    }

    pub(super) fn ax_terminal_text_hits(&self, region: Option<Bbox>) -> Vec<TextHit> {
        if !is_terminal_app_name(&self.window.app_name) && !is_terminal_app_name(&self.window.title)
        {
            return Vec::new();
        }

        let fallback_bbox = self.current_window_bounds();
        let mut hits = Vec::new();
        for node in self.scene_graph().nodes.values() {
            if node.role != Role::TextArea {
                continue;
            }
            let bbox = node.bbox.unwrap_or(fallback_bbox);
            if region.map(|r| !bbox_intersects(bbox, r)).unwrap_or(false) {
                continue;
            }
            let Some(text) = node.value.as_deref().or(node.label.as_deref()) else {
                continue;
            };
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                hits.push(TextHit {
                    text: line.to_string(),
                    bbox,
                    confidence: 1.0,
                });
                if hits.len() >= 500 {
                    return hits;
                }
            }
            if hits.is_empty() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    hits.push(TextHit {
                        text: trimmed.to_string(),
                        bbox,
                        confidence: 1.0,
                    });
                }
            }
        }
        hits
    }

    pub(super) fn current_window_bounds(&self) -> Bbox {
        #[cfg(target_os = "macos")]
        if let Some((x, y, w, h)) = dunst_vision::capture::window_bounds(self.target.window_id) {
            return Bbox { x, y, w, h };
        }
        self.cached_window_rect.unwrap_or(Bbox {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        })
    }

    #[cfg(target_os = "macos")]
    pub(super) fn display_for_window(&self, window: Bbox) -> Option<DisplaySummary> {
        dunst_vision::capture::display_for_rect(window.x, window.y, window.w, window.h)
            .map(display_summary)
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn display_for_window(&self, _window: Bbox) -> Option<DisplaySummary> {
        None
    }
}

fn fallback_browser_tab_from_window_title(
    graph: &SceneGraph,
    query: Option<&str>,
) -> Option<BrowserTab> {
    if !is_browser_app_name(&graph.window.app_name) {
        return None;
    }
    let title = graph.window.title.trim();
    if title.is_empty() {
        return None;
    }
    let id = "tab_fallback_window_title";
    if let Some(q) = query {
        let haystack = format!("{id} {}", normalize_match(title));
        if !normalized_contains_query(&haystack, q) {
            return None;
        }
    }
    Some(BrowserTab {
        id: id.into(),
        url: likely_url(title),
        title: title.into(),
        selected: true,
        bbox: None,
    })
}

fn node_matches_scope(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar: Option<&str>,
    scope: &str,
) -> bool {
    match scope {
        "all" | "" => true,
        "page" => !read_chrome_node(graph, node, window_rect, menubar),
        "browser_chrome" | "chrome" => read_chrome_node(graph, node, window_rect, menubar),
        _ => true,
    }
}

fn hit_action_modes(affordance: &dunst_core::Affordance) -> Vec<HitActionMode> {
    affordance
        .actions
        .iter()
        .map(|action| HitActionMode {
            action: *action,
            tool_hint: tool_hint_for_action(*action).to_string(),
            target_id: Some(affordance.id.clone()),
            arguments: None,
            drop_targets: if *action == SemanticAction::Drag {
                affordance.drag_targets.clone()
            } else {
                Vec::new()
            },
            risk: affordance.risk.clone(),
        })
        .collect()
}

fn append_page_scroll_targets(targets: &mut Vec<HitTarget>, window: Bbox) {
    if window.w <= 0.0 || window.h <= 0.0 {
        return;
    }
    let risk = page_scroll_risk();
    let bbox = Bbox {
        x: window.x + window.w * 0.12,
        y: window.y + window.h * 0.16,
        w: window.w * 0.60,
        h: window.h * 0.76,
    };
    for direction in ["down", "up", "bottom", "top"] {
        let id = page_scroll_target_id(direction);
        targets.push(HitTarget {
            id: id.clone(),
            source: "page".into(),
            role: "group",
            label: Some(format!("Page scroll {direction}")),
            value: None,
            bbox: Some(bbox),
            safe_click: synthetic_safe_zone(
                bbox,
                "page_scroll_region",
                "Use the scroll tool with this pseudo-target id; for scroll_at, prefer an OCR text/card point inside the content instead of a blank gutter.",
            ),
            confidence: 0.65,
            action_modes: vec![HitActionMode {
                action: SemanticAction::Scroll,
                tool_hint: "scroll".into(),
                target_id: Some(id.clone()),
                arguments: Some(json!({
                    "id": id,
                    "direction": direction,
                    "pages": if matches!(direction, "top" | "bottom") { 1 } else { 3 },
                })),
                drop_targets: Vec::new(),
                risk: risk.clone(),
            }],
            risk: risk.clone(),
        });
    }
}

fn page_scroll_risk() -> RiskAssessment {
    RiskAssessment {
        level: RiskLevel::High,
        requires_approval: true,
        reasons: vec![
            "page pseudo-scroll uses raw keyboard input when no AX scroll container is available"
                .into(),
        ],
    }
}

/// OCR-derived hit targets can only be actuated through raw pointer input
/// (`click_near_text`), which the executor always approval-gates (see
/// `ocr_point_risk_at`). Floor the advertised affordance risk to that same gate
/// so `get_hit_targets` never promises a no-approval click that the action layer
/// then blocks with `pending_approval`. Text-derived reasons (e.g. destructive
/// keywords) are preserved on top of the raw-input reason.
fn raw_ocr_click_risk(text_risk: RiskAssessment) -> RiskAssessment {
    let mut reasons =
        vec!["click is delivered as approval-gated raw OCR input, not an AX element".to_string()];
    reasons.extend(text_risk.reasons);
    RiskAssessment {
        level: RiskLevel::High,
        requires_approval: true,
        reasons,
    }
}

#[derive(Clone, Copy)]
struct OcrFormLabel {
    role: &'static str,
    max_gap_y: f64,
}

fn append_ocr_form_field_targets(
    targets: &mut Vec<HitTarget>,
    hits: &[TextHit],
    risk_engine: &RiskEngine,
    max_fields: usize,
) {
    let mut added = 0usize;
    for (idx, label_hit) in hits.iter().enumerate() {
        if added >= max_fields {
            break;
        }
        let Some(kind) = ocr_form_label_kind(&label_hit.text) else {
            continue;
        };
        let Some(value_hit) = nearest_ocr_field_value(label_hit, &hits[idx + 1..], kind) else {
            continue;
        };
        if value_hit.confidence < 0.45
            || bbox_duplicate_of_existing_form_field(value_hit.bbox, targets)
        {
            continue;
        }
        let label_center = bbox_center(label_hit.bbox);
        let value_center = bbox_center(value_hit.bbox);
        let offset = (
            value_center.0 - label_center.0,
            value_center.1 - label_center.1,
        );
        let text = format!("{} {}", label_hit.text, value_hit.text);
        let risk = raw_ocr_click_risk(risk_engine.assess_text(&text));
        targets.push(ocr_form_field_target(
            idx, kind, label_hit, value_hit, offset, risk,
        ));
        added += 1;
    }
}

fn ocr_form_label_kind(text: &str) -> Option<OcrFormLabel> {
    let normalized = normalize_match(text);
    let compact: String = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if normalized.contains("description") {
        return Some(OcrFormLabel {
            role: "text_area",
            max_gap_y: 120.0,
        });
    }
    if normalized.contains("titre")
        || normalized.contains("title")
        || compact.contains("realisation")
        || compact.contains("realization")
    {
        return Some(OcrFormLabel {
            role: "text_field",
            max_gap_y: 80.0,
        });
    }
    None
}

fn nearest_ocr_field_value<'a>(
    label: &TextHit,
    candidates: &'a [TextHit],
    kind: OcrFormLabel,
) -> Option<&'a TextHit> {
    candidates
        .iter()
        .filter(|candidate| candidate.confidence >= 0.45)
        .filter(|candidate| ocr_form_label_kind(&candidate.text).is_none())
        .filter(|candidate| candidate.bbox.y >= label.bbox.y + label.bbox.h * 0.45)
        .filter(|candidate| candidate.bbox.y - label.bbox.y <= kind.max_gap_y)
        .filter(|candidate| ocr_form_field_x_aligned(label.bbox, candidate.bbox))
        .min_by(|a, b| {
            let ay = (a.bbox.y - label.bbox.y).abs();
            let by = (b.bbox.y - label.bbox.y).abs();
            ay.partial_cmp(&by)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ax = (a.bbox.x - label.bbox.x).abs();
                    let bx = (b.bbox.x - label.bbox.x).abs();
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
        })
}

fn ocr_form_field_x_aligned(label: Bbox, value: Bbox) -> bool {
    let overlap = (label.x + label.w).min(value.x + value.w) - label.x.max(value.x);
    let overlap_ratio = overlap.max(0.0) / label.w.max(1.0);
    overlap_ratio >= 0.15 || (value.x - label.x).abs() <= 96.0
}

fn bbox_center(bbox: Bbox) -> (f64, f64) {
    (bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0)
}

fn bbox_duplicate_of_existing_form_field(bbox: Bbox, targets: &[HitTarget]) -> bool {
    let area = (bbox.w.max(0.0) * bbox.h.max(0.0)).max(1.0);
    targets.iter().any(|target| {
        if target.source == "page" || (target.source == "ocr" && target.role == "group") {
            return false;
        }
        target
            .bbox
            .map(|existing| rect_intersection_area(existing, bbox) / area > 0.72)
            .unwrap_or(false)
    })
}

fn ocr_form_field_target(
    idx: usize,
    kind: OcrFormLabel,
    label_hit: &TextHit,
    value_hit: &TextHit,
    offset: (f64, f64),
    risk: RiskAssessment,
) -> HitTarget {
    let id = format!(
        "ocr_field_{idx}_{}",
        compact_synthetic_label(&label_hit.text)
    );
    HitTarget {
        id,
        source: "ocr".into(),
        role: kind.role,
        label: Some(label_hit.text.clone()),
        value: Some(value_hit.text.clone()),
        bbox: Some(value_hit.bbox),
        safe_click: synthetic_safe_zone(
            value_hit.bbox,
            "ocr_form_value_bbox_inset",
            "Use click_near_text with the supplied label-relative offset, then verify with OCR before typing or saving.",
        ),
        confidence: label_hit.confidence.min(value_hit.confidence),
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": label_hit.text.clone(),
                "content_only": true,
                "accurate": false,
                "offset_x": offset.0,
                "offset_y": offset.1,
                "expected_text": value_hit.text.clone(),
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

fn card_hit_target(card: OcrCard, risk: RiskAssessment) -> HitTarget {
    let mut value = card
        .lines
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    if value.is_empty() {
        value = card.title.clone();
    }
    HitTarget {
        id: card.id.clone(),
        source: "ocr".into(),
        role: "group",
        label: Some(card.title.clone()),
        value: Some(value),
        bbox: Some(card.bbox),
        safe_click: synthetic_safe_zone(
            card.bbox,
            "ocr_card_bbox_inset",
            "Prefer click_near_text with the card title; use the zone only after OCR verification.",
        ),
        confidence: card.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": card.title,
                "content_only": true,
                "accurate": false,
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

fn ocr_hit_target(idx: usize, hit: &TextHit, risk: RiskAssessment) -> HitTarget {
    let id = format!("ocr_text_{idx}_{}", compact_synthetic_label(&hit.text));
    HitTarget {
        id,
        source: "ocr".into(),
        role: "static_text",
        label: Some(hit.text.clone()),
        value: None,
        bbox: Some(hit.bbox),
        safe_click: synthetic_safe_zone(
            hit.bbox,
            "ocr_text_bbox_inset",
            "Prefer click_near_text with this text; use the zone only after OCR verification.",
        ),
        confidence: hit.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": hit.text,
                "content_only": true,
                "accurate": false,
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

fn shape_hit_target(idx: usize, shape: ShapeHit) -> HitTarget {
    let center = (
        shape.bbox.x + shape.bbox.w / 2.0,
        shape.bbox.y + shape.bbox.h / 2.0,
    );
    let risk = RiskAssessment {
        level: RiskLevel::Medium,
        requires_approval: false,
        reasons: vec!["vision-derived shape target; verify semantics before mutating".into()],
    };
    HitTarget {
        id: format!(
            "vision_shape_{idx}_{}",
            compact_synthetic_label(&shape.kind)
        ),
        source: "vision".into(),
        role: "image",
        label: Some(format!("{} shape", shape.kind)),
        value: None,
        bbox: Some(shape.bbox),
        safe_click: synthetic_safe_zone(
            shape.bbox,
            "vision_shape_bbox_inset",
            "Use read_at/hover verification before any raw click on this shape.",
        ),
        confidence: shape.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Hover,
            tool_hint: "read_at".into(),
            target_id: None,
            arguments: Some(json!({ "x": center.0, "y": center.1 })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

fn tool_hint_for_action(action: SemanticAction) -> &'static str {
    match action {
        SemanticAction::Click | SemanticAction::Toggle | SemanticAction::Focus => "click_element",
        SemanticAction::Hover => "hover_probe",
        SemanticAction::Type => "type_into",
        SemanticAction::OpenMenu => "open_menu",
        SemanticAction::Pick => "pick_option",
        SemanticAction::Scroll => "scroll",
        SemanticAction::Drag => "drag_element",
        SemanticAction::Raise => "raise_element",
        SemanticAction::KeyPress => "press_key",
        SemanticAction::Hotkey => "hotkey",
    }
}

fn safe_click_zone(bbox: Bbox) -> Option<SafeClickZone> {
    if bbox.w <= 0.0 || bbox.h <= 0.0 {
        return None;
    }
    let inset = (bbox.w.min(bbox.h) * 0.12).min(8.0);
    let inset = if bbox.w - inset * 2.0 >= 4.0 && bbox.h - inset * 2.0 >= 4.0 {
        inset
    } else {
        0.0
    };
    let zone = Bbox {
        x: bbox.x + inset,
        y: bbox.y + inset,
        w: bbox.w - inset * 2.0,
        h: bbox.h - inset * 2.0,
    };
    Some(SafeClickZone {
        bbox: zone,
        center: (zone.x + zone.w / 2.0, zone.y + zone.h / 2.0),
        source: "accessibility_bbox_inset".into(),
        note: "Prefer click_element by id; use this zone only when an element-bound click is unavailable."
            .into(),
    })
}

fn synthetic_safe_zone(bbox: Bbox, source: &str, note: &str) -> Option<SafeClickZone> {
    safe_click_zone(bbox).map(|mut zone| {
        zone.source = source.to_string();
        zone.note = note.to_string();
        zone
    })
}

fn bbox_duplicate_of_existing(bbox: Bbox, targets: &[HitTarget]) -> bool {
    let area = (bbox.w.max(0.0) * bbox.h.max(0.0)).max(1.0);
    targets.iter().any(|target| {
        if target.source == "page" {
            return false;
        }
        target
            .bbox
            .map(|existing| rect_intersection_area(existing, bbox) / area > 0.72)
            .unwrap_or(false)
    })
}

fn compact_synthetic_label(text: &str) -> String {
    let normalized = normalize_match(text);
    let mut out = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "target".into()
    } else {
        trimmed.into()
    }
}

fn hit_target_order(a: &HitTarget, b: &HitTarget) -> std::cmp::Ordering {
    hit_source_rank(&a.source)
        .cmp(&hit_source_rank(&b.source))
        .then_with(|| hit_target_position_order(a, b))
}

fn hit_target_position_order(a: &HitTarget, b: &HitTarget) -> std::cmp::Ordering {
    let ay = a.bbox.map(|b| rounded_i64(b.y)).unwrap_or(i64::MAX);
    let by = b.bbox.map(|b| rounded_i64(b.y)).unwrap_or(i64::MAX);
    ay.cmp(&by)
        .then_with(|| {
            let ax = a.bbox.map(|b| rounded_i64(b.x)).unwrap_or(i64::MAX);
            let bx = b.bbox.map(|b| rounded_i64(b.x)).unwrap_or(i64::MAX);
            ax.cmp(&bx)
        })
        .then_with(|| a.role.cmp(b.role))
        .then_with(|| a.id.cmp(&b.id))
}

fn hit_source_rank(source: &str) -> u8 {
    match source {
        "accessibility" => 0,
        "page" => 1,
        "ocr" => 2,
        "vision" => 3,
        _ => 4,
    }
}

fn hit_target_matches_find_query(target: &HitTarget, query: &str) -> bool {
    normalized_contains_query(&normalize_match(&target.id), query)
        || normalized_contains_query(&normalize_match(target.role), query)
        || target
            .label
            .as_deref()
            .map(|label| normalized_contains_query(&normalize_match(label), query))
            .unwrap_or(false)
        || target
            .value
            .as_deref()
            .map(|value| normalized_contains_query(&normalize_match(value), query))
            .unwrap_or(false)
}

fn source_name(source: dunst_core::Source) -> &'static str {
    match source {
        dunst_core::Source::Accessibility => "accessibility",
        dunst_core::Source::Vision => "vision",
        dunst_core::Source::Ocr => "ocr",
    }
}

fn hash_optional_bbox<H: Hasher>(bbox: Option<Bbox>, hasher: &mut H) {
    bbox.is_some().hash(hasher);
    if let Some(bbox) = bbox {
        hash_bbox(bbox, hasher);
    }
}

fn hash_bbox<H: Hasher>(bbox: Bbox, hasher: &mut H) {
    rounded_i64(bbox.x).hash(hasher);
    rounded_i64(bbox.y).hash(hasher);
    rounded_i64(bbox.w).hash(hasher);
    rounded_i64(bbox.h).hash(hasher);
}

fn rounded_i64(value: f64) -> i64 {
    value.round() as i64
}

#[cfg(test)]
mod hit_target_tests {
    use super::*;

    #[test]
    fn raw_ocr_click_risk_floors_benign_text_to_the_executor_gate() {
        // A benign OCR label (e.g. a tab title) carries no destructive keyword,
        // so assess_text returns a no-approval risk. But the only way to click
        // an OCR hit is raw pointer input, which ocr_point_risk_at always
        // approval-gates. The advertised affordance risk must match that gate.
        let benign = RiskAssessment {
            level: RiskLevel::Low,
            requires_approval: false,
            reasons: vec!["benign text".into()],
        };
        let floored = raw_ocr_click_risk(benign);
        assert_eq!(floored.level, RiskLevel::High);
        assert!(
            floored.requires_approval,
            "OCR click affordance must advertise the same approval gate the executor enforces"
        );
        assert!(
            floored
                .reasons
                .iter()
                .any(|reason| reason.contains("raw OCR input")),
            "floored risk should explain the raw-input delivery: {:?}",
            floored.reasons
        );
        assert!(
            floored.reasons.iter().any(|reason| reason == "benign text"),
            "text-derived reasons must be preserved: {:?}",
            floored.reasons
        );
    }

    #[test]
    fn page_scroll_bbox_does_not_mask_ocr_targets() {
        let page = HitTarget {
            id: "page@scroll:down".into(),
            source: "page".into(),
            role: "group",
            label: None,
            value: None,
            bbox: Some(Bbox {
                x: 0.0,
                y: 0.0,
                w: 1_000.0,
                h: 800.0,
            }),
            safe_click: None,
            confidence: 0.65,
            action_modes: Vec::new(),
            risk: RiskAssessment::low(),
        };

        assert!(
            !bbox_duplicate_of_existing(
                Bbox {
                    x: 100.0,
                    y: 120.0,
                    w: 80.0,
                    h: 20.0,
                },
                &[page],
            ),
            "page pseudo-targets are scroll surfaces, not real semantic duplicates"
        );
    }

    #[test]
    fn ocr_form_label_maps_to_following_value_target() {
        let label = TextHit {
            text: "Titre de la réalisation O".into(),
            bbox: Bbox {
                x: 3141.0,
                y: 722.0,
                w: 138.0,
                h: 12.0,
            },
            confidence: 1.0,
        };
        let value = TextHit {
            text: "openai/gpt-5.5".into(),
            bbox: Bbox {
                x: 3150.0,
                y: 751.0,
                w: 98.0,
                h: 14.0,
            },
            confidence: 1.0,
        };

        let kind = ocr_form_label_kind(&label.text).expect("label detected");
        let candidates = [value.clone()];
        let found =
            nearest_ocr_field_value(&label, &candidates, kind).expect("following value detected");
        assert_eq!(found.text, value.text);
        assert!(ocr_form_field_x_aligned(label.bbox, value.bbox));
    }
}
