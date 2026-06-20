use super::*;

#[cfg(target_os = "macos")]
pub(super) fn display_summary(display: dunst_vision::capture::DisplayInfo) -> DisplaySummary {
    DisplaySummary {
        index: display.index,
        display_id: display.display_id,
        bounds: Bbox {
            x: display.x,
            y: display.y,
            w: display.w,
            h: display.h,
        },
        pixels: PixelSize {
            width: display.pixels_wide,
            height: display.pixels_high,
        },
        scale: display.scale,
        is_main: display.is_main,
    }
}

pub(super) fn target_frame_for_display(
    current: Bbox,
    display: &Bbox,
    preserve_size: bool,
    cascade_offset: usize,
) -> (f64, f64, f64, f64) {
    let padding = 24.0;
    let max_w = (display.w - padding * 2.0).max(1.0);
    let max_h = (display.h - padding * 2.0).max(1.0);
    let (w, h) = if preserve_size {
        (current.w.min(max_w).max(1.0), current.h.min(max_h).max(1.0))
    } else {
        (max_w, max_h)
    };
    let offset = (cascade_offset as f64 * 28.0).min(140.0);
    let max_x = display.x + display.w - w - padding;
    let max_y = display.y + display.h - h - padding;
    let x = (display.x + ((display.w - w) / 2.0).max(padding) + offset).min(max_x);
    let y = (display.y + ((display.h - h) / 2.0).max(padding) + offset).min(max_y);
    (x.max(display.x + padding), y.max(display.y + padding), w, h)
}

pub(super) fn desktop_view_from_windows(
    displays: Vec<DisplaySummary>,
    mut windows: Vec<DesktopWindow>,
    degraded_reason: Option<String>,
) -> DesktopView {
    windows.sort_by_key(|w| w.z_order);
    for (idx, window) in windows.iter_mut().enumerate() {
        window.z_order = idx;
        window.is_frontmost = false;
    }
    for idx in 0..windows.len() {
        let bounds = windows[idx].bounds;
        let mut covered_by = Vec::new();
        let mut covers = Vec::new();
        for other in &windows {
            if other.window_id == windows[idx].window_id {
                continue;
            }
            if rect_intersection_area(bounds, other.bounds) <= 0.0 {
                continue;
            }
            if other.z_order < windows[idx].z_order {
                covered_by.push(other.window_id);
            } else {
                covers.push(other.window_id);
            }
        }
        windows[idx].covered_by = covered_by;
        windows[idx].covers = covers;
        windows[idx].is_frontmost = idx == 0;
    }
    let frontmost = windows.first().cloned();
    let degraded = degraded_reason.is_some();
    DesktopView {
        degraded,
        reason: degraded_reason,
        displays,
        windows,
        frontmost,
    }
}

pub(super) fn layout_frames(
    count: usize,
    display: &Bbox,
    mode: &str,
) -> dunst_core::Result<Vec<Bbox>> {
    let mode = mode.to_ascii_lowercase();
    let padding = 24.0;
    let gap = 12.0;
    let usable = Bbox {
        x: display.x + padding,
        y: display.y + padding,
        w: (display.w - padding * 2.0).max(1.0),
        h: (display.h - padding * 2.0).max(1.0),
    };
    let frames = match mode.as_str() {
        "maximize" | "maximise" | "full" => vec![usable; count],
        "cascade" => (0..count)
            .map(|idx| {
                let (x, y, w, h) = target_frame_for_display(usable, display, false, idx);
                Bbox { x, y, w, h }
            })
            .collect(),
        "columns" | "side_by_side" | "side-by-side" => grid_frames(count, &usable, count, 1, gap),
        "rows" => grid_frames(count, &usable, 1, count, gap),
        "grid" => {
            let cols = (count as f64).sqrt().ceil() as usize;
            let rows = count.div_ceil(cols);
            grid_frames(count, &usable, cols, rows, gap)
        }
        other => {
            return Err(VisualOpsError::Execution(format!(
                "invalid arrange mode {other:?}; expected grid|columns|rows|cascade|maximize"
            )))
        }
    };
    Ok(frames)
}

pub(super) fn grid_frames(
    count: usize,
    area: &Bbox,
    cols: usize,
    rows: usize,
    gap: f64,
) -> Vec<Bbox> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let cell_w = ((area.w - gap * (cols.saturating_sub(1) as f64)) / cols as f64).max(1.0);
    let cell_h = ((area.h - gap * (rows.saturating_sub(1) as f64)) / rows as f64).max(1.0);
    (0..count)
        .map(|idx| {
            let col = idx % cols;
            let row = idx / cols;
            Bbox {
                x: area.x + col as f64 * (cell_w + gap),
                y: area.y + row as f64 * (cell_h + gap),
                w: cell_w,
                h: cell_h,
            }
        })
        .collect()
}

pub(super) fn rect_intersection_area(a: Bbox, b: Bbox) -> f64 {
    let ax2 = a.x + a.w;
    let ay2 = a.y + a.h;
    let bx2 = b.x + b.w;
    let by2 = b.y + b.h;
    let w = ax2.min(bx2) - a.x.max(b.x);
    let h = ay2.min(by2) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

pub(super) fn clipped_region_to_window(region: Bbox, window: Bbox) -> Option<Bbox> {
    let x0 = region.x.max(window.x);
    let y0 = region.y.max(window.y);
    let x1 = (region.x + region.w).min(window.x + window.w);
    let y1 = (region.y + region.h).min(window.y + window.h);
    (x1 > x0 && y1 > y0).then_some(Bbox {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

pub(super) fn visual_probe_key(region: Bbox, columns: usize, rows: usize) -> VisualProbeKey {
    VisualProbeKey {
        region: (
            region.x.round() as i64,
            region.y.round() as i64,
            region.w.round() as i64,
            region.h.round() as i64,
        ),
        columns,
        rows,
    }
}

pub(super) fn compare_signatures(
    previous: &[u8],
    current: &[u8],
    threshold: u8,
) -> (usize, u8, f64) {
    let len = previous.len().min(current.len());
    if len == 0 {
        return (0, 0, 0.0);
    }
    let mut changed = 0usize;
    let mut max_delta = 0u8;
    let mut sum = 0u64;
    for idx in 0..len {
        let delta = previous[idx].abs_diff(current[idx]);
        if delta > threshold {
            changed += 1;
        }
        max_delta = max_delta.max(delta);
        sum += u64::from(delta);
    }
    (changed, max_delta, sum as f64 / len as f64)
}

pub(super) fn region_ax_key(node: &dunst_core::RawAxNode) -> String {
    let bbox = node
        .frame
        .map(|b| {
            format!(
                "{:.0},{:.0},{:.0},{:.0}",
                b.x.round(),
                b.y.round(),
                b.w.round(),
                b.h.round()
            )
        })
        .unwrap_or_else(|| "no-bbox".into());
    format!(
        "{}|{}|{}|{}",
        node.ax_role,
        node.ax_identifier.as_deref().unwrap_or(""),
        node.label.as_deref().unwrap_or(""),
        bbox
    )
}

pub(super) fn region_ax_element(key: String, node: dunst_core::RawAxNode) -> RegionAxElement {
    RegionAxElement {
        key,
        ax_role: node.ax_role,
        label: node.label,
        value: node.value,
        ax_identifier: node.ax_identifier,
        ax_actions: node.ax_actions,
        bbox: node.frame,
        enabled: node.enabled,
        focused: node.focused,
        sample_count: 0,
    }
}
