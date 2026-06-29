use super::*;

pub(super) const MAX_BATCH_PICKS: usize = 64;
const MAX_RESCANS: u32 = 8;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SelectionPlan {
    pub steps: Vec<SelectionStep>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SelectionStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub choice_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub op: SelectionOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_after: Option<ExpectedState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Bbox>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ExpectedState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SelectionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionOp {
    Select,
    Deselect,
    SetText,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyOutcome {
    pub status: ApplyStatus,
    pub batch_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_risk: Option<RiskLevel>,
    #[serde(rename = "preview", skip_serializing_if = "Vec::is_empty")]
    pub pending_preview: Vec<BatchPreviewStep>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ui_epoch: String,
    pub rescans: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<BatchVerify>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remaining_steps: Vec<SelectionStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    PendingApproval,
    Applied,
    PartiallyApplied,
    Refused,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BatchPreviewStep {
    pub choice_id: String,
    pub op: SelectionOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub risk: RiskLevel,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StepResult {
    pub choice_id: String,
    pub op: SelectionOp,
    pub result: StepResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<ResolvedBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepResultStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBy {
    Id,
    LabelAfterRescan,
    BboxAfterRescan,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BatchVerify {
    pub ok: bool,
    pub checks: Vec<VerifyCheck>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyCheck {
    pub choice_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<SelectionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<SelectionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_value: Option<String>,
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub(super) struct BatchApprovalContext {
    pub(super) batch_id: String,
    pub(super) remaining_budget: usize,
    pub(super) expected_epoch: String,
}

#[derive(Clone)]
struct ResolvedChoice {
    choice: Choice,
    resolved_by: ResolvedBy,
}

impl Engine {
    pub fn apply_selections(
        &mut self,
        plan: SelectionPlan,
        expected_epoch: &str,
    ) -> dunst_core::Result<ApplyOutcome> {
        validate_plan(&plan, expected_epoch)?;
        let current = self.current_ui_epoch_fingerprint();
        if current != expected_epoch {
            return Err(DunstError::Execution(
                "stale UI epoch: expected_epoch no longer matches the current target state; call enumerate_choices again before mutating"
                    .into(),
            ));
        }

        let batch_id =
            raw_apply_selections_target_id(&selection_plan_hash(&plan), plan.steps.len());
        let initial_model = self.enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 500,
        })?;
        if plan.steps.iter().all(|step| {
            resolve_step(step, &initial_model).is_none()
                && step.label.as_deref().is_none_or(str::is_empty)
                && step.bbox.is_none()
        }) {
            return Ok(ApplyOutcome {
                status: ApplyStatus::Refused,
                batch_id,
                approval_hint: None,
                max_risk: None,
                pending_preview: Vec::new(),
                ui_epoch: current,
                rescans: 0,
                steps: Vec::new(),
                verify: None,
                remaining_steps: plan.steps,
                reason: Some("unresolvable_step".to_string()),
            });
        }
        let preview = self.preview_selection_plan(&plan, &initial_model);
        let max_risk = preview
            .iter()
            .map(|step| step.risk)
            .max()
            .unwrap_or(RiskLevel::Low);
        let batch_risk = RiskAssessment {
            level: max_risk,
            requires_approval: true,
            reasons: vec![format!(
                "batch choice selection requires one operator approval for {} step(s)",
                plan.steps.len()
            )],
        };

        if self
            .gate_raw_input(
                &batch_id,
                SemanticAction::Click,
                Some(format!("apply {} selection step(s)", plan.steps.len())),
                Some("apply choice selections as one approved batch"),
                batch_risk,
            )
            .is_some()
        {
            return Ok(ApplyOutcome {
                status: ApplyStatus::PendingApproval,
                batch_id,
                approval_hint: Some(
                    "operator must run approve(batch_id); approve authorizes the whole batch once"
                        .to_string(),
                ),
                max_risk: Some(max_risk),
                pending_preview: preview,
                ui_epoch: String::new(),
                rescans: 0,
                steps: Vec::new(),
                verify: None,
                remaining_steps: Vec::new(),
                reason: None,
            });
        }

        self.begin_internal_batch_context(
            batch_id.clone(),
            plan.steps.len().min(MAX_BATCH_PICKS),
            expected_epoch.to_string(),
        );
        let outcome =
            self.execute_selection_batch(plan, batch_id.clone(), expected_epoch, initial_model);
        self.clear_internal_batch_context();
        self.clear_raw_approval(&batch_id);
        outcome
    }

    pub(super) fn begin_internal_batch_context(
        &mut self,
        batch_id: String,
        remaining_budget: usize,
        expected_epoch: String,
    ) {
        self.active_batch = Some(BatchApprovalContext {
            batch_id,
            remaining_budget,
            expected_epoch,
        });
    }

    pub(super) fn clear_internal_batch_context(&mut self) {
        self.active_batch = None;
    }

    pub(super) fn batch_context_allows_mutation(&self) -> bool {
        self.active_batch.as_ref().is_some_and(|ctx| {
            ctx.remaining_budget > 0 && !ctx.batch_id.is_empty() && !ctx.expected_epoch.is_empty()
        })
    }

    fn consume_batch_budget(&mut self) -> bool {
        let Some(ctx) = &mut self.active_batch else {
            return false;
        };
        if ctx.remaining_budget == 0 {
            return false;
        }
        ctx.remaining_budget -= 1;
        true
    }

    fn execute_selection_batch(
        &mut self,
        plan: SelectionPlan,
        batch_id: String,
        expected_epoch: &str,
        mut model: ChoiceModel,
    ) -> dunst_core::Result<ApplyOutcome> {
        let mut pinned_epoch = expected_epoch.to_string();
        let mut rescans = 0u32;
        let mut results = Vec::new();
        let mut remaining_steps = Vec::new();
        let mut stopped = false;

        for (idx, step) in plan.steps.iter().enumerate() {
            if stopped {
                remaining_steps.push(step.clone());
                continue;
            }
            let resolved = resolve_step(step, &model);
            let Some(resolved) = resolved else {
                results.push(StepResult {
                    choice_id: step.choice_id.clone(),
                    op: step.op,
                    result: StepResultStatus::Failed,
                    resolved_by: None,
                    label: step.label.clone(),
                    error: Some("unresolvable".to_string()),
                });
                remaining_steps.push(step.clone());
                continue;
            };

            if step_already_satisfied(step, &resolved.choice) {
                results.push(StepResult {
                    choice_id: step.choice_id.clone(),
                    op: step.op,
                    result: StepResultStatus::Success,
                    resolved_by: Some(resolved.resolved_by),
                    label: Some(resolved.choice.label),
                    error: None,
                });
                continue;
            }

            if !self.consume_batch_budget() {
                results.push(StepResult {
                    choice_id: step.choice_id.clone(),
                    op: step.op,
                    result: StepResultStatus::Skipped,
                    resolved_by: Some(resolved.resolved_by),
                    label: Some(resolved.choice.label.clone()),
                    error: Some("batch budget exhausted".to_string()),
                });
                remaining_steps.extend(plan.steps[idx..].iter().cloned());
                stopped = true;
                continue;
            }

            let label = resolved.choice.label.clone();
            let audit = self.execute_selection_step(step, &resolved.choice);
            match audit {
                Ok(Some(entry)) if entry.result == ActionResult::Success => {
                    results.push(StepResult {
                        choice_id: step.choice_id.clone(),
                        op: step.op,
                        result: StepResultStatus::Success,
                        resolved_by: Some(resolved.resolved_by),
                        label: Some(label),
                        error: None,
                    });
                    if idx + 1 < plan.steps.len() {
                        let current = self.current_ui_epoch_fingerprint();
                        if current != pinned_epoch {
                            if graph_diff_looks_like_reflow(&entry.graph_diff) {
                                if rescans >= MAX_RESCANS {
                                    remaining_steps.extend(plan.steps[idx + 1..].iter().cloned());
                                    stopped = true;
                                } else {
                                    rescans += 1;
                                    model = self.enumerate_choices(EnumerateOpts {
                                        scope: "page",
                                        include_latent: true,
                                        scroll_scan: false,
                                        max_scroll_pages: 1,
                                        limit: 500,
                                    })?;
                                    pinned_epoch = model.ui_epoch.clone();
                                }
                            } else {
                                pinned_epoch = current;
                            }
                        }
                    }
                }
                Ok(Some(entry)) => {
                    results.push(StepResult {
                        choice_id: step.choice_id.clone(),
                        op: step.op,
                        result: StepResultStatus::Failed,
                        resolved_by: Some(resolved.resolved_by),
                        label: Some(label),
                        error: Some(format!("actuator returned {:?}", entry.result)),
                    });
                    remaining_steps.extend(plan.steps[idx + 1..].iter().cloned());
                    stopped = true;
                }
                Ok(None) => {
                    results.push(StepResult {
                        choice_id: step.choice_id.clone(),
                        op: step.op,
                        result: StepResultStatus::Success,
                        resolved_by: Some(resolved.resolved_by),
                        label: Some(label),
                        error: None,
                    });
                }
                Err(err) => {
                    results.push(StepResult {
                        choice_id: step.choice_id.clone(),
                        op: step.op,
                        result: StepResultStatus::Failed,
                        resolved_by: Some(resolved.resolved_by),
                        label: Some(label),
                        error: Some(err.to_string()),
                    });
                    remaining_steps.extend(plan.steps[idx + 1..].iter().cloned());
                    stopped = true;
                }
            }
        }

        let verify = self.verify_selection_batch(&plan)?;
        let success = !stopped
            && remaining_steps.is_empty()
            && results
                .iter()
                .all(|step| step.result == StepResultStatus::Success)
            && verify.ok;
        let status = if success {
            ApplyStatus::Applied
        } else {
            ApplyStatus::PartiallyApplied
        };
        Ok(ApplyOutcome {
            status,
            batch_id,
            approval_hint: None,
            max_risk: None,
            pending_preview: Vec::new(),
            ui_epoch: self.current_ui_epoch_fingerprint(),
            rescans,
            steps: results,
            verify: Some(verify),
            remaining_steps,
            reason: None,
        })
    }

    fn execute_selection_step(
        &mut self,
        step: &SelectionStep,
        choice: &Choice,
    ) -> dunst_core::Result<Option<AuditEntry>> {
        match step.op {
            SelectionOp::Select | SelectionOp::Deselect => match choice.actuator {
                ActuatorHint::PickOption => self
                    .pick_option(&choice.label, false, Some("batch choice pick"))
                    .map(|result| Some(result.audit)),
                ActuatorHint::ClickNearText => self
                    .click_near_text(
                        &choice.label,
                        OcrClickOptions {
                            content_only: true,
                            accurate: true,
                            occurrence: 1,
                            expected_text: None,
                            reasoning: Some("batch OCR choice click"),
                            offset: (0.0, 0.0),
                        },
                    )
                    .map(|result| Some(result.audit)),
                _ => self
                    .click_element(&choice.id, Some("batch choice click"))
                    .map(Some),
            },
            SelectionOp::SetText => {
                let value = step.value.as_deref().unwrap_or_default();
                if matches!(choice.source.as_str(), "accessibility" | "ax") {
                    self.type_into(&choice.id, value, Some("batch set choice text"))
                        .map(Some)
                } else {
                    self.set_field_text(value).map(Some)
                }
            }
        }
    }

    fn preview_selection_plan(
        &self,
        plan: &SelectionPlan,
        model: &ChoiceModel,
    ) -> Vec<BatchPreviewStep> {
        plan.steps
            .iter()
            .map(|step| {
                let resolved = resolve_step(step, model);
                BatchPreviewStep {
                    choice_id: step.choice_id.clone(),
                    op: step.op,
                    label: resolved
                        .as_ref()
                        .map(|resolved| resolved.choice.label.clone())
                        .or_else(|| step.label.clone()),
                    risk: resolved
                        .as_ref()
                        .map(|resolved| resolved.choice.risk.level)
                        .unwrap_or(RiskLevel::High),
                }
            })
            .collect()
    }

    fn verify_selection_batch(&mut self, plan: &SelectionPlan) -> dunst_core::Result<BatchVerify> {
        let model = self.enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 500,
        })?;
        let mut checks = Vec::new();
        for step in &plan.steps {
            let resolved = resolve_step(step, &model);
            let (actual_state, actual_value) = resolved
                .as_ref()
                .map(|resolved| (Some(resolved.choice.state), resolved.choice.value.clone()))
                .unwrap_or((None, None));
            let expected_state = expected_state_for_step(step);
            let expected_value = expected_value_for_step(step);
            let state_ok = expected_state
                .zip(actual_state)
                .map(|(expected, actual)| expected == actual)
                .unwrap_or(true);
            let value_ok = expected_value
                .as_ref()
                .zip(actual_value.as_ref())
                .map(|(expected, actual)| expected == actual)
                .unwrap_or_else(|| expected_value.is_none());
            checks.push(VerifyCheck {
                choice_id: step.choice_id.clone(),
                expected: expected_state,
                actual: actual_state,
                expected_value,
                actual_value,
                ok: resolved.is_some() && state_ok && value_ok,
            });
        }
        Ok(BatchVerify {
            ok: checks.iter().all(|check| check.ok),
            checks,
        })
    }
}

fn validate_plan(plan: &SelectionPlan, expected_epoch: &str) -> dunst_core::Result<()> {
    if expected_epoch.trim().is_empty() {
        return Err(DunstError::Execution(
            "apply_selections requires non-empty expected_epoch".into(),
        ));
    }
    if plan.steps.is_empty() {
        return Err(DunstError::Execution(
            "apply_selections requires at least one step".into(),
        ));
    }
    if plan.steps.len() > MAX_BATCH_PICKS {
        return Err(DunstError::Execution(format!(
            "apply_selections accepts at most {MAX_BATCH_PICKS} steps"
        )));
    }
    for (idx, step) in plan.steps.iter().enumerate() {
        if step.choice_id.trim().is_empty() {
            return Err(DunstError::Execution(format!(
                "plan step {idx} has empty choice_id"
            )));
        }
        if step.op == SelectionOp::SetText && step.value.is_none() {
            return Err(DunstError::Execution(format!(
                "plan step {idx} op=set_text requires value"
            )));
        }
    }
    Ok(())
}

fn resolve_step(step: &SelectionStep, model: &ChoiceModel) -> Option<ResolvedChoice> {
    if let Some(choice) = model
        .groups
        .iter()
        .flat_map(|group| &group.choices)
        .find(|choice| choice.id == step.choice_id)
    {
        return Some(ResolvedChoice {
            choice: choice.clone(),
            resolved_by: ResolvedBy::Id,
        });
    }

    if let Some(label) = step.label.as_deref() {
        let label_norm = normalize_match(label);
        let mut label_matches: Vec<&Choice> = model
            .groups
            .iter()
            .filter(|group| {
                step.group_id
                    .as_deref()
                    .map(|group_id| group.id == group_id)
                    .unwrap_or(true)
            })
            .flat_map(|group| &group.choices)
            .filter(|choice| normalize_match(&choice.label) == label_norm)
            .collect();
        if label_matches.is_empty() && step.group_id.is_some() {
            label_matches = model
                .groups
                .iter()
                .flat_map(|group| &group.choices)
                .filter(|choice| normalize_match(&choice.label) == label_norm)
                .collect();
        }
        if label_matches.len() == 1 {
            return Some(ResolvedChoice {
                choice: label_matches[0].clone(),
                resolved_by: ResolvedBy::LabelAfterRescan,
            });
        }
    }

    let bbox = step.bbox?;
    nearest_bbox_choice(model, bbox).map(|choice| ResolvedChoice {
        choice: choice.clone(),
        resolved_by: ResolvedBy::BboxAfterRescan,
    })
}

fn nearest_bbox_choice(model: &ChoiceModel, bbox: Bbox) -> Option<&Choice> {
    let center = (bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
    model
        .groups
        .iter()
        .flat_map(|group| &group.choices)
        .filter_map(|choice| {
            let choice_bbox = choice.bbox?;
            let choice_center = (
                choice_bbox.x + choice_bbox.w / 2.0,
                choice_bbox.y + choice_bbox.h / 2.0,
            );
            let dx = center.0 - choice_center.0;
            let dy = center.1 - choice_center.1;
            let distance = (dx * dx) + (dy * dy);
            (distance <= 48.0 * 48.0).then_some((choice, distance))
        })
        .min_by(|(_, left), (_, right)| {
            left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(choice, _)| choice)
}

fn step_already_satisfied(step: &SelectionStep, choice: &Choice) -> bool {
    match step.op {
        SelectionOp::Select => choice.state == SelectionState::Selected,
        SelectionOp::Deselect => choice.state == SelectionState::Unselected,
        SelectionOp::SetText => step
            .value
            .as_deref()
            .zip(choice.value.as_deref())
            .is_some_and(|(expected, actual)| expected == actual),
    }
}

fn expected_state_for_step(step: &SelectionStep) -> Option<SelectionState> {
    step.expected_after
        .as_ref()
        .and_then(|expected| expected.state)
        .or(match step.op {
            SelectionOp::Select => Some(SelectionState::Selected),
            SelectionOp::Deselect => Some(SelectionState::Unselected),
            SelectionOp::SetText => None,
        })
}

fn expected_value_for_step(step: &SelectionStep) -> Option<String> {
    step.expected_after
        .as_ref()
        .and_then(|expected| expected.value.clone())
        .or_else(|| {
            if step.op == SelectionOp::SetText {
                step.value.clone()
            } else {
                None
            }
        })
}

fn graph_diff_looks_like_reflow(diff: &GraphDiff) -> bool {
    diff.changes.iter().any(|change| match change {
        NodeChange::Added { .. } | NodeChange::Removed { .. } => true,
        NodeChange::Changed { field, .. } => {
            matches!(field.as_str(), "bbox" | "enabled" | "children" | "parent")
        }
    })
}

fn selection_plan_hash(plan: &SelectionPlan) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let bytes = serde_json::to_vec(plan).unwrap_or_default();
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(super) fn valid_batch_selection_target_id(target_id: &str) -> Option<usize> {
    let rest = target_id.strip_prefix("batch@selections:")?;
    let (hash, count) = rest.split_once(':')?;
    if hash.len() != 16 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let count = count.parse::<usize>().ok()?;
    (1..=MAX_BATCH_PICKS).contains(&count).then_some(count)
}
