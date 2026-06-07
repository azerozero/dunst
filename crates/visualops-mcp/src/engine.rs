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
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, GraphDiff, Perceptor,
    RiskAssessment, RiskLevel, SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError,
    WindowRef,
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
        let risk: RiskAssessment = aff.risk.clone();
        let approved = self.approvals.contains(id);

        // Gate: high-risk actions need prior approval.
        if risk.requires_approval && !approved {
            let entry = self.record(id, action, argument, reasoning, risk, ActionResult::PendingApproval, GraphDiff::default());
            return Ok(entry);
        }

        // Execute, then re-perceive and diff.
        let exec = self.executor.perform(&self.target, &node, action, argument);
        let result = match &exec {
            Ok(()) => ActionResult::Success,
            Err(_) => ActionResult::Failed,
        };
        let _ = self.refresh();
        let diff = self.diff_since();
        let entry = self.record(id, action, argument, reasoning, risk, result, diff);
        Ok(entry)
    }

    fn record(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        risk: RiskAssessment,
        result: ActionResult,
        graph_diff: GraphDiff,
    ) -> AuditEntry {
        let entry = AuditEntry {
            ts_ms: visualops_core::now_ms(),
            target_id: id.to_string(),
            action,
            argument: argument.map(str::to_owned),
            risk,
            reasoning: reasoning.map(str::to_owned),
            result,
            graph_diff,
        };
        self.trace.push(entry.clone());
        entry
    }

    // --- audit --------------------------------------------------------------

    pub fn trace(&self) -> &[AuditEntry] {
        &self.trace
    }

    pub fn export_trace(&self) -> visualops_core::Result<String> {
        Ok(serde_json::to_string_pretty(&self.trace)?)
    }
}

/// True if any element in the graph is high-risk (used by demos/reports).
pub fn has_high_risk(g: &AffordanceGraph) -> bool {
    g.affordances.values().any(|a| a.risk.level == RiskLevel::High)
}
