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
        kAXErrorSuccess, kAXFocusedAttribute, kAXHelpAttribute, kAXIdentifierAttribute,
        kAXMainWindowAttribute, kAXMenuBarAttribute, kAXPositionAttribute, kAXPressAction,
        kAXRaiseAction, kAXRoleAttribute, kAXShowMenuAction, kAXSizeAttribute,
        kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCGRect, kAXValueTypeCGPoint,
        kAXValueTypeCGSize, kAXWindowsAttribute, AXError, AXIsProcessTrusted,
        AXUIElementCopyActionNames, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
        AXUIElementPerformAction, AXUIElementRef, AXUIElementSetAttributeValue, AXValueGetValue,
        AXValueRef,
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

    struct AxValue(AXValueRef);

    impl AxValue {
        fn as_ptr(&self) -> AXValueRef {
            self.0
        }
    }

    impl Drop for AxValue {
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
        let mut state = WalkState::default();
        let mut roots = vec![walk_element(&window, 0, &mut state)?];
        if let Some(menu_bar) = attr_ax_element(&app, kAXMenuBarAttribute) {
            roots.push(walk_element(&menu_bar, 0, &mut state)?);
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
            SemanticAction::Click | SemanticAction::Pick => perform_ax_action(&element, kAXPressAction),
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

    fn walk_element(
        element: &AxElement,
        depth: usize,
        state: &mut WalkState,
    ) -> Result<RawAxNode> {
        // PERF/post-POC: batch hot attribute reads with AXUIElementCopyMultipleAttributeValues.
        state.count += 1;
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
                node.children.push(walk_element(&child, depth + 1, state)?);
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
        let err = unsafe { AXUIElementPerformAction(element.as_ptr(), action.as_concrete_TypeRef()) };
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
        value.downcast::<CFString>().map(|s| s.to_string())
    }

    fn attr_label_string(element: &AxElement, attr: &str) -> Option<String> {
        attr_string(element, attr).filter(|s| !s.is_empty())
    }

    fn attr_bool(element: &AxElement, attr: &str) -> Option<bool> {
        let value = attr_value(element, attr)?;
        value.downcast::<CFBoolean>().map(bool::from)
    }

    fn attr_array(element: &AxElement, attr: &str) -> Option<CFArray> {
        let value = attr_value(element, attr)?;
        value.downcast_into::<CFArray>()
    }

    fn attr_ax_element(element: &AxElement, attr: &str) -> Option<AxElement> {
        let value = attr_value(element, attr)?;
        if unsafe { CFGetTypeID(value.as_CFTypeRef()) } == unsafe { accessibility_sys::AXUIElementGetTypeID() } {
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

    fn attr_cgrect(element: &AxElement, attr: &str) -> Option<Bbox> {
        let value = attr_ax_value(element, attr)?;
        let mut rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
        let ok = unsafe {
            AXValueGetValue(
                value.as_ptr(),
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

    fn attr_cgpoint(element: &AxElement, attr: &str) -> Option<CGPoint> {
        let value = attr_ax_value(element, attr)?;
        let mut point = CGPoint::new(0.0, 0.0);
        let ok = unsafe {
            AXValueGetValue(
                value.as_ptr(),
                kAXValueTypeCGPoint,
                &mut point as *mut CGPoint as *mut c_void,
            )
        };
        ok.then_some(point)
    }

    fn attr_cgsize(element: &AxElement, attr: &str) -> Option<CGSize> {
        let value = attr_ax_value(element, attr)?;
        let mut size = CGSize::new(0.0, 0.0);
        let ok = unsafe {
            AXValueGetValue(
                value.as_ptr(),
                kAXValueTypeCGSize,
                &mut size as *mut CGSize as *mut c_void,
            )
        };
        ok.then_some(size)
    }

    fn attr_ax_value(element: &AxElement, attr: &str) -> Option<AxValue> {
        let value = attr_value(element, attr)?;
        if unsafe { CFGetTypeID(value.as_CFTypeRef()) } == unsafe { accessibility_sys::AXValueGetTypeID() } {
            let raw = value.as_CFTypeRef() as AXValueRef;
            mem::forget(value);
            Some(AxValue(raw))
        } else {
            None
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
