use super::*;

pub(super) fn perform_ax_action(
    element: &AxElement,
    action: &str,
) -> std::result::Result<(), ActionFailure> {
    let action = CFString::new(action);
    // SAFETY: `element` is a valid AXUIElementRef, and `action` is a valid
    // CFString for the duration of the AX call; the AXError is checked.
    let err = unsafe { AXUIElementPerformAction(element.as_ptr(), action.as_concrete_TypeRef()) };
    ax_action_result(err, "perform AX action")
}

pub(super) fn scroll_element(
    element: &AxElement,
    argument: Option<&str>,
) -> std::result::Result<(), ActionFailure> {
    let (direction, pages) = parse_scroll_argument(argument);
    let scrollbar = find_vertical_scrollbar(element).ok_or_else(|| {
        ActionFailure::Execution("element and ancestors expose no AXVerticalScrollBar".into())
    })?;

    let min = attr_number(&scrollbar, kAXMinValueAttribute).unwrap_or(0.0);
    let max = attr_number(&scrollbar, kAXMaxValueAttribute).unwrap_or(1.0);
    if max <= min {
        return Err(ActionFailure::Execution(format!(
            "invalid AX scrollbar range: min={min} max={max}"
        )));
    }

    let current = attr_number(&scrollbar, kAXValueAttribute).unwrap_or(min);
    let increment = attr_number(&scrollbar, kAXValueIncrementAttribute)
        .filter(|v| *v > 0.0)
        .unwrap_or((max - min) * 0.85);
    let delta = increment * pages.clamp(1, 20) as f64;
    let next = match direction {
        "up" => current - delta,
        "top" => min,
        "bottom" => max,
        _ => current + delta,
    }
    .clamp(min, max);

    set_number_attr(&scrollbar, kAXValueAttribute, next)
}

pub(super) fn parse_scroll_argument(argument: Option<&str>) -> (&str, usize) {
    let Some(argument) = argument else {
        return ("down", 1);
    };
    let mut parts = argument.splitn(2, ':');
    let direction = match parts.next().unwrap_or("down") {
        "up" => "up",
        "top" => "top",
        "bottom" => "bottom",
        _ => "down",
    };
    let pages = parts
        .next()
        .and_then(|p| p.parse::<usize>().ok())
        .unwrap_or(1);
    (direction, pages)
}

pub(super) fn find_vertical_scrollbar(element: &AxElement) -> Option<AxElement> {
    if attr_string(element, kAXRoleAttribute).as_deref() == Some("AXScrollBar") {
        return retain_ax_element(element);
    }
    if let Some(scrollbar) = attr_ax_element(element, kAXVerticalScrollBarAttribute) {
        return Some(scrollbar);
    }
    let mut current = attr_ax_element(element, kAXParentAttribute);
    for _ in 0..16 {
        let element = current?;
        if attr_string(&element, kAXRoleAttribute).as_deref() == Some("AXScrollBar") {
            return Some(element);
        }
        if let Some(scrollbar) = attr_ax_element(&element, kAXVerticalScrollBarAttribute) {
            return Some(scrollbar);
        }
        current = attr_ax_element(&element, kAXParentAttribute);
    }
    None
}

pub(super) fn retain_ax_element(element: &AxElement) -> Option<AxElement> {
    Some(element.retain_clone())
}

pub(super) fn set_bool_attr(
    element: &AxElement,
    attr: &str,
    value: bool,
) -> std::result::Result<(), ActionFailure> {
    ax_action_result(
        set_bool_attr_raw(element, attr, value),
        "set AX bool attribute",
    )
}

pub(super) fn set_number_attr(
    element: &AxElement,
    attr: &str,
    value: f64,
) -> std::result::Result<(), ActionFailure> {
    ax_action_result(
        set_number_attr_raw(element, attr, value),
        "set AX number attribute",
    )
}

pub(super) fn set_bool_attr_raw(element: &AxElement, attr: &str, value: bool) -> AXError {
    let attr = CFString::new(attr);
    let value = CFBoolean::from(value);
    // SAFETY: `element`, `attr`, and `value` are valid CF/AX objects for
    // the duration of the call; AXError is returned to the caller.
    unsafe {
        AXUIElementSetAttributeValue(
            element.as_ptr(),
            attr.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        )
    }
}

pub(super) fn set_number_attr_raw(element: &AxElement, attr: &str, value: f64) -> AXError {
    let attr = CFString::new(attr);
    let value = CFNumber::from(value);
    // SAFETY: `element`, `attr`, and `value` are valid CF/AX objects for
    // the duration of the call; AXError is returned to the caller.
    unsafe {
        AXUIElementSetAttributeValue(
            element.as_ptr(),
            attr.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        )
    }
}

pub(super) fn set_cgpoint_attr(
    element: &AxElement,
    attr: &str,
    value: CGPoint,
) -> std::result::Result<(), ActionFailure> {
    ax_action_result(
        set_axvalue_attr_raw(
            element,
            attr,
            kAXValueTypeCGPoint,
            (&value as *const CGPoint).cast(),
        ),
        "set AX point attribute",
    )
}

pub(super) fn set_cgsize_attr(
    element: &AxElement,
    attr: &str,
    value: CGSize,
) -> std::result::Result<(), ActionFailure> {
    ax_action_result(
        set_axvalue_attr_raw(
            element,
            attr,
            kAXValueTypeCGSize,
            (&value as *const CGSize).cast(),
        ),
        "set AX size attribute",
    )
}

pub(super) fn set_axvalue_attr_raw(
    element: &AxElement,
    attr: &str,
    value_type: u32,
    value: *const c_void,
) -> AXError {
    let attr = CFString::new(attr);
    // SAFETY: `value` points to a stack CGPoint/CGSize that lives through
    // AXValueCreate; the returned AXValueRef is null-checked and released.
    let ax_value = unsafe { accessibility_sys::AXValueCreate(value_type, value) };
    if ax_value.is_null() {
        return kAXErrorIllegalArgument;
    }
    // SAFETY: `element`, `attr`, and `ax_value` are valid CF/AX objects for
    // the duration of the call; the AXValue create-rule retain is balanced.
    unsafe {
        let err = AXUIElementSetAttributeValue(
            element.as_ptr(),
            attr.as_concrete_TypeRef(),
            ax_value.cast(),
        );
        CFRelease(ax_value.cast());
        err
    }
}

pub(super) fn set_string_attr_raw(element: &AxElement, attr: &str, value: &str) -> AXError {
    let attr = CFString::new(attr);
    let value = CFString::new(value);
    // SAFETY: `element`, `attr`, and `value` are valid CF/AX objects for
    // the duration of the call; AXError is returned to the caller.
    unsafe {
        AXUIElementSetAttributeValue(
            element.as_ptr(),
            attr.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        )
    }
}

pub(super) fn ax_action_result(
    err: AXError,
    operation: &'static str,
) -> std::result::Result<(), ActionFailure> {
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(ActionFailure::Ax { operation, err })
    }
}
