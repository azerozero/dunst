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
    use std::{
        cell::RefCell,
        collections::HashMap,
        env,
        ffi::{c_uchar, c_void},
        mem, ptr, thread,
        time::{Duration, Instant},
    };

    use accessibility_sys::{
        error_string, kAXChildrenAttribute, kAXDescriptionAttribute, kAXEnabledAttribute,
        kAXErrorCannotComplete, kAXErrorInvalidUIElement, kAXErrorNoValue, kAXErrorSuccess,
        kAXFocusedAttribute, kAXHelpAttribute, kAXIdentifierAttribute, kAXMainWindowAttribute,
        kAXMenuBarAttribute, kAXPositionAttribute, kAXPressAction, kAXRaiseAction,
        kAXRoleAttribute, kAXShowMenuAction, kAXSizeAttribute, kAXTitleAttribute,
        kAXValueAttribute, kAXValueTypeAXError, kAXValueTypeCGPoint, kAXValueTypeCGRect,
        kAXValueTypeCGSize, kAXWindowsAttribute, AXError, AXIsProcessTrusted,
        AXUIElementCopyActionNames, AXUIElementCopyAttributeValue,
        AXUIElementCopyMultipleAttributeValues, AXUIElementCreateApplication,
        AXUIElementIsAttributeSettable, AXUIElementPerformAction, AXUIElementRef,
        AXUIElementSetAttributeValue, AXValueGetType, AXValueGetValue, AXValueRef,
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
        display::CGDisplay,
        event::{CGEvent, CGEventType, CGMouseButton},
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
    const IDX_ROLE: usize = 0;
    const IDX_VALUE: usize = 1;
    const IDX_TITLE: usize = 2;
    const IDX_DESCRIPTION: usize = 3;
    const IDX_HELP: usize = 4;
    const IDX_IDENTIFIER: usize = 5;
    const IDX_FRAME: usize = 6;
    const IDX_POSITION: usize = 7;
    const IDX_SIZE: usize = 8;
    const IDX_ENABLED: usize = 9;
    const IDX_FOCUSED: usize = 10;
    const IDX_CHILDREN: usize = 11;
    const DRAG_STEPS: usize = 8;
    const DRAG_STEP_DELAY: Duration = Duration::from_millis(8);
    const AX_MESSAGING_TIMEOUT_SECS: f32 = 1.0;

    thread_local! {
        static AX_CACHE: RefCell<HashMap<CacheKey, AxElement>> = RefCell::new(HashMap::new());
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct TargetKey {
        pid: i32,
        window_id: u32,
    }

    impl TargetKey {
        fn from_target(target: &Target) -> Self {
            Self {
                pid: target.pid,
                window_id: target.window_id,
            }
        }
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct CacheKey {
        target: TargetKey,
        element: ElementKey,
    }

    impl CacheKey {
        fn from_scene(target: &Target, node: &SceneNode) -> Self {
            Self {
                target: TargetKey::from_target(target),
                element: ElementKey::from_scene(node),
            }
        }
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct ElementKey {
        ax_identifier: Option<String>,
        ax_role: String,
        label: Option<String>,
        bbox: Option<(i64, i64, i64, i64)>,
    }

    impl ElementKey {
        fn from_raw(node: &RawAxNode) -> Self {
            Self {
                ax_identifier: node.ax_identifier.clone(),
                ax_role: node.ax_role.clone(),
                label: node.label.clone(),
                bbox: node.frame.and_then(round_bbox),
            }
        }

        fn from_scene(node: &SceneNode) -> Self {
            Self {
                ax_identifier: node.ax_identifier.clone(),
                ax_role: node.ax_role.clone(),
                label: node.label.clone(),
                bbox: node.bbox.and_then(round_bbox),
            }
        }
    }

    struct AxElement(AXUIElementRef);

    impl AxElement {
        fn from_owned(raw: AXUIElementRef) -> Self {
            set_ax_timeout(raw);
            Self(raw)
        }

        fn as_ptr(&self) -> AXUIElementRef {
            self.0
        }

        fn retain_clone(&self) -> Self {
            unsafe {
                core_foundation_sys::base::CFRetain(self.0 as CFTypeRef);
            }
            Self::from_owned(self.0)
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

    fn set_ax_timeout(element: AXUIElementRef) {
        if !element.is_null() {
            unsafe {
                accessibility_sys::AXUIElementSetMessagingTimeout(
                    element,
                    AX_MESSAGING_TIMEOUT_SECS,
                );
            }
        }
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

    fn perform_with_cache(
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
        key: &CacheKey,
        path: &mut &'static str,
    ) -> Result<()> {
        if matches!(action, SemanticAction::Hover | SemanticAction::Drag) {
            return perform_on_element(None, target.pid, node, action, argument)
                .map_err(ActionFailure::into);
        }

        if env::var_os("VO_ACTION_DISABLE_CACHE").is_none() {
            if let Some(element) = cached_element(key) {
                if !cached_element_matches_target(&element, target) {
                    remove_cached_element(key);
                } else {
                    *path = "cached";
                    match perform_on_element(Some(&element), target.pid, node, action, argument) {
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
            VisualOpsError::ElementNotFound(format!(
                "role={:?} label={:?} identifier={:?}",
                node.ax_role, node.label, node.ax_identifier
            ))
        })?;
        perform_on_element(Some(&element), target.pid, node, action, argument)
            .map_err(ActionFailure::into)
    }

    fn perform_on_element(
        element: Option<&AxElement>,
        pid: i32,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> std::result::Result<(), ActionFailure> {
        match action {
            // AX actions/set-attribute are non-intrusive: no global cursor movement
            // and no foreground activation. `Raise` below is the intentional exception.
            SemanticAction::Click | SemanticAction::Pick => {
                let element = require_ax_element(element)?;
                perform_ax_action(element, kAXPressAction)
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
                type_text(element, pid, text)
            }
            SemanticAction::Hover => hover(pid, node),
            SemanticAction::Drag => drag(pid, node, argument),
            other => Err(ActionFailure::Execution(format!(
                "semantic action {other:?} is not supported by macOS AX backend"
            ))),
        }
    }

    fn require_ax_element(
        element: Option<&AxElement>,
    ) -> std::result::Result<&AxElement, ActionFailure> {
        element
            .ok_or_else(|| ActionFailure::Execution("action requires a resolved AX element".into()))
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
            Ok(AxElement::from_owned(app))
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

        if requested_window_id != 0 {
            if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
                if ax_window_id(&main_window) == Some(requested_window_id) {
                    return Ok(main_window);
                }
            }
            return Err(VisualOpsError::Perception(format!(
                "window id {requested_window_id} not found; target window may be closed"
            )));
        }

        if let Some(main_window) = attr_ax_element(app, kAXMainWindowAttribute) {
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

    fn round_bbox(bbox: Bbox) -> Option<(i64, i64, i64, i64)> {
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

    struct NodeFields {
        ax_role: String,
        value: Option<String>,
        title: Option<String>,
        description: Option<String>,
        help: Option<String>,
        ax_identifier: Option<String>,
        ax_actions: Vec<String>,
        frame: Option<Bbox>,
        enabled: bool,
        focused: bool,
    }

    fn assemble_node(fields: NodeFields) -> RawAxNode {
        let label = fields
            .title
            .or(fields.description)
            .or_else(|| static_text_value_label(&fields.ax_role, &fields.value));

        RawAxNode {
            ax_role: fields.ax_role,
            label,
            help: fields.help,
            value: fields.value,
            ax_identifier: fields.ax_identifier,
            ax_actions: fields.ax_actions,
            frame: fields.frame,
            enabled: fields.enabled,
            focused: fields.focused,
            children: Vec::new(),
        }
    }

    fn static_text_value_label(ax_role: &str, value: &Option<String>) -> Option<String> {
        if ax_role == "AXStaticText" {
            value.clone().filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    fn walk_element(
        element: &AxElement,
        target_key: &TargetKey,
        depth: usize,
        state: &mut WalkState,
        attrs: &WalkAttributes,
    ) -> Result<RawAxNode> {
        state.count += 1;
        let Some(batch) = BatchValues::read(element, attrs) else {
            return walk_element_single(element, target_key, depth, state, attrs);
        };
        let ax_role = batch
            .get(IDX_ROLE)
            .and_then(cf_string)
            .unwrap_or_else(|| "AXUnknown".into());

        let fields = NodeFields {
            ax_role,
            value: batch.get(IDX_VALUE).and_then(cf_string),
            title: batch.get(IDX_TITLE).and_then(cf_label_string),
            description: batch.get(IDX_DESCRIPTION).and_then(cf_label_string),
            help: batch.get(IDX_HELP).and_then(cf_string),
            ax_identifier: batch.get(IDX_IDENTIFIER).and_then(cf_string),
            ax_actions: read_node_actions(element),
            frame: frame_from_batch(&batch),
            enabled: batch.get(IDX_ENABLED).and_then(cf_bool).unwrap_or(true),
            focused: batch.get(IDX_FOCUSED).and_then(cf_bool).unwrap_or(false),
        };
        finish_walk_element(
            element,
            target_key,
            depth,
            state,
            attrs,
            fields,
            batch.get(IDX_CHILDREN).and_then(cf_array),
        )
    }

    fn walk_element_single(
        element: &AxElement,
        target_key: &TargetKey,
        depth: usize,
        state: &mut WalkState,
        attrs: &WalkAttributes,
    ) -> Result<RawAxNode> {
        let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());

        let fields = NodeFields {
            ax_role,
            value: attr_string(element, kAXValueAttribute),
            title: attr_label_string(element, kAXTitleAttribute),
            description: attr_label_string(element, kAXDescriptionAttribute),
            help: attr_string(element, kAXHelpAttribute),
            ax_identifier: attr_string(element, kAXIdentifierAttribute),
            ax_actions: read_node_actions(element),
            frame: frame(element),
            enabled: attr_bool(element, kAXEnabledAttribute).unwrap_or(true),
            focused: attr_bool(element, kAXFocusedAttribute).unwrap_or(false),
        };
        finish_walk_element(
            element,
            target_key,
            depth,
            state,
            attrs,
            fields,
            attr_array(element, kAXChildrenAttribute),
        )
    }

    fn finish_walk_element(
        element: &AxElement,
        target_key: &TargetKey,
        depth: usize,
        state: &mut WalkState,
        attrs: &WalkAttributes,
        fields: NodeFields,
        children: Option<CFArray>,
    ) -> Result<RawAxNode> {
        let mut node = assemble_node(fields);
        cache_element(target_key, &node, element);

        if depth >= MAX_DEPTH || state.count >= MAX_NODES {
            state.capped = true;
            return Ok(node);
        }

        if let Some(children) = children {
            for child in ax_elements(&children) {
                if state.count >= MAX_NODES {
                    state.capped = true;
                    break;
                }
                node.children
                    .push(walk_element(&child, target_key, depth + 1, state, attrs)?);
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
        element_key(element)
            .map(|key| key == ElementKey::from_scene(wanted))
            .unwrap_or(false)
    }

    fn clear_cache() {
        AX_CACHE.with(|cache| cache.borrow_mut().clear());
    }

    fn cache_element(target_key: &TargetKey, node: &RawAxNode, element: &AxElement) {
        AX_CACHE.with(|cache| {
            cache.borrow_mut().insert(
                CacheKey {
                    target: target_key.clone(),
                    element: ElementKey::from_raw(node),
                },
                element.retain_clone(),
            );
        });
    }

    fn cached_element(key: &CacheKey) -> Option<AxElement> {
        AX_CACHE.with(|cache| cache.borrow().get(key).map(AxElement::retain_clone))
    }

    fn remove_cached_element(key: &CacheKey) {
        AX_CACHE.with(|cache| {
            cache.borrow_mut().remove(key);
        });
    }

    fn cached_element_matches_target(element: &AxElement, target: &Target) -> bool {
        target.window_id == 0 || ax_window_id(element) == Some(target.window_id)
    }

    fn element_key(element: &AxElement) -> Option<ElementKey> {
        let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
        let value = attr_string(element, kAXValueAttribute);
        let label = attr_label_string(element, kAXTitleAttribute)
            .or_else(|| attr_label_string(element, kAXDescriptionAttribute))
            .or_else(|| {
                if ax_role == "AXStaticText" {
                    value.filter(|s| !s.is_empty())
                } else {
                    None
                }
            });
        Some(ElementKey {
            ax_identifier: attr_string(element, kAXIdentifierAttribute),
            ax_role,
            label,
            bbox: frame(element).and_then(round_bbox),
        })
    }

    enum ActionFailure {
        Ax {
            operation: &'static str,
            err: AXError,
        },
        Execution(String),
    }

    impl ActionFailure {
        fn is_stale(&self) -> bool {
            matches!(
                self,
                Self::Ax { err, .. }
                    if *err == kAXErrorInvalidUIElement || *err == kAXErrorCannotComplete
            )
        }
    }

    impl From<ActionFailure> for VisualOpsError {
        fn from(value: ActionFailure) -> Self {
            match value {
                ActionFailure::Ax { operation, err } => VisualOpsError::Execution(format!(
                    "{operation} failed: {} ({err})",
                    error_string(err)
                )),
                ActionFailure::Execution(message) => VisualOpsError::Execution(message),
            }
        }
    }

    fn perform_ax_action(
        element: &AxElement,
        action: &str,
    ) -> std::result::Result<(), ActionFailure> {
        let action = CFString::new(action);
        let err =
            unsafe { AXUIElementPerformAction(element.as_ptr(), action.as_concrete_TypeRef()) };
        ax_action_result(err, "perform AX action")
    }

    fn set_bool_attr(
        element: &AxElement,
        attr: &str,
        value: bool,
    ) -> std::result::Result<(), ActionFailure> {
        ax_action_result(
            set_bool_attr_raw(element, attr, value),
            "set AX bool attribute",
        )
    }

    fn set_bool_attr_raw(element: &AxElement, attr: &str, value: bool) -> AXError {
        let attr = CFString::new(attr);
        let value = CFBoolean::from(value);
        unsafe {
            AXUIElementSetAttributeValue(
                element.as_ptr(),
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        }
    }

    fn set_string_attr_raw(element: &AxElement, attr: &str, value: &str) -> AXError {
        let attr = CFString::new(attr);
        let value = CFString::new(value);
        unsafe {
            AXUIElementSetAttributeValue(
                element.as_ptr(),
                attr.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        }
    }

    fn ax_action_result(
        err: AXError,
        operation: &'static str,
    ) -> std::result::Result<(), ActionFailure> {
        if err == kAXErrorSuccess {
            Ok(())
        } else {
            Err(ActionFailure::Ax { operation, err })
        }
    }

    fn type_text(
        element: &AxElement,
        pid: i32,
        text: &str,
    ) -> std::result::Result<(), ActionFailure> {
        if attr_settable(element, kAXValueAttribute) {
            let err = set_string_attr_raw(element, kAXValueAttribute, text);
            if err == kAXErrorSuccess {
                match attr_string(element, kAXValueAttribute) {
                    Some(value) if value != text => {}
                    _ => return Ok(()),
                }
            } else if is_stale_ax_error(err) {
                return Err(ActionFailure::Ax {
                    operation: "set AX string attribute",
                    err,
                });
            }
        }

        // AX set-value replaces text; synthetic Unicode keystrokes append to the focused editor.
        set_bool_attr(element, kAXFocusedAttribute, true)?;
        post_unicode_text(pid, text)
    }

    fn attr_settable(element: &AxElement, attr: &str) -> bool {
        let attr = CFString::new(attr);
        let mut settable: c_uchar = 0;
        let err = unsafe {
            AXUIElementIsAttributeSettable(
                element.as_ptr(),
                attr.as_concrete_TypeRef(),
                &mut settable,
            )
        };
        err == kAXErrorSuccess && settable != 0
    }

    fn post_unicode_text(pid: i32, text: &str) -> std::result::Result<(), ActionFailure> {
        let source = event_source("create keyboard CGEventSource")?;
        for ch in text.chars() {
            let s = ch.to_string();
            let down = CGEvent::new_keyboard_event(source.clone(), 0, true).map_err(|err| {
                ActionFailure::Execution(format!("create key down CGEvent: {err:?}"))
            })?;
            down.set_string(&s);
            down.post_to_pid(pid);

            let up = CGEvent::new_keyboard_event(source.clone(), 0, false).map_err(|err| {
                ActionFailure::Execution(format!("create key up CGEvent: {err:?}"))
            })?;
            up.set_string(&s);
            up.post_to_pid(pid);
        }
        Ok(())
    }

    fn hover(pid: i32, node: &SceneNode) -> std::result::Result<(), ActionFailure> {
        let Some(bbox) = node.bbox else {
            return Ok(());
        };
        let source = event_source("create hover CGEventSource")?;
        let point = CGPoint::new(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
        let saved_cursor = current_cursor_position(&source)?;
        let result = (|| {
            let event = CGEvent::new_mouse_event(
                source,
                CGEventType::MouseMoved,
                point,
                CGMouseButton::Left,
            )
            .map_err(|err| ActionFailure::Execution(format!("create hover CGEvent: {err:?}")))?;
            event.post_to_pid(pid);
            Ok(())
        })();
        restore_cursor_position(saved_cursor)?;
        result
    }

    fn drag(
        pid: i32,
        node: &SceneNode,
        argument: Option<&str>,
    ) -> std::result::Result<(), ActionFailure> {
        let start_bbox = node
            .bbox
            .ok_or_else(|| ActionFailure::Execution("Drag requires a source bbox".into()))?;
        let display_bounds = CGDisplay::main().bounds();
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

    fn parse_drop_point(argument: Option<&str>) -> std::result::Result<CGPoint, ActionFailure> {
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

    fn clamp_point_to_bounds(point: CGPoint, bounds: CGRect) -> CGPoint {
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

    fn post_mouse(
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

    fn current_cursor_position(
        source: &CGEventSource,
    ) -> std::result::Result<CGPoint, ActionFailure> {
        let event = CGEvent::new(source.clone())
            .map_err(|err| ActionFailure::Execution(format!("read cursor CGEvent: {err:?}")))?;
        Ok(event.location())
    }

    fn restore_cursor_position(point: CGPoint) -> std::result::Result<(), ActionFailure> {
        CGDisplay::warp_mouse_cursor_position(point)
            .map_err(|err| ActionFailure::Execution(format!("restore cursor position: {err:?}")))
    }

    fn event_source(operation: &'static str) -> std::result::Result<CGEventSource, ActionFailure> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|err| ActionFailure::Execution(format!("{operation}: {err:?}")))
    }

    fn is_stale_ax_error(err: AXError) -> bool {
        err == kAXErrorInvalidUIElement || err == kAXErrorCannotComplete
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
            Some(AxElement::from_owned(raw))
        } else {
            None
        }
    }

    fn read_node_actions(element: &AxElement) -> Vec<String> {
        // PERF: this remains one AX round-trip per node; kAXActions can be folded
        // into the batch request if the FFI constant is introduced locally.
        action_names(element)
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
                elements.push(AxElement::from_owned(cf_ref as AXUIElementRef));
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
