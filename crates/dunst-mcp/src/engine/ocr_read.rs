use super::*;

impl Engine {
    /// Read text at **several** screen points. The default path uses background
    /// hover and OCRs only the target window. Set `borrow_cursor=true` for the
    /// older real-cursor path: one borrow for the whole sweep, warp to each point,
    /// OCR a screen fovea, then restore the cursor. macOS-only.
    #[cfg(target_os = "macos")]
    pub fn read_series(
        &self,
        points: &[(f64, f64)],
        borrow_cursor: bool,
    ) -> dunst_core::Result<Vec<Vec<TextHit>>> {
        if points.is_empty() {
            return Ok(Vec::new());
        }
        for &(x, y) in points {
            self.ensure_point_in_target_window(x, y, "read_series")?;
        }
        if !borrow_cursor {
            return self.read_series_background(points);
        }
        let visibility = self.target_visibility();
        if !visibility.covered_by.is_empty() {
            return Err(DunstError::Execution(format!(
                "read_series borrow_cursor=true requires visible target pixels, but target window {} is covered by {:?}; use expose_target_window or read without borrow_cursor",
                visibility.target_window_id,
                visibility
                    .covered_by
                    .iter()
                    .map(|window| window.window_id)
                    .collect::<Vec<_>>()
            )));
        }
        let (x0, y0) = points[0];
        let saved = dunst_platform::cursor_borrow_to(x0, y0)?;
        let mut out = Vec::with_capacity(points.len());
        for &(x, y) in points {
            // Move to the point (the hover triggers reliably — no circle needed),
            // then DISPLAY-capture a fovea around the cursor: the crosshair value
            // bubble is a GPU overlay a window capture misses, but a composited
            // screen grab includes it — and it's app/browser agnostic + fast.
            // A small move INTO the point (a delta, not a circle) makes the
            // crosshair render; then let it paint before the composited grab.
            let _ = retry_user_active_guard(|| {
                dunst_platform::hover_at_point(self.target.pid, x - 8.0, y)
            });
            std::thread::sleep(std::time::Duration::from_millis(30));
            let _ =
                retry_user_active_guard(|| dunst_platform::hover_at_point(self.target.pid, x, y));
            std::thread::sleep(std::time::Duration::from_millis(320));
            match self.ocr_screen_fovea(x, y) {
                Ok(hits) => out.push(hits),
                Err(err) => {
                    let _ = dunst_platform::cursor_restore(saved.0, saved.1);
                    return Err(err);
                }
            }
        }
        let _ = dunst_platform::cursor_restore(saved.0, saved.1);
        Ok(out)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn read_series(
        &self,
        _points: &[(f64, f64)],
        _borrow_cursor: bool,
    ) -> dunst_core::Result<Vec<Vec<TextHit>>> {
        Err(DunstError::Execution(
            "read_series requires a macOS backend".into(),
        ))
    }

    /// Background series read: no OS cursor borrow. This uses the same target-pid
    /// hover path as `hover_at`, then OCRs a clipped fovea from the target window
    /// only.
    #[cfg(target_os = "macos")]
    pub(super) fn read_series_background(
        &self,
        points: &[(f64, f64)],
    ) -> dunst_core::Result<Vec<Vec<TextHit>>> {
        let mut out = Vec::with_capacity(points.len());
        for &(x, y) in points {
            let (lead_x, lead_y) = self.clamp_point_to_target_window(x - 8.0, y);
            self.hover_target_background(lead_x, lead_y)?;
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.hover_target_background(x, y)?;
            std::thread::sleep(std::time::Duration::from_millis(320));
            out.push(self.ocr_window_fovea(x, y)?);
        }
        Ok(out)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn hover_target_background(&self, x: f64, y: f64) -> dunst_core::Result<()> {
        self.ensure_point_in_target_window(x, y, "background hover")?;
        let (ox, oy) = dunst_vision::capture::window_bounds(self.target.window_id)
            .map(|(x, y, _, _)| (x, y))
            .unwrap_or((0.0, 0.0));
        retry_user_active_guard(|| {
            dunst_platform::hover_web_background(
                self.target.pid,
                self.target.window_id,
                x,
                y,
                ox,
                oy,
            )
        })
    }

    pub(super) fn clamp_point_to_target_window(&self, x: f64, y: f64) -> (f64, f64) {
        let window = self.current_window_bounds();
        (
            x.clamp(window.x, window.x + window.w),
            y.clamp(window.y, window.y + window.h),
        )
    }

    /// OCR a fovea around `(cx, cy)` from the target window capture, never from a
    /// raw display rectangle. This is the default read path so a point inside one
    /// Firefox window cannot accidentally read pixels from another Firefox
    /// window.
    #[cfg(target_os = "macos")]
    pub(super) fn ocr_window_fovea(&self, cx: f64, cy: f64) -> dunst_core::Result<Vec<TextHit>> {
        const W: f64 = 680.0;
        const H: f64 = 420.0;
        let window = self.current_window_bounds();
        let region = clipped_region_to_window(
            Bbox {
                x: cx - W / 2.0,
                y: cy - H / 2.0,
                w: W,
                h: H,
            },
            window,
        )
        .ok_or_else(|| {
            DunstError::Perception("window fovea does not intersect target window".into())
        })?;
        self.read_text(Some(region), false)
    }

    /// OCR a small fovea of the **composited display** around `(cx, cy)` — the
    /// crosshair / value-at-cursor bubble renders near the cursor. Display capture
    /// includes GPU overlays a window capture misses, and reads any app's pixels.
    #[cfg(target_os = "macos")]
    pub(super) fn ocr_screen_fovea(&self, cx: f64, cy: f64) -> dunst_core::Result<Vec<TextHit>> {
        const W: f64 = 680.0;
        const H: f64 = 420.0;
        let (x, y) = (cx - W / 2.0, cy - H / 2.0);
        // `screencapture` grabs the COMPOSITED screen, including GPU/WebGL overlays
        // (a chart crosshair value bubble) that CoreGraphics window/display capture
        // miss. Its -R rect is in global screen points. App/browser agnostic. The
        // fovea is generous because the bubble renders at a data-dependent offset
        // from the cursor.
        let path = unique_png_path("dunst_fovea");
        let ok = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-t", "png", "-R"])
            .arg(format!("{x},{y},{W},{H}"))
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err(DunstError::Perception("screen fovea capture failed".into()));
        }
        let geom = dunst_vision::CaptureGeometry {
            window_origin_pt: (x, y),
            window_size_pt: (W, H),
            image_size_px: (W * 2.0, H * 2.0),
            backing_scale: 2.0,
        };
        let boxes = match dunst_vision::ocr::ocr_image_file(
            &path.to_string_lossy(),
            dunst_vision::ocr::RecognitionMode::Fast,
        ) {
            Ok(boxes) => boxes,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(DunstError::Perception(format!(
                    "screen fovea OCR failed: {e}"
                )));
            }
        };
        let _ = std::fs::remove_file(&path);
        Ok(boxes
            .into_iter()
            .map(|b| TextHit {
                text: b.text,
                bbox: dunst_vision::coords::vision_norm_to_screen_pt(b.norm, &geom),
                confidence: b.confidence,
            })
            .collect())
    }

    /// Single-point [`read_series`](Self::read_series): borrow the cursor, hover
    /// `(x, y)`, OCR around it, restore.
    pub fn read_at(&self, x: f64, y: f64, borrow_cursor: bool) -> dunst_core::Result<Vec<TextHit>> {
        Ok(self
            .read_series(&[(x, y)], borrow_cursor)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    /// Find OCR text in the target-window capture and return stable hit ids
    /// suitable for a follow-up `click_near_text`.
    pub fn find_ocr_text(
        &self,
        query: &str,
        content_only: bool,
        accurate: bool,
        limit: usize,
    ) -> dunst_core::Result<OcrTextSearchResult> {
        let query = query.trim();
        if query.is_empty() {
            return Err(DunstError::Execution(
                "find_ocr_text requires a non-empty query".into(),
            ));
        }
        let detailed = self.read_text_detailed(None, accurate, content_only)?;
        let needle = normalize_match(query);
        let mut hits: Vec<OcrTextHit> = detailed
            .hits
            .iter()
            .enumerate()
            .filter_map(|(idx, hit)| ocr_text_hit(idx, hit, &needle))
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.confidence
                        .partial_cmp(&a.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    a.bbox
                        .y
                        .partial_cmp(&b.bbox.y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    a.bbox
                        .x
                        .partial_cmp(&b.bbox.x)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        hits.truncate(limit.clamp(1, 50));
        let mut warnings = detailed.warnings;
        if hits.is_empty() {
            warnings.push(format!(
                "no OCR hit matched {query:?}; try read_text_detailed or set content_only=false if the target is browser chrome"
            ));
        }
        Ok(OcrTextSearchResult {
            query: query.to_string(),
            content_only,
            target_visibility: detailed.target_visibility,
            hits,
            warnings,
            recommended_next_steps: detailed.recommended_next_steps,
        })
    }

    /// Click the best OCR match by text, not by hand-picked coordinates. The
    /// click itself still goes through the raw-click approval gate, but the
    /// target point is now derived from a named OCR hit and returned for audit.
    pub fn click_near_text(
        &mut self,
        query: &str,
        content_only: bool,
        accurate: bool,
        occurrence: usize,
        expected_text: Option<&str>,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<OcrClickResult> {
        let search = self.find_ocr_text(query, content_only, accurate, occurrence.max(1))?;
        let hit = search
            .hits
            .get(occurrence.saturating_sub(1))
            .or_else(|| search.hits.first())
            .cloned()
            .ok_or_else(|| DunstError::ElementNotFound(format!("OCR text {query:?}")))?;
        let audit = self.click_ocr_text_hit(&hit, "click", reasoning)?;
        let (expected_text_found, verification_hint) = if audit.result == ActionResult::Success {
            match expected_text.map(str::trim).filter(|s| !s.is_empty()) {
                Some(expected) => {
                    let after = self.read_text_detailed(None, true, content_only)?;
                    let found = after
                        .hits
                        .iter()
                        .any(|text| normalized_contains_query(&normalize_match(&text.text), &normalize_match(expected)));
                    let hint = (!found).then(|| {
                        format!(
                            "click_near_text succeeded at the input layer, but expected text {expected:?} was not found afterward; treat the click as semantically unverified"
                        )
                    });
                    (Some(found), hint)
                }
                None => (
                    None,
                    Some(
                        "No expected_text postcondition was provided; re-read page_state/read_text_detailed before the next mutating action."
                            .into(),
                    ),
                ),
            }
        } else {
            (None, None)
        };
        Ok(OcrClickResult {
            query: query.to_string(),
            hit,
            audit,
            expected_text: expected_text.map(str::to_owned),
            expected_text_found,
            verification_hint,
        })
    }

    /// Detect a likely modal/overlay and safe close candidates. This is a
    /// conservative heuristic: it returns candidates when the UI exposes text
    /// such as "Close", "Fermer", "Not now", or "Plus tard"; it does not infer
    /// a close coordinate from decoration alone.
    pub fn detect_modal(&self) -> dunst_core::Result<ModalState> {
        let window = self.current_window_bounds();
        let modal_bbox = likely_modal_bbox(self.scene_graph(), window);
        let detailed = self.read_text_detailed(None, true, false)?;
        let mut close_candidates: Vec<_> = detailed
            .all_hits
            .iter()
            .enumerate()
            .filter_map(|(idx, hit)| modal_close_hit(idx, hit, modal_bbox))
            .collect();
        close_candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.bbox
                        .y
                        .partial_cmp(&b.bbox.y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    b.bbox
                        .x
                        .partial_cmp(&a.bbox.x)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        close_candidates.truncate(8);
        let detected = modal_bbox.is_some() || !close_candidates.is_empty();
        let reason = if modal_bbox.is_some() {
            Some("large centered overlay-like region detected".into())
        } else if !close_candidates.is_empty() {
            Some("modal close/dismiss text candidate detected by OCR".into())
        } else {
            None
        };
        let warning = (detected && close_candidates.is_empty()).then(|| {
            "modal-like region found, but no safe close text/button was detected; do not guess a raw close coordinate"
                .into()
        });
        Ok(ModalState {
            detected,
            modal_bbox,
            close_candidates,
            reason,
            warning,
        })
    }

    /// Dismiss a modal only when a close/dismiss OCR candidate exists. This
    /// deliberately refuses to click guessed corners or backdrop regions because
    /// those were the source of accidental restaurant/card opens in the trace.
    pub fn dismiss_modal(
        &mut self,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<ModalDismissResult> {
        let modal_before = self.detect_modal()?;
        let clicked = modal_before
            .close_candidates
            .first()
            .cloned()
            .ok_or_else(|| {
                DunstError::Execution(
                    "dismiss_modal found no safe OCR close/dismiss candidate; use detect_modal/read_text_detailed and avoid raw coordinate guesses"
                        .into(),
                )
            })?;
        let audit = self.click_ocr_text_hit(&clicked, "dismiss_modal", reasoning)?;
        let modal_after = (audit.result == ActionResult::Success)
            .then(|| self.detect_modal().ok())
            .flatten();
        let dismissed = modal_after.as_ref().map(|state| !state.detected);
        let verification_hint = match dismissed {
            Some(true) => None,
            Some(false) => Some(
                "dismiss_modal clicked a safe OCR candidate, but a modal still appears detected; re-read the UI before any raw click behind it."
                    .into(),
            ),
            None => Some(
                "dismiss_modal is pending approval or failed before verification; approve/retry only if the OCR candidate is the intended close control."
                    .into(),
            ),
        };
        Ok(ModalDismissResult {
            modal_before,
            clicked,
            audit,
            modal_after,
            dismissed,
            verification_hint,
        })
    }

    /// Group visible OCR lines into card-like candidates. This is intentionally
    /// heuristic but useful for web grids where AX exposes only a root group:
    /// restaurant/product cards become named click targets with facts and bboxes.
    pub fn extract_ocr_cards(
        &self,
        accurate: bool,
        content_only: bool,
        limit: usize,
    ) -> dunst_core::Result<OcrCardsResult> {
        let detailed = self.read_text_detailed(None, accurate, content_only)?;
        let mut hits = detailed.hits.clone();
        hits.sort_by(|a, b| {
            a.bbox
                .y
                .partial_cmp(&b.bbox.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.bbox
                        .x
                        .partial_cmp(&b.bbox.x)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let mut cards = Vec::new();
        for (idx, title_hit) in hits.iter().enumerate() {
            if !looks_like_card_title(&title_hit.text) {
                continue;
            }
            let mut lines = vec![title_hit.text.clone()];
            let mut bbox = title_hit.bbox;
            let mut confidences = vec![title_hit.confidence];
            for next in hits.iter().skip(idx + 1) {
                if next.bbox.y < title_hit.bbox.y || next.bbox.y - title_hit.bbox.y > 128.0 {
                    continue;
                }
                if (next.bbox.x - title_hit.bbox.x).abs() > 96.0 {
                    continue;
                }
                lines.push(next.text.clone());
                bbox = bbox_union(bbox, next.bbox);
                confidences.push(next.confidence);
                if lines.len() >= 8 {
                    break;
                }
            }
            if lines.len() < 2 {
                continue;
            }
            let fields = card_fields(&lines);
            cards.push(OcrCard {
                id: format!(
                    "ocr_card_{}_{}",
                    cards.len(),
                    compact_ocr_label(&title_hit.text)
                ),
                bbox,
                title: title_hit.text.clone(),
                lines,
                rating: fields.rating,
                reviews: fields.reviews,
                eta: fields.eta,
                fee: fields.fee,
                promo: fields.promo,
                confidence: confidences.iter().sum::<f32>() / confidences.len() as f32,
            });
            if cards.len() >= limit.clamp(1, 50) {
                break;
            }
        }
        Ok(OcrCardsResult {
            target_visibility: detailed.target_visibility,
            content_region: detailed.content_region,
            cards,
            warnings: detailed.warnings,
            recommended_next_steps: detailed.recommended_next_steps,
        })
    }

    /// **Detect → confirm rendered → traverse → series.** Coarse-to-fine CV first
    /// answers "is a chart actually rendered (not a blank plot) and where" from a
    /// cheap window grab; only if present does it traverse the plot at mid-height,
    /// reading the value-at-cursor at `samples` points. Returns a blank-but-honest
    /// [`ScanResult`] (`present: false`) when there is nothing to read, instead of
    /// hovering an empty plot. macOS-only.
    #[cfg(target_os = "macos")]
    pub fn scan_chart(&self, samples: usize) -> dunst_core::Result<ScanResult> {
        // Make the (possibly backgrounded) window active WITHOUT raising it, so a
        // web canvas paints; give it a beat to render before we look.
        let focused = dunst_platform::focus_without_raise(self.target.window_id);
        if focused {
            // Give the just-activated web canvas time to paint before we look.
            std::thread::sleep(std::time::Duration::from_millis(900));
        }
        // Composited capture so the rendered curve (GPU canvas) is included —
        // CGWindowListCreateImage misses it.
        let captured = dunst_vision::capture::capture_window_composited(self.target.window_id)
            .map_err(|e| {
                DunstError::Perception(format!("chart scan requires a live window: {e}"))
            })?;
        // Read the chart by GEOMETRY — no hover, occlusion-proof: derive the plot
        // from the OCR'd axis labels, calibrate the Y axis from its price labels,
        // then map the curve's pixel height at each sampled x to a value. A chart
        // is "present" only if a curve actually covers most columns.
        let hits = self.read_text(None, false).unwrap_or_default();
        let Some(region) = region_from_axis(&hits) else {
            return Ok(ScanResult {
                present: false,
                focused,
                fill_ratio: 0.0,
                region: None,
                samples: Vec::new(),
            });
        };
        let calib = build_y_calibration(&hits, &region);
        let n = samples.clamp(2, 12);
        let xs: Vec<f64> = (0..n)
            .map(|k| {
                let f = if n > 1 {
                    k as f64 / (n - 1) as f64
                } else {
                    0.5
                };
                region.x + region.w * (0.03 + 0.94 * f)
            })
            .collect();
        let ys =
            dunst_vision::detect::curve_screen_y(&captured.image, &captured.geometry, &region, &xs);
        let found = ys.iter().filter(|y| y.is_some()).count();
        let present = found * 2 >= n; // a real curve covers most columns
        let samples_out: Vec<ChartSample> = xs
            .iter()
            .zip(ys)
            .map(|(&x, screen_y)| {
                let value = screen_y
                    .zip(calib.as_ref())
                    .map(|(sy, c)| format!("{:.2}", c.value_at(sy)));
                ChartSample {
                    x,
                    value,
                    time: nearest_time_label(&hits, x, &region),
                    raw: Vec::new(),
                }
            })
            .collect();
        Ok(ScanResult {
            present,
            focused,
            fill_ratio: found as f32 / n as f32,
            region: Some(region),
            samples: if present { samples_out } else { Vec::new() },
        })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scan_chart(&self, _samples: usize) -> dunst_core::Result<ScanResult> {
        Err(DunstError::Execution(
            "scan_chart requires a macOS backend".into(),
        ))
    }

    /// Make the target window AppKit-active **without raising it** (SkyLight
    /// focus-without-raise) so a backgrounded web canvas paints. Best-effort.
    #[cfg(target_os = "macos")]
    pub fn focus_window(&self) -> bool {
        dunst_platform::focus_without_raise(self.target.window_id)
    }
}

fn ocr_text_hit(idx: usize, hit: &TextHit, needle: &str) -> Option<OcrTextHit> {
    let haystack = normalize_match(&hit.text);
    if !normalized_contains_query(&haystack, needle) {
        return None;
    }
    let score = if haystack == needle {
        100.0
    } else if haystack.starts_with(needle) {
        80.0
    } else {
        60.0
    } + f64::from(hit.confidence) * 10.0;
    let center = (hit.bbox.x + hit.bbox.w / 2.0, hit.bbox.y + hit.bbox.h / 2.0);
    Some(OcrTextHit {
        id: format!("ocr_text_{idx}_{}", compact_ocr_label(&hit.text)),
        text: hit.text.clone(),
        bbox: hit.bbox,
        confidence: hit.confidence,
        center,
        score,
    })
}

fn compact_ocr_label(text: &str) -> String {
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
    out.trim_matches('_').to_string()
}

fn likely_modal_bbox(graph: &SceneGraph, window: Bbox) -> Option<Bbox> {
    let window_area = window.w.max(0.0) * window.h.max(0.0);
    if window_area <= 0.0 {
        return None;
    }
    graph
        .nodes
        .values()
        .filter(|node| matches!(node.role, Role::Group | Role::Unknown | Role::Window))
        .filter_map(|node| node.bbox)
        .filter(|bbox| rect_intersection_area(*bbox, window) > 0.0)
        .filter(|bbox| {
            let area = bbox.w.max(0.0) * bbox.h.max(0.0);
            let ratio = area / window_area;
            let cx = bbox.x + bbox.w / 2.0;
            let cy = bbox.y + bbox.h / 2.0;
            let wcx = window.x + window.w / 2.0;
            let wcy = window.y + window.h / 2.0;
            (0.12..=0.82).contains(&ratio)
                && (cx - wcx).abs() <= window.w * 0.22
                && (cy - wcy).abs() <= window.h * 0.24
        })
        .min_by(|a, b| {
            let aa = a.w * a.h;
            let ba = b.w * b.h;
            aa.partial_cmp(&ba).unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn modal_close_hit(idx: usize, hit: &TextHit, modal_bbox: Option<Bbox>) -> Option<OcrTextHit> {
    let text = normalize_match(&hit.text);
    let is_close = matches!(
        text.as_str(),
        "x" | "×"
            | "close"
            | "fermer"
            | "not now"
            | "maybe later"
            | "plus tard"
            | "non merci"
            | "no thanks"
            | "ignorer"
    ) || text.contains("fermer")
        || text.contains("not now")
        || text.contains("plus tard")
        || text.contains("non merci")
        || text.contains("no thanks");
    if !is_close {
        return None;
    }
    if modal_bbox
        .map(|bbox| rect_intersection_area(hit.bbox, bbox) <= 0.0)
        .unwrap_or(false)
    {
        return None;
    }
    let mut candidate = ocr_text_hit(idx, hit, &text)?;
    candidate.score += if matches!(text.as_str(), "x" | "×" | "close" | "fermer") {
        15.0
    } else {
        5.0
    };
    Some(candidate)
}

fn bbox_union(a: Bbox, b: Bbox) -> Bbox {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    Bbox {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

fn looks_like_card_title(text: &str) -> bool {
    let normalized = normalize_match(text);
    let trimmed = normalized.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if trimmed.contains("livraison")
        || trimmed.contains("frais")
        || trimmed.contains("min")
        || trimmed.contains("offert")
        || trimmed.contains("achete")
        || trimmed.contains("exclusive")
        || trimmed.contains("€")
        || trimmed.contains("avis")
    {
        return false;
    }
    trimmed.chars().any(|ch| ch.is_alphabetic())
}

#[derive(Default)]
struct CardFields {
    rating: Option<String>,
    reviews: Option<String>,
    eta: Option<String>,
    fee: Option<String>,
    promo: Option<String>,
}

fn card_fields(lines: &[String]) -> CardFields {
    let mut fields = CardFields::default();
    for line in lines.iter().skip(1) {
        let normalized = normalize_match(line);
        if fields.rating.is_none()
            && normalized.chars().any(|ch| ch.is_ascii_digit())
            && (normalized.contains('.') || normalized.contains(',') || normalized.contains('*'))
        {
            fields.rating = Some(line.clone());
        }
        if fields.reviews.is_none()
            && (normalized.contains('+') || normalized.contains("avis") || normalized.contains('('))
        {
            fields.reviews = Some(line.clone());
        }
        if fields.eta.is_none() && normalized.contains("min") {
            fields.eta = Some(line.clone());
        }
        if fields.fee.is_none() && (normalized.contains("livraison") || normalized.contains("0 €"))
        {
            fields.fee = Some(line.clone());
        }
        if fields.promo.is_none()
            && (normalized.contains("offert")
                || normalized.contains("promo")
                || normalized.contains("exclusive")
                || normalized.contains("achete"))
        {
            fields.promo = Some(line.clone());
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(text: &str, x: f64, y: f64) -> TextHit {
        TextHit {
            text: text.into(),
            bbox: Bbox {
                x,
                y,
                w: 120.0,
                h: 20.0,
            },
            confidence: 0.95,
        }
    }

    #[test]
    fn card_title_filter_drops_metadata_lines() {
        assert!(looks_like_card_title("Osaka Brest"));
        assert!(!looks_like_card_title("Frais de livraison à 0 €"));
        assert!(!looks_like_card_title("4.6* (4000+) • 12 min"));
    }

    #[test]
    fn card_fields_extract_rating_eta_fee_and_promo() {
        let lines = vec![
            "Pepe Chicken".to_string(),
            "Frais de livraison à 0 €".to_string(),
            "4.6* (2000+) • 10 min".to_string(),
            "1 acheté = 1 offert".to_string(),
        ];
        let fields = card_fields(&lines);
        assert_eq!(fields.fee.as_deref(), Some("Frais de livraison à 0 €"));
        assert_eq!(fields.rating.as_deref(), Some("4.6* (2000+) • 10 min"));
        assert_eq!(fields.reviews.as_deref(), Some("4.6* (2000+) • 10 min"));
        assert_eq!(fields.eta.as_deref(), Some("4.6* (2000+) • 10 min"));
        assert_eq!(fields.promo.as_deref(), Some("1 acheté = 1 offert"));
    }

    #[test]
    fn modal_close_hit_accepts_safe_close_text_inside_modal() {
        let modal = Bbox {
            x: 100.0,
            y: 100.0,
            w: 400.0,
            h: 300.0,
        };
        let candidate = modal_close_hit(0, &hit("Fermer", 440.0, 120.0), Some(modal))
            .expect("close text inside modal should be accepted");
        assert!(candidate.score > 100.0);
    }

    #[test]
    fn modal_close_hit_rejects_close_text_outside_modal() {
        let modal = Bbox {
            x: 100.0,
            y: 100.0,
            w: 400.0,
            h: 300.0,
        };
        assert!(modal_close_hit(0, &hit("Fermer", 700.0, 700.0), Some(modal)).is_none());
    }
}
