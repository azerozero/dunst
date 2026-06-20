use super::*;

impl Engine {
    /// OCR the target window via Apple Vision (P1). A pure **read probe** like the
    /// scene-graph getters: it does **not** risk-gate and records **no** audit entry.
    /// `region_screen_pt` limits OCR to a screen-point rectangle; `None` reads the
    /// whole window. Each hit's bbox is mapped from Vision's normalised space to
    /// screen points. macOS-only — see the non-macOS stub below.
    #[cfg(target_os = "macos")]
    pub fn read_text(
        &self,
        region_screen_pt: Option<Bbox>,
        accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        use dunst_vision::ocr::RecognitionMode;
        if let Some(region) = region_screen_pt {
            if region.w <= 0.0 || region.h <= 0.0 {
                return Err(VisualOpsError::Perception(
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
        let captured = dunst_vision::capture::capture_window_composited(self.target.window_id)
            .map_err(|err| {
                VisualOpsError::Perception(format!(
                    "OCR requires a live macOS window (capture failed: {err})"
                ))
            })?;
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
                return Err(VisualOpsError::Perception(format!("OCR failed: {err}")));
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
    #[cfg(not(target_os = "macos"))]
    pub fn read_text(
        &self,
        _region_screen_pt: Option<Bbox>,
        _accurate: bool,
    ) -> dunst_core::Result<Vec<TextHit>> {
        Err(VisualOpsError::Perception(
            "OCR requires a live macOS window".into(),
        ))
    }

    /// Detect geometric primitives (rect/bar/circle/line) in the target window
    /// via the CV `shapes` layer — the figures (charts, custom-drawn UI) AX and
    /// OCR can't expose. A pure **read probe** like [`read_text`](Self::read_text):
    /// no risk-gating, no audit entry. macOS-only.
    #[cfg(target_os = "macos")]
    pub fn read_shapes(&self) -> dunst_core::Result<Vec<ShapeHit>> {
        // Composited capture (see read_text): CGWindowListCreateImage is blank for
        // GPU/WebGL-rendered windows, so grab what is actually on screen instead.
        let captured = dunst_vision::capture::capture_window_composited(self.target.window_id)
            .map_err(|err| {
                VisualOpsError::Perception(format!(
                    "shape detection requires a live macOS window (capture failed: {err})"
                ))
            })?;
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
    #[cfg(not(target_os = "macos"))]
    pub fn read_shapes(&self) -> dunst_core::Result<Vec<ShapeHit>> {
        Err(VisualOpsError::Perception(
            "shape detection requires a live macOS window".into(),
        ))
    }
}
