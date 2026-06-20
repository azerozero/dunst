use std::time::{Duration, Instant};

use dunst_core::{
    ActionResult, AuditEntry, GraphDiff, RiskAssessment, RiskLevel, Role, SceneGraph, SceneNode,
    SemanticAction, VisualOpsError,
};

use super::{is_element_not_found, normalize_match, retry_user_active_guard, Engine};

const TYPE_VERIFY_SETTLE_TIMEOUT: Duration = Duration::from_millis(1_000);
const TYPE_VERIFY_POLL_INTERVAL: Duration = Duration::from_millis(80);
const CLICK_VERIFY_SETTLE_TIMEOUT: Duration = Duration::from_millis(1_000);
const CLICK_VERIFY_POLL_INTERVAL: Duration = Duration::from_millis(80);

/// A second risk-bearing participant in an action — the **drop target** of a drag
/// (audit #3). Carried into [`Engine::act`] so the gate can combine its risk with
/// the dragged element's.
#[derive(Clone)]
pub(super) struct CoTarget {
    pub(super) id: String,
    pub(super) risk: RiskAssessment,
}

#[derive(Clone, Debug)]
struct RemovalExpectation {
    label: Option<String>,
    id: String,
    before_count: usize,
}

#[derive(Clone, Debug)]
struct CheckboxExpectation {
    id: String,
    before_value: Option<String>,
}

impl Engine {
    /// Compute an action's **effective risk** and the set of ids whose approval
    /// clears its gate. Folds a composite drag's drop target (audit #3), a
    /// destructive typed payload (audit #13), and foreground-affecting action
    /// side effects into the source element's own risk via [`merge_risk`]. Pure
    /// over its inputs and `self.risk` — no scene mutation — so it is
    /// unit-testable in isolation (the `effective_risk_*` tests).
    ///
    /// Returns `(effective, gated_ids)`: `effective.requires_approval` decides
    /// whether the gate fires; `gated_ids` lists every high-risk participant that
    /// must be approved (the element, the drop target, or the typed-into field).
    pub(super) fn effective_risk(
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
        if let Some(ar) = action_side_effect_risk(action) {
            effective = merge_risk(&effective, &ar, "action");
        }

        // Every high-risk participant must be approved to clear the gate: the
        // element itself, a composite drag's drop target, the typed-into field, or
        // the element whose action has an intrinsic side effect such as foregrounding.
        let mut gated_ids: Vec<String> = Vec::new();
        if source_risk.requires_approval {
            gated_ids.push(id.to_string());
        }
        if let Some(co) = co_target {
            if co.risk.requires_approval {
                gated_ids.push(co.id.clone());
            }
        }
        if text_risk
            .as_ref()
            .map(|r| r.requires_approval)
            .unwrap_or(false)
            && !gated_ids.iter().any(|g| g == id)
        {
            gated_ids.push(id.to_string());
        }
        if action_side_effect_risk(action)
            .as_ref()
            .map(|r| r.requires_approval)
            .unwrap_or(false)
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
    pub(super) fn act_refreshing_missing(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        co_target: Option<CoTarget>,
    ) -> dunst_core::Result<AuditEntry> {
        match self.act(id, action, argument, reasoning, co_target.clone()) {
            Err(err) if is_element_not_found(&err) => {
                self.refresh()?;
                self.act(id, action, argument, reasoning, co_target)
            }
            other => other,
        }
    }

    pub(super) fn act(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        co_target: Option<CoTarget>,
    ) -> dunst_core::Result<AuditEntry> {
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
        let approved =
            !gated_ids.is_empty() && gated_ids.iter().all(|g| self.approvals.contains(g));

        // Build the audit record once; the two outcome paths only differ in
        // `result` and `graph_diff` (applied via struct update below).
        let base = AuditEntry {
            ts_ms: dunst_core::now_ms(),
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

        let removal_expectation = removal_expectation(action, &node, self.scene_graph());
        let checkbox_expectation = checkbox_expectation(action, &node);

        // Execute, then re-perceive and diff.
        let executor_result = match retry_user_active_guard(|| {
            self.executor.perform(&self.target, &node, action, argument)
        }) {
            Ok(()) => ActionResult::Success,
            Err(_) => ActionResult::Failed,
        };
        // One-shot consumption (audit #2): a grant authorises exactly one
        // successful action; drop it (and clear any pending-gate marker) so a
        // repeat re-gates. (`refresh` below also clears all grants — this keeps
        // the semantics explicit and independent of refresh ordering.)
        if executor_result == ActionResult::Success {
            for g in &gated_ids {
                self.approvals.remove(g);
                self.pending_gate_ids.remove(g);
            }
        }
        let _ = self.refresh();
        let mut graph_diff = self.diff_since();
        let mut result = verified_action_result(
            &executor_result,
            action,
            id,
            argument,
            &graph_diff,
            self.scene_graph().get(id),
        );
        if executor_result == ActionResult::Success
            && result == ActionResult::Success
            && removal_expectation.as_ref().is_some_and(|expectation| {
                !removal_expectation_satisfied(expectation, &graph_diff, self.scene_graph())
            })
        {
            result = ActionResult::Failed;
        }
        if executor_result == ActionResult::Success
            && result == ActionResult::Success
            && checkbox_expectation.as_ref().is_some_and(|expectation| {
                !checkbox_expectation_satisfied(expectation, self.scene_graph())
            })
        {
            let started = Instant::now();
            while started.elapsed() < CLICK_VERIFY_SETTLE_TIMEOUT {
                std::thread::sleep(CLICK_VERIFY_POLL_INTERVAL);
                if self.refresh().is_err() {
                    break;
                }
                graph_diff = self.diff_since();
                if checkbox_expectation.as_ref().is_some_and(|expectation| {
                    checkbox_expectation_satisfied(expectation, self.scene_graph())
                }) {
                    break;
                }
            }
            if checkbox_expectation.as_ref().is_some_and(|expectation| {
                !checkbox_expectation_satisfied(expectation, self.scene_graph())
            }) {
                result = ActionResult::Failed;
            }
        }
        if executor_result == ActionResult::Success
            && result == ActionResult::Failed
            && matches!(action, SemanticAction::Type)
            && argument.is_some_and(|arg| !arg.is_empty())
        {
            let started = Instant::now();
            while started.elapsed() < TYPE_VERIFY_SETTLE_TIMEOUT {
                std::thread::sleep(TYPE_VERIFY_POLL_INTERVAL);
                if self.refresh().is_err() {
                    break;
                }
                graph_diff = self.diff_since();
                result = verified_action_result(
                    &executor_result,
                    action,
                    id,
                    argument,
                    &graph_diff,
                    self.scene_graph().get(id),
                );
                if result == ActionResult::Success {
                    break;
                }
            }
        }
        Ok(self.push_entry(AuditEntry {
            result,
            graph_diff,
            ..base
        }))
    }
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

fn action_side_effect_risk(action: SemanticAction) -> Option<RiskAssessment> {
    match action {
        SemanticAction::Raise => Some(RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons: vec!["raises target window to the foreground".to_string()],
        }),
        _ => None,
    }
}

fn verified_action_result(
    executor_result: &ActionResult,
    action: SemanticAction,
    id: &str,
    argument: Option<&str>,
    graph_diff: &GraphDiff,
    current_node: Option<&SceneNode>,
) -> ActionResult {
    if *executor_result != ActionResult::Success {
        return executor_result.clone();
    }

    match action {
        SemanticAction::Type if argument.is_some_and(|arg| !arg.is_empty()) => {
            if typed_target_value_matches_expected(
                id,
                argument.unwrap_or_default(),
                graph_diff,
                current_node,
            ) {
                ActionResult::Success
            } else {
                ActionResult::Failed
            }
        }
        _ => ActionResult::Success,
    }
}

fn removal_expectation(
    action: SemanticAction,
    node: &SceneNode,
    graph: &SceneGraph,
) -> Option<RemovalExpectation> {
    if action != SemanticAction::Click || !click_should_remove_target(node) {
        return None;
    }

    let before_count = node
        .label
        .as_deref()
        .map(|label| count_nodes_with_label(graph, label))
        .unwrap_or(usize::from(graph.get(&node.id).is_some()));

    Some(RemovalExpectation {
        label: node.label.clone(),
        id: node.id.clone(),
        before_count,
    })
}

fn click_should_remove_target(node: &SceneNode) -> bool {
    if node.id.starts_with("btn_remove_") {
        return true;
    }
    node.label
        .as_deref()
        .map(normalize_match)
        .is_some_and(|label| label.starts_with("remove "))
}

fn removal_expectation_satisfied(
    expectation: &RemovalExpectation,
    graph_diff: &GraphDiff,
    graph: &SceneGraph,
) -> bool {
    if graph_diff.changes.iter().any(|change| {
        matches!(
            change,
            dunst_core::NodeChange::Removed { id, .. } if id == &expectation.id
        )
    }) {
        return true;
    }

    match expectation.label.as_deref() {
        Some(label) => count_nodes_with_label(graph, label) < expectation.before_count,
        None => graph.get(&expectation.id).is_none(),
    }
}

fn count_nodes_with_label(graph: &SceneGraph, label: &str) -> usize {
    graph
        .nodes
        .values()
        .filter(|node| node.label.as_deref() == Some(label))
        .count()
}

fn checkbox_expectation(action: SemanticAction, node: &SceneNode) -> Option<CheckboxExpectation> {
    if action != SemanticAction::Click || node.role != Role::Checkbox {
        return None;
    }
    Some(CheckboxExpectation {
        id: node.id.clone(),
        before_value: node.value.clone(),
    })
}

fn checkbox_expectation_satisfied(expectation: &CheckboxExpectation, graph: &SceneGraph) -> bool {
    graph
        .get(&expectation.id)
        .map(|node| node.value != expectation.before_value)
        .unwrap_or(false)
}

pub(super) fn typed_target_value_matches_expected(
    id: &str,
    expected: &str,
    graph_diff: &GraphDiff,
    current_node: Option<&SceneNode>,
) -> bool {
    current_node
        .and_then(|node| node.value.as_deref().or(node.label.as_deref()))
        .is_some_and(|value| value == expected)
        || graph_diff.changes.iter().any(|change| {
            matches!(
                change,
                dunst_core::NodeChange::Changed { id: changed_id, field, after, .. }
                    if changed_id == id && matches!(field.as_str(), "value" | "label") && after == expected
            )
        })
}
