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

    /// Launch an app **without bringing it to the foreground** when the platform
    /// backend supports it,
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
    pub fn launch_app(
        &self,
        app: &str,
        url: Option<&str>,
        extra_args: &[String],
    ) -> LaunchAppResult {
        let launched = dunst_platform::launch_app(app, url, extra_args);
        std::thread::sleep(Duration::from_millis(350));
        self.launch_app_result(app, url, launched)
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
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);

        // Prefer an already-open matching tab/window before calling `open`.
        // Repeated platform URL-open calls can create/select
        // tabs depending on browser preferences, which is the wrong primitive
        // for continuing inside an already-attached page.
        let existing_candidates = self.matching_windows_for_app(app);
        if let Some(selected) = best_window_for_url(&existing_candidates, &terms) {
            let launch = self.launch_app_result(app, Some(url), false);
            return self.attach_url_window_result(
                launch,
                existing_candidates,
                Some(selected),
                &terms,
                &host_labels,
            );
        }

        let launch = self.launch_app(app, Some(url), extra_args);
        std::thread::sleep(Duration::from_millis(650));
        let candidates = launch.matching_windows.clone();
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

        self.attach_url_window_result(launch, candidates, selected, &terms, &host_labels)
    }

    /// Navigate the attached browser to `url` and re-verify. Unlike
    /// `open_url_and_attach_tab`, this ALWAYS forces a fresh load (it never
    /// re-selects a stale existing tab/window that merely matches the URL terms —
    /// the failure mode that lands on the wrong tab) and targets the
    /// currently-attached app. This is the reliable way to drive a backgrounded
    /// browser to a new page: address-bar keystrokes are not an option in the
    /// background, where synthetic keys fall through to page content and arrive as
    /// in-page shortcuts (e.g. GitHub's `g i` → Issues) instead of the URL bar.
    #[cfg(target_os = "macos")]
    pub fn navigate(&mut self, url: &str) -> OpenUrlAttachResult {
        let app = self.window.app_name.clone();
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);
        let launch = self.launch_app(&app, Some(url), &[]);
        std::thread::sleep(Duration::from_millis(700));
        let candidates = launch.matching_windows.clone();
        let selected = best_window_for_url(&candidates, &terms).or_else(|| {
            candidates
                .iter()
                .find(|window| window.window_id == self.target.window_id)
                .cloned()
                .or_else(|| candidates.first().cloned())
        });
        self.attach_url_window_result(launch, candidates, selected, &terms, &host_labels)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn navigate(&mut self, url: &str) -> OpenUrlAttachResult {
        let app = self.window.app_name.clone();
        self.open_url_and_attach_tab(&app, url, &[])
    }

    fn matching_windows_for_app(&self, app: &str) -> Vec<WindowSummary> {
        let app_needle = normalize_match(app);
        self.list_windows(false)
            .into_iter()
            .filter(|window| normalize_match(&window.app).contains(&app_needle))
            .collect()
    }

    fn attach_url_window_result(
        &mut self,
        launch: LaunchAppResult,
        candidates: Vec<WindowSummary>,
        selected: Option<WindowSummary>,
        terms: &[String],
        host_labels: &[String],
    ) -> OpenUrlAttachResult {
        let mut attached = None;
        let mut attached_window_title = None;
        let mut selected_tab = None;
        let mut verified = false;
        let mut verified_by = None;
        if let Some(window) = selected {
            if self.attach_window(window.window_id).is_ok() {
                let tabs = self.list_browser_tabs(None, true);
                selected_tab = tabs.into_iter().find(|tab| tab.selected);
                let title = self.scene_graph().window.title.clone();
                let page_state = self.page_state(20);
                verified_by = verification_source_for_url(
                    &title,
                    selected_tab.as_ref(),
                    page_state.url.as_deref(),
                    terms,
                    host_labels,
                )
                .map(str::to_string);
                verified = verified_by.is_some();
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
        } else if attached.is_some() && launch.launched {
            Some("URL was opened and a browser window was attached, but the selected tab/title/page URL did not verify against the URL; call list_browser_tabs/window_view before acting, or read_text_detailed(content_only=false) when Firefox hides browser chrome from AX.".into())
        } else if attached.is_some() {
            Some("An existing browser window was attached without opening a new URL, but the selected tab/title/page URL did not verify against the URL; call list_browser_tabs/window_view before acting, or read_text_detailed(content_only=false) when Firefox hides browser chrome from AX.".into())
        } else if launch.launched {
            Some("URL was opened but no matching browser window could be attached unambiguously; use list_windows and attach explicitly.".into())
        } else {
            Some("No existing matching browser window could be attached unambiguously, and the URL was not opened; use list_windows and attach explicitly or call launch_app/open_url_and_attach_tab only when navigation is intended.".into())
        };

        OpenUrlAttachResult {
            launch,
            attached,
            attached_window_title,
            selected_tab,
            candidates,
            verified,
            verified_by,
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
            verified_by: None,
            verification_hint: Some("open_url_and_attach_tab requires a macOS backend".into()),
        }
    }

    /// Quit an app gracefully (no foreground) by name.
    pub fn close_app(&self, app: &str) -> bool {
        dunst_platform::close_app(app)
    }
}

fn url_match_terms(url: &str) -> Vec<String> {
    let decoded = percent_decode_lossy(url);
    let normalized = normalize_match(&decoded);
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
        .filter_map(|window| {
            let title = normalize_match(&window.title);
            let score = terms
                .iter()
                .filter(|term| title.contains(term.as_str()))
                .count();
            (score > 0).then_some((score, window.on_screen, window.window_id, window))
        })
        .max_by_key(|(score, on_screen, window_id, _)| {
            (*score, *on_screen, std::cmp::Reverse(*window_id))
        })
        .map(|(_, _, _, window)| window.clone())
}

fn percent_decode_lossy(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn verification_source_for_url(
    window_title: &str,
    selected_tab: Option<&BrowserTab>,
    page_url: Option<&str>,
    terms: &[String],
    host_labels: &[String],
) -> Option<&'static str> {
    // Strong signals: the actual page/tab URL contains a match term. A real URL
    // identifies the route, so any term (including the host) is a valid match.
    if normalized_contains_any(page_url.unwrap_or_default(), terms) {
        return Some("page_url");
    }
    if selected_tab
        .and_then(|tab| tab.url.as_deref())
        .is_some_and(|url| normalized_contains_any(url, terms))
    {
        return Some("selected_tab_url");
    }
    // Weak signals: titles. A site carries its brand/host in the title of every
    // route (e.g. "Collective" for app.collective.work/<anything>), so a title
    // only confirms navigation when it matches a term specific to the requested
    // path/query — never the host/brand alone. Otherwise a generic brand title
    // would falsely verify any sub-route the SPA never actually loaded.
    let title_matches_specific_term = |title: &str| {
        let title = normalize_match(title);
        terms
            .iter()
            .filter(|term| !is_generic_url_term(term, host_labels))
            .any(|term| title.contains(term.as_str()))
    };
    if selected_tab.is_some_and(|tab| title_matches_specific_term(&tab.title)) {
        return Some("selected_tab_title");
    }
    if title_matches_specific_term(window_title) {
        return Some("window_title");
    }
    None
}

/// Host labels of a URL (full host plus its dot-separated components), used to
/// decide which match terms are generic brand/host noise versus specific to the
/// requested path. E.g. `app.collective.work` -> `["app.collective.work",
/// "app", "collective", "work"]`.
fn url_host_labels(url: &str) -> Vec<String> {
    let decoded = percent_decode_lossy(url);
    let normalized = normalize_match(&decoded);
    let mut labels = Vec::new();
    if let Some(host) = normalized
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(str::trim)
        .filter(|host| !host.is_empty())
    {
        let host = host.trim_start_matches("www.");
        labels.push(host.to_string());
        for label in host.split('.').filter(|label| !label.is_empty()) {
            if !labels.iter().any(|existing| existing == label) {
                labels.push(label.to_string());
            }
        }
    }
    labels
}

/// A term is generic (cannot, on its own, confirm a title navigated) when it is
/// the URL scheme or one of the host labels.
fn is_generic_url_term(term: &str, host_labels: &[String]) -> bool {
    matches!(term, "http" | "https") || host_labels.iter().any(|label| label == term)
}

fn normalized_contains_any(value: &str, terms: &[String]) -> bool {
    let normalized = normalize_match(value);
    !normalized.is_empty() && terms.iter().any(|term| normalized.contains(term))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str, url: Option<&str>) -> BrowserTab {
        BrowserTab {
            id: "tab_1".into(),
            url: url.map(str::to_string),
            title: title.into(),
            selected: true,
            bbox: None,
        }
    }

    #[test]
    fn url_verification_accepts_page_url_when_firefox_title_is_generic() {
        let url = "https://www.linkedin.com/in/cl%C3%A9ment-liard/";
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);

        assert_eq!(
            verification_source_for_url("Mozilla Firefox", None, Some(url), &terms, &host_labels,),
            Some("page_url")
        );
    }

    #[test]
    fn url_match_terms_decode_percent_encoded_profile_slugs() {
        let terms = url_match_terms("https://www.linkedin.com/in/cl%C3%A9ment-liard/");

        assert!(terms.iter().any(|term| term == "clement"));
        assert!(terms.iter().any(|term| term == "liard"));
    }

    #[test]
    fn best_window_for_url_prefers_specific_existing_window() {
        let terms = url_match_terms("https://www.linkedin.com/in/cl%C3%A9ment-liard/");
        let windows = vec![
            window(1, "Feed | LinkedIn"),
            window(2, "Clément LIARD | LinkedIn"),
        ];

        assert_eq!(best_window_for_url(&windows, &terms).unwrap().window_id, 2);
    }

    #[test]
    fn url_verification_reports_the_signal_that_matched() {
        let url = "https://github.com/AlexsJones/llmfit";
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);

        assert_eq!(
            verification_source_for_url(
                "Mozilla Firefox",
                Some(&tab("GitHub - AlexsJones/llmfit", None)),
                None,
                &terms,
                &host_labels,
            ),
            Some("selected_tab_title")
        );
        assert_eq!(
            verification_source_for_url(
                "Mozilla Firefox",
                Some(&tab("New Tab", Some(url))),
                None,
                &terms,
                &host_labels,
            ),
            Some("selected_tab_url")
        );
        assert_eq!(
            verification_source_for_url(
                "GitHub - AlexsJones/llmfit",
                None,
                None,
                &terms,
                &host_labels,
            ),
            Some("window_title")
        );
    }

    #[test]
    fn url_verification_rejects_generic_browser_state() {
        let url = "https://www.linkedin.com/in/cl%C3%A9ment-liard/";
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);

        assert_eq!(
            verification_source_for_url(
                "Mozilla Firefox",
                Some(&tab("New Tab", None)),
                None,
                &terms,
                &host_labels,
            ),
            None
        );
    }

    #[test]
    fn url_verification_rejects_brand_only_title_for_spa_subroute() {
        // app.collective.work shows "Collective" in the title of every route, so
        // a generic brand title must NOT confirm a specific sub-route the SPA may
        // never have loaded (the ?tab=2 no-op navigation that read as verified).
        let url = "https://app.collective.work/collective/clement-liard/profile?tab=2";
        let terms = url_match_terms(url);
        let host_labels = url_host_labels(url);

        assert_eq!(
            verification_source_for_url(
                "Collective",
                Some(&tab("Collective", None)),
                None,
                &terms,
                &host_labels,
            ),
            None,
            "brand-only title must not verify a specific sub-route"
        );

        // A title carrying a path-specific term (the profile slug) does verify.
        assert_eq!(
            verification_source_for_url(
                "Collective",
                Some(&tab("Clément LIARD | Collective", None)),
                None,
                &terms,
                &host_labels,
            ),
            Some("selected_tab_title"),
            "path-specific title term should still verify"
        );
    }

    fn window(window_id: u32, title: &str) -> WindowSummary {
        WindowSummary {
            window_id,
            pid: 42,
            app: "Firefox".into(),
            title: title.into(),
            bounds: Bbox {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            },
            on_screen: true,
        }
    }
}
