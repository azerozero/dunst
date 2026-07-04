//! Pure target-synthesis helpers extracted from [`super`] (`read.rs`): the
//! affordance/OCR/shape → [`HitTarget`] derivation, safe-click zoning, OCR
//! form-field label↔value pairing, risk flooring for raw OCR clicks, and the
//! hit-target ordering. Every function is `self`-free; grouping them keeps the
//! `Engine` perception methods in `read.rs` readable as one concern.

use super::*;

pub(super) fn fallback_browser_tab_from_window_title(
    graph: &SceneGraph,
    query: Option<&str>,
) -> Option<BrowserTab> {
    if !is_browser_app_name(&graph.window.app_name) {
        return None;
    }
    let title = graph.window.title.trim();
    if title.is_empty() {
        return None;
    }
    let id = "tab_fallback_window_title";
    if let Some(q) = query {
        let haystack = format!("{id} {}", normalize_match(title));
        if !normalized_contains_query(&haystack, q) {
            return None;
        }
    }
    Some(BrowserTab {
        id: id.into(),
        url: likely_url(title),
        title: title.into(),
        selected: true,
        bbox: None,
    })
}

pub(super) fn node_matches_scope(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar: Option<&str>,
    scope: &str,
) -> bool {
    match scope {
        "all" | "" => true,
        "page" => !read_chrome_node(graph, node, window_rect, menubar),
        "browser_chrome" | "chrome" => read_chrome_node(graph, node, window_rect, menubar),
        _ => true,
    }
}

pub(super) fn hit_action_modes(affordance: &dunst_core::Affordance) -> Vec<HitActionMode> {
    affordance
        .actions
        .iter()
        .map(|action| HitActionMode {
            action: *action,
            tool_hint: tool_hint_for_action(*action).to_string(),
            target_id: Some(affordance.id.clone()),
            arguments: None,
            drop_targets: if *action == SemanticAction::Drag {
                affordance.drag_targets.clone()
            } else {
                Vec::new()
            },
            risk: affordance.risk.clone(),
        })
        .collect()
}

pub(super) fn append_page_scroll_targets(targets: &mut Vec<HitTarget>, window: Bbox) {
    if window.w <= 0.0 || window.h <= 0.0 {
        return;
    }
    let risk = page_scroll_risk();
    let bbox = Bbox {
        x: window.x + window.w * 0.12,
        y: window.y + window.h * 0.16,
        w: window.w * 0.60,
        h: window.h * 0.76,
    };
    for direction in ["down", "up", "bottom", "top"] {
        let id = page_scroll_target_id(direction);
        targets.push(HitTarget {
            id: id.clone(),
            source: "page".into(),
            role: "group",
            label: Some(format!("Page scroll {direction}")),
            value: None,
            bbox: Some(bbox),
            safe_click: synthetic_safe_zone(
                bbox,
                "page_scroll_region",
                "Use the scroll tool with this pseudo-target id; for scroll_at, prefer an OCR text/card point inside the content instead of a blank gutter.",
            ),
            confidence: 0.65,
            action_modes: vec![HitActionMode {
                action: SemanticAction::Scroll,
                tool_hint: "scroll".into(),
                target_id: Some(id.clone()),
                arguments: Some(json!({
                    "id": id,
                    "direction": direction,
                    "pages": if matches!(direction, "top" | "bottom") { 1 } else { 3 },
                })),
                drop_targets: Vec::new(),
                risk: risk.clone(),
            }],
            risk: risk.clone(),
        });
    }
}

pub(super) fn page_scroll_risk() -> RiskAssessment {
    RiskAssessment {
        level: RiskLevel::High,
        requires_approval: true,
        reasons: vec![
            "page pseudo-scroll uses raw keyboard input when no AX scroll container is available"
                .into(),
        ],
    }
}

/// OCR-derived hit targets can only be actuated through raw pointer input
/// (`click_near_text`), which the executor always approval-gates (see
/// `ocr_point_risk_at`). Floor the advertised affordance risk to that same gate
/// so `get_hit_targets` never promises a no-approval click that the action layer
/// then blocks with `pending_approval`. Text-derived reasons (e.g. destructive
/// keywords) are preserved on top of the raw-input reason.
pub(super) fn raw_ocr_click_risk(text_risk: RiskAssessment) -> RiskAssessment {
    let mut reasons =
        vec!["click is delivered as approval-gated raw OCR input, not an AX element".to_string()];
    reasons.extend(text_risk.reasons);
    RiskAssessment {
        level: RiskLevel::High,
        requires_approval: true,
        reasons,
    }
}

#[derive(Clone, Copy)]
pub(super) struct OcrFormLabel {
    role: &'static str,
    max_gap_y: f64,
}

pub(super) fn append_ocr_form_field_targets(
    targets: &mut Vec<HitTarget>,
    hits: &[TextHit],
    risk_engine: &RiskEngine,
    max_fields: usize,
) {
    let mut added = 0usize;
    for (idx, label_hit) in hits.iter().enumerate() {
        if added >= max_fields {
            break;
        }
        let Some(kind) = ocr_form_label_kind(&label_hit.text) else {
            continue;
        };
        let Some(value_hit) = nearest_ocr_field_value(label_hit, &hits[idx + 1..], kind) else {
            continue;
        };
        if value_hit.confidence < 0.45
            || bbox_duplicate_of_existing_form_field(value_hit.bbox, targets)
        {
            continue;
        }
        let label_center = bbox_center(label_hit.bbox);
        let value_center = bbox_center(value_hit.bbox);
        let offset = (
            value_center.0 - label_center.0,
            value_center.1 - label_center.1,
        );
        let text = format!("{} {}", label_hit.text, value_hit.text);
        let risk = raw_ocr_click_risk(risk_engine.assess_text(&text));
        targets.push(ocr_form_field_target(
            idx, kind, label_hit, value_hit, offset, risk,
        ));
        added += 1;
    }
}

pub(super) fn ocr_form_label_kind(text: &str) -> Option<OcrFormLabel> {
    let normalized = normalize_match(text);
    let compact: String = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if normalized.contains("description") {
        return Some(OcrFormLabel {
            role: "text_area",
            max_gap_y: 120.0,
        });
    }
    if normalized.contains("titre")
        || normalized.contains("title")
        || compact.contains("realisation")
        || compact.contains("realization")
    {
        return Some(OcrFormLabel {
            role: "text_field",
            max_gap_y: 80.0,
        });
    }
    None
}

pub(super) fn nearest_ocr_field_value<'a>(
    label: &TextHit,
    candidates: &'a [TextHit],
    kind: OcrFormLabel,
) -> Option<&'a TextHit> {
    candidates
        .iter()
        .filter(|candidate| candidate.confidence >= 0.45)
        .filter(|candidate| ocr_form_label_kind(&candidate.text).is_none())
        .filter(|candidate| candidate.bbox.y >= label.bbox.y + label.bbox.h * 0.45)
        .filter(|candidate| candidate.bbox.y - label.bbox.y <= kind.max_gap_y)
        .filter(|candidate| ocr_form_field_x_aligned(label.bbox, candidate.bbox))
        .min_by(|a, b| {
            let ay = (a.bbox.y - label.bbox.y).abs();
            let by = (b.bbox.y - label.bbox.y).abs();
            ay.partial_cmp(&by)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ax = (a.bbox.x - label.bbox.x).abs();
                    let bx = (b.bbox.x - label.bbox.x).abs();
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
        })
}

pub(super) fn ocr_form_field_x_aligned(label: Bbox, value: Bbox) -> bool {
    let overlap = (label.x + label.w).min(value.x + value.w) - label.x.max(value.x);
    let overlap_ratio = overlap.max(0.0) / label.w.max(1.0);
    overlap_ratio >= 0.15 || (value.x - label.x).abs() <= 96.0
}

pub(super) fn bbox_center(bbox: Bbox) -> (f64, f64) {
    (bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0)
}

pub(super) fn bbox_duplicate_of_existing_form_field(bbox: Bbox, targets: &[HitTarget]) -> bool {
    let area = (bbox.w.max(0.0) * bbox.h.max(0.0)).max(1.0);
    targets.iter().any(|target| {
        if target.source == "page" || (target.source == "ocr" && target.role == "group") {
            return false;
        }
        target
            .bbox
            .map(|existing| rect_intersection_area(existing, bbox) / area > 0.72)
            .unwrap_or(false)
    })
}

pub(super) fn ocr_form_field_target(
    idx: usize,
    kind: OcrFormLabel,
    label_hit: &TextHit,
    value_hit: &TextHit,
    offset: (f64, f64),
    risk: RiskAssessment,
) -> HitTarget {
    let id = format!(
        "ocr_field_{idx}_{}",
        compact_synthetic_label(&label_hit.text)
    );
    HitTarget {
        id,
        source: "ocr".into(),
        role: kind.role,
        label: Some(label_hit.text.clone()),
        value: Some(value_hit.text.clone()),
        bbox: Some(value_hit.bbox),
        safe_click: synthetic_safe_zone(
            value_hit.bbox,
            "ocr_form_value_bbox_inset",
            "Use click_near_text with the supplied label-relative offset, then verify with OCR before typing or saving.",
        ),
        confidence: label_hit.confidence.min(value_hit.confidence),
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": label_hit.text.clone(),
                "content_only": true,
                "accurate": false,
                "offset_x": offset.0,
                "offset_y": offset.1,
                "expected_text": value_hit.text.clone(),
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

pub(super) fn card_hit_target(card: OcrCard, risk: RiskAssessment) -> HitTarget {
    let mut value = card
        .lines
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    if value.is_empty() {
        value = card.title.clone();
    }
    HitTarget {
        id: card.id.clone(),
        source: "ocr".into(),
        role: "group",
        label: Some(card.title.clone()),
        value: Some(value),
        bbox: Some(card.bbox),
        safe_click: synthetic_safe_zone(
            card.bbox,
            "ocr_card_bbox_inset",
            "Prefer click_near_text with the card title; use the zone only after OCR verification.",
        ),
        confidence: card.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": card.title,
                "content_only": true,
                "accurate": false,
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

pub(super) fn ocr_hit_target(idx: usize, hit: &TextHit, risk: RiskAssessment) -> HitTarget {
    let id = format!("ocr_text_{idx}_{}", compact_synthetic_label(&hit.text));
    HitTarget {
        id,
        source: "ocr".into(),
        role: "static_text",
        label: Some(hit.text.clone()),
        value: None,
        bbox: Some(hit.bbox),
        safe_click: synthetic_safe_zone(
            hit.bbox,
            "ocr_text_bbox_inset",
            "Prefer click_near_text with this text; use the zone only after OCR verification.",
        ),
        confidence: hit.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".into(),
            target_id: None,
            arguments: Some(json!({
                "query": hit.text,
                "content_only": true,
                "accurate": false,
            })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

pub(super) fn shape_hit_target(idx: usize, shape: ShapeHit) -> HitTarget {
    let center = (
        shape.bbox.x + shape.bbox.w / 2.0,
        shape.bbox.y + shape.bbox.h / 2.0,
    );
    let risk = RiskAssessment {
        level: RiskLevel::Medium,
        requires_approval: false,
        reasons: vec!["vision-derived shape target; verify semantics before mutating".into()],
    };
    HitTarget {
        id: format!(
            "vision_shape_{idx}_{}",
            compact_synthetic_label(&shape.kind)
        ),
        source: "vision".into(),
        role: "image",
        label: Some(format!("{} shape", shape.kind)),
        value: None,
        bbox: Some(shape.bbox),
        safe_click: synthetic_safe_zone(
            shape.bbox,
            "vision_shape_bbox_inset",
            "Use read_at/hover verification before any raw click on this shape.",
        ),
        confidence: shape.confidence,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Hover,
            tool_hint: "read_at".into(),
            target_id: None,
            arguments: Some(json!({ "x": center.0, "y": center.1 })),
            drop_targets: Vec::new(),
            risk: risk.clone(),
        }],
        risk,
    }
}

pub(super) fn tool_hint_for_action(action: SemanticAction) -> &'static str {
    match action {
        SemanticAction::Click | SemanticAction::Toggle | SemanticAction::Focus => "click_element",
        SemanticAction::Hover => "hover_probe",
        SemanticAction::Type => "type_into",
        SemanticAction::OpenMenu => "open_menu",
        SemanticAction::Pick => "pick_option",
        SemanticAction::Scroll => "scroll",
        SemanticAction::Drag => "drag_element",
        SemanticAction::Raise => "raise_element",
        SemanticAction::KeyPress => "press_key",
        SemanticAction::Hotkey => "hotkey",
    }
}

pub(super) fn safe_click_zone(bbox: Bbox) -> Option<SafeClickZone> {
    if bbox.w <= 0.0 || bbox.h <= 0.0 {
        return None;
    }
    let inset = (bbox.w.min(bbox.h) * 0.12).min(8.0);
    let inset = if bbox.w - inset * 2.0 >= 4.0 && bbox.h - inset * 2.0 >= 4.0 {
        inset
    } else {
        0.0
    };
    let zone = Bbox {
        x: bbox.x + inset,
        y: bbox.y + inset,
        w: bbox.w - inset * 2.0,
        h: bbox.h - inset * 2.0,
    };
    Some(SafeClickZone {
        bbox: zone,
        center: (zone.x + zone.w / 2.0, zone.y + zone.h / 2.0),
        source: "accessibility_bbox_inset".into(),
        note: "Prefer click_element by id; use this zone only when an element-bound click is unavailable."
            .into(),
    })
}

pub(super) fn synthetic_safe_zone(bbox: Bbox, source: &str, note: &str) -> Option<SafeClickZone> {
    safe_click_zone(bbox).map(|mut zone| {
        zone.source = source.to_string();
        zone.note = note.to_string();
        zone
    })
}

pub(super) fn bbox_duplicate_of_existing(bbox: Bbox, targets: &[HitTarget]) -> bool {
    let area = (bbox.w.max(0.0) * bbox.h.max(0.0)).max(1.0);
    targets.iter().any(|target| {
        if target.source == "page" {
            return false;
        }
        target
            .bbox
            .map(|existing| rect_intersection_area(existing, bbox) / area > 0.72)
            .unwrap_or(false)
    })
}

pub(super) fn compact_synthetic_label(text: &str) -> String {
    let normalized = normalize_match(text);
    let mut out = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "target".into()
    } else {
        trimmed.into()
    }
}

pub(super) fn hit_target_order(a: &HitTarget, b: &HitTarget) -> std::cmp::Ordering {
    hit_source_rank(&a.source)
        .cmp(&hit_source_rank(&b.source))
        .then_with(|| hit_target_position_order(a, b))
}

pub(super) fn hit_target_position_order(a: &HitTarget, b: &HitTarget) -> std::cmp::Ordering {
    let ay = a.bbox.map(|b| rounded_i64(b.y)).unwrap_or(i64::MAX);
    let by = b.bbox.map(|b| rounded_i64(b.y)).unwrap_or(i64::MAX);
    ay.cmp(&by)
        .then_with(|| {
            let ax = a.bbox.map(|b| rounded_i64(b.x)).unwrap_or(i64::MAX);
            let bx = b.bbox.map(|b| rounded_i64(b.x)).unwrap_or(i64::MAX);
            ax.cmp(&bx)
        })
        .then_with(|| a.role.cmp(b.role))
        .then_with(|| a.id.cmp(&b.id))
}

pub(super) fn hit_source_rank(source: &str) -> u8 {
    match source {
        "accessibility" => 0,
        "page" => 1,
        "ocr" => 2,
        "vision" => 3,
        _ => 4,
    }
}

pub(super) fn hit_target_matches_find_query(target: &HitTarget, query: &str) -> bool {
    normalized_contains_query(&normalize_match(&target.id), query)
        || normalized_contains_query(&normalize_match(target.role), query)
        || target
            .label
            .as_deref()
            .map(|label| normalized_contains_query(&normalize_match(label), query))
            .unwrap_or(false)
        || target
            .value
            .as_deref()
            .map(|value| normalized_contains_query(&normalize_match(value), query))
            .unwrap_or(false)
}

pub(super) fn source_name(source: dunst_core::Source) -> &'static str {
    match source {
        dunst_core::Source::Accessibility => "accessibility",
        dunst_core::Source::Vision => "vision",
        dunst_core::Source::Ocr => "ocr",
    }
}
