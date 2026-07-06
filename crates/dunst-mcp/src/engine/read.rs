use super::*;
use std::hash::{Hash, Hasher};

mod epoch;
mod hit_target;
mod perception;

use epoch::*;
use hit_target::*;

const BBOX_FINGERPRINT_QUANTUM: f64 = 2.0;
const VISIBLE_FRACTION_FINGERPRINT_BUCKETS: f64 = 20.0;

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
            tab.url.hash(&mut hasher);
            tab.selected.hash(&mut hasher);
            hash_optional_bbox(tab.bbox, &mut hasher);
        }
        target_visibility.status.hash(&mut hasher);
        visible_fraction_bucket(target_visibility.visible_fraction).hash(&mut hasher);
        for window in &target_visibility.covered_by {
            window.window_id.hash(&mut hasher);
            hash_bbox(window.bounds, &mut hasher);
        }

        let transient_ids = transient_epoch_node_ids(g, affordances);
        let menubar_root = self.cached_menubar_root.as_deref();
        let mut nodes: Vec<(Vec<usize>, &SceneNode)> = g
            .nodes
            .values()
            .filter(|node| !transient_ids.contains(&node.id))
            .filter(|node| !node_in_menu_bar(g, node, menubar_root))
            .map(|node| (epoch_filtered_path(g, node, &transient_ids), node))
            .collect();
        nodes.sort_by(|(left_path, left), (right_path, right)| {
            left_path
                .cmp(right_path)
                .then_with(|| left.id.cmp(&right.id))
        });
        for (path, node) in nodes {
            let affordance = affordances.affordances.get(&node.id);
            let control_state = node_epoch_control_state(node);
            path.hash(&mut hasher);
            if node_epoch_hashes_id(affordance, control_state) {
                node.id.hash(&mut hasher);
            }
            node.role.as_str().hash(&mut hasher);
            control_state.hash(&mut hasher);
            node.enabled.hash(&mut hasher);
            hash_optional_bbox(node.bbox, &mut hasher);
            if let Some(affordance) = affordance {
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

        tabs.sort_by(|a, b| bbox_reading_order(a.bbox, &a.id, b.bbox, &b.id));
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
                && self.page_state_is_key_element(
                    node,
                    chrome,
                    window_rect,
                    menubar,
                    &suppressed_repetitive_destructive,
                )
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

    /// Whether `node` qualifies as a [`page_state`](Self::page_state) *key
    /// element*: an enabled, non-chrome, non-repetitive-destructive candidate that
    /// actually exposes an affordance. Split out of the collection loop so each
    /// gate reads as one named concern.
    fn page_state_is_key_element(
        &self,
        node: &SceneNode,
        chrome: bool,
        window_rect: Option<Bbox>,
        menubar: Option<&str>,
        suppressed_repetitive_destructive: &BTreeSet<String>,
    ) -> bool {
        !chrome
            && !page_state_suppressed_repetitive_destructive(
                node,
                suppressed_repetitive_destructive,
            )
            && page_state_key_element_candidate(node, window_rect, menubar)
            && node.enabled
            && self
                .affordance_graph()
                .affordances
                .get(&node.id)
                .is_some_and(|a| !a.actions.is_empty())
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
                .then_with(|| bbox_reading_order(a.bbox, &a.id, b.bbox, &b.id))
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

#[cfg(test)]
mod tests;
