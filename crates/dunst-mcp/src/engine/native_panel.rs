use super::*;

const PANEL_SERVICE: &str = "Open and Save Panel Service";

impl Engine {
    /// Suggest AX-resolvable dialogs, never changing the target. A remote panel
    /// can be CG-hidden while its host-process proxy is visible but AX-inaccessible.
    pub fn detect_native_panel(&self) -> Value {
        let windows = self.list_windows(true);
        let inspected: Vec<_> = windows
            .iter()
            .filter(|w| {
                (w.on_screen && w.pid == self.target.pid)
                    || (w.app == PANEL_SERVICE
                        && matching_host(&windows, self.target.pid, w).is_some())
            })
            .filter_map(|w| {
                dunst_platform::window_focus_state(w.pid, w.window_id)
                    .ok()
                    .map(|state| (w, state))
            })
            .collect();
        json!({
            "target": {"pid": self.target.pid, "window_id": self.target.window_id},
            "candidates": panel_candidates(self.target.pid, self.target.window_id, &windows, &inspected),
            "target_changed": false,
            "requires_explicit_attach": true,
            "note": "Candidates are AX-resolvable. A service panel may be CG-hidden while its matching host proxy is visible. Bounds are only a relationship hint, not proof of ownership: confirm the intended panel before explicit attach. No automatic switching; an empty result does not prove no modal exists."
        })
    }
}

fn matching_host<'a>(
    windows: &'a [WindowSummary],
    pid: i32,
    panel: &WindowSummary,
) -> Option<&'a WindowSummary> {
    windows.iter().find(|host| {
        host.pid == pid
            && host.on_screen
            && host.window_id != panel.window_id
            && same_bounds(host.bounds, panel.bounds)
    })
}

fn panel_candidates(
    target_pid: i32,
    target_window_id: u32,
    windows: &[WindowSummary],
    inspected: &[(&WindowSummary, dunst_platform::WindowFocusState)],
) -> Vec<Value> {
    inspected.iter().filter_map(|(w, state)| {
        if w.window_id == target_window_id { return None; }
        let same_app_dialog = w.pid == target_pid && w.on_screen && state.is_dialog;
        let host = (w.app == PANEL_SERVICE).then(|| matching_host(windows, target_pid, w)).flatten();
        if !same_app_dialog && host.is_none() { return None; }
        Some(json!({
            "window": w,
            "relationship": if same_app_dialog { "same_process_dialog" } else { "service_panel_matching_visible_host_bounds" },
            "matching_host_window_id": host.map(|h| h.window_id),
            "owner_verified": false,
            "ax_resolved": true,
            "app_focused": state.confirms_focus(w.window_id),
            "attach": {"window_id": w.window_id}
        }))
    }).collect()
}

fn same_bounds(a: Bbox, b: Bbox) -> bool {
    a.w > 0.0
        && a.h > 0.0
        && b.w > 0.0
        && b.h > 0.0
        && [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)]
            .iter()
            .all(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() <= 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u32, pid: i32, app: &str) -> WindowSummary {
        WindowSummary {
            window_id: id,
            pid,
            app: app.into(),
            title: "Enregistrer".into(),
            bounds: Bbox {
                x: 10.0,
                y: 20.0,
                w: 800.0,
                h: 400.0,
            },
            on_screen: true,
        }
    }

    #[test]
    fn remote_panel_uses_visible_cg_proxy_even_when_only_service_resolves_in_ax() {
        let mut host = window(2, 10, "Firefox");
        let mut service = window(3, 20, PANEL_SERVICE);
        service.on_screen = false;
        let state = dunst_platform::WindowFocusState {
            is_dialog: false,
            focused_window_id: Some(3),
        };
        let inspected = [(&service, state)];
        let candidates = panel_candidates(10, 1, &[host.clone(), service.clone()], &inspected);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["matching_host_window_id"], 2);
        assert_eq!(candidates[0]["app_focused"], true);
        assert_eq!(candidates[0]["owner_verified"], false);
        assert!(panel_candidates(10, 1, &[], &inspected).is_empty());
        host.on_screen = false;
        assert!(panel_candidates(10, 1, &[host.clone()], &inspected).is_empty());
        host.on_screen = true;
        host.bounds.x += 100.0;
        assert!(panel_candidates(10, 1, &[host], &inspected).is_empty());
    }

    #[test]
    fn discovery_keeps_ambiguity_and_excludes_unresolved_or_unrelated_panels() {
        let host = window(2, 10, "Firefox");
        let service = window(3, 20, PANEL_SERVICE);
        let other = window(4, 30, PANEL_SERVICE);
        let state = dunst_platform::WindowFocusState {
            is_dialog: true,
            focused_window_id: None,
        };
        let windows = [host.clone(), service.clone(), other.clone()];
        assert!(panel_candidates(10, 1, &windows, &[]).is_empty());
        let inspected = [(&host, state), (&service, state), (&other, state)];
        assert_eq!(panel_candidates(10, 1, &windows, &inspected).len(), 3);
        assert!(panel_candidates(99, 1, &windows, &inspected).is_empty());
        let unrelated = window(5, 40, "Other app");
        assert!(panel_candidates(10, 1, &windows, &[(&unrelated, state)]).is_empty());
    }
}
