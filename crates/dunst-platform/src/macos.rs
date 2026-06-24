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
    access::ScreenCaptureAccess,
    display::CGDisplay,
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
        EventField, KeyCode, ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::{CGPoint, CGRect, CGSize},
};
use dunst_core::{
    Bbox, DunstError, RawAxNode, Result, Role, SceneNode, SemanticAction, Target, WindowRef,
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
const DEFAULT_USER_IDLE_GUARD_MS: u64 = 150;
const TYPE_SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(80);

pub fn screen_capture_trusted() -> bool {
    ScreenCaptureAccess.preflight()
}

const TYPE_SETTLE_BASE_MS: u64 = 300;
const TYPE_SETTLE_PER_CHAR_MS: u64 = 12;
const TYPE_SETTLE_MAX_MS: u64 = 10_000;
const RETURN_KEYCODE: CGKeyCode = 36;

mod ax_actions;
mod ax_backend;
mod ax_tree;
mod cf;
mod pointer_events;
mod skylight;
mod text_input;
mod web_events;

use ax_actions::*;
use ax_backend::*;
pub(crate) use ax_backend::{
    accessibility_trusted, capture, element_at_point, perform, set_focused_field_text,
    set_window_frame, window_ref,
};
use ax_tree::*;
use cf::*;
use pointer_events::*;
pub(crate) use pointer_events::{
    click_at_point, cursor_borrow_move_to, cursor_borrow_to, cursor_restore, focus_without_raise,
    hover_at_point, right_click_at_point, scroll_at_point, unstick_cursor,
};
use text_input::*;
use web_events::*;
pub(crate) use web_events::{
    click_web_background, hover_web_background, key_web_background, press_key,
    scroll_web_background, type_text_background,
};

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
