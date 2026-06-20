use super::*;

pub(super) fn hover(pid: i32, node: &SceneNode) -> std::result::Result<(), ActionFailure> {
    ensure_user_idle_action("hover")?;
    let Some(bbox) = node.bbox else {
        return Ok(());
    };
    let source = event_source("create hover CGEventSource")?;
    let point = CGPoint::new(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
    let saved_cursor = current_cursor_position(&source)?;
    let result = (|| {
        let event =
            CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
                .map_err(|err| ActionFailure::Execution(format!("create hover CGEvent: {err:?}")))?;
        event.post_to_pid(pid);
        Ok(())
    })();
    restore_cursor_position(saved_cursor)?;
    result
}

pub(super) fn drag(
    pid: i32,
    node: &SceneNode,
    argument: Option<&str>,
) -> std::result::Result<(), ActionFailure> {
    ensure_user_idle_action("drag")?;
    let start_bbox = node
        .bbox
        .ok_or_else(|| ActionFailure::Execution("Drag requires a source bbox".into()))?;
    let display_bounds = all_displays_bounds();
    let end = clamp_point_to_bounds(parse_drop_point(argument)?, display_bounds);
    let start = clamp_point_to_bounds(
        CGPoint::new(
            start_bbox.x + start_bbox.w / 2.0,
            start_bbox.y + start_bbox.h / 2.0,
        ),
        display_bounds,
    );
    let source = event_source("create drag CGEventSource")?;
    let saved_cursor = current_cursor_position(&source)?;
    let mut mouse_down_posted = false;

    let result = (|| {
        post_mouse(source.clone(), pid, CGEventType::LeftMouseDown, start)?;
        mouse_down_posted = true;
        thread::sleep(DRAG_STEP_DELAY);
        for step in 1..=DRAG_STEPS {
            let t = step as f64 / DRAG_STEPS as f64;
            let point = CGPoint::new(
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
            );
            post_mouse(source.clone(), pid, CGEventType::LeftMouseDragged, point)?;
            thread::sleep(DRAG_STEP_DELAY);
        }
        post_mouse(source.clone(), pid, CGEventType::LeftMouseUp, end)?;
        mouse_down_posted = false;
        Ok(())
    })();
    if result.is_err() && mouse_down_posted {
        let _ = post_mouse(source.clone(), pid, CGEventType::LeftMouseUp, end);
    }
    restore_cursor_position(saved_cursor)?;
    result
}

pub(super) fn parse_drop_point(
    argument: Option<&str>,
) -> std::result::Result<CGPoint, ActionFailure> {
    let argument = argument
        .ok_or_else(|| ActionFailure::Execution("Drag requires an \"x,y\" argument".into()))?;
    let (x, y) = argument
        .split_once(',')
        .ok_or_else(|| ActionFailure::Execution("Drag requires an \"x,y\" argument".into()))?;
    let x = x
        .trim()
        .parse::<f64>()
        .map_err(|_| ActionFailure::Execution("Drag requires an \"x,y\" argument".into()))?;
    let y = y
        .trim()
        .parse::<f64>()
        .map_err(|_| ActionFailure::Execution("Drag requires an \"x,y\" argument".into()))?;
    Ok(CGPoint::new(x, y))
}

pub fn click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    click_at_point_impl(pid, x, y).map_err(ActionFailure::into)
}

pub(super) fn click_at_point_impl(
    pid: i32,
    x: f64,
    y: f64,
) -> std::result::Result<(), ActionFailure> {
    ensure_user_idle_action("click_at")?;
    let point = clamp_point_to_bounds(CGPoint::new(x, y), all_displays_bounds());
    let source = event_source("create click CGEventSource")?;
    let saved_cursor = current_cursor_position(&source)?;
    let mut mouse_down_posted = false;

    let result = (|| {
        post_mouse(source.clone(), pid, CGEventType::LeftMouseDown, point)?;
        mouse_down_posted = true;
        post_mouse(source.clone(), pid, CGEventType::LeftMouseUp, point)?;
        mouse_down_posted = false;
        Ok(())
    })();
    if result.is_err() && mouse_down_posted {
        let _ = post_mouse(source.clone(), pid, CGEventType::LeftMouseUp, point);
    }
    restore_cursor_position(saved_cursor)?;
    result
}

pub fn hover_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    hover_at_point_impl(pid, x, y).map_err(ActionFailure::into)
}

pub(super) fn hover_at_point_impl(
    _pid: i32,
    x: f64,
    y: f64,
) -> std::result::Result<(), ActionFailure> {
    ensure_user_idle_action("hover_at")?;
    // A web/canvas hover (a chart crosshair, value-at-cursor) reads the REAL
    // cursor position, so a background `post_to_pid` move never triggers it.
    // Warp the cursor to the point and post a GLOBAL (HID) MouseMoved so the
    // window under the cursor sees the hover. This DOES move the visible cursor
    // — unavoidable to reveal a value-at-cursor — but the window does NOT need
    // focus: macOS routes mouse-moved/hover to the window under the cursor
    // regardless of which app is frontmost.
    let point = clamp_point_to_bounds(CGPoint::new(x, y), all_displays_bounds());
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|err| ActionFailure::Execution(format!("warp cursor for hover: {err:?}")))?;
    let source = event_source("create hover CGEventSource")?;
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|err| ActionFailure::Execution(format!("create hover CGEvent: {err:?}")))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn cursor_borrow_to(x: f64, y: f64) -> Result<(f64, f64)> {
    cursor_borrow_to_impl(x, y).map_err(ActionFailure::into)
}

pub(super) fn cursor_borrow_to_impl(
    x: f64,
    y: f64,
) -> std::result::Result<(f64, f64), ActionFailure> {
    ensure_user_idle_action("read_at cursor borrow")?;
    let source = event_source("create borrow CGEventSource")?;
    let saved = current_cursor_position(&source)?;
    let point = clamp_point_to_bounds(CGPoint::new(x, y), all_displays_bounds());
    // NOTE: we deliberately do NOT decouple the hardware mouse. Decoupling
    // (CGAssociateMouseAndMouseCursorPosition(false)) stops the warped cursor's
    // move from routing to the window under it, so a web/canvas hover never
    // fires and the crosshair stays hidden. Keep the mouse coupled (the user
    // just shouldn't fight it during the brief borrow); we restore the cursor
    // afterwards.
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|err| ActionFailure::Execution(format!("warp for borrowed hover: {err:?}")))?;
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|err| ActionFailure::Execution(format!("create hover CGEvent: {err:?}")))?;
    event.post(CGEventTapLocation::HID);
    Ok((saved.x, saved.y))
}

pub fn cursor_restore(x: f64, y: f64) -> Result<()> {
    cursor_restore_impl(x, y).map_err(ActionFailure::into)
}

pub(super) fn cursor_restore_impl(x: f64, y: f64) -> std::result::Result<(), ActionFailure> {
    let warped = CGDisplay::warp_mouse_cursor_position(CGPoint::new(x, y))
        .map_err(|err| ActionFailure::Execution(format!("restore cursor: {err:?}")));
    // Always re-couple the hardware mouse, even if the warp failed, so we never
    // leave the user's mouse decoupled.
    let _ = CGDisplay::associate_mouse_and_mouse_cursor_position(true);
    warped
}

pub fn focus_without_raise(window_id: u32) -> bool {
    skylight::focus_without_raise(window_id)
}
