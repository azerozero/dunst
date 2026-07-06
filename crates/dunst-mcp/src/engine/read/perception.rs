use super::*;

impl Engine {
    /// OCR the target window via Apple Vision (P1). A pure **read probe** like the
    /// scene-graph getters: it does **not** risk-gate and records **no** audit entry.
    /// `region_screen_pt` limits OCR to a screen-point rectangle; `None` reads the
    /// whole window. Each hit's bbox is mapped from Vision's normalised space to
    /// screen points. macOS-only — see the non-macOS stub below.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying
    /// [`read_text_detailed`](Self::read_text_detailed) pass fails: an invalid
    /// region, a region outside the target window, a failed window capture, or a
    /// Vision OCR failure with no AX terminal-text fallback.
    #[cfg(target_os = "macos")]
    pub fn read_text(
        &self,
        region_screen_pt: Option<Bbox>,
        accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        // `read_text_detailed(.., content_only=false).hits` is exactly the raw OCR
        // hits — no content-region filter is applied when content_only is false —
        // so go straight to the raw pass instead of building, then discarding, the
        // full diagnostic result (visibility, warnings, and its all_hits clone).
        self.read_text_raw(region_screen_pt, accurate)
    }

    /// Detailed OCR path used by the safer MCP tools. It keeps the old
    /// `read_text` contract intact while exposing window-coverage and
    /// content-region diagnostics to agents that need to decide whether a raw
    /// click would hit the intended surface.
    ///
    /// # Errors
    ///
    /// Returns an error if a supplied region has non-positive width or height or
    /// falls outside the target window, if capturing the target window fails, or
    /// if Vision OCR fails and no AX terminal-text fallback is available.
    #[cfg(target_os = "macos")]
    pub fn read_text_detailed(
        &self,
        region_screen_pt: Option<Bbox>,
        accurate: bool,
        content_only: bool,
    ) -> dunst_core::Result<ReadTextResult> {
        let all_hits = self.read_text_raw(region_screen_pt, accurate)?;
        let window = self.current_window_bounds();
        let target_visibility = self.target_visibility();
        let content_region = content_only
            .then(|| browser_content_region(&self.window.app_name, window, region_screen_pt))
            .flatten();
        let hits = if let Some(region) = content_region {
            content_filtered_hits(&all_hits, region)
        } else {
            all_hits.clone()
        };
        let mut warnings = target_visibility.warnings.clone();
        if content_only && !all_hits.is_empty() && hits.is_empty() {
            warnings.push(
                "content_only OCR filtered every hit; the requested region may be browser chrome, a modal, or outside the web content area"
                    .into(),
            );
        }
        if target_visibility.visible_fraction < 0.95 {
            warnings.push(format!(
                "target visibility is {:.0}%; visible-screen interactions should expose the target window first",
                target_visibility.visible_fraction * 100.0
            ));
        }
        let recommended_next_steps = recommended_visibility_steps(&target_visibility, content_only);
        Ok(ReadTextResult {
            target: TargetState {
                pid: self.target.pid,
                window_id: self.target.window_id,
                app_name: self.window.app_name.clone(),
            },
            window,
            target_visibility,
            content_only,
            content_region,
            hits,
            all_hits,
            warnings,
            recommended_next_steps,
        })
    }

    #[cfg(target_os = "macos")]
    fn read_text_raw(
        &self,
        region_screen_pt: Option<Bbox>,
        accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        if let Some(region) = region_screen_pt {
            if region.w <= 0.0 || region.h <= 0.0 {
                return Err(DunstError::Perception(
                    "OCR region width/height must be positive".into(),
                ));
            }
            self.ensure_region_in_target_window(region, "read_text")?;
        }
        let key = ocr_cache_key(self.target.window_id, region_screen_pt, accurate);
        if let Some(cached) = self
            .ocr_cache
            .borrow()
            .as_ref()
            .and_then(|cache| cache.fresh(OCR_CACHE_TTL))
        {
            if cached.key == key {
                return Ok(cached.hits);
            }
        }
        // Always capture the target window, even for a requested region. Using a
        // raw screen-rect capture here can OCR whichever window happens to cover
        // that rectangle, which is exactly the wrong failure mode when several
        // Firefox windows are open.
        let captured = self.capture_target_composited("OCR")?;
        self.ocr_hits_from_captured(&captured, region_screen_pt, accurate)
    }

    /// Composited capture of the target window, shared by the OCR, shape, and
    /// zone read paths so a single tool call grabs the screen once. `what` names
    /// the caller in the error message if the grab fails.
    ///
    /// # Errors
    ///
    /// Returns [`DunstError::Perception`] if the composited window grab fails —
    /// there is no live macOS window to read.
    #[cfg(target_os = "macos")]
    pub(crate) fn capture_target_composited(
        &self,
        what: &str,
    ) -> dunst_core::Result<dunst_vision::capture::CapturedWindow> {
        dunst_vision::capture::capture_window_composited(self.target.window_id).map_err(|err| {
            DunstError::Perception(format!(
                "{what} requires a live macOS window (capture failed: {err})"
            ))
        })
    }

    /// OCRs an **already-captured** target frame and maps the boxes to
    /// screen-point [`TextHit`]s, applying the same AX-terminal fallback and
    /// OCR-cache write as [`read_text_raw`](Self::read_text_raw). Lets a caller
    /// that already grabbed the window (vision zones or chart scan) reuse that one
    /// capture instead of grabbing the screen a second time.
    ///
    /// # Errors
    ///
    /// Returns [`DunstError::Perception`] if Vision OCR fails and no AX
    /// terminal-text fallback is available.
    #[cfg(target_os = "macos")]
    pub(crate) fn ocr_hits_from_captured(
        &self,
        captured: &dunst_vision::capture::CapturedWindow,
        region_screen_pt: Option<Bbox>,
        accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        use dunst_vision::ocr::RecognitionMode;
        let key = ocr_cache_key(self.target.window_id, region_screen_pt, accurate);
        let mode = if accurate {
            RecognitionMode::Accurate
        } else {
            RecognitionMode::Fast
        };
        let boxes = match dunst_vision::ocr::ocr_region_with_mode(
            &captured.image,
            &captured.geometry,
            region_screen_pt,
            mode,
        ) {
            Ok(boxes) => boxes,
            Err(err) => {
                let fallback = self.ax_terminal_text_hits(region_screen_pt);
                if !fallback.is_empty() {
                    *self.ocr_cache.borrow_mut() = Some(TimedCache {
                        captured_at: Instant::now(),
                        value: OcrCacheEntry {
                            key,
                            hits: fallback.clone(),
                        },
                    });
                    return Ok(fallback);
                }
                return Err(DunstError::Perception(format!("OCR failed: {err}")));
            }
        };
        let hits: Vec<TextHit> = boxes
            .into_iter()
            .map(|text_box| TextHit {
                text: text_box.text,
                bbox: match region_screen_pt {
                    Some(region) => dunst_vision::coords::vision_norm_to_screen_pt_in_region(
                        text_box.norm,
                        region,
                    ),
                    None => dunst_vision::coords::vision_norm_to_screen_pt(
                        text_box.norm,
                        &captured.geometry,
                    ),
                },
                confidence: text_box.confidence,
            })
            .collect();
        *self.ocr_cache.borrow_mut() = Some(TimedCache {
            captured_at: Instant::now(),
            value: OcrCacheEntry {
                key,
                hits: hits.clone(),
            },
        });
        Ok(hits)
    }

    /// Non-macOS stub: Apple Vision OCR needs a live macOS window. Keeps
    /// `dunst-mcp` compilable (and the `read_text` tool present) on other targets.
    ///
    /// # Errors
    ///
    /// Always returns an error: OCR requires a live macOS window.
    #[cfg(not(target_os = "macos"))]
    pub fn read_text(
        &self,
        _region_screen_pt: Option<Bbox>,
        _accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        Err(DunstError::Perception(
            "OCR requires a live macOS window".into(),
        ))
    }

    /// Non-macOS stub: Apple Vision OCR needs a live macOS window.
    ///
    /// # Errors
    ///
    /// Always returns an error: OCR requires a live macOS window.
    #[cfg(not(target_os = "macos"))]
    pub fn read_text_detailed(
        &self,
        _region_screen_pt: Option<Bbox>,
        _accurate: bool,
        _content_only: bool,
    ) -> dunst_core::Result<ReadTextResult> {
        Err(DunstError::Perception(
            "OCR requires a live macOS window".into(),
        ))
    }

    /// Detect geometric primitives (rect/bar/circle/line) in the target window
    /// via the CV `shapes` layer — the figures (charts, custom-drawn UI) AX and
    /// OCR can't expose. A pure **read probe** like [`read_text`](Self::read_text):
    /// no risk-gating, no audit entry. macOS-only.
    ///
    /// # Errors
    ///
    /// Returns an error if capturing the target window fails — there is no live
    /// macOS window to run shape detection on.
    #[cfg(target_os = "macos")]
    pub fn read_shapes(&self) -> dunst_core::Result<Vec<ShapeHit>> {
        // Composited capture (see read_text): CGWindowListCreateImage is blank for
        // GPU/WebGL-rendered windows, so grab what is actually on screen instead.
        let captured = self.capture_target_composited("shape detection")?;
        Ok(
            dunst_vision::shapes::detect_shapes(&captured.image, &captured.geometry)
                .into_iter()
                .map(|shape| ShapeHit {
                    kind: format!("{:?}", shape.kind),
                    bbox: shape.bbox,
                    confidence: shape.confidence,
                })
                .collect(),
        )
    }

    /// Non-macOS stub: shape detection needs a live macOS window.
    ///
    /// # Errors
    ///
    /// Always returns an error: shape detection requires a live macOS window.
    #[cfg(not(target_os = "macos"))]
    pub fn read_shapes(&self) -> dunst_core::Result<Vec<ShapeHit>> {
        Err(DunstError::Perception(
            "shape detection requires a live macOS window".into(),
        ))
    }

    /// Nested vision zones: a containment tree over detected shapes, OCR text,
    /// and visible controls, so an agent reads "region > card > control/text"
    /// instead of three flat lists. Closes the gap where the accessibility tree
    /// is nested but the vision path ([`read_shapes`](Self::read_shapes) + OCR)
    /// is flat — most useful on sparse-AX surfaces (canvas/WebGL web apps).
    ///
    /// Rectangles become containers; OCR runs and actionable controls nest
    /// inside them by pure geometry (see [`dunst_vision::zones`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the window capture (for shape detection) or the OCR
    /// pass fails.
    #[cfg(target_os = "macos")]
    pub fn read_zones(&self) -> dunst_core::Result<Value> {
        use dunst_vision::shapes::ShapeKind;
        use dunst_vision::zones::{build_zone_tree, ZoneItem, ZoneKind};

        let mut items: Vec<ZoneItem> = Vec::new();

        // ONE composited capture feeds BOTH shape detection and OCR. Previously
        // read_shapes and read_text_raw each grabbed the screen, so a single
        // read_zones ran two `screencapture` subprocesses on the same window.
        let captured = self.capture_target_composited("vision zones")?;

        // Detected primitives: rectangles are candidate containers, the rest
        // (bars/circles/lines) stay leaves.
        for (idx, shape) in dunst_vision::shapes::detect_shapes(&captured.image, &captured.geometry)
            .into_iter()
            .enumerate()
        {
            // `Rect` = hollow bordered box, `Panel` = filled card/section — both
            // are containers content nests inside; the rest stay leaves.
            let kind = if matches!(shape.kind, ShapeKind::Rect | ShapeKind::Panel) {
                ZoneKind::Region
            } else {
                ZoneKind::Shape
            };
            items.push(ZoneItem {
                id: format!("shape_{idx}"),
                bbox: shape.bbox,
                kind,
                label: None,
                confidence: shape.confidence,
            });
        }

        // OCR text runs across the whole window, reusing the capture above.
        for (idx, hit) in self
            .ocr_hits_from_captured(&captured, None, true)?
            .into_iter()
            .enumerate()
        {
            items.push(ZoneItem {
                id: format!("text_{idx}"),
                bbox: hit.bbox,
                kind: ZoneKind::Text,
                label: Some(hit.text),
                confidence: hit.confidence,
            });
        }

        // Visible, actionable controls (AX-derived) so the tree is clickable:
        // a control lands inside the card/region that visually contains it.
        for target in self.hit_targets(false, "all", 500, None).targets {
            if let Some(bbox) = target.bbox {
                items.push(ZoneItem {
                    id: target.id,
                    bbox,
                    kind: ZoneKind::Control,
                    label: target.label,
                    confidence: target.confidence,
                });
            }
        }

        let zones = build_zone_tree(items);
        Ok(json!({
            "zones": zones.iter().map(zone_to_json).collect::<Vec<_>>(),
            "counts": { "roots": zones.len(), "total": count_zones(&zones) },
        }))
    }

    /// Non-macOS stub: vision zones need a live macOS window.
    #[cfg(not(target_os = "macos"))]
    pub fn read_zones(&self) -> dunst_core::Result<Value> {
        Err(DunstError::Perception(
            "vision zones require a live macOS window".into(),
        ))
    }
}

/// Serializes a [`dunst_vision::zones::Zone`] to JSON (the type is serde-free in
/// `dunst-vision`), omitting an absent label and an empty child list.
#[cfg(target_os = "macos")]
fn zone_to_json(zone: &dunst_vision::zones::Zone) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("id".to_string(), json!(zone.id));
    obj.insert("kind".to_string(), json!(zone.kind.as_str()));
    if let Some(label) = &zone.label {
        obj.insert("label".to_string(), json!(label));
    }
    obj.insert(
        "bbox".to_string(),
        serde_json::to_value(zone.bbox).unwrap_or(Value::Null),
    );
    obj.insert("confidence".to_string(), json!(zone.confidence));
    if !zone.children.is_empty() {
        let children: Vec<Value> = zone.children.iter().map(zone_to_json).collect();
        obj.insert("children".to_string(), Value::Array(children));
    }
    Value::Object(obj)
}

/// Total node count of a zone forest, including nested children.
#[cfg(target_os = "macos")]
fn count_zones(zones: &[dunst_vision::zones::Zone]) -> usize {
    zones
        .iter()
        .map(|zone| 1 + count_zones(&zone.children))
        .sum()
}

fn browser_content_region(app_name: &str, window: Bbox, requested: Option<Bbox>) -> Option<Bbox> {
    let normalized = normalize_match(app_name);
    let chrome_height = if [
        "firefox",
        "google chrome",
        "chrome",
        "chromium",
        "safari",
        "microsoft edge",
        "brave browser",
        "arc",
    ]
    .iter()
    .any(|name| normalized.contains(name))
    {
        96.0
    } else {
        0.0
    };
    let base = Bbox {
        x: window.x,
        y: window.y + chrome_height,
        w: window.w,
        h: (window.h - chrome_height).max(1.0),
    };
    requested
        .and_then(|region| clipped_region_to_window(region, base))
        .or(Some(base))
}

fn content_filtered_hits(hits: &[TextHit], region: Bbox) -> Vec<TextHit> {
    hits.iter()
        .filter(|hit| hit.confidence >= 0.45)
        .filter(|hit| rect_intersection_area(hit.bbox, region) > 0.0)
        .cloned()
        .collect()
}

fn recommended_visibility_steps(visibility: &TargetVisibility, content_only: bool) -> Vec<String> {
    let mut steps = Vec::new();
    if let Some(hint) = visibility.fallback_hint.clone() {
        steps.push(hint);
    }
    if content_only {
        steps.push(
            "Use read_text_detailed(content_only=false) only when browser chrome, tabs, or address bar text is the intended target."
                .into(),
        );
    }
    if !visibility.covered_by.is_empty() {
        steps.push(
            "Do not use click_at/read_at with visible-screen assumptions until target_visibility.covered_by is empty."
                .into(),
        );
    }
    steps
}
