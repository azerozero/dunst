//! The VisualOps engine — the runtime-agnostic service behind the MCP tools.
//!
//! Holds a [`Perceptor`] + [`ActionExecutor`] + [`RiskEngine`], maintains the
//! current/previous [`SceneGraph`] and [`AffordanceGraph`], enforces
//! risk-based approval gating, and records an [`AuditEntry`] per action.
//!
//! This struct is transport-independent: the MCP server (`serve`) and the CLI
//! `demo` both drive the same methods.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use visualops_core::{
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, Bbox, GraphDiff, Perceptor,
    RiskAssessment, Role, SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError,
    WindowRef,
};
use visualops_graph::{audit, derive_affordances, scene, RiskEngine};

/// Projection requested for [`Engine::scene_graph_view`] (WP-J / J1). The MCP
/// server defaults to [`Compact`](SceneView::Compact) so a real client can take
/// the graph inline; [`Full`](SceneView::Full) is the unchanged escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneView {
    /// Per-node `{id, role, label, value?, bbox, enabled, focused, parent,
    /// n_children}` — the heavy/derivable AX fields are dropped (~5–10× lighter).
    Compact,
    /// Today's behaviour: the full [`SceneGraph`], every field.
    Full,
    /// No per-node list — `{n_nodes, roots, counts_by_role, n_actionable, window}`.
    Summary,
}

impl SceneView {
    /// Parse the MCP `view` argument; `None` for an unrecognised value.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

/// One OCR'd line returned by [`Engine::read_text`]: the recognised `text`, its
/// bounding box in **screen points** (mapped from Vision's normalised box via
/// `coords::vision_norm_to_screen_pt`), and Vision's `confidence` in `[0,1]`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextHit {
    pub text: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

/// One geometric primitive returned by [`Engine::read_shapes`]: its `kind`
/// (`"Rect"`/`"Bar"`/`"Circle"`/`"Line"`/`"Unknown"`), bounding box in **screen
/// points**, and a heuristic `confidence`. The CV layer for figures (charts,
/// custom-drawn UI) that neither AX nor OCR exposes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShapeHit {
    pub kind: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

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
    trace: Vec<AuditEntry>,
}

impl Engine {
    pub fn new(
        perceptor: Box<dyn Perceptor>,
        executor: Box<dyn ActionExecutor>,
        target: Target,
    ) -> visualops_core::Result<Self> {
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
            trace: Vec::new(),
        };
        e.refresh()?;
        Ok(e)
    }

    // --- read tools ---------------------------------------------------------

    /// Re-perceive the target and rebuild scene + affordance graphs. The prior
    /// graph is kept as `previous` for `diff_since`.
    pub fn refresh(&mut self) -> visualops_core::Result<()> {
        let roots = self.perceptor.capture(&self.target)?;
        let graph = scene::build_scene_graph(roots, self.window.clone(), visualops_core::now_ms());
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
        Ok(())
    }

    pub fn scene_graph(&self) -> &SceneGraph {
        self.current.as_ref().expect("refreshed in new()")
    }

    pub fn affordance_graph(&self) -> &AffordanceGraph {
        self.affordances.as_ref().expect("refreshed in new()")
    }

    /// Substring match (case-insensitive) over label / id / ax_role.
    pub fn find_element(&self, query: &str) -> Vec<&SceneNode> {
        let q = query.to_lowercase();
        self.scene_graph()
            .nodes
            .values()
            .filter(|n| {
                n.id.to_lowercase().contains(&q)
                    || n.label.as_deref().map(|l| l.to_lowercase().contains(&q)).unwrap_or(false)
                    || n.ax_role.to_lowercase().contains(&q)
            })
            .collect()
    }

    /// OCR the target window via Apple Vision (P1). A pure **read probe** like the
    /// scene-graph getters: it does **not** risk-gate and records **no** audit entry.
    /// `region_screen_pt` limits OCR to a screen-point rectangle; `None` reads the
    /// whole window. Each hit's bbox is mapped from Vision's normalised space to
    /// screen points. macOS-only — see the non-macOS stub below.
    #[cfg(target_os = "macos")]
    pub fn read_text(&self, region_screen_pt: Option<Bbox>) -> visualops_core::Result<Vec<TextHit>> {
        let captured =
            visualops_vision::capture::capture_window(self.target.window_id).map_err(|e| {
                VisualOpsError::Perception(format!(
                    "OCR requires a live macOS window (capture failed: {e})"
                ))
            })?;
        let boxes =
            visualops_vision::ocr::ocr_region(&captured.image, &captured.geometry, region_screen_pt)
                .map_err(|e| VisualOpsError::Perception(format!("OCR failed: {e}")))?;
        Ok(boxes
            .into_iter()
            .map(|b| TextHit {
                text: b.text,
                bbox: visualops_vision::coords::vision_norm_to_screen_pt(b.norm, &captured.geometry),
                confidence: b.confidence,
            })
            .collect())
    }

    /// Non-macOS stub: Apple Vision OCR needs a live macOS window. Keeps
    /// `visualops-mcp` compilable (and the `read_text` tool present) on other targets.
    #[cfg(not(target_os = "macos"))]
    pub fn read_text(&self, _region_screen_pt: Option<Bbox>) -> visualops_core::Result<Vec<TextHit>> {
        Err(VisualOpsError::Perception(
            "OCR requires a live macOS window".into(),
        ))
    }

    /// Detect geometric primitives (rect/bar/circle/line) in the target window
    /// via the CV `shapes` layer — the figures (charts, custom-drawn UI) AX and
    /// OCR can't expose. A pure **read probe** like [`read_text`](Self::read_text):
    /// no risk-gating, no audit entry. macOS-only.
    #[cfg(target_os = "macos")]
    pub fn read_shapes(&self) -> visualops_core::Result<Vec<ShapeHit>> {
        let captured =
            visualops_vision::capture::capture_window(self.target.window_id).map_err(|e| {
                VisualOpsError::Perception(format!(
                    "shape detection requires a live macOS window (capture failed: {e})"
                ))
            })?;
        Ok(
            visualops_vision::shapes::detect_shapes(&captured.image, &captured.geometry)
                .into_iter()
                .map(|s| ShapeHit {
                    kind: format!("{:?}", s.kind),
                    bbox: s.bbox,
                    confidence: s.confidence,
                })
                .collect(),
        )
    }

    /// Non-macOS stub: shape detection needs a live macOS window.
    #[cfg(not(target_os = "macos"))]
    pub fn read_shapes(&self) -> visualops_core::Result<Vec<ShapeHit>> {
        Err(VisualOpsError::Perception(
            "shape detection requires a live macOS window".into(),
        ))
    }

    /// IDs whose affordance offers `action`. WP-J/J2: latent (off-screen /
    /// zero-bbox) nodes — e.g. collapsed-menu items — are omitted by default so
    /// the agent isn't handed phantom targets. The gated action path is
    /// unaffected: it resolves ids against the graph, not this listing.
    ///
    /// Ergonomic default over [`query_affordances_filtered`](Self::query_affordances_filtered);
    /// the MCP server calls the latter directly, so in the binary this wrapper is
    /// exercised only by callers/tests that want the filtered listing.
    // `expect` is scoped to non-test builds: these fns ARE used by the test module,
    // so a bare `#[expect(dead_code)]` would be "unfulfilled" under the test target.
    // In the binary they are genuinely dead — and clippy will flag this expectation
    // the moment a non-test caller appears (the point of `expect` over `allow`).
    #[cfg_attr(not(test), expect(dead_code, reason = "ergonomic unfiltered wrapper, exercised only by tests"))]
    pub fn query_affordances(&self, action: SemanticAction) -> Vec<String> {
        self.query_affordances_filtered(action, false)
    }

    /// As [`query_affordances`](Self::query_affordances), but `include_latent`
    /// returns every id exposing `action`, latent ones included.
    pub fn query_affordances_filtered(
        &self,
        action: SemanticAction,
        include_latent: bool,
    ) -> Vec<String> {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        self.affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .filter(|a| {
                include_latent
                    || g.get(&a.id)
                        .map(|n| node_visible_or_menu(n, window_rect, menubar))
                        .unwrap_or(false)
            })
            .map(|a| a.id.clone())
            .collect()
    }

    /// WP-J/J2: the affordance graph as JSON, latent nodes omitted unless
    /// `include_latent`. Shape matches [`AffordanceGraph`] (`{ "affordances": … }`).
    pub fn affordances_view(&self, include_latent: bool) -> Value {
        let ag = self.affordance_graph();
        if include_latent {
            return serde_json::to_value(ag).unwrap_or(Value::Null);
        }
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        let mut map = serde_json::Map::new();
        for (id, aff) in &ag.affordances {
            if g.get(id).map(|n| node_visible_or_menu(n, window_rect, menubar)).unwrap_or(false) {
                map.insert(id.clone(), serde_json::to_value(aff).unwrap_or(Value::Null));
            }
        }
        json!({ "affordances": Value::Object(map) })
    }

    /// WP-J/J1: the scene graph under a projection `view`, optionally limited to
    /// actionable nodes. `Full` without `actionable_only` is byte-for-byte the
    /// old `get_scene_graph` payload (the escape hatch).
    pub fn scene_graph_view(&self, view: SceneView, actionable_only: bool) -> Value {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        match view {
            SceneView::Full if !actionable_only => serde_json::to_value(g).unwrap_or(Value::Null),
            SceneView::Full => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if node_actionable(n, window_rect, menubar) {
                        map.insert(id.clone(), serde_json::to_value(n).unwrap_or(Value::Null));
                    }
                }
                json!({
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Compact => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if actionable_only && !node_actionable(n, window_rect, menubar) {
                        continue;
                    }
                    map.insert(id.clone(), compact_node(n));
                }
                json!({
                    "view": "compact",
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Summary => {
                let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
                let mut n_actionable = 0usize;
                for n in g.nodes.values() {
                    *counts.entry(n.role.as_str()).or_insert(0) += 1;
                    if node_actionable(n, window_rect, menubar) {
                        n_actionable += 1;
                    }
                }
                json!({
                    "view": "summary",
                    "n_nodes": g.nodes.len(),
                    "roots": g.roots,
                    "counts_by_role": counts,
                    "n_actionable": n_actionable,
                    "window": g.window,
                })
            }
        }
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
    /// `label` | `value` | `enabled`.
    pub fn verify_state(&self, id: &str, field: &str, expected: &str) -> visualops_core::Result<bool> {
        let n = self
            .scene_graph()
            .get(id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
        let actual = match field {
            "label" => n.label.clone().unwrap_or_default(),
            "value" => n.value.clone().unwrap_or_default(),
            "enabled" => n.enabled.to_string(),
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
    pub fn approve(&mut self, id: &str) -> visualops_core::Result<()> {
        if self.scene_graph().get(id).is_none() {
            return Err(VisualOpsError::ElementNotFound(id.into()));
        }
        let own_gated = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.requires_approval)
            .unwrap_or(false);
        if !own_gated && !self.pending_gate_ids.contains(id) {
            return Err(VisualOpsError::Execution(format!(
                "{id} is not gated; no approval required"
            )));
        }
        self.approvals.insert(id.to_string());
        Ok(())
    }

    // --- action tools -------------------------------------------------------

    pub fn click_element(&mut self, id: &str, reasoning: Option<&str>) -> visualops_core::Result<AuditEntry> {
        self.act(id, SemanticAction::Click, None, reasoning, None)
    }

    pub fn type_into(&mut self, id: &str, text: &str, reasoning: Option<&str>) -> visualops_core::Result<AuditEntry> {
        // Guard the synchronous keystroke path against a multi-MB payload (audit C9).
        const MAX_TYPE_LEN: usize = 100_000;
        if text.len() > MAX_TYPE_LEN {
            return Err(visualops_core::VisualOpsError::Execution(format!(
                "type text too long: {} bytes (max {MAX_TYPE_LEN})",
                text.len()
            )));
        }
        self.act(id, SemanticAction::Type, Some(text), reasoning, None)
    }

    pub fn hover_probe(&mut self, id: &str) -> visualops_core::Result<AuditEntry> {
        self.act(id, SemanticAction::Hover, None, Some("hover probe"), None)
    }

    /// Drag `source_id` onto `target_id`. The drop point handed to the executor
    /// is the **target** node's bbox centre in screen coordinates, formatted as
    /// `"x,y"` (the frozen WP-F drag mini-contract). This is a thin wrapper over
    /// the gated [`act`] path — `act` checks the *source* exposes `Drag`, gates
    /// on risk, runs the executor, re-perceives, diffs and audits.
    ///
    /// Audit #3 — **composite risk**: a drop is as dangerous as the riskier of its
    /// source and its target (dropping a file onto "Supprimer" is a delete, even
    /// though the file row is harmless). The drop target's risk is folded in here
    /// and `act` gates on the max, so a high-risk target forces approval even when
    /// the source is low-risk.
    pub fn drag_element(
        &mut self,
        source_id: &str,
        target_id: &str,
        reasoning: Option<&str>,
    ) -> visualops_core::Result<AuditEntry> {
        let target = self
            .scene_graph()
            .get(target_id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(target_id.into()))?;
        let bbox = target.bbox.ok_or_else(|| {
            VisualOpsError::Execution(format!(
                "target {target_id} has no bbox; a drop needs a concrete point"
            ))
        })?;
        let x = bbox.x + bbox.w / 2.0;
        let y = bbox.y + bbox.h / 2.0;
        // Fold the drop target's risk into the gate (audit #3). Every node has an
        // affordance entry; default to low if one is somehow missing.
        let target_risk = self
            .affordance_graph()
            .affordances
            .get(target_id)
            .map(|a| a.risk.clone())
            .unwrap_or_else(RiskAssessment::low);
        let co_target = CoTarget { id: target_id.to_string(), risk: target_risk };
        self.act(
            source_id,
            SemanticAction::Drag,
            Some(&format!("{x},{y}")),
            reasoning,
            Some(co_target),
        )
    }

    // --- raw input tools (ungated) ------------------------------------------

    /// Click at a raw **screen point** (P1 navigation: OCR a link with `read_text`,
    /// then click its bbox centre).
    ///
    /// ⚠️ Unlike [`click_element`](Self::click_element), this is **not** bound to an
    /// element or affordance, so it **bypasses per-element risk gating entirely** —
    /// there is no label to assess, so the entry is audited LOW. The agent owns the
    /// coordinate it derived from OCR and is responsible for it; a raw click can land
    /// on anything under that point. It is still audited (`target_id =
    /// "screen@x,y"`) and re-perceives afterwards like [`act`](Self::act).
    #[cfg(target_os = "macos")]
    pub fn click_at(&mut self, x: f64, y: f64) -> visualops_core::Result<AuditEntry> {
        let outcome = visualops_platform::click_at_point(self.target.pid, x, y);
        self.audit_raw_input(
            format!("screen@{x},{y}"),
            SemanticAction::Click,
            Some(format!("{x},{y}")),
            Some("raw screen click — bypasses per-element risk gating (agent owns the OCR coordinate)"),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn click_at(&mut self, _x: f64, _y: f64) -> visualops_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution("click_at requires a macOS backend".into()))
    }

    /// Press a named key (e.g. `"Return"`/`"Enter"` to submit a typed URL). A raw,
    /// ungated keyboard input audited LOW; re-perceives afterwards like
    /// [`act`](Self::act).
    #[cfg(target_os = "macos")]
    pub fn press_key(&mut self, key: &str) -> visualops_core::Result<AuditEntry> {
        let outcome = visualops_platform::press_key(self.target.pid, key);
        self.audit_raw_input(
            "keyboard".to_string(),
            SemanticAction::Type,
            Some(key.to_string()),
            Some("raw key press (ungated)"),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn press_key(&mut self, _key: &str) -> visualops_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution("press_key requires a macOS backend".into()))
    }

    /// Background hover at a screen point so the target shows a hover state (e.g.
    /// a chart crosshair tooltip / value-at-cursor) without moving the visible
    /// cursor. A pure probe — no risk-gating, no audit, **no refresh** — so a
    /// following `read_text` reads the hovered result.
    #[cfg(target_os = "macos")]
    pub fn hover_at(&self, x: f64, y: f64) -> visualops_core::Result<()> {
        visualops_platform::hover_at_point(self.target.pid, x, y)
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn hover_at(&self, _x: f64, _y: f64) -> visualops_core::Result<()> {
        Err(VisualOpsError::Execution("hover_at requires a macOS backend".into()))
    }

    /// Read text at **several** screen points by time-multiplexing the OS cursor
    /// (the operator's idea): one borrow for the whole sweep — decouple the
    /// hardware mouse, warp to each point in turn to trigger its hover (e.g. a
    /// chart crosshair value-at-cursor), OCR a region around it — then restore the
    /// cursor and re-couple the mouse. Reading N values at intervals freezes the
    /// user's mouse **once** (briefly), not N times. For Chrome/web prefer CDP
    /// (no borrow). macOS-only.
    #[cfg(target_os = "macos")]
    pub fn read_series(&self, points: &[(f64, f64)]) -> visualops_core::Result<Vec<Vec<TextHit>>> {
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let (x0, y0) = points[0];
        let saved = visualops_platform::cursor_borrow_to(x0, y0)?;
        let mut out = Vec::with_capacity(points.len());
        for &(x, y) in points {
            // Move to the point (the hover already triggers reliably — no circle
            // needed) and OCR a tight region where the value-at-cursor renders.
            let _ = visualops_platform::hover_at_point(self.target.pid, x, y);
            std::thread::sleep(std::time::Duration::from_millis(120));
            let region = Bbox { x: x - 30.0, y: y - 70.0, w: 240.0, h: 150.0 };
            out.push(self.read_text(Some(region)).unwrap_or_default());
        }
        let _ = visualops_platform::cursor_restore(saved.0, saved.1);
        Ok(out)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn read_series(&self, _points: &[(f64, f64)]) -> visualops_core::Result<Vec<Vec<TextHit>>> {
        Err(VisualOpsError::Execution("read_series requires a macOS backend".into()))
    }

    /// Single-point [`read_series`](Self::read_series): borrow the cursor, hover
    /// `(x, y)`, OCR around it, restore.
    pub fn read_at(&self, x: f64, y: f64) -> visualops_core::Result<Vec<TextHit>> {
        Ok(self.read_series(&[(x, y)])?.into_iter().next().unwrap_or_default())
    }

    /// Record a **raw input** (`click_at` / `press_key`) — a coordinate/key not bound
    /// to any element, hence no affordance and no per-element gating (LOW risk by
    /// construction). The attempt is always written to the trace; on platform
    /// failure the entry is `Failed` and the error is surfaced to the caller. Mirrors
    /// [`act`](Self::act)'s re-perceive (`refresh` + `diff_since`).
    #[cfg(target_os = "macos")]
    fn audit_raw_input(
        &mut self,
        target_id: String,
        action: SemanticAction,
        argument: Option<String>,
        reasoning: Option<&str>,
        outcome: visualops_core::Result<()>,
    ) -> visualops_core::Result<AuditEntry> {
        let ts_ms = visualops_core::now_ms();
        let result = if outcome.is_ok() {
            ActionResult::Success
        } else {
            ActionResult::Failed
        };
        let _ = self.refresh();
        let graph_diff = self.diff_since();
        let entry = self.push_entry(AuditEntry {
            ts_ms,
            target_id,
            action,
            argument,
            risk: RiskAssessment::low(),
            reasoning: reasoning.map(str::to_owned),
            result,
            graph_diff,
        });
        outcome.map(|()| entry)
    }

    /// Compute an action's **effective risk** and the set of ids whose approval
    /// clears its gate. Folds a composite drag's drop target (audit #3) and a
    /// destructive typed payload (audit #13) into the source element's own risk via
    /// [`merge_risk`]. Pure over its inputs and `self.risk` — no scene mutation — so
    /// it is unit-testable in isolation (the `effective_risk_*` tests).
    ///
    /// Returns `(effective, gated_ids)`: `effective.requires_approval` decides
    /// whether the gate fires; `gated_ids` lists every high-risk participant that
    /// must be approved (the element, the drop target, or the typed-into field).
    fn effective_risk(
        &self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        source_risk: &RiskAssessment,
        co_target: Option<&CoTarget>,
    ) -> (RiskAssessment, Vec<String>) {
        // Audit #13: for a Type action the *payload* can be destructive even when
        // the field itself is harmless — assess the typed text and fold it in.
        let text_risk = match (action, argument) {
            (SemanticAction::Type, Some(arg)) => Some(self.risk.assess_text(arg)),
            _ => None,
        };

        // Effective risk = max(source, drop target [#3], typed text [#13]). The
        // merged `reasons` ("drop target: …" / "typed text: …") say which facet
        // raised the gate.
        let mut effective = match co_target {
            Some(co) => merge_risk(source_risk, &co.risk, "drop target"),
            None => source_risk.clone(),
        };
        if let Some(tr) = &text_risk {
            effective = merge_risk(&effective, tr, "typed text");
        }

        // Every high-risk participant must be approved to clear the gate: the
        // element itself, a composite drag's drop target, or the typed-into field.
        let mut gated_ids: Vec<String> = Vec::new();
        if source_risk.requires_approval {
            gated_ids.push(id.to_string());
        }
        if let Some(co) = co_target {
            if co.risk.requires_approval {
                gated_ids.push(co.id.clone());
            }
        }
        if text_risk.as_ref().map(|r| r.requires_approval).unwrap_or(false)
            && !gated_ids.iter().any(|g| g == id)
        {
            gated_ids.push(id.to_string());
        }
        (effective, gated_ids)
    }

    /// The gated action path: **resolve → effective_risk → gate → execute →
    /// audit**. Always returns an [`AuditEntry`] describing the outcome (also
    /// appended to the trace); only structural problems (unknown id / unavailable
    /// action) are `Err`.
    ///
    /// `co_target` carries a second risk-bearing participant (audit #3 — a drag's
    /// drop target). The gate fires on the **max** of the acted-on element and the
    /// co-target, and the grant must cover *every* high-risk participant.
    fn act(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        co_target: Option<CoTarget>,
    ) -> visualops_core::Result<AuditEntry> {
        let node = self
            .scene_graph()
            .get(id)
            .cloned()
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
        // Read the source affordance once and drop the borrow before we mutate.
        let source_risk = {
            let aff = self
                .affordance_graph()
                .affordances
                .get(id)
                .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
            if !aff.actions.contains(&action) {
                return Err(VisualOpsError::ActionUnavailable {
                    id: id.into(),
                    action: format!("{action:?}"),
                });
            }
            aff.risk.clone()
        };

        // Risk: fold in a composite drag target (#3) and a destructive typed
        // payload (#13). `effective.requires_approval` decides the gate; `gated_ids`
        // names the participants whose approval clears it.
        let (effective, gated_ids) =
            self.effective_risk(id, action, argument, &source_risk, co_target.as_ref());
        // A gate with no nameable participant must NOT pass vacuously: require a
        // non-empty, fully-approved set. (When `effective.requires_approval` is
        // true, `gated_ids` is always non-empty by construction in `effective_risk`.)
        let approved = !gated_ids.is_empty() && gated_ids.iter().all(|g| self.approvals.contains(g));

        // Build the audit record once; the two outcome paths only differ in
        // `result` and `graph_diff` (applied via struct update below).
        let base = AuditEntry {
            ts_ms: visualops_core::now_ms(),
            target_id: id.to_string(),
            action,
            argument: argument.map(str::to_owned),
            risk: effective.clone(),
            reasoning: reasoning.map(str::to_owned),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        };

        // Gate: high-risk actions need prior approval. Note the executor is
        // never invoked on this path. Record the gated participants so a later
        // `approve` can authorise a contextually-gated id (audit #13).
        if effective.requires_approval && !approved {
            for g in &gated_ids {
                self.pending_gate_ids.insert(g.clone());
            }
            return Ok(self.push_entry(base));
        }

        // Execute, then re-perceive and diff.
        let result = match self.executor.perform(&self.target, &node, action, argument) {
            Ok(()) => ActionResult::Success,
            Err(_) => ActionResult::Failed,
        };
        // One-shot consumption (audit #2): a grant authorises exactly one
        // successful action; drop it (and clear any pending-gate marker) so a
        // repeat re-gates. (`refresh` below also clears all grants — this keeps
        // the semantics explicit and independent of refresh ordering.)
        if result == ActionResult::Success {
            for g in &gated_ids {
                self.approvals.remove(g);
                self.pending_gate_ids.remove(g);
            }
        }
        let _ = self.refresh();
        let graph_diff = self.diff_since();
        Ok(self.push_entry(AuditEntry { result, graph_diff, ..base }))
    }

    fn push_entry(&mut self, entry: AuditEntry) -> AuditEntry {
        self.trace.push(entry.clone());
        entry
    }

    // --- audit --------------------------------------------------------------

    /// Public accessor over the audit trail; exercised by the gating tests and
    /// part of the engine API the MCP layer may surface.
    #[cfg_attr(not(test), expect(dead_code, reason = "public audit-trail accessor, exercised only by tests"))]
    pub fn trace(&self) -> &[AuditEntry] {
        &self.trace
    }

    pub fn export_trace(&self) -> visualops_core::Result<String> {
        Ok(serde_json::to_string_pretty(&self.trace)?)
    }
}

// --- WP-J / J2: latent (non-actionable) node geometry -----------------------

/// The window's on-screen rect, read from the `Window` node's bbox (the scene
/// graph's [`WindowRef`] carries no geometry). `None` when no window node has a
/// bbox — then [`node_on_screen`]'s off-window test is skipped. Memoised by
/// [`Engine::refresh`] into `cached_window_rect` (audit #9).
fn compute_window_rect(g: &SceneGraph) -> Option<Bbox> {
    g.nodes.values().find(|n| n.role == Role::Window).and_then(|n| n.bbox)
}

/// Id of the menubar **root** — the `MenuBar`-role node in `roots` (its
/// `AXMenuBarItem` children share that role but have a parent, so iterating
/// `roots` disambiguates). Its direct children are the top-level menu openers
/// exempted from the latent filter by [`is_top_level_menu`]. Memoised by
/// [`Engine::refresh`] into `cached_menubar_root` (audit #9).
fn compute_menubar_root(g: &SceneGraph) -> Option<String> {
    g.roots
        .iter()
        .find(|id| g.get(id).map(|n| n.role == Role::MenuBar).unwrap_or(false))
        .cloned()
}

/// Two axis-aligned boxes overlap (shared positive area).
fn bbox_intersects(a: Bbox, b: Bbox) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

/// WP-J/J2 — whether a node has a real on-screen footprint. A node is **latent**
/// (the negation) when it has no bbox, a zero/negative-area bbox, or a bbox that
/// lies entirely outside the window rect — exactly the shape of collapsed-menu
/// `AXMenuItem`s, which sit at `(0,0)`/off-window until their parent opens. This
/// is read-only geometry over `bbox` + the window rect: the scene/affordance
/// graphs are never mutated, so `find_element` and click-by-id still reach these
/// nodes; only the *listings* filter them.
fn node_on_screen(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    let Some(b) = node.bbox else { return false };
    if b.w <= 0.0 || b.h <= 0.0 {
        return false;
    }
    match window_rect {
        Some(w) => bbox_intersects(b, w),
        None => true,
    }
}

/// WP-J follow-up — a node is a **top-level menu opener** when it sits directly
/// under the menubar root (Fichier, Édition, Format, …). These are legitimately
/// actionable (click / open_menu opens the menu) even with a null/off-window
/// bbox, so they are exempt from the latent filter. The rule is *structural*
/// (parent == menubar root id): deep collapsed submenu items — whose parent is a
/// closed `Menu`, not the menubar root — are NOT exempt and stay filtered.
fn is_top_level_menu(node: &SceneNode, menubar_root: Option<&str>) -> bool {
    matches!(
        (node.parent.as_deref(), menubar_root),
        (Some(parent), Some(root)) if parent == root
    )
}

/// Visible in actionable listings: a real on-screen footprint OR a top-level
/// menu opener (see [`is_top_level_menu`]). This is the predicate the affordance
/// listings filter on (geometry, no `enabled` requirement).
fn node_visible_or_menu(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    node_on_screen(node, window_rect) || is_top_level_menu(node, menubar_root)
}

/// J1 actionability: visible (on-screen or a top-level menu opener) **and**
/// enabled (what `actionable_only` keeps and `summary.n_actionable` counts).
fn node_actionable(node: &SceneNode, window_rect: Option<Bbox>, menubar_root: Option<&str>) -> bool {
    node.enabled && node_visible_or_menu(node, window_rect, menubar_root)
}

/// WP-J/J1 compact projection of one node: keep only the agent-facing fields and
/// drop the heavy/derivable AX detail (`ax_role`, `help`, `ax_actions`,
/// `ax_identifier`, `last_seen_ms`), collapsing `children` to a count.
fn compact_node(n: &SceneNode) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("id".into(), json!(n.id));
    o.insert("role".into(), json!(n.role.as_str()));
    if let Some(l) = &n.label {
        o.insert("label".into(), json!(l));
    }
    if let Some(v) = &n.value {
        o.insert("value".into(), json!(v));
    }
    o.insert("bbox".into(), serde_json::to_value(n.bbox).unwrap_or(Value::Null));
    o.insert("enabled".into(), json!(n.enabled));
    o.insert("focused".into(), json!(n.focused));
    if let Some(p) = &n.parent {
        o.insert("parent".into(), json!(p));
    }
    o.insert("n_children".into(), json!(n.children.len()));
    Value::Object(o)
}

/// A second risk-bearing participant in an action — the **drop target** of a drag
/// (audit #3). Carried into [`Engine::act`] so the gate can combine its risk with
/// the dragged element's.
struct CoTarget {
    id: String,
    risk: RiskAssessment,
}

/// Combine a base risk with an extra risk-bearing facet (a drag's drop target,
/// audit #3; or the typed payload, audit #13): the higher tier, approval required
/// if *either* requires it, and the extra's reasons merged in with `label: …` so
/// the audit shows which facet raised the gate. `RiskLevel` is `Ord`, so `max` is
/// the stricter tier.
fn merge_risk(base: &RiskAssessment, extra: &RiskAssessment, label: &str) -> RiskAssessment {
    RiskAssessment {
        level: base.level.max(extra.level),
        requires_approval: base.requires_approval || extra.requires_approval,
        reasons: base
            .reasons
            .iter()
            .cloned()
            .chain(extra.reasons.iter().map(|r| format!("{label}: {r}")))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use visualops_core::mock::MockPerceptor;
    use visualops_core::RiskLevel;

    /// Executor that counts invocations, so we can assert a gated action never
    /// reaches the OS.
    struct CountingExecutor(Arc<AtomicUsize>);
    impl ActionExecutor for CountingExecutor {
        fn perform(
            &self,
            _t: &Target,
            _n: &SceneNode,
            _a: SemanticAction,
            _arg: Option<&str>,
        ) -> visualops_core::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn engine_with_counter() -> (Engine, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::new(CountingExecutor(calls.clone()));
        let eng = Engine::new(perceptor, exec, Target { pid: 1363, window_id: 105 }).unwrap();
        (eng, calls)
    }

    type RecordedCall = (String, SemanticAction, Option<String>);

    /// Executor that records every `(id, action, argument)` it receives, so we
    /// can assert exactly what the engine resolved an action to.
    struct RecordingExecutor(Arc<Mutex<Vec<RecordedCall>>>);
    impl ActionExecutor for RecordingExecutor {
        fn perform(
            &self,
            _t: &Target,
            n: &SceneNode,
            a: SemanticAction,
            arg: Option<&str>,
        ) -> visualops_core::Result<()> {
            self.0.lock().unwrap().push((n.id.clone(), a, arg.map(str::to_owned)));
            Ok(())
        }
    }

    fn engine_with_recorder() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let eng = Engine::new(perceptor, exec, Target { pid: 1363, window_id: 105 }).unwrap();
        (eng, calls)
    }

    /// An id from the affordance graph that exposes `Drag` and is *not* risk
    /// gated, so the executor actually runs (rows/cells in the notes fixture).
    fn non_gated_drag_source(eng: &Engine) -> String {
        eng.query_affordances(SemanticAction::Drag)
            .into_iter()
            .find(|id| !eng.affordance_graph().affordances[id].risk.requires_approval)
            .expect("a non-gated draggable source in the notes fixture")
    }

    fn id_for(eng: &Engine, query: &str) -> String {
        eng.find_element(query)
            .first()
            .map(|n| n.id.clone())
            .unwrap_or_else(|| panic!("no element for {query:?}"))
    }

    #[test]
    fn low_risk_click_proceeds_and_executes() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Nouvelle note");
        let entry = eng.click_element(&id, Some("create")).unwrap();
        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn high_risk_click_is_gated_then_approved() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        // 1. Denied pending approval — and the executor must NOT have run.
        let e1 = eng.click_element(&id, Some("delete")).unwrap();
        assert_eq!(e1.result, ActionResult::PendingApproval);
        assert_eq!(e1.risk.level, RiskLevel::High);
        assert!(e1.risk.requires_approval);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "executor must not run on a gated action"
        );

        // 2. Approve, retry — proceeds, executor called exactly once.
        eng.approve(&id).unwrap();
        let e2 = eng.click_element(&id, Some("approved")).unwrap();
        assert_eq!(e2.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // --- Audit #2: validated, one-shot, refresh-invalidated approvals --------

    #[test]
    fn approval_is_one_shot_consumed_by_act() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        eng.approve(&id).unwrap();
        assert_eq!(eng.click_element(&id, Some("1st")).unwrap().result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The grant authorised exactly one action: a second high-risk click on the
        // same element (re-resolved after the post-action refresh) gates again.
        let id2 = id_for(&eng, "Supprimer");
        let e2 = eng.click_element(&id2, Some("2nd")).unwrap();
        assert_eq!(e2.result, ActionResult::PendingApproval, "grant must not survive one use");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no second execution without re-approval");
    }

    #[test]
    fn approval_is_invalidated_by_refresh() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        eng.approve(&id).unwrap();
        eng.refresh().unwrap(); // scene re-perceived → the grant must be dropped

        let id2 = id_for(&eng, "Supprimer");
        let e = eng.click_element(&id2, Some("after refresh")).unwrap();
        assert_eq!(e.result, ActionResult::PendingApproval, "refresh invalidates approvals");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "executor never ran");
    }

    #[test]
    fn approve_rejects_unknown_and_non_gated_ids() {
        let (mut eng, calls) = engine_with_counter();

        // Unknown id → ElementNotFound; nothing is stored.
        let err = eng.approve("no_such_id").unwrap_err();
        assert!(matches!(err, VisualOpsError::ElementNotFound(_)));

        // A low-risk element (toolbar button) is not gated → error, nothing stored.
        let low = id_for(&eng, "Nouvelle note");
        assert!(eng.approve(&low).is_err(), "approving a non-gated id is rejected");

        // And because the bogus grants were rejected, the high-risk gate is intact:
        // "Supprimer" is still PendingApproval (no spurious approval leaked).
        let supprimer = id_for(&eng, "Supprimer");
        let e = eng.click_element(&supprimer, None).unwrap();
        assert_eq!(e.result, ActionResult::PendingApproval);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    // --- Audit #3: composite drag risk (max of source / drop target) ---------

    /// A purpose-built fixture for the composite-drag gate: the bundled Notes
    /// fixture has no node that is *both* draggable (Row/Cell) and high-risk with a
    /// bbox (its high-risk items are bbox-less menu items), so we mint a tiny tree
    /// with a harmless draggable row and a high-risk drop target that has a bbox.
    fn composite_drag_engine() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
        fn raw(
            ax_role: &str,
            label: Option<&str>,
            frame: Option<Bbox>,
            ax_actions: &[&str],
            children: Vec<visualops_core::RawAxNode>,
        ) -> visualops_core::RawAxNode {
            visualops_core::RawAxNode {
                ax_role: ax_role.into(),
                label: label.map(str::to_owned),
                help: None,
                value: None,
                ax_identifier: None,
                ax_actions: ax_actions.iter().map(|s| s.to_string()).collect(),
                frame,
                enabled: true,
                focused: false,
                children,
            }
        }
        let bb = |x: f64| Some(Bbox { x, y: 100.0, w: 50.0, h: 20.0 });
        // Row under a Table → draggable (the Table is an ancestor drop container).
        let row = raw("AXRow", Some("note-a"), bb(10.0), &["press"], vec![]);
        let table = raw("AXTable", None, bb(10.0), &[], vec![row]);
        // High-risk drop target WITH a bbox (so drag_element can compute a drop).
        let danger = raw("AXButton", Some("Supprimer"), bb(200.0), &["press"], vec![]);
        let window = raw(
            "AXWindow",
            Some("W"),
            Some(Bbox { x: 0.0, y: 0.0, w: 400.0, h: 400.0 }),
            &[],
            vec![table, danger],
        );

        let calls = Arc::new(Mutex::new(Vec::new()));
        let perceptor = Box::new(MockPerceptor::new(
            vec![window],
            WindowRef { pid: 1, window_id: 1, app_name: "T".into(), title: "T".into() },
        ));
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let eng = Engine::new(perceptor, exec, Target { pid: 1, window_id: 1 }).unwrap();
        (eng, calls)
    }

    #[test]
    fn drag_onto_high_risk_target_is_gated_then_approvable() {
        let (mut eng, calls) = composite_drag_engine();
        let source = id_for(&eng, "note-a"); // low-risk draggable row
        let target = id_for(&eng, "Supprimer"); // high-risk drop target, has bbox

        // Precondition: source is harmless, target is the dangerous one.
        assert!(!eng.affordance_graph().affordances[&source].risk.requires_approval);
        assert!(eng.affordance_graph().affordances[&target].risk.requires_approval);

        // The gate fires on the TARGET's risk even though the source is low.
        let gated = eng.drag_element(&source, &target, Some("dangerous drop")).unwrap();
        assert_eq!(gated.result, ActionResult::PendingApproval, "high-risk drop target must gate");
        assert_eq!(gated.risk.level, RiskLevel::High, "effective risk is max(source, target)");
        assert!(gated.risk.requires_approval);
        assert!(
            gated.risk.reasons.iter().any(|r| r.contains("drop target") && r.to_lowercase().contains("supprimer")),
            "audit reason attributes the risk to the drop target: {:?}",
            gated.risk.reasons
        );
        assert!(calls.lock().unwrap().is_empty(), "gated drag never reaches the executor");

        // Approving the dangerous target (its own risk is high → approve accepts it)
        // clears the composite gate for exactly one drag.
        eng.approve(&target).unwrap();
        let ok = eng.drag_element(&source, &target, Some("approved drop")).unwrap();
        assert_eq!(ok.result, ActionResult::Success);
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1, "executor ran exactly once, on the source");
        assert_eq!(recorded[0].0, source);
        assert_eq!(recorded[0].1, SemanticAction::Drag);
    }

    // --- Audit #13: a destructive *typed value* gates a low-risk field --------

    #[test]
    fn destructive_typed_text_gates_low_risk_field_and_is_approvable() {
        let (mut eng, calls) = engine_with_counter();
        let field = id_for(&eng, "Corps de la note"); // low-risk, typeable text area
        assert!(
            !eng.affordance_graph().affordances[&field].risk.requires_approval,
            "the field itself is low-risk"
        );

        // Out of context, a low-risk field is NOT approvable (audit #2 still holds).
        assert!(eng.approve(&field).is_err(), "low-risk field not approvable out of context");

        // A destructive payload raises the gate even though the field is harmless.
        let gated = eng.type_into(&field, "supprimer tout", Some("danger")).unwrap();
        assert_eq!(gated.result, ActionResult::PendingApproval, "destructive text gates the field");
        assert_eq!(gated.risk.level, RiskLevel::High, "effective risk = max(field, text)");
        assert!(
            gated
                .risk
                .reasons
                .iter()
                .any(|r| r.contains("typed text") && r.to_lowercase().contains("supprimer")),
            "audit attributes the risk to the typed text: {:?}",
            gated.risk.reasons
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "gated type never reaches the executor");

        // The field is now the subject of a pending gate → approvable; type proceeds.
        eng.approve(&field).unwrap();
        let ok = eng.type_into(&field, "supprimer tout", Some("approved")).unwrap();
        assert_eq!(ok.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // One-shot: a second destructive type gates again (grant consumed + refresh).
        let regated = eng.type_into(&field, "supprimer tout", None).unwrap();
        assert_eq!(regated.result, ActionResult::PendingApproval);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Benign text into the same field is never gated.
        let benign = eng.type_into(&field, "bonjour", None).unwrap();
        assert_eq!(benign.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    // --- effective_risk in isolation (C2 refactor) --------------------------

    #[test]
    fn effective_risk_folds_drag_target_and_typed_text() {
        let (eng, _) = engine_with_counter();
        let low = RiskAssessment::low();
        let high = RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons: vec!["matched keyword: supprimer".into()],
        };

        // Low source dragged onto a high-risk target → effective High, target gated.
        let co = CoTarget { id: "tgt".into(), risk: high.clone() };
        let (eff, gated) =
            eng.effective_risk("src", SemanticAction::Drag, None, &low, Some(&co));
        assert_eq!(eff.level, RiskLevel::High);
        assert!(eff.requires_approval);
        assert_eq!(gated, vec!["tgt".to_string()]);
        assert!(eff.reasons.iter().any(|r| r.contains("drop target")));

        // Destructive text into a low-risk field → effective High, field gated.
        let (eff2, gated2) =
            eng.effective_risk("field", SemanticAction::Type, Some("supprimer tout"), &low, None);
        assert_eq!(eff2.level, RiskLevel::High);
        assert!(eff2.requires_approval);
        assert_eq!(gated2, vec!["field".to_string()]);
        assert!(eff2.reasons.iter().any(|r| r.contains("typed text")));

        // Benign: low source, no co-target, benign text → Low, no gate.
        let (eff3, gated3) = eng.effective_risk("x", SemanticAction::Click, None, &low, None);
        assert!(!eff3.requires_approval);
        assert_eq!(eff3.level, RiskLevel::Low);
        assert!(gated3.is_empty());
    }

    #[test]
    fn unavailable_action_is_an_error() {
        let (mut eng, calls) = engine_with_counter();
        // A button has no Type affordance.
        let id = id_for(&eng, "Nouvelle note");
        let err = eng.type_into(&id, "x", None).unwrap_err();
        assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn every_attempt_is_audited() {
        let (mut eng, _c) = engine_with_counter();
        let _ = eng.click_element(&id_for(&eng, "Supprimer"), None); // gated
        let _ = eng.click_element(&id_for(&eng, "Nouvelle note"), None); // ok
        assert_eq!(eng.trace().len(), 2);
    }

    #[test]
    fn drag_records_target_bbox_centre() {
        let (mut eng, calls) = engine_with_recorder();
        let source = non_gated_drag_source(&eng);
        let target = id_for(&eng, "Nouvelle note");

        // Expected drop point = centre of the *target* node's bbox, formatted
        // exactly as the engine formats it.
        let bbox = eng.scene_graph().get(&target).unwrap().bbox.unwrap();
        let expected = format!("{},{}", bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);

        let entry = eng.drag_element(&source, &target, Some("reorder")).unwrap();

        // The executor saw exactly (source, Drag, Some("x,y")).
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            (source.clone(), SemanticAction::Drag, Some(expected))
        );

        // The audit entry describes the drag on the source and is in the trace.
        assert_eq!(entry.action, SemanticAction::Drag);
        assert_eq!(entry.target_id, source);
        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(eng.trace().len(), 1);
        assert_eq!(eng.trace()[0].action, SemanticAction::Drag);
    }

    #[test]
    fn drag_unknown_target_is_an_error() {
        let (mut eng, calls) = engine_with_recorder();
        let source = non_gated_drag_source(&eng);

        let err = eng.drag_element(&source, "no_such_target", None).unwrap_err();
        assert!(matches!(err, VisualOpsError::ElementNotFound(_)));

        // No executor call, no audit entry: the failure is structural, pre-act.
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(eng.trace().len(), 0);
    }

    #[test]
    fn drag_source_without_affordance_is_unavailable() {
        let (mut eng, calls) = engine_with_recorder();
        // A toolbar button exposes Click, never Drag; the target has a bbox.
        let source = id_for(&eng, "Nouvelle note");
        let target = id_for(&eng, "Nouvelle note");

        let err = eng.drag_element(&source, &target, None).unwrap_err();
        assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(eng.trace().len(), 0);
    }

    // --- WP-J / J1: get_scene_graph projection ------------------------------

    #[test]
    fn compact_view_omits_heavy_fields_and_keeps_n_children() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Compact, false);
        let id = id_for(&eng, "Nouvelle note");
        let node = v["nodes"].get(id.as_str()).expect("compact node present");

        // Heavy/derivable AX fields are dropped.
        for dropped in ["ax_role", "help", "ax_actions", "ax_identifier", "last_seen_ms", "children", "confidence", "source"] {
            assert!(node.get(dropped).is_none(), "compact node must drop {dropped}");
        }
        // Kept fields, with children collapsed to a count.
        assert!(node.get("n_children").is_some(), "n_children kept");
        assert!(node.get("bbox").is_some(), "bbox kept");
        assert_eq!(node["role"], json!("button"));
    }

    #[test]
    fn compact_view_is_materially_smaller_than_full() {
        let (eng, _) = engine_with_counter();
        let full = eng.scene_graph_view(SceneView::Full, false);
        let compact = eng.scene_graph_view(SceneView::Compact, false);
        let full_len = serde_json::to_string(&full).unwrap().len();
        let compact_len = serde_json::to_string(&compact).unwrap().len();
        // Visible with `cargo test -- --nocapture`; the real before/after note.
        eprintln!(
            "get_scene_graph fixture size — full: {full_len} B, compact: {compact_len} B (×{:.1} lighter)",
            full_len as f64 / compact_len.max(1) as f64
        );
        assert!(compact_len < full_len, "compact ({compact_len}) must be smaller than full ({full_len})");
    }

    #[test]
    fn full_view_is_byte_identical_to_raw_scene_graph() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Full, false);
        let raw = serde_json::to_value(eng.scene_graph()).unwrap();
        assert_eq!(v, raw, "full view is the unchanged escape hatch");
    }

    #[test]
    fn summary_view_has_counts_and_roots_but_no_nodes() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Summary, false);
        assert!(v.get("nodes").is_none(), "summary carries no per-node list");
        let n_nodes = v["n_nodes"].as_u64().expect("n_nodes");
        let n_actionable = v["n_actionable"].as_u64().expect("n_actionable");
        assert!(n_nodes >= 1);
        assert!(v["roots"].is_array());
        assert!(v["counts_by_role"].is_object());
        assert!(v["window"].is_object());
        assert!(n_actionable <= n_nodes, "actionable is a subset");
        assert!(n_actionable >= 1, "at least the toolbar button is actionable");
    }

    #[test]
    fn actionable_only_drops_latent_menu_items() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer"); // latent AXMenuItem (no bbox)
        let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen toolbar button
        let v = eng.scene_graph_view(SceneView::Compact, true);
        assert!(v["nodes"].get(supprimer.as_str()).is_none(), "latent node dropped by actionable_only");
        assert!(v["nodes"].get(nouvelle.as_str()).is_some(), "on-screen node kept");
    }

    // --- WP-J / J2: latent affordance filtering -----------------------------

    #[test]
    fn query_affordances_excludes_latent_by_default_but_include_latent_keeps_them() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer"); // latent menu item exposing Click
        let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen button

        let default = eng.query_affordances(SemanticAction::Click);
        assert!(!default.contains(&supprimer), "latent menu item filtered from default listing");
        assert!(default.contains(&nouvelle), "on-screen button still listed");

        let all = eng.query_affordances_filtered(SemanticAction::Click, true);
        assert!(all.contains(&supprimer), "include_latent surfaces the latent item");
        assert!(all.len() > default.len(), "include_latent is a strict superset here");
    }

    #[test]
    fn get_affordances_view_filters_latent_but_keeps_it_under_include_latent() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer");
        let filtered = eng.affordances_view(false);
        assert!(filtered["affordances"].get(supprimer.as_str()).is_none(), "latent omitted by default");
        let all = eng.affordances_view(true);
        assert!(all["affordances"].get(supprimer.as_str()).is_some(), "include_latent keeps it");
    }

    #[test]
    fn find_element_and_gating_still_reach_latent_nodes() {
        // CRITICAL (WP-J): filtering the *listing* must NOT hide latent nodes from
        // find_element, nor stop the risk gate from acting on them by id.
        let (mut eng, calls) = engine_with_counter();
        assert!(!eng.find_element("Supprimer").is_empty(), "find_element still locates the latent item");

        let supprimer = id_for(&eng, "Supprimer");
        // click_element by id reaches the gate (PendingApproval), not ActionUnavailable,
        // and the executor never runs.
        let e = eng.click_element(&supprimer, Some("delete")).unwrap();
        assert_eq!(e.result, ActionResult::PendingApproval);
        assert!(e.risk.requires_approval);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "gated action never reaches the executor");
    }

    #[test]
    fn top_level_menu_opener_listed_but_deep_submenu_item_filtered() {
        let (mut eng, calls) = engine_with_counter();
        // "Édition" is a top-level menu opener: direct child of the menubar root,
        // bbox null. "Supprimer" is a deep item under a closed Menu, bbox null.
        let edition = id_for(&eng, "Édition");
        let supprimer = id_for(&eng, "Supprimer");

        // Both are geometrically latent (no bbox) — only structure differs.
        assert!(eng.scene_graph().get(&edition).unwrap().bbox.is_none());
        assert!(eng.scene_graph().get(&supprimer).unwrap().bbox.is_none());

        // The exemption is STRUCTURAL, not role-based: Édition's parent IS the
        // menubar root; Supprimer's parent is a closed Menu, not the root.
        let menubar_root = eng
            .scene_graph()
            .roots
            .iter()
            .find(|id| eng.scene_graph().get(id).map(|n| n.role == Role::MenuBar).unwrap_or(false))
            .cloned()
            .expect("menubar root in roots");
        assert_eq!(
            eng.scene_graph().get(&edition).unwrap().parent.as_deref(),
            Some(menubar_root.as_str()),
            "Édition sits directly under the menubar root"
        );
        assert_ne!(
            eng.scene_graph().get(&supprimer).unwrap().parent.as_deref(),
            Some(menubar_root.as_str()),
            "Supprimer sits under a closed Menu, not the menubar root"
        );

        // query_affordances("click"): the opener is listed, the deep item is not.
        let click = eng.query_affordances(SemanticAction::Click);
        assert!(click.contains(&edition), "top-level menu opener listed despite null bbox");
        assert!(!click.contains(&supprimer), "deep submenu item stays filtered (phantom)");

        // include_latent brings back the deep phantom too (superset).
        let all = eng.query_affordances_filtered(SemanticAction::Click, true);
        assert!(all.contains(&edition));
        assert!(all.contains(&supprimer));

        // get_affordances mirrors the same exemption.
        let aff = eng.affordances_view(false);
        assert!(aff["affordances"].get(edition.as_str()).is_some(), "opener kept in get_affordances");
        assert!(aff["affordances"].get(supprimer.as_str()).is_none(), "deep item omitted in get_affordances");

        // find_element still locates both; the gate still acts on the deep item by id.
        assert!(!eng.find_element("Édition").is_empty());
        assert!(!eng.find_element("Supprimer").is_empty());
        let gated = eng.click_element(&supprimer, Some("delete")).unwrap();
        assert_eq!(gated.result, ActionResult::PendingApproval);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "exemption never opens the gate");
    }
}
