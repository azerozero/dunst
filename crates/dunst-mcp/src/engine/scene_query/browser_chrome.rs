use super::*;

pub(in crate::engine) fn read_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    if is_top_level_menu(node, menubar_root)
        || is_menu_descendant(graph, node, menubar_root)
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

/// Whether `node` sits **inside** the app menu bar. Its own role can be a plain
/// `TextField`/`Button` (so the role match above misses it), yet an ancestor is a
/// `Menu`/`MenuBar`/`MenuItem` or the memoised menubar root. Such nodes are app
/// chrome, never page content: the canonical case is `_SC_SEARCH_FIELD`, the
/// macOS Help-menu search box present in every app, which otherwise surfaces as
/// the only `scope:"page"` type target on an AX-sparse web feed and gets typed
/// into by mistake. Walks a bounded ancestry so a deep page node stays cheap.
pub(super) fn is_menu_descendant(
    graph: &SceneGraph,
    node: &SceneNode,
    menubar_root: Option<&str>,
) -> bool {
    let mut current = node.parent.as_deref();
    for _ in 0..16 {
        let Some(parent_id) = current else {
            return false;
        };
        if menubar_root == Some(parent_id) {
            return true;
        }
        let Some(parent) = graph.get(parent_id) else {
            return false;
        };
        if matches!(parent.role, Role::MenuBar | Role::Menu | Role::MenuItem) {
            return true;
        }
        current = parent.parent.as_deref();
    }
    false
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

pub(in crate::engine) fn is_browser_app_name(app_name: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn node(id: &str, role: Role, parent: Option<&str>, bbox: Option<Bbox>) -> SceneNode {
        SceneNode {
            id: id.into(),
            role,
            ax_role: String::new(),
            label: None,
            help: None,
            value: None,
            bbox,
            confidence: 1.0,
            source: dunst_core::Source::Accessibility,
            enabled: true,
            focused: false,
            ax_actions: Vec::new(),
            ax_identifier: None,
            cmd_char: None,
            cmd_modifiers: None,
            cmd_virtual_key: None,
            last_seen_ms: 0,
            path: Vec::new(),
            parent: parent.map(str::to_string),
            children: Vec::new(),
        }
    }

    fn firefox_graph(nodes: Vec<SceneNode>) -> SceneGraph {
        let mut map = BTreeMap::new();
        for n in nodes {
            map.insert(n.id.clone(), n);
        }
        SceneGraph {
            nodes: map,
            roots: vec!["menubar".into(), "web_area".into()],
            captured_at_ms: 0,
            window: dunst_core::WindowRef {
                app_name: "Firefox".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn menu_bar_search_field_is_chrome_not_page() {
        let window = Bbox {
            x: 2560.0,
            y: 356.0,
            w: 1728.0,
            h: 1084.0,
        };
        // Menu bar → Help "search" menu item → the macOS `_SC_SEARCH_FIELD` text
        // field, whose bbox sits off-window like a collapsed menu control.
        let menubar = node("menubar", Role::MenuBar, None, None);
        let search_item = node(
            "mi_searchfieldaction",
            Role::MenuItem,
            Some("menubar"),
            None,
        );
        let help_field = node(
            "field_sc_search_field",
            Role::TextField,
            Some("mi_searchfieldaction"),
            Some(Bbox {
                x: -1.0,
                y: 1415.0,
                w: 342.0,
                h: 26.0,
            }),
        );
        // A genuine page text field: on-window, mid-page, not under the menu bar.
        let web_area = node("web_area", Role::Group, None, None);
        let page_field = node(
            "field_page_search",
            Role::TextField,
            Some("web_area"),
            Some(Bbox {
                x: 3000.0,
                y: 600.0,
                w: 300.0,
                h: 30.0,
            }),
        );

        let graph = firefox_graph(vec![
            menubar,
            search_item,
            help_field.clone(),
            web_area,
            page_field.clone(),
        ]);

        assert!(
            is_menu_descendant(&graph, &help_field, Some("menubar")),
            "the Help-menu search field must be recognised as a menu descendant"
        );
        assert!(
            read_chrome_node(&graph, &help_field, Some(window), Some("menubar")),
            "the menu-bar search field must classify as chrome, not page"
        );
        assert!(
            !read_chrome_node(&graph, &page_field, Some(window), Some("menubar")),
            "a real on-page text field must stay page content"
        );
    }
}
