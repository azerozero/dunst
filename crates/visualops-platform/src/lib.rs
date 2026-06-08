//! macOS platform backend: the **real** [`Perceptor`] (AX tree walk) and
//! [`ActionExecutor`] (perform AX action / set value / CGEvent).
//!
//! This is the only crate that touches macOS FFI. See `docs/WP-A-platform.md`
//! for the full spec, the AX attribute list, and done-criteria.

use visualops_core::{
    ActionExecutor, Perceptor, RawAxNode, Result, SceneNode, SemanticAction, Target, WindowRef,
};

/// AX-backed perception + action for macOS.
#[derive(Debug, Default)]
pub struct MacosBackend {
    _private: (),
}

impl MacosBackend {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::c_void, mem, ptr};

    use accessibility_sys::{
        error_string, kAXChildrenAttribute, kAXDescriptionAttribute, kAXEnabledAttribute,
        kAXErrorNoValue, kAXErrorSuccess, kAXFocusedAttribute, kAXHelpAttribute,
        kAXIdentifierAttribute, kAXMainWindowAttribute, kAXMenuBarAttribute, kAXPositionAttribute,
        kAXPressAction, kAXRaiseAction, kAXRoleAttribute, kAXShowMenuAction, kAXSizeAttribute,
        kAXTitleAttribute, kAXValueAttribute, kAXValueTypeAXError, kAXValueTypeCGPoint,
        kAXValueTypeCGRect, kAXValueTypeCGSize, kAXWindowsAttribute, AXError, AXIsProcessTrusted,
        AXUIElementCopyActionNames, AXUIElementCopyAttributeValue,
        AXUIElementCopyMultipleAttributeValues, AXUIElementCreateApplication,
        AXUIElementPerformAction, AXUIElementRef, AXUIElementSetAttributeValue, AXValueGetType,
        AXValueGetValue, AXValueRef,
    };
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFRelease, CFType, CFTypeRef, TCFType},
        boolean::CFBoolean,
        string::CFString,
    };
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::CFNullGetTypeID;
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton},
        event_source::{CGEventSource, CGEventSourceStateID},
        geometry::{CGPoint, CGRect, CGSize},
    };
    use visualops_core::{
        Bbox, RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
    };

    const MAX_NODES: usize = 5_000;
    const MAX_DEPTH: usize = 40;
    const AX_FRAME_ATTRIBUTE: &str = "AXFrame";
    const BATCH_ATTR_COUNT: usize = 12;

    struct AxElement(AXUIElementRef);

    impl AxElement {
        fn as_ptr(&self) -> AXUIElementRef {
            self.0
        }
    }

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
            }
        }
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;
    }

    pub fn capture(target: &Target) -> Result<Vec<RawAxNode>> {
        ensure_trusted()?;
        let app = app_element(target.pid)?;
        let window = resolve_window(&app, target.window_id)?;
        let walk_attrs = WalkAttributes::new();
        let mut state = WalkState::default();
        let mut roots = vec![walk_element(&window, 0, &mut state, &walk_attrs)?];
        if let Some(menu_bar) = attr_ax_element(&app, kAXMenuBarAttribute) {
            roots.push(walk_element(&menu_bar, 0, &mut state, &walk_attrs)?);
        }
        if state.capped {
            eprintln!(
                "visualops-platform: AX tree capped at {MAX_NODES} nodes / depth {MAX_DEPTH}"
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
            window_id: target.window_id,
            app_name: attr_string(&app, kAXTitleAttribute).unwrap_or_default(),
            title: attr_string(&window, kAXTitleAttribute).unwrap_or_default(),
        })
    }

    pub fn perform(
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()> {
        ensure_trusted()?;
        let app = app_element(target.pid)?;
        let window = resolve_window(&app, target.window_id)?;
        let element = find_element(window, node)?.ok_or_else(|| {
            VisualOpsError::ElementNotFound(format!(
                "role={:?} label={:?} identifier={:?}",
                node.ax_role, node.label, node.ax_identifier
            ))
        })?;

        match action {
            SemanticAction::Click | SemanticAction::Pick => {
                perform_ax_action(&element, kAXPressAction)
            }
            SemanticAction::OpenMenu => perform_ax_action(&element, kAXShowMenuAction),
            SemanticAction::Raise => perform_ax_action(&element, kAXRaiseAction),
            SemanticAction::Focus => set_bool_attr(&element, kAXFocusedAttribute, true),
            SemanticAction::Type => {
                let text = argument.ok_or_else(|| {
                    VisualOpsError::Execution("type action requires an argument".into())
                })?;
                // PERF/post-POC: add CGEvent text fallback after the AX set-value baseline.
                set_string_attr(&element, kAXValueAttribute, text)
            }
            SemanticAction::Hover => hover(node),
            other => Err(VisualOpsError::Execution(format!(
                "semantic action {other:?} is not supported by macOS AX backend"
            ))),
        }
    }

    fn ensure_trusted() -> Result<()> {
        let trusted = unsafe { AXIsProcessTrusted() };
        if trusted {
            Ok(())
        } else {
            Err(VisualOpsError::Perception(
                "accessibility not granted for this process".into(),
            ))
        }
    }

    fn app_element(pid: i32) -> Result<AxElement> {
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            Err(VisualOpsError::Perception(format!(
                "AXUIElementCreateApplication returned null for pid {pid}"
            )))
        } else {
            unsafe {
                accessibility_sys::AXUIElementSetMessagingTimeout(app, 1.0);
            }
            Ok(AxElement(app))
        }
    }

    fn resolve_window(app: &AxElement, requested_window_id: u32) -> Result<AxElement> {
        let mut windows = attr_array(app, kAXWindowsAttribute)
            .map(|windows| ax_elements(&windows))
            .unwrap_or_default();

        if let Some(index) = windows
            .iter()
            .position(|window| ax_window_id(window) == Some(requested_window_id))
        {
            return Ok(windows.remove(index));
        }

        if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
            if requested_window_id != 0 {
                eprintln!(
                    "visualops-platform: window id {requested_window_id} not found; using AXMainWindow"
                );
            }
            return Ok(main_window);
        }

        if !windows.is_empty() {
            eprintln!(
                "visualops-platform: window id {requested_window_id} not found; using first AX window"
            );
            return Ok(windows.remove(0));
        }

        Err(VisualOpsError::Perception(format!(
            "no AX window found for requested window id {requested_window_id}"
        )))
    }

    fn ax_window_id(element: &AxElement) -> Option<u32> {
        let mut window_id = 0;
        let err = unsafe { _AXUIElementGetWindow(element.as_ptr(), &mut window_id) };
        (err == kAXErrorSuccess).then_some(window_id)
    }

    #[derive(Default)]
    struct WalkState {
        count: usize,
        capped: bool,
    }

    struct WalkAttributes {
        request: CFArray<CFString>,
    }

    impl WalkAttributes {
        fn new() -> Self {
            Self {
                request: CFArray::from_CFTypes(&[
                    CFString::new(kAXRoleAttribute),
                    CFString::new(kAXValueAttribute),
                    CFString::new(kAXTitleAttribute),
                    CFString::new(kAXDescriptionAttribute),
                    CFString::new(kAXHelpAttribute),
                    CFString::new(kAXIdentifierAttribute),
                    CFString::new(AX_FRAME_ATTRIBUTE),
                    CFString::new(kAXPositionAttribute),
                    CFString::new(kAXSizeAttribute),
                    CFString::new(kAXEnabledAttribute),
                    CFString::new(kAXFocusedAttribute),
                    CFString::new(kAXChildrenAttribute),
                ]),
            }
        }
    }

    struct BatchValues {
        values: CFArray,
        len: usize,
    }

    impl BatchValues {
        fn read(element: &AxElement, attrs: &WalkAttributes) -> Option<Self> {
            let mut values: CFArrayRef = ptr::null();
            let err = unsafe {
                AXUIElementCopyMultipleAttributeValues(
                    element.as_ptr(),
                    attrs.request.as_concrete_TypeRef(),
                    0,
                    &mut values,
                )
            };
            if err == kAXErrorSuccess && !values.is_null() {
                let values = unsafe { CFArray::wrap_under_create_rule(values) };
                let len = unsafe { CFArrayGetCount(values.as_concrete_TypeRef()) as usize };
                Some(Self { values, len })
            } else {
                None
            }
        }

        fn get(&self, index: usize) -> Option<CFTypeRef> {
            if index >= BATCH_ATTR_COUNT || index >= self.len {
                return None;
            }
            let value = unsafe {
                CFArrayGetValueAtIndex(self.values.as_concrete_TypeRef(), index as isize)
                    as CFTypeRef
            };
            normalize_batch_value(value)
        }
    }

    fn walk_element(
        element: &AxElement,
        depth: usize,
        state: &mut WalkState,
        attrs: &WalkAttributes,
    ) -> Result<RawAxNode> {
        state.count += 1;
        let Some(batch) = BatchValues::read(element, attrs) else {
            return walk_element_single(element, depth, state, attrs);
        };
        let ax_role = batch
            .get(0)
            .and_then(cf_string)
            .unwrap_or_else(|| "AXUnknown".into());
        let value = batch.get(1).and_then(cf_string);
        let label = batch
            .get(2)
            .and_then(cf_label_string)
            .or_else(|| batch.get(3).and_then(cf_label_string))
            .or_else(|| {
                if ax_role == "AXStaticText" {
                    value.clone().filter(|s| !s.is_empty())
                } else {
                    None
                }
            });

        let mut node = RawAxNode {
            ax_role,
            label,
            help: batch.get(4).and_then(cf_string),
            value,
            ax_identifier: batch.get(5).and_then(cf_string),
            ax_actions: action_names(element),
            frame: frame_from_batch(&batch),
            enabled: batch.get(9).and_then(cf_bool).unwrap_or(true),
            focused: batch.get(10).and_then(cf_bool).unwrap_or(false),
            children: Vec::new(),
        };

        if depth >= MAX_DEPTH || state.count >= MAX_NODES {
            state.capped = true;
            return Ok(node);
        }

        if let Some(children) = batch.get(11).and_then(cf_array) {
            for child in ax_elements(&children) {
                if state.count >= MAX_NODES {
                    state.capped = true;
                    break;
                }
                node.children
                    .push(walk_element(&child, depth + 1, state, attrs)?);
            }
        }

        Ok(node)
    }

    fn walk_element_single(
        element: &AxElement,
        depth: usize,
        state: &mut WalkState,
        attrs: &WalkAttributes,
    ) -> Result<RawAxNode> {
        let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
        let value = attr_string(element, kAXValueAttribute);
        let label = attr_label_string(element, kAXTitleAttribute)
            .or_else(|| attr_label_string(element, kAXDescriptionAttribute))
            .or_else(|| {
                if ax_role == "AXStaticText" {
                    value.clone().filter(|s| !s.is_empty())
                } else {
                    None
                }
            });

        let mut node = RawAxNode {
            ax_role,
            label,
            help: attr_string(element, kAXHelpAttribute),
            value,
            ax_identifier: attr_string(element, kAXIdentifierAttribute),
            ax_actions: action_names(element),
            frame: frame(element),
            enabled: attr_bool(element, kAXEnabledAttribute).unwrap_or(true),
            focused: attr_bool(element, kAXFocusedAttribute).unwrap_or(false),
            children: Vec::new(),
        };

        if depth >= MAX_DEPTH || state.count >= MAX_NODES {
            state.capped = true;
            return Ok(node);
        }

        if let Some(children) = attr_array(element, kAXChildrenAttribute) {
            for child in ax_elements(&children) {
                if state.count >= MAX_NODES {
                    state.capped = true;
                    break;
                }
                node.children
                    .push(walk_element(&child, depth + 1, state, attrs)?);
            }
        }

        Ok(node)
    }

    fn find_element(root: AxElement, wanted: &SceneNode) -> Result<Option<AxElement>> {
        let mut stack = vec![(root, 0usize)];
        let mut seen = 0usize;

        while let Some((element, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_NODES || depth > MAX_DEPTH {
                eprintln!("visualops-platform: live element search capped");
                break;
            }

            if element_matches(&element, wanted) {
                return Ok(Some(element));
            }

            if let Some(children) = attr_array(&element, kAXChildrenAttribute) {
                let mut child_elements = ax_elements(&children);
                child_elements.reverse();
                for child in child_elements {
                    stack.push((child, depth + 1));
                }
            }
        }

        Ok(None)
    }

    fn element_matches(element: &AxElement, wanted: &SceneNode) -> bool {
        let role_matches = attr_string(element, kAXRoleAttribute)
            .map(|role| role == wanted.ax_role)
            .unwrap_or(false);
        if !role_matches {
            return false;
        }

        let identifier = attr_string(element, kAXIdentifierAttribute);
        if wanted.ax_identifier.is_some() && identifier != wanted.ax_identifier {
            return false;
        }

        let label = attr_label_string(element, kAXTitleAttribute)
            .or_else(|| attr_label_string(element, kAXDescriptionAttribute))
            .or_else(|| {
                if wanted.ax_role == "AXStaticText" {
                    attr_string(element, kAXValueAttribute).filter(|s| !s.is_empty())
                } else {
                    None
                }
            });

        wanted.label.is_none() || label == wanted.label
    }

    fn perform_ax_action(element: &AxElement, action: &str) -> Result<()> {
        let action = CFString::new(action);
        let err =
            unsafe { AXUIElementPerformAction(element.as_ptr(), action.as_concrete_TypeRef()) };
        ax_result(err, "perform AX action")
    }

    fn set_bool_attr(element: &AxElement, attr: &str, value: bool) -> Result<()> {
        let attr = CFString::new(attr);
        let value = CFBoolean::from(value);
        let err = unsafe {
            AXUIElementSetAttributeValue(
                element.as_ptr(),
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        };
        ax_result(err, "set AX bool attribute")
    }

    fn set_string_attr(element: &AxElement, attr: &str, value: &str) -> Result<()> {
        let attr = CFString::new(attr);
        let value = CFString::new(value);
        let err = unsafe {
            AXUIElementSetAttributeValue(
                element.as_ptr(),
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        };
        ax_result(err, "set AX string attribute")
    }

    fn hover(node: &SceneNode) -> Result<()> {
        let Some(bbox) = node.bbox else {
            return Ok(());
        };
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|err| VisualOpsError::Execution(format!("create CGEventSource: {err:?}")))?;
        let point = CGPoint::new(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::MouseMoved,
            point,
            CGMouseButton::Left,
        )
        .map_err(|err| VisualOpsError::Execution(format!("create hover CGEvent: {err:?}")))?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn ax_result(err: AXError, operation: &str) -> Result<()> {
        if err == kAXErrorSuccess {
            Ok(())
        } else {
            Err(VisualOpsError::Execution(format!(
                "{operation} failed: {} ({err})",
                error_string(err)
            )))
        }
    }

    fn attr_value(element: &AxElement, attr: &str) -> Option<CFType> {
        let attr = CFString::new(attr);
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(element.as_ptr(), attr.as_concrete_TypeRef(), &mut value)
        };
        if err == kAXErrorSuccess && !value.is_null() {
            Some(unsafe { CFType::wrap_under_create_rule(value) })
        } else {
            None
        }
    }

    fn attr_string(element: &AxElement, attr: &str) -> Option<String> {
        let value = attr_value(element, attr)?;
        cf_string(value.as_CFTypeRef())
    }

    fn attr_label_string(element: &AxElement, attr: &str) -> Option<String> {
        attr_string(element, attr).filter(|s| !s.is_empty())
    }

    fn attr_bool(element: &AxElement, attr: &str) -> Option<bool> {
        let value = attr_value(element, attr)?;
        cf_bool(value.as_CFTypeRef())
    }

    fn attr_array(element: &AxElement, attr: &str) -> Option<CFArray> {
        let value = attr_value(element, attr)?;
        cf_array(value.as_CFTypeRef())
    }

    fn attr_ax_element(element: &AxElement, attr: &str) -> Option<AxElement> {
        let value = attr_value(element, attr)?;
        if unsafe { CFGetTypeID(value.as_CFTypeRef()) }
            == unsafe { accessibility_sys::AXUIElementGetTypeID() }
        {
            let raw = value.as_CFTypeRef() as AXUIElementRef;
            mem::forget(value);
            Some(AxElement(raw))
        } else {
            None
        }
    }

    fn action_names(element: &AxElement) -> Vec<String> {
        let mut actions: CFArrayRef = ptr::null();
        let err = unsafe { AXUIElementCopyActionNames(element.as_ptr(), &mut actions) };
        if err != kAXErrorSuccess || actions.is_null() {
            return Vec::new();
        }

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

    fn cf_strings(array: &CFArray) -> Vec<String> {
        let mut values = Vec::new();
        let len = unsafe { CFArrayGetCount(array.as_concrete_TypeRef()) };
        for index in 0..len {
            let value = unsafe { CFArrayGetValueAtIndex(array.as_concrete_TypeRef(), index) };
            if value.is_null() {
                continue;
            }
            let value = unsafe { CFType::wrap_under_get_rule(value as CFTypeRef) };
            if let Some(string) = value.downcast::<CFString>() {
                values.push(string.to_string());
            }
        }
        values
    }

    fn ax_elements(array: &CFArray) -> Vec<AxElement> {
        let mut elements = Vec::new();
        let len = unsafe { CFArrayGetCount(array.as_concrete_TypeRef()) };
        let ax_type = unsafe { accessibility_sys::AXUIElementGetTypeID() };
        for index in 0..len {
            let value = unsafe { CFArrayGetValueAtIndex(array.as_concrete_TypeRef(), index) };
            if value.is_null() {
                continue;
            }
            let cf_ref = value as CFTypeRef;
            if unsafe { CFGetTypeID(cf_ref) } == ax_type {
                unsafe {
                    core_foundation_sys::base::CFRetain(cf_ref);
                }
                elements.push(AxElement(cf_ref as AXUIElementRef));
            }
        }
        elements
    }

    fn frame(element: &AxElement) -> Option<Bbox> {
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

    fn frame_from_batch(values: &BatchValues) -> Option<Bbox> {
        cgrect_from_value(values.get(6)?).or_else(|| {
            let origin = cgpoint_from_value(values.get(7)?)?;
            let size = cgsize_from_value(values.get(8)?)?;
            Some(Bbox {
                x: origin.x,
                y: origin.y,
                w: size.width,
                h: size.height,
            })
        })
    }

    fn attr_cgrect(element: &AxElement, attr: &str) -> Option<Bbox> {
        let value = attr_value(element, attr)?;
        cgrect_from_value(value.as_CFTypeRef())
    }

    fn attr_cgpoint(element: &AxElement, attr: &str) -> Option<CGPoint> {
        let value = attr_value(element, attr)?;
        cgpoint_from_value(value.as_CFTypeRef())
    }

    fn attr_cgsize(element: &AxElement, attr: &str) -> Option<CGSize> {
        let value = attr_value(element, attr)?;
        cgsize_from_value(value.as_CFTypeRef())
    }

    fn cgrect_from_value(value: CFTypeRef) -> Option<Bbox> {
        cgrect_from_ax_value(ax_value_ref(value)?)
    }

    fn cgpoint_from_value(value: CFTypeRef) -> Option<CGPoint> {
        cgpoint_from_ax_value(ax_value_ref(value)?)
    }

    fn cgsize_from_value(value: CFTypeRef) -> Option<CGSize> {
        cgsize_from_ax_value(ax_value_ref(value)?)
    }

    fn cgrect_from_ax_value(value: AXValueRef) -> Option<Bbox> {
        let mut rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
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

    fn cgpoint_from_ax_value(value: AXValueRef) -> Option<CGPoint> {
        let mut point = CGPoint::new(0.0, 0.0);
        let ok = unsafe {
            AXValueGetValue(
                value,
                kAXValueTypeCGPoint,
                &mut point as *mut CGPoint as *mut c_void,
            )
        };
        ok.then_some(point)
    }

    fn cgsize_from_ax_value(value: AXValueRef) -> Option<CGSize> {
        let mut size = CGSize::new(0.0, 0.0);
        let ok = unsafe {
            AXValueGetValue(
                value,
                kAXValueTypeCGSize,
                &mut size as *mut CGSize as *mut c_void,
            )
        };
        ok.then_some(size)
    }

    fn cf_string(value: CFTypeRef) -> Option<String> {
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast::<CFString>().map(|s| s.to_string())
    }

    fn cf_label_string(value: CFTypeRef) -> Option<String> {
        cf_string(value).filter(|s| !s.is_empty())
    }

    fn cf_bool(value: CFTypeRef) -> Option<bool> {
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast::<CFBoolean>().map(bool::from)
    }

    fn cf_array(value: CFTypeRef) -> Option<CFArray> {
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast_into::<CFArray>()
    }

    fn ax_value_ref(value: CFTypeRef) -> Option<AXValueRef> {
        if unsafe { CFGetTypeID(value) } != unsafe { accessibility_sys::AXValueGetTypeID() } {
            return None;
        }
        let value = value as AXValueRef;
        if unsafe { AXValueGetType(value) } == kAXValueTypeAXError {
            let mut err = kAXErrorSuccess;
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

    fn normalize_batch_value(value: CFTypeRef) -> Option<CFTypeRef> {
        if value.is_null() {
            return None;
        }
        let type_id = unsafe { CFGetTypeID(value) };
        if type_id == unsafe { CFNullGetTypeID() } {
            return None;
        }
        if type_id == unsafe { accessibility_sys::AXValueGetTypeID() } {
            let ax_value = value as AXValueRef;
            if unsafe { AXValueGetType(ax_value) } == kAXValueTypeAXError {
                let mut err = kAXErrorSuccess;
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
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use visualops_core::{
        RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
    };

    pub fn capture(_target: &Target) -> Result<Vec<RawAxNode>> {
        Err(VisualOpsError::Perception(
            "macOS accessibility backend is only available on macOS".into(),
        ))
    }

    pub fn window_ref(target: &Target) -> Result<WindowRef> {
        Err(VisualOpsError::Perception(format!(
            "macOS accessibility backend is only available on macOS (pid={}, window_id={})",
            target.pid, target.window_id
        )))
    }

    pub fn perform(
        _target: &Target,
        _node: &SceneNode,
        _action: SemanticAction,
        _argument: Option<&str>,
    ) -> Result<()> {
        Err(VisualOpsError::Execution(
            "macOS accessibility backend is only available on macOS".into(),
        ))
    }
}

impl Perceptor for MacosBackend {
    fn capture(&self, target: &Target) -> Result<Vec<RawAxNode>> {
        macos::capture(target)
    }

    fn window_ref(&self, target: &Target) -> Result<WindowRef> {
        macos::window_ref(target)
    }
}

impl ActionExecutor for MacosBackend {
    fn perform(
        &self,
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()> {
        macos::perform(target, node, action, argument)
    }
}
