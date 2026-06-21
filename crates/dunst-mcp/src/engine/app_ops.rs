use super::*;

impl Engine {
    /// List running GUI apps (those owning at least one top-level window),
    /// aggregated from the window list — coarser discovery than `list_windows`:
    /// which app to `launch_app`/`attach`, and whether it is already running.
    /// `query` filters by case-insensitive substring of the app name (doubles as
    /// "search app"). Sorted by window count desc, then name. The `pid` is the
    /// owner of its first window — pass any of an app's windows to `attach`.
    #[cfg(target_os = "macos")]
    pub fn list_apps(&self, query: Option<&str>) -> Vec<AppSummary> {
        use std::collections::BTreeMap;
        let needle = query.map(|q| q.trim().to_lowercase());
        // app name -> (pid, window_count, any_on_screen)
        let mut by_app: BTreeMap<String, (i32, usize, bool)> = BTreeMap::new();
        for w in dunst_vision::capture::list_windows() {
            if w.app.trim().is_empty() {
                continue;
            }
            if let Some(n) = &needle {
                if !w.app.to_lowercase().contains(n.as_str()) {
                    continue;
                }
            }
            let e = by_app.entry(w.app).or_insert((w.pid, 0, false));
            e.1 += 1;
            e.2 |= w.on_screen;
        }
        let mut apps: Vec<AppSummary> = by_app
            .into_iter()
            .map(|(app, (pid, windows, on_screen))| AppSummary {
                app,
                pid,
                windows,
                on_screen,
            })
            .collect();
        apps.sort_by(|a, b| b.windows.cmp(&a.windows).then_with(|| a.app.cmp(&b.app)));
        apps
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn list_apps(&self, _query: Option<&str>) -> Vec<AppSummary> {
        Vec::new()
    }

    /// List installed `.app` bundles without launching them. Reads
    /// `Contents/Info.plist` metadata from the standard macOS application roots.
    #[cfg(target_os = "macos")]
    pub fn list_launchable_apps(&self, query: Option<&str>, limit: usize) -> Vec<LaunchableApp> {
        let needle = query.map(normalize_match);
        let running = self
            .list_apps(None)
            .into_iter()
            .map(|a| normalize_match(&a.app))
            .collect::<BTreeSet<_>>();

        let mut apps = Vec::new();
        let mut seen = BTreeSet::new();
        for root in app_search_roots() {
            collect_app_bundles(
                &root,
                0,
                &mut seen,
                &mut apps,
                limit.max(1).saturating_mul(4),
            );
        }

        let mut out: Vec<LaunchableApp> = apps
            .into_iter()
            .filter_map(|path| launchable_app_from_bundle(&path, &running))
            .filter(|app| {
                let Some(n) = needle.as_ref() else {
                    return true;
                };
                normalize_match(&app.name).contains(n)
                    || normalize_match(&app.display_name).contains(n)
                    || app
                        .bundle_id
                        .as_deref()
                        .map(normalize_match)
                        .is_some_and(|b| b.contains(n))
            })
            .collect();
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        });
        out.truncate(limit.clamp(1, 500));
        out
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn list_launchable_apps(&self, _query: Option<&str>, _limit: usize) -> Vec<LaunchableApp> {
        Vec::new()
    }

    /// Resolve one installed app by bundle path, bundle id, or display/name,
    /// without launching it.
    #[cfg(target_os = "macos")]
    pub fn app_info(
        &self,
        app: Option<&str>,
        bundle_id: Option<&str>,
        path: Option<&str>,
    ) -> Option<LaunchableApp> {
        let running = self
            .list_apps(None)
            .into_iter()
            .map(|a| normalize_match(&a.app))
            .collect::<BTreeSet<_>>();
        if let Some(path) = path {
            return launchable_app_from_bundle(Path::new(path), &running);
        }

        let app_needle = app.map(normalize_match);
        let bundle_needle = bundle_id.map(normalize_match);
        self.list_launchable_apps(None, 500)
            .into_iter()
            .find(|candidate| {
                bundle_needle.as_ref().is_some_and(|needle| {
                    candidate
                        .bundle_id
                        .as_deref()
                        .map(normalize_match)
                        .is_some_and(|b| b == *needle)
                }) || app_needle.as_ref().is_some_and(|needle| {
                    normalize_match(&candidate.name) == *needle
                        || normalize_match(&candidate.display_name) == *needle
                        || normalize_match(&candidate.name).contains(needle)
                        || normalize_match(&candidate.display_name).contains(needle)
                })
            })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn app_info(
        &self,
        _app: Option<&str>,
        _bundle_id: Option<&str>,
        _path: Option<&str>,
    ) -> Option<LaunchableApp> {
        None
    }

    /// Launch an app **without bringing it to the foreground** (`open -g`),
    /// optionally opening `url` in it. Closes the last external dependency — the
    /// agent can now start a target itself, then list_windows + attach.
    ///
    /// `extra_args` are passed straight to the app's argv (`open … --args …`),
    /// which only takes effect when this call actually *launches* the app (not if
    /// it is already running). The motivating case: a backgrounded Chromium paints
    /// nothing because the OS marks its never-foregrounded window occluded and the
    /// Page-Visibility API pauses the `<canvas>` — so `scan_chart` reads a blank
    /// plot. Launching with `--disable-features=CalculateNativeWinOcclusion`
    /// `--disable-renderer-backgrounding` `--disable-background-timer-throttling`
    /// `--disable-backgrounding-occluded-windows` keeps it painting while it stays
    /// in the background (verified: TradingView curve renders, frontmost ≠ Chrome).
    #[cfg(target_os = "macos")]
    pub fn launch_app(
        &self,
        app: &str,
        url: Option<&str>,
        extra_args: &[String],
    ) -> LaunchAppResult {
        let mut cmd = std::process::Command::new("/usr/bin/open");
        cmd.args(["-g", "-a", app]);
        // `open` treats paths/URLs before `--args` as documents to open, and
        // everything after `--args` as application argv. Keep the URL before
        // `--args`; otherwise Chrome/Firefox can launch but stay on a new tab.
        if let Some(u) = url {
            cmd.arg(u);
        }
        if !extra_args.is_empty() {
            cmd.arg("--args");
            cmd.args(extra_args);
        }
        let launched = cmd.status().map(|s| s.success()).unwrap_or(false);
        std::thread::sleep(Duration::from_millis(350));
        self.launch_app_result(app, url, launched)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn launch_app(
        &self,
        app: &str,
        url: Option<&str>,
        _extra_args: &[String],
    ) -> LaunchAppResult {
        self.launch_app_result(app, url, false)
    }

    fn launch_app_result(&self, app: &str, url: Option<&str>, launched: bool) -> LaunchAppResult {
        let app_needle = normalize_match(app);
        let matching_windows: Vec<WindowSummary> = self
            .list_windows(false)
            .into_iter()
            .filter(|window| normalize_match(&window.app).contains(&app_needle))
            .collect();
        let target_window_title = self.scene_graph().window.title.clone();
        let target_matches_requested_app =
            normalize_match(&self.scene_graph().window.app_name).contains(&app_needle);
        let target = TargetState {
            pid: self.target.pid,
            window_id: self.target.window_id,
            app_name: self.scene_graph().window.app_name.clone(),
        };
        let verification_hint = if !target_matches_requested_app {
            Some("target window is not owned by the requested app; call list_windows and attach before acting".to_string())
        } else if url.is_some() {
            Some("URL opens may select an existing tab or another window; call refresh plus list_browser_tabs/window_view and attach if the selected tab is not in the target window".to_string())
        } else {
            None
        };

        LaunchAppResult {
            launched,
            app: app.to_string(),
            url: url.map(str::to_owned),
            target,
            target_window_title,
            matching_windows,
            verification_hint,
        }
    }

    /// Open a URL and immediately retarget Dunst to the best matching window.
    /// This is a conservative helper for browser automation: it does not claim
    /// success unless the selected tab/window after attach plausibly matches the
    /// requested URL.
    #[cfg(target_os = "macos")]
    pub fn open_url_and_attach_tab(
        &mut self,
        app: &str,
        url: &str,
        extra_args: &[String],
    ) -> OpenUrlAttachResult {
        let launch = self.launch_app(app, Some(url), extra_args);
        std::thread::sleep(Duration::from_millis(650));
        let candidates = launch.matching_windows.clone();
        let terms = url_match_terms(url);
        let selected = best_window_for_url(&candidates, &terms).or_else(|| {
            if candidates.len() == 1 {
                candidates.first().cloned()
            } else {
                candidates
                    .iter()
                    .find(|window| window.window_id == self.target.window_id)
                    .cloned()
            }
        });

        let mut attached = None;
        let mut attached_window_title = None;
        let mut selected_tab = None;
        let mut verified = false;
        if let Some(window) = selected {
            if self.attach_window(window.window_id).is_ok() {
                let tabs = self.list_browser_tabs(None, true);
                selected_tab = tabs.into_iter().find(|tab| tab.selected);
                let title = self.scene_graph().window.title.clone();
                verified = window_or_tab_matches_terms(&title, selected_tab.as_ref(), &terms);
                attached_window_title = Some(title);
                attached = Some(TargetState {
                    pid: self.target.pid,
                    window_id: self.target.window_id,
                    app_name: self.window.app_name.clone(),
                });
            }
        }

        let verification_hint = if verified {
            None
        } else if attached.is_some() {
            Some("URL was opened and a browser window was attached, but the selected tab/window title did not verify against the URL; call list_browser_tabs/window_view before acting.".into())
        } else {
            Some("URL was opened but no matching browser window could be attached unambiguously; use list_windows and attach explicitly.".into())
        };

        OpenUrlAttachResult {
            launch,
            attached,
            attached_window_title,
            selected_tab,
            candidates,
            verified,
            verification_hint,
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open_url_and_attach_tab(
        &mut self,
        app: &str,
        url: &str,
        extra_args: &[String],
    ) -> OpenUrlAttachResult {
        let launch = self.launch_app(app, Some(url), extra_args);
        OpenUrlAttachResult {
            candidates: launch.matching_windows.clone(),
            launch,
            attached: None,
            attached_window_title: None,
            selected_tab: None,
            verified: false,
            verification_hint: Some("open_url_and_attach_tab requires a macOS backend".into()),
        }
    }

    /// Quit an app gracefully (no foreground) by name.
    #[cfg(target_os = "macos")]
    pub fn close_app(&self, app: &str) -> bool {
        std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "quit application (item 1 of argv)",
                "-e",
                "end run",
                app,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn close_app(&self, _app: &str) -> bool {
        false
    }
}

fn url_match_terms(url: &str) -> Vec<String> {
    let normalized = normalize_match(url);
    let mut terms = Vec::new();
    if let Some(host) = normalized
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(str::trim)
        .filter(|host| !host.is_empty())
    {
        terms.push(host.trim_start_matches("www.").to_string());
    }
    for token in normalized
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 4)
        .take(8)
    {
        if !terms.iter().any(|term| term == token) {
            terms.push(token.to_string());
        }
    }
    terms
}

fn best_window_for_url(windows: &[WindowSummary], terms: &[String]) -> Option<WindowSummary> {
    windows
        .iter()
        .filter(|window| {
            let title = normalize_match(&window.title);
            terms.iter().any(|term| title.contains(term))
        })
        .min_by_key(|window| window.window_id)
        .cloned()
}

fn window_or_tab_matches_terms(
    window_title: &str,
    selected_tab: Option<&BrowserTab>,
    terms: &[String],
) -> bool {
    let window = normalize_match(window_title);
    let tab = selected_tab
        .map(|tab| normalize_match(&tab.title))
        .unwrap_or_default();
    terms
        .iter()
        .any(|term| window.contains(term) || tab.contains(term))
}
