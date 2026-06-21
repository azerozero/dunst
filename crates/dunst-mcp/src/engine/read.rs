use super::*;

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
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        self.affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .filter(|a| {
                include_latent
                    || g.get(&a.id)
                        .map(|n| node_visible_or_menu(n, window_rect, menubar))
                        .unwrap_or(false)
            })
            .map(|a| a.id.clone())
            .collect()
    }

    /// WP-J/J2: the affordance graph as JSON, latent nodes omitted unless
    /// `include_latent`. Shape matches [`AffordanceGraph`] (`{ "affordances": … }`).
    pub fn affordances_view(&self, include_latent: bool) -> Value {
        let ag = self.affordance_graph();
        if include_latent {
            return serde_json::to_value(ag).unwrap_or(Value::Null);
        }
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        let mut map = serde_json::Map::new();
        for (id, aff) in &ag.affordances {
            if g.get(id)
                .map(|n| node_visible_or_menu(n, window_rect, menubar))
                .unwrap_or(false)
            {
                map.insert(id.clone(), serde_json::to_value(aff).unwrap_or(Value::Null));
            }
        }
        json!({ "affordances": Value::Object(map) })
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

        PageState {
            target: TargetState {
                pid: g.window.pid,
                window_id: g.window.window_id,
                app_name: g.window.app_name.clone(),
            },
            title: g.window.title.clone(),
            url,
            browser_tab,
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
