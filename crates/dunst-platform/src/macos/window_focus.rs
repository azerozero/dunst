use super::*;

pub(crate) fn window_focus_state(pid: i32, window_id: u32) -> Result<crate::WindowFocusState> {
    ensure_trusted()?;
    let app = app_element(pid)?;
    let window = resolve_window(&app, window_id)?;
    let role = attr_string(&window, kAXRoleAttribute).unwrap_or_default();
    let subrole = attr_string(&window, "AXSubrole").unwrap_or_default();
    Ok(crate::WindowFocusState {
        is_dialog: role == "AXSheet"
            || matches!(subrole.as_str(), "AXDialog" | "AXSystemDialog" | "AXSheet"),
        focused_window_id: attr_ax_element(&app, "AXFocusedWindow")
            .and_then(|window| ax_window_id(&window)),
    })
}
