#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolRoute {
    Read,
    Element,
    Batch,
    Raw,
    WindowApp,
    Screenshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RegisteredTool {
    pub name: &'static str,
    pub route: ToolRoute,
}

pub(super) const TOOL_REGISTRY: &[RegisteredTool] = &[
    tool("version", ToolRoute::Read),
    tool("platform_capabilities", ToolRoute::Read),
    tool("refresh", ToolRoute::Read),
    tool("get_scene_graph", ToolRoute::Read),
    tool("page_state", ToolRoute::Read),
    tool("text_snapshot", ToolRoute::Read),
    tool("wait_for_text_stable", ToolRoute::Read),
    tool("list_browser_tabs", ToolRoute::Read),
    tool("list_displays", ToolRoute::Read),
    tool("window_view", ToolRoute::Read),
    tool("desktop_view", ToolRoute::Read),
    tool("target_visibility", ToolRoute::Read),
    tool("visual_change_probe", ToolRoute::Read),
    tool("analyze_region_ax", ToolRoute::Read),
    tool("get_affordances", ToolRoute::Read),
    tool("get_hit_targets", ToolRoute::Read),
    tool("find_element", ToolRoute::Read),
    tool("wait_for_element", ToolRoute::Read),
    tool("read_text", ToolRoute::Read),
    tool("read_text_detailed", ToolRoute::Read),
    tool("read_shapes", ToolRoute::Read),
    tool("find_ocr_text", ToolRoute::Read),
    tool("detect_modal", ToolRoute::Read),
    tool("extract_ocr_cards", ToolRoute::Read),
    tool("query_affordances", ToolRoute::Read),
    tool("enumerate_choices", ToolRoute::Read),
    tool("read_at", ToolRoute::Read),
    tool("read_series", ToolRoute::Read),
    tool("scan_chart", ToolRoute::Read),
    tool("diff_since", ToolRoute::Read),
    tool("export_trace", ToolRoute::Read),
    tool("click_element", ToolRoute::Element),
    tool("raise_element", ToolRoute::Element),
    tool("pick_option", ToolRoute::Element),
    tool("type_into", ToolRoute::Element),
    tool("hover_probe", ToolRoute::Element),
    tool("drag_element", ToolRoute::Element),
    tool("select_file", ToolRoute::Element),
    tool("approve", ToolRoute::Element),
    tool("verify_state", ToolRoute::Element),
    tool("apply_selections", ToolRoute::Batch),
    tool("click_at", ToolRoute::Raw),
    tool("click_near_text", ToolRoute::Raw),
    tool("dismiss_modal", ToolRoute::Raw),
    tool("reveal_hover_click", ToolRoute::Raw),
    tool("hover_at", ToolRoute::Raw),
    tool("focus_window", ToolRoute::Raw),
    tool("unstick_cursor", ToolRoute::Raw),
    tool("right_click_at", ToolRoute::Raw),
    tool("double_click_at", ToolRoute::Raw),
    tool("open_menu", ToolRoute::Raw),
    tool("press_key", ToolRoute::Raw),
    tool("type_keys", ToolRoute::Raw),
    tool("set_field_text", ToolRoute::Raw),
    tool("paste_text", ToolRoute::Raw),
    tool("scroll", ToolRoute::Raw),
    tool("scroll_at", ToolRoute::Raw),
    tool("zoom", ToolRoute::Raw),
    tool("hotkey", ToolRoute::Raw),
    tool("list_windows", ToolRoute::WindowApp),
    tool("move_window_to_display", ToolRoute::WindowApp),
    tool("move_app_to_display", ToolRoute::WindowApp),
    tool("arrange_windows", ToolRoute::WindowApp),
    tool("expose_target_window", ToolRoute::WindowApp),
    tool("list_apps", ToolRoute::WindowApp),
    tool("list_launchable_apps", ToolRoute::WindowApp),
    tool("app_info", ToolRoute::WindowApp),
    tool("attach", ToolRoute::WindowApp),
    tool("launch_app", ToolRoute::WindowApp),
    tool("open_url_and_attach_tab", ToolRoute::WindowApp),
    tool("navigate", ToolRoute::WindowApp),
    tool("close_app", ToolRoute::WindowApp),
    tool("screenshot", ToolRoute::Screenshot),
];

pub(super) fn tool_route(name: &str) -> Option<ToolRoute> {
    TOOL_REGISTRY
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.route)
}

const fn tool(name: &'static str, route: ToolRoute) -> RegisteredTool {
    RegisteredTool { name, route }
}
