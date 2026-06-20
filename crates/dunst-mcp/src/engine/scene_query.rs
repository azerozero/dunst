use super::*;

// --- WP-J / J2: latent (non-actionable) node geometry -----------------------

/// The window's on-screen rect, read from the `Window` node's bbox (the scene
/// graph's [`WindowRef`] carries no geometry). `None` when no window node has a
/// bbox — then [`node_on_screen`]'s off-window test is skipped. Memoised by
/// [`Engine::refresh`] into `cached_window_rect` (audit #9).
pub(super) fn compute_window_rect(g: &SceneGraph) -> Option<Bbox> {
    g.nodes
        .values()
        .find(|n| n.role == Role::Window)
        .and_then(|n| n.bbox)
}

/// Id of the menubar **root** — the `MenuBar`-role node in `roots` (its
/// `AXMenuBarItem` children share that role but have a parent, so iterating
/// `roots` disambiguates). Its direct children are the top-level menu openers
/// exempted from the latent filter by [`is_top_level_menu`]. Memoised by
/// [`Engine::refresh`] into `cached_menubar_root` (audit #9).
pub(super) fn compute_menubar_root(g: &SceneGraph) -> Option<String> {
    g.roots
        .iter()
        .find(|id| g.get(id).map(|n| n.role == Role::MenuBar).unwrap_or(false))
        .cloned()
}

/// Two axis-aligned boxes overlap (shared positive area).
pub(super) fn bbox_intersects(a: Bbox, b: Bbox) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

pub(super) fn point_in_bbox((x, y): (f64, f64), b: Bbox) -> bool {
    x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h
}

pub(super) fn option_selected_state(node: &SceneNode) -> Option<bool> {
    if matches!(node.role, Role::Radio | Role::Checkbox) && node.focused {
        return Some(true);
    }
    let raw = node
        .value
        .as_deref()
        .or(node.label.as_deref())
        .or(node.help.as_deref())?;
    let value = normalize_match(raw);
    if matches!(
        value.as_str(),
        "1" | "true" | "yes" | "on" | "selected" | "checked" | "selectionne" | "coche"
    ) {
        return Some(true);
    }
    if value.contains("not selected")
        || value.contains("not checked")
        || value.contains("non selectionne")
        || matches!(value.as_str(), "0" | "false" | "no" | "off" | "unchecked")
    {
        return Some(false);
    }
    None
}

/// WP-J/J2 — whether a node has a real on-screen footprint. A node is **latent**
/// (the negation) when it has no bbox, a zero/negative-area bbox, or a bbox that
/// lies entirely outside the window rect — exactly the shape of collapsed-menu
/// `AXMenuItem`s, which sit at `(0,0)`/off-window until their parent opens. This
/// is read-only geometry over `bbox` + the window rect: the scene/affordance
/// graphs are never mutated, so `find_element` and click-by-id still reach these
/// nodes; only the *listings* filter them.
pub(super) fn node_on_screen(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    let Some(b) = node.bbox else { return false };
    if b.w <= 0.0 || b.h <= 0.0 {
        return false;
    }
    match window_rect {
        Some(w) => bbox_intersects(b, w),
        None => true,
    }
}

/// WP-J follow-up — a node is a **top-level menu opener** when it sits directly
/// under the menubar root (Fichier, Édition, Format, …). These are legitimately
/// actionable (click / open_menu opens the menu) even with a null/off-window
/// bbox, so they are exempt from the latent filter. The rule is *structural*
/// (parent == menubar root id): deep collapsed submenu items — whose parent is a
/// closed `Menu`, not the menubar root — are NOT exempt and stay filtered.
pub(super) fn is_top_level_menu(node: &SceneNode, menubar_root: Option<&str>) -> bool {
    matches!(
        (node.parent.as_deref(), menubar_root),
        (Some(parent), Some(root)) if parent == root
    )
}

/// Visible in actionable listings: a real on-screen footprint OR a top-level
/// menu opener (see [`is_top_level_menu`]). This is the predicate the affordance
/// listings filter on (geometry, no `enabled` requirement).
pub(super) fn node_visible_or_menu(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    node_on_screen(node, window_rect) || is_top_level_menu(node, menubar_root)
}

pub(super) fn read_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    if is_top_level_menu(node, menubar_root)
        || matches!(
            node.role,
            Role::Window | Role::Toolbar | Role::MenuBar | Role::Menu | Role::MenuItem
        )
    {
        return true;
    }
    is_unlabeled_window_chrome_button(node, window_rect)
        || browser_chrome_node(graph, node, window_rect)
        || web_app_chrome_node(graph, node, window_rect)
}

pub(super) fn page_state_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    read_chrome_node(graph, node, window_rect, menubar_root)
}

pub(super) fn browser_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
) -> bool {
    if !is_browser_app_name(&graph.window.app_name) {
        return false;
    }
    if node_in_browser_tab_strip(graph, node, window_rect) {
        return true;
    }
    let Some(window) = window_rect else {
        return false;
    };
    let Some(b) = node.bbox else { return false };
    bbox_intersects(b, window)
        && b.y <= window.y + 104.0
        && matches!(
            node.role,
            Role::Button
                | Role::MenuButton
                | Role::TextField
                | Role::TextArea
                | Role::StaticText
                | Role::Radio
                | Role::Toolbar
        )
}

pub(super) fn web_app_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
) -> bool {
    if !is_browser_app_name(&graph.window.app_name) {
        return false;
    }
    let Some(window) = window_rect else {
        return false;
    };
    let Some(b) = node.bbox else { return false };
    if !bbox_intersects(b, window) {
        return false;
    }
    let Some(raw) = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.help.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    let text = normalize_match(raw);

    if likely_url(raw).is_some() && (b.y <= window.y + 220.0 || b.x <= window.x + window.w * 0.32) {
        return true;
    }
    if matches!(
        text.as_str(),
        "open intercom messenger"
            | "help center"
            | "copy"
            | "copier"
            | "compte"
            | "account"
            | "nouveautes"
            | "notifications"
    ) {
        return true;
    }

    let left_rail = b.x <= window.x + window.w * 0.28;
    let top_nav = b.y <= window.y + 180.0;
    (left_rail || top_nav)
        && matches!(
            text.as_str(),
            "accueil" | "home" | "connect" | "profil" | "profile" | "parametres" | "settings"
        )
}

pub(super) fn node_in_browser_tab_strip(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
) -> bool {
    if looks_like_browser_tab(node, window_rect) {
        return true;
    }
    let mut current = node.parent.as_deref();
    for _ in 0..4 {
        let Some(parent_id) = current else {
            return false;
        };
        let Some(parent) = graph.get(parent_id) else {
            return false;
        };
        if looks_like_browser_tab(parent, window_rect) {
            return true;
        }
        current = parent.parent.as_deref();
    }
    false
}

pub(super) fn is_browser_app_name(app_name: &str) -> bool {
    let app = normalize_match(app_name);
    [
        "firefox",
        "google chrome",
        "chromium",
        "safari",
        "zen",
        "arc",
        "brave",
        "microsoft edge",
        "edge",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

pub(super) fn page_state_key_element_candidate(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    let has_text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.help.as_deref())
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if node.bbox.is_some_and(|b| b.w < 4.0 || b.h < 4.0) {
        return false;
    }
    if !has_text && matches!(node.role, Role::Unknown | Role::Group | Role::Image) {
        return false;
    }
    if !has_text
        && node.bbox.is_some_and(|b| {
            window_rect.is_some_and(|window| {
                let node_area = b.w.max(0.0) * b.h.max(0.0);
                let window_area = window.w.max(0.0) * window.h.max(0.0);
                window_area > 0.0 && node_area >= window_area * 0.50
            })
        })
    {
        return false;
    }
    if is_top_level_menu(node, menubar_root)
        || matches!(
            node.role,
            Role::Window | Role::Toolbar | Role::MenuBar | Role::Menu | Role::MenuItem
        )
    {
        return false;
    }
    !is_unlabeled_window_chrome_button(node, window_rect)
}

pub(super) fn page_state_repetitive_destructive_keys(
    graph: &SceneGraph,
    affordances: &AffordanceGraph,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> BTreeSet<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in graph.nodes.values() {
        if !node_visible_or_menu(node, window_rect, menubar_root) {
            continue;
        }
        if let Some(key) = repetitive_destructive_key(node, affordances) {
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(key, count)| (count >= 5).then_some(key))
        .collect()
}

pub(super) fn page_state_suppressed_repetitive_destructive(
    node: &SceneNode,
    suppressed: &BTreeSet<String>,
) -> bool {
    suppressed.contains(&repetitive_destructive_key_for_text(node).unwrap_or_default())
}

pub(super) fn repetitive_destructive_key(
    node: &SceneNode,
    affordances: &AffordanceGraph,
) -> Option<String> {
    if !matches!(node.role, Role::Button | Role::MenuButton | Role::Group) {
        return None;
    }
    let affordance = affordances.affordances.get(&node.id)?;
    if !affordance.actions.iter().any(|action| {
        matches!(
            action,
            SemanticAction::Click | SemanticAction::Pick | SemanticAction::Toggle
        )
    }) {
        return None;
    }
    repetitive_destructive_key_for_text(node)
}

pub(super) fn repetitive_destructive_key_for_text(node: &SceneNode) -> Option<String> {
    let text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.ax_identifier.as_deref())?;
    let normalized = normalize_match(text);
    let key = match normalized.as_str() {
        "x" | "×" | "remove" | "delete" | "supprimer" | "retirer" => normalized,
        _ if normalized.starts_with("remove ") => "remove".to_string(),
        _ if normalized.starts_with("delete ") => "delete".to_string(),
        _ if normalized.starts_with("supprimer ") => "supprimer".to_string(),
        _ if normalized.starts_with("retirer ") => "retirer".to_string(),
        _ => return None,
    };
    Some(key)
}

pub(super) fn is_unlabeled_window_chrome_button(
    node: &SceneNode,
    window_rect: Option<Bbox>,
) -> bool {
    if !matches!(node.role, Role::Button | Role::MenuButton) {
        return false;
    }
    let has_text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if has_text {
        return false;
    }
    let Some(b) = node.bbox else { return false };
    if b.w > 24.0 || b.h > 24.0 {
        return false;
    }
    match window_rect {
        Some(w) => b.x <= w.x + 96.0 && b.y <= w.y + 48.0,
        None => false,
    }
}

/// J1 actionability: visible (on-screen or a top-level menu opener) **and**
/// enabled (what `actionable_only` keeps and `summary.n_actionable` counts).
pub(super) fn node_actionable(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    node.enabled && node_visible_or_menu(node, window_rect, menubar_root)
}

/// Ranking for search results: exact label/value/id matches first, then prefix
/// and containment matches. Within the same textual quality, page-visible
/// enabled targets outrank visible disabled/read-only nodes, then latent noise.
/// The final tie-breakers keep output deterministic without changing the graph.
pub(super) fn find_rank(
    node: &SceneNode,
    query: &str,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
    associated_control: bool,
) -> (u8, u8, u8, &'static str, String) {
    let tier = if node_actionable(node, window_rect, menubar_root) {
        0
    } else if node_visible_or_menu(node, window_rect, menubar_root) {
        1
    } else if node.bbox.is_some() {
        2
    } else {
        3
    };
    (
        find_match_priority(node, query, associated_control),
        tier,
        find_role_priority(node.role),
        node.role.as_str(),
        node.id.clone(),
    )
}

pub(super) fn find_match_priority(node: &SceneNode, query: &str, associated_control: bool) -> u8 {
    if associated_control {
        return 0;
    }
    let id = normalize_match(&node.id);
    let mut texts = Vec::new();
    if let Some(label) = node.label.as_deref() {
        texts.push(normalize_match(label));
    }
    if let Some(value) = node.value.as_deref() {
        texts.push(normalize_match(value));
    }
    texts.push(id);

    if texts.iter().any(|text| text == query) {
        return 0;
    }
    if texts.iter().any(|text| text.starts_with(query)) {
        return 1;
    }
    if texts.iter().any(|text| text.contains(query)) {
        return 2;
    }
    3
}

pub(super) fn find_role_priority(role: Role) -> u8 {
    match role {
        Role::TextField | Role::TextArea => 0,
        Role::Checkbox | Role::Radio | Role::MenuButton => 1,
        Role::Button | Role::Row | Role::Cell => 2,
        Role::List | Role::Table | Role::Outline => 3,
        Role::Group | Role::Unknown => 4,
        Role::Window | Role::Toolbar | Role::Menu | Role::MenuBar | Role::MenuItem => 5,
        Role::Image => 6,
        Role::StaticText => 7,
    }
}

pub(super) fn associated_control_for_label<'a>(
    label: &SceneNode,
    graph: &'a SceneGraph,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> Option<&'a SceneNode> {
    if label.role != Role::StaticText {
        return None;
    }
    graph
        .nodes
        .values()
        .filter(|candidate| {
            candidate.id != label.id
                && is_label_associable_control(candidate.role)
                && node_visible_or_menu(candidate, window_rect, menubar_root)
        })
        .filter_map(|candidate| {
            associated_control_score(label, candidate).map(|score| (score, candidate))
        })
        .min_by_key(|(score, candidate)| (*score, candidate.id.clone()))
        .map(|(_, candidate)| candidate)
}

pub(super) fn is_label_associable_control(role: Role) -> bool {
    matches!(
        role,
        Role::TextField | Role::TextArea | Role::Checkbox | Role::Radio | Role::MenuButton
    )
}

pub(super) fn associated_control_score(
    label: &SceneNode,
    candidate: &SceneNode,
) -> Option<(u8, u8, i64, i64)> {
    let label_box = label.bbox?;
    let candidate_box = candidate.bbox?;
    let same_parent = label.parent.as_deref() == candidate.parent.as_deref();
    let vertical_gap = candidate_box.y - (label_box.y + label_box.h);
    let horizontal_delta = (candidate_box.x - label_box.x).abs();
    let overlaps_x = intervals_overlap(
        label_box.x - 24.0,
        label_box.x + label_box.w + 24.0,
        candidate_box.x,
        candidate_box.x + candidate_box.w,
    );
    let overlaps_y = intervals_overlap(
        label_box.y - 8.0,
        label_box.y + label_box.h + 8.0,
        candidate_box.y,
        candidate_box.y + candidate_box.h,
    );
    let below_label = (-4.0..=96.0).contains(&vertical_gap) && overlaps_x;
    let right_of_label = overlaps_y
        && candidate_box.x >= label_box.x + label_box.w - 8.0
        && horizontal_delta <= 360.0;

    if !below_label && !right_of_label {
        return None;
    }

    Some((
        u8::from(!same_parent),
        if below_label { 0 } else { 1 },
        vertical_gap.max(0.0).round() as i64,
        horizontal_delta.round() as i64,
    ))
}

pub(super) fn intervals_overlap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    a_start <= b_end && b_start <= a_end
}

/// WP-J/J1 compact projection of one node: keep only the agent-facing fields and
/// drop the heavy/derivable AX detail (`ax_role`, `help`, `ax_actions`,
/// `ax_identifier`, `last_seen_ms`), collapsing `children` to a count.
pub(super) fn compact_node(n: &SceneNode) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("id".into(), json!(n.id));
    o.insert("role".into(), json!(n.role.as_str()));
    if let Some(l) = &n.label {
        o.insert("label".into(), json!(l));
    }
    if let Some(v) = &n.value {
        o.insert("value".into(), json!(v));
    }
    o.insert(
        "bbox".into(),
        serde_json::to_value(n.bbox).unwrap_or(Value::Null),
    );
    o.insert("enabled".into(), json!(n.enabled));
    o.insert("focused".into(), json!(n.focused));
    if let Some(p) = &n.parent {
        o.insert("parent".into(), json!(p));
    }
    o.insert("n_children".into(), json!(n.children.len()));
    Value::Object(o)
}

pub(super) struct OptionCandidate {
    pub(super) matched_id: String,
    pub(super) action_id: String,
    pub(super) action: SemanticAction,
}
