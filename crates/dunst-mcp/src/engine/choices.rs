use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub struct EnumerateOpts<'a> {
    pub scope: &'a str,
    pub include_latent: bool,
    pub scroll_scan: bool,
    pub max_scroll_pages: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChoiceModel {
    pub ui_epoch: String,
    pub scope: String,
    pub coverage: Coverage,
    pub groups: Vec<ChoiceGroup>,
    pub warnings: Vec<String>,
    pub scroll_plan: Vec<ScrollHint>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChoiceGroup {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub kind: GroupKind,
    pub requirement: Requirement,
    pub classification_confidence: f32,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Choice {
    pub id: String,
    pub group_id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub state: SelectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Bbox>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_click: Option<SafeClickZone>,
    pub actuator: ActuatorHint,
    pub risk: RiskAssessment,
    pub source: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScrollHint {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Bbox>,
    pub risk: RiskAssessment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    SingleSelect,
    MultiSelect,
    TextField,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionState {
    Selected,
    Unselected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    Required,
    Optional,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActuatorHint {
    ClickElement,
    PickOption,
    ClickNearText,
    SetFieldText,
    Scroll,
}

#[derive(Clone, Debug)]
struct GroupSeed {
    id: String,
    label: Option<String>,
    kind: GroupKind,
    requirement: Requirement,
    confidence: f32,
}

impl Engine {
    pub fn enumerate_choices(
        &mut self,
        opts: EnumerateOpts<'_>,
    ) -> dunst_core::Result<ChoiceModel> {
        let opts = NormalizedEnumerateOpts::from(opts);
        let (result, targets, warnings, scroll_scan_completed) = if opts.scroll_scan {
            self.scroll_scan_choice_targets(&opts)
        } else {
            let result = self.hit_targets(opts.include_latent, opts.scope, opts.limit, None);
            let targets = result.targets.clone();
            (result, targets, Vec::new(), false)
        };
        Ok(self.choice_model_from_targets(
            &result,
            targets,
            opts.scope,
            opts.scroll_scan,
            scroll_scan_completed,
            warnings,
        ))
    }

    fn choice_model_from_targets(
        &self,
        result: &HitTargetsResult,
        targets: Vec<HitTarget>,
        scope: &str,
        scroll_scan: bool,
        scroll_scan_completed: bool,
        mut warnings: Vec<String>,
    ) -> ChoiceModel {
        warnings.extend(result.supplemental_warnings.clone());
        let mut scroll_plan = Vec::new();
        let mut groups: BTreeMap<String, ChoiceGroup> = BTreeMap::new();
        let mut has_vision_only_choices = false;

        for target in targets {
            if let Some(hint) = scroll_hint(&target) {
                scroll_plan.push(hint);
                continue;
            }
            let Some((kind, confidence)) = classify_choice_target(&target) else {
                continue;
            };
            if matches!(target.source.as_str(), "ocr" | "vision") {
                has_vision_only_choices = true;
            }
            let seed = self.choice_group_seed(&target, kind, confidence);
            let choice = Choice {
                id: target.id.clone(),
                group_id: seed.id.clone(),
                label: choice_label(&target),
                value: target.value.clone(),
                state: selection_state(target.value.as_deref()),
                bbox: target.bbox,
                safe_click: target.safe_click.clone(),
                actuator: actuator_hint(&target, kind),
                risk: target.risk.clone(),
                source: target.source.clone(),
            };
            groups
                .entry(seed.id.clone())
                .and_modify(|group| {
                    group.requirement = max_requirement(group.requirement, seed.requirement);
                    group.classification_confidence =
                        group.classification_confidence.max(seed.confidence);
                    if group.label.is_none() {
                        group.label = seed.label.clone();
                    }
                    group.choices.push(choice.clone());
                })
                .or_insert_with(|| ChoiceGroup {
                    id: seed.id,
                    label: seed.label,
                    kind: seed.kind,
                    requirement: seed.requirement,
                    classification_confidence: seed.confidence,
                    choices: vec![choice],
                });
        }

        let mut groups: Vec<ChoiceGroup> = groups.into_values().collect();
        for group in &mut groups {
            group.choices.sort_by(choice_order);
            enforce_single_select_cardinality(group, &mut warnings);
        }
        groups.sort_by(group_order);

        let coverage = if scroll_scan {
            if scroll_scan_completed {
                scroll_plan.clear();
                Coverage::Complete
            } else if scroll_plan.is_empty() {
                Coverage::Complete
            } else {
                Coverage::Partial
            }
        } else if has_vision_only_choices && !scroll_plan.is_empty() {
            Coverage::Partial
        } else {
            scroll_plan.clear();
            Coverage::Complete
        };

        ChoiceModel {
            ui_epoch: result.ui_epoch.fingerprint.clone(),
            scope: scope.to_string(),
            coverage,
            groups,
            warnings,
            scroll_plan,
        }
    }

    fn choice_group_seed(&self, target: &HitTarget, kind: GroupKind, confidence: f32) -> GroupSeed {
        if matches!(kind, GroupKind::TextField | GroupKind::Action) {
            let label = target.label.clone();
            return GroupSeed {
                id: format!("grp_{}", stable_group_token(&target.id)),
                requirement: requirement_from_labels([label.as_deref(), target.value.as_deref()]),
                label,
                kind,
                confidence,
            };
        }

        let graph = self.scene_graph();
        let parent = graph
            .get(&target.id)
            .and_then(|node| node.parent.as_deref())
            .and_then(|parent| graph.get(parent));
        let parent_id = parent.map(|node| node.id.as_str()).unwrap_or("ungrouped");
        let parent_label = parent
            .and_then(|node| {
                if matches!(node.role, Role::Window | Role::Toolbar | Role::MenuBar) {
                    None
                } else {
                    node.label.as_deref().or(node.value.as_deref())
                }
            })
            .map(str::to_string);
        let label = parent_label.clone();
        let requirement = requirement_from_labels([
            label.as_deref(),
            target.label.as_deref(),
            target.value.as_deref(),
        ]);
        GroupSeed {
            id: format!(
                "grp_{}_{}",
                group_kind_token(kind),
                stable_group_token(parent_id)
            ),
            label,
            kind,
            requirement,
            confidence,
        }
    }

    fn scroll_scan_choice_targets(
        &mut self,
        opts: &NormalizedEnumerateOpts<'_>,
    ) -> (HitTargetsResult, Vec<HitTarget>, Vec<String>, bool) {
        let initial = self.hit_targets(opts.include_latent, opts.scope, opts.limit, None);
        let targets = initial.targets.clone();
        let mut warnings = Vec::new();

        #[cfg(test)]
        {
            warnings.push(
                "scroll_scan requested under the mock test backend; returned the current choice surface"
                    .to_string(),
            );
            (initial, targets, warnings, true)
        }

        #[cfg(all(target_os = "macos", not(test)))]
        {
            let mut targets = targets;
            let mut pages_scrolled = 0usize;
            self.begin_internal_batch_context(
                "batch@survey-scroll".to_string(),
                opts.max_scroll_pages.saturating_mul(2).saturating_add(2),
                initial.ui_epoch.fingerprint.clone(),
            );
            for _ in 0..opts.max_scroll_pages {
                match self.scroll("down", 1, None) {
                    Ok(entry) if entry.result == ActionResult::Success => {
                        pages_scrolled += 1;
                        let next =
                            self.hit_targets(opts.include_latent, opts.scope, opts.limit, None);
                        merge_hit_targets(&mut targets, next.targets);
                    }
                    Ok(entry) => {
                        warnings.push(format!(
                            "scroll_scan stopped after non-successful survey scroll: {:?}",
                            entry.result
                        ));
                        break;
                    }
                    Err(err) => {
                        warnings.push(format!("scroll_scan stopped: {err}"));
                        break;
                    }
                }
            }
            for _ in 0..pages_scrolled {
                if let Err(err) = self.scroll("up", 1, None) {
                    warnings.push(format!(
                        "scroll_scan could not restore the original scroll position exactly: {err}"
                    ));
                    break;
                }
            }
            self.clear_internal_batch_context();
            (initial, targets, warnings, pages_scrolled > 0)
        }

        #[cfg(all(not(target_os = "macos"), not(test)))]
        {
            warnings.push(
                "scroll_scan requested, but survey scrolling requires the macOS backend; returned the current choice surface"
                    .to_string(),
            );
            (initial, targets, warnings, true)
        }
    }

    #[cfg(test)]
    pub(super) fn choice_model_from_targets_for_test(
        &self,
        result: &HitTargetsResult,
        targets: Vec<HitTarget>,
        scope: &str,
    ) -> ChoiceModel {
        self.choice_model_from_targets(result, targets, scope, false, false, Vec::new())
    }
}

#[derive(Clone, Copy)]
struct NormalizedEnumerateOpts<'a> {
    scope: &'a str,
    include_latent: bool,
    scroll_scan: bool,
    #[cfg_attr(test, allow(dead_code))]
    max_scroll_pages: usize,
    limit: usize,
}

impl<'a> From<EnumerateOpts<'a>> for NormalizedEnumerateOpts<'a> {
    fn from(opts: EnumerateOpts<'a>) -> Self {
        Self {
            scope: match opts.scope {
                "all" | "browser_chrome" => opts.scope,
                _ => "page",
            },
            include_latent: opts.include_latent,
            scroll_scan: opts.scroll_scan,
            max_scroll_pages: opts.max_scroll_pages.clamp(1, 12),
            limit: opts.limit.clamp(1, 500),
        }
    }
}

fn classify_choice_target(target: &HitTarget) -> Option<(GroupKind, f32)> {
    let role = target.role;
    if role == "radio" {
        return Some((GroupKind::SingleSelect, 0.95));
    }
    if matches!(role, "checkbox" | "switch") {
        return Some((GroupKind::MultiSelect, 0.95));
    }
    if matches!(role, "text_field" | "text_area" | "search_field") {
        return Some((GroupKind::TextField, 0.95));
    }
    if matches!(role, "popup_button" | "combobox" | "menu_button") {
        return Some((GroupKind::SingleSelect, 0.85));
    }
    if target
        .action_modes
        .iter()
        .any(|mode| matches!(mode.action, SemanticAction::Pick | SemanticAction::OpenMenu))
    {
        return Some((GroupKind::SingleSelect, 0.75));
    }
    if target
        .action_modes
        .iter()
        .any(|mode| mode.action == SemanticAction::Type)
    {
        return Some((GroupKind::TextField, 0.75));
    }
    if role == "button"
        && target
            .action_modes
            .iter()
            .any(|mode| mode.action == SemanticAction::Click)
    {
        return Some((GroupKind::Action, 0.7));
    }
    if target.source == "ocr"
        && target
            .action_modes
            .iter()
            .any(|mode| mode.action == SemanticAction::Click)
    {
        return Some((GroupKind::Action, 0.55));
    }
    None
}

fn actuator_hint(target: &HitTarget, kind: GroupKind) -> ActuatorHint {
    if target.id.starts_with("page@scroll:") {
        return ActuatorHint::Scroll;
    }
    if matches!(target.source.as_str(), "ocr" | "vision") {
        return ActuatorHint::ClickNearText;
    }
    if kind == GroupKind::TextField {
        return ActuatorHint::SetFieldText;
    }
    if target
        .action_modes
        .iter()
        .any(|mode| matches!(mode.action, SemanticAction::Pick | SemanticAction::OpenMenu))
    {
        return ActuatorHint::PickOption;
    }
    ActuatorHint::ClickElement
}

fn scroll_hint(target: &HitTarget) -> Option<ScrollHint> {
    let direction = target.id.strip_prefix("page@scroll:")?.to_string();
    Some(ScrollHint {
        id: target.id.clone(),
        direction: Some(direction),
        bbox: target.bbox,
        risk: target.risk.clone(),
    })
}

fn choice_label(target: &HitTarget) -> String {
    target
        .label
        .as_deref()
        .or(target.value.as_deref())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(target.id.as_str())
        .trim()
        .to_string()
}

fn selection_state(value: Option<&str>) -> SelectionState {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return SelectionState::Unknown;
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "selected" | "checked" | "selectionne" | "sélectionné" => {
            SelectionState::Selected
        }
        "0" | "false" | "no" | "off" | "unselected" | "unchecked" | "deselected"
        | "non selectionne" | "non sélectionné" => SelectionState::Unselected,
        _ => SelectionState::Unknown,
    }
}

fn requirement_from_labels<'a>(labels: impl IntoIterator<Item = Option<&'a str>>) -> Requirement {
    for label in labels.into_iter().flatten() {
        let normalized = label.to_ascii_lowercase();
        if normalized.contains('*')
            || normalized.contains("required")
            || normalized.contains("obligatoire")
            || normalized.contains("requis")
            || normalized.contains("requise")
        {
            return Requirement::Required;
        }
    }
    Requirement::Optional
}

fn max_requirement(left: Requirement, right: Requirement) -> Requirement {
    match (left, right) {
        (Requirement::Required, _) | (_, Requirement::Required) => Requirement::Required,
        (Requirement::Unknown, _) | (_, Requirement::Unknown) => Requirement::Unknown,
        _ => Requirement::Optional,
    }
}

fn enforce_single_select_cardinality(group: &mut ChoiceGroup, warnings: &mut Vec<String>) {
    if group.kind != GroupKind::SingleSelect {
        return;
    }
    let mut seen = BTreeSet::new();
    for choice in &mut group.choices {
        if choice.state != SelectionState::Selected {
            continue;
        }
        if seen.insert(group.id.clone()) {
            continue;
        }
        warnings.push(format!(
            "single-select group {} reported multiple selected choices; keeping later state unknown for {}",
            group.id, choice.id
        ));
        choice.state = SelectionState::Unknown;
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn merge_hit_targets(targets: &mut Vec<HitTarget>, incoming: Vec<HitTarget>) {
    for target in incoming {
        if targets
            .iter()
            .any(|existing| same_hit_target(existing, &target))
        {
            continue;
        }
        targets.push(target);
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn same_hit_target(left: &HitTarget, right: &HitTarget) -> bool {
    if left.id == right.id {
        return true;
    }
    match (left.bbox, right.bbox) {
        (Some(a), Some(b)) => {
            (a.x - b.x).abs() < 1.0
                && (a.y - b.y).abs() < 1.0
                && (a.w - b.w).abs() < 1.0
                && (a.h - b.h).abs() < 1.0
                && choice_label(left).eq_ignore_ascii_case(&choice_label(right))
        }
        _ => false,
    }
}

fn group_order(left: &ChoiceGroup, right: &ChoiceGroup) -> std::cmp::Ordering {
    group_min_y(left)
        .partial_cmp(&group_min_y(right))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            group_min_x(left)
                .partial_cmp(&group_min_x(right))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.id.cmp(&right.id))
}

fn choice_order(left: &Choice, right: &Choice) -> std::cmp::Ordering {
    let ly = left.bbox.map(|bbox| bbox.y).unwrap_or(f64::MAX);
    let ry = right.bbox.map(|bbox| bbox.y).unwrap_or(f64::MAX);
    ly.partial_cmp(&ry)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            let lx = left.bbox.map(|bbox| bbox.x).unwrap_or(f64::MAX);
            let rx = right.bbox.map(|bbox| bbox.x).unwrap_or(f64::MAX);
            lx.partial_cmp(&rx).unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.id.cmp(&right.id))
}

fn group_min_y(group: &ChoiceGroup) -> f64 {
    group
        .choices
        .iter()
        .filter_map(|choice| choice.bbox.map(|bbox| bbox.y))
        .fold(f64::MAX, f64::min)
}

fn group_min_x(group: &ChoiceGroup) -> f64 {
    group
        .choices
        .iter()
        .filter_map(|choice| choice.bbox.map(|bbox| bbox.x))
        .fold(f64::MAX, f64::min)
}

fn stable_group_token(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn group_kind_token(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::SingleSelect => "single",
        GroupKind::MultiSelect => "multi",
        GroupKind::TextField => "text",
        GroupKind::Action => "action",
    }
}
