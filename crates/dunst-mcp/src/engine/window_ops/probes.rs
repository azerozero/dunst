use super::*;

impl Engine {
    /// Pixel-grid probe over a screen region. This is a cheap movement/change
    /// detector: it samples a spaced luminance grid, compares it with the previous
    /// probe for the same region/grid, and optionally triggers a full AX refresh
    /// if pixels changed. AX itself cannot refresh only a rectangle.
    #[cfg(target_os = "macos")]
    pub fn visual_change_probe(
        &mut self,
        region: Option<Bbox>,
        columns: usize,
        rows: usize,
        threshold: u8,
        refresh_on_change: bool,
    ) -> dunst_core::Result<VisualChangeProbe> {
        let region = region.unwrap_or_else(|| self.current_window_bounds());
        self.ensure_region_in_target_window(region, "visual_change_probe")?;
        if region.w <= 0.0 || region.h <= 0.0 {
            return Err(VisualOpsError::Perception(
                "visual_change_probe region width/height must be positive".into(),
            ));
        }
        let columns = columns.clamp(2, 128);
        let rows = rows.clamp(2, 128);
        let captured =
            dunst_vision::capture::capture_screen_rect(region.x, region.y, region.w, region.h)
                .map_err(|err| {
                    VisualOpsError::Perception(format!("visual probe capture failed: {err}"))
                })?;
        let signature = dunst_vision::capture::sample_luma_signature(&captured, columns, rows)
            .ok_or_else(|| {
                VisualOpsError::Perception("visual probe could not sample captured pixels".into())
            })?;
        let key = visual_probe_key(region, columns, rows);
        let previous = self.visual_probe_cache.borrow().clone();
        let (baseline, cells_changed, max_delta, mean_delta) = match previous {
            Some(prev) if prev.key == key && prev.signature.len() == signature.len() => {
                let (cells_changed, max_delta, mean_delta) =
                    compare_signatures(&prev.signature, &signature, threshold);
                (false, cells_changed, max_delta, mean_delta)
            }
            _ => (true, 0, 0, 0.0),
        };
        *self.visual_probe_cache.borrow_mut() = Some(VisualProbeCacheEntry { key, signature });
        let changed = !baseline && cells_changed > 0;
        let mut refreshed = false;
        if changed && refresh_on_change {
            self.refresh()?;
            refreshed = true;
        }
        Ok(VisualChangeProbe {
            changed,
            baseline,
            refreshed,
            region,
            columns,
            rows,
            cells_total: columns * rows,
            cells_changed,
            threshold,
            max_delta,
            mean_delta,
        })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn visual_change_probe(
        &mut self,
        _region: Option<Bbox>,
        _columns: usize,
        _rows: usize,
        _threshold: u8,
        _refresh_on_change: bool,
    ) -> dunst_core::Result<VisualChangeProbe> {
        Err(VisualOpsError::Perception(
            "visual_change_probe requires a macOS backend".into(),
        ))
    }

    /// Analyze only a screen region through AX hit-tests. This samples a grid of
    /// points with `AXUIElementCopyElementAtPosition` and returns the unique
    /// shallow AX elements found there. It is not a full subtree refresh, but it
    /// is a targeted AX read for "what is in this rectangle?".
    #[cfg(target_os = "macos")]
    pub fn analyze_region_ax(
        &self,
        region: Option<Bbox>,
        columns: usize,
        rows: usize,
    ) -> RegionAxAnalysis {
        let region = region.unwrap_or_else(|| self.current_window_bounds());
        if let Err(err) = self.ensure_region_in_target_window(region, "analyze_region_ax") {
            return RegionAxAnalysis {
                region,
                columns,
                rows,
                points_total: columns * rows,
                hits: 0,
                unique_elements: Vec::new(),
                samples: vec![RegionAxSample {
                    x: region.x + region.w / 2.0,
                    y: region.y + region.h / 2.0,
                    element_key: None,
                    error: Some(err.to_string()),
                }],
            };
        }
        let columns = columns.clamp(1, 64);
        let rows = rows.clamp(1, 64);
        let mut by_key: BTreeMap<String, RegionAxElement> = BTreeMap::new();
        let mut samples = Vec::with_capacity(columns * rows);

        for row in 0..rows {
            let y = region.y + (row as f64 + 0.5) * region.h / rows as f64;
            for col in 0..columns {
                let x = region.x + (col as f64 + 0.5) * region.w / columns as f64;
                match dunst_platform::element_at_point(self.target.pid, x, y) {
                    Ok(node) => {
                        let key = region_ax_key(&node);
                        by_key
                            .entry(key.clone())
                            .or_insert_with(|| region_ax_element(key.clone(), node))
                            .sample_count += 1;
                        samples.push(RegionAxSample {
                            x,
                            y,
                            element_key: Some(key),
                            error: None,
                        });
                    }
                    Err(err) => samples.push(RegionAxSample {
                        x,
                        y,
                        element_key: None,
                        error: Some(err.to_string()),
                    }),
                }
            }
        }

        RegionAxAnalysis {
            region,
            columns,
            rows,
            points_total: columns * rows,
            hits: samples
                .iter()
                .filter(|sample| sample.element_key.is_some())
                .count(),
            unique_elements: by_key.into_values().collect(),
            samples,
        }
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn analyze_region_ax(
        &self,
        region: Option<Bbox>,
        columns: usize,
        rows: usize,
    ) -> RegionAxAnalysis {
        RegionAxAnalysis {
            region: region.unwrap_or(Bbox {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            }),
            columns,
            rows,
            points_total: columns * rows,
            hits: 0,
            unique_elements: Vec::new(),
            samples: Vec::new(),
        }
    }
}
