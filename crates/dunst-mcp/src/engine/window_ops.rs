use super::*;

impl Engine {
    /// Active display topology: resolution in pixels, bounds in global screen
    /// points, scale factor, and Dunst's 1-based display index.
    #[cfg(target_os = "macos")]
    pub fn list_displays(&self) -> Vec<DisplaySummary> {
        if let Some(displays) = self
            .display_cache
            .borrow()
            .as_ref()
            .and_then(|c| c.fresh(DISPLAY_CACHE_TTL))
        {
            return displays;
        }
        let displays: Vec<DisplaySummary> = dunst_vision::capture::list_displays()
            .into_iter()
            .map(display_summary)
            .collect();
        *self.display_cache.borrow_mut() = Some(TimedCache {
            captured_at: Instant::now(),
            value: displays.clone(),
        });
        displays
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn list_displays(&self) -> Vec<DisplaySummary> {
        Vec::new()
    }

    /// A compact scoped view of the target window and owning display. This is the
    /// "zoom into the window" read path: no full scene graph, no screenshot.
    pub fn window_view(&self, limit: usize) -> WindowView {
        let page = self.page_state(limit);
        let window = self.current_window_bounds();
        let display = self.display_for_window(window);
        let window_in_display = display.as_ref().map(|d| Bbox {
            x: window.x - d.bounds.x,
            y: window.y - d.bounds.y,
            w: window.w,
            h: window.h,
        });
        WindowView {
            target: page.target,
            title: page.title,
            url: page.url,
            window,
            display,
            window_in_display,
            visible_text: page.visible_text,
            key_elements: page.key_elements,
        }
    }

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
                .map_err(|e| {
                    VisualOpsError::Perception(format!("visual probe capture failed: {e}"))
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
            hits: samples.iter().filter(|s| s.element_key.is_some()).count(),
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

    /// Move the target window to the display index returned by `list_displays`.
    /// The default behaviour preserves the window size but clamps it inside the
    /// target display, then centres it.
    pub fn move_window_to_display(
        &mut self,
        display_index: usize,
        preserve_size: bool,
    ) -> dunst_core::Result<WindowView> {
        let displays = self.list_displays();
        let display = displays
            .iter()
            .find(|d| d.index == display_index)
            .ok_or_else(|| {
                VisualOpsError::Execution(format!(
                    "display index {display_index} not found; call list_displays first"
                ))
            })?;
        let current = self.current_window_bounds();
        let (x, y, w, h) = target_frame_for_display(current, &display.bounds, preserve_size, 0);
        dunst_platform::set_window_frame(
            self.target.pid,
            self.target.window_id,
            x,
            y,
            Some(w),
            Some(h),
        )?;
        *self.desktop_cache.borrow_mut() = None;
        self.refresh()?;
        Ok(self.window_view(12))
    }

    /// Move every sizeable top-level window owned by `app` to a display.
    #[cfg(target_os = "macos")]
    pub fn move_app_to_display(
        &self,
        app: &str,
        display_index: usize,
        preserve_size: bool,
    ) -> dunst_core::Result<MoveAppResult> {
        let needle = normalize_match(app);
        let display = self
            .list_displays()
            .into_iter()
            .find(|d| d.index == display_index)
            .ok_or_else(|| {
                VisualOpsError::Execution(format!(
                    "display index {display_index} not found; call list_displays first"
                ))
            })?;
        let windows: Vec<_> = dunst_vision::capture::list_windows()
            .into_iter()
            .filter(|w| {
                w.w >= 300.0
                    && w.h >= 200.0
                    && !w.title.trim().is_empty()
                    && normalize_match(&w.app).contains(&needle)
            })
            .collect();
        if windows.is_empty() {
            return Err(VisualOpsError::Execution(format!(
                "no drivable windows found for app {app:?}"
            )));
        }

        let mut moved_windows = Vec::new();
        for (offset, window) in windows.into_iter().enumerate() {
            let current = Bbox {
                x: window.x,
                y: window.y,
                w: window.w,
                h: window.h,
            };
            let (x, y, w, h) =
                target_frame_for_display(current, &display.bounds, preserve_size, offset);
            dunst_platform::set_window_frame(window.pid, window.window_id, x, y, Some(w), Some(h))?;
            *self.desktop_cache.borrow_mut() = None;
            moved_windows.push(WindowSummary {
                window_id: window.window_id,
                pid: window.pid,
                app: window.app,
                title: window.title,
                bounds: Bbox { x, y, w, h },
                on_screen: window.on_screen,
            });
        }
        Ok(MoveAppResult {
            app: app.to_string(),
            display,
            moved: moved_windows.len(),
            windows: moved_windows,
        })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn move_app_to_display(
        &self,
        _app: &str,
        _display_index: usize,
        _preserve_size: bool,
    ) -> dunst_core::Result<MoveAppResult> {
        Err(VisualOpsError::Execution(
            "move_app_to_display requires a macOS backend".into(),
        ))
    }

    /// Whole-desktop window topology: displays, top-level windows, front/back
    /// order, and geometric overlaps. `all=false` filters to sizeable titled
    /// windows, matching `list_windows`.
    #[cfg(target_os = "macos")]
    pub fn desktop_view(&self, all: bool) -> DesktopView {
        let key = DesktopCacheKey { all };
        if let Some(cached) = self
            .desktop_cache
            .borrow()
            .as_ref()
            .and_then(|c| c.fresh(DISPLAY_CACHE_TTL))
        {
            if cached.key == key {
                return cached.view;
            }
        }
        let displays = self.list_displays();
        let degraded_reason = displays.is_empty().then(|| {
            "CoreGraphics returned no valid display with non-zero bounds/pixels; run in a live macOS GUI session with Screen Recording permission"
                .to_string()
        });
        let windows: Vec<_> = dunst_vision::capture::list_windows()
            .into_iter()
            .enumerate()
            .filter(|(_, w)| all || (w.w >= 300.0 && w.h >= 200.0 && !w.title.trim().is_empty()))
            .map(|(z_order, w)| {
                let bounds = Bbox {
                    x: w.x,
                    y: w.y,
                    w: w.w,
                    h: w.h,
                };
                let display = displays
                    .iter()
                    .find(|d| rect_intersection_area(bounds, d.bounds) > 0.0)
                    .cloned();
                DesktopWindow {
                    window_id: w.window_id,
                    pid: w.pid,
                    app: w.app,
                    title: w.title,
                    bounds,
                    on_screen: w.on_screen,
                    z_order,
                    is_frontmost: false,
                    display,
                    covered_by: Vec::new(),
                    covers: Vec::new(),
                }
            })
            .collect();
        let view = desktop_view_from_windows(displays, windows, degraded_reason);
        *self.desktop_cache.borrow_mut() = Some(TimedCache {
            captured_at: Instant::now(),
            value: DesktopCacheEntry {
                key,
                view: view.clone(),
            },
        });
        view
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn desktop_view(&self, _all: bool) -> DesktopView {
        DesktopView {
            degraded: true,
            reason: Some("desktop_view requires a macOS backend".into()),
            displays: Vec::new(),
            windows: Vec::new(),
            frontmost: None,
        }
    }

    /// Arrange selected windows onto one display. Selection must be explicit:
    /// pass `window_ids`, an `app` substring, or `all=true`.
    #[cfg(target_os = "macos")]
    pub fn arrange_windows(
        &self,
        display_index: usize,
        mode: &str,
        app: Option<&str>,
        window_ids: &[u32],
        all: bool,
    ) -> dunst_core::Result<ArrangeResult> {
        if !all && app.is_none() && window_ids.is_empty() {
            return Err(VisualOpsError::Execution(
                "arrange_windows requires window_ids, app, or all=true".into(),
            ));
        }
        let display = self
            .list_displays()
            .into_iter()
            .find(|d| d.index == display_index)
            .ok_or_else(|| {
                VisualOpsError::Execution(format!(
                    "display index {display_index} not found; call list_displays first"
                ))
            })?;
        let app_needle = app.map(normalize_match);
        let ids = window_ids.iter().copied().collect::<BTreeSet<_>>();
        let mut selected: Vec<_> = dunst_vision::capture::list_windows()
            .into_iter()
            .filter(|w| w.w >= 300.0 && w.h >= 200.0 && !w.title.trim().is_empty())
            .filter(|w| {
                all || ids.contains(&w.window_id)
                    || app_needle
                        .as_ref()
                        .is_some_and(|needle| normalize_match(&w.app).contains(needle))
            })
            .collect();
        selected.sort_by_key(|w| w.window_id);
        if selected.is_empty() {
            return Err(VisualOpsError::Execution(
                "arrange_windows found no matching drivable windows".into(),
            ));
        }

        let frames = layout_frames(selected.len(), &display.bounds, mode)?;
        let mut moved_windows = Vec::new();
        for (window, frame) in selected.into_iter().zip(frames) {
            dunst_platform::set_window_frame(
                window.pid,
                window.window_id,
                frame.x,
                frame.y,
                Some(frame.w),
                Some(frame.h),
            )?;
            *self.desktop_cache.borrow_mut() = None;
            moved_windows.push(WindowSummary {
                window_id: window.window_id,
                pid: window.pid,
                app: window.app,
                title: window.title,
                bounds: frame,
                on_screen: window.on_screen,
            });
        }

        Ok(ArrangeResult {
            display,
            mode: mode.to_ascii_lowercase(),
            moved: moved_windows.len(),
            windows: moved_windows,
        })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn arrange_windows(
        &self,
        _display_index: usize,
        _mode: &str,
        _app: Option<&str>,
        _window_ids: &[u32],
        _all: bool,
    ) -> dunst_core::Result<ArrangeResult> {
        Err(VisualOpsError::Execution(
            "arrange_windows requires a macOS backend".into(),
        ))
    }

    /// Enumerate top-level windows for picking a `window_id` to drive — the MCP's
    /// own target discovery (no external tool). By default returns only **real,
    /// drivable** windows (a sizeable content window), dropping the tab-strip /
    /// shadow / menubar fragments that swamp the raw list; pass `all` for every
    /// layer-0 window.
    #[cfg(target_os = "macos")]
    pub fn list_windows(&self, all: bool) -> Vec<WindowSummary> {
        dunst_vision::capture::list_windows()
            .into_iter()
            .filter(|w| all || (w.w >= 300.0 && w.h >= 200.0 && !w.title.trim().is_empty()))
            .map(|w| WindowSummary {
                window_id: w.window_id,
                pid: w.pid,
                app: w.app,
                title: w.title,
                bounds: Bbox {
                    x: w.x,
                    y: w.y,
                    w: w.w,
                    h: w.h,
                },
                on_screen: w.on_screen,
            })
            .collect()
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn list_windows(&self, _all: bool) -> Vec<WindowSummary> {
        Vec::new()
    }

    /// Composited screenshot of the target window as base64 PNG — lets the agent
    /// SEE the pixels directly (multimodal), alongside OCR/CV. Works backgrounded.
    #[cfg(target_os = "macos")]
    pub fn screenshot(&self) -> Option<String> {
        if let Some(cached) = self
            .screenshot_cache
            .borrow()
            .as_ref()
            .and_then(|c| c.fresh(SCREENSHOT_CACHE_TTL))
        {
            return Some(cached);
        }
        let path = unique_png_path("dunst_shot");
        let ok = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", &format!("-l{}", self.target.window_id)])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return None;
        }
        let bytes = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        let encoded = bytes.map(|b| base64_encode(&b))?;
        *self.screenshot_cache.borrow_mut() = Some(TimedCache {
            captured_at: Instant::now(),
            value: encoded.clone(),
        });
        Some(encoded)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn screenshot(&self) -> Option<String> {
        None
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn focus_window(&self) -> bool {
        false
    }
}
