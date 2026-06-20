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
    kAXErrorCannotComplete, kAXErrorIllegalArgument, kAXErrorInvalidUIElement, kAXErrorNoValue,
    kAXErrorSuccess, kAXFocusedAttribute, kAXHelpAttribute, kAXIdentifierAttribute,
    kAXMainWindowAttribute, kAXMaxValueAttribute, kAXMenuBarAttribute, kAXMinValueAttribute,
    kAXNumberOfCharactersAttribute, kAXParentAttribute, kAXPositionAttribute, kAXPressAction,
    kAXRaiseAction, kAXRoleAttribute, kAXSelectedTextRangeAttribute, kAXShowMenuAction,
    kAXSizeAttribute, kAXTitleAttribute, kAXValueAttribute, kAXValueIncrementAttribute,
    kAXValueTypeAXError, kAXValueTypeCFRange, kAXValueTypeCGPoint, kAXValueTypeCGRect,
    kAXValueTypeCGSize, kAXVerticalScrollBarAttribute, kAXWindowsAttribute, AXError,
    AXIsProcessTrusted, AXUIElementCopyActionNames, AXUIElementCopyAttributeValue,
    AXUIElementCopyElementAtPosition, AXUIElementCopyMultipleAttributeValues,
    AXUIElementCreateApplication, AXUIElementIsAttributeSettable, AXUIElementPerformAction,
    AXUIElementRef, AXUIElementSetAttributeValue, AXValueGetType, AXValueGetValue, AXValueRef,
};
use core_foundation::{
    array::CFArray,
    base::{CFGetTypeID, CFRelease, CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFIndex, CFNullGetTypeID, CFRange};
use core_graphics::{
    display::CGDisplay,
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
        EventField, KeyCode,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::{CGPoint, CGRect, CGSize},
};
use dunst_core::{
    Bbox, RawAxNode, Result, Role, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
};
use foreign_types::ForeignType;
use objc2::msg_send;
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;

const DEFAULT_MAX_NODES: usize = 5_000;
const DEFAULT_MAX_DEPTH: usize = 40;
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
const DEFAULT_USER_IDLE_GUARD_MS: u64 = 300;
const TYPE_SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(80);
const TYPE_SETTLE_BASE_MS: u64 = 300;
const TYPE_SETTLE_PER_CHAR_MS: u64 = 12;
const TYPE_SETTLE_MAX_MS: u64 = 10_000;
const RETURN_KEYCODE: CGKeyCode = 36;

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

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(
        state_id: CGEventSourceStateID,
        event_type: CGEventType,
    ) -> f64;
}

fn set_ax_timeout(element: AXUIElementRef) {
    if !element.is_null() {
        // SAFETY: `element` is a valid AXUIElementRef supplied by AX APIs
        // or retained by AxElement; the timeout value is finite.
        unsafe {
            accessibility_sys::AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECS);
        }
    }
}

fn max_nodes() -> usize {
    env_usize("DUNST_AX_MAX_NODES", DEFAULT_MAX_NODES).max(100)
}

fn max_depth() -> usize {
    env_usize("DUNST_AX_MAX_DEPTH", DEFAULT_MAX_DEPTH).max(4)
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn user_idle_guard_ms() -> u64 {
    env_u64("DUNST_MCP_USER_IDLE_GUARD_MS", DEFAULT_USER_IDLE_GUARD_MS)
}

fn last_input_age_ms() -> Option<u64> {
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

fn user_idle_block_message(operation: &str) -> Option<String> {
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

fn ensure_user_idle(operation: &str) -> Result<()> {
    if let Some(message) = user_idle_block_message(operation) {
        return Err(VisualOpsError::Execution(message));
    }
    Ok(())
}

fn ensure_user_idle_action(operation: &str) -> std::result::Result<(), ActionFailure> {
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
        window_id: target.window_id,
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
        return Err(VisualOpsError::Perception(format!(
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

fn perform_with_cache(
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
        VisualOpsError::ElementNotFound(format!(
            "role={:?} label={:?} identifier={:?}",
            node.ax_role, node.label, node.ax_identifier
        ))
    })?;
    perform_on_element(Some(&element), target, node, action, argument).map_err(ActionFailure::into)
}

fn perform_on_element(
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

fn require_ax_element(
    element: Option<&AxElement>,
) -> std::result::Result<&AxElement, ActionFailure> {
    element.ok_or_else(|| ActionFailure::Execution("action requires a resolved AX element".into()))
}

fn click_element_action(
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

fn target_window_origin(target: &Target) -> std::result::Result<(f64, f64), ActionFailure> {
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
    set_cgsize_attr(&window, kAXSizeAttribute, size).map_err(VisualOpsError::from)?;
    set_cgpoint_attr(&window, kAXPositionAttribute, CGPoint::new(x, y))
        .map_err(VisualOpsError::from)
}

fn ensure_trusted() -> Result<()> {
    if accessibility_trusted() {
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
            "dunst-platform: window id {requested_window_id} not found; using first AX window"
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
            CFArrayGetValueAtIndex(self.values.as_concrete_TypeRef(), index as isize) as CFTypeRef
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

fn shallow_raw_node(element: &AxElement) -> RawAxNode {
    let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
    assemble_node(NodeFields {
        ax_role,
        value: attr_value_string(element, kAXValueAttribute),
        title: attr_label_string(element, kAXTitleAttribute),
        description: attr_label_string(element, kAXDescriptionAttribute),
        help: attr_string(element, kAXHelpAttribute),
        ax_identifier: attr_string(element, kAXIdentifierAttribute),
        ax_actions: read_node_actions(element),
        frame: frame(element),
        enabled: attr_bool(element, kAXEnabledAttribute).unwrap_or(true),
        focused: attr_bool(element, kAXFocusedAttribute).unwrap_or(false),
    })
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
        value: batch.get(IDX_VALUE).and_then(cf_value_string),
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

    if depth >= max_depth() || state.count >= max_nodes() {
        state.capped = true;
        return Ok(node);
    }

    if let Some(children) = children {
        for child in ax_elements(&children) {
            if state.count >= max_nodes() {
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
    let has_path = !wanted.path.is_empty();
    let require_path = has_path && collision_suffix_id(&wanted.id);
    let mut path_mismatch = false;
    let mut stack = vec![(root, 0usize, vec![0usize])];
    let mut seen = 0usize;

    while let Some((element, depth, path)) = stack.pop() {
        seen += 1;
        if seen > max_nodes() || depth > max_depth() {
            eprintln!("dunst-platform: live element search capped");
            break;
        }

        if has_path && path == wanted.path {
            if element_matches(&element, wanted) {
                return Ok(Some(element));
            }
            path_mismatch = true;
            if require_path {
                break;
            }
        }

        if !require_path && element_matches(&element, wanted) {
            return Ok(Some(element));
        }

        if let Some(children) = attr_array(&element, kAXChildrenAttribute) {
            let child_elements = ax_elements(&children);
            for (idx, child) in child_elements.into_iter().enumerate().rev() {
                let mut child_path = path.clone();
                child_path.push(idx);
                stack.push((child, depth + 1, child_path));
            }
        }
    }

    if require_path && path_mismatch {
        return Err(VisualOpsError::ElementNotFound(format!(
            "id={} path={:?} resolved to a different live AX element",
            wanted.id, wanted.path
        )));
    }
    Ok(None)
}

fn element_matches(element: &AxElement, wanted: &SceneNode) -> bool {
    element_key(element)
        .map(|key| key == ElementKey::from_scene(wanted))
        .unwrap_or(false)
}

fn collision_suffix_id(id: &str) -> bool {
    let Some((_, suffix)) = id.rsplit_once('_') else {
        return false;
    };
    suffix.len() <= 3 && suffix.parse::<u32>().map(|n| n >= 2).unwrap_or(false)
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

fn perform_ax_action(element: &AxElement, action: &str) -> std::result::Result<(), ActionFailure> {
    let action = CFString::new(action);
    // SAFETY: `element` is a valid AXUIElementRef, and `action` is a valid
    // CFString for the duration of the AX call; the AXError is checked.
    let err = unsafe { AXUIElementPerformAction(element.as_ptr(), action.as_concrete_TypeRef()) };
    ax_action_result(err, "perform AX action")
}

fn scroll_element(
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

fn parse_scroll_argument(argument: Option<&str>) -> (&str, usize) {
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

fn find_vertical_scrollbar(element: &AxElement) -> Option<AxElement> {
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

fn retain_ax_element(element: &AxElement) -> Option<AxElement> {
    Some(element.retain_clone())
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

fn set_number_attr(
    element: &AxElement,
    attr: &str,
    value: f64,
) -> std::result::Result<(), ActionFailure> {
    ax_action_result(
        set_number_attr_raw(element, attr, value),
        "set AX number attribute",
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

fn set_number_attr_raw(element: &AxElement, attr: &str, value: f64) -> AXError {
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

fn set_cgpoint_attr(
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

fn set_cgsize_attr(
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

fn set_axvalue_attr_raw(
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
    target: &Target,
    text: &str,
) -> std::result::Result<(), ActionFailure> {
    if let Some(outcome) = type_text_by_replacing_selection(element, target, text)? {
        return outcome;
    }

    if attr_settable(element, kAXValueAttribute) {
        let before = attr_string(element, kAXValueAttribute);
        let err = set_string_attr_raw(element, kAXValueAttribute, text);
        if err == kAXErrorSuccess {
            match wait_for_string_attr(element, kAXValueAttribute, text) {
                Some(value) if value == text => return Ok(()),
                Some(value) if before.as_deref() != Some(value.as_str()) => {
                    return Err(ActionFailure::Execution(format!(
                        "AX set-value changed the field but did not produce the requested value: expected {} chars, observed {} chars",
                        text.chars().count(),
                        value.chars().count()
                    )));
                }
                Some(_) => {}
                None => return Ok(()),
            }
            return Err(ActionFailure::Execution(
                "AX set-value reported success but the field did not change; keyboard fallback suppressed to avoid appending text".into(),
            ));
        } else if is_stale_ax_error(err) {
            return Err(ActionFailure::Ax {
                operation: "set AX string attribute",
                err,
            });
        }
    }

    // AX set-value replaces text; synthetic Unicode keystrokes append to the focused editor.
    set_bool_attr(element, kAXFocusedAttribute, true)?;
    std::thread::sleep(std::time::Duration::from_millis(80));
    post_window_bound_text(target, text)
}

fn type_text_by_replacing_selection(
    element: &AxElement,
    target: &Target,
    text: &str,
) -> std::result::Result<Option<std::result::Result<(), ActionFailure>>, ActionFailure> {
    let Some(len) = text_character_count(element) else {
        return Ok(None);
    };
    if !attr_settable(element, kAXSelectedTextRangeAttribute) {
        return Ok(None);
    }

    let before = attr_string(element, kAXValueAttribute);
    set_bool_attr(element, kAXFocusedAttribute, true)?;
    let range = CFRange::init(0, len);
    let err = set_axvalue_attr_raw(
        element,
        kAXSelectedTextRangeAttribute,
        kAXValueTypeCFRange,
        (&range as *const CFRange).cast(),
    );
    if err == kAXErrorIllegalArgument || err == kAXErrorNoValue {
        return Ok(None);
    }
    if err != kAXErrorSuccess {
        return Err(ActionFailure::Ax {
            operation: "set AX selected text range",
            err,
        });
    }

    std::thread::sleep(std::time::Duration::from_millis(80));
    post_window_bound_text(target, text)?;
    Ok(Some(match wait_for_string_attr(element, kAXValueAttribute, text) {
        Some(value) if value == text => Ok(()),
        Some(value)
            if before
                .as_deref()
                .map(|before| value == format!("{before}{text}") || value == format!("{text}{before}"))
                .unwrap_or(false) =>
        {
            Err(ActionFailure::Execution(
                "keyboard replacement appended instead of replacing; selected text range was not honored".into(),
            ))
        }
        Some(value) if before.as_deref() != Some(value.as_str()) => {
            Err(ActionFailure::Execution(format!(
                "keyboard replacement changed the field but did not produce the requested value: expected {} chars, observed {} chars",
                text.chars().count(),
                value.chars().count()
            )))
        }
        Some(_) => Err(ActionFailure::Execution(
            "keyboard replacement posted but the field did not change".into(),
        )),
        None => Ok(()),
    }))
}

fn post_window_bound_text(target: &Target, text: &str) -> std::result::Result<(), ActionFailure> {
    if target.window_id == 0 {
        return Err(ActionFailure::Execution(
            "element-bound typing requires a target window id; process-wide keyboard fallback suppressed".into(),
        ));
    }
    type_text_background_impl(target.pid, target.window_id, text)
}

fn wait_for_string_attr(element: &AxElement, attr: &str, expected: &str) -> Option<String> {
    let timeout = type_settle_timeout(expected);
    let started = Instant::now();
    let mut last = attr_string(element, attr)?;
    if last == expected {
        return Some(last);
    }
    loop {
        if started.elapsed() >= timeout {
            return Some(last);
        }
        std::thread::sleep(TYPE_SETTLE_POLL_INTERVAL);
        match attr_string(element, attr) {
            Some(value) if value == expected => return Some(value),
            Some(value) => last = value,
            None => return None,
        }
    }
}

fn type_settle_timeout(text: &str) -> Duration {
    let chars = text.chars().count() as u64;
    Duration::from_millis(
        (TYPE_SETTLE_BASE_MS + chars * TYPE_SETTLE_PER_CHAR_MS).min(TYPE_SETTLE_MAX_MS),
    )
}

fn text_character_count(element: &AxElement) -> Option<CFIndex> {
    attr_number(element, kAXNumberOfCharactersAttribute)
        .map(|n| n.max(0.0) as CFIndex)
        .or_else(|| {
            attr_string(element, kAXValueAttribute).map(|value| value.chars().count() as CFIndex)
        })
}

fn attr_settable(element: &AxElement, attr: &str) -> bool {
    let attr = CFString::new(attr);
    let mut settable: c_uchar = 0;
    // SAFETY: `element` and `attr` are valid; `settable` is a valid
    // out-parameter and AXError is checked before reading it as true.
    let err = unsafe {
        AXUIElementIsAttributeSettable(element.as_ptr(), attr.as_concrete_TypeRef(), &mut settable)
    };
    err == kAXErrorSuccess && settable != 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextInputAtom {
    Char(char),
    Return,
}

fn for_text_input_atoms<F>(text: &str, mut f: F) -> std::result::Result<(), ActionFailure>
where
    F: FnMut(TextInputAtom) -> std::result::Result<(), ActionFailure>,
{
    let mut previous_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\r' => {
                f(TextInputAtom::Return)?;
                previous_was_cr = true;
            }
            '\n' if previous_was_cr => {
                previous_was_cr = false;
            }
            '\n' => {
                f(TextInputAtom::Return)?;
                previous_was_cr = false;
            }
            ch => {
                f(TextInputAtom::Char(ch))?;
                previous_was_cr = false;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{for_text_input_atoms, ActionFailure, TextInputAtom};

    #[test]
    fn text_input_atoms_map_line_endings_to_return_keypresses() {
        let mut atoms = Vec::new();
        let result = for_text_input_atoms("a\nb\r\nc\rd", |atom| {
            atoms.push(atom);
            Ok::<_, ActionFailure>(())
        });
        assert!(result.is_ok());

        assert_eq!(
            atoms,
            vec![
                TextInputAtom::Char('a'),
                TextInputAtom::Return,
                TextInputAtom::Char('b'),
                TextInputAtom::Return,
                TextInputAtom::Char('c'),
                TextInputAtom::Return,
                TextInputAtom::Char('d'),
            ]
        );
    }
}

fn hover(pid: i32, node: &SceneNode) -> std::result::Result<(), ActionFailure> {
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

fn drag(
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

fn hover_at_point_impl(_pid: i32, x: f64, y: f64) -> std::result::Result<(), ActionFailure> {
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

fn cursor_borrow_to_impl(x: f64, y: f64) -> std::result::Result<(f64, f64), ActionFailure> {
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
                    window_owner: std::mem::transmute::<*mut c_void, GetWindowOwner>(window_owner),
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
    pub fn post_event_to_pid(pid: i32, event: *const c_void) -> bool {
        let Some(f) = post_fn() else {
            return false;
        };
        // SAFETY: `f` is SLEventPostToPid; `event` is a live CGEventRef.
        unsafe { f(pid, event) };
        true
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

    type SetAuthMessage = unsafe extern "C" fn(*const c_void, *mut c_void);
    static SET_AUTH: OnceLock<Option<SetAuthMessage>> = OnceLock::new();

    fn set_auth_fn() -> Option<SetAuthMessage> {
        *SET_AUTH.get_or_init(|| {
            let p = rtld_default_sym(c"SLEventSetAuthenticationMessage");
            if p.is_null() {
                None
            } else {
                // SAFETY: non-null SkyLight symbol; documented C ABI.
                Some(unsafe { std::mem::transmute::<*mut c_void, SetAuthMessage>(p) })
            }
        })
    }

    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    type Factory =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32, u32) -> *mut c_void;

    /// The SkyLight event record lives at a pointer slot inside the opaque
    /// CGEvent struct (offset 24/32/16, per cua-driver); return the first
    /// non-null. Needed to build the auth message.
    fn extract_event_record(event: *const c_void) -> *mut c_void {
        for off in [24usize, 32, 16] {
            // SAFETY: reads a pointer-sized slot inside the live CGEvent struct,
            // a documented (private) layout; non-null is checked by the caller.
            let p = unsafe { *((event as *const u8).add(off) as *const *mut c_void) };
            if !p.is_null() {
                return p;
            }
        }
        std::ptr::null_mut()
    }

    /// Attach the `SLSEventAuthenticationMessage` envelope macOS 14+ requires
    /// for a synthetic KEYBOARD event to be accepted as trusted by web content
    /// (Chromium). No-op (unsigned post) if anything fails to resolve.
    pub fn attach_auth_message(event: *const c_void, pid: i32) {
        let Some(set_auth) = set_auth_fn() else {
            return;
        };
        let record = extract_event_record(event);
        if record.is_null() {
            return;
        }
        // SAFETY: standard ObjC runtime lookups with valid C strings.
        let class = unsafe { objc_getClass(c"SLSEventAuthenticationMessage".as_ptr()) };
        if class.is_null() {
            return;
        }
        // SAFETY: valid selector string.
        let sel = unsafe { sel_registerName(c"messageWithEventRecord:pid:version:".as_ptr()) };
        // SAFETY: objc_msgSend re-typed to the factory's concrete signature
        // `(Class, SEL, SLSEventRecord*, int32, uint32) -> id`.
        let factory: Factory = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        // SAFETY: invokes the class factory; record/class/sel are valid.
        let msg = unsafe { factory(class, sel, record, pid, 0) };
        if !msg.is_null() {
            // SAFETY: `set_auth` is SLEventSetAuthenticationMessage; args valid.
            unsafe { set_auth(event, msg) };
        }
    }
}

extern "C" {
    /// Public CoreGraphics: set a CGEvent's screen location.
    fn CGEventSetLocation(event: *const c_void, location: CGPoint);
}

/// Build a CGEvent through the **NSEvent bridge** so it carries the
/// `windowNumber` routing Chromium's user-activation gate latches onto (the
/// plain CGEvent path is dropped). Returns a +1-owned CGEvent.
fn cg_event_via_nsevent(etype: CGEventType, click_count: isize, window_id: u32) -> Option<CGEvent> {
    let ns_type = match etype {
        CGEventType::LeftMouseDown => NSEventType::LeftMouseDown,
        CGEventType::LeftMouseUp => NSEventType::LeftMouseUp,
        CGEventType::RightMouseDown => NSEventType::RightMouseDown,
        CGEventType::RightMouseUp => NSEventType::RightMouseUp,
        CGEventType::MouseMoved => NSEventType::MouseMoved,
        _ => return None,
    };
    // SAFETY: standard AppKit class-method; a nil graphics context is allowed
    // and all scalar arguments are valid.
    let ns = unsafe {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            ns_type,
            NSPoint { x: 0.0, y: 0.0 },
            NSEventModifierFlags(0),
            0.0,
            window_id as isize,
            None,
            0,
            click_count,
            1.0,
        )
    }?;
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

fn hover_web_background_impl(
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

fn type_text_background_impl(
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

fn post_background_unicode_char(pid: i32, ch: char) -> std::result::Result<(), ActionFailure> {
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

fn post_background_keycode_pair(
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

fn post_background_key_event(pid: i32, event: &CGEvent) -> std::result::Result<(), ActionFailure> {
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

fn current_cursor_position(source: &CGEventSource) -> std::result::Result<CGPoint, ActionFailure> {
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

fn attr_value_string(element: &AxElement, attr: &str) -> Option<String> {
    let value = attr_value(element, attr)?;
    cf_value_string(value.as_CFTypeRef())
}

fn attr_label_string(element: &AxElement, attr: &str) -> Option<String> {
    attr_string(element, attr).filter(|s| !s.is_empty())
}

fn attr_bool(element: &AxElement, attr: &str) -> Option<bool> {
    let value = attr_value(element, attr)?;
    cf_bool(value.as_CFTypeRef())
}

fn attr_number(element: &AxElement, attr: &str) -> Option<f64> {
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

fn cf_value_string(value: CFTypeRef) -> Option<String> {
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
