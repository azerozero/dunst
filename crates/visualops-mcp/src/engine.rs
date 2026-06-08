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
    RiskLevel, Role, SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
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

    /// IDs whose affordance offers `action`. WP-J/J2: latent (off-screen /
    /// zero-bbox) nodes — e.g. collapsed-menu items — are omitted by default so
    /// the agent isn't handed phantom targets. The gated action path is
    /// unaffected: it resolves ids against the graph, not this listing.
    ///
    /// Ergonomic default over [`query_affordances_filtered`](Self::query_affordances_filtered);
    /// the MCP server calls the latter directly, so in the binary this wrapper is
    /// exercised only by callers/tests that want the filtered listing.
    #[allow(dead_code)]
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
        let window_rect = self.window_rect();
        let g = self.scene_graph();
        self.affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .filter(|a| {
                include_latent
                    || g.get(&a.id).map(|n| node_on_screen(n, window_rect)).unwrap_or(false)
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
        let window_rect = self.window_rect();
        let g = self.scene_graph();
        let mut map = serde_json::Map::new();
        for (id, aff) in &ag.affordances {
            if g.get(id).map(|n| node_on_screen(n, window_rect)).unwrap_or(false) {
                map.insert(id.clone(), serde_json::to_value(aff).unwrap_or(Value::Null));
            }
        }
        json!({ "affordances": Value::Object(map) })
    }

    /// The window's on-screen rect, read from the `Window` node's bbox (the
    /// scene graph's [`WindowRef`] carries no geometry). `None` when no window
    /// node has a bbox — then [`node_on_screen`]'s off-window test is skipped.
    fn window_rect(&self) -> Option<Bbox> {
        self.scene_graph()
            .nodes
            .values()
            .find(|n| n.role == Role::Window)
            .and_then(|n| n.bbox)
    }

    /// WP-J/J1: the scene graph under a projection `view`, optionally limited to
    /// actionable nodes. `Full` without `actionable_only` is byte-for-byte the
    /// old `get_scene_graph` payload (the escape hatch).
    pub fn scene_graph_view(&self, view: SceneView, actionable_only: bool) -> Value {
        let g = self.scene_graph();
        let window_rect = self.window_rect();
        match view {
            SceneView::Full if !actionable_only => serde_json::to_value(g).unwrap_or(Value::Null),
            SceneView::Full => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if node_actionable(n, window_rect) {
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
                    if actionable_only && !node_actionable(n, window_rect) {
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
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                let mut n_actionable = 0usize;
                for n in g.nodes.values() {
                    *counts.entry(role_key(n.role)).or_insert(0) += 1;
                    if node_actionable(n, window_rect) {
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

    /// Whitelist a high-risk element so the next action on it proceeds.
    pub fn approve(&mut self, id: &str) {
        self.approvals.insert(id.to_string());
    }

    // --- action tools -------------------------------------------------------

    pub fn click_element(&mut self, id: &str, reasoning: Option<&str>) -> visualops_core::Result<AuditEntry> {
        self.act(id, SemanticAction::Click, None, reasoning)
    }

    pub fn type_into(&mut self, id: &str, text: &str, reasoning: Option<&str>) -> visualops_core::Result<AuditEntry> {
        self.act(id, SemanticAction::Type, Some(text), reasoning)
    }

    pub fn hover_probe(&mut self, id: &str) -> visualops_core::Result<AuditEntry> {
        self.act(id, SemanticAction::Hover, None, Some("hover probe"))
    }

    /// Drag `source_id` onto `target_id`. The drop point handed to the executor
    /// is the **target** node's bbox centre in screen coordinates, formatted as
    /// `"x,y"` (the frozen WP-F drag mini-contract). This is a thin wrapper over
    /// the gated [`act`] path — `act` checks the *source* exposes `Drag`, gates
    /// on risk, runs the executor, re-perceives, diffs and audits.
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
        self.act(source_id, SemanticAction::Drag, Some(&format!("{x},{y}")), reasoning)
    }

    /// The gated action path. Always returns an [`AuditEntry`] describing the
    /// outcome (also appended to the trace); only structural problems
    /// (unknown id / unavailable action) are `Err`.
    fn act(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
    ) -> visualops_core::Result<AuditEntry> {
        let node = self
            .scene_graph()
            .get(id)
            .cloned()
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
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
        let approved = self.approvals.contains(id);

        // Build the audit record once; the two outcome paths only differ in
        // `result` and `graph_diff` (applied via struct update below).
        let base = AuditEntry {
            ts_ms: visualops_core::now_ms(),
            target_id: id.to_string(),
            action,
            argument: argument.map(str::to_owned),
            risk: aff.risk.clone(),
            reasoning: reasoning.map(str::to_owned),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        };

        // Gate: high-risk actions need prior approval. Note the executor is
        // never invoked on this path.
        if base.risk.requires_approval && !approved {
            return Ok(self.push_entry(base));
        }

        // Execute, then re-perceive and diff.
        let result = match self.executor.perform(&self.target, &node, action, argument) {
            Ok(()) => ActionResult::Success,
            Err(_) => ActionResult::Failed,
        };
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
    #[allow(dead_code)]
    pub fn trace(&self) -> &[AuditEntry] {
        &self.trace
    }

    pub fn export_trace(&self) -> visualops_core::Result<String> {
        Ok(serde_json::to_string_pretty(&self.trace)?)
    }
}

// --- WP-J / J2: latent (non-actionable) node geometry -----------------------

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

/// J1 actionability: on-screen **and** enabled (what `actionable_only` keeps and
/// what `summary.n_actionable` counts).
fn node_actionable(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    node.enabled && node_on_screen(node, window_rect)
}

/// JSON string for a node's normalised [`Role`] (e.g. `"menu_item"`), reusing the
/// serde rename so histogram keys match the scene-graph encoding.
fn role_key(role: Role) -> String {
    match serde_json::to_value(role) {
        Ok(Value::String(s)) => s,
        _ => "unknown".to_string(),
    }
}

/// WP-J/J1 compact projection of one node: keep only the agent-facing fields and
/// drop the heavy/derivable AX detail (`ax_role`, `help`, `ax_actions`,
/// `ax_identifier`, `last_seen_ms`), collapsing `children` to a count.
fn compact_node(n: &SceneNode) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("id".into(), json!(n.id));
    o.insert("role".into(), json!(role_key(n.role)));
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

/// True if any element in the graph is high-risk (utility for demos/reports).
#[allow(dead_code)]
pub fn has_high_risk(g: &AffordanceGraph) -> bool {
    g.affordances.values().any(|a| a.risk.level == RiskLevel::High)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use visualops_core::mock::MockPerceptor;

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
        eng.approve(&id);
        let e2 = eng.click_element(&id, Some("approved")).unwrap();
        assert_eq!(e2.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
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
}
