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
        Err(VisualOpsError::Execution(
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
            VisualOpsError::Perception("window fovea does not intersect target window".into())
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
            return Err(VisualOpsError::Perception(
                "screen fovea capture failed".into(),
            ));
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
                return Err(VisualOpsError::Perception(format!(
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
                VisualOpsError::Perception(format!("chart scan requires a live window: {e}"))
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
        Err(VisualOpsError::Execution(
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
