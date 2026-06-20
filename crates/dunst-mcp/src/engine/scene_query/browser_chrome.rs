use super::*;

pub(in crate::engine) fn read_chrome_node(
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

pub(in crate::engine) fn page_state_chrome_node(
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
    let Some(bbox) = node.bbox else { return false };
    bbox_intersects(bbox, window)
        && bbox.y <= window.y + 104.0
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
    let Some(bbox) = node.bbox else { return false };
    if !bbox_intersects(bbox, window) {
        return false;
    }
    let Some(raw) = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.help.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return false;
    };
    let text = normalize_match(raw);

    if likely_url(raw).is_some()
        && (bbox.y <= window.y + 220.0 || bbox.x <= window.x + window.w * 0.32)
    {
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

    let left_rail = bbox.x <= window.x + window.w * 0.28;
    let top_nav = bbox.y <= window.y + 180.0;
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
