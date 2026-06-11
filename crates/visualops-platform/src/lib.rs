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

/// Post a background click at a screen point to a macOS process without moving
/// the visible cursor.
#[cfg(target_os = "macos")]
pub fn click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::click_at_point(pid, x, y)
}

/// Post a named keyboard key to a macOS process without touching the mouse.
#[cfg(target_os = "macos")]
pub fn press_key(pid: i32, key: &str) -> Result<()> {
    macos::press_key(pid, key)
}

/// Post a background mouse-move (hover) at a screen point so the target's hover
/// handlers fire (e.g. a chart crosshair tooltip / value-at-cursor) without
/// moving the visible cursor — read the result afterwards with OCR.
#[cfg(target_os = "macos")]
pub fn hover_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
    macos::hover_at_point(pid, x, y)
}

/// Time-multiplex the single OS cursor for a synthetic hover on a non-CDP
/// surface: save the current position, **decouple the hardware mouse** (so the
/// user can't fight the warp), warp to `(x, y)` and post a hover. Returns the
/// saved position to restore with [`cursor_restore`]. The user's mouse is frozen
/// until restore — keep the borrow brief (~tens of ms).
#[cfg(target_os = "macos")]
pub fn cursor_borrow_to(x: f64, y: f64) -> Result<(f64, f64)> {
    macos::cursor_borrow_to(x, y)
}

/// End a [`cursor_borrow_to`]: warp the cursor back to `(x, y)` and **re-couple**
/// the hardware mouse so the user controls it again.
#[cfg(target_os = "macos")]
pub fn cursor_restore(x: f64, y: f64) -> Result<()> {
    macos::cursor_restore(x, y)
}

/// Make `window_id`'s app **AppKit-active without raising it or switching Spaces**
/// (SkyLight focus-without-raise, the recipe cua-driver ports from yabai). A
/// backgrounded web canvas (e.g. a chart) only paints when its window is active,
/// so this lets it render before a capture — without foregrounding. Returns
/// `false` if the private SkyLight SPIs don't resolve (best-effort, no-op
/// fallback).
#[cfg(target_os = "macos")]
pub fn focus_without_raise(window_id: u32) -> bool {
    macos::focus_without_raise(window_id)
}

/// Click a **backgrounded / occluded** window's (web) content at a screen point
/// via SkyLight — trusted, no cursor move, no foreground. `window_origin` is the
/// window's top-left in screen points (for the window-local coordinate the gate
/// needs). Returns `false` if SkyLight is unavailable so the caller can fall back
/// to a cursor click.
#[cfg(target_os = "macos")]
pub fn click_web_background(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
) -> bool {
    macos::click_web_background(pid, window_id, x, y, origin_x, origin_y)
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS FFI ownership contract:
    //!
    //! CoreFoundation "create/copy" APIs return +1 owned references and are
    //! wrapped with `wrap_under_create_rule`. "get" APIs return borrowed
    //! references and are wrapped only for the current scope or explicitly
    //! retained before storage. `AxElement` always owns a +1 `AXUIElementRef`;
    //! cloning calls `CFRetain`, and `Drop` balances that retain with
    //! `CFRelease`.

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
        event::{
            CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField, KeyCode,
        },
        event_source::{CGEventSource, CGEventSourceStateID},
        geometry::{CGPoint, CGRect, CGSize},
    };
    use foreign_types::ForeignType;
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
            // SAFETY: `self.0` is a non-null AXUIElementRef owned by this
            // AxElement. CFRetain creates a second +1 reference for the clone.
            unsafe {
                core_foundation_sys::base::CFRetain(self.0 as CFTypeRef);
            }
            Self::from_owned(self.0)
        }
    }

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: AxElement owns exactly one +1 reference by contract.
                // Drop releases that ownership once.
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
            // SAFETY: `element` is a valid AXUIElementRef supplied by AX APIs
            // or retained by AxElement; the timeout value is finite.
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
        // SAFETY: AXIsProcessTrusted takes no pointers and returns a Boolean
        // process trust status.
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
        // SAFETY: AXUIElementCreateApplication is a create-rule AX API. The
        // returned pointer is null-checked and then owned by AxElement.
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
        // SAFETY: `element.as_ptr()` is a valid AXUIElementRef and
        // `window_id` is a valid out-parameter for the duration of the call.
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
            // SAFETY: `element` and the CFArray of attribute names are valid;
            // `values` is a valid out-parameter and is checked before wrapping.
            let err = unsafe {
                AXUIElementCopyMultipleAttributeValues(
                    element.as_ptr(),
                    attrs.request.as_concrete_TypeRef(),
                    0,
                    &mut values,
                )
            };
            if err == kAXErrorSuccess && !values.is_null() {
                // SAFETY: AXUIElementCopyMultipleAttributeValues follows the
                // create rule on success, transferring a +1 CFArray to Rust.
                let values = unsafe { CFArray::wrap_under_create_rule(values) };
                // SAFETY: `values` is a valid CFArray just wrapped above.
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
            // SAFETY: index is bounds-checked against the CFArray count; the
            // returned item is borrowed and used only during this call path.
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
        // SAFETY: `element` is a valid AXUIElementRef, and `action` is a valid
        // CFString for the duration of the AX call; the AXError is checked.
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

    fn set_string_attr_raw(element: &AxElement, attr: &str, value: &str) -> AXError {
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
        // SAFETY: `element` and `attr` are valid; `settable` is a valid
        // out-parameter and AXError is checked before reading it as true.
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

    pub fn click_at_point(pid: i32, x: f64, y: f64) -> Result<()> {
        click_at_point_impl(pid, x, y).map_err(ActionFailure::into)
    }

    fn click_at_point_impl(pid: i32, x: f64, y: f64) -> std::result::Result<(), ActionFailure> {
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

    fn hover_at_point_impl(_pid: i32, x: f64, y: f64) -> std::result::Result<(), ActionFailure> {
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
        let event = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|err| ActionFailure::Execution(format!("create hover CGEvent: {err:?}")))?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    pub fn cursor_borrow_to(x: f64, y: f64) -> Result<(f64, f64)> {
        cursor_borrow_to_impl(x, y).map_err(ActionFailure::into)
    }

    fn cursor_borrow_to_impl(x: f64, y: f64) -> std::result::Result<(f64, f64), ActionFailure> {
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

    fn cursor_restore_impl(x: f64, y: f64) -> std::result::Result<(), ActionFailure> {
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

    /// Best-effort bridge to SkyLight's private focus-without-raise SPIs, resolved
    /// once via `dlopen`/`dlsym` so a missing framework degrades to a no-op rather
    /// than a link failure. Recipe ported from cua-driver (itself from yabai's
    /// `window_manager_focus_window_without_raise`).
    mod skylight {
        use std::ffi::{c_char, c_int, c_void, CStr};
        use std::sync::OnceLock;

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Psn {
            high: u32,
            low: u32,
        }

        type GetFrontProcess = unsafe extern "C" fn(*mut Psn) -> i32;
        type MainConnectionId = unsafe extern "C" fn() -> u32;
        type GetWindowOwner = unsafe extern "C" fn(u32, u32, *mut u32) -> i32;
        type GetConnectionPsn = unsafe extern "C" fn(u32, *mut Psn) -> i32;
        type PostEventRecordTo = unsafe extern "C" fn(*const Psn, *const u8) -> i32;

        extern "C" {
            fn dlopen(path: *const c_char, flag: c_int) -> *mut c_void;
            fn dlsym(handle: *mut c_void, sym: *const c_char) -> *mut c_void;
        }

        struct Spi {
            get_front: GetFrontProcess,
            main_cid: MainConnectionId,
            window_owner: GetWindowOwner,
            conn_psn: GetConnectionPsn,
            post: PostEventRecordTo,
        }
        // SAFETY: the fields are C function pointers into a system framework,
        // read-only after one-time resolution; sharing them across threads is sound.
        unsafe impl Send for Spi {}
        // SAFETY: same as the `Send` impl above — immutable resolved fn pointers.
        unsafe impl Sync for Spi {}

        static SPI: OnceLock<Option<Spi>> = OnceLock::new();

        fn resolve() -> &'static Option<Spi> {
            SPI.get_or_init(|| {
                const RTLD_NOW: c_int = 2;
                let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
                // SAFETY: `path` is a valid NUL-terminated C string; dlopen returns
                // null on failure, which is checked.
                let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
                if handle.is_null() {
                    return None;
                }
                // SAFETY: `handle` is a live dlopen handle and each name is a valid
                // C string; dlsym returns null for missing symbols (checked below).
                let sym = |name: &CStr| unsafe { dlsym(handle, name.as_ptr()) };
                let get_front = sym(c"_SLPSGetFrontProcess");
                let main_cid = sym(c"SLSMainConnectionID");
                let window_owner = sym(c"SLSGetWindowOwner");
                let conn_psn = sym(c"SLSGetConnectionPSN");
                let post = sym(c"SLPSPostEventRecordTo");
                if get_front.is_null()
                    || main_cid.is_null()
                    || window_owner.is_null()
                    || conn_psn.is_null()
                    || post.is_null()
                {
                    return None;
                }
                // SAFETY: each pointer is a non-null symbol from SkyLight with the
                // documented C signature transmuted to the matching fn type.
                unsafe {
                    Some(Spi {
                        get_front: std::mem::transmute::<*mut c_void, GetFrontProcess>(get_front),
                        main_cid: std::mem::transmute::<*mut c_void, MainConnectionId>(main_cid),
                        window_owner: std::mem::transmute::<*mut c_void, GetWindowOwner>(
                            window_owner,
                        ),
                        conn_psn: std::mem::transmute::<*mut c_void, GetConnectionPsn>(conn_psn),
                        post: std::mem::transmute::<*mut c_void, PostEventRecordTo>(post),
                    })
                }
            })
        }

        pub fn focus_without_raise(window_id: u32) -> bool {
            let Some(spi) = resolve() else {
                return false;
            };
            // SAFETY: the fns are resolved SkyLight SPIs; `prev`/`target` are
            // correctly sized PSN out-parameters and `buf` is the 248-byte event
            // record the SLPSPostEventRecordTo recipe expects.
            unsafe {
                let mut prev = Psn { high: 0, low: 0 };
                if (spi.get_front)(&mut prev) != 0 {
                    return false;
                }
                let cid = (spi.main_cid)();
                let mut owner: u32 = 0;
                if (spi.window_owner)(cid, window_id, &mut owner) != 0 {
                    return false;
                }
                let mut target = Psn { high: 0, low: 0 };
                if (spi.conn_psn)(owner, &mut target) != 0 {
                    return false;
                }
                let mut buf = [0u8; 0xF8];
                buf[0x04] = 0xF8;
                buf[0x08] = 0x0D;
                buf[0x3C] = (window_id & 0xFF) as u8;
                buf[0x3D] = ((window_id >> 8) & 0xFF) as u8;
                buf[0x3E] = ((window_id >> 16) & 0xFF) as u8;
                buf[0x3F] = ((window_id >> 24) & 0xFF) as u8;
                buf[0x8A] = 0x02; // defocus the previous front
                let defocus = (spi.post)(&prev, buf.as_ptr());
                buf[0x8A] = 0x01; // focus the target window
                let focus = (spi.post)(&target, buf.as_ptr());
                defocus == 0 && focus == 0
            }
        }

        type EventPostToPid = unsafe extern "C" fn(c_int, *const c_void) -> c_int;
        static POST: OnceLock<Option<EventPostToPid>> = OnceLock::new();

        /// RTLD_DEFAULT on macOS = `(void*)-2`: dlsym then searches ALL loaded
        /// images (so a CoreGraphics private symbol resolves too, not just
        /// SkyLight's). We dlopen SkyLight first to make sure it is loaded.
        fn rtld_default_sym(name: &CStr) -> *mut c_void {
            let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
            // SAFETY: valid C string; result intentionally ignored (just loads it).
            unsafe { dlopen(path.as_ptr(), 2) };
            let rtld_default = (-2_isize) as *mut c_void;
            // SAFETY: RTLD_DEFAULT is the documented global search handle; `name`
            // is a valid C string.
            unsafe { dlsym(rtld_default, name.as_ptr()) }
        }

        fn post_fn() -> Option<EventPostToPid> {
            *POST.get_or_init(|| {
                let p = rtld_default_sym(c"SLEventPostToPid");
                if p.is_null() {
                    None
                } else {
                    // SAFETY: non-null SkyLight symbol with the documented C ABI.
                    Some(unsafe { std::mem::transmute::<*mut c_void, EventPostToPid>(p) })
                }
            })
        }

        /// Whether `SLEventPostToPid` resolved (the background mouse path is live).
        pub fn mouse_post_available() -> bool {
            post_fn().is_some()
        }

        /// Post a CGEvent (raw `CGEventRef`) to `pid` via SkyLight, reaching a
        /// backgrounded window's (web) content. Mouse events carry NO auth message
        /// (per cua-driver: it diverts them off the IOHID pipeline Chromium reads).
        pub fn post_event_to_pid(pid: i32, event: *const c_void) {
            if let Some(f) = post_fn() {
                // SAFETY: `f` is SLEventPostToPid; `event` is a live CGEventRef.
                unsafe { f(pid, event) };
            }
        }

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct CgPoint {
            x: f64,
            y: f64,
        }
        type SetWindowLocation = unsafe extern "C" fn(*const c_void, CgPoint);
        static SET_WIN_LOC: OnceLock<Option<SetWindowLocation>> = OnceLock::new();

        fn set_win_loc_fn() -> Option<SetWindowLocation> {
            *SET_WIN_LOC.get_or_init(|| {
                // CGEventSetWindowLocation lives in CoreGraphics, not SkyLight — so
                // resolve it via the global RTLD_DEFAULT search.
                let p = rtld_default_sym(c"CGEventSetWindowLocation");
                if p.is_null() {
                    None
                } else {
                    // SAFETY: non-null symbol; documented ABI (CGEventRef, CGPoint).
                    Some(unsafe { std::mem::transmute::<*mut c_void, SetWindowLocation>(p) })
                }
            })
        }

        /// Stamp the window-LOCAL coordinate onto a CGEvent (private SPI). No-op if
        /// the symbol is unavailable.
        pub fn set_window_location(event: *const c_void, x: f64, y: f64) {
            if let Some(f) = set_win_loc_fn() {
                // SAFETY: `f` is CGEventSetWindowLocation; `event` is a live ref.
                unsafe { f(event, CgPoint { x, y }) };
            }
        }
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
    ) -> bool {
        if window_id == 0 || !skylight::mouse_post_available() {
            return false;
        }
        focus_without_raise(window_id);
        thread::sleep(Duration::from_millis(50));

        let screen = CGPoint::new(sx, sy);
        let local = CGPoint::new(sx - ox, sy - oy);
        let off = CGPoint::new(-1.0, -1.0);
        let make = |etype: CGEventType, point: CGPoint, win_local: CGPoint| -> Option<CGEvent> {
            let source = event_source("skylight click CGEventSource").ok()?;
            let event = CGEvent::new_mouse_event(source, etype, point, CGMouseButton::Left).ok()?;
            event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, 0);
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

        if let Some(m) = make(CGEventType::MouseMoved, screen, local) {
            post(&m);
        }
        thread::sleep(Duration::from_millis(15));
        if let Some(d) = make(CGEventType::LeftMouseDown, off, off) {
            post(&d);
        }
        thread::sleep(Duration::from_millis(1));
        if let Some(u) = make(CGEventType::LeftMouseUp, off, off) {
            post(&u);
        }
        thread::sleep(Duration::from_millis(100));
        if let Some(d) = make(CGEventType::LeftMouseDown, screen, local) {
            post(&d);
        }
        thread::sleep(Duration::from_millis(1));
        if let Some(u) = make(CGEventType::LeftMouseUp, screen, local) {
            post(&u);
        }
        true
    }

    pub fn press_key(pid: i32, key: &str) -> Result<()> {
        press_key_impl(pid, key).map_err(ActionFailure::into)
    }

    fn press_key_impl(pid: i32, key: &str) -> std::result::Result<(), ActionFailure> {
        let keycode = named_keycode(key)?;
        let source = event_source("create keyboard CGEventSource")?;
        post_keycode(source, pid, keycode)
    }

    fn named_keycode(key: &str) -> std::result::Result<CGKeyCode, ActionFailure> {
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
            other => Err(ActionFailure::Execution(format!(
                "unsupported key {other:?}; expected return|enter, tab, escape, space, delete, up/down/left/right"
            ))),
        }
    }

    fn post_keycode(
        source: CGEventSource,
        pid: i32,
        keycode: CGKeyCode,
    ) -> std::result::Result<(), ActionFailure> {
        let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
            .map_err(|err| ActionFailure::Execution(format!("create key down CGEvent: {err:?}")))?;
        down.post_to_pid(pid);

        let up = CGEvent::new_keyboard_event(source, keycode, false)
            .map_err(|err| ActionFailure::Execution(format!("create key up CGEvent: {err:?}")))?;
        up.post_to_pid(pid);
        Ok(())
    }

    /// Union of every active display's bounds (the full virtual desktop). Cursor
    /// warps and CGEvents use the GLOBAL coordinate space, so clamping to the main
    /// display alone would pin a point on a secondary monitor to the main display's
    /// edge — which is exactly where a window on an external screen lives.
    fn all_displays_bounds() -> CGRect {
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

    fn read_node_actions(element: &AxElement) -> Vec<String> {
        // PERF: this remains one AX round-trip per node; kAXActions can be folded
        // into the batch request if the FFI constant is introduced locally.
        action_names(element)
    }

    fn action_names(element: &AxElement) -> Vec<String> {
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

    fn cf_strings(array: &CFArray) -> Vec<String> {
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

    fn ax_elements(array: &CFArray) -> Vec<AxElement> {
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

    fn cgpoint_from_ax_value(value: AXValueRef) -> Option<CGPoint> {
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

    fn cgsize_from_ax_value(value: AXValueRef) -> Option<CGSize> {
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

    fn cf_string(value: CFTypeRef) -> Option<String> {
        // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
        // retains it for this temporary wrapper.
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast::<CFString>().map(|s| s.to_string())
    }

    fn cf_label_string(value: CFTypeRef) -> Option<String> {
        cf_string(value).filter(|s| !s.is_empty())
    }

    fn cf_bool(value: CFTypeRef) -> Option<bool> {
        // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
        // retains it for this temporary wrapper.
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast::<CFBoolean>().map(bool::from)
    }

    fn cf_array(value: CFTypeRef) -> Option<CFArray> {
        // SAFETY: caller passes a valid borrowed CF object; wrap_under_get_rule
        // retains it for this temporary wrapper.
        let value = unsafe { CFType::wrap_under_get_rule(value) };
        value.downcast_into::<CFArray>()
    }

    fn ax_value_ref(value: CFTypeRef) -> Option<AXValueRef> {
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

    fn normalize_batch_value(value: CFTypeRef) -> Option<CFTypeRef> {
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
