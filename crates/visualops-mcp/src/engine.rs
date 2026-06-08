//! The VisualOps engine — the runtime-agnostic service behind the MCP tools.
//!
//! Holds a [`Perceptor`] + [`ActionExecutor`] + [`RiskEngine`], maintains the
//! current/previous [`SceneGraph`] and [`AffordanceGraph`], enforces
//! risk-based approval gating, and records an [`AuditEntry`] per action.
//!
//! This struct is transport-independent: the MCP server (`serve`) and the CLI
//! `demo` both drive the same methods.

use std::collections::BTreeSet;

use visualops_core::{
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, GraphDiff, Perceptor, RiskLevel,
    SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError, WindowRef,
};
use visualops_graph::{audit, derive_affordances, scene, RiskEngine};

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

    /// IDs whose affordance offers `action`.
    pub fn query_affordances(&self, action: SemanticAction) -> Vec<String> {
        self.affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .map(|a| a.id.clone())
            .collect()
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
}
