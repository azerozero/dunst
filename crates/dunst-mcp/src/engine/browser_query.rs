use super::*;

pub(super) fn looks_like_browser_tab(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    let Some(b) = node.bbox else { return false };
    if b.w <= 0.0 || b.h <= 0.0 || b.h > 90.0 {
        return false;
    }
    let Some(window) = window_rect else {
        return true;
    };
    // Browser tab strips sit in the top browser chrome. This filters out page
    // radio controls such as Reddit sort/filter tabs named after communities.
    bbox_intersects(b, window) && b.y >= window.y - 2.0 && b.y <= window.y + 96.0
}

pub(super) fn browser_tab_title(graph: &SceneGraph, node: &SceneNode) -> String {
    let mut candidates = Vec::new();
    if let Some(label) = node.label.as_deref() {
        candidates.push(label);
    }
    if let Some(value) = node.value.as_deref() {
        candidates.push(value);
    }
    for child_id in &node.children {
        if let Some(child) = graph.get(child_id) {
            if let Some(label) = child.label.as_deref() {
                candidates.push(label);
            }
            if let Some(value) = child.value.as_deref() {
                candidates.push(value);
            }
        }
    }

    candidates
        .into_iter()
        .map(str::trim)
        .find(|s| {
            !s.is_empty()
                && !s.eq_ignore_ascii_case("fermer")
                && !normalize_match(s).starts_with("fermer l")
                && !normalize_match(s).starts_with("close tab")
        })
        .unwrap_or("")
        .to_string()
}

pub(super) fn browser_tab_selected(graph: &SceneGraph, node: &SceneNode, title: &str) -> bool {
    let window_title = normalize_match(&graph.window.title);
    let tab_title = normalize_match(title);
    node.focused
        || node
            .value
            .as_deref()
            .map(normalize_match)
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "selected" | "selectionne"))
        || (!window_title.is_empty()
            && !tab_title.is_empty()
            && (window_title == tab_title
                || window_title.starts_with(&tab_title)
                || tab_title.starts_with(&window_title)))
}
