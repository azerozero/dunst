use super::*;

impl Engine {
    /// Open a menu-bar menu by name (e.g. "File"/"Fichier") — finds the menubar
    /// item and presses it (AX). Native menus; the items then appear in the graph.
    pub fn open_menu(&mut self, name: &str) -> dunst_core::Result<AuditEntry> {
        let id = self
            .scene_graph()
            .nodes
            .values()
            .find(|node| {
                node.ax_role.contains("Menu")
                    && node
                        .label
                        .as_deref()
                        .is_some_and(|label| label.eq_ignore_ascii_case(name.trim()))
            })
            .map(|node| node.id.clone());
        match id {
            Some(id) => self.click_element(&id, Some(&format!("open menu {name}"))),
            None => Err(DunstError::Execution(format!(
                "no menu {name:?} found in the menubar"
            ))),
        }
    }

    /// Press a named key (e.g. `"Return"`/`"Enter"` to submit a typed URL).
    /// Raw keyboard input is high-risk because it is not tied to a scene element.
    #[cfg(target_os = "macos")]
    pub fn press_key(&mut self, key: &str, repeat: usize) -> dunst_core::Result<AuditEntry> {
        if !is_press_key_name(key) {
            return Err(DunstError::Execution(format!(
                "unsupported key {key:?}; expected return|enter, tab, escape, space, delete, up/down/left/right, pageup/pagedown, home/end"
            )));
        }
        let repeat = repeat.clamp(1, 20);
        let argument = if repeat == 1 {
            key.to_string()
        } else {
            format!("{key} x{repeat}")
        };
        let target_id = raw_press_key_target_id(key, repeat);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::KeyPress,
            Some(argument.clone()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..repeat {
            outcome = retry_user_active_guard(|| {
                dunst_platform::press_key(self.target.pid, self.target.window_id, key)
            });
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(180));
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::KeyPress,
            Some(argument),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn press_key(&mut self, _key: &str, _repeat: usize) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "press_key requires a macOS backend".into(),
        ))
    }

    /// Type `text` into the focused element via the **SkyLight auth-signed**
    /// keyboard path, so it reaches a backgrounded/occluded window's web content
    /// (trusted, no cursor, no foreground). First focus the field (e.g. click_at
    /// it). Raw keyboard input is high-risk because it is not tied to a scene
    /// element.
    #[cfg(target_os = "macos")]
    pub fn type_keys(&mut self, text: &str) -> dunst_core::Result<AuditEntry> {
        let target_id = raw_type_keys_target_id(text);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw keyboard text into focused element"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::type_text_background(self.target.pid, self.target.window_id, text)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw keyboard text into focused element (background web via SkyLight auth)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Paste `text` into the focused element by temporarily replacing the
    /// clipboard, sending Cmd+V to the target window, then restoring the
    /// previous plain-text clipboard contents. This keeps the platform clipboard
    /// mutation on the guarded raw-input path.
    pub fn paste_text(
        &mut self,
        text: &str,
        restore_clipboard: bool,
    ) -> dunst_core::Result<AuditEntry> {
        let target_id = raw_paste_text_target_id(text);
        let risk = Self::raw_input_risk(vec![
            "temporarily writes system clipboard before sending Cmd+V to the focused target".into(),
            "clipboard restore preserves plain text only; rich clipboard formats may not survive"
                .into(),
        ]);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw clipboard paste into focused element"),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let outcome = paste_text_via_clipboard(
            self.target.pid,
            self.target.window_id,
            text,
            restore_clipboard,
        );
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(text.to_string()),
            Some("raw clipboard paste into focused element (clipboard restore attempted)"),
            risk,
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn type_keys(&mut self, _text: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "type_keys requires a macOS backend".into(),
        ))
    }

    /// Scroll the FOCUSED page in the background via auth-signed Page/Home/End keys
    /// (reaches web content, no cursor, no foreground). `direction` =
    /// up|down|top|bottom; `pages` = how many Page presses (down/up). Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn scroll(
        &mut self,
        direction: &str,
        pages: usize,
        focus_id: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let direction = normalized_scroll_direction(direction);
        if let Some(id) = focus_id {
            if let Some(page_direction) = page_scroll_target_direction(id) {
                let direction = page_direction.unwrap_or(direction);
                if self.remembered_scroll_strategy().is_some() && matches!(direction, "down" | "up")
                {
                    let (x, y) = self
                        .remembered_scroll_point()
                        .unwrap_or_else(|| self.page_scroll_fallback_point());
                    return self.scroll_at(x, y, direction, pages, false);
                }
                return self.scroll_with_background_keys(direction, pages);
            }
            let can_scroll = self
                .affordance_graph()
                .affordances
                .get(id)
                .map(|affordance| affordance.actions.contains(&SemanticAction::Scroll))
                .unwrap_or(false);
            if !can_scroll {
                return Err(DunstError::ActionUnavailable {
                    id: id.to_string(),
                    action: format!("{:?}", SemanticAction::Scroll),
                });
            }
            return self.act(
                id,
                SemanticAction::Scroll,
                Some(&format!("{direction}:{pages}")),
                Some("direct AX scrollbar scroll"),
                None,
            );
        }

        if self.remembered_scroll_strategy().is_some() && matches!(direction, "down" | "up") {
            let (x, y) = self
                .remembered_scroll_point()
                .unwrap_or_else(|| self.page_scroll_fallback_point());
            return self.scroll_at(x, y, direction, pages, false);
        }
        self.scroll_with_background_keys(direction, pages)
    }

    #[cfg(target_os = "macos")]
    fn scroll_with_background_keys(
        &mut self,
        direction: &str,
        pages: usize,
    ) -> dunst_core::Result<AuditEntry> {
        // macOS virtual keycodes: PageDown=0x79, PageUp=0x74, Home=0x73, End=0x77.
        let (keycode, count) = match direction {
            "up" => (0x74_u16, pages.clamp(1, 20)),
            "top" => (0x73, 1),
            "bottom" => (0x77, 1),
            _ => (0x79, pages.clamp(1, 20)), // down (default)
        };
        let scope = self.scroll_strategy_key();
        let target_id = format!("keyboard@scroll:{direction}:{count}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Scroll,
            Some(format!("scroll {direction} x{count}")),
            Some("background web scroll"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..count {
            outcome = retry_user_active_guard(|| {
                dunst_platform::key_web_background(
                    self.target.pid,
                    self.target.window_id,
                    keycode,
                    0,
                )
            });
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(380));
        }
        let result = self.audit_raw_input(
            target_id,
            SemanticAction::Scroll,
            Some(format!("scroll {direction} x{count}")),
            Some("background web scroll (Page/Home/End keys, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        );
        if let Ok(entry) = &result {
            self.note_background_scroll_result(scope, entry);
        }
        result
    }

    /// Wheel-scroll at a concrete screen point in the target window. This is the
    /// fallback for web pages/cards that do not expose an AX scrollbar: the point
    /// chooses the scroll container, while the raw input gate still requires
    /// operator approval before mutating the page.
    #[cfg(target_os = "macos")]
    pub fn scroll_at(
        &mut self,
        x: f64,
        y: f64,
        direction: &str,
        pages: usize,
        mut borrow_cursor: bool,
    ) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, "scroll_at")?;
        let direction = normalized_scroll_direction(direction);
        let count = pages.clamp(1, 20);
        if matches!(direction, "top" | "bottom") {
            return self.scroll_with_background_keys(direction, count);
        }
        let scope = self.scroll_strategy_key();
        let remembered_strategy = !borrow_cursor
            && self
                .scroll_strategy_cache
                .get(&scope)
                .is_some_and(|memory| memory.strategy == ScrollStrategy::RealCursorWheel);
        if remembered_strategy {
            borrow_cursor = true;
        }
        if borrow_cursor {
            match self.ensure_real_cursor_scroll_point_visible(x, y) {
                Ok(()) => {}
                Err(_) if remembered_strategy => {
                    self.scroll_strategy_cache.remove(&scope);
                    borrow_cursor = false;
                }
                Err(err) => return Err(err),
            }
        }
        let target_id = if borrow_cursor {
            cursor_scroll_target_id(direction, count, x, y)
        } else {
            wheel_scroll_target_id(direction, count, x, y)
        };
        let argument = if borrow_cursor {
            format!("real cursor wheel scroll {direction} x{count} at {x:.1},{y:.1}")
        } else {
            format!("wheel scroll {direction} x{count} at {x:.1},{y:.1}")
        };
        let reasoning = if remembered_strategy && borrow_cursor {
            "session-learned real cursor wheel scroll at target point"
        } else if borrow_cursor {
            "real cursor wheel scroll at target point"
        } else {
            "background wheel scroll at target point"
        };
        let mut risk = self.raw_point_risk(x, y);
        if borrow_cursor {
            risk.reasons.extend([
                "briefly moves and restores the real OS cursor".to_string(),
                "the scroll is delivered to the visible surface under the cursor".to_string(),
            ]);
        }
        if remembered_strategy && borrow_cursor {
            risk.reasons.push(
                "session cache selected the previously validated real-cursor scroll fallback for this app/page"
                    .to_string(),
            );
        }
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Scroll,
            Some(argument.clone()),
            Some(reasoning),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let delta = match direction {
            "up" => 720,
            _ => -720,
        };
        let mut outcome = Ok(());
        for _ in 0..count {
            outcome = if borrow_cursor {
                retry_user_active_guard(|| dunst_platform::scroll_at_point(x, y, delta))
            } else {
                let (ox, oy) = dunst_vision::capture::window_bounds(self.target.window_id)
                    .map(|(x, y, _, _)| (x, y))
                    .unwrap_or((
                        self.current_window_bounds().x,
                        self.current_window_bounds().y,
                    ));
                retry_user_active_guard(|| {
                    dunst_platform::scroll_web_background(
                        self.target.pid,
                        self.target.window_id,
                        x,
                        y,
                        ox,
                        oy,
                        delta,
                    )
                })
            };
            if outcome.is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(160));
        }
        let result = self.audit_raw_input(
            target_id,
            SemanticAction::Scroll,
            Some(argument),
            Some(reasoning),
            risk,
            outcome,
        );
        match &result {
            Ok(entry) if borrow_cursor => {
                self.note_real_cursor_scroll_result_at(scope, entry, Some((x, y)))
            }
            Ok(entry) => self.note_background_scroll_result(scope, entry),
            Err(_) if remembered_strategy => {
                self.scroll_strategy_cache.remove(&scope);
            }
            Err(_) => {}
        }
        result
    }

    pub(in crate::engine) fn remembered_scroll_strategy(&self) -> Option<ScrollStrategy> {
        let scope = self.scroll_strategy_key();
        self.scroll_strategy_cache
            .get(&scope)
            .map(|memory| memory.strategy)
    }

    pub(in crate::engine) fn note_background_scroll_result(
        &mut self,
        scope: ScrollStrategyKey,
        entry: &AuditEntry,
    ) {
        if scroll_result_low_signal(entry) {
            self.scroll_background_low_signal.insert(scope);
        }
    }

    #[cfg(test)]
    pub(in crate::engine) fn note_real_cursor_scroll_result(
        &mut self,
        scope: ScrollStrategyKey,
        entry: &AuditEntry,
    ) {
        self.note_real_cursor_scroll_result_at(scope, entry, None);
    }

    fn note_real_cursor_scroll_result_at(
        &mut self,
        scope: ScrollStrategyKey,
        entry: &AuditEntry,
        point: Option<(f64, f64)>,
    ) {
        if entry.result != dunst_core::ActionResult::Success {
            return;
        }
        if self.scroll_background_low_signal.remove(&scope)
            || self
                .scroll_strategy_cache
                .get(&scope)
                .is_some_and(|memory| memory.strategy == ScrollStrategy::RealCursorWheel)
        {
            let existing_ratio = self
                .scroll_strategy_cache
                .get(&scope)
                .and_then(|memory| memory.point_ratio);
            let point_ratio = point
                .and_then(|(x, y)| self.scroll_point_ratio(x, y))
                .or(existing_ratio);
            self.scroll_strategy_cache.insert(
                scope,
                ScrollStrategyMemory {
                    strategy: ScrollStrategy::RealCursorWheel,
                    point_ratio,
                },
            );
        }
    }

    fn remembered_scroll_point(&self) -> Option<(f64, f64)> {
        let scope = self.scroll_strategy_key();
        let memory = self.scroll_strategy_cache.get(&scope)?;
        if memory.strategy != ScrollStrategy::RealCursorWheel {
            return None;
        }
        let (rx, ry) = memory.point_ratio?;
        let window = self.current_window_bounds();
        Some((window.x + window.w * rx, window.y + window.h * ry))
    }

    fn page_scroll_fallback_point(&self) -> (f64, f64) {
        let window = self.current_window_bounds();
        (window.x + window.w * 0.42, window.y + window.h * 0.54)
    }

    fn scroll_point_ratio(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let window = self.current_window_bounds();
        if window.w <= 0.0 || window.h <= 0.0 {
            return None;
        }
        Some((
            ((x - window.x) / window.w).clamp(0.0, 1.0),
            ((y - window.y) / window.h).clamp(0.0, 1.0),
        ))
    }

    pub(in crate::engine) fn scroll_strategy_key(&self) -> ScrollStrategyKey {
        let graph = self.scene_graph();
        let app = scroll_scope_token(&graph.window.app_name)
            .unwrap_or_else(|| format!("pid:{}", graph.window.pid));
        let page = self
            .current_browser_host()
            .map(|host| format!("host:{host}"))
            .or_else(|| title_site_scope(&graph.window.title, &graph.window.app_name))
            .unwrap_or_else(|| format!("window:{}", graph.window.window_id));
        ScrollStrategyKey { app, page }
    }

    fn current_browser_host(&self) -> Option<String> {
        self.list_browser_tabs(None, true)
            .into_iter()
            .find(|tab| tab.selected)
            .and_then(|tab| tab.url)
            .and_then(|url| host_from_url(&url))
            .or_else(|| {
                self.scene_graph().nodes.values().find_map(|node| {
                    node.value
                        .as_deref()
                        .or(node.label.as_deref())
                        .and_then(likely_url)
                        .and_then(|url| host_from_url(&url))
                })
            })
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scroll(
        &mut self,
        _direction: &str,
        _pages: usize,
        _focus_id: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "scroll requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scroll_at(
        &mut self,
        _x: f64,
        _y: f64,
        _direction: &str,
        _pages: usize,
        _borrow_cursor: bool,
    ) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "scroll_at requires a macOS backend".into(),
        ))
    }

    /// Zoom the focused page (browser/native) in the background: `in`/`out`/`reset`
    /// → Cmd+= / Cmd+- / Cmd+0, auth-signed (reaches web). Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn zoom(&mut self, direction: &str) -> dunst_core::Result<AuditEntry> {
        const CMD: u64 = 0x0010_0000;
        // keycodes: '=' 0x18, '-' 0x1B, '0' 0x1D.
        let keycode = match direction {
            "out" => 0x1B_u16,
            "reset" => 0x1D,
            _ => 0x18, // in (default)
        };
        let target_id = format!("keyboard@zoom:{direction}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Hotkey,
            Some(format!("zoom {direction}")),
            Some("background zoom"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::key_web_background(self.target.pid, self.target.window_id, keycode, CMD)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Hotkey,
            Some(format!("zoom {direction}")),
            Some("background zoom (Cmd =/-/0, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn zoom(&mut self, _direction: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "zoom requires a macOS backend".into(),
        ))
    }

    /// A keyboard shortcut in the background: modifiers (cmd|shift|opt|ctrl, `+`-
    /// separated) plus a key (a single character, or a name like enter/tab/escape/
    /// space/delete/left/right/up/down). E.g. "cmd+l" (focus omnibox), "cmd+t",
    /// "cmd+w". Auth-signed so it reaches web content. Layout-sensitive text
    /// selection shortcuts such as "cmd+a" are rejected; use `type_into` for
    /// field replacement. Re-perceives.
    #[cfg(target_os = "macos")]
    pub fn hotkey(&mut self, combo: &str) -> dunst_core::Result<AuditEntry> {
        if let Some(message) = layout_sensitive_hotkey_message(combo) {
            return Err(DunstError::Execution(message));
        }
        let (flags, keycode) = parse_combo(combo)
            .ok_or_else(|| DunstError::Execution(format!("unrecognised hotkey {combo:?}")))?;
        let target_id = format!("keyboard@hotkey:{combo}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Hotkey,
            Some(combo.to_string()),
            Some("background hotkey"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::key_web_background(
                self.target.pid,
                self.target.window_id,
                keycode,
                flags,
            )
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Hotkey,
            Some(combo.to_string()),
            Some("background hotkey (modifier combo, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn hotkey(&mut self, _combo: &str) -> dunst_core::Result<AuditEntry> {
        Err(DunstError::Execution(
            "hotkey requires a macOS backend".into(),
        ))
    }
}

fn paste_text_via_clipboard(
    pid: i32,
    window_id: u32,
    text: &str,
    restore_clipboard: bool,
) -> dunst_core::Result<()> {
    retry_user_active_guard(|| {
        dunst_platform::paste_text_background(pid, window_id, text, restore_clipboard)
    })
}

pub(in crate::engine) fn page_scroll_target_id(direction: &str) -> String {
    format!("page@scroll:{}", normalized_scroll_direction(direction))
}

pub(super) fn page_scroll_target_direction(id: &str) -> Option<Option<&str>> {
    if id == "page@scroll" {
        return Some(None);
    }
    id.strip_prefix("page@scroll:")
        .map(|direction| Some(normalized_scroll_direction(direction)))
}

fn normalized_scroll_direction(direction: &str) -> &'static str {
    match direction.trim().to_ascii_lowercase().as_str() {
        "up" => "up",
        "top" => "top",
        "bottom" => "bottom",
        _ => "down",
    }
}

fn wheel_scroll_target_id(direction: &str, count: usize, x: f64, y: f64) -> String {
    format!(
        "wheel@scroll:{}:{}:{:.0},{:.0}",
        normalized_scroll_direction(direction),
        count.clamp(1, 20),
        x,
        y
    )
}

fn cursor_scroll_target_id(direction: &str, count: usize, x: f64, y: f64) -> String {
    format!(
        "cursor@scroll:{}:{}:{:.0},{:.0}",
        normalized_scroll_direction(direction),
        count.clamp(1, 20),
        x,
        y
    )
}

fn scroll_result_low_signal(entry: &AuditEntry) -> bool {
    entry.result == dunst_core::ActionResult::Success
        && (entry.graph_diff.changes.is_empty()
            || entry
                .graph_diff
                .changes
                .iter()
                .all(scroll_low_signal_change))
}

fn scroll_low_signal_change(change: &dunst_core::NodeChange) -> bool {
    match change {
        dunst_core::NodeChange::Added { id, .. } | dunst_core::NodeChange::Removed { id, .. } => {
            low_signal_menu_id(id)
        }
        dunst_core::NodeChange::Changed { id, field, .. } => {
            low_signal_menu_id(id)
                && matches!(field.as_str(), "children" | "enabled" | "parent" | "label")
        }
    }
}

fn low_signal_menu_id(id: &str) -> bool {
    id.starts_with("mi_") || id.starts_with("menu_")
}

fn host_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host_port = after_scheme
        .rsplit('@')
        .next()
        .unwrap_or(after_scheme)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = host_port
        .split(':')
        .next()
        .unwrap_or(host_port)
        .trim()
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    (!host.is_empty() && host.contains('.')).then_some(host)
}

fn title_site_scope(title: &str, app: &str) -> Option<String> {
    for separator in [" | ", " — ", " – "] {
        if let Some((_, candidate)) = title.rsplit_once(separator) {
            if let Some(token) = scroll_scope_token(candidate) {
                if !generic_title_scope(&token, app) {
                    return Some(format!("title:{token}"));
                }
            }
        }
    }
    if let Some((left, _)) = title.split_once(" - ") {
        if let Some(token) = scroll_scope_token(left) {
            if !generic_title_scope(&token, app) {
                return Some(format!("title:{token}"));
            }
        }
    }
    scroll_scope_token(title)
        .filter(|token| !generic_title_scope(token, app))
        .map(|token| format!("title:{token}"))
}

fn generic_title_scope(token: &str, app: &str) -> bool {
    let app = scroll_scope_token(app).unwrap_or_default();
    token.is_empty()
        || token == app
        || matches!(
            token,
            "mozilla-firefox" | "navigation-privee" | "private-browsing"
        )
}

fn scroll_scope_token(value: &str) -> Option<String> {
    let normalized = normalize_match(value);
    let mut token = String::with_capacity(normalized.len().min(80));
    let mut previous_dash = false;
    for ch in normalized.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' {
            token.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            token.push('-');
            previous_dash = true;
        }
        if token.len() >= 80 {
            break;
        }
    }
    let token = token.trim_matches('-').to_string();
    (!token.is_empty()).then_some(token)
}
