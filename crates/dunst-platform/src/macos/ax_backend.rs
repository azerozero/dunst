use super::*;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub(super) fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub(super) fn CGEventSourceSecondsSinceLastEventType(
        state_id: CGEventSourceStateID,
        event_type: CGEventType,
    ) -> f64;
}

pub(super) fn set_ax_timeout(element: AXUIElementRef) {
    if !element.is_null() {
        // SAFETY: `element` is a valid AXUIElementRef supplied by AX APIs
        // or retained by AxElement; the timeout value is finite.
        unsafe {
            accessibility_sys::AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECS);
        }
    }
}

pub(super) fn max_nodes() -> usize {
    env_usize("DUNST_AX_MAX_NODES", DEFAULT_MAX_NODES).max(100)
}

pub(super) fn max_depth() -> usize {
    env_usize("DUNST_AX_MAX_DEPTH", DEFAULT_MAX_DEPTH).max(4)
}

pub(super) fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

pub(super) fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

pub(super) fn user_idle_guard_ms() -> u64 {
    env_u64("DUNST_MCP_USER_IDLE_GUARD_MS", DEFAULT_USER_IDLE_GUARD_MS)
}

pub(super) fn last_input_age_ms() -> Option<u64> {
    // SAFETY: u32::MAX is the documented kCGAnyInputEventType pseudo-value.
    // CGEventType is repr(u32), so the bit pattern matches CoreGraphics.
    let any_input = unsafe { mem::transmute::<u32, CGEventType>(u32::MAX) };
    // SAFETY: this CoreGraphics query has no pointer arguments or retained
    // ownership; both enum values use documented constants.
    let seconds = unsafe {
        CGEventSourceSecondsSinceLastEventType(
            CGEventSourceStateID::CombinedSessionState,
            any_input,
        )
    };
    if seconds.is_finite() && seconds >= 0.0 {
        Some((seconds * 1_000.0).round() as u64)
    } else {
        None
    }
}

pub(super) fn user_idle_block_message(operation: &str) -> Option<String> {
    let guard_ms = user_idle_guard_ms();
    if guard_ms == 0 {
        return None;
    }
    let age_ms = last_input_age_ms()?;
    if age_ms >= guard_ms {
        return None;
    }
    Some(format!(
        "user-active guard blocked {operation}: last keyboard/mouse input was {age_ms} ms ago (< {guard_ms} ms). Retry after the operator is idle, or set DUNST_MCP_USER_IDLE_GUARD_MS=0 to disable this guard."
    ))
}

pub(super) fn ensure_user_idle(operation: &str) -> Result<()> {
    if let Some(message) = user_idle_block_message(operation) {
        return Err(DunstError::Execution(message));
    }
    Ok(())
}

pub(super) fn ensure_user_idle_action(operation: &str) -> std::result::Result<(), ActionFailure> {
    if let Some(message) = user_idle_block_message(operation) {
        return Err(ActionFailure::Execution(message));
    }
    Ok(())
}

pub fn capture(target: &Target) -> Result<Vec<RawAxNode>> {
    ensure_trusted()?;
    clear_cache();
    let target_key = TargetKey::from_target(target);
    let app = app_element(target.pid)?;
    let window = resolve_window(&app, target.window_id)?;
    let walk_attrs = WalkAttributes::new();
    let mut state = WalkState::default();
    let mut roots = vec![walk_element(
        &window,
        &target_key,
        0,
        &mut state,
        &walk_attrs,
    )?];
    if let Some(menu_bar) = attr_ax_element(&app, kAXMenuBarAttribute) {
        roots.push(walk_element(
            &menu_bar,
            &target_key,
            0,
            &mut state,
            &walk_attrs,
        )?);
    }
    if state.capped {
        eprintln!(
            "dunst-platform: AX tree capped at {} nodes / depth {}",
            max_nodes(),
            max_depth()
        );
    }
    Ok(roots)
}

pub fn window_ref(target: &Target) -> Result<WindowRef> {
    ensure_trusted()?;
    let app = app_element(target.pid)?;
    let window = resolve_window(&app, target.window_id)?;
    Ok(WindowRef {
        pid: target.pid,
        window_id: ax_window_id(&window).unwrap_or(target.window_id),
        app_name: attr_string(&app, kAXTitleAttribute).unwrap_or_default(),
        title: attr_string(&window, kAXTitleAttribute).unwrap_or_default(),
    })
}

pub fn element_at_point(pid: i32, x: f64, y: f64) -> Result<RawAxNode> {
    ensure_trusted()?;
    let app = app_element(pid)?;
    let mut raw: AXUIElementRef = ptr::null_mut();
    // SAFETY: `app` is a valid AX application element; `raw` is a checked
    // out-parameter that follows the copy rule on success.
    let err =
        unsafe { AXUIElementCopyElementAtPosition(app.as_ptr(), x as f32, y as f32, &mut raw) };
    if err != kAXErrorSuccess || raw.is_null() {
        return Err(DunstError::Perception(format!(
            "AX hit-test failed at ({x:.1},{y:.1}): {} ({err})",
            error_string(err)
        )));
    }
    Ok(shallow_raw_node(&AxElement::from_owned(raw)))
}

pub fn perform(
    target: &Target,
    node: &SceneNode,
    action: SemanticAction,
    argument: Option<&str>,
) -> Result<()> {
    ensure_trusted()?;
    let started = Instant::now();
    let key = CacheKey::from_scene(target, node);
    let mut path = "direct";
    let result = perform_with_cache(target, node, action, argument, &key, &mut path);
    if env::var_os("VO_ACTION_TIMING").is_some() {
        eprintln!(
            "performed {action:?} via {path} in {:.3} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
    result
}

pub(super) fn perform_with_cache(
    target: &Target,
    node: &SceneNode,
    action: SemanticAction,
    argument: Option<&str>,
    key: &CacheKey,
    path: &mut &'static str,
) -> Result<()> {
    if matches!(action, SemanticAction::Hover | SemanticAction::Drag) {
        return perform_on_element(None, target, node, action, argument)
            .map_err(ActionFailure::into);
    }

    let use_cache =
        env::var_os("VO_ACTION_DISABLE_CACHE").is_none() && !collision_suffix_id(&node.id);
    if use_cache {
        if let Some(element) = cached_element(key) {
            if !cached_element_matches_target(&element, target) {
                remove_cached_element(key);
            } else {
                *path = "cached";
                match perform_on_element(Some(&element), target, node, action, argument) {
                    Ok(()) => return Ok(()),
                    Err(err) if err.is_stale() => {
                        remove_cached_element(key);
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
    }

    *path = "fallback";
    let app = app_element(target.pid)?;
    let window = resolve_window(&app, target.window_id)?;
    let element = find_element(window, node)?.ok_or_else(|| {
        DunstError::ElementNotFound(format!(
            "role={:?} label={:?} identifier={:?}",
            node.ax_role, node.label, node.ax_identifier
        ))
    })?;
    perform_on_element(Some(&element), target, node, action, argument).map_err(ActionFailure::into)
}

pub(super) fn perform_on_element(
    element: Option<&AxElement>,
    target: &Target,
    node: &SceneNode,
    action: SemanticAction,
    argument: Option<&str>,
) -> std::result::Result<(), ActionFailure> {
    match action {
        // AX actions/set-attribute are non-intrusive: no global cursor movement
        // and no foreground activation. `Raise` below is the intentional exception.
        SemanticAction::Click | SemanticAction::Pick => {
            let element = require_ax_element(element)?;
            click_element_action(element, target, node)
        }
        SemanticAction::OpenMenu => {
            let element = require_ax_element(element)?;
            perform_ax_action(element, kAXShowMenuAction)
        }
        SemanticAction::Raise => {
            let element = require_ax_element(element)?;
            perform_ax_action(element, kAXRaiseAction)
        }
        SemanticAction::Focus => {
            set_bool_attr(require_ax_element(element)?, kAXFocusedAttribute, true)
        }
        SemanticAction::Type => {
            let text = argument.ok_or_else(|| {
                ActionFailure::Execution("type action requires an argument".into())
            })?;
            let element = require_ax_element(element)?;
            type_text(element, target, text)
        }
        SemanticAction::Hover => hover(target.pid, node),
        SemanticAction::Drag => drag(target.pid, node, argument),
        SemanticAction::Scroll => {
            let element = require_ax_element(element)?;
            scroll_element(element, argument)
        }
        other => Err(ActionFailure::Execution(format!(
            "semantic action {other:?} is not supported by macOS AX backend"
        ))),
    }
}

pub(super) fn require_ax_element(
    element: Option<&AxElement>,
) -> std::result::Result<&AxElement, ActionFailure> {
    element.ok_or_else(|| ActionFailure::Execution("action requires a resolved AX element".into()))
}

pub(super) fn click_element_action(
    element: &AxElement,
    target: &Target,
    node: &SceneNode,
) -> std::result::Result<(), ActionFailure> {
    if !matches!(node.role, Role::TextField | Role::TextArea) {
        return perform_ax_action(element, kAXPressAction);
    }

    match perform_ax_action(element, kAXPressAction) {
        Ok(()) => {
            thread::sleep(Duration::from_millis(50));
            if attr_bool(element, kAXFocusedAttribute).unwrap_or(false) {
                return Ok(());
            }
        }
        Err(err) if err.is_stale() => return Err(err),
        Err(_) => {}
    }

    let _ = set_bool_attr(element, kAXFocusedAttribute, true);
    thread::sleep(Duration::from_millis(40));
    if attr_bool(element, kAXFocusedAttribute).unwrap_or(false) {
        return Ok(());
    }

    let bbox = node
        .bbox
        .ok_or_else(|| ActionFailure::Execution("text field click fallback needs a bbox".into()))?;
    let (origin_x, origin_y) = target_window_origin(target)?;
    let x = bbox.x + bbox.w / 2.0;
    let y = bbox.y + bbox.h / 2.0;
    if !click_web_background(target.pid, target.window_id, x, y, origin_x, origin_y, 0) {
        return Err(ActionFailure::Execution(
            "element-bound text-field click fallback could not post a background web click".into(),
        ));
    }
    thread::sleep(Duration::from_millis(120));
    if attr_bool(element, kAXFocusedAttribute).unwrap_or(false) {
        Ok(())
    } else {
        Err(ActionFailure::Execution(
            "element-bound text-field click fallback posted but AXFocused stayed false".into(),
        ))
    }
}

pub(super) fn target_window_origin(
    target: &Target,
) -> std::result::Result<(f64, f64), ActionFailure> {
    let app = app_element(target.pid).map_err(|err| ActionFailure::Execution(err.to_string()))?;
    let window = resolve_window(&app, target.window_id)
        .map_err(|err| ActionFailure::Execution(err.to_string()))?;
    let bbox = frame(&window).ok_or_else(|| {
        ActionFailure::Execution("target window frame unavailable for click fallback".into())
    })?;
    Ok((bbox.x, bbox.y))
}

pub fn accessibility_trusted() -> bool {
    // SAFETY: AXIsProcessTrusted takes no pointers and returns a Boolean
    // process trust status.
    unsafe { AXIsProcessTrusted() }
}

pub fn set_window_frame(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<()> {
    ensure_trusted()?;
    let app = app_element(pid)?;
    let window = resolve_window(&app, window_id)?;
    let mut size = attr_cgsize(&window, kAXSizeAttribute).unwrap_or_else(|| {
        CGSize::new(
            width.unwrap_or(0.0).max(1.0),
            height.unwrap_or(0.0).max(1.0),
        )
    });
    if let Some(w) = width {
        size.width = w.max(1.0);
    }
    if let Some(h) = height {
        size.height = h.max(1.0);
    }
    set_cgsize_attr(&window, kAXSizeAttribute, size).map_err(DunstError::from)?;
    set_cgpoint_attr(&window, kAXPositionAttribute, CGPoint::new(x, y)).map_err(DunstError::from)
}

pub(super) fn ensure_trusted() -> Result<()> {
    if accessibility_trusted() {
        Ok(())
    } else {
        Err(DunstError::Perception(
            "accessibility not granted for this process".into(),
        ))
    }
}

pub(super) fn app_element(pid: i32) -> Result<AxElement> {
    // SAFETY: AXUIElementCreateApplication is a create-rule AX API. The
    // returned pointer is null-checked and then owned by AxElement.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        Err(DunstError::Perception(format!(
            "AXUIElementCreateApplication returned null for pid {pid}"
        )))
    } else {
        Ok(AxElement::from_owned(app))
    }
}

pub(super) fn resolve_window(app: &AxElement, requested_window_id: u32) -> Result<AxElement> {
    let mut windows = attr_array(app, kAXWindowsAttribute)
        .map(|windows| ax_elements(&windows))
        .unwrap_or_default();

    if let Some(index) = windows
        .iter()
        .position(|window| ax_window_id(window) == Some(requested_window_id))
    {
        return Ok(windows.remove(index));
    }

    if requested_window_id != 0 {
        if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
            if ax_window_id(&main_window) == Some(requested_window_id) {
                return Ok(main_window);
            }
        }
        return Err(DunstError::Perception(format!(
            "window id {requested_window_id} not found; target window may be closed"
        )));
    }

    if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
        return Ok(main_window);
    }

    if !windows.is_empty() {
        eprintln!(
            "dunst-platform: window id {requested_window_id} not found; using first AX window"
        );
        return Ok(windows.remove(0));
    }

    Err(DunstError::Perception(format!(
        "no AX window found for requested window id {requested_window_id}"
    )))
}

pub(super) fn ax_window_id(element: &AxElement) -> Option<u32> {
    let mut window_id = 0;
    // SAFETY: `element.as_ptr()` is a valid AXUIElementRef and
    // `window_id` is a valid out-parameter for the duration of the call.
    let err = unsafe { _AXUIElementGetWindow(element.as_ptr(), &mut window_id) };
    (err == kAXErrorSuccess).then_some(window_id)
}

pub(super) fn round_bbox(bbox: Bbox) -> Option<(i64, i64, i64, i64)> {
    if bbox.x.is_finite() && bbox.y.is_finite() && bbox.w.is_finite() && bbox.h.is_finite() {
        // Saturating `as i64` conversion is intentional here: these values
        // come from AX/CG FFI and are used only as coarse cache keys.
        Some((
            bbox.x.round() as i64,
            bbox.y.round() as i64,
            bbox.w.round() as i64,
            bbox.h.round() as i64,
        ))
    } else {
        None
    }
}

pub(super) enum ActionFailure {
    Ax {
        operation: &'static str,
        err: AXError,
    },
    Execution(String),
}

impl ActionFailure {
    pub(super) fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::Ax { err, .. }
                if *err == kAXErrorInvalidUIElement || *err == kAXErrorCannotComplete
        )
    }
}

impl From<ActionFailure> for DunstError {
    fn from(value: ActionFailure) -> Self {
        match value {
            ActionFailure::Ax { operation, err } => {
                DunstError::Execution(format!("{operation} failed: {} ({err})", error_string(err)))
            }
            ActionFailure::Execution(message) => DunstError::Execution(message),
        }
    }
}
