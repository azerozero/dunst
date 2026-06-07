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
    use std::{ffi::c_void, ptr};

    use accessibility_sys::{
        error_string, kAXChildrenAttribute, kAXDescriptionAttribute, kAXEnabledAttribute,
        kAXErrorSuccess, kAXFocusedAttribute, kAXHelpAttribute, kAXIdentifierAttribute,
        kAXMainWindowAttribute, kAXPositionAttribute, kAXPressAction, kAXRaiseAction,
        kAXRoleAttribute, kAXShowMenuAction, kAXSizeAttribute, kAXTitleAttribute,
        kAXValueAttribute, kAXValueTypeCGRect, kAXValueTypeCGPoint, kAXValueTypeCGSize,
        kAXWindowsAttribute, AXError, AXIsProcessTrusted, AXUIElementCopyActionNames,
        AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementPerformAction,
        AXUIElementRef, AXUIElementSetAttributeValue, AXValueGetValue, AXValueRef,
    };
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFRelease, CFType, CFTypeRef, TCFType},
        boolean::CFBoolean,
        string::CFString,
    };
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton},
        event_source::{CGEventSource, CGEventSourceStateID},
        geometry::{CGPoint, CGRect, CGSize},
    };
    use visualops_core::{Bbox, RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef};

    const MAX_NODES: usize = 5_000;
    const MAX_DEPTH: usize = 40;
    const AX_FRAME_ATTRIBUTE: &str = "AXFrame";

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> AXError;
    }

    pub fn capture(target: &Target) -> Result<Vec<RawAxNode>> {
        ensure_trusted()?;
        let app = app_element(target.pid)?;
        let window = resolve_window(app, target.window_id)?;
        let mut state = WalkState::default();
        let root = walk_element(window, 0, &mut state)?;
        if state.capped {
            eprintln!(
                "visualops-platform: AX tree capped at {MAX_NODES} nodes / depth {MAX_DEPTH}"
            );
        }
        Ok(vec![root])
    }

    pub fn window_ref(target: &Target) -> Result<WindowRef> {
        ensure_trusted()?;
        let app = app_element(target.pid)?;
        let window = resolve_window(app, target.window_id)?;
        Ok(WindowRef {
            pid: target.pid,
            window_id: target.window_id,
            app_name: attr_string(app, kAXTitleAttribute).unwrap_or_default(),
            title: attr_string(window, kAXTitleAttribute).unwrap_or_default(),
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
        let window = resolve_window(app, target.window_id)?;
        let element = find_element(window, node)?.ok_or_else(|| {
            VisualOpsError::ElementNotFound(format!(
                "role={:?} label={:?} identifier={:?}",
                node.ax_role, node.label, node.ax_identifier
            ))
        })?;

        match action {
            SemanticAction::Click | SemanticAction::Pick => perform_ax_action(element, kAXPressAction),
            SemanticAction::OpenMenu => perform_ax_action(element, kAXShowMenuAction),
            SemanticAction::Raise => perform_ax_action(element, kAXRaiseAction),
            SemanticAction::Focus => set_bool_attr(element, kAXFocusedAttribute, true),
            SemanticAction::Type => {
                let text = argument.ok_or_else(|| {
                    VisualOpsError::Execution("type action requires an argument".into())
                })?;
                set_string_attr(element, kAXValueAttribute, text)
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

    fn app_element(pid: i32) -> Result<AXUIElementRef> {
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            Err(VisualOpsError::Perception(format!(
                "AXUIElementCreateApplication returned null for pid {pid}"
            )))
        } else {
            unsafe {
                accessibility_sys::AXUIElementSetMessagingTimeout(app, 5.0);
            }
            Ok(app)
        }
    }

    fn resolve_window(app: AXUIElementRef, requested_window_id: u32) -> Result<AXUIElementRef> {
        if let Some(windows) = attr_array(app, kAXWindowsAttribute) {
            for window in ax_elements(&windows) {
                if let Some(window_id) = ax_window_id(window) {
                    if window_id == requested_window_id {
                        return Ok(window);
                    }
                }
            }
        }

        if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
            eprintln!(
                "visualops-platform: window id {requested_window_id} not found; using AXMainWindow"
            );
            return Ok(main_window);
        }

        if let Some(windows) = attr_array(app, kAXWindowsAttribute) {
            if let Some(first_window) = ax_elements(&windows).into_iter().next() {
                eprintln!(
                    "visualops-platform: window id {requested_window_id} not found; using first AX window"
                );
                return Ok(first_window);
            }
        }

        Err(VisualOpsError::Perception(format!(
            "no AX window found for requested window id {requested_window_id}"
        )))
    }

    fn ax_window_id(element: AXUIElementRef) -> Option<u32> {
        let mut window_id = 0;
        let err = unsafe { _AXUIElementGetWindow(element, &mut window_id) };
        (err == kAXErrorSuccess).then_some(window_id)
    }

    #[derive(Default)]
    struct WalkState {
        count: usize,
        capped: bool,
    }

    fn walk_element(
        element: AXUIElementRef,
        depth: usize,
        state: &mut WalkState,
    ) -> Result<RawAxNode> {
        state.count += 1;
        let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
        let value = attr_string(element, kAXValueAttribute);
        let label = attr_string(element, kAXTitleAttribute)
            .or_else(|| attr_string(element, kAXDescriptionAttribute))
            .or_else(|| {
                if ax_role == "AXStaticText" {
                    value.clone()
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
                node.children.push(walk_element(child, depth + 1, state)?);
            }
        }

        Ok(node)
    }

    fn find_element(root: AXUIElementRef, wanted: &SceneNode) -> Result<Option<AXUIElementRef>> {
        let mut stack = vec![(root, 0usize)];
        let mut seen = 0usize;

        while let Some((element, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_NODES || depth > MAX_DEPTH {
                eprintln!("visualops-platform: live element search capped");
                break;
            }

            if element_matches(element, wanted) {
                return Ok(Some(element));
            }

            if let Some(children) = attr_array(element, kAXChildrenAttribute) {
                let mut child_elements = ax_elements(&children);
                child_elements.reverse();
                for child in child_elements {
                    stack.push((child, depth + 1));
                }
            }
        }

        Ok(None)
    }

    fn element_matches(element: AXUIElementRef, wanted: &SceneNode) -> bool {
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

        let label = attr_string(element, kAXTitleAttribute)
            .or_else(|| attr_string(element, kAXDescriptionAttribute))
            .or_else(|| attr_string(element, kAXValueAttribute));

        wanted.label.is_none() || label == wanted.label
    }

    fn perform_ax_action(element: AXUIElementRef, action: &str) -> Result<()> {
        let action = CFString::new(action);
        let err = unsafe { AXUIElementPerformAction(element, action.as_concrete_TypeRef()) };
        ax_result(err, "perform AX action")
    }

    fn set_bool_attr(element: AXUIElementRef, attr: &str, value: bool) -> Result<()> {
        let attr = CFString::new(attr);
        let value = CFBoolean::from(value);
        let err = unsafe {
            AXUIElementSetAttributeValue(
                element,
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        };
        ax_result(err, "set AX bool attribute")
    }

    fn set_string_attr(element: AXUIElementRef, attr: &str, value: &str) -> Result<()> {
        let attr = CFString::new(attr);
        let value = CFString::new(value);
        let err = unsafe {
            AXUIElementSetAttributeValue(
                element,
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

    fn attr_value(element: AXUIElementRef, attr: &str) -> Option<CFType> {
        let attr = CFString::new(attr);
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
        };
        if err == kAXErrorSuccess && !value.is_null() {
            Some(unsafe { CFType::wrap_under_create_rule(value) })
        } else {
            None
        }
    }

    fn attr_string(element: AXUIElementRef, attr: &str) -> Option<String> {
        let value = attr_value(element, attr)?;
        value
            .downcast::<CFString>()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }

    fn attr_bool(element: AXUIElementRef, attr: &str) -> Option<bool> {
        let value = attr_value(element, attr)?;
        value.downcast::<CFBoolean>().map(bool::from)
    }

    fn attr_array(element: AXUIElementRef, attr: &str) -> Option<CFArray> {
        let value = attr_value(element, attr)?;
        value.downcast_into::<CFArray>()
    }

    fn attr_ax_element(element: AXUIElementRef, attr: &str) -> Option<AXUIElementRef> {
        let value = attr_value(element, attr)?;
        if unsafe { CFGetTypeID(value.as_CFTypeRef()) } == unsafe { accessibility_sys::AXUIElementGetTypeID() } {
            let raw = value.as_CFTypeRef() as AXUIElementRef;
            std::mem::forget(value);
            Some(raw)
        } else {
            None
        }
    }

    fn action_names(element: AXUIElementRef) -> Vec<String> {
        let mut actions: CFArrayRef = ptr::null();
        let err = unsafe { AXUIElementCopyActionNames(element, &mut actions) };
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

    fn ax_elements(array: &CFArray) -> Vec<AXUIElementRef> {
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
                elements.push(cf_ref as AXUIElementRef);
            }
        }
        elements
    }

    fn frame(element: AXUIElementRef) -> Option<Bbox> {
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

    fn attr_cgrect(element: AXUIElementRef, attr: &str) -> Option<Bbox> {
        let value = attr_ax_value(element, attr)?;
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

    fn attr_cgpoint(element: AXUIElementRef, attr: &str) -> Option<CGPoint> {
        let value = attr_ax_value(element, attr)?;
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

    fn attr_cgsize(element: AXUIElementRef, attr: &str) -> Option<CGSize> {
        let value = attr_ax_value(element, attr)?;
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

    fn attr_ax_value(element: AXUIElementRef, attr: &str) -> Option<AXValueRef> {
        let value = attr_value(element, attr)?;
        if unsafe { CFGetTypeID(value.as_CFTypeRef()) } == unsafe { accessibility_sys::AXValueGetTypeID() } {
            let raw = value.as_CFTypeRef() as AXValueRef;
            std::mem::forget(value);
            Some(raw)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    fn release_ax_element(element: AXUIElementRef) {
        if !element.is_null() {
            unsafe { CFRelease(element as CFTypeRef) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use visualops_core::{RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef};

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
