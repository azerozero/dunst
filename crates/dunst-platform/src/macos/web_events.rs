use super::*;

extern "C" {
    /// Public CoreGraphics: set a CGEvent's screen location.
    pub(super) fn CGEventSetLocation(event: *const c_void, location: CGPoint);
}

/// Build a CGEvent through the **NSEvent bridge** so it carries the
/// `windowNumber` routing Chromium's user-activation gate latches onto (the
/// plain CGEvent path is dropped). Returns a +1-owned CGEvent.
pub(super) fn cg_event_via_nsevent(
    etype: CGEventType,
    click_count: isize,
    window_id: u32,
) -> Option<CGEvent> {
    let ns_type = match etype {
        CGEventType::LeftMouseDown => NSEventType::LeftMouseDown,
        CGEventType::LeftMouseUp => NSEventType::LeftMouseUp,
        CGEventType::RightMouseDown => NSEventType::RightMouseDown,
        CGEventType::RightMouseUp => NSEventType::RightMouseUp,
        CGEventType::MouseMoved => NSEventType::MouseMoved,
        _ => return None,
    };
    let ns = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        ns_type,
        NSPoint { x: 0.0, y: 0.0 },
        NSEventModifierFlags(0),
        0.0,
        window_id as isize,
        None,
        0,
        click_count,
        1.0,
    )?;
    // `-[NSEvent CGEvent]` returns a `CGEventRef` (`^{__CGEvent=}`); objc2
    // checks the return encoding, so name it via an opaque struct rather than
    // a bare `void*`.
    struct CgEventOpaque {
        _p: [u8; 0],
    }
    // SAFETY: the encoding mirrors CoreGraphics' opaque `__CGEvent` struct.
    unsafe impl objc2::RefEncode for CgEventOpaque {
        const ENCODING_REF: objc2::Encoding =
            objc2::Encoding::Pointer(&objc2::Encoding::Struct("__CGEvent", &[]));
    }
    // SAFETY: `-[NSEvent CGEvent]` returns a CGEventRef owned by `ns`.
    let raw: *mut CgEventOpaque = unsafe { msg_send![&*ns, CGEvent] };
    if raw.is_null() {
        return None;
    }
    // SAFETY: retain the borrowed CGEventRef so the owned wrapper outlives `ns`.
    unsafe { core_foundation_sys::base::CFRetain(raw.cast()) };
    // SAFETY: `raw` is now a +1-owned CGEventRef handed to CGEvent's owner.
    Some(unsafe { CGEvent::from_ptr(raw.cast()) })
}

/// Full background web click via SkyLight (cua-driver's `clickViaAuthSignedPost`
/// recipe): focus-without-raise, then move → off-screen primer click → real
/// click, each CGEvent stamped with the fields Chromium's synthetic-event gate
/// requires (button/subtype/clickState, window-under-pointer = window_id,
/// target pid, window-local coord), posted via `SLEventPostToPid`. Reaches a
/// backgrounded / occluded window's web content WITHOUT moving the cursor.
/// Returns false if SkyLight is unavailable (caller falls back to the cursor).
pub fn click_web_background(
    pid: i32,
    window_id: u32,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
    button: u8,
) -> bool {
    if window_id == 0 || !skylight::mouse_post_available() {
        return false;
    }
    if user_idle_block_message("background click").is_some() {
        return false;
    }
    focus_without_raise(window_id);
    thread::sleep(Duration::from_millis(50));

    let screen = CGPoint::new(sx, sy);
    let local = CGPoint::new(sx - ox, sy - oy);
    let off = CGPoint::new(-1.0, -1.0);
    let make = |etype: CGEventType, point: CGPoint, win_local: CGPoint| -> Option<CGEvent> {
        // Bridge through NSEvent so the event carries the windowNumber routing
        // Chromium's gate requires; then re-stamp the real screen location.
        let event = cg_event_via_nsevent(etype, 1, window_id)?;
        // SAFETY: `event` is a live CGEventRef; `point` is a valid CGPoint.
        unsafe { CGEventSetLocation(event.as_ptr().cast(), point) };
        event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, button as i64);
        event.set_integer_value_field(EventField::MOUSE_EVENT_SUB_TYPE, 3);
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 1);
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER,
            window_id as i64,
        );
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
            window_id as i64,
        );
        event.set_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID, pid as i64);
        skylight::set_window_location(event.as_ptr().cast(), win_local.x, win_local.y);
        Some(event)
    };
    let post = |ev: &CGEvent| skylight::post_event_to_pid(pid, ev.as_ptr().cast());

    let Some(m) = make(CGEventType::MouseMoved, screen, local) else {
        return false;
    };
    if !post(&m) {
        return false;
    }
    thread::sleep(Duration::from_millis(15));

    let Some(d) = make(CGEventType::LeftMouseDown, off, off) else {
        return false;
    };
    if !post(&d) {
        return false;
    }
    thread::sleep(Duration::from_millis(1));

    let Some(u) = make(CGEventType::LeftMouseUp, off, off) else {
        return false;
    };
    if !post(&u) {
        return false;
    }
    thread::sleep(Duration::from_millis(100));

    let (down_t, up_t) = if button == 1 {
        (CGEventType::RightMouseDown, CGEventType::RightMouseUp)
    } else {
        (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp)
    };
    let Some(d) = make(down_t, screen, local) else {
        return false;
    };
    if !post(&d) {
        return false;
    }
    thread::sleep(Duration::from_millis(1));

    let Some(u) = make(up_t, screen, local) else {
        return false;
    };
    post(&u)
}

/// Background web hover via SkyLight. Unlike `hover_at_point_impl`, this
/// never warps the real cursor; it is the default hover path for target-window
/// probes. Callers that need a real OS cursor hover must opt into the cursor
/// borrow path explicitly.
pub fn hover_web_background(
    pid: i32,
    window_id: u32,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
) -> Result<()> {
    hover_web_background_impl(pid, window_id, sx, sy, ox, oy).map_err(ActionFailure::into)
}

pub(super) fn hover_web_background_impl(
    pid: i32,
    window_id: u32,
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
) -> std::result::Result<(), ActionFailure> {
    if window_id == 0 || !skylight::mouse_post_available() {
        return Err(ActionFailure::Execution(
            "background hover requires the SkyLight backend".into(),
        ));
    }
    ensure_user_idle_action("background hover")?;
    focus_without_raise(window_id);
    thread::sleep(Duration::from_millis(40));

    let screen = CGPoint::new(sx, sy);
    let local = CGPoint::new(sx - ox, sy - oy);
    let event = cg_event_via_nsevent(CGEventType::MouseMoved, 0, window_id)
        .ok_or_else(|| ActionFailure::Execution("create background hover NSEvent".into()))?;
    // SAFETY: `event` is a live CGEventRef; `screen` is a valid CGPoint.
    unsafe { CGEventSetLocation(event.as_ptr().cast(), screen) };
    event.set_integer_value_field(EventField::MOUSE_EVENT_SUB_TYPE, 3);
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER,
        window_id as i64,
    );
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
        window_id as i64,
    );
    event.set_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID, pid as i64);
    skylight::set_window_location(event.as_ptr().cast(), local.x, local.y);
    if skylight::post_event_to_pid(pid, event.as_ptr().cast()) {
        Ok(())
    } else {
        Err(ActionFailure::Execution(
            "post background hover via SkyLight".into(),
        ))
    }
}

/// Type `text` into the focused element of a backgrounded window's (web)
/// content via SkyLight: focus-without-raise, then per character a key
/// down/up whose CGEvent carries the typed unicode + the
/// `SLSEventAuthenticationMessage` Chromium requires, posted via
/// `SLEventPostToPid`. No cursor, no foreground. Fails if SkyLight is absent
/// or if any expected key event cannot be created and posted.
pub fn type_text_background(pid: i32, window_id: u32, text: &str) -> Result<()> {
    type_text_background_impl(pid, window_id, text).map_err(ActionFailure::into)
}

pub(super) fn type_text_background_impl(
    pid: i32,
    window_id: u32,
    text: &str,
) -> std::result::Result<(), ActionFailure> {
    if !skylight::mouse_post_available() {
        return Err(ActionFailure::Execution(
            "background typing requires the SkyLight backend; process-wide keyboard fallback suppressed".into(),
        ));
    }
    if window_id == 0 {
        return Err(ActionFailure::Execution(
            "background typing requires a target window id".into(),
        ));
    }
    ensure_user_idle_action("background type")?;
    focus_without_raise(window_id);
    thread::sleep(Duration::from_millis(50));
    for_text_input_atoms(text, |atom| {
        match atom {
            TextInputAtom::Char(ch) => post_background_unicode_char(pid, ch)?,
            TextInputAtom::Return => post_background_keycode_pair(pid, RETURN_KEYCODE)?,
        }
        thread::sleep(Duration::from_millis(8));
        Ok(())
    })?;
    Ok(())
}

pub(super) fn post_background_unicode_char(
    pid: i32,
    ch: char,
) -> std::result::Result<(), ActionFailure> {
    let s = ch.to_string();
    for down in [true, false] {
        let source = event_source("skylight key CGEventSource")?;
        let event = CGEvent::new_keyboard_event(source, 0, down).map_err(|err| {
            ActionFailure::Execution(format!("create background key CGEvent: {err:?}"))
        })?;
        event.set_string(&s);
        post_background_key_event(pid, &event)?;
    }
    Ok(())
}

pub(super) fn post_background_keycode_pair(
    pid: i32,
    keycode: CGKeyCode,
) -> std::result::Result<(), ActionFailure> {
    for down in [true, false] {
        let source = event_source("skylight key CGEventSource")?;
        let event = CGEvent::new_keyboard_event(source, keycode, down).map_err(|err| {
            ActionFailure::Execution(format!("create background key CGEvent: {err:?}"))
        })?;
        post_background_key_event(pid, &event)?;
    }
    Ok(())
}

pub(super) fn post_background_key_event(
    pid: i32,
    event: &CGEvent,
) -> std::result::Result<(), ActionFailure> {
    event.set_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID, pid as i64);
    skylight::attach_auth_message(event.as_ptr().cast(), pid);
    if skylight::post_event_to_pid(pid, event.as_ptr().cast()) {
        Ok(())
    } else {
        Err(ActionFailure::Execution(
            "post background key CGEvent via SkyLight".into(),
        ))
    }
}

/// Post a single named keycode (down+up) to a backgrounded window's (web)
/// content via the SkyLight auth-signed keyboard path — used for scrolling
/// (Page Down/Up, Home, End) and other non-character keys. Fails if SkyLight
/// is absent or if either key event cannot be created and posted.
pub fn key_web_background(pid: i32, window_id: u32, keycode: u16, flags: u64) -> Result<()> {
    if !skylight::mouse_post_available() {
        return Err(VisualOpsError::Execution(
            "key_web_background requires the SkyLight backend".into(),
        ));
    }
    ensure_user_idle("background key")?;
    if window_id != 0 {
        focus_without_raise(window_id);
        thread::sleep(Duration::from_millis(40));
    }
    let mods = CGEventFlags::from_bits_truncate(flags);
    for down in [true, false] {
        let source = event_source("skylight key CGEventSource")?;
        let event = CGEvent::new_keyboard_event(source, keycode, down).map_err(|err| {
            VisualOpsError::Execution(format!("create background key CGEvent: {err:?}"))
        })?;
        if !mods.is_empty() {
            event.set_flags(mods);
        }
        event.set_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID, pid as i64);
        skylight::attach_auth_message(event.as_ptr().cast(), pid);
        if !skylight::post_event_to_pid(pid, event.as_ptr().cast()) {
            return Err(VisualOpsError::Execution(
                "post background key CGEvent via SkyLight".into(),
            ));
        }
    }
    Ok(())
}

pub fn press_key(pid: i32, window_id: u32, key: &str) -> Result<()> {
    let keycode = named_keycode(key).map_err(VisualOpsError::from)?;
    key_web_background(pid, window_id, keycode, 0)
}

pub(super) fn named_keycode(key: &str) -> std::result::Result<CGKeyCode, ActionFailure> {
    match key.trim().to_ascii_lowercase().as_str() {
        "return" | "enter" => Ok(KeyCode::RETURN),
        "tab" => Ok(KeyCode::TAB),
        "escape" | "esc" => Ok(KeyCode::ESCAPE),
        "space" | "spacebar" => Ok(KeyCode::SPACE),
        "delete" | "backspace" => Ok(KeyCode::DELETE),
        "up" | "arrowup" | "up_arrow" => Ok(KeyCode::UP_ARROW),
        "down" | "arrowdown" | "down_arrow" => Ok(KeyCode::DOWN_ARROW),
        "left" | "arrowleft" | "left_arrow" => Ok(KeyCode::LEFT_ARROW),
        "right" | "arrowright" | "right_arrow" => Ok(KeyCode::RIGHT_ARROW),
        "home" => Ok(0x73),
        "end" => Ok(0x77),
        "pageup" | "page_up" => Ok(0x74),
        "pagedown" | "page_down" => Ok(0x79),
        other => Err(ActionFailure::Execution(format!(
            "unsupported key {other:?}; expected return|enter, tab, escape, space, delete, up/down/left/right, pageup/pagedown, home/end"
        ))),
    }
}

/// Union of every active display's bounds (the full virtual desktop). Cursor
/// warps and CGEvents use the GLOBAL coordinate space, so clamping to the main
/// display alone would pin a point on a secondary monitor to the main display's
/// edge — which is exactly where a window on an external screen lives.
pub(super) fn all_displays_bounds() -> CGRect {
    let main = CGDisplay::main().bounds();
    let (mut min_x, mut min_y) = (main.origin.x, main.origin.y);
    let (mut max_x, mut max_y) = (
        main.origin.x + main.size.width,
        main.origin.y + main.size.height,
    );
    if let Ok(ids) = CGDisplay::active_displays() {
        for id in ids {
            let b = CGDisplay::new(id).bounds();
            min_x = min_x.min(b.origin.x);
            min_y = min_y.min(b.origin.y);
            max_x = max_x.max(b.origin.x + b.size.width);
            max_y = max_y.max(b.origin.y + b.size.height);
        }
    }
    CGRect::new(
        &CGPoint::new(min_x, min_y),
        &CGSize::new(max_x - min_x, max_y - min_y),
    )
}

pub(super) fn clamp_point_to_bounds(point: CGPoint, bounds: CGRect) -> CGPoint {
    let min_x = bounds.origin.x;
    let min_y = bounds.origin.y;
    let max_x = if bounds.size.width > 1.0 {
        bounds.origin.x + bounds.size.width - 1.0
    } else {
        bounds.origin.x
    };
    let max_y = if bounds.size.height > 1.0 {
        bounds.origin.y + bounds.size.height - 1.0
    } else {
        bounds.origin.y
    };

    CGPoint::new(point.x.clamp(min_x, max_x), point.y.clamp(min_y, max_y))
}

pub(super) fn post_mouse(
    source: CGEventSource,
    pid: i32,
    event_type: CGEventType,
    point: CGPoint,
) -> std::result::Result<(), ActionFailure> {
    let event = CGEvent::new_mouse_event(source, event_type, point, CGMouseButton::Left)
        .map_err(|err| ActionFailure::Execution(format!("create mouse CGEvent: {err:?}")))?;
    event.post_to_pid(pid);
    Ok(())
}

pub(super) fn current_cursor_position(
    source: &CGEventSource,
) -> std::result::Result<CGPoint, ActionFailure> {
    let event = CGEvent::new(source.clone())
        .map_err(|err| ActionFailure::Execution(format!("read cursor CGEvent: {err:?}")))?;
    Ok(event.location())
}

pub(super) fn restore_cursor_position(point: CGPoint) -> std::result::Result<(), ActionFailure> {
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|err| ActionFailure::Execution(format!("restore cursor position: {err:?}")))
}

pub(super) fn event_source(
    operation: &'static str,
) -> std::result::Result<CGEventSource, ActionFailure> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|err| ActionFailure::Execution(format!("{operation}: {err:?}")))
}
