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
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, Bbox, GraphDiff, Perceptor,
    RiskAssessment, RiskLevel, Role, SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError,
    WindowRef,
};
use dunst_graph::{audit, derive_affordances, scene, RiskEngine};
use serde_json::{json, Value};

mod action;
mod action_resolution;
mod app_ops;
mod apps;
mod browser_query;
mod chart;
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
use query_support::*;
use runtime_support::*;
use scene_query::*;
use window_geometry::*;

pub use types::*;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const READ_REFRESH_TTL: Duration = Duration::from_millis(500);
const DISPLAY_CACHE_TTL: Duration = Duration::from_millis(1_000);
const OCR_CACHE_TTL: Duration = Duration::from_millis(250);
const SCREENSHOT_CACHE_TTL: Duration = Duration::from_millis(250);
#[cfg(target_os = "macos")]
const SELECT_FILE_OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(12);

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
    screenshot_cache: RefCell<Option<TimedCache<String>>>,
    visual_probe_cache: RefCell<Option<VisualProbeCacheEntry>>,
    trace: Vec<AuditEntry>,
}

impl Engine {
    pub fn new(
        perceptor: Box<dyn Perceptor>,
        executor: Box<dyn ActionExecutor>,
        target: Target,
    ) -> dunst_core::Result<Self> {
        let window = perceptor.window_ref(&target)?;
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
            pending_gate_ids: BTreeSet::new(),
            cached_window_rect: None,
            cached_menubar_root: None,
            last_refresh_at: None,
            display_cache: RefCell::new(None),
            desktop_cache: RefCell::new(None),
            ocr_cache: RefCell::new(None),
            screenshot_cache: RefCell::new(None),
            visual_probe_cache: RefCell::new(None),
            trace: Vec::new(),
        };
        e.refresh()?;
        Ok(e)
    }

    // --- read tools ---------------------------------------------------------

    /// Re-perceive the target and rebuild scene + affordance graphs. The prior
    /// graph is kept as `previous` for `diff_since`.
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
    pub fn refresh_if_stale(&mut self) -> dunst_core::Result<bool> {
        self.refresh_if_older_than(READ_REFRESH_TTL)
    }

    /// Re-perceive only if the current AX graph is older than `ttl`.
    ///
    /// Read-side callers use this to coalesce bursts of `force_refresh:true`
    /// requests without weakening explicit mutation paths, which still call
    /// [`refresh`](Self::refresh) after an action.
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

    /// Re-target the engine to a different window at runtime — the MCP client
    /// picks one from `list_windows` and attaches, so the server has no fixed,
    /// hardcoded target. Re-perceives the new window.
    pub fn attach(&mut self, pid: i32, window_id: u32) -> dunst_core::Result<()> {
        self.target = Target { pid, window_id };
        self.window = self.perceptor.window_ref(&self.target)?;
        self.refresh()
    }

    /// Attach by `window_id` alone, resolving the owning pid via `list_windows`.
    #[cfg(target_os = "macos")]
    pub fn attach_window(&mut self, window_id: u32) -> dunst_core::Result<()> {
        let pid = dunst_vision::capture::list_windows()
            .into_iter()
            .find(|w| w.window_id == window_id)
            .map(|w| w.pid)
            .ok_or_else(|| VisualOpsError::Perception(format!("window {window_id} not found")))?;
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
    #[cfg(not(target_os = "macos"))]
    pub fn attach_window(&mut self, _window_id: u32) -> dunst_core::Result<()> {
        Err(VisualOpsError::Perception(
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
                normalized_contains_query(&normalize_match(&n.id), &q)
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
    pub fn verify_state(&self, id: &str, field: &str, expected: &str) -> dunst_core::Result<bool> {
        let n = self
            .scene_graph()
            .get(id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
        let actual = match field {
            "label" => n.label.clone().unwrap_or_default(),
            "value" => n.value.clone().unwrap_or_default(),
            "enabled" => n.enabled.to_string(),
            "focused" => n.focused.to_string(),
            other => return Err(VisualOpsError::Execution(format!("unknown field {other}"))),
        };
        Ok(actual == expected)
    }

    // --- approval -----------------------------------------------------------

    /// Whitelist a high-risk element so the **next** gated action on it proceeds.
    ///
    /// Audit #2 — validated at call time, not blindly stored. The id must exist in
    /// the current scene (`ElementNotFound` otherwise) and be genuinely gated:
    /// * its own current risk requires approval (a high-risk element / drop target), **or**
    /// * it is the subject of a pending contextual gate — e.g. a destructive value
    ///   typed into an otherwise low-risk field (audit #13).
    ///
    /// Approving a phantom or a plain low-risk id is an error, so a grant can never
    /// be parked on something that isn't gated. The grant is **one-shot**:
    /// [`act`](Self::act) consumes it on the next successful action, and every
    /// [`refresh`](Self::refresh) clears all grants.
    pub fn approve(&mut self, id: &str) -> dunst_core::Result<()> {
        let is_pending_synthetic = self.pending_gate_ids.contains(id);
        let is_scene_id = self.scene_graph().get(id).is_some();
        if !is_scene_id && !is_pending_synthetic {
            return Err(VisualOpsError::ElementNotFound(id.into()));
        }
        let own_gated = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.requires_approval)
            .unwrap_or(false);
        if !own_gated && !is_pending_synthetic {
            return Err(VisualOpsError::Execution(format!(
                "{id} is not gated; no approval required"
            )));
        }
        self.approvals.insert(id.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod helper_tests;
#[cfg(test)]
mod tests;
