use super::*;

pub(super) fn is_stale_ax_error(err: AXError) -> bool {
    err == kAXErrorInvalidUIElement || err == kAXErrorCannotComplete
}

pub(super) fn attr_value(element: &AxElement, attr: &str) -> Option<CFType> {
    let attr = CFString::new(attr);
    let mut value: CFTypeRef = ptr::null();
    // SAFETY: `element` and `attr` are valid AX/CF objects; `value` is a
    // valid out-parameter and is null/error checked before wrapping.
    let err = unsafe {
        AXUIElementCopyAttributeValue(element.as_ptr(), attr.as_concrete_TypeRef(), &mut value)
    };
    if err == kAXErrorSuccess && !value.is_null() {
        // SAFETY: AXUIElementCopyAttributeValue returns a +1 CF object on
        // success, transferred to CFType with create-rule ownership.
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    } else {
        None
    }
}

pub(super) fn attr_string(element: &AxElement, attr: &str) -> Option<String> {
    let value = attr_value(element, attr)?;
    cf_string(value.as_CFTypeRef())
}

pub(super) fn attr_value_string(element: &AxElement, attr: &str) -> Option<String> {
    let value = attr_value(element, attr)?;
    cf_value_string(value.as_CFTypeRef())
}

pub(super) fn attr_label_string(element: &AxElement, attr: &str) -> Option<String> {
    attr_string(element, attr).filter(|s| !s.is_empty())
}

pub(super) fn attr_bool(element: &AxElement, attr: &str) -> Option<bool> {
    let value = attr_value(element, attr)?;
    cf_bool(value.as_CFTypeRef())
}

pub(super) fn attr_number(element: &AxElement, attr: &str) -> Option<f64> {
    let value = attr_value(element, attr)?;
    // SAFETY: `value` is a valid CF object owned by `attr_value`.
    let value_type = unsafe { CFGetTypeID(value.as_CFTypeRef()) };
    if value_type != CFNumber::type_id() {
        return None;
    }
    // SAFETY: the CFTypeID check above proves this object is a CFNumber.
    let number = unsafe { CFNumber::wrap_under_get_rule(value.as_CFTypeRef() as _) };
    number.to_f64()
}

pub(super) fn attr_array(element: &AxElement, attr: &str) -> Option<CFArray> {
    let value = attr_value(element, attr)?;
    cf_array(value.as_CFTypeRef())
}

pub(super) fn attr_ax_element(element: &AxElement, attr: &str) -> Option<AxElement> {
    let value = attr_value(element, attr)?;
    // SAFETY: `value` is a valid CF object owned by `attr_value`.
    let value_type = unsafe { CFGetTypeID(value.as_CFTypeRef()) };
    // SAFETY: AXUIElementGetTypeID takes no inputs and returns a stable CFTypeID.
    let ax_type = unsafe { accessibility_sys::AXUIElementGetTypeID() };
    if value_type == ax_type {
        let raw = value.as_CFTypeRef() as AXUIElementRef;
        mem::forget(value);
        Some(AxElement::from_owned(raw))
    } else {
        None
    }
}

pub(super) fn read_node_actions(element: &AxElement) -> Vec<String> {
    // PERF: this remains one AX round-trip per node; kAXActions can be folded
    // into the batch request if the FFI constant is introduced locally.
    action_names(element)
}

pub(super) fn action_names(element: &AxElement) -> Vec<String> {
    let mut actions: CFArrayRef = ptr::null();
    // SAFETY: `element` is a valid AXUIElementRef and `actions` is a valid
    // out-parameter; success/null are checked before wrapping.
    let err = unsafe { AXUIElementCopyActionNames(element.as_ptr(), &mut actions) };
    if err != kAXErrorSuccess || actions.is_null() {
        return Vec::new();
    }

    // SAFETY: AXUIElementCopyActionNames returns a +1 CFArray on success.
    let actions = unsafe { CFArray::wrap_under_create_rule(actions) };
    cf_strings(&actions)
        .into_iter()
        .map(|action| {
            action
                .strip_prefix("AX")
                .unwrap_or(action.as_str())
                .to_ascii_lowercase()
        })
        .collect()
}

pub(super) fn cf_strings(array: &CFArray) -> Vec<String> {
    let mut values = Vec::new();
    // SAFETY: `array` is a valid CFArray reference for this scope.
    let len = unsafe { CFArrayGetCount(array.as_concrete_TypeRef()) };
    for index in 0..len {
        // SAFETY: `index` is in 0..len, so CFArray returns a borrowed item
        // valid for as long as `array` is alive.
        let value = unsafe { CFArrayGetValueAtIndex(array.as_concrete_TypeRef(), index) };
        if value.is_null() {
            continue;
        }
        // SAFETY: the CFArray item is a borrowed CF object. wrap_under_get_rule
        // retains it for the temporary CFType wrapper.
        let value = unsafe { CFType::wrap_under_get_rule(value as CFTypeRef) };
        if let Some(string) = value.downcast::<CFString>() {
            values.push(string.to_string());
        }
    }
    values
}

pub(super) fn ax_elements(array: &CFArray) -> Vec<AxElement> {
    let mut elements = Vec::new();
    // SAFETY: `array` is a valid CFArray reference for this scope.
    let len = unsafe { CFArrayGetCount(array.as_concrete_TypeRef()) };
    // SAFETY: AXUIElementGetTypeID takes no inputs and returns a stable CFTypeID.
    let ax_type = unsafe { accessibility_sys::AXUIElementGetTypeID() };
    for index in 0..len {
        // SAFETY: `index` is in 0..len, so CFArray returns a borrowed item
        // valid for as long as `array` is alive.
        let value = unsafe { CFArrayGetValueAtIndex(array.as_concrete_TypeRef(), index) };
        if value.is_null() {
            continue;
        }
        let cf_ref = value as CFTypeRef;
        // SAFETY: `cf_ref` is a non-null CF object borrowed from `array`.
        if unsafe { CFGetTypeID(cf_ref) } == ax_type {
            // SAFETY: the CFArray item is borrowed; CFRetain creates the
            // +1 ownership required by AxElement::from_owned.
            unsafe {
                core_foundation_sys::base::CFRetain(cf_ref);
            }
            elements.push(AxElement::from_owned(cf_ref as AXUIElementRef));
        }
    }
    elements
}

pub(super) fn frame(element: &AxElement) -> Option<Bbox> {
    attr_cgrect(element, AX_FRAME_ATTRIBUTE).or_else(|| {
        let origin = attr_cgpoint(element, kAXPositionAttribute)?;
        let size = attr_cgsize(element, kAXSizeAttribute)?;
        Some(Bbox {
            x: origin.x,
            y: origin.y,
            w: size.width,
            h: size.height,
        })
    })
}

pub(super) fn frame_from_batch(values: &BatchValues) -> Option<Bbox> {
    cgrect_from_value(values.get(IDX_FRAME)?).or_else(|| {
        let origin = cgpoint_from_value(values.get(IDX_POSITION)?)?;
        let size = cgsize_from_value(values.get(IDX_SIZE)?)?;
        Some(Bbox {
            x: origin.x,
            y: origin.y,
            w: size.width,
            h: size.height,
        })
    })
}

pub(super) fn attr_cgrect(element: &AxElement, attr: &str) -> Option<Bbox> {
    let value = attr_value(element, attr)?;
    cgrect_from_value(value.as_CFTypeRef())
}

pub(super) fn attr_cgpoint(element: &AxElement, attr: &str) -> Option<CGPoint> {
    let value = attr_value(element, attr)?;
    cgpoint_from_value(value.as_CFTypeRef())
}

pub(super) fn attr_cgsize(element: &AxElement, attr: &str) -> Option<CGSize> {
    let value = attr_value(element, attr)?;
    cgsize_from_value(value.as_CFTypeRef())
}

pub(super) fn cgrect_from_value(value: CFTypeRef) -> Option<Bbox> {
    cgrect_from_ax_value(ax_value_ref(value)?)
}

pub(super) fn cgpoint_from_value(value: CFTypeRef) -> Option<CGPoint> {
    cgpoint_from_ax_value(ax_value_ref(value)?)
}

pub(super) fn cgsize_from_value(value: CFTypeRef) -> Option<CGSize> {
    cgsize_from_ax_value(ax_value_ref(value)?)
}

pub(super) fn cgrect_from_ax_value(value: AXValueRef) -> Option<Bbox> {
    let mut rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
    // SAFETY: `value` has already been type-checked as an AXValueRef; `rect`
    // is a correctly sized out-parameter for kAXValueTypeCGRect.
    let ok = unsafe {
        AXValueGetValue(
            value,
            kAXValueTypeCGRect,
            &mut rect as *mut CGRect as *mut c_void,
        )
    };
    ok.then_some(Bbox {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    })
}

pub(super) fn cgpoint_from_ax_value(value: AXValueRef) -> Option<CGPoint> {
    let mut point = CGPoint::new(0.0, 0.0);
    // SAFETY: `value` has already been type-checked as an AXValueRef; `point`
    // is a correctly sized out-parameter for kAXValueTypeCGPoint.
    let ok = unsafe {
        AXValueGetValue(
            value,
            kAXValueTypeCGPoint,
            &mut point as *mut CGPoint as *mut c_void,
        )
    };
    ok.then_some(point)
}

pub(super) fn cgsize_from_ax_value(value: AXValueRef) -> Option<CGSize> {
    let mut size = CGSize::new(0.0, 0.0);
    // SAFETY: `value` has already been type-checked as an AXValueRef; `size`
    // is a correctly sized out-parameter for kAXValueTypeCGSize.
    let ok = unsafe {
        AXValueGetValue(
            value,
            kAXValueTypeCGSize,
            &mut size as *mut CGSize as *mut c_void,
        )
    };
    ok.then_some(size)
}

pub(super) fn cf_string(value: CFTypeRef) -> Option<String> {
    // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
    // retains it for this temporary wrapper.
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    value.downcast::<CFString>().map(|s| s.to_string())
}

pub(super) fn cf_value_string(value: CFTypeRef) -> Option<String> {
    if let Some(string) = cf_string(value) {
        return Some(string);
    }

    // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
    // retains it for this temporary wrapper.
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    // SAFETY: `value` wraps a valid CF object retained for this scope.
    let value_type = unsafe { CFGetTypeID(value.as_CFTypeRef()) };
    if value_type == CFBoolean::type_id() {
        return value
            .downcast::<CFBoolean>()
            .map(|boolean| bool::from(boolean).to_string());
    }

    if value_type == CFNumber::type_id() {
        // SAFETY: the CFTypeID check above proves this object is a CFNumber.
        let number = unsafe { CFNumber::wrap_under_get_rule(value.as_CFTypeRef() as _) };
        let number = number.to_f64()?;
        if number.fract() == 0.0 {
            Some(format!("{number:.0}"))
        } else {
            Some(number.to_string())
        }
    } else {
        None
    }
}

pub(super) fn cf_label_string(value: CFTypeRef) -> Option<String> {
    cf_string(value).filter(|s| !s.is_empty())
}

pub(super) fn cf_bool(value: CFTypeRef) -> Option<bool> {
    // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
    // retains it for this temporary wrapper.
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    value.downcast::<CFBoolean>().map(bool::from)
}

pub(super) fn cf_array(value: CFTypeRef) -> Option<CFArray> {
    // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
    // retains it for this temporary wrapper.
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    value.downcast_into::<CFArray>()
}

pub(super) fn ax_value_ref(value: CFTypeRef) -> Option<AXValueRef> {
    // SAFETY: caller passes a valid borrowed CF object.
    let value_type = unsafe { CFGetTypeID(value) };
    // SAFETY: AXValueGetTypeID takes no inputs and returns a stable CFTypeID.
    let ax_value_type = unsafe { accessibility_sys::AXValueGetTypeID() };
    if value_type != ax_value_type {
        return None;
    }
    let value = value as AXValueRef;
    // SAFETY: the CFTypeID check above proves `value` is an AXValueRef.
    if unsafe { AXValueGetType(value) } == kAXValueTypeAXError {
        let mut err = kAXErrorSuccess;
        // SAFETY: `value` is an AXValueRef containing an AXError payload;
        // `err` is a correctly sized out-parameter.
        let ok = unsafe {
            AXValueGetValue(
                value,
                kAXValueTypeAXError,
                &mut err as *mut AXError as *mut c_void,
            )
        };
        if ok && err == kAXErrorNoValue {
            return None;
        }
    }
    Some(value)
}

pub(super) fn normalize_batch_value(value: CFTypeRef) -> Option<CFTypeRef> {
    if value.is_null() {
        return None;
    }
    // SAFETY: `value` is a non-null borrowed CF object.
    let type_id = unsafe { CFGetTypeID(value) };
    // SAFETY: CFNullGetTypeID takes no inputs and returns a stable CFTypeID.
    if type_id == unsafe { CFNullGetTypeID() } {
        return None;
    }
    // SAFETY: AXValueGetTypeID takes no inputs and returns a stable CFTypeID.
    if type_id == unsafe { accessibility_sys::AXValueGetTypeID() } {
        let ax_value = value as AXValueRef;
        // SAFETY: the CFTypeID check above proves `ax_value` is an AXValueRef.
        if unsafe { AXValueGetType(ax_value) } == kAXValueTypeAXError {
            let mut err = kAXErrorSuccess;
            // SAFETY: `ax_value` is an AXValueRef containing an AXError
            // payload; `err` is a correctly sized out-parameter.
            let ok = unsafe {
                AXValueGetValue(
                    ax_value,
                    kAXValueTypeAXError,
                    &mut err as *mut AXError as *mut c_void,
                )
            };
            if ok && err == kAXErrorNoValue {
                return None;
            }
        }
    }
    Some(value)
}
