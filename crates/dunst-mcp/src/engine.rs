//! The Dunst engine — the runtime-agnostic service behind the MCP tools.
//!
//! Holds a [`Perceptor`] + [`ActionExecutor`] + [`RiskEngine`], maintains the
//! current/previous [`SceneGraph`] and [`AffordanceGraph`], enforces
//! risk-based approval gating, and records an [`AuditEntry`] per action.
//!
//! This struct is transport-independent: the MCP server (`serve`) and the CLI
//! `demo` both drive the same methods.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use dunst_core::{
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, Bbox, DunstError, GraphDiff,
    NodeChange, Perceptor, RiskAssessment, RiskLevel, Role, SceneGraph, SceneNode, SemanticAction,
    SessionIdentity, Target, WindowRef,
};
use dunst_graph::{audit, derive_affordances, scene, RiskEngine};
use serde_json::{json, Value};

mod action;
mod action_resolution;
mod app_ops;
mod apps;
mod browser_query;
mod chart;
mod choices;
mod element_actions;
mod file_select;
mod input;
mod ocr_read;
mod query_support;
mod raw_input;
mod raw_input_gate;
mod read;
mod runtime_support;
mod scene_query;
mod selections;
mod types;
mod window_geometry;
mod window_ops;

#[cfg(test)]
use action::typed_target_value_matches_expected;
use action::CoTarget;
#[cfg(test)]
use apps::launchable_app_from_info_json;
#[cfg(target_os = "macos")]
use apps::{app_search_roots, collect_app_bundles, launchable_app_from_bundle};
use browser_query::*;
use chart::{build_y_calibration, nearest_time_label, region_from_axis};
#[cfg(test)]
use chart::{is_axis_token, looks_like_clock, parse_value};
#[cfg(test)]
use input::char_keycode;
use input::{is_press_key_name, layout_sensitive_hotkey_message, parse_combo};
#[cfg(any(target_os = "macos", test))]
use input::{menu_hotkey_matches, menu_hotkey_matches_virtual_key, parse_menu_hotkey_combo};
use query_support::*;
use raw_input::page_scroll_target_id;
use raw_input_gate::{
    is_synthetic_approval_target_id, raw_apply_selections_target_id, raw_paste_text_target_id,
    raw_press_key_target_id, raw_set_field_text_target_id, raw_type_keys_target_id,
    RawApprovalInflight, RawApprovalKey,
};
use runtime_support::*;
pub(crate) use runtime_support::{panic_payload_message, truncate_chars};
use scene_query::*;
use window_geometry::*;

pub use choices::*;
pub use selections::*;
pub use types::*;

const READ_REFRESH_TTL: Duration = Duration::from_millis(500);
const DISPLAY_CACHE_TTL: Duration = Duration::from_millis(1_000);
const OCR_CACHE_TTL: Duration = Duration::from_millis(250);
const SCREENSHOT_CACHE_TTL: Duration = Duration::from_millis(250);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Engine {
    perceptor: Box<dyn Perceptor>,
    executor: Box<dyn ActionExecutor>,
    risk: RiskEngine,
    target: Target,
    window: WindowRef,
    current: Option<SceneGraph>,
    previous: Option<SceneGraph>,
    affordances: Option<AffordanceGraph>,
    /// Element IDs that have been explicitly approved for high-risk actions.
    approvals: BTreeSet<String>,
    /// Synthetic raw-input grants keyed by a scoped target id. Unlike
    /// element-bound approvals, these are count-limited instead of consumed by
    /// every refresh, so repeated keypresses can complete without approval
    /// churn after the operator has approved that exact raw input family.
    raw_approvals: BTreeMap<RawApprovalKey, RawApprovalGrant>,
    /// Raw approval grant consumed by an in-flight raw action. If the platform
    /// rejects the action only because the operator is active, the grant is
    /// restored so the automatic retry path does not ask for approval again.
    raw_approval_inflight: BTreeMap<String, RawApprovalInflight>,
    /// Bounded synthetic approval context for a single `apply_selections` call
    /// or internal survey-scroll sweep. Existing element/raw gates consult this
    /// to avoid re-prompting for each constituent action.
    active_batch: Option<BatchApprovalContext>,
    /// IDs currently awaiting approval — the gated participants of the actions that
    /// returned `PendingApproval` since the last refresh. Lets [`approve`](Self::approve)
    /// accept an element whose danger is *contextual* (a destructive value typed into
    /// an otherwise low-risk field, audit #13), without loosening the rule that a
    /// plain low-risk id can't be approved.
    pending_gate_ids: BTreeSet<String>,
    /// Memoised at [`refresh`](Self::refresh) (audit #9): the window rect and the
    /// menubar-root id, so the per-listing latent filter doesn't re-scan every node
    /// on each call.
    cached_window_rect: Option<Bbox>,
    cached_menubar_root: Option<String>,
    last_refresh_at: Option<Instant>,
    display_cache: RefCell<Option<TimedCache<Vec<DisplaySummary>>>>,
    desktop_cache: RefCell<Option<TimedCache<DesktopCacheEntry>>>,
    ocr_cache: RefCell<Option<TimedCache<OcrCacheEntry>>>,
    screenshot_cache: RefCell<Option<TimedCache<ScreenshotCacheEntry>>>,
    visual_probe_cache: RefCell<Option<VisualProbeCacheEntry>>,
    scroll_strategy_cache: BTreeMap<ScrollStrategyKey, ScrollStrategyMemory>,
    scroll_background_low_signal: BTreeSet<ScrollStrategyKey>,
    /// Caller-installed, bounded pre-authorization for raw input in the attached
    /// window. While live, gated raw actions execute without a per-action
    /// `approve` round-trip, spending a budget until it or the TTL runs out.
    /// Scoped to a `window_id`, so re-attaching a different window drops it; it
    /// survives `refresh` (a flow spans several perceptions) but never a re-attach.
    raw_preauth: Option<RawPreauthorization>,
    session_identity: Option<SessionIdentity>,
    trace: Vec<AuditEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ScrollStrategyKey {
    app: String,
    page: String,
}

/// Session-learned memory that the real-cursor wheel scroll worked for a
/// given `(app, page)` scope. Presence of an entry in `scroll_strategy_cache`
/// means "remember the real-cursor wheel fallback here"; there is currently
/// only one strategy worth remembering, so this struct no longer tags itself
/// with a strategy enum.
#[derive(Clone, Debug)]
struct ScrollStrategyMemory {
    point_ratio: Option<(f64, f64)>,
}

impl Engine {
    /// Builds an engine bound to `target`, resolving its window and running the
    /// first perception.
    ///
    /// # Errors
    ///
    /// Returns an error if the target's window cannot be resolved, or if the
    /// initial [`refresh`](Self::refresh) fails to capture the scene.
    pub fn new(
        perceptor: Box<dyn Perceptor>,
        executor: Box<dyn ActionExecutor>,
        mut target: Target,
    ) -> dunst_core::Result<Self> {
        let window = perceptor.window_ref(&target)?;
        if target.window_id == 0 && window.window_id != 0 {
            target.window_id = window.window_id;
        }
        let mut e = Engine {
            perceptor,
            executor,
            risk: RiskEngine::new(),
            target,
            window,
            current: None,
            previous: None,
            affordances: None,
            approvals: BTreeSet::new(),
            raw_approvals: BTreeMap::new(),
            raw_approval_inflight: BTreeMap::new(),
            active_batch: None,
            pending_gate_ids: BTreeSet::new(),
            cached_window_rect: None,
            cached_menubar_root: None,
            last_refresh_at: None,
            display_cache: RefCell::new(None),
            desktop_cache: RefCell::new(None),
            ocr_cache: RefCell::new(None),
            screenshot_cache: RefCell::new(None),
            visual_probe_cache: RefCell::new(None),
            scroll_strategy_cache: BTreeMap::new(),
            scroll_background_low_signal: BTreeSet::new(),
            raw_preauth: None,
            session_identity: None,
            trace: Vec::new(),
        };
        e.refresh()?;
        Ok(e)
    }

    // --- read tools ---------------------------------------------------------

    /// Re-perceive the target and rebuild scene + affordance graphs. The prior
    /// graph is kept as `previous` for `diff_since`.
    ///
    /// # Errors
    ///
    /// Returns an error if the perceptor fails to capture the target window's
    /// accessibility tree.
    pub fn refresh(&mut self) -> dunst_core::Result<()> {
        let roots = self.perceptor.capture(&self.target)?;
        let graph = scene::build_scene_graph(roots, self.window.clone(), dunst_core::now_ms());
        let aff = derive_affordances(&graph, &self.risk);
        self.previous = self.current.take();
        self.current = Some(graph);
        self.affordances = Some(aff);
        // Audit #9: compute the window rect + menubar root once per perception and
        // cache them, instead of re-scanning every node on each listing call.
        self.cached_window_rect = compute_window_rect(self.scene_graph());
        self.cached_menubar_root = compute_menubar_root(self.scene_graph());
        // Audit #2: a re-perception means the scene state the operator approved
        // may no longer hold (the dangerous element could have moved, changed
        // risk, or vanished). Drop every outstanding grant — and any pending gate —
        // so an approval can never silently survive a state change.
        self.approvals.clear();
        self.pending_gate_ids.clear();
        self.last_refresh_at = Some(Instant::now());
        *self.ocr_cache.borrow_mut() = None;
        *self.screenshot_cache.borrow_mut() = None;
        Ok(())
    }

    /// Re-perceive only if the current AX graph is older than the read-cache TTL.
    /// Mutating action paths call [`refresh`](Self::refresh) directly and bypass
    /// this throttle, so post-action state remains strongly fresh.
    ///
    /// # Errors
    ///
    /// Propagates any capture error from the underlying [`refresh`](Self::refresh)
    /// when a re-perception is triggered.
    pub fn refresh_if_stale(&mut self) -> dunst_core::Result<bool> {
        self.refresh_if_older_than(READ_REFRESH_TTL)
    }

    /// Re-perceive only if the current AX graph is older than `ttl`.
    ///
    /// Read-side callers use this to coalesce bursts of `force_refresh:true`
    /// requests without weakening explicit mutation paths, which still call
    /// [`refresh`](Self::refresh) after an action.
    ///
    /// # Errors
    ///
    /// Propagates any capture error from [`refresh`](Self::refresh) when the graph
    /// is older than `ttl` and a re-perception is performed.
    pub fn refresh_if_older_than(&mut self, ttl: Duration) -> dunst_core::Result<bool> {
        if self.last_refresh_at.is_some_and(|at| at.elapsed() <= ttl) {
            return Ok(false);
        }
        self.refresh().map(|()| true)
    }

    /// Whether the current scene graph was captured within `ttl`.
    pub fn graph_recent(&self, ttl: Duration) -> bool {
        self.last_refresh_at.is_some_and(|at| at.elapsed() <= ttl)
    }

    pub fn set_session_identity(&mut self, identity: SessionIdentity) {
        self.session_identity = Some(identity);
    }

    pub fn session_identity(&self) -> Option<&SessionIdentity> {
        self.session_identity.as_ref()
    }

    /// Re-target the engine to a different window at runtime — the MCP client
    /// picks one from `list_windows` and attaches, so the server has no fixed,
    /// hardcoded target. Re-perceives the new window.
    ///
    /// # Errors
    ///
    /// Returns an error if the new target's window cannot be resolved, or if the
    /// following [`refresh`](Self::refresh) fails to capture the scene.
    pub fn attach(&mut self, pid: i32, window_id: u32) -> dunst_core::Result<()> {
        self.approvals.clear();
        self.raw_approvals.clear();
        self.raw_approval_inflight.clear();
        self.active_batch = None;
        self.pending_gate_ids.clear();
        self.raw_preauth = None;
        self.target = Target { pid, window_id };
        self.window = self.perceptor.window_ref(&self.target)?;
        if self.target.window_id == 0 && self.window.window_id != 0 {
            self.target.window_id = self.window.window_id;
        }
        self.refresh()
    }

    /// Attach by `window_id` alone, resolving the owning pid via `list_windows`.
    ///
    /// # Errors
    ///
    /// Returns an error if no top-level window matches `window_id`, or if the
    /// subsequent [`attach`](Self::attach) fails to resolve or perceive it.
    #[cfg(target_os = "macos")]
    pub fn attach_window(&mut self, window_id: u32) -> dunst_core::Result<()> {
        let pid = dunst_vision::capture::list_windows()
            .into_iter()
            .find(|w| w.window_id == window_id)
            .map(|w| w.pid)
            .ok_or_else(|| DunstError::Perception(format!("window {window_id} not found")))?;
        // A stdio server may start on the device-free Notes fixture so it is
        // inspectable before a client chooses a real target. Once the client
        // attaches to a live WindowServer id, perception and actions must switch
        // to the macOS backend; otherwise the target tuple changes but the AX
        // graph still comes from the fixture.
        self.perceptor = Box::new(dunst_platform::MacosBackend::new());
        self.executor = Box::new(dunst_platform::MacosBackend::new());
        self.attach(pid, window_id)
    }

    /// Non-macOS stub.
    ///
    /// # Errors
    ///
    /// Always returns an error: attaching by window id requires the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn attach_window(&mut self, _window_id: u32) -> dunst_core::Result<()> {
        Err(DunstError::Perception(
            "attach requires a macOS backend".into(),
        ))
    }

    /// The current target as `(pid, window_id)`.
    pub fn target(&self) -> (i32, u32) {
        (self.target.pid, self.target.window_id)
    }

    pub fn scene_graph(&self) -> &SceneGraph {
        self.current.as_ref().expect("refreshed in new()")
    }

    pub fn affordance_graph(&self) -> &AffordanceGraph {
        self.affordances.as_ref().expect("refreshed in new()")
    }

    /// Substring match (case-insensitive) over label / id / ax_role.
    ///
    /// Matches are ranked so visible, enabled targets come first, but latent
    /// nodes are still returned. That preserves the contract that find-by-query
    /// can reach collapsed/off-screen elements while making live browser noise
    /// (menu items, off-window chrome) less likely to be picked first.
    pub fn find_element(&self, query: &str) -> Vec<&SceneNode> {
        self.find_element_filtered(query, false)
    }

    /// As [`find_element`](Self::find_element), optionally dropping latent /
    /// off-window matches. The filtered form is useful for live web automation
    /// where browser chrome and history menu items can match the same text as
    /// the page target.
    pub fn find_element_filtered(&self, query: &str, visible_only: bool) -> Vec<&SceneNode> {
        let q = normalize_match(query);
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let graph = self.scene_graph();
        let mut matches: Vec<&SceneNode> = graph
            .nodes
            .values()
            .filter(|n| {
                normalize_match(&n.id).contains(&q)
                    || n.label
                        .as_deref()
                        .map(|l| normalized_contains_query(&normalize_match(l), &q))
                        .unwrap_or(false)
                    || normalized_contains_query(&normalize_match(&n.ax_role), &q)
            })
            .filter(|n| !visible_only || node_visible_or_menu(n, window_rect, menubar))
            .collect();
        let mut seen: BTreeSet<String> = matches.iter().map(|node| node.id.clone()).collect();
        let mut associated_controls: BTreeSet<String> = BTreeSet::new();
        for label in matches.clone() {
            if let Some(control) = associated_control_for_label(label, graph, window_rect, menubar)
            {
                associated_controls.insert(control.id.clone());
                if seen.insert(control.id.clone()) {
                    matches.push(control);
                }
            }
        }
        matches.sort_by_key(|n| {
            find_rank(
                n,
                &q,
                window_rect,
                menubar,
                associated_controls.contains(&n.id),
            )
        });
        matches
    }

    // --- verification -------------------------------------------------------

    /// Diff `previous -> current` (empty if only one snapshot exists).
    pub fn diff_since(&self) -> GraphDiff {
        match (&self.previous, &self.current) {
            (Some(p), Some(c)) => audit::diff(p, c),
            _ => GraphDiff::default(),
        }
    }

    /// Assert a node's `field` currently equals `expected`. `field` is one of
    /// `label` | `value` | `enabled` | `focused`.
    ///
    /// # Errors
    ///
    /// Returns an error if no node has `id`, or if `field` is not one of `label`,
    /// `value`, `enabled`, or `focused`.
    pub fn verify_state(&self, id: &str, field: &str, expected: &str) -> dunst_core::Result<bool> {
        let n = self
            .scene_graph()
            .get(id)
            .ok_or_else(|| DunstError::ElementNotFound(id.into()))?;
        let actual = match field {
            "label" => n.label.clone().unwrap_or_default(),
            "value" => n.value.clone().unwrap_or_default(),
            "enabled" => n.enabled.to_string(),
            "focused" => n.focused.to_string(),
            other => return Err(DunstError::Execution(format!("unknown field {other}"))),
        };
        Ok(actual == expected)
    }

    // --- approval -----------------------------------------------------------

    /// Whitelist a high-risk element or validated synthetic raw target so the next gated
    /// action can proceed.
    ///
    /// Element ids are validated against the current scene and must still be genuinely gated:
    /// * their own current risk requires approval, **or**
    /// * they are the subject of a pending contextual gate.
    ///
    /// Synthetic raw target ids are validated structurally and, when they carry coordinates,
    /// bounded to the current target window. That lets an operator re-approve the same raw
    /// target after a pending response was lost without parking a grant on an arbitrary id.
    ///
    /// Element grants are one-shot and refresh-cleared. Raw grants are event-count and
    /// TTL-limited, and scoped to the currently attached target window.
    ///
    /// # Errors
    ///
    /// Returns an error if a synthetic raw target fails structural or window-bound
    /// validation, if `id` is neither a current scene node nor a pending gate, or
    /// if the element exists but is not gated (no approval is required).
    pub fn approve(&mut self, id: &str) -> dunst_core::Result<()> {
        if is_synthetic_approval_target_id(id) {
            self.validate_synthetic_raw_approval(id)?;
            self.approve_raw_input(id);
            return Ok(());
        }

        let is_pending_synthetic = self.pending_gate_ids.contains(id);
        let is_scene_id = self.scene_graph().get(id).is_some();
        if !is_scene_id && !is_pending_synthetic {
            return Err(DunstError::ElementNotFound(id.into()));
        }
        let own_gated = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.requires_approval)
            .unwrap_or(false);
        if !own_gated && !is_pending_synthetic {
            return Err(DunstError::Execution(format!(
                "{id} is not gated; no approval required"
            )));
        }
        self.approvals.insert(id.to_string());
        Ok(())
    }

    /// Install a bounded pre-authorization for raw input in the attached window:
    /// the next `budget` gated raw actions (clamped) run without a per-action
    /// `approve`, until the budget or `ttl_ms` (clamped) is spent, or the target
    /// window changes. Returns the effective `(window_id, budget, ttl_ms)`.
    pub fn preauthorize_raw_input(&mut self, budget: usize, ttl_ms: u64) -> (u32, usize, u64) {
        let budget = budget.clamp(1, MAX_PREAUTH_BUDGET);
        let ttl_ms = ttl_ms.clamp(1_000, MAX_PREAUTH_TTL_MS);
        let window_id = self.target.window_id;
        self.raw_preauth = Some(RawPreauthorization {
            window_id,
            remaining: budget,
            expires_at: Instant::now() + Duration::from_millis(ttl_ms),
        });
        (window_id, budget, ttl_ms)
    }

    /// Drop any active raw-input pre-authorization. Returns whether one was live.
    pub fn revoke_raw_preauthorization(&mut self) -> bool {
        self.raw_preauth.take().is_some()
    }

    /// `(window_id, remaining_budget, ms_left)` if a pre-authorization is still
    /// live for the attached window, else `None`.
    pub fn raw_preauthorization_remaining(&self) -> Option<(u32, usize, u64)> {
        let pre = self.raw_preauth.as_ref()?;
        let now = Instant::now();
        if pre.window_id != self.target.window_id || now >= pre.expires_at || pre.remaining == 0 {
            return None;
        }
        Some((
            pre.window_id,
            pre.remaining,
            (pre.expires_at - now).as_millis() as u64,
        ))
    }
}

const MAX_PREAUTH_BUDGET: usize = 100;
const MAX_PREAUTH_TTL_MS: u64 = 600_000;

#[derive(Clone)]
struct RawApprovalGrant {
    remaining: usize,
    expires_at: Instant,
}

/// A caller-installed, window-scoped budget that lets raw input execute without a
/// per-action approval until it is spent. See [`Engine::preauthorize_raw_input`].
#[derive(Clone)]
struct RawPreauthorization {
    window_id: u32,
    remaining: usize,
    expires_at: Instant,
}

#[cfg(test)]
mod helper_tests;
#[cfg(test)]
mod tests;
