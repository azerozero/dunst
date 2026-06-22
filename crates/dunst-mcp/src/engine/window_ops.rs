use super::*;

mod probes;

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
            browser_tab: page.browser_tab,
            target_visibility: page.target_visibility,
            window,
            display,
            window_in_display,
            visible_text: page.visible_text,
            key_elements: page.key_elements,
        }
    }

    /// Current target's visibility in the desktop stack. Read-only; callers use
    /// this to decide whether OCR/screenshot/raw pointer actions are trustworthy.
    pub fn target_visibility(&self) -> TargetVisibility {
        let view = self.desktop_view(false);
        target_visibility_from_desktop(
            self.target.window_id,
            self.scene_graph().window.title.clone(),
            self.current_window_bounds(),
            &view,
        )
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
                DunstError::Execution(format!(
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
                DunstError::Execution(format!(
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
            return Err(DunstError::Execution(format!(
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
        Err(DunstError::Execution(
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
            return Err(DunstError::Execution(
                "arrange_windows requires window_ids, app, or all=true".into(),
            ));
        }
        let display = self
            .list_displays()
            .into_iter()
            .find(|d| d.index == display_index)
            .ok_or_else(|| {
                DunstError::Execution(format!(
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
            return Err(DunstError::Execution(
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

    /// Try to expose the attached target window and verify whether the desktop
    /// stack now makes it visible. This is deliberately narrower than arranging
    /// every app window: it first raises only the target, then optionally moves
    /// covering windows into a side-by-side layout on the target display.
    #[cfg(target_os = "macos")]
    pub fn expose_target_window(
        &mut self,
        arrange_if_needed: bool,
    ) -> dunst_core::Result<ExposeTargetWindowResult> {
        let before = self.target_visibility();
        let mut raised = false;
        let mut arranged = false;
        let mut raise_audit = None;

        if !before.is_frontmost || !before.covered_by.is_empty() {
            let node_id = self
                .scene_graph()
                .nodes
                .values()
                .find(|node| node.role == Role::Window)
                .map(|node| node.id.clone())
                .unwrap_or_else(|| format!("win_{}", self.target.window_id));
            match self.raise_element(
                &node_id,
                Some("expose target window before visual interaction"),
            ) {
                Ok(entry) => {
                    raised = entry.result == ActionResult::Success;
                    raise_audit = Some(entry);
                }
                Err(_) => {
                    raised = false;
                }
            }
        }

        *self.desktop_cache.borrow_mut() = None;
        let mut after = self.target_visibility();
        if arrange_if_needed && raised && !after.covered_by.is_empty() {
            let mut ids = vec![self.target.window_id];
            ids.extend(after.covered_by.iter().map(|window| window.window_id));
            ids.sort_unstable();
            ids.dedup();
            let display = after
                .covered_by
                .iter()
                .find_map(|window| window.display.as_ref().map(|d| d.index))
                .or_else(|| {
                    self.display_for_window(self.current_window_bounds())
                        .map(|d| d.index)
                })
                .unwrap_or(1);
            let _ = self.arrange_windows(display, "columns", None, &ids, false);
            arranged = true;
            *self.desktop_cache.borrow_mut() = None;
            after = self.target_visibility();
        }

        let verification_hint = if raise_audit
            .as_ref()
            .is_some_and(|entry| entry.result == ActionResult::PendingApproval)
        {
            Some("Target expose is pending approval; approve the raise_audit.target_id, then retry expose_target_window.".into())
        } else if !after.covered_by.is_empty() {
            Some("Target remains covered after expose_target_window; use desktop_view to choose the covering window or move the target to another display.".into())
        } else {
            None
        };
        Ok(ExposeTargetWindowResult {
            before,
            after,
            raise_audit,
            raised,
            arranged,
            verification_hint,
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn expose_target_window(
        &mut self,
        _arrange_if_needed: bool,
    ) -> dunst_core::Result<ExposeTargetWindowResult> {
        let before = self.target_visibility();
        Ok(ExposeTargetWindowResult {
            after: before.clone(),
            before,
            raise_audit: None,
            raised: false,
            arranged: false,
            verification_hint: Some("expose_target_window requires a macOS backend".into()),
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
        Err(DunstError::Execution(
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
    pub fn screenshot(&self) -> Option<ScreenshotResult> {
        if let Some(cached) = self
            .screenshot_cache
            .borrow()
            .as_ref()
            .and_then(|c| c.fresh(SCREENSHOT_CACHE_TTL))
        {
            let target_visibility = self.target_visibility();
            return Some(ScreenshotResult {
                png_base64: cached,
                warnings: target_visibility.warnings.clone(),
                recommended_next_steps: target_visibility
                    .fallback_hint
                    .clone()
                    .into_iter()
                    .collect(),
                target_visibility,
            });
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
        let target_visibility = self.target_visibility();
        Some(ScreenshotResult {
            png_base64: encoded,
            warnings: target_visibility.warnings.clone(),
            recommended_next_steps: target_visibility
                .fallback_hint
                .clone()
                .into_iter()
                .collect(),
            target_visibility,
        })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn screenshot(&self) -> Option<ScreenshotResult> {
        None
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn focus_window(&self) -> bool {
        false
    }
}
