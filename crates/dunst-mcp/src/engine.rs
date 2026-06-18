//! The Dunst engine — the runtime-agnostic service behind the MCP tools.
//!
//! Holds a [`Perceptor`] + [`ActionExecutor`] + [`RiskEngine`], maintains the
//! current/previous [`SceneGraph`] and [`AffordanceGraph`], enforces
//! risk-based approval gating, and records an [`AuditEntry`] per action.
//!
//! This struct is transport-independent: the MCP server (`serve`) and the CLI
//! `demo` both drive the same methods.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use dunst_core::{
    ActionExecutor, ActionResult, AffordanceGraph, AuditEntry, Bbox, GraphDiff, Perceptor,
    RiskAssessment, RiskLevel, Role, SceneGraph, SceneNode, SemanticAction, Target, VisualOpsError,
    WindowRef,
};
use dunst_graph::{audit, derive_affordances, scene, RiskEngine};
use serde_json::{json, Value};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const READ_REFRESH_TTL: Duration = Duration::from_millis(500);
const DISPLAY_CACHE_TTL: Duration = Duration::from_millis(1_000);
const OCR_CACHE_TTL: Duration = Duration::from_millis(250);
const SCREENSHOT_CACHE_TTL: Duration = Duration::from_millis(250);
const TYPE_VERIFY_SETTLE_TIMEOUT: Duration = Duration::from_millis(1_000);
const TYPE_VERIFY_POLL_INTERVAL: Duration = Duration::from_millis(80);

fn unique_png_path(prefix: &str) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{}_{}.png",
        std::process::id(),
        nanos,
        n
    ))
}

/// Projection requested for [`Engine::scene_graph_view`] (WP-J / J1). The MCP
/// server defaults to [`Compact`](SceneView::Compact) so a real client can take
/// the graph inline; [`Full`](SceneView::Full) is the unchanged escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneView {
    /// Per-node `{id, role, label, value?, bbox, enabled, focused, parent,
    /// n_children}` — the heavy/derivable AX fields are dropped (~5–10× lighter).
    Compact,
    /// Today's behaviour: the full [`SceneGraph`], every field.
    Full,
    /// No per-node list — `{n_nodes, roots, counts_by_role, n_actionable, window}`.
    Summary,
}

impl SceneView {
    /// Parse the MCP `view` argument; `None` for an unrecognised value.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
}

/// One OCR'd line returned by [`Engine::read_text`]: the recognised `text`, its
/// bounding box in **screen points** (mapped from Vision's normalised box via
/// `coords::vision_norm_to_screen_pt`), and Vision's `confidence` in `[0,1]`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextHit {
    pub text: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

/// One geometric primitive returned by [`Engine::read_shapes`]: its `kind`
/// (`"Rect"`/`"Bar"`/`"Circle"`/`"Line"`/`"Unknown"`), bounding box in **screen
/// points**, and a heuristic `confidence`. The CV layer for figures (charts,
/// custom-drawn UI) that neither AX nor OCR exposes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShapeHit {
    pub kind: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

/// One traversal point of [`Engine::scan_chart`]: the screen x it hovered, a
/// best-effort value/time pulled from the crosshair bubble, and the raw OCR.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChartSample {
    pub x: f64,
    pub value: Option<String>,
    pub time: Option<String>,
    pub raw: Vec<String>,
}

/// Result of [`Engine::scan_chart`]: whether a chart is **rendered** (vs a blank
/// plot), where it sits, and the value series sampled along it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanResult {
    pub present: bool,
    /// Whether SkyLight focus-without-raise activated the window (so a web canvas
    /// paints) before the scan.
    pub focused: bool,
    pub fill_ratio: f32,
    pub region: Option<Bbox>,
    pub samples: Vec<ChartSample>,
}

/// One top-level window, for [`Engine::list_windows`] — target discovery.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowSummary {
    pub window_id: u32,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub bounds: Bbox,
    pub on_screen: bool,
}

/// One running GUI app, for [`Engine::list_apps`] — coarser-than-windows
/// discovery (which app to `launch_app`/`attach`, and is it already running).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppSummary {
    pub app: String,
    pub pid: i32,
    /// number of top-level windows this app owns
    pub windows: usize,
    /// at least one of its windows is currently on-screen
    pub on_screen: bool,
}

/// One active display/monitor in global macOS screen coordinates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DisplaySummary {
    /// Dunst's 1-based display number: main display first, then by arrangement.
    pub index: usize,
    pub display_id: u32,
    pub bounds: Bbox,
    pub pixels: PixelSize,
    pub scale: f64,
    pub is_main: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PixelSize {
    pub width: u64,
    pub height: u64,
}

/// One installed `.app` bundle that can be launched without starting it first.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LaunchableApp {
    pub name: String,
    pub display_name: String,
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub path: String,
    pub executable: Option<String>,
    pub running: bool,
}

/// Lightweight page/window state for agents that need orientation without a
/// screenshot or full scene graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageState {
    pub target: TargetState,
    pub title: String,
    pub url: Option<String>,
    pub visible_text: Vec<String>,
    pub key_elements: Vec<KeyElement>,
}

/// One browser tab visible in the target window tab strip.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrowserTab {
    pub id: String,
    pub title: String,
    pub selected: bool,
    pub url: Option<String>,
    pub bbox: Option<Bbox>,
}

/// One AX text snippet returned without the full scene graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextSnippet {
    pub id: String,
    pub role: &'static str,
    pub text: String,
    pub visible: bool,
    pub bbox: Option<Bbox>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TargetState {
    pub pid: i32,
    pub window_id: u32,
    pub app_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyElement {
    pub id: String,
    pub role: &'static str,
    pub label: Option<String>,
    pub value: Option<String>,
    pub bbox: Option<Bbox>,
}

/// Where [`Engine::select_file`] should click before filling the native file
/// chooser. `None` means the chooser is already open.
#[derive(Debug, Clone)]
pub enum FileSelectTrigger {
    ElementId(String),
    Point { x: f64, y: f64 },
}

/// Result of resolving and pressing a popover/list/radio option by visible text.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OptionPickResult {
    pub query: String,
    pub matched_id: String,
    pub action_id: String,
    pub action_role: &'static str,
    pub action: SemanticAction,
    pub selected_before: Option<bool>,
    pub selected_after: Option<bool>,
    pub closed_after: Option<bool>,
    pub audit: AuditEntry,
}

/// A scoped “enter the window” view: enough geometry and orientation to act on
/// the target without returning the full AX scene graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowView {
    pub target: TargetState,
    pub title: String,
    pub url: Option<String>,
    pub window: Bbox,
    pub display: Option<DisplaySummary>,
    pub window_in_display: Option<Bbox>,
    pub visible_text: Vec<String>,
    pub key_elements: Vec<KeyElement>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MoveAppResult {
    pub app: String,
    pub display: DisplaySummary,
    pub moved: usize,
    pub windows: Vec<WindowSummary>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopView {
    pub degraded: bool,
    pub reason: Option<String>,
    pub displays: Vec<DisplaySummary>,
    pub windows: Vec<DesktopWindow>,
    pub frontmost: Option<DesktopWindow>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopWindow {
    pub window_id: u32,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub bounds: Bbox,
    pub on_screen: bool,
    /// `0` is frontmost among the returned top-level windows.
    pub z_order: usize,
    pub is_frontmost: bool,
    pub display: Option<DisplaySummary>,
    /// Windows in front of this one that geometrically overlap it.
    pub covered_by: Vec<u32>,
    /// Windows behind this one that it geometrically overlaps.
    pub covers: Vec<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArrangeResult {
    pub display: DisplaySummary,
    pub mode: String,
    pub moved: usize,
    pub windows: Vec<WindowSummary>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VisualChangeProbe {
    pub changed: bool,
    pub baseline: bool,
    pub refreshed: bool,
    pub region: Bbox,
    pub columns: usize,
    pub rows: usize,
    pub cells_total: usize,
    pub cells_changed: usize,
    pub threshold: u8,
    pub max_delta: u8,
    pub mean_delta: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegionAxAnalysis {
    pub region: Bbox,
    pub columns: usize,
    pub rows: usize,
    pub points_total: usize,
    pub hits: usize,
    pub unique_elements: Vec<RegionAxElement>,
    pub samples: Vec<RegionAxSample>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegionAxElement {
    pub key: String,
    pub ax_role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub ax_identifier: Option<String>,
    pub ax_actions: Vec<String>,
    pub bbox: Option<Bbox>,
    pub enabled: bool,
    pub focused: bool,
    pub sample_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegionAxSample {
    pub x: f64,
    pub y: f64,
    pub element_key: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
struct TimedCache<T> {
    captured_at: Instant,
    value: T,
}

impl<T: Clone> TimedCache<T> {
    fn fresh(&self, ttl: Duration) -> Option<T> {
        (self.captured_at.elapsed() <= ttl).then(|| self.value.clone())
    }
}

#[derive(Clone, Copy, PartialEq)]
struct OcrCacheKey {
    window_id: u32,
    region: Option<(i64, i64, i64, i64)>,
    accurate: bool,
}

#[derive(Clone)]
struct OcrCacheEntry {
    key: OcrCacheKey,
    hits: Vec<TextHit>,
}

fn ocr_cache_key(window_id: u32, region: Option<Bbox>, accurate: bool) -> OcrCacheKey {
    OcrCacheKey {
        window_id,
        region: region.map(|b| {
            (
                b.x.round() as i64,
                b.y.round() as i64,
                b.w.round() as i64,
                b.h.round() as i64,
            )
        }),
        accurate,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DesktopCacheKey {
    all: bool,
}

#[derive(Clone)]
struct DesktopCacheEntry {
    key: DesktopCacheKey,
    view: DesktopView,
}

#[derive(Clone, Copy, PartialEq)]
struct VisualProbeKey {
    region: (i64, i64, i64, i64),
    columns: usize,
    rows: usize,
}

#[derive(Clone)]
struct VisualProbeCacheEntry {
    key: VisualProbeKey,
    signature: Vec<u8>,
}

/// Parse a (possibly French-formatted) numeric label like `"8 220,00"` or
/// `"8161,84'"` into a value. Space = thousands, comma = decimal; trailing OCR
/// junk is dropped.
fn parse_value(s: &str) -> Option<f64> {
    let kept: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect();
    if kept.is_empty() {
        return None;
    }
    kept.replacen(',', ".", 1)
        .replace(',', "")
        .parse::<f64>()
        .ok()
}

/// Standard base64 of `data` (for returning a screenshot PNG as MCP image data).
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Parse a hotkey combo like `"cmd+l"` into `(modifier flags, keycode)`.
fn parse_combo(combo: &str) -> Option<(u64, u16)> {
    let mut flags = 0u64;
    let mut key = None;
    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => flags |= 0x0010_0000,
            "shift" => flags |= 0x0002_0000,
            "opt" | "option" | "alt" => flags |= 0x0008_0000,
            "ctrl" | "control" => flags |= 0x0004_0000,
            other => key = keycode_for(other),
        }
    }
    Some((flags, key?))
}

fn layout_sensitive_hotkey_message(combo: &str) -> Option<String> {
    let mut has_cmd = false;
    let mut has_non_cmd_modifier = false;
    let mut key = None;

    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => has_cmd = true,
            "shift" | "opt" | "option" | "alt" | "ctrl" | "control" => has_non_cmd_modifier = true,
            other => key = Some(other.to_string()),
        }
    }

    match (has_cmd, has_non_cmd_modifier, key.as_deref()) {
        (true, false, Some("a")) => Some(
            "hotkey \"cmd+a\" is keyboard-layout sensitive on macOS and can hit the wrong Command shortcut on non-US layouts; use type_into on a text element instead"
                .into(),
        ),
        _ => None,
    }
}

/// macOS virtual keycode for a key name or single character (US ANSI layout).
fn keycode_for(k: &str) -> Option<u16> {
    Some(match k {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "escape" | "esc" => 0x35,
        "space" => 0x31,
        "delete" | "backspace" => 0x33,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        "pagedown" => 0x79,
        "pageup" => 0x74,
        "home" => 0x73,
        "end" => 0x77,
        "plus" => 0x18,
        "minus" => 0x1B,
        s if s.chars().count() == 1 => char_keycode(s.chars().next()?)?,
        _ => return None,
    })
}

fn is_press_key_name(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "return"
            | "enter"
            | "tab"
            | "escape"
            | "esc"
            | "space"
            | "spacebar"
            | "delete"
            | "backspace"
            | "up"
            | "arrowup"
            | "up_arrow"
            | "down"
            | "arrowdown"
            | "down_arrow"
            | "left"
            | "arrowleft"
            | "left_arrow"
            | "right"
            | "arrowright"
            | "right_arrow"
            | "pageup"
            | "page_up"
            | "pagedown"
            | "page_down"
            | "home"
            | "end"
    )
}

/// macOS virtual keycode for a single character (US ANSI layout).
fn char_keycode(c: char) -> Option<u16> {
    Some(match c.to_ascii_lowercase() {
        'a' => 0x00,
        'b' => 0x0B,
        'c' => 0x08,
        'd' => 0x02,
        'e' => 0x0E,
        'f' => 0x03,
        'g' => 0x05,
        'h' => 0x04,
        'i' => 0x22,
        'j' => 0x26,
        'k' => 0x28,
        'l' => 0x25,
        'm' => 0x2E,
        'n' => 0x2D,
        'o' => 0x1F,
        'p' => 0x23,
        'q' => 0x0C,
        'r' => 0x0F,
        's' => 0x01,
        't' => 0x11,
        'u' => 0x20,
        'v' => 0x09,
        'w' => 0x0D,
        'x' => 0x07,
        'y' => 0x10,
        'z' => 0x06,
        '0' => 0x1D,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '5' => 0x17,
        '6' => 0x16,
        '7' => 0x1A,
        '8' => 0x1C,
        '9' => 0x19,
        '=' => 0x18,
        '-' => 0x1B,
        _ => return None,
    })
}

/// Heuristic: contains a clock time like `HH:MM`.
fn looks_like_clock(s: &str) -> bool {
    let b = s.as_bytes();
    (1..b.len().saturating_sub(2)).any(|i| {
        b[i] == b':'
            && b[i - 1].is_ascii_digit()
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
    })
}

/// Linear map from a screen-y (pixels, down-positive) to a chart value, fit from
/// two OCR'd Y-axis price labels.
struct YCalibration {
    y_ref: f64,
    v_ref: f64,
    slope: f64,
}

impl YCalibration {
    fn value_at(&self, screen_y: f64) -> f64 {
        self.v_ref + (screen_y - self.y_ref) * self.slope
    }
}

/// Y-axis price labels `(screen_y, value)` right of `min_cx`, filtered to the
/// **densest value cluster** — the gridlines cluster tightly (e.g. 8140..8220)
/// while header values and performance percentages are spread out and dropped.
fn yaxis_points(hits: &[TextHit], min_cx: f64) -> Vec<(f64, f64)> {
    let cands: Vec<(f64, f64)> = hits
        .iter()
        .filter_map(|h| {
            let cx = h.bbox.x + h.bbox.w / 2.0;
            if cx < min_cx {
                return None;
            }
            let v = parse_value(&h.text)?;
            (v >= 1.0).then_some((h.bbox.y + h.bbox.h / 2.0, v))
        })
        .collect();
    if cands.len() < 2 {
        return cands;
    }
    let center = cands.iter().map(|&(_, v)| v).max_by_key(|&v| {
        cands
            .iter()
            .filter(|&&(_, u)| (u - v).abs() <= v * 0.05)
            .count()
    });
    match center {
        Some(c) => cands
            .into_iter()
            .filter(|&(_, v)| (v - c).abs() <= c * 0.05)
            .collect(),
        None => cands,
    }
}

/// Build a Y-axis calibration from the gridline price labels, using the two with
/// the largest vertical separation.
fn build_y_calibration(hits: &[TextHit], region: &Bbox) -> Option<YCalibration> {
    let mut pts = yaxis_points(hits, region.x + region.w * 0.82);
    if pts.len() < 2 {
        return None;
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (y_a, v_a) = pts[0];
    let (y_b, v_b) = pts[pts.len() - 1];
    if (y_b - y_a).abs() < 1.0 {
        return None;
    }
    Some(YCalibration {
        y_ref: y_a,
        v_ref: v_a,
        slope: (v_b - v_a) / (y_b - y_a),
    })
}

/// Derive the plot rectangle from the OCR'd axis labels: x-range from the time
/// labels (`HH:MM`) along the bottom, y-range from the price labels down the
/// right-hand Y axis. Robust where a thin-curve / pale-fill chart defeats blob
/// detection.
fn region_from_axis(hits: &[TextHit]) -> Option<Bbox> {
    let time_xs: Vec<f64> = hits
        .iter()
        .filter(|h| looks_like_clock(&h.text))
        .map(|h| h.bbox.x + h.bbox.w / 2.0)
        .collect();
    if time_xs.len() < 3 {
        return None;
    }
    let x_min = time_xs.iter().copied().fold(f64::MAX, f64::min);
    let x_max = time_xs.iter().copied().fold(f64::MIN, f64::max);
    let price_pts = yaxis_points(hits, x_max - 40.0);
    if price_pts.len() < 2 {
        return None;
    }
    let y_top = price_pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let y_bot = price_pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    if x_max - x_min < 50.0 || y_bot - y_top < 30.0 {
        return None;
    }
    Some(Bbox {
        x: x_min,
        y: y_top,
        w: x_max - x_min,
        h: y_bot - y_top,
    })
}

/// The X-axis time/date label nearest `x`, restricted to the axis row at the
/// BOTTOM of `region` so a header timestamp (e.g. "à la clôture de 17:35") can't
/// masquerade as the axis. A clock OR a short token (a day-date like "09 Juin").
fn nearest_time_label(hits: &[TextHit], x: f64, region: &Bbox) -> Option<String> {
    // axis band: from just below mid-plot to a little under the plot bottom.
    let y_lo = region.y + region.h * 0.6;
    let y_hi = region.y + region.h + 60.0;
    hits.iter()
        .filter(|h| {
            let cy = h.bbox.y + h.bbox.h / 2.0;
            cy >= y_lo && cy <= y_hi && (looks_like_clock(&h.text) || is_axis_token(&h.text))
        })
        .min_by(|a, b| {
            let da = (a.bbox.x + a.bbox.w / 2.0 - x).abs();
            let db = (b.bbox.x + b.bbox.w / 2.0 - x).abs();
            da.total_cmp(&db)
        })
        .map(|h| h.text.trim().to_string())
}

/// A short axis tick token: a clock, a bare day-number, or a `<num> <month>` date.
fn is_axis_token(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.len() <= 12
        && t.chars().next().is_some_and(|c| c.is_ascii_digit())
        && t.chars().filter(char::is_ascii_digit).count() <= 4
}

#[cfg(target_os = "macos")]
fn display_summary(display: dunst_vision::capture::DisplayInfo) -> DisplaySummary {
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

fn target_frame_for_display(
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

fn desktop_view_from_windows(
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

fn layout_frames(count: usize, display: &Bbox, mode: &str) -> dunst_core::Result<Vec<Bbox>> {
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

fn grid_frames(count: usize, area: &Bbox, cols: usize, rows: usize, gap: f64) -> Vec<Bbox> {
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

fn rect_intersection_area(a: Bbox, b: Bbox) -> f64 {
    let ax2 = a.x + a.w;
    let ay2 = a.y + a.h;
    let bx2 = b.x + b.w;
    let by2 = b.y + b.h;
    let w = ax2.min(bx2) - a.x.max(b.x);
    let h = ay2.min(by2) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

fn clipped_region_to_window(region: Bbox, window: Bbox) -> Option<Bbox> {
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

fn visual_probe_key(region: Bbox, columns: usize, rows: usize) -> VisualProbeKey {
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

fn compare_signatures(previous: &[u8], current: &[u8], threshold: u8) -> (usize, u8, f64) {
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

fn region_ax_key(node: &dunst_core::RawAxNode) -> String {
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

fn region_ax_element(key: String, node: dunst_core::RawAxNode) -> RegionAxElement {
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

pub struct Engine {
    perceptor: Box<dyn Perceptor>,
    executor: Box<dyn ActionExecutor>,
    risk: RiskEngine,
    target: Target,
    window: WindowRef,
    current: Option<SceneGraph>,
    previous: Option<SceneGraph>,
    affordances: Option<AffordanceGraph>,
    /// Element IDs that have been explicitly approved for high-risk actions.
    approvals: BTreeSet<String>,
    /// IDs currently awaiting approval — the gated participants of the actions that
    /// returned `PendingApproval` since the last refresh. Lets [`approve`](Self::approve)
    /// accept an element whose danger is *contextual* (a destructive value typed into
    /// an otherwise low-risk field, audit #13), without loosening the rule that a
    /// plain low-risk id can't be approved.
    pending_gate_ids: BTreeSet<String>,
    /// Memoised at [`refresh`](Self::refresh) (audit #9): the window rect and the
    /// menubar-root id, so the per-listing latent filter doesn't re-scan every node
    /// on each call.
    cached_window_rect: Option<Bbox>,
    cached_menubar_root: Option<String>,
    last_refresh_at: Option<Instant>,
    display_cache: RefCell<Option<TimedCache<Vec<DisplaySummary>>>>,
    desktop_cache: RefCell<Option<TimedCache<DesktopCacheEntry>>>,
    ocr_cache: RefCell<Option<TimedCache<OcrCacheEntry>>>,
    screenshot_cache: RefCell<Option<TimedCache<String>>>,
    visual_probe_cache: RefCell<Option<VisualProbeCacheEntry>>,
    trace: Vec<AuditEntry>,
}

impl Engine {
    pub fn new(
        perceptor: Box<dyn Perceptor>,
        executor: Box<dyn ActionExecutor>,
        target: Target,
    ) -> dunst_core::Result<Self> {
        let window = perceptor.window_ref(&target)?;
        let mut e = Engine {
            perceptor,
            executor,
            risk: RiskEngine::new(),
            target,
            window,
            current: None,
            previous: None,
            affordances: None,
            approvals: BTreeSet::new(),
            pending_gate_ids: BTreeSet::new(),
            cached_window_rect: None,
            cached_menubar_root: None,
            last_refresh_at: None,
            display_cache: RefCell::new(None),
            desktop_cache: RefCell::new(None),
            ocr_cache: RefCell::new(None),
            screenshot_cache: RefCell::new(None),
            visual_probe_cache: RefCell::new(None),
            trace: Vec::new(),
        };
        e.refresh()?;
        Ok(e)
    }

    // --- read tools ---------------------------------------------------------

    /// Re-perceive the target and rebuild scene + affordance graphs. The prior
    /// graph is kept as `previous` for `diff_since`.
    pub fn refresh(&mut self) -> dunst_core::Result<()> {
        let roots = self.perceptor.capture(&self.target)?;
        let graph = scene::build_scene_graph(roots, self.window.clone(), dunst_core::now_ms());
        let aff = derive_affordances(&graph, &self.risk);
        self.previous = self.current.take();
        self.current = Some(graph);
        self.affordances = Some(aff);
        // Audit #9: compute the window rect + menubar root once per perception and
        // cache them, instead of re-scanning every node on each listing call.
        self.cached_window_rect = compute_window_rect(self.scene_graph());
        self.cached_menubar_root = compute_menubar_root(self.scene_graph());
        // Audit #2: a re-perception means the scene state the operator approved
        // may no longer hold (the dangerous element could have moved, changed
        // risk, or vanished). Drop every outstanding grant — and any pending gate —
        // so an approval can never silently survive a state change.
        self.approvals.clear();
        self.pending_gate_ids.clear();
        self.last_refresh_at = Some(Instant::now());
        *self.ocr_cache.borrow_mut() = None;
        *self.screenshot_cache.borrow_mut() = None;
        Ok(())
    }

    /// Re-perceive only if the current AX graph is older than the read-cache TTL.
    /// Mutating action paths call [`refresh`](Self::refresh) directly and bypass
    /// this throttle, so post-action state remains strongly fresh.
    pub fn refresh_if_stale(&mut self) -> dunst_core::Result<bool> {
        self.refresh_if_older_than(READ_REFRESH_TTL)
    }

    /// Re-perceive only if the current AX graph is older than `ttl`.
    ///
    /// Read-side callers use this to coalesce bursts of `force_refresh:true`
    /// requests without weakening explicit mutation paths, which still call
    /// [`refresh`](Self::refresh) after an action.
    pub fn refresh_if_older_than(&mut self, ttl: Duration) -> dunst_core::Result<bool> {
        if self.last_refresh_at.is_some_and(|at| at.elapsed() <= ttl) {
            return Ok(false);
        }
        self.refresh().map(|()| true)
    }

    /// Whether the current scene graph was captured within `ttl`.
    pub fn graph_recent(&self, ttl: Duration) -> bool {
        self.last_refresh_at.is_some_and(|at| at.elapsed() <= ttl)
    }

    /// Re-target the engine to a different window at runtime — the MCP client
    /// picks one from `list_windows` and attaches, so the server has no fixed,
    /// hardcoded target. Re-perceives the new window.
    pub fn attach(&mut self, pid: i32, window_id: u32) -> dunst_core::Result<()> {
        self.target = Target { pid, window_id };
        self.window = self.perceptor.window_ref(&self.target)?;
        self.refresh()
    }

    /// Attach by `window_id` alone, resolving the owning pid via `list_windows`.
    #[cfg(target_os = "macos")]
    pub fn attach_window(&mut self, window_id: u32) -> dunst_core::Result<()> {
        let pid = dunst_vision::capture::list_windows()
            .into_iter()
            .find(|w| w.window_id == window_id)
            .map(|w| w.pid)
            .ok_or_else(|| VisualOpsError::Perception(format!("window {window_id} not found")))?;
        // A stdio server may start on the device-free Notes fixture so it is
        // inspectable before a client chooses a real target. Once the client
        // attaches to a live WindowServer id, perception and actions must switch
        // to the macOS backend; otherwise the target tuple changes but the AX
        // graph still comes from the fixture.
        self.perceptor = Box::new(dunst_platform::MacosBackend::new());
        self.executor = Box::new(dunst_platform::MacosBackend::new());
        self.attach(pid, window_id)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn attach_window(&mut self, _window_id: u32) -> dunst_core::Result<()> {
        Err(VisualOpsError::Perception(
            "attach requires a macOS backend".into(),
        ))
    }

    /// The current target as `(pid, window_id)`.
    pub fn target(&self) -> (i32, u32) {
        (self.target.pid, self.target.window_id)
    }

    pub fn scene_graph(&self) -> &SceneGraph {
        self.current.as_ref().expect("refreshed in new()")
    }

    pub fn affordance_graph(&self) -> &AffordanceGraph {
        self.affordances.as_ref().expect("refreshed in new()")
    }

    /// Substring match (case-insensitive) over label / id / ax_role.
    ///
    /// Matches are ranked so visible, enabled targets come first, but latent
    /// nodes are still returned. That preserves the contract that find-by-query
    /// can reach collapsed/off-screen elements while making live browser noise
    /// (menu items, off-window chrome) less likely to be picked first.
    pub fn find_element(&self, query: &str) -> Vec<&SceneNode> {
        self.find_element_filtered(query, false)
    }

    /// As [`find_element`](Self::find_element), optionally dropping latent /
    /// off-window matches. The filtered form is useful for live web automation
    /// where browser chrome and history menu items can match the same text as
    /// the page target.
    pub fn find_element_filtered(&self, query: &str, visible_only: bool) -> Vec<&SceneNode> {
        let q = normalize_match(query);
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let graph = self.scene_graph();
        let mut matches: Vec<&SceneNode> = graph
            .nodes
            .values()
            .filter(|n| {
                normalized_contains_query(&normalize_match(&n.id), &q)
                    || n.label
                        .as_deref()
                        .map(|l| normalized_contains_query(&normalize_match(l), &q))
                        .unwrap_or(false)
                    || normalized_contains_query(&normalize_match(&n.ax_role), &q)
            })
            .filter(|n| !visible_only || node_visible_or_menu(n, window_rect, menubar))
            .collect();
        let mut seen: BTreeSet<String> = matches.iter().map(|node| node.id.clone()).collect();
        for label in matches.clone() {
            if let Some(control) = associated_control_for_label(label, graph, window_rect, menubar)
            {
                if seen.insert(control.id.clone()) {
                    matches.push(control);
                }
            }
        }
        matches.sort_by_key(|n| find_rank(n, window_rect, menubar));
        matches
    }

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
            .and_then(|c| c.fresh(OCR_CACHE_TTL))
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
            .map_err(|e| {
                VisualOpsError::Perception(format!(
                    "OCR requires a live macOS window (capture failed: {e})"
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
            .map(|b| TextHit {
                text: b.text,
                bbox: match region_screen_pt {
                    Some(region) => {
                        dunst_vision::coords::vision_norm_to_screen_pt_in_region(b.norm, region)
                    }
                    None => {
                        dunst_vision::coords::vision_norm_to_screen_pt(b.norm, &captured.geometry)
                    }
                },
                confidence: b.confidence,
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
        // GPU/WebGL-rendered windows — chart canvases are exactly what the CV shape
        // detector exists to read — so grab what is actually on screen instead.
        let captured = dunst_vision::capture::capture_window_composited(self.target.window_id)
            .map_err(|e| {
                VisualOpsError::Perception(format!(
                    "shape detection requires a live macOS window (capture failed: {e})"
                ))
            })?;
        Ok(
            dunst_vision::shapes::detect_shapes(&captured.image, &captured.geometry)
                .into_iter()
                .map(|s| ShapeHit {
                    kind: format!("{:?}", s.kind),
                    bbox: s.bbox,
                    confidence: s.confidence,
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

    /// IDs whose affordance offers `action`. WP-J/J2: latent (off-screen /
    /// zero-bbox) nodes — e.g. collapsed-menu items — are omitted by default so
    /// the agent isn't handed phantom targets. The gated action path is
    /// unaffected: it resolves ids against the graph, not this listing.
    ///
    /// Ergonomic default over [`query_affordances_filtered`](Self::query_affordances_filtered);
    /// the MCP server calls the latter directly, so in the binary this wrapper is
    /// exercised only by callers/tests that want the filtered listing.
    // `expect` is scoped to non-test builds: these fns ARE used by the test module,
    // so a bare `#[expect(dead_code)]` would be "unfulfilled" under the test target.
    // In the binary they are genuinely dead — and clippy will flag this expectation
    // the moment a non-test caller appears (the point of `expect` over `allow`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "ergonomic unfiltered wrapper, exercised only by tests"
        )
    )]
    pub fn query_affordances(&self, action: SemanticAction) -> Vec<String> {
        self.query_affordances_filtered(action, false)
    }

    /// As [`query_affordances`](Self::query_affordances), but `include_latent`
    /// returns every id exposing `action`, latent ones included.
    pub fn query_affordances_filtered(
        &self,
        action: SemanticAction,
        include_latent: bool,
    ) -> Vec<String> {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        self.affordance_graph()
            .affordances
            .values()
            .filter(|a| a.actions.contains(&action))
            .filter(|a| {
                include_latent
                    || g.get(&a.id)
                        .map(|n| node_visible_or_menu(n, window_rect, menubar))
                        .unwrap_or(false)
            })
            .map(|a| a.id.clone())
            .collect()
    }

    /// WP-J/J2: the affordance graph as JSON, latent nodes omitted unless
    /// `include_latent`. Shape matches [`AffordanceGraph`] (`{ "affordances": … }`).
    pub fn affordances_view(&self, include_latent: bool) -> Value {
        let ag = self.affordance_graph();
        if include_latent {
            return serde_json::to_value(ag).unwrap_or(Value::Null);
        }
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        let mut map = serde_json::Map::new();
        for (id, aff) in &ag.affordances {
            if g.get(id)
                .map(|n| node_visible_or_menu(n, window_rect, menubar))
                .unwrap_or(false)
            {
                map.insert(id.clone(), serde_json::to_value(aff).unwrap_or(Value::Null));
            }
        }
        json!({ "affordances": Value::Object(map) })
    }

    /// WP-J/J1: the scene graph under a projection `view`, optionally limited to
    /// actionable nodes. `Full` without `actionable_only` is byte-for-byte the
    /// old `get_scene_graph` payload (the escape hatch).
    pub fn scene_graph_view(&self, view: SceneView, actionable_only: bool) -> Value {
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let g = self.scene_graph();
        match view {
            SceneView::Full if !actionable_only => serde_json::to_value(g).unwrap_or(Value::Null),
            SceneView::Full => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if node_actionable(n, window_rect, menubar) {
                        map.insert(id.clone(), serde_json::to_value(n).unwrap_or(Value::Null));
                    }
                }
                json!({
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Compact => {
                let mut map = serde_json::Map::new();
                for (id, n) in &g.nodes {
                    if actionable_only && !node_actionable(n, window_rect, menubar) {
                        continue;
                    }
                    map.insert(id.clone(), compact_node(n));
                }
                json!({
                    "view": "compact",
                    "captured_at_ms": g.captured_at_ms,
                    "window": g.window,
                    "roots": g.roots,
                    "nodes": Value::Object(map),
                })
            }
            SceneView::Summary => {
                let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
                let mut n_actionable = 0usize;
                for n in g.nodes.values() {
                    *counts.entry(n.role.as_str()).or_insert(0) += 1;
                    if node_actionable(n, window_rect, menubar) {
                        n_actionable += 1;
                    }
                }
                json!({
                    "view": "summary",
                    "n_nodes": g.nodes.len(),
                    "roots": g.roots,
                    "counts_by_role": counts,
                    "n_actionable": n_actionable,
                    "window": g.window,
                })
            }
        }
    }

    /// Browser-tab projection from the current AX graph. Firefox/Chrome expose
    /// visible tab-strip tabs as AXRadioButton nodes near the top of the window;
    /// using this avoids confusing a page/sidebar item named "ClaudeAI" with a
    /// real browser tab.
    pub fn list_browser_tabs(&self, query: Option<&str>, visible_only: bool) -> Vec<BrowserTab> {
        let q = query.map(normalize_match);
        let window_rect = self.cached_window_rect;
        let mut tabs = Vec::new();

        for node in self.scene_graph().nodes.values() {
            if node.role != Role::Radio || node.ax_role != "AXRadioButton" {
                continue;
            }
            if !looks_like_browser_tab(node, window_rect) {
                continue;
            }
            if visible_only && !node_on_screen(node, window_rect) {
                continue;
            }

            let title = browser_tab_title(self.scene_graph(), node);
            if title.is_empty() {
                continue;
            }
            if let Some(q) = q.as_deref() {
                let haystack = format!("{} {}", normalize_match(&node.id), normalize_match(&title));
                if !normalized_contains_query(&haystack, q) {
                    continue;
                }
            }

            let selected = browser_tab_selected(self.scene_graph(), node, &title);
            tabs.push(BrowserTab {
                id: node.id.clone(),
                url: likely_url(&title),
                title,
                selected,
                bbox: node.bbox,
            });
        }

        tabs.sort_by(|a, b| {
            let ay = a.bbox.map(|b| b.y).unwrap_or(f64::MAX);
            let by = b.bbox.map(|b| b.y).unwrap_or(f64::MAX);
            ay.partial_cmp(&by)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ax = a.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    let bx = b.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        tabs
    }

    /// Lightweight orientation snapshot: window title, likely URL, visible text
    /// snippets and key visible action targets. Intended for "where am I?" checks
    /// without requesting a screenshot or full graph.
    pub fn page_state(&self, limit: usize) -> PageState {
        let limit = limit.clamp(1, 50);
        let g = self.scene_graph();
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let suppressed_repetitive_destructive = page_state_repetitive_destructive_keys(
            g,
            self.affordance_graph(),
            window_rect,
            menubar,
        );

        let mut visible_text = Vec::new();
        let mut key_elements = Vec::new();
        let mut url = None;

        for node in g.nodes.values() {
            if !node_visible_or_menu(node, window_rect, menubar) {
                continue;
            }
            let chrome = page_state_chrome_node(g, node, window_rect, menubar);

            if url.is_none() {
                url = node
                    .value
                    .as_deref()
                    .or(node.label.as_deref())
                    .and_then(likely_url);
            }

            if !chrome
                && matches!(
                    node.role,
                    Role::StaticText | Role::TextField | Role::TextArea
                )
            {
                if let Some(text) = node
                    .label
                    .as_deref()
                    .or(node.value.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    push_unique_string(&mut visible_text, text, limit);
                }
            }

            if key_elements.len() < limit
                && !chrome
                && !page_state_suppressed_repetitive_destructive(
                    node,
                    &suppressed_repetitive_destructive,
                )
                && page_state_key_element_candidate(node, window_rect, menubar)
                && node.enabled
                && self
                    .affordance_graph()
                    .affordances
                    .get(&node.id)
                    .map(|a| !a.actions.is_empty())
                    .unwrap_or(false)
            {
                key_elements.push(KeyElement {
                    id: node.id.clone(),
                    role: node.role.as_str(),
                    label: node.label.clone(),
                    value: node.value.clone(),
                    bbox: node.bbox,
                });
            }

            if visible_text.len() >= limit && key_elements.len() >= limit && url.is_some() {
                break;
            }
        }

        PageState {
            target: TargetState {
                pid: g.window.pid,
                window_id: g.window.window_id,
                app_name: g.window.app_name.clone(),
            },
            title: g.window.title.clone(),
            url,
            visible_text,
            key_elements,
        }
    }

    /// AX-only text extraction for LLM chats and document-like pages. This is
    /// lighter than `get_scene_graph full` and more reliable than OCR when the
    /// browser exposes response text through accessibility.
    pub fn text_snapshot(
        &self,
        query: Option<&str>,
        visible_only: bool,
        limit: usize,
    ) -> Vec<TextSnippet> {
        let limit = limit.clamp(1, 500);
        let q = query.map(normalize_match);
        let g = self.scene_graph();
        let window_rect = self.cached_window_rect;
        let menubar = self.cached_menubar_root.as_deref();
        let mut snippets = Vec::new();

        for node in g.nodes.values() {
            if !matches!(
                node.role,
                Role::StaticText | Role::TextField | Role::TextArea
            ) {
                continue;
            }

            let (primary, secondary) = match node.role {
                Role::TextField | Role::TextArea => (node.value.as_deref(), node.label.as_deref()),
                _ => (node.label.as_deref(), node.value.as_deref()),
            };
            let text = primary
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| secondary.map(str::trim).filter(|s| !s.is_empty()));
            let Some(text) = text else {
                continue;
            };

            let visible = node_visible_or_menu(node, window_rect, menubar);
            if visible_only && !visible {
                continue;
            }
            if read_chrome_node(g, node, window_rect, menubar) {
                continue;
            }

            if let Some(q) = q.as_deref() {
                let haystack = format!(
                    "{} {} {}",
                    normalize_match(&node.id),
                    node.role.as_str(),
                    normalize_match(text)
                );
                if !normalized_contains_query(&haystack, q) {
                    continue;
                }
            }

            snippets.push(TextSnippet {
                id: node.id.clone(),
                role: node.role.as_str(),
                text: text.to_string(),
                visible,
                bbox: node.bbox,
            });
        }

        snippets.sort_by(|a, b| {
            let avis = if a.visible { 0 } else { 1 };
            let bvis = if b.visible { 0 } else { 1 };
            avis.cmp(&bvis)
                .then_with(|| {
                    let ay = a.bbox.map(|b| b.y).unwrap_or(f64::MAX);
                    let by = b.bbox.map(|b| b.y).unwrap_or(f64::MAX);
                    ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    let ax = a.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    let bx = b.bbox.map(|b| b.x).unwrap_or(f64::MAX);
                    ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        snippets.truncate(limit);
        snippets
    }

    fn ax_terminal_text_hits(&self, region: Option<Bbox>) -> Vec<TextHit> {
        if !is_terminal_app_name(&self.window.app_name) && !is_terminal_app_name(&self.window.title)
        {
            return Vec::new();
        }

        let fallback_bbox = self.current_window_bounds();
        let mut hits = Vec::new();
        for node in self.scene_graph().nodes.values() {
            if node.role != Role::TextArea {
                continue;
            }
            let bbox = node.bbox.unwrap_or(fallback_bbox);
            if region.map(|r| !bbox_intersects(bbox, r)).unwrap_or(false) {
                continue;
            }
            let Some(text) = node.value.as_deref().or(node.label.as_deref()) else {
                continue;
            };
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                hits.push(TextHit {
                    text: line.to_string(),
                    bbox,
                    confidence: 1.0,
                });
                if hits.len() >= 500 {
                    return hits;
                }
            }
            if hits.is_empty() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    hits.push(TextHit {
                        text: trimmed.to_string(),
                        bbox,
                        confidence: 1.0,
                    });
                }
            }
        }
        hits
    }

    fn current_window_bounds(&self) -> Bbox {
        #[cfg(target_os = "macos")]
        if let Some((x, y, w, h)) = dunst_vision::capture::window_bounds(self.target.window_id) {
            return Bbox { x, y, w, h };
        }
        self.cached_window_rect.unwrap_or(Bbox {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        })
    }

    #[cfg(target_os = "macos")]
    fn display_for_window(&self, window: Bbox) -> Option<DisplaySummary> {
        dunst_vision::capture::display_for_rect(window.x, window.y, window.w, window.h)
            .map(display_summary)
    }

    #[cfg(not(target_os = "macos"))]
    fn display_for_window(&self, _window: Bbox) -> Option<DisplaySummary> {
        None
    }

    // --- verification -------------------------------------------------------

    /// Diff `previous -> current` (empty if only one snapshot exists).
    pub fn diff_since(&self) -> GraphDiff {
        match (&self.previous, &self.current) {
            (Some(p), Some(c)) => audit::diff(p, c),
            _ => GraphDiff::default(),
        }
    }

    /// Assert a node's `field` currently equals `expected`. `field` is one of
    /// `label` | `value` | `enabled` | `focused`.
    pub fn verify_state(&self, id: &str, field: &str, expected: &str) -> dunst_core::Result<bool> {
        let n = self
            .scene_graph()
            .get(id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
        let actual = match field {
            "label" => n.label.clone().unwrap_or_default(),
            "value" => n.value.clone().unwrap_or_default(),
            "enabled" => n.enabled.to_string(),
            "focused" => n.focused.to_string(),
            other => return Err(VisualOpsError::Execution(format!("unknown field {other}"))),
        };
        Ok(actual == expected)
    }

    // --- approval -----------------------------------------------------------

    /// Whitelist a high-risk element so the **next** gated action on it proceeds.
    ///
    /// Audit #2 — validated at call time, not blindly stored. The id must exist in
    /// the current scene (`ElementNotFound` otherwise) and be genuinely gated:
    /// * its own current risk requires approval (a high-risk element / drop target), **or**
    /// * it is the subject of a pending contextual gate — e.g. a destructive value
    ///   typed into an otherwise low-risk field (audit #13).
    ///
    /// Approving a phantom or a plain low-risk id is an error, so a grant can never
    /// be parked on something that isn't gated. The grant is **one-shot**:
    /// [`act`](Self::act) consumes it on the next successful action, and every
    /// [`refresh`](Self::refresh) clears all grants.
    pub fn approve(&mut self, id: &str) -> dunst_core::Result<()> {
        let is_pending_synthetic = self.pending_gate_ids.contains(id);
        let is_scene_id = self.scene_graph().get(id).is_some();
        if !is_scene_id && !is_pending_synthetic {
            return Err(VisualOpsError::ElementNotFound(id.into()));
        }
        let own_gated = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.requires_approval)
            .unwrap_or(false);
        if !own_gated && !is_pending_synthetic {
            return Err(VisualOpsError::Execution(format!(
                "{id} is not gated; no approval required"
            )));
        }
        self.approvals.insert(id.to_string());
        Ok(())
    }

    // --- action tools -------------------------------------------------------

    pub fn click_element(
        &mut self,
        id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let (target_id, action) =
            self.resolve_action_target_refreshing_missing(id, &[SemanticAction::Click])?;
        self.act_refreshing_missing(&target_id, action, None, reasoning, None)
    }

    pub fn raise_element(
        &mut self,
        id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        self.act_refreshing_missing(id, SemanticAction::Raise, None, reasoning, None)
    }

    pub fn pick_option(
        &mut self,
        query: &str,
        visible_only: bool,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<OptionPickResult> {
        let candidate = self.resolve_option_candidate(query, visible_only)?;
        let selected_before = self.option_selected(&candidate.action_id, &candidate.matched_id);
        let action_role = self
            .scene_graph()
            .get(&candidate.action_id)
            .map(|n| n.role.as_str())
            .unwrap_or("unknown");
        let audit = self.act(
            &candidate.action_id,
            candidate.action,
            None,
            reasoning.or(Some("pick option")),
            None,
        )?;
        let (selected_after, closed_after) = if audit.result == ActionResult::Success {
            let after = self.option_selected(&candidate.action_id, &candidate.matched_id);
            let still_visible = self
                .find_element_filtered(query, true)
                .into_iter()
                .any(|n| n.id == candidate.action_id || n.id == candidate.matched_id);
            (after, Some(!still_visible))
        } else {
            (selected_before, None)
        };
        Ok(OptionPickResult {
            query: query.to_string(),
            matched_id: candidate.matched_id,
            action_id: candidate.action_id,
            action_role,
            action: candidate.action,
            selected_before,
            selected_after,
            closed_after,
            audit,
        })
    }

    pub fn type_into(
        &mut self,
        id: &str,
        text: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        // Guard the synchronous keystroke path against a multi-MB payload (audit C9).
        const MAX_TYPE_LEN: usize = 100_000;
        if text.len() > MAX_TYPE_LEN {
            return Err(dunst_core::VisualOpsError::Execution(format!(
                "type text too long: {} bytes (max {MAX_TYPE_LEN})",
                text.len()
            )));
        }
        self.act_refreshing_missing(id, SemanticAction::Type, Some(text), reasoning, None)
    }

    pub fn hover_probe(&mut self, id: &str) -> dunst_core::Result<AuditEntry> {
        self.act_refreshing_missing(id, SemanticAction::Hover, None, Some("hover probe"), None)
    }

    /// Drag `source_id` onto `target_id`. The drop point handed to the executor
    /// is the **target** node's bbox centre in screen coordinates, formatted as
    /// `"x,y"` (the frozen WP-F drag mini-contract). This is a thin wrapper over
    /// the gated action path — `act` checks the *source* exposes `Drag`, gates
    /// on risk, runs the executor, re-perceives, diffs and audits.
    ///
    /// Audit #3 — **composite risk**: a drop is as dangerous as the riskier of its
    /// source and its target (dropping a file onto "Supprimer" is a delete, even
    /// though the file row is harmless). The drop target's risk is folded in here
    /// and `act` gates on the max, so a high-risk target forces approval even when
    /// the source is low-risk.
    pub fn drag_element(
        &mut self,
        source_id: &str,
        target_id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let target = self
            .scene_graph()
            .get(target_id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(target_id.into()))?;
        let bbox = target.bbox.ok_or_else(|| {
            VisualOpsError::Execution(format!(
                "target {target_id} has no bbox; a drop needs a concrete point"
            ))
        })?;
        let x = bbox.x + bbox.w / 2.0;
        let y = bbox.y + bbox.h / 2.0;
        // Fold the drop target's risk into the gate (audit #3). Every node has an
        // affordance entry; default to low if one is somehow missing.
        let target_risk = self
            .affordance_graph()
            .affordances
            .get(target_id)
            .map(|a| a.risk.clone())
            .unwrap_or_else(RiskAssessment::low);
        let co_target = CoTarget {
            id: target_id.to_string(),
            risk: target_risk,
        };
        self.act_refreshing_missing(
            source_id,
            SemanticAction::Drag,
            Some(&format!("{x},{y}")),
            reasoning,
            Some(co_target),
        )
    }

    // --- raw input tools ----------------------------------------------------

    /// Click at a raw **screen point** (P1 navigation: OCR a link with `read_text`,
    /// then click its bbox centre).
    ///
    /// Unlike [`click_element`](Self::click_element), this is not bound to an
    /// element or affordance. A raw click can land on anything under that point,
    /// so it is gated as a high-risk raw action and audited under
    /// `target_id = "screen@x,y"`.
    #[cfg(target_os = "macos")]
    pub fn click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.click_at_button(x, y, 0, "click")
    }

    /// Right-click at a raw screen point (context menus). Background web via SkyLight.
    #[cfg(target_os = "macos")]
    pub fn right_click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.click_at_button(x, y, 1, "right-click")
    }

    /// Double-click at a raw screen point — two quick clicks.
    #[cfg(target_os = "macos")]
    pub fn double_click_at(&mut self, x: f64, y: f64) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, "double-click")?;
        let target_id = format!("screen@{x},{y}:double-click");
        let risk = self.raw_point_risk(x, y);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Click,
            Some(format!("double-click {x},{y}")),
            Some("raw screen double-click"),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let mut outcome = self.raw_click_outcome(x, y, 0);
        std::thread::sleep(std::time::Duration::from_millis(90));
        if outcome.is_ok() {
            outcome = self.raw_click_outcome(x, y, 0);
        }
        self.audit_raw_input(
            target_id,
            SemanticAction::Click,
            Some(format!("double-click {x},{y}")),
            Some("raw screen double-click"),
            risk,
            outcome,
        )
    }

    /// Shared raw click at a screen point. Prefers the SkyLight background path
    /// (reaches a backgrounded/occluded web target, trusted, no cursor move),
    /// falling back to a cursor click. Raw input is high-risk because it is not
    /// tied to a scene element.
    #[cfg(target_os = "macos")]
    fn click_at_button(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        label: &str,
    ) -> dunst_core::Result<AuditEntry> {
        self.ensure_point_in_target_window(x, y, label)?;
        let target_id = format!("screen@{x},{y}:{label}");
        let risk = self.raw_point_risk(x, y);
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Click,
            Some(format!("{label} {x},{y}")),
            Some("raw screen click"),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let outcome = self.raw_click_outcome(x, y, button);
        self.audit_raw_input(
            target_id,
            SemanticAction::Click,
            Some(format!("{label} {x},{y}")),
            Some("raw screen click"),
            risk,
            outcome,
        )
    }

    #[cfg(target_os = "macos")]
    fn raw_click_outcome(&self, x: f64, y: f64, button: u8) -> dunst_core::Result<()> {
        retry_user_active_guard(|| {
            let (ox, oy) = dunst_vision::capture::window_bounds(self.target.window_id)
                .map(|(x, y, _, _)| (x, y))
                .unwrap_or((0.0, 0.0));
            if dunst_platform::click_web_background(
                self.target.pid,
                self.target.window_id,
                x,
                y,
                ox,
                oy,
                button,
            ) {
                Ok(())
            } else if button == 0 {
                dunst_platform::click_at_point(self.target.pid, x, y)
            } else {
                Err(VisualOpsError::Execution(
                    "right-click requires the SkyLight backend".into(),
                ))
            }
        })
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "click_at requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn right_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "right_click_at requires a macOS backend".into(),
        ))
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn double_click_at(&mut self, _x: f64, _y: f64) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "double_click_at requires a macOS backend".into(),
        ))
    }

    /// Open a menu-bar menu by name (e.g. "File"/"Fichier") — finds the menubar
    /// item and presses it (AX). Native menus; the items then appear in the graph.
    pub fn open_menu(&mut self, name: &str) -> dunst_core::Result<AuditEntry> {
        let id = self
            .scene_graph()
            .nodes
            .values()
            .find(|n| {
                n.ax_role.contains("Menu")
                    && n.label
                        .as_deref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(name.trim()))
            })
            .map(|n| n.id.clone());
        match id {
            Some(id) => self.click_element(&id, Some(&format!("open menu {name}"))),
            None => Err(VisualOpsError::Execution(format!(
                "no menu {name:?} found in the menubar"
            ))),
        }
    }

    /// Press a named key (e.g. `"Return"`/`"Enter"` to submit a typed URL).
    /// Raw keyboard input is high-risk because it is not tied to a scene element.
    #[cfg(target_os = "macos")]
    pub fn press_key(&mut self, key: &str) -> dunst_core::Result<AuditEntry> {
        if !is_press_key_name(key) {
            return Err(VisualOpsError::Execution(format!(
                "unsupported key {key:?}; expected return|enter, tab, escape, space, delete, up/down/left/right, pageup/pagedown, home/end"
            )));
        }
        let target_id = format!("keyboard@press:{key}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(key.to_string()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::press_key(self.target.pid, self.target.window_id, key)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(key.to_string()),
            Some("raw key press"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn press_key(&mut self, _key: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
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
        let target_id = "keyboard@type_keys".to_string();
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

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn type_keys(&mut self, _text: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "type_keys requires a macOS backend".into(),
        ))
    }

    /// Select a local file in a native macOS file chooser. When a trigger is
    /// provided, this performs a real System Events click inside the target
    /// window first because browser `input[type=file]` controls often reject
    /// AX/background clicks.
    #[cfg(target_os = "macos")]
    pub fn select_file(
        &mut self,
        path: &str,
        trigger: Option<FileSelectTrigger>,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let file = canonical_file_path(path)?;
        let trigger_point = self.file_select_trigger_point(trigger.as_ref())?;
        let target_id = format!("file@{}", file.display());
        let risk = RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons: vec![
                "selects a local file for upload".to_string(),
                "drives a native file chooser with System Events".to_string(),
            ],
        };
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(file.display().to_string()),
            reasoning.or(Some("select local file for upload")),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| self.select_file_outcome(&file, trigger_point));
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(file.display().to_string()),
            reasoning.or(Some("select local file for upload")),
            risk,
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn select_file(
        &mut self,
        _path: &str,
        _trigger: Option<FileSelectTrigger>,
        _reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "select_file requires a macOS backend".into(),
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
        if let Some(id) = focus_id {
            let can_scroll = self
                .affordance_graph()
                .affordances
                .get(id)
                .map(|a| a.actions.contains(&SemanticAction::Scroll))
                .unwrap_or(false);
            if !can_scroll {
                return Err(VisualOpsError::ActionUnavailable {
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

        // macOS virtual keycodes: PageDown=0x79, PageUp=0x74, Home=0x73, End=0x77.
        let (keycode, n) = match direction {
            "up" => (0x74_u16, pages.clamp(1, 20)),
            "top" => (0x73, 1),
            "bottom" => (0x77, 1),
            _ => (0x79, pages.clamp(1, 20)), // down (default)
        };
        let target_id = format!("keyboard@scroll:{direction}:{n}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(format!("scroll {direction} x{n}")),
            Some("background web scroll"),
            Self::raw_input_risk(Vec::new()),
        ) {
            return Ok(entry);
        }
        let mut outcome = Ok(());
        for _ in 0..n {
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
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(format!("scroll {direction} x{n}")),
            Some("background web scroll (Page/Home/End keys, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn scroll(
        &mut self,
        _direction: &str,
        _pages: usize,
        _focus_id: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "scroll requires a macOS backend".into(),
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
            SemanticAction::Type,
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
            SemanticAction::Type,
            Some(format!("zoom {direction}")),
            Some("background zoom (Cmd =/-/0, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn zoom(&mut self, _direction: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
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
            return Err(VisualOpsError::Execution(message));
        }
        let (flags, keycode) = parse_combo(combo)
            .ok_or_else(|| VisualOpsError::Execution(format!("unrecognised hotkey {combo:?}")))?;
        let target_id = format!("keyboard@hotkey:{combo}");
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
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
            SemanticAction::Type,
            Some(combo.to_string()),
            Some("background hotkey (modifier combo, auth-signed)"),
            Self::raw_input_risk(Vec::new()),
            outcome,
        )
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn hotkey(&mut self, _combo: &str) -> dunst_core::Result<AuditEntry> {
        Err(VisualOpsError::Execution(
            "hotkey requires a macOS backend".into(),
        ))
    }

    /// Background hover at a screen point so the target shows a hover state (e.g.
    /// a chart crosshair tooltip / value-at-cursor) without moving the visible
    /// cursor. A pure probe — no risk-gating, no audit, **no refresh** — so a
    /// following `read_text` reads the hovered result.
    #[cfg(target_os = "macos")]
    pub fn hover_at(&self, x: f64, y: f64) -> dunst_core::Result<()> {
        self.ensure_point_in_target_window(x, y, "hover_at")?;
        self.hover_target_background(x, y)
    }

    /// Non-macOS stub: raw CGEvent input needs the macOS backend.
    #[cfg(not(target_os = "macos"))]
    pub fn hover_at(&self, _x: f64, _y: f64) -> dunst_core::Result<()> {
        Err(VisualOpsError::Execution(
            "hover_at requires a macOS backend".into(),
        ))
    }

    fn file_select_trigger_point(
        &self,
        trigger: Option<&FileSelectTrigger>,
    ) -> dunst_core::Result<Option<(f64, f64)>> {
        match trigger {
            None => Ok(None),
            Some(FileSelectTrigger::Point { x, y }) => {
                self.ensure_point_in_target_window(*x, *y, "select_file trigger")?;
                Ok(Some((*x, *y)))
            }
            Some(FileSelectTrigger::ElementId(id)) => {
                let node = self
                    .scene_graph()
                    .get(id)
                    .ok_or_else(|| VisualOpsError::ElementNotFound(id.clone()))?;
                let bbox = node.bbox.ok_or_else(|| {
                    VisualOpsError::Execution(format!("element {id:?} has no screen bbox"))
                })?;
                if bbox.w <= 0.0 || bbox.h <= 0.0 {
                    return Err(VisualOpsError::Execution(format!(
                        "element {id:?} has an empty screen bbox"
                    )));
                }
                let point = (bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
                self.ensure_point_in_target_window(point.0, point.1, "select_file trigger")?;
                Ok(Some(point))
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn select_file_outcome(
        &self,
        file: &Path,
        trigger_point: Option<(f64, f64)>,
    ) -> dunst_core::Result<()> {
        let mut cmd = std::process::Command::new("/usr/bin/osascript");
        cmd.args([
            "-e",
            "on run argv",
            "-e",
            "set filePath to item 1 of argv",
            "-e",
            "set shouldClick to item 2 of argv",
            "-e",
            "tell application \"System Events\"",
            "-e",
            "if shouldClick is \"1\" then",
            "-e",
            "set px to (item 3 of argv) as integer",
            "-e",
            "set py to (item 4 of argv) as integer",
            "-e",
            "click at {px, py}",
            "-e",
            "delay 0.6",
            "-e",
            "end if",
            "-e",
            "set chooserOpen to false",
            "-e",
            "repeat with p in (every process whose name is \"Open and Save Panel Service\")",
            "-e",
            "set chooserOpen to true",
            "-e",
            "end repeat",
            "-e",
            "if chooserOpen is false then error \"native file chooser did not open\"",
            "-e",
            "keystroke \"g\" using {command down, shift down}",
            "-e",
            "delay 0.2",
            "-e",
            "keystroke filePath",
            "-e",
            "delay 0.2",
            "-e",
            "key code 36",
            "-e",
            "delay 0.5",
            "-e",
            "key code 36",
            "-e",
            "end tell",
            "-e",
            "end run",
        ]);
        cmd.arg(file.as_os_str());
        match trigger_point {
            Some((x, y)) => {
                cmd.arg("1");
                cmd.arg(format!("{}", x.round() as i64));
                cmd.arg(format!("{}", y.round() as i64));
            }
            None => {
                cmd.arg("0");
                cmd.arg("0");
                cmd.arg("0");
            }
        }
        let output = cmd
            .output()
            .map_err(|e| VisualOpsError::Execution(format!("select_file failed: {e}")))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(VisualOpsError::Execution(format!(
            "select_file failed: {}",
            if stderr.is_empty() { stdout } else { stderr }
        )))
    }

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
    fn read_series_background(
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
    fn hover_target_background(&self, x: f64, y: f64) -> dunst_core::Result<()> {
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

    fn clamp_point_to_target_window(&self, x: f64, y: f64) -> (f64, f64) {
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
    fn ocr_window_fovea(&self, cx: f64, cy: f64) -> dunst_core::Result<Vec<TextHit>> {
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
    fn ocr_screen_fovea(&self, cx: f64, cy: f64) -> dunst_core::Result<Vec<TextHit>> {
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
    pub fn launch_app(&self, app: &str, url: Option<&str>, extra_args: &[String]) -> bool {
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
        cmd.status().map(|s| s.success()).unwrap_or(false)
    }

    /// Non-macOS stub.
    #[cfg(not(target_os = "macos"))]
    pub fn launch_app(&self, _app: &str, _url: Option<&str>, _extra_args: &[String]) -> bool {
        false
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

    fn raw_input_risk(extra_reasons: Vec<String>) -> RiskAssessment {
        let mut reasons = vec!["raw input is not bound to a scene element".to_string()];
        reasons.extend(extra_reasons);
        RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons,
        }
    }

    fn raw_point_risk(&self, x: f64, y: f64) -> RiskAssessment {
        let mut reasons = Vec::new();
        let point = (x, y);
        if self
            .cached_window_rect
            .map(|w| !point_in_bbox(point, w))
            .unwrap_or(false)
        {
            reasons.push("raw point is outside the target window".to_string());
        } else {
            let menubar = self.cached_menubar_root.as_deref();
            let hits_visible_node = self.scene_graph().nodes.values().any(|node| {
                !matches!(node.role, Role::Window | Role::MenuBar | Role::Toolbar)
                    && node_visible_or_menu(node, self.cached_window_rect, menubar)
                    && node.bbox.map(|b| point_in_bbox(point, b)).unwrap_or(false)
            });
            if !hits_visible_node {
                reasons.push(
                    "raw point is not inside any visible scene element; possible backdrop or blank area"
                        .to_string(),
                );
            }
        }
        Self::raw_input_risk(reasons)
    }

    fn ensure_point_in_target_window(
        &self,
        x: f64,
        y: f64,
        operation: &str,
    ) -> dunst_core::Result<()> {
        if off_target_raw_allowed() {
            return Ok(());
        }
        let window = self.current_window_bounds();
        if point_in_bbox((x, y), window) {
            return Ok(());
        }
        Err(VisualOpsError::Execution(format!(
            "{operation} point ({x:.1},{y:.1}) is outside the target window {} {:?}; attach the intended window or set DUNST_MCP_ALLOW_OFF_TARGET_RAW=1",
            self.target.window_id,
            window
        )))
    }

    fn ensure_region_in_target_window(
        &self,
        region: Bbox,
        operation: &str,
    ) -> dunst_core::Result<()> {
        if off_target_raw_allowed() {
            return Ok(());
        }
        let window = self.current_window_bounds();
        if rect_intersection_area(region, window) > 0.0
            && region.x >= window.x
            && region.y >= window.y
            && region.x + region.w <= window.x + window.w
            && region.y + region.h <= window.y + window.h
        {
            return Ok(());
        }
        Err(VisualOpsError::Execution(format!(
            "{operation} region {:?} is outside the target window {} {:?}; pass target-window screen coordinates or set DUNST_MCP_ALLOW_OFF_TARGET_RAW=1",
            region,
            self.target.window_id,
            window
        )))
    }

    /// Return a pending-approval audit entry when a raw input has not been
    /// explicitly approved. Raw inputs are nameable by synthetic target ids such
    /// as `screen@x,y:click` and `keyboard@hotkey:cmd+l`.
    fn gate_raw_input(
        &mut self,
        target_id: &str,
        action: SemanticAction,
        argument: Option<String>,
        reasoning: Option<&str>,
        risk: RiskAssessment,
    ) -> Option<AuditEntry> {
        if self.approvals.contains(target_id) {
            return None;
        }
        self.pending_gate_ids.insert(target_id.to_string());
        Some(self.push_entry(AuditEntry {
            ts_ms: dunst_core::now_ms(),
            target_id: target_id.to_string(),
            action,
            argument,
            risk,
            reasoning: reasoning.map(str::to_owned),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        }))
    }

    /// Record a raw input attempt. The attempt is always written to the trace; on
    /// platform failure the entry is `Failed` and the error is surfaced to the
    /// caller. Mirrors [`act`](Self::act)'s re-perceive (`refresh` + `diff_since`).
    #[cfg(target_os = "macos")]
    fn audit_raw_input(
        &mut self,
        target_id: String,
        action: SemanticAction,
        argument: Option<String>,
        reasoning: Option<&str>,
        risk: RiskAssessment,
        outcome: dunst_core::Result<()>,
    ) -> dunst_core::Result<AuditEntry> {
        let ts_ms = dunst_core::now_ms();
        let user_active_blocked = outcome
            .as_ref()
            .err()
            .map(|e| e.to_string().contains("user-active guard blocked"))
            .unwrap_or(false);
        let result = if outcome.is_ok() {
            ActionResult::Success
        } else {
            ActionResult::Failed
        };
        let graph_diff = if result == ActionResult::Success {
            self.approvals.remove(&target_id);
            self.pending_gate_ids.remove(&target_id);
            let _ = self.refresh();
            self.diff_since()
        } else if user_active_blocked {
            GraphDiff::default()
        } else {
            self.approvals.remove(&target_id);
            self.pending_gate_ids.remove(&target_id);
            let _ = self.refresh();
            self.diff_since()
        };
        let entry = self.push_entry(AuditEntry {
            ts_ms,
            target_id,
            action,
            argument,
            risk,
            reasoning: reasoning.map(str::to_owned),
            result,
            graph_diff,
        });
        outcome.map(|()| entry)
    }

    fn resolve_option_candidate(
        &self,
        query: &str,
        visible_only: bool,
    ) -> dunst_core::Result<OptionCandidate> {
        let matches: Vec<String> = self
            .find_element_filtered(query, visible_only)
            .into_iter()
            .map(|n| n.id.clone())
            .collect();

        for matched_id in matches {
            if let Ok((action_id, action)) = self.resolve_action_target(
                &matched_id,
                &[
                    SemanticAction::Pick,
                    SemanticAction::Click,
                    SemanticAction::Toggle,
                ],
            ) {
                return Ok(OptionCandidate {
                    matched_id,
                    action_id,
                    action,
                });
            }
        }

        Err(VisualOpsError::Execution(format!(
            "no clickable option found for query {query:?}"
        )))
    }

    fn resolve_action_target_refreshing_missing(
        &mut self,
        id: &str,
        preferred: &[SemanticAction],
    ) -> dunst_core::Result<(String, SemanticAction)> {
        match self.resolve_action_target(id, preferred) {
            Err(err) if is_element_not_found(&err) => {
                self.refresh()?;
                self.resolve_action_target(id, preferred)
            }
            other => other,
        }
    }

    fn resolve_action_target(
        &self,
        id: &str,
        preferred: &[SemanticAction],
    ) -> dunst_core::Result<(String, SemanticAction)> {
        self.scene_graph()
            .get(id)
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;

        let mut actions = Vec::new();
        for action in preferred {
            push_unique_action(&mut actions, *action);
            if *action == SemanticAction::Click {
                push_unique_action(&mut actions, SemanticAction::Pick);
                push_unique_action(&mut actions, SemanticAction::Toggle);
                push_unique_action(&mut actions, SemanticAction::OpenMenu);
            }
        }

        if let Some(action) = self.first_supported_action(id, &actions) {
            return Ok((id.to_string(), action));
        }

        let requested_risk = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.clone())
            .unwrap_or_else(RiskAssessment::low);
        if requested_risk.requires_approval {
            return Err(VisualOpsError::ActionUnavailable {
                id: id.into(),
                action: preferred
                    .first()
                    .map(|action| format!("{action:?}"))
                    .unwrap_or_else(|| "action".to_string()),
            });
        }

        let mut current = self.scene_graph().get(id).and_then(|n| n.parent.as_deref());
        while let Some(parent_id) = current {
            if let Some(action) = self.first_supported_action(parent_id, &actions) {
                return Ok((parent_id.to_string(), action));
            }
            current = self
                .scene_graph()
                .get(parent_id)
                .and_then(|n| n.parent.as_deref());
        }

        Err(VisualOpsError::ActionUnavailable {
            id: id.into(),
            action: preferred
                .first()
                .map(|action| format!("{action:?}"))
                .unwrap_or_else(|| "action".to_string()),
        })
    }

    fn first_supported_action(
        &self,
        id: &str,
        actions: &[SemanticAction],
    ) -> Option<SemanticAction> {
        let affordance = self.affordance_graph().affordances.get(id)?;
        actions
            .iter()
            .copied()
            .find(|action| affordance.actions.contains(action))
    }

    fn option_selected(&self, action_id: &str, matched_id: &str) -> Option<bool> {
        self.scene_graph()
            .get(action_id)
            .and_then(option_selected_state)
            .or_else(|| {
                self.scene_graph()
                    .get(matched_id)
                    .and_then(option_selected_state)
            })
    }

    /// Compute an action's **effective risk** and the set of ids whose approval
    /// clears its gate. Folds a composite drag's drop target (audit #3) and a
    /// destructive typed payload (audit #13) into the source element's own risk via
    /// [`merge_risk`]. Pure over its inputs and `self.risk` — no scene mutation — so
    /// it is unit-testable in isolation (the `effective_risk_*` tests).
    ///
    /// Returns `(effective, gated_ids)`: `effective.requires_approval` decides
    /// whether the gate fires; `gated_ids` lists every high-risk participant that
    /// must be approved (the element, the drop target, or the typed-into field).
    fn effective_risk(
        &self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        source_risk: &RiskAssessment,
        co_target: Option<&CoTarget>,
    ) -> (RiskAssessment, Vec<String>) {
        // Audit #13: for a Type action the *payload* can be destructive even when
        // the field itself is harmless — assess the typed text and fold it in.
        let text_risk = match (action, argument) {
            (SemanticAction::Type, Some(arg)) => Some(self.risk.assess_text(arg)),
            _ => None,
        };

        // Effective risk = max(source, drop target [#3], typed text [#13]). The
        // merged `reasons` ("drop target: …" / "typed text: …") say which facet
        // raised the gate.
        let mut effective = match co_target {
            Some(co) => merge_risk(source_risk, &co.risk, "drop target"),
            None => source_risk.clone(),
        };
        if let Some(tr) = &text_risk {
            effective = merge_risk(&effective, tr, "typed text");
        }

        // Every high-risk participant must be approved to clear the gate: the
        // element itself, a composite drag's drop target, or the typed-into field.
        let mut gated_ids: Vec<String> = Vec::new();
        if source_risk.requires_approval {
            gated_ids.push(id.to_string());
        }
        if let Some(co) = co_target {
            if co.risk.requires_approval {
                gated_ids.push(co.id.clone());
            }
        }
        if text_risk
            .as_ref()
            .map(|r| r.requires_approval)
            .unwrap_or(false)
            && !gated_ids.iter().any(|g| g == id)
        {
            gated_ids.push(id.to_string());
        }
        (effective, gated_ids)
    }

    /// The gated action path: **resolve → effective_risk → gate → execute →
    /// audit**. Always returns an [`AuditEntry`] describing the outcome (also
    /// appended to the trace); only structural problems (unknown id / unavailable
    /// action) are `Err`.
    ///
    /// `co_target` carries a second risk-bearing participant (audit #3 — a drag's
    /// drop target). The gate fires on the **max** of the acted-on element and the
    /// co-target, and the grant must cover *every* high-risk participant.
    fn act_refreshing_missing(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        co_target: Option<CoTarget>,
    ) -> dunst_core::Result<AuditEntry> {
        match self.act(id, action, argument, reasoning, co_target.clone()) {
            Err(err) if is_element_not_found(&err) => {
                self.refresh()?;
                self.act(id, action, argument, reasoning, co_target)
            }
            other => other,
        }
    }

    fn act(
        &mut self,
        id: &str,
        action: SemanticAction,
        argument: Option<&str>,
        reasoning: Option<&str>,
        co_target: Option<CoTarget>,
    ) -> dunst_core::Result<AuditEntry> {
        let node = self
            .scene_graph()
            .get(id)
            .cloned()
            .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
        // Read the source affordance once and drop the borrow before we mutate.
        let source_risk = {
            let aff = self
                .affordance_graph()
                .affordances
                .get(id)
                .ok_or_else(|| VisualOpsError::ElementNotFound(id.into()))?;
            if !aff.actions.contains(&action) {
                return Err(VisualOpsError::ActionUnavailable {
                    id: id.into(),
                    action: format!("{action:?}"),
                });
            }
            aff.risk.clone()
        };

        // Risk: fold in a composite drag target (#3) and a destructive typed
        // payload (#13). `effective.requires_approval` decides the gate; `gated_ids`
        // names the participants whose approval clears it.
        let (effective, gated_ids) =
            self.effective_risk(id, action, argument, &source_risk, co_target.as_ref());
        // A gate with no nameable participant must NOT pass vacuously: require a
        // non-empty, fully-approved set. (When `effective.requires_approval` is
        // true, `gated_ids` is always non-empty by construction in `effective_risk`.)
        let approved =
            !gated_ids.is_empty() && gated_ids.iter().all(|g| self.approvals.contains(g));

        // Build the audit record once; the two outcome paths only differ in
        // `result` and `graph_diff` (applied via struct update below).
        let base = AuditEntry {
            ts_ms: dunst_core::now_ms(),
            target_id: id.to_string(),
            action,
            argument: argument.map(str::to_owned),
            risk: effective.clone(),
            reasoning: reasoning.map(str::to_owned),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        };

        // Gate: high-risk actions need prior approval. Note the executor is
        // never invoked on this path. Record the gated participants so a later
        // `approve` can authorise a contextually-gated id (audit #13).
        if effective.requires_approval && !approved {
            for g in &gated_ids {
                self.pending_gate_ids.insert(g.clone());
            }
            return Ok(self.push_entry(base));
        }

        // Execute, then re-perceive and diff.
        let executor_result = match retry_user_active_guard(|| {
            self.executor.perform(&self.target, &node, action, argument)
        }) {
            Ok(()) => ActionResult::Success,
            Err(_) => ActionResult::Failed,
        };
        // One-shot consumption (audit #2): a grant authorises exactly one
        // successful action; drop it (and clear any pending-gate marker) so a
        // repeat re-gates. (`refresh` below also clears all grants — this keeps
        // the semantics explicit and independent of refresh ordering.)
        if executor_result == ActionResult::Success {
            for g in &gated_ids {
                self.approvals.remove(g);
                self.pending_gate_ids.remove(g);
            }
        }
        let _ = self.refresh();
        let mut graph_diff = self.diff_since();
        let mut result = verified_action_result(
            &executor_result,
            action,
            id,
            argument,
            &graph_diff,
            self.scene_graph().get(id),
        );
        if executor_result == ActionResult::Success
            && result == ActionResult::Failed
            && matches!(action, SemanticAction::Type)
            && argument.is_some_and(|arg| !arg.is_empty())
        {
            let started = Instant::now();
            while started.elapsed() < TYPE_VERIFY_SETTLE_TIMEOUT {
                std::thread::sleep(TYPE_VERIFY_POLL_INTERVAL);
                if self.refresh().is_err() {
                    break;
                }
                graph_diff = self.diff_since();
                result = verified_action_result(
                    &executor_result,
                    action,
                    id,
                    argument,
                    &graph_diff,
                    self.scene_graph().get(id),
                );
                if result == ActionResult::Success {
                    break;
                }
            }
        }
        Ok(self.push_entry(AuditEntry {
            result,
            graph_diff,
            ..base
        }))
    }

    fn push_entry(&mut self, entry: AuditEntry) -> AuditEntry {
        self.trace.push(entry.clone());
        entry
    }

    // --- audit --------------------------------------------------------------

    /// Public accessor over the audit trail; exercised by the gating tests and
    /// part of the engine API the MCP layer may surface.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "public audit-trail accessor, exercised only by tests"
        )
    )]
    pub fn trace(&self) -> &[AuditEntry] {
        &self.trace
    }

    pub fn export_trace(&self) -> dunst_core::Result<String> {
        Ok(serde_json::to_string_pretty(&self.trace)?)
    }
}

// --- WP-J / J2: latent (non-actionable) node geometry -----------------------

/// The window's on-screen rect, read from the `Window` node's bbox (the scene
/// graph's [`WindowRef`] carries no geometry). `None` when no window node has a
/// bbox — then [`node_on_screen`]'s off-window test is skipped. Memoised by
/// [`Engine::refresh`] into `cached_window_rect` (audit #9).
fn compute_window_rect(g: &SceneGraph) -> Option<Bbox> {
    g.nodes
        .values()
        .find(|n| n.role == Role::Window)
        .and_then(|n| n.bbox)
}

/// Id of the menubar **root** — the `MenuBar`-role node in `roots` (its
/// `AXMenuBarItem` children share that role but have a parent, so iterating
/// `roots` disambiguates). Its direct children are the top-level menu openers
/// exempted from the latent filter by [`is_top_level_menu`]. Memoised by
/// [`Engine::refresh`] into `cached_menubar_root` (audit #9).
fn compute_menubar_root(g: &SceneGraph) -> Option<String> {
    g.roots
        .iter()
        .find(|id| g.get(id).map(|n| n.role == Role::MenuBar).unwrap_or(false))
        .cloned()
}

/// Two axis-aligned boxes overlap (shared positive area).
fn bbox_intersects(a: Bbox, b: Bbox) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

fn point_in_bbox((x, y): (f64, f64), b: Bbox) -> bool {
    x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h
}

fn looks_like_browser_tab(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    let Some(b) = node.bbox else { return false };
    if b.w <= 0.0 || b.h <= 0.0 || b.h > 90.0 {
        return false;
    }
    let Some(window) = window_rect else {
        return true;
    };
    // Browser tab strips sit in the top browser chrome. This filters out page
    // radio controls such as Reddit sort/filter tabs named after communities.
    bbox_intersects(b, window) && b.y >= window.y - 2.0 && b.y <= window.y + 96.0
}

fn browser_tab_title(graph: &SceneGraph, node: &SceneNode) -> String {
    let mut candidates = Vec::new();
    if let Some(label) = node.label.as_deref() {
        candidates.push(label);
    }
    if let Some(value) = node.value.as_deref() {
        candidates.push(value);
    }
    for child_id in &node.children {
        if let Some(child) = graph.get(child_id) {
            if let Some(label) = child.label.as_deref() {
                candidates.push(label);
            }
            if let Some(value) = child.value.as_deref() {
                candidates.push(value);
            }
        }
    }

    candidates
        .into_iter()
        .map(str::trim)
        .find(|s| {
            !s.is_empty()
                && !s.eq_ignore_ascii_case("fermer")
                && !normalize_match(s).starts_with("fermer l")
                && !normalize_match(s).starts_with("close tab")
        })
        .unwrap_or("")
        .to_string()
}

fn browser_tab_selected(graph: &SceneGraph, node: &SceneNode, title: &str) -> bool {
    let window_title = normalize_match(&graph.window.title);
    let tab_title = normalize_match(title);
    node.focused
        || node
            .value
            .as_deref()
            .map(normalize_match)
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "selected" | "selectionne"))
        || (!window_title.is_empty()
            && !tab_title.is_empty()
            && (window_title == tab_title
                || window_title.starts_with(&tab_title)
                || tab_title.starts_with(&window_title)))
}

fn option_selected_state(node: &SceneNode) -> Option<bool> {
    if matches!(node.role, Role::Radio | Role::Checkbox) && node.focused {
        return Some(true);
    }
    let raw = node
        .value
        .as_deref()
        .or(node.label.as_deref())
        .or(node.help.as_deref())?;
    let value = normalize_match(raw);
    if matches!(
        value.as_str(),
        "1" | "true" | "yes" | "on" | "selected" | "checked" | "selectionne" | "coche"
    ) {
        return Some(true);
    }
    if value.contains("not selected")
        || value.contains("not checked")
        || value.contains("non selectionne")
        || matches!(value.as_str(), "0" | "false" | "no" | "off" | "unchecked")
    {
        return Some(false);
    }
    None
}

fn likely_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }
    if trimmed.starts_with("www.") && trimmed.contains('.') {
        return Some(format!("https://{trimmed}"));
    }
    None
}

fn push_unique_string(out: &mut Vec<String>, value: &str, limit: usize) {
    if out.len() >= limit || out.iter().any(|existing| existing == value) {
        return;
    }
    out.push(value.to_string());
}

fn push_unique_action(out: &mut Vec<SemanticAction>, action: SemanticAction) {
    if !out.contains(&action) {
        out.push(action);
    }
}

fn canonical_file_path(path: &str) -> dunst_core::Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(VisualOpsError::Execution(
            "select_file requires a non-empty path".into(),
        ));
    }
    let path = Path::new(trimmed);
    let canonical = path
        .canonicalize()
        .map_err(|e| VisualOpsError::Execution(format!("file {trimmed:?} not accessible: {e}")))?;
    if !canonical.is_file() {
        return Err(VisualOpsError::Execution(format!(
            "path {:?} is not a file",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn off_target_raw_allowed() -> bool {
    std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn normalize_match(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => normalized.push('a'),
            'ç' => normalized.push('c'),
            'è' | 'é' | 'ê' | 'ë' => normalized.push('e'),
            'ì' | 'í' | 'î' | 'ï' => normalized.push('i'),
            'ñ' => normalized.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => normalized.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => normalized.push('u'),
            'ý' | 'ÿ' => normalized.push('y'),
            'æ' => normalized.push_str("ae"),
            'œ' => normalized.push_str("oe"),
            other => normalized.push(other),
        }
    }
    normalized
}

fn normalized_contains_query(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if !haystack.contains(query) {
        return false;
    }
    if !query.chars().all(|ch| ch.is_ascii_alphanumeric()) || query.chars().count() < 4 {
        return true;
    }
    normalized_contains_word(haystack, query)
}

fn normalized_contains_word(haystack: &str, query: &str) -> bool {
    let h: Vec<char> = haystack.chars().collect();
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() || q.len() > h.len() {
        return false;
    }
    for start in 0..=h.len() - q.len() {
        if h[start..start + q.len()] != q[..] {
            continue;
        }
        let before = start == 0 || !h[start - 1].is_ascii_alphanumeric();
        let after = start + q.len() == h.len() || !h[start + q.len()].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
    }
    false
}

fn retry_user_active_guard<T, F>(f: F) -> dunst_core::Result<T>
where
    F: FnMut() -> dunst_core::Result<T>,
{
    retry_user_active_guard_after(Duration::from_millis(400), f)
}

fn retry_user_active_guard_after<T, F>(delay: Duration, mut f: F) -> dunst_core::Result<T>
where
    F: FnMut() -> dunst_core::Result<T>,
{
    let mut next_delay = delay;
    let mut last_guard_err = None;
    for _ in 0..4 {
        match f() {
            Err(err) if is_user_active_guard_error(&err) => {
                last_guard_err = Some(err);
                std::thread::sleep(next_delay);
                next_delay += delay;
            }
            other => return other,
        }
    }
    Err(last_guard_err.expect("guard retry loop stores the last guard error"))
}

fn is_user_active_guard_error(err: &VisualOpsError) -> bool {
    err.to_string().contains("user-active guard blocked")
}

fn is_element_not_found(err: &VisualOpsError) -> bool {
    matches!(err, VisualOpsError::ElementNotFound(_))
}

fn is_terminal_app_name(value: &str) -> bool {
    let app = normalize_match(value);
    [
        "iterm",
        "iterm2",
        "terminal",
        "wezterm",
        "ghostty",
        "alacritty",
        "kitty",
        "warp",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

#[cfg(target_os = "macos")]
fn app_search_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

#[cfg(target_os = "macos")]
fn collect_app_bundles(
    dir: &Path,
    depth: usize,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<PathBuf>,
    max: usize,
) {
    if depth > 3 || out.len() >= max {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= max {
            break;
        }
        let path = entry.path();
        let is_app = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("app"));
        if is_app {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key) {
                out.push(path);
            }
            continue;
        }
        if path.is_dir() {
            collect_app_bundles(&path, depth + 1, seen, out, max);
        }
    }
}

#[cfg(target_os = "macos")]
fn launchable_app_from_bundle(path: &Path, running: &BTreeSet<String>) -> Option<LaunchableApp> {
    let info_path = path.join("Contents/Info.plist");
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&info_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let info: Value = serde_json::from_slice(&output.stdout).ok()?;
    launchable_app_from_info_json(path, &info, running)
}

fn launchable_app_from_info_json(
    path: &Path,
    info: &Value,
    running: &BTreeSet<String>,
) -> Option<LaunchableApp> {
    let bundle_name = path.file_stem()?.to_string_lossy().to_string();
    let display_name = info_string(info, "CFBundleDisplayName")
        .or_else(|| info_string(info, "CFBundleName"))
        .unwrap_or_else(|| bundle_name.clone());
    let executable = info_string(info, "CFBundleExecutable").map(|exe| {
        path.join("Contents/MacOS")
            .join(exe)
            .to_string_lossy()
            .to_string()
    });
    let running = running.contains(&normalize_match(&display_name))
        || running.contains(&normalize_match(&bundle_name));
    Some(LaunchableApp {
        name: bundle_name,
        display_name,
        bundle_id: info_string(info, "CFBundleIdentifier"),
        version: info_string(info, "CFBundleShortVersionString")
            .or_else(|| info_string(info, "CFBundleVersion")),
        category: info_string(info, "LSApplicationCategoryType"),
        description: info_string(info, "CFBundleGetInfoString")
            .or_else(|| info_string(info, "NSHumanReadableCopyright")),
        path: path.to_string_lossy().to_string(),
        executable,
        running,
    })
}

fn info_string(info: &Value, key: &str) -> Option<String> {
    info.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// WP-J/J2 — whether a node has a real on-screen footprint. A node is **latent**
/// (the negation) when it has no bbox, a zero/negative-area bbox, or a bbox that
/// lies entirely outside the window rect — exactly the shape of collapsed-menu
/// `AXMenuItem`s, which sit at `(0,0)`/off-window until their parent opens. This
/// is read-only geometry over `bbox` + the window rect: the scene/affordance
/// graphs are never mutated, so `find_element` and click-by-id still reach these
/// nodes; only the *listings* filter them.
fn node_on_screen(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    let Some(b) = node.bbox else { return false };
    if b.w <= 0.0 || b.h <= 0.0 {
        return false;
    }
    match window_rect {
        Some(w) => bbox_intersects(b, w),
        None => true,
    }
}

/// WP-J follow-up — a node is a **top-level menu opener** when it sits directly
/// under the menubar root (Fichier, Édition, Format, …). These are legitimately
/// actionable (click / open_menu opens the menu) even with a null/off-window
/// bbox, so they are exempt from the latent filter. The rule is *structural*
/// (parent == menubar root id): deep collapsed submenu items — whose parent is a
/// closed `Menu`, not the menubar root — are NOT exempt and stay filtered.
fn is_top_level_menu(node: &SceneNode, menubar_root: Option<&str>) -> bool {
    matches!(
        (node.parent.as_deref(), menubar_root),
        (Some(parent), Some(root)) if parent == root
    )
}

/// Visible in actionable listings: a real on-screen footprint OR a top-level
/// menu opener (see [`is_top_level_menu`]). This is the predicate the affordance
/// listings filter on (geometry, no `enabled` requirement).
fn node_visible_or_menu(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    node_on_screen(node, window_rect) || is_top_level_menu(node, menubar_root)
}

fn read_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    if is_top_level_menu(node, menubar_root)
        || matches!(
            node.role,
            Role::Window | Role::Toolbar | Role::MenuBar | Role::Menu | Role::MenuItem
        )
    {
        return true;
    }
    is_unlabeled_window_chrome_button(node, window_rect)
        || browser_chrome_node(graph, node, window_rect)
        || web_app_chrome_node(graph, node, window_rect)
}

fn page_state_chrome_node(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    read_chrome_node(graph, node, window_rect, menubar_root)
}

fn browser_chrome_node(graph: &SceneGraph, node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    if !is_browser_app_name(&graph.window.app_name) {
        return false;
    }
    if node_in_browser_tab_strip(graph, node, window_rect) {
        return true;
    }
    let Some(window) = window_rect else {
        return false;
    };
    let Some(b) = node.bbox else { return false };
    bbox_intersects(b, window)
        && b.y <= window.y + 104.0
        && matches!(
            node.role,
            Role::Button
                | Role::MenuButton
                | Role::TextField
                | Role::TextArea
                | Role::StaticText
                | Role::Radio
                | Role::Toolbar
        )
}

fn web_app_chrome_node(graph: &SceneGraph, node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    if !is_browser_app_name(&graph.window.app_name) {
        return false;
    }
    let Some(window) = window_rect else {
        return false;
    };
    let Some(b) = node.bbox else { return false };
    if !bbox_intersects(b, window) {
        return false;
    }
    let Some(raw) = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.help.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    let text = normalize_match(raw);

    if likely_url(raw).is_some() && (b.y <= window.y + 220.0 || b.x <= window.x + window.w * 0.32) {
        return true;
    }
    if matches!(
        text.as_str(),
        "open intercom messenger"
            | "help center"
            | "copy"
            | "copier"
            | "compte"
            | "account"
            | "nouveautes"
            | "notifications"
    ) {
        return true;
    }

    let left_rail = b.x <= window.x + window.w * 0.28;
    let top_nav = b.y <= window.y + 180.0;
    (left_rail || top_nav)
        && matches!(
            text.as_str(),
            "accueil" | "home" | "connect" | "profil" | "profile" | "parametres" | "settings"
        )
}

fn node_in_browser_tab_strip(
    graph: &SceneGraph,
    node: &SceneNode,
    window_rect: Option<Bbox>,
) -> bool {
    if looks_like_browser_tab(node, window_rect) {
        return true;
    }
    let mut current = node.parent.as_deref();
    for _ in 0..4 {
        let Some(parent_id) = current else {
            return false;
        };
        let Some(parent) = graph.get(parent_id) else {
            return false;
        };
        if looks_like_browser_tab(parent, window_rect) {
            return true;
        }
        current = parent.parent.as_deref();
    }
    false
}

fn is_browser_app_name(app_name: &str) -> bool {
    let app = normalize_match(app_name);
    [
        "firefox",
        "google chrome",
        "chromium",
        "safari",
        "zen",
        "arc",
        "brave",
        "microsoft edge",
        "edge",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}

fn page_state_key_element_candidate(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    let has_text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.help.as_deref())
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if node.bbox.is_some_and(|b| b.w < 4.0 || b.h < 4.0) {
        return false;
    }
    if !has_text && matches!(node.role, Role::Unknown | Role::Group | Role::Image) {
        return false;
    }
    if !has_text
        && node.bbox.is_some_and(|b| {
            window_rect.is_some_and(|window| {
                let node_area = b.w.max(0.0) * b.h.max(0.0);
                let window_area = window.w.max(0.0) * window.h.max(0.0);
                window_area > 0.0 && node_area >= window_area * 0.50
            })
        })
    {
        return false;
    }
    if is_top_level_menu(node, menubar_root)
        || matches!(
            node.role,
            Role::Window | Role::Toolbar | Role::MenuBar | Role::Menu | Role::MenuItem
        )
    {
        return false;
    }
    !is_unlabeled_window_chrome_button(node, window_rect)
}

fn page_state_repetitive_destructive_keys(
    graph: &SceneGraph,
    affordances: &AffordanceGraph,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> BTreeSet<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in graph.nodes.values() {
        if !node_visible_or_menu(node, window_rect, menubar_root) {
            continue;
        }
        if let Some(key) = repetitive_destructive_key(node, affordances) {
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(key, count)| (count >= 5).then_some(key))
        .collect()
}

fn page_state_suppressed_repetitive_destructive(
    node: &SceneNode,
    suppressed: &BTreeSet<String>,
) -> bool {
    suppressed.contains(&repetitive_destructive_key_for_text(node).unwrap_or_default())
}

fn repetitive_destructive_key(node: &SceneNode, affordances: &AffordanceGraph) -> Option<String> {
    if !matches!(node.role, Role::Button | Role::MenuButton | Role::Group) {
        return None;
    }
    let affordance = affordances.affordances.get(&node.id)?;
    if !affordance.actions.iter().any(|action| {
        matches!(
            action,
            SemanticAction::Click | SemanticAction::Pick | SemanticAction::Toggle
        )
    }) {
        return None;
    }
    repetitive_destructive_key_for_text(node)
}

fn repetitive_destructive_key_for_text(node: &SceneNode) -> Option<String> {
    let text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .or(node.ax_identifier.as_deref())?;
    let normalized = normalize_match(text);
    let key = match normalized.as_str() {
        "x" | "×" | "remove" | "delete" | "supprimer" | "retirer" => normalized,
        _ if normalized.starts_with("remove ") => "remove".to_string(),
        _ if normalized.starts_with("delete ") => "delete".to_string(),
        _ if normalized.starts_with("supprimer ") => "supprimer".to_string(),
        _ if normalized.starts_with("retirer ") => "retirer".to_string(),
        _ => return None,
    };
    Some(key)
}

fn is_unlabeled_window_chrome_button(node: &SceneNode, window_rect: Option<Bbox>) -> bool {
    if !matches!(node.role, Role::Button | Role::MenuButton) {
        return false;
    }
    let has_text = node
        .label
        .as_deref()
        .or(node.value.as_deref())
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if has_text {
        return false;
    }
    let Some(b) = node.bbox else { return false };
    if b.w > 24.0 || b.h > 24.0 {
        return false;
    }
    match window_rect {
        Some(w) => b.x <= w.x + 96.0 && b.y <= w.y + 48.0,
        None => false,
    }
}

/// J1 actionability: visible (on-screen or a top-level menu opener) **and**
/// enabled (what `actionable_only` keeps and `summary.n_actionable` counts).
fn node_actionable(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> bool {
    node.enabled && node_visible_or_menu(node, window_rect, menubar_root)
}

/// Ranking for search results: page-visible enabled targets first, then visible
/// disabled/read-only nodes, then latent/off-window noise. The final tie-breakers
/// keep output deterministic without changing the underlying scene graph.
fn find_rank(
    node: &SceneNode,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> (u8, u8, &'static str, String) {
    let tier = if node_actionable(node, window_rect, menubar_root) {
        0
    } else if node_visible_or_menu(node, window_rect, menubar_root) {
        1
    } else if node.bbox.is_some() {
        2
    } else {
        3
    };
    (
        tier,
        find_role_priority(node.role),
        node.role.as_str(),
        node.id.clone(),
    )
}

fn find_role_priority(role: Role) -> u8 {
    match role {
        Role::TextField | Role::TextArea => 0,
        Role::Checkbox | Role::Radio | Role::MenuButton => 1,
        Role::Button | Role::Row | Role::Cell => 2,
        Role::List | Role::Table | Role::Outline => 3,
        Role::Group | Role::Unknown => 4,
        Role::Window | Role::Toolbar | Role::Menu | Role::MenuBar | Role::MenuItem => 5,
        Role::Image => 6,
        Role::StaticText => 7,
    }
}

fn associated_control_for_label<'a>(
    label: &SceneNode,
    graph: &'a SceneGraph,
    window_rect: Option<Bbox>,
    menubar_root: Option<&str>,
) -> Option<&'a SceneNode> {
    if label.role != Role::StaticText {
        return None;
    }
    graph
        .nodes
        .values()
        .filter(|candidate| {
            candidate.id != label.id
                && is_label_associable_control(candidate.role)
                && node_visible_or_menu(candidate, window_rect, menubar_root)
        })
        .filter_map(|candidate| {
            associated_control_score(label, candidate).map(|score| (score, candidate))
        })
        .min_by_key(|(score, candidate)| (*score, candidate.id.clone()))
        .map(|(_, candidate)| candidate)
}

fn is_label_associable_control(role: Role) -> bool {
    matches!(
        role,
        Role::TextField | Role::TextArea | Role::Checkbox | Role::Radio | Role::MenuButton
    )
}

fn associated_control_score(
    label: &SceneNode,
    candidate: &SceneNode,
) -> Option<(u8, u8, i64, i64)> {
    let label_box = label.bbox?;
    let candidate_box = candidate.bbox?;
    let same_parent = label.parent.as_deref() == candidate.parent.as_deref();
    let vertical_gap = candidate_box.y - (label_box.y + label_box.h);
    let horizontal_delta = (candidate_box.x - label_box.x).abs();
    let overlaps_x = intervals_overlap(
        label_box.x - 24.0,
        label_box.x + label_box.w + 24.0,
        candidate_box.x,
        candidate_box.x + candidate_box.w,
    );
    let overlaps_y = intervals_overlap(
        label_box.y - 8.0,
        label_box.y + label_box.h + 8.0,
        candidate_box.y,
        candidate_box.y + candidate_box.h,
    );
    let below_label = (-4.0..=96.0).contains(&vertical_gap) && overlaps_x;
    let right_of_label = overlaps_y
        && candidate_box.x >= label_box.x + label_box.w - 8.0
        && horizontal_delta <= 360.0;

    if !below_label && !right_of_label {
        return None;
    }

    Some((
        u8::from(!same_parent),
        if below_label { 0 } else { 1 },
        vertical_gap.max(0.0).round() as i64,
        horizontal_delta.round() as i64,
    ))
}

fn intervals_overlap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    a_start <= b_end && b_start <= a_end
}

/// WP-J/J1 compact projection of one node: keep only the agent-facing fields and
/// drop the heavy/derivable AX detail (`ax_role`, `help`, `ax_actions`,
/// `ax_identifier`, `last_seen_ms`), collapsing `children` to a count.
fn compact_node(n: &SceneNode) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("id".into(), json!(n.id));
    o.insert("role".into(), json!(n.role.as_str()));
    if let Some(l) = &n.label {
        o.insert("label".into(), json!(l));
    }
    if let Some(v) = &n.value {
        o.insert("value".into(), json!(v));
    }
    o.insert(
        "bbox".into(),
        serde_json::to_value(n.bbox).unwrap_or(Value::Null),
    );
    o.insert("enabled".into(), json!(n.enabled));
    o.insert("focused".into(), json!(n.focused));
    if let Some(p) = &n.parent {
        o.insert("parent".into(), json!(p));
    }
    o.insert("n_children".into(), json!(n.children.len()));
    Value::Object(o)
}

/// A second risk-bearing participant in an action — the **drop target** of a drag
/// (audit #3). Carried into [`Engine::act`] so the gate can combine its risk with
/// the dragged element's.
#[derive(Clone)]
struct CoTarget {
    id: String,
    risk: RiskAssessment,
}

struct OptionCandidate {
    matched_id: String,
    action_id: String,
    action: SemanticAction,
}

/// Combine a base risk with an extra risk-bearing facet (a drag's drop target,
/// audit #3; or the typed payload, audit #13): the higher tier, approval required
/// if *either* requires it, and the extra's reasons merged in with `label: …` so
/// the audit shows which facet raised the gate. `RiskLevel` is `Ord`, so `max` is
/// the stricter tier.
fn merge_risk(base: &RiskAssessment, extra: &RiskAssessment, label: &str) -> RiskAssessment {
    RiskAssessment {
        level: base.level.max(extra.level),
        requires_approval: base.requires_approval || extra.requires_approval,
        reasons: base
            .reasons
            .iter()
            .cloned()
            .chain(extra.reasons.iter().map(|r| format!("{label}: {r}")))
            .collect(),
    }
}

fn verified_action_result(
    executor_result: &ActionResult,
    action: SemanticAction,
    id: &str,
    argument: Option<&str>,
    graph_diff: &GraphDiff,
    current_node: Option<&SceneNode>,
) -> ActionResult {
    if *executor_result != ActionResult::Success {
        return executor_result.clone();
    }

    match action {
        SemanticAction::Type if argument.is_some_and(|arg| !arg.is_empty()) => {
            if typed_target_value_matches_expected(
                id,
                argument.unwrap_or_default(),
                graph_diff,
                current_node,
            ) {
                ActionResult::Success
            } else {
                ActionResult::Failed
            }
        }
        _ => ActionResult::Success,
    }
}

fn typed_target_value_matches_expected(
    id: &str,
    expected: &str,
    graph_diff: &GraphDiff,
    current_node: Option<&SceneNode>,
) -> bool {
    current_node
        .and_then(|node| node.value.as_deref().or(node.label.as_deref()))
        .is_some_and(|value| value == expected)
        || graph_diff.changes.iter().any(|change| {
            matches!(
                change,
                dunst_core::NodeChange::Changed { id: changed_id, field, after, .. }
                    if changed_id == id && matches!(field.as_str(), "value" | "label") && after == expected
            )
        })
}

#[cfg(test)]
mod helper_tests {
    use super::{
        base64_encode, char_keycode, is_axis_token, is_press_key_name,
        launchable_app_from_info_json, layout_sensitive_hotkey_message, looks_like_clock,
        parse_combo, parse_value, typed_target_value_matches_expected,
    };
    use dunst_core::{GraphDiff, NodeChange};
    use serde_json::json;
    use std::{collections::BTreeSet, path::Path};

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn parse_combo_reads_modifiers_and_key() {
        assert_eq!(parse_combo("cmd+l"), Some((0x0010_0000, 0x25)));
        assert_eq!(parse_combo("cmd+shift+t"), Some((0x0012_0000, 0x11)));
        assert_eq!(parse_combo("ctrl+a"), Some((0x0004_0000, 0x00)));
        assert_eq!(parse_combo("enter"), Some((0, 0x24)));
        assert_eq!(parse_combo("cmd+ "), None); // no key
    }

    #[test]
    fn cmd_a_is_rejected_as_layout_sensitive() {
        let message = layout_sensitive_hotkey_message("cmd+a").unwrap();
        assert!(message.contains("keyboard-layout sensitive"));
        assert!(layout_sensitive_hotkey_message("ctrl+a").is_none());
        assert!(layout_sensitive_hotkey_message("cmd+l").is_none());
        assert!(layout_sensitive_hotkey_message("cmd+shift+a").is_none());
    }

    #[test]
    fn press_key_whitelist_includes_navigation_keys() {
        for key in ["Home", "End", "PageUp", "PageDown", "page_up", "page_down"] {
            assert!(is_press_key_name(key), "{key} should be accepted");
        }
        assert!(!is_press_key_name("definitely-not-a-real-key"));
    }

    #[test]
    fn typed_verification_rejects_partial_target_value() {
        let diff = GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "field_title".into(),
                field: "value".into(),
                before: "old".into(),
                after: "nce - partial".into(),
            }],
        };

        assert!(!typed_target_value_matches_expected(
            "field_title",
            "Freelance - full",
            &diff,
            None,
        ));
    }

    #[test]
    fn typed_verification_accepts_exact_target_value() {
        let diff = GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "field_title".into(),
                field: "value".into(),
                before: "old".into(),
                after: "Freelance - full".into(),
            }],
        };

        assert!(typed_target_value_matches_expected(
            "field_title",
            "Freelance - full",
            &diff,
            None,
        ));
    }

    #[test]
    fn char_and_axis_helpers() {
        assert_eq!(char_keycode('a'), Some(0x00));
        assert_eq!(char_keycode('Z'), Some(0x06));
        assert_eq!(char_keycode('='), Some(0x18));
        assert!(looks_like_clock("13:00 UTC+2"));
        assert!(!looks_like_clock("clôture"));
        assert!(is_axis_token("09:30"));
        assert!(is_axis_token("11"));
        assert!(!is_axis_token("À la clôture de 17:35"));
        assert_eq!(parse_value("8 220,00"), Some(8220.0));
        assert_eq!(parse_value("8161,84'"), Some(8161.84));
    }

    #[test]
    fn launchable_app_info_is_derived_from_plist_json() {
        let info = json!({
            "CFBundleDisplayName": "Demo Browser",
            "CFBundleName": "Demo",
            "CFBundleIdentifier": "com.example.demo",
            "CFBundleShortVersionString": "1.2.3",
            "LSApplicationCategoryType": "public.app-category.productivity",
            "CFBundleExecutable": "DemoExec",
            "CFBundleGetInfoString": "Demo description"
        });
        let mut running = BTreeSet::new();
        running.insert("demo browser".to_string());
        let app =
            launchable_app_from_info_json(Path::new("/Applications/Demo.app"), &info, &running)
                .expect("plist json maps to launchable app");
        assert_eq!(app.name, "Demo");
        assert_eq!(app.display_name, "Demo Browser");
        assert_eq!(app.bundle_id.as_deref(), Some("com.example.demo"));
        assert_eq!(app.version.as_deref(), Some("1.2.3"));
        assert_eq!(
            app.category.as_deref(),
            Some("public.app-category.productivity")
        );
        assert_eq!(app.description.as_deref(), Some("Demo description"));
        assert!(app
            .executable
            .as_deref()
            .unwrap()
            .ends_with("Contents/MacOS/DemoExec"));
        assert!(app.running);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dunst_core::mock::MockPerceptor;
    use dunst_core::RiskLevel;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Executor that counts invocations, so we can assert a gated action never
    /// reaches the OS.
    struct CountingExecutor(Arc<AtomicUsize>);
    impl ActionExecutor for CountingExecutor {
        fn perform(
            &self,
            _t: &Target,
            _n: &SceneNode,
            _a: SemanticAction,
            _arg: Option<&str>,
        ) -> dunst_core::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn engine_from_json(json: &str, app_name: &str, title: &str) -> Engine {
        let perceptor = Box::new(
            MockPerceptor::from_json(
                json,
                WindowRef {
                    pid: 4242,
                    window_id: 2424,
                    app_name: app_name.into(),
                    title: title.into(),
                },
            )
            .unwrap(),
        );
        let exec = Box::new(CountingExecutor(Arc::new(AtomicUsize::new(0))));
        Engine::new(
            perceptor,
            exec,
            Target {
                pid: 4242,
                window_id: 2424,
            },
        )
        .unwrap()
    }

    fn engine_with_counter() -> (Engine, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::new(CountingExecutor(calls.clone()));
        let eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1363,
                window_id: 105,
            },
        )
        .unwrap();
        (eng, calls)
    }

    type RecordedCall = (String, SemanticAction, Option<String>);

    /// Executor that records every `(id, action, argument)` it receives, so we
    /// can assert exactly what the engine resolved an action to.
    struct RecordingExecutor(Arc<Mutex<Vec<RecordedCall>>>);
    impl ActionExecutor for RecordingExecutor {
        fn perform(
            &self,
            _t: &Target,
            n: &SceneNode,
            a: SemanticAction,
            arg: Option<&str>,
        ) -> dunst_core::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push((n.id.clone(), a, arg.map(str::to_owned)));
            Ok(())
        }
    }

    fn engine_with_recorder() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1363,
                window_id: 105,
            },
        )
        .unwrap();
        (eng, calls)
    }

    /// An id from the affordance graph that exposes `Drag` and is *not* risk
    /// gated, so the executor actually runs (rows/cells in the notes fixture).
    fn non_gated_drag_source(eng: &Engine) -> String {
        eng.query_affordances(SemanticAction::Drag)
            .into_iter()
            .find(|id| {
                !eng.affordance_graph().affordances[id]
                    .risk
                    .requires_approval
            })
            .expect("a non-gated draggable source in the notes fixture")
    }

    fn id_for(eng: &Engine, query: &str) -> String {
        eng.find_element(query)
            .first()
            .map(|n| n.id.clone())
            .unwrap_or_else(|| panic!("no element for {query:?}"))
    }

    fn raw_node(
        ax_role: &str,
        label: Option<&str>,
        value: Option<&str>,
        frame: Option<Bbox>,
        ax_actions: &[&str],
        children: Vec<dunst_core::RawAxNode>,
    ) -> dunst_core::RawAxNode {
        dunst_core::RawAxNode {
            ax_role: ax_role.into(),
            label: label.map(str::to_owned),
            help: None,
            value: value.map(str::to_owned),
            ax_identifier: None,
            ax_actions: ax_actions.iter().map(|s| s.to_string()).collect(),
            frame,
            enabled: true,
            focused: false,
            children,
        }
    }

    fn test_bbox(x: f64, y: f64, w: f64, h: f64) -> Option<Bbox> {
        Some(Bbox { x, y, w, h })
    }

    fn engine_from_roots(
        roots: Vec<dunst_core::RawAxNode>,
        app_name: &str,
        title: &str,
    ) -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let perceptor = Box::new(MockPerceptor::new(
            roots,
            WindowRef {
                pid: 1,
                window_id: 1,
                app_name: app_name.into(),
                title: title.into(),
            },
        ));
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1,
                window_id: 1,
            },
        )
        .unwrap();
        (eng, calls)
    }

    struct SequencePerceptor {
        captures: Mutex<Vec<Vec<dunst_core::RawAxNode>>>,
        last: Mutex<Vec<dunst_core::RawAxNode>>,
        window: WindowRef,
    }

    impl SequencePerceptor {
        fn new(captures: Vec<Vec<dunst_core::RawAxNode>>, window: WindowRef) -> Self {
            Self {
                captures: Mutex::new(captures),
                last: Mutex::new(Vec::new()),
                window,
            }
        }
    }

    impl Perceptor for SequencePerceptor {
        fn capture(&self, _target: &Target) -> dunst_core::Result<Vec<dunst_core::RawAxNode>> {
            let next = {
                let mut captures = self.captures.lock().unwrap();
                if captures.is_empty() {
                    None
                } else {
                    Some(captures.remove(0))
                }
            };
            if let Some(roots) = next {
                *self.last.lock().unwrap() = roots.clone();
                Ok(roots)
            } else {
                Ok(self.last.lock().unwrap().clone())
            }
        }

        fn window_ref(&self, _target: &Target) -> dunst_core::Result<WindowRef> {
            Ok(self.window.clone())
        }
    }

    #[test]
    fn low_risk_click_proceeds_and_executes() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Nouvelle note");
        let entry = eng.click_element(&id, Some("create")).unwrap();
        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn click_element_refreshes_once_when_id_is_missing_from_stale_graph() {
        let stale = raw_node(
            "AXWindow",
            Some("CVs"),
            None,
            test_bbox(0.0, 0.0, 700.0, 900.0),
            &[],
            vec![],
        );
        let fresh = raw_node(
            "AXWindow",
            Some("CVs"),
            None,
            test_bbox(0.0, 0.0, 700.0, 900.0),
            &[],
            vec![raw_node(
                "AXButton",
                Some("Importer"),
                None,
                test_bbox(300.0, 200.0, 120.0, 32.0),
                &["press"],
                vec![],
            )],
        );
        let perceptor = Box::new(SequencePerceptor::new(
            vec![vec![stale], vec![fresh]],
            WindowRef {
                pid: 1,
                window_id: 1,
                app_name: "Firefox".into(),
                title: "Collective".into(),
            },
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let mut eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1,
                window_id: 1,
            },
        )
        .unwrap();

        assert!(
            eng.scene_graph().get("btn_importer").is_none(),
            "initial graph is stale and lacks the target"
        );
        let entry = eng
            .click_element("btn_importer", Some("retry stale graph"))
            .unwrap();
        assert_eq!(entry.result, ActionResult::Success);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "btn_importer");
    }

    #[test]
    fn type_into_waits_for_ax_value_to_settle() {
        let expected =
            "Freelance — Architecte DevSecOps & Platform Engineering | Mission Défense air-gapped";
        let window = |value: &str| {
            raw_node(
                "AXWindow",
                Some("Collective"),
                None,
                test_bbox(0.0, 0.0, 700.0, 900.0),
                &[],
                vec![raw_node(
                    "AXTextField",
                    Some("Titre de poste"),
                    Some(value),
                    test_bbox(100.0, 120.0, 568.0, 32.0),
                    &["press"],
                    vec![],
                )],
            )
        };
        let perceptor = Box::new(SequencePerceptor::new(
            vec![
                vec![window("nce — Architecte DevSecOps & Platform Engineering | Mission Défense air-gapped")],
                vec![window("Freel")],
                vec![window(expected)],
            ],
            WindowRef {
                pid: 1,
                window_id: 1,
                app_name: "Firefox".into(),
                title: "Collective".into(),
            },
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let mut eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1,
                window_id: 1,
            },
        )
        .unwrap();
        let field = id_for(&eng, "Titre de poste");

        let entry = eng
            .type_into(&field, expected, Some("wait for AX settle"))
            .unwrap();

        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(
            eng.scene_graph().get(&field).unwrap().value.as_deref(),
            Some(expected)
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, field);
    }

    #[test]
    fn high_risk_click_is_gated_then_approved() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        // 1. Denied pending approval — and the executor must NOT have run.
        let e1 = eng.click_element(&id, Some("delete")).unwrap();
        assert_eq!(e1.result, ActionResult::PendingApproval);
        assert_eq!(e1.risk.level, RiskLevel::High);
        assert!(e1.risk.requires_approval);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "executor must not run on a gated action"
        );

        // 2. Approve, retry — proceeds, executor called exactly once.
        eng.approve(&id).unwrap();
        let e2 = eng.click_element(&id, Some("approved")).unwrap();
        assert_eq!(e2.result, ActionResult::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // --- Audit #2: validated, one-shot, refresh-invalidated approvals --------

    #[test]
    fn approval_is_one_shot_consumed_by_act() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        eng.approve(&id).unwrap();
        assert_eq!(
            eng.click_element(&id, Some("1st")).unwrap().result,
            ActionResult::Success
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The grant authorised exactly one action: a second high-risk click on the
        // same element (re-resolved after the post-action refresh) gates again.
        let id2 = id_for(&eng, "Supprimer");
        let e2 = eng.click_element(&id2, Some("2nd")).unwrap();
        assert_eq!(
            e2.result,
            ActionResult::PendingApproval,
            "grant must not survive one use"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no second execution without re-approval"
        );
    }

    #[test]
    fn approval_is_invalidated_by_refresh() {
        let (mut eng, calls) = engine_with_counter();
        let id = id_for(&eng, "Supprimer");

        eng.approve(&id).unwrap();
        eng.refresh().unwrap(); // scene re-perceived → the grant must be dropped

        let id2 = id_for(&eng, "Supprimer");
        let e = eng.click_element(&id2, Some("after refresh")).unwrap();
        assert_eq!(
            e.result,
            ActionResult::PendingApproval,
            "refresh invalidates approvals"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "executor never ran");
    }

    #[test]
    fn approve_rejects_unknown_and_non_gated_ids() {
        let (mut eng, calls) = engine_with_counter();

        // Unknown id → ElementNotFound; nothing is stored.
        let err = eng.approve("no_such_id").unwrap_err();
        assert!(matches!(err, VisualOpsError::ElementNotFound(_)));

        // A low-risk element (toolbar button) is not gated → error, nothing stored.
        let low = id_for(&eng, "Nouvelle note");
        assert!(
            eng.approve(&low).is_err(),
            "approving a non-gated id is rejected"
        );

        // And because the bogus grants were rejected, the high-risk gate is intact:
        // "Supprimer" is still PendingApproval (no spurious approval leaked).
        let supprimer = id_for(&eng, "Supprimer");
        let e = eng.click_element(&supprimer, None).unwrap();
        assert_eq!(e.result, ActionResult::PendingApproval);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn raw_input_gate_requires_pending_synthetic_approval() {
        let (mut eng, _) = engine_with_counter();

        let pending = eng
            .gate_raw_input(
                "screen@10,20:click",
                SemanticAction::Click,
                Some("click 10,20".to_string()),
                Some("raw screen click"),
                Engine::raw_input_risk(Vec::new()),
            )
            .expect("unapproved raw input must gate");
        assert_eq!(pending.result, ActionResult::PendingApproval);
        assert_eq!(pending.risk.level, RiskLevel::High);
        assert!(pending.risk.requires_approval);

        eng.approve("screen@10,20:click").unwrap();
        assert!(
            eng.gate_raw_input(
                "screen@10,20:click",
                SemanticAction::Click,
                Some("click 10,20".to_string()),
                Some("raw screen click"),
                Engine::raw_input_risk(Vec::new()),
            )
            .is_none(),
            "approved pending raw target should pass the gate once"
        );

        eng.approvals.remove("screen@10,20:click");
        let err = eng.approve("screen@10,20:other").unwrap_err();
        assert!(matches!(err, VisualOpsError::ElementNotFound(_)));
    }

    #[test]
    fn raw_user_active_failure_preserves_approval_for_retry() {
        let (mut eng, _) = engine_with_counter();
        let target = "keyboard@scroll:down:2";

        eng.pending_gate_ids.insert(target.to_string());
        eng.approve(target).unwrap();
        assert!(eng.approvals.contains(target));

        let outcome = Err(VisualOpsError::Execution(
            "user-active guard blocked background key: last keyboard/mouse input was 244 ms ago (< 300 ms)".into(),
        ));
        let err = eng
            .audit_raw_input(
                target.to_string(),
                SemanticAction::Type,
                Some("scroll down x2".to_string()),
                Some("background web scroll"),
                Engine::raw_input_risk(Vec::new()),
                outcome,
            )
            .unwrap_err();
        assert!(err.to_string().contains("user-active guard blocked"));
        assert!(
            eng.approvals.contains(target),
            "user-active guard should not consume an already approved raw action"
        );
    }

    // --- Audit #3: composite drag risk (max of source / drop target) ---------

    /// A purpose-built fixture for the composite-drag gate: the bundled Notes
    /// fixture has no node that is *both* draggable (Row/Cell) and high-risk with a
    /// bbox (its high-risk items are bbox-less menu items), so we mint a tiny tree
    /// with a harmless draggable row and a high-risk drop target that has a bbox.
    fn composite_drag_engine() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
        fn raw(
            ax_role: &str,
            label: Option<&str>,
            frame: Option<Bbox>,
            ax_actions: &[&str],
            children: Vec<dunst_core::RawAxNode>,
        ) -> dunst_core::RawAxNode {
            dunst_core::RawAxNode {
                ax_role: ax_role.into(),
                label: label.map(str::to_owned),
                help: None,
                value: None,
                ax_identifier: None,
                ax_actions: ax_actions.iter().map(|s| s.to_string()).collect(),
                frame,
                enabled: true,
                focused: false,
                children,
            }
        }
        let bb = |x: f64| {
            Some(Bbox {
                x,
                y: 100.0,
                w: 50.0,
                h: 20.0,
            })
        };
        // Row under a Table → draggable (the Table is an ancestor drop container).
        let row = raw("AXRow", Some("note-a"), bb(10.0), &["press"], vec![]);
        let table = raw("AXTable", None, bb(10.0), &[], vec![row]);
        // High-risk drop target WITH a bbox (so drag_element can compute a drop).
        let danger = raw("AXButton", Some("Supprimer"), bb(200.0), &["press"], vec![]);
        let window = raw(
            "AXWindow",
            Some("W"),
            Some(Bbox {
                x: 0.0,
                y: 0.0,
                w: 400.0,
                h: 400.0,
            }),
            &[],
            vec![table, danger],
        );

        let calls = Arc::new(Mutex::new(Vec::new()));
        let perceptor = Box::new(MockPerceptor::new(
            vec![window],
            WindowRef {
                pid: 1,
                window_id: 1,
                app_name: "T".into(),
                title: "T".into(),
            },
        ));
        let exec = Box::new(RecordingExecutor(calls.clone()));
        let eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1,
                window_id: 1,
            },
        )
        .unwrap();
        (eng, calls)
    }

    #[test]
    fn drag_onto_high_risk_target_is_gated_then_approvable() {
        let (mut eng, calls) = composite_drag_engine();
        let source = id_for(&eng, "note-a"); // low-risk draggable row
        let target = id_for(&eng, "Supprimer"); // high-risk drop target, has bbox

        // Precondition: source is harmless, target is the dangerous one.
        assert!(
            !eng.affordance_graph().affordances[&source]
                .risk
                .requires_approval
        );
        assert!(
            eng.affordance_graph().affordances[&target]
                .risk
                .requires_approval
        );

        // The gate fires on the TARGET's risk even though the source is low.
        let gated = eng
            .drag_element(&source, &target, Some("dangerous drop"))
            .unwrap();
        assert_eq!(
            gated.result,
            ActionResult::PendingApproval,
            "high-risk drop target must gate"
        );
        assert_eq!(
            gated.risk.level,
            RiskLevel::High,
            "effective risk is max(source, target)"
        );
        assert!(gated.risk.requires_approval);
        assert!(
            gated
                .risk
                .reasons
                .iter()
                .any(|r| r.contains("drop target") && r.to_lowercase().contains("supprimer")),
            "audit reason attributes the risk to the drop target: {:?}",
            gated.risk.reasons
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "gated drag never reaches the executor"
        );

        // Approving the dangerous target (its own risk is high → approve accepts it)
        // clears the composite gate for exactly one drag.
        eng.approve(&target).unwrap();
        let ok = eng
            .drag_element(&source, &target, Some("approved drop"))
            .unwrap();
        assert_eq!(ok.result, ActionResult::Success);
        let recorded = calls.lock().unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "executor ran exactly once, on the source"
        );
        assert_eq!(recorded[0].0, source);
        assert_eq!(recorded[0].1, SemanticAction::Drag);
    }

    // --- Audit #13: a destructive *typed value* gates a low-risk field --------

    #[test]
    fn destructive_typed_text_gates_low_risk_field_and_is_approvable() {
        let (mut eng, calls) = engine_with_counter();
        let field = id_for(&eng, "Corps de la note"); // low-risk, typeable text area
        assert!(
            !eng.affordance_graph().affordances[&field]
                .risk
                .requires_approval,
            "the field itself is low-risk"
        );

        // Out of context, a low-risk field is NOT approvable (audit #2 still holds).
        assert!(
            eng.approve(&field).is_err(),
            "low-risk field not approvable out of context"
        );

        // A destructive payload raises the gate even though the field is harmless.
        let gated = eng
            .type_into(&field, "supprimer tout", Some("danger"))
            .unwrap();
        assert_eq!(
            gated.result,
            ActionResult::PendingApproval,
            "destructive text gates the field"
        );
        assert_eq!(
            gated.risk.level,
            RiskLevel::High,
            "effective risk = max(field, text)"
        );
        assert!(
            gated
                .risk
                .reasons
                .iter()
                .any(|r| r.contains("typed text") && r.to_lowercase().contains("supprimer")),
            "audit attributes the risk to the typed text: {:?}",
            gated.risk.reasons
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "gated type never reaches the executor"
        );

        // The field is now the subject of a pending gate → approvable; type proceeds.
        eng.approve(&field).unwrap();
        let ok = eng
            .type_into(&field, "supprimer tout", Some("approved"))
            .unwrap();
        assert_eq!(
            ok.result,
            ActionResult::Failed,
            "mock executor records the type attempt, but the unchanged fixture must fail verification"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // One-shot: a second destructive type gates again (grant consumed + refresh).
        let regated = eng.type_into(&field, "supprimer tout", None).unwrap();
        assert_eq!(regated.result, ActionResult::PendingApproval);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Benign text into the same field is never gated.
        let benign = eng.type_into(&field, "bonjour", None).unwrap();
        assert_eq!(benign.result, ActionResult::Failed);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Regression: "provider" contains the French destructive keyword
        // "vider", but it is not a destructive word on token boundaries.
        let provider = eng
            .type_into(&field, "failover multi-provider", None)
            .unwrap();
        assert_eq!(provider.result, ActionResult::Failed);
        assert_eq!(provider.risk.level, RiskLevel::Low);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    // --- effective_risk in isolation (C2 refactor) --------------------------

    #[test]
    fn effective_risk_folds_drag_target_and_typed_text() {
        let (eng, _) = engine_with_counter();
        let low = RiskAssessment::low();
        let high = RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons: vec!["matched keyword: supprimer".into()],
        };

        // Low source dragged onto a high-risk target → effective High, target gated.
        let co = CoTarget {
            id: "tgt".into(),
            risk: high.clone(),
        };
        let (eff, gated) = eng.effective_risk("src", SemanticAction::Drag, None, &low, Some(&co));
        assert_eq!(eff.level, RiskLevel::High);
        assert!(eff.requires_approval);
        assert_eq!(gated, vec!["tgt".to_string()]);
        assert!(eff.reasons.iter().any(|r| r.contains("drop target")));

        // Destructive text into a low-risk field → effective High, field gated.
        let (eff2, gated2) = eng.effective_risk(
            "field",
            SemanticAction::Type,
            Some("supprimer tout"),
            &low,
            None,
        );
        assert_eq!(eff2.level, RiskLevel::High);
        assert!(eff2.requires_approval);
        assert_eq!(gated2, vec!["field".to_string()]);
        assert!(eff2.reasons.iter().any(|r| r.contains("typed text")));

        // Benign: low source, no co-target, benign text → Low, no gate.
        let (eff3, gated3) = eng.effective_risk("x", SemanticAction::Click, None, &low, None);
        assert!(!eff3.requires_approval);
        assert_eq!(eff3.level, RiskLevel::Low);
        assert!(gated3.is_empty());
    }

    #[test]
    fn unavailable_action_is_an_error() {
        let (mut eng, calls) = engine_with_counter();
        // A button has no Type affordance.
        let id = id_for(&eng, "Nouvelle note");
        let err = eng.type_into(&id, "x", None).unwrap_err();
        assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn every_attempt_is_audited() {
        let (mut eng, _c) = engine_with_counter();
        let _ = eng.click_element(&id_for(&eng, "Supprimer"), None); // gated
        let _ = eng.click_element(&id_for(&eng, "Nouvelle note"), None); // ok
        assert_eq!(eng.trace().len(), 2);
    }

    #[test]
    fn drag_records_target_bbox_centre() {
        let (mut eng, calls) = engine_with_recorder();
        let source = non_gated_drag_source(&eng);
        let target = id_for(&eng, "Nouvelle note");

        // Expected drop point = centre of the *target* node's bbox, formatted
        // exactly as the engine formats it.
        let bbox = eng.scene_graph().get(&target).unwrap().bbox.unwrap();
        let expected = format!("{},{}", bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);

        let entry = eng.drag_element(&source, &target, Some("reorder")).unwrap();

        // The executor saw exactly (source, Drag, Some("x,y")).
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            (source.clone(), SemanticAction::Drag, Some(expected))
        );

        // The audit entry describes the drag on the source and is in the trace.
        assert_eq!(entry.action, SemanticAction::Drag);
        assert_eq!(entry.target_id, source);
        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(eng.trace().len(), 1);
        assert_eq!(eng.trace()[0].action, SemanticAction::Drag);
    }

    #[test]
    fn drag_unknown_target_is_an_error() {
        let (mut eng, calls) = engine_with_recorder();
        let source = non_gated_drag_source(&eng);

        let err = eng
            .drag_element(&source, "no_such_target", None)
            .unwrap_err();
        assert!(matches!(err, VisualOpsError::ElementNotFound(_)));

        // No executor call, no audit entry: the failure is structural, pre-act.
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(eng.trace().len(), 0);
    }

    #[test]
    fn drag_source_without_affordance_is_unavailable() {
        let (mut eng, calls) = engine_with_recorder();
        // A toolbar button exposes Click, never Drag; the target has a bbox.
        let source = id_for(&eng, "Nouvelle note");
        let target = id_for(&eng, "Nouvelle note");

        let err = eng.drag_element(&source, &target, None).unwrap_err();
        assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(eng.trace().len(), 0);
    }

    // --- WP-J / J1: get_scene_graph projection ------------------------------

    #[test]
    fn compact_view_omits_heavy_fields_and_keeps_n_children() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Compact, false);
        let id = id_for(&eng, "Nouvelle note");
        let node = v["nodes"].get(id.as_str()).expect("compact node present");

        // Heavy/derivable AX fields are dropped.
        for dropped in [
            "ax_role",
            "help",
            "ax_actions",
            "ax_identifier",
            "last_seen_ms",
            "children",
            "confidence",
            "source",
        ] {
            assert!(
                node.get(dropped).is_none(),
                "compact node must drop {dropped}"
            );
        }
        // Kept fields, with children collapsed to a count.
        assert!(node.get("n_children").is_some(), "n_children kept");
        assert!(node.get("bbox").is_some(), "bbox kept");
        assert_eq!(node["role"], json!("button"));
    }

    #[test]
    fn compact_view_is_materially_smaller_than_full() {
        let (eng, _) = engine_with_counter();
        let full = eng.scene_graph_view(SceneView::Full, false);
        let compact = eng.scene_graph_view(SceneView::Compact, false);
        let full_len = serde_json::to_string(&full).unwrap().len();
        let compact_len = serde_json::to_string(&compact).unwrap().len();
        // Visible with `cargo test -- --nocapture`; the real before/after note.
        eprintln!(
            "get_scene_graph fixture size — full: {full_len} B, compact: {compact_len} B (×{:.1} lighter)",
            full_len as f64 / compact_len.max(1) as f64
        );
        assert!(
            compact_len < full_len,
            "compact ({compact_len}) must be smaller than full ({full_len})"
        );
    }

    #[test]
    fn full_view_is_byte_identical_to_raw_scene_graph() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Full, false);
        let raw = serde_json::to_value(eng.scene_graph()).unwrap();
        assert_eq!(v, raw, "full view is the unchanged escape hatch");
    }

    #[test]
    fn summary_view_has_counts_and_roots_but_no_nodes() {
        let (eng, _) = engine_with_counter();
        let v = eng.scene_graph_view(SceneView::Summary, false);
        assert!(v.get("nodes").is_none(), "summary carries no per-node list");
        let n_nodes = v["n_nodes"].as_u64().expect("n_nodes");
        let n_actionable = v["n_actionable"].as_u64().expect("n_actionable");
        assert!(n_nodes >= 1);
        assert!(v["roots"].is_array());
        assert!(v["counts_by_role"].is_object());
        assert!(v["window"].is_object());
        assert!(n_actionable <= n_nodes, "actionable is a subset");
        assert!(
            n_actionable >= 1,
            "at least the toolbar button is actionable"
        );
    }

    #[test]
    fn actionable_only_drops_latent_menu_items() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer"); // latent AXMenuItem (no bbox)
        let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen toolbar button
        let v = eng.scene_graph_view(SceneView::Compact, true);
        assert!(
            v["nodes"].get(supprimer.as_str()).is_none(),
            "latent node dropped by actionable_only"
        );
        assert!(
            v["nodes"].get(nouvelle.as_str()).is_some(),
            "on-screen node kept"
        );
    }

    // --- WP-J / J2: latent affordance filtering -----------------------------

    #[test]
    fn query_affordances_excludes_latent_by_default_but_include_latent_keeps_them() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer"); // latent menu item exposing Click
        let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen button

        let default = eng.query_affordances(SemanticAction::Click);
        assert!(
            !default.contains(&supprimer),
            "latent menu item filtered from default listing"
        );
        assert!(default.contains(&nouvelle), "on-screen button still listed");

        let all = eng.query_affordances_filtered(SemanticAction::Click, true);
        assert!(
            all.contains(&supprimer),
            "include_latent surfaces the latent item"
        );
        assert!(
            all.len() > default.len(),
            "include_latent is a strict superset here"
        );
    }

    #[test]
    fn get_affordances_view_filters_latent_but_keeps_it_under_include_latent() {
        let (eng, _) = engine_with_counter();
        let supprimer = id_for(&eng, "Supprimer");
        let filtered = eng.affordances_view(false);
        assert!(
            filtered["affordances"].get(supprimer.as_str()).is_none(),
            "latent omitted by default"
        );
        let all = eng.affordances_view(true);
        assert!(
            all["affordances"].get(supprimer.as_str()).is_some(),
            "include_latent keeps it"
        );
    }

    #[test]
    fn find_element_and_gating_still_reach_latent_nodes() {
        // CRITICAL (WP-J): filtering the *listing* must NOT hide latent nodes from
        // find_element, nor stop the risk gate from acting on them by id.
        let (mut eng, calls) = engine_with_counter();
        assert!(
            !eng.find_element("Supprimer").is_empty(),
            "find_element still locates the latent item"
        );

        let supprimer = id_for(&eng, "Supprimer");
        // click_element by id reaches the gate (PendingApproval), not ActionUnavailable,
        // and the executor never runs.
        let e = eng.click_element(&supprimer, Some("delete")).unwrap();
        assert_eq!(e.result, ActionResult::PendingApproval);
        assert!(e.risk.requires_approval);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "gated action never reaches the executor"
        );
    }

    #[test]
    fn visible_only_find_drops_latent_noise_but_default_keeps_it() {
        let (eng, _) = engine_with_counter();
        assert!(
            !eng.find_element("Supprimer").is_empty(),
            "default find still reaches latent nodes"
        );
        assert!(
            eng.find_element_filtered("Supprimer", true).is_empty(),
            "visible_only drops collapsed/off-window matches"
        );
        assert!(
            !eng.find_element_filtered("Nouvelle note", true).is_empty(),
            "visible_only keeps on-screen matches"
        );
    }

    #[test]
    fn find_element_matches_accents_insensitively() {
        let window = raw_node(
            "AXWindow",
            Some("Profil"),
            None,
            test_bbox(0.0, 0.0, 700.0, 500.0),
            &[],
            vec![raw_node(
                "AXButton",
                Some("Éditer les expertises"),
                None,
                test_bbox(260.0, 80.0, 140.0, 36.0),
                &["press"],
                vec![],
            )],
        );
        let (eng, _) = engine_from_roots(vec![window], "Browser", "Profil");

        assert!(
            !eng.find_element_filtered("éditer", true).is_empty(),
            "accented query should match"
        );
        assert!(
            !eng.find_element_filtered("editer", true).is_empty(),
            "unaccented query should match accented UI text"
        );
    }

    #[test]
    fn find_element_promotes_editable_control_associated_with_matching_label() {
        let window = raw_node(
            "AXWindow",
            Some("Experience"),
            None,
            test_bbox(0.0, 0.0, 700.0, 500.0),
            &[],
            vec![
                raw_node(
                    "AXStaticText",
                    Some("Description"),
                    None,
                    test_bbox(40.0, 80.0, 100.0, 20.0),
                    &[],
                    vec![],
                ),
                raw_node(
                    "AXTextArea",
                    None,
                    Some("Existing body"),
                    test_bbox(40.0, 108.0, 500.0, 160.0),
                    &["press"],
                    vec![],
                ),
            ],
        );
        let (eng, _) = engine_from_roots(vec![window], "Firefox", "Experience");

        let matches = eng.find_element_filtered("description", true);
        assert_eq!(
            matches.first().map(|node| node.role),
            Some(Role::TextArea),
            "nearest editable field should rank before the static label: {matches:?}"
        );
        assert!(
            matches.iter().any(|node| node.role == Role::StaticText),
            "the matching label remains present for orientation"
        );
    }

    #[test]
    fn page_state_is_lightweight_orientation_snapshot() {
        let (eng, _) = engine_with_counter();
        let state = eng.page_state(8);
        assert_eq!(state.target.pid, 1363);
        assert_eq!(state.title, "Notes – Aucune note");
        assert!(state.key_elements.len() <= 8);
        assert!(
            state.key_elements.iter().all(|e| e.role != "menu_bar"),
            "page_state should not spend key-element budget on menu bar chrome"
        );
        assert!(
            state
                .key_elements
                .iter()
                .any(|e| e.label.as_deref() == Some("Nouvelle note")),
            "page_state should include key visible actions"
        );
    }

    #[test]
    fn page_state_does_not_use_window_root_as_key_element() {
        let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "jarvis github - Recherche Google",
            "ax_actions": ["raise"],
            "frame": { "x": 0, "y": 32, "w": 2560, "h": 1326 }
          }
        ]"#;
        let eng = engine_from_json(json, "Zen", "jarvis github - Recherche Google");
        let state = eng.page_state(10);
        assert!(
            state.key_elements.is_empty(),
            "window root should not consume page_state key-element budget: {:?}",
            state.key_elements
        );
    }

    #[test]
    fn page_state_drops_unlabeled_full_size_unknown_containers() {
        let window = raw_node(
            "AXWindow",
            Some("Collective"),
            None,
            test_bbox(2560.0, 440.0, 1728.0, 1000.0),
            &[],
            vec![
                raw_node(
                    "AXUnknown",
                    None,
                    None,
                    test_bbox(2560.0, 440.0, 1728.0, 1000.0),
                    &["press"],
                    vec![],
                ),
                raw_node(
                    "AXButton",
                    Some("Modifier"),
                    None,
                    test_bbox(3627.0, 1306.0, 81.0, 32.0),
                    &["press"],
                    vec![],
                ),
            ],
        );
        let (eng, _) = engine_from_roots(vec![window], "Firefox", "Collective");

        let state = eng.page_state(2);
        assert!(
            state
                .key_elements
                .iter()
                .all(|element| element.role != "unknown"),
            "unlabeled full-size unknown containers should be suppressed: {:?}",
            state.key_elements
        );
        assert!(
            state
                .key_elements
                .iter()
                .any(|element| element.label.as_deref() == Some("Modifier")),
            "real action should stay visible: {:?}",
            state.key_elements
        );
    }

    #[test]
    fn verify_state_supports_focused_field() {
        let mut description = raw_node(
            "AXTextArea",
            Some("Description"),
            Some("Texte"),
            test_bbox(40.0, 80.0, 240.0, 120.0),
            &["press"],
            vec![],
        );
        description.focused = true;
        let window = raw_node(
            "AXWindow",
            Some("Form"),
            None,
            test_bbox(0.0, 0.0, 500.0, 400.0),
            &[],
            vec![description],
        );
        let (eng, _) = engine_from_roots(vec![window], "Browser", "Form");
        let field = id_for(&eng, "Description");

        assert!(eng.verify_state(&field, "focused", "true").unwrap());
        assert!(!eng.verify_state(&field, "focused", "false").unwrap());
    }

    #[test]
    fn raise_element_executes_raise_affordance() {
        let window = raw_node(
            "AXWindow",
            Some("Collective"),
            None,
            test_bbox(0.0, 0.0, 500.0, 400.0),
            &["raise"],
            vec![],
        );
        let (mut eng, calls) = engine_from_roots(vec![window], "Firefox", "Collective");
        let id = id_for(&eng, "Collective");

        let entry = eng
            .raise_element(&id, Some("bring target window forward"))
            .unwrap();
        assert_eq!(entry.result, ActionResult::Success);
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, id);
        assert_eq!(recorded[0].1, SemanticAction::Raise);
    }

    #[test]
    fn click_element_presses_ax_actionable_button_outside_viewport() {
        let window = raw_node(
            "AXWindow",
            Some("Long modal"),
            None,
            test_bbox(0.0, 0.0, 500.0, 800.0),
            &[],
            vec![raw_node(
                "AXButton",
                Some("Sauvegarder"),
                None,
                test_bbox(40.0, 1469.0, 140.0, 36.0),
                &["press"],
                vec![],
            )],
        );
        let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Long modal");
        assert!(
            eng.find_element_filtered("Sauvegarder", true).is_empty(),
            "visible_only should still hide off-viewport controls"
        );

        let save = id_for(&eng, "Sauvegarder");
        let entry = eng.click_element(&save, Some("save long modal")).unwrap();
        assert_eq!(entry.result, ActionResult::Success);
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(calls.lock().unwrap()[0].0, save);
    }

    #[test]
    fn page_state_filters_repeated_remove_buttons_from_key_budget() {
        let mut children = Vec::new();
        for idx in 0..100 {
            children.push(raw_node(
                "AXButton",
                Some("Remove"),
                None,
                test_bbox(20.0, 60.0 + idx as f64 * 10.0, 22.0, 8.0),
                &["press"],
                vec![],
            ));
        }
        children.push(raw_node(
            "AXButton",
            Some("Sauvegarder"),
            None,
            test_bbox(260.0, 80.0, 140.0, 36.0),
            &["press"],
            vec![],
        ));
        let window = raw_node(
            "AXWindow",
            Some("Expertises"),
            None,
            test_bbox(0.0, 0.0, 700.0, 1200.0),
            &[],
            children,
        );
        let (eng, _) = engine_from_roots(vec![window], "Browser", "Expertises");

        let state = eng.page_state(8);
        assert!(
            state
                .key_elements
                .iter()
                .all(|e| e.label.as_deref() != Some("Remove")),
            "repeated destructive buttons should not consume page_state budget: {:?}",
            state.key_elements
        );
        assert!(
            state
                .key_elements
                .iter()
                .any(|e| e.label.as_deref() == Some("Sauvegarder")),
            "useful controls must remain visible in the compact summary"
        );
    }

    #[test]
    fn page_state_drops_tiny_technical_controls_from_key_budget() {
        let window = raw_node(
            "AXWindow",
            Some("Expertises"),
            None,
            test_bbox(0.0, 0.0, 700.0, 900.0),
            &[],
            vec![
                raw_node(
                    "AXCheckBox",
                    Some("Rust"),
                    None,
                    test_bbox(120.0, 180.0, 1.0, 1.0),
                    &["press"],
                    vec![],
                ),
                raw_node(
                    "AXButton",
                    Some("Ajouter"),
                    None,
                    test_bbox(260.0, 180.0, 120.0, 36.0),
                    &["press"],
                    vec![],
                ),
            ],
        );
        let (eng, _) = engine_from_roots(vec![window], "Firefox", "Expertises");

        let state = eng.page_state(8);
        assert!(
            state.key_elements.iter().all(|e| e.id != "chk_rust"),
            "1x1 technical checkbox should not consume page_state budget: {:?}",
            state.key_elements
        );
        assert!(
            state
                .key_elements
                .iter()
                .any(|e| e.label.as_deref() == Some("Ajouter")),
            "real visible action should remain present: {:?}",
            state.key_elements
        );
    }

    #[test]
    fn pick_option_resolves_static_text_to_clickable_parent() {
        let window = raw_node(
            "AXWindow",
            Some("Options"),
            None,
            test_bbox(0.0, 0.0, 600.0, 500.0),
            &[],
            vec![raw_node(
                "AXGroup",
                None,
                None,
                test_bbox(40.0, 100.0, 300.0, 36.0),
                &["press"],
                vec![raw_node(
                    "AXStaticText",
                    Some("Disponibilité Collective"),
                    None,
                    test_bbox(56.0, 108.0, 220.0, 18.0),
                    &[],
                    vec![],
                )],
            )],
        );
        let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Options");
        let text = id_for(&eng, "Disponibilité Collective");

        let click = eng
            .click_element(&text, Some("select option text"))
            .unwrap();
        assert_eq!(click.result, ActionResult::Success);
        let clicked_id = calls.lock().unwrap()[0].0.clone();
        assert_ne!(clicked_id, text, "static text should resolve to its parent");
        assert!(clicked_id.starts_with("grp_"));

        let picked = eng
            .pick_option("Disponibilité Collective", true, Some("select option"))
            .unwrap();
        assert_eq!(picked.audit.result, ActionResult::Success);
        assert_eq!(picked.matched_id, text);
        assert_eq!(picked.action_id, clicked_id);
    }

    #[test]
    fn pick_option_reads_french_selected_state_after_normalization() {
        let window = raw_node(
            "AXWindow",
            Some("Options"),
            None,
            test_bbox(0.0, 0.0, 600.0, 500.0),
            &[],
            vec![raw_node(
                "AXGroup",
                None,
                Some("Sélectionné"),
                test_bbox(40.0, 100.0, 300.0, 36.0),
                &["press"],
                vec![raw_node(
                    "AXStaticText",
                    Some("Disponibilité Collective"),
                    None,
                    test_bbox(56.0, 108.0, 220.0, 18.0),
                    &[],
                    vec![],
                )],
            )],
        );
        let (mut eng, _) = engine_from_roots(vec![window], "Browser", "Options");

        let picked = eng
            .pick_option("Disponibilité Collective", true, Some("select option"))
            .unwrap();
        assert_eq!(picked.selected_before, Some(true));
        assert_eq!(picked.selected_after, Some(true));
    }

    #[test]
    fn parent_resolution_does_not_bypass_high_risk_static_text() {
        let window = raw_node(
            "AXWindow",
            Some("Options"),
            None,
            test_bbox(0.0, 0.0, 600.0, 500.0),
            &[],
            vec![raw_node(
                "AXGroup",
                None,
                None,
                test_bbox(40.0, 100.0, 300.0, 36.0),
                &["press"],
                vec![raw_node(
                    "AXStaticText",
                    Some("Remove expertise"),
                    None,
                    test_bbox(56.0, 108.0, 220.0, 18.0),
                    &[],
                    vec![],
                )],
            )],
        );
        let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Options");
        let text = id_for(&eng, "Remove expertise");

        let err = eng
            .click_element(&text, Some("remove via text"))
            .unwrap_err();
        assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
        assert!(
            calls.lock().unwrap().is_empty(),
            "high-risk static text must not execute through an unlabeled parent"
        );
    }

    #[test]
    fn text_snapshot_filters_browser_chrome_but_keeps_page_text() {
        let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "Claude",
            "frame": { "x": 100, "y": 100, "w": 1000, "h": 800 },
            "children": [
              {
                "ax_role": "AXRadioButton",
                "label": "Premortem tab",
                "ax_actions": ["press"],
                "frame": { "x": 120, "y": 112, "w": 220, "h": 36 },
                "children": [
                  {
                    "ax_role": "AXStaticText",
                    "label": "Premortem tab",
                    "frame": { "x": 150, "y": 122, "w": 120, "h": 16 }
                  }
                ]
              },
              {
                "ax_role": "AXButton",
                "label": "Actualiser",
                "ax_actions": ["press"],
                "frame": { "x": 130, "y": 175, "w": 36, "h": 36 }
              },
              {
                "ax_role": "AXGroup",
                "frame": { "x": 120, "y": 230, "w": 850, "h": 620 },
                "children": [
                  {
                    "ax_role": "AXStaticText",
                    "label": "Final verdict: NO-GO until warm-up is done",
                    "frame": { "x": 150, "y": 260, "w": 420, "h": 22 }
                  }
                ]
              }
            ]
          }
        ]"#;
        let eng = engine_from_json(json, "Firefox", "Premortem - Claude");

        let snippets = eng.text_snapshot(None, true, 20);
        assert!(snippets.iter().any(|s| s.text.contains("Final verdict")));
        assert!(snippets.iter().all(|s| s.text != "Premortem tab"));

        let state = eng.page_state(20);
        assert!(state
            .visible_text
            .iter()
            .any(|s| s.contains("Final verdict")));
        assert!(state.visible_text.iter().all(|s| s != "Premortem tab"));
        assert!(state
            .key_elements
            .iter()
            .all(|e| e.label.as_deref() != Some("Actualiser")));
    }

    #[test]
    fn text_snapshot_filters_web_app_navigation_chrome() {
        let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "Collective",
            "frame": { "x": 2560, "y": 440, "w": 1728, "h": 1000 },
            "children": [
              {
                "ax_role": "AXStaticText",
                "label": "Accueil",
                "frame": { "x": 2612, "y": 536, "w": 49, "h": 18 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "www.collective.work/profile/clement-liard",
                "frame": { "x": 2888, "y": 557, "w": 237, "h": 15 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "Connect",
                "frame": { "x": 2576, "y": 577, "w": 49, "h": 15 }
              },
              {
                "ax_role": "AXButton",
                "label": "copy",
                "ax_actions": ["press"],
                "frame": { "x": 3133, "y": 556, "w": 16, "h": 16 }
              },
              {
                "ax_role": "AXButton",
                "label": "Open Intercom Messenger",
                "ax_actions": ["press"],
                "frame": { "x": 4196, "y": 1350, "w": 48, "h": 48 }
              },
              {
                "ax_role": "AXButton",
                "label": "Modifier les informations principales",
                "ax_actions": ["press"],
                "frame": { "x": 3181, "y": 669, "w": 32, "h": 32 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "Freelance Architecte DevSecOps & IA souveraine",
                "frame": { "x": 3343, "y": 721, "w": 500, "h": 22 }
              }
            ]
          }
        ]"#;
        let eng = engine_from_json(json, "Firefox", "Collective");

        let snippets = eng.text_snapshot(None, true, 20);
        let texts: Vec<&str> = snippets.iter().map(|s| s.text.as_str()).collect();
        assert!(texts
            .iter()
            .any(|text| text.contains("Freelance Architecte DevSecOps")));
        assert!(!texts.contains(&"Accueil"));
        assert!(!texts.contains(&"Connect"));
        assert!(!texts
            .iter()
            .any(|text| text.contains("collective.work/profile")));

        let state = eng.page_state(20);
        assert!(state
            .visible_text
            .iter()
            .any(|text| text.contains("Freelance Architecte DevSecOps")));
        assert!(state.visible_text.iter().all(|text| text != "Accueil"));
        assert!(state
            .key_elements
            .iter()
            .all(|element| element.label.as_deref() != Some("copy")));
        assert!(state
            .key_elements
            .iter()
            .all(|element| { element.label.as_deref() != Some("Open Intercom Messenger") }));
        assert!(state.key_elements.iter().any(|element| {
            element.label.as_deref() == Some("Modifier les informations principales")
        }));
    }

    #[test]
    fn text_snapshot_query_matches_whole_words_not_substrings_inside_words() {
        let window = raw_node(
            "AXWindow",
            Some("Expertises"),
            None,
            test_bbox(0.0, 0.0, 800.0, 600.0),
            &[],
            vec![
                raw_node(
                    "AXStaticText",
                    Some("Zero Trust"),
                    None,
                    test_bbox(20.0, 180.0, 120.0, 20.0),
                    &[],
                    vec![],
                ),
                raw_node(
                    "AXStaticText",
                    Some("Rust"),
                    None,
                    test_bbox(20.0, 120.0, 80.0, 20.0),
                    &[],
                    vec![],
                ),
            ],
        );
        let (eng, _) = engine_from_roots(vec![window], "Firefox", "Expertises");

        let rust = eng.text_snapshot(Some("Rust"), false, 10);
        assert_eq!(rust.len(), 1);
        assert_eq!(rust[0].text, "Rust");

        let trust = eng.text_snapshot(Some("Trust"), false, 10);
        assert_eq!(trust.len(), 1);
        assert_eq!(trust[0].text, "Zero Trust");
    }

    #[test]
    fn user_active_guard_retry_runs_once_before_returning() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_in_closure = attempts.clone();
        let result = retry_user_active_guard_after(Duration::from_millis(0), || {
            if attempts_in_closure.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(VisualOpsError::Execution(
                    "user-active guard blocked hover_at: last keyboard/mouse input was 1 ms ago (< 300 ms)".into(),
                ))
            } else {
                Ok("ok")
            }
        })
        .unwrap();

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn internal_hover_lead_point_is_clamped_to_target_window() {
        let (eng, _) = engine_with_counter();
        let window = eng.current_window_bounds();
        let (x, y) = eng.clamp_point_to_target_window(window.x - 8.0, window.y - 8.0);
        assert!(point_in_bbox((x, y), window));
        assert_eq!(x, window.x);
        assert_eq!(y, window.y);
    }

    #[test]
    fn text_snapshot_returns_visible_ax_text_without_full_graph() {
        let (eng, _) = engine_with_counter();
        let snippets = eng.text_snapshot(Some("Corps de la note"), true, 10);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].role, "text_area");
        assert_eq!(snippets[0].text, "Corps de la note");
        assert!(snippets[0].visible);
    }

    #[test]
    fn terminal_ocr_fallback_reads_ax_text_area_value() {
        let window = raw_node(
            "AXWindow",
            Some("iTerm2"),
            None,
            test_bbox(0.0, 0.0, 800.0, 600.0),
            &[],
            vec![raw_node(
                "AXTextArea",
                None,
                Some("cargo test\nfinished ok"),
                test_bbox(10.0, 10.0, 780.0, 560.0),
                &[],
                vec![],
            )],
        );
        let (eng, _) = engine_from_roots(vec![window], "iTerm2", "shell");

        let hits = eng.ax_terminal_text_hits(None);
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec!["cargo test", "finished ok"]
        );
    }

    #[test]
    fn refresh_if_stale_reuses_recent_graph() {
        let (mut eng, _) = engine_with_counter();
        let refreshed = eng.refresh_if_stale().unwrap();
        assert!(
            !refreshed,
            "newly-created engine should still be inside read TTL"
        );
    }

    #[test]
    fn visual_signature_comparison_counts_threshold_crossings() {
        let previous = [10, 20, 30, 40];
        let current = [10, 25, 60, 39];
        let (changed, max_delta, mean_delta) = compare_signatures(&previous, &current, 4);
        assert_eq!(changed, 2);
        assert_eq!(max_delta, 30);
        assert!((mean_delta - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn window_view_adds_window_geometry_without_full_graph() {
        let (eng, _) = engine_with_counter();
        let view = eng.window_view(4);
        assert_eq!(view.target.pid, 1363);
        assert_eq!(view.title, "Notes – Aucune note");
        assert!(view.window.w > 0.0);
        assert!(view.window.h > 0.0);
        assert!(view.key_elements.len() <= 4);
        assert!(view.visible_text.len() <= 4);
    }

    #[test]
    fn desktop_view_marks_missing_display_topology_as_degraded() {
        let view = desktop_view_from_windows(
            Vec::new(),
            Vec::new(),
            Some("no valid display topology".into()),
        );
        assert!(view.degraded);
        assert_eq!(view.reason.as_deref(), Some("no valid display topology"));
        assert!(view.displays.is_empty());
        assert!(view.windows.is_empty());
    }

    #[test]
    fn desktop_view_renumbers_z_order_after_filtering() {
        let front = DesktopWindow {
            window_id: 1,
            pid: 10,
            app: "Finder".into(),
            title: "front".into(),
            bounds: Bbox {
                x: 0.0,
                y: 0.0,
                w: 500.0,
                h: 500.0,
            },
            on_screen: true,
            z_order: 7,
            is_frontmost: false,
            display: None,
            covered_by: Vec::new(),
            covers: Vec::new(),
        };
        let back = DesktopWindow {
            window_id: 2,
            pid: 20,
            app: "Obsidian".into(),
            title: "back".into(),
            bounds: Bbox {
                x: 50.0,
                y: 50.0,
                w: 500.0,
                h: 500.0,
            },
            on_screen: true,
            z_order: 9,
            is_frontmost: false,
            display: None,
            covered_by: Vec::new(),
            covers: Vec::new(),
        };

        let view = desktop_view_from_windows(Vec::new(), vec![back, front], None);
        assert_eq!(view.frontmost.as_ref().unwrap().window_id, 1);
        assert_eq!(view.frontmost.as_ref().unwrap().z_order, 0);
        assert!(view.frontmost.as_ref().unwrap().is_frontmost);
        assert_eq!(view.windows[1].z_order, 1);
        assert_eq!(view.windows[0].covers, vec![2]);
        assert_eq!(view.windows[1].covered_by, vec![1]);
    }

    #[test]
    fn raw_point_risk_flags_possible_backdrop_clicks() {
        let (eng, _) = engine_with_counter();
        let risk = eng.raw_point_risk(10_000.0, 10_000.0);
        assert_eq!(risk.level, RiskLevel::High);
        assert!(
            risk.reasons
                .iter()
                .any(|r| r.contains("outside the target window")),
            "risk reasons should flag off-window raw points: {:?}",
            risk.reasons
        );
    }

    #[test]
    fn raw_point_guard_rejects_off_target_points() {
        let old = std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW").ok();
        std::env::remove_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW");
        let (eng, _) = engine_with_counter();
        let err = eng
            .ensure_point_in_target_window(10_000.0, 10_000.0, "click")
            .unwrap_err()
            .to_string();
        if let Some(value) = old {
            std::env::set_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW", value);
        }
        assert!(
            err.contains("outside the target window"),
            "off-target raw coordinates should fail clearly: {err}"
        );
    }

    #[test]
    fn raw_region_guard_rejects_off_target_regions() {
        let old = std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW").ok();
        std::env::remove_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW");
        let (eng, _) = engine_with_counter();
        let err = eng
            .ensure_region_in_target_window(
                Bbox {
                    x: 10_000.0,
                    y: 10_000.0,
                    w: 100.0,
                    h: 100.0,
                },
                "read_text",
            )
            .unwrap_err()
            .to_string();
        if let Some(value) = old {
            std::env::set_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW", value);
        }
        assert!(
            err.contains("outside the target window"),
            "off-target regions should fail clearly: {err}"
        );
    }

    #[test]
    fn top_level_menu_opener_listed_but_deep_submenu_item_filtered() {
        let (mut eng, calls) = engine_with_counter();
        // "Édition" is a top-level menu opener: direct child of the menubar root,
        // bbox null. "Supprimer" is a deep item under a closed Menu, bbox null.
        let edition = id_for(&eng, "Édition");
        let supprimer = id_for(&eng, "Supprimer");

        // Both are geometrically latent (no bbox) — only structure differs.
        assert!(eng.scene_graph().get(&edition).unwrap().bbox.is_none());
        assert!(eng.scene_graph().get(&supprimer).unwrap().bbox.is_none());

        // The exemption is STRUCTURAL, not role-based: Édition's parent IS the
        // menubar root; Supprimer's parent is a closed Menu, not the root.
        let menubar_root = eng
            .scene_graph()
            .roots
            .iter()
            .find(|id| {
                eng.scene_graph()
                    .get(id)
                    .map(|n| n.role == Role::MenuBar)
                    .unwrap_or(false)
            })
            .cloned()
            .expect("menubar root in roots");
        assert_eq!(
            eng.scene_graph().get(&edition).unwrap().parent.as_deref(),
            Some(menubar_root.as_str()),
            "Édition sits directly under the menubar root"
        );
        assert_ne!(
            eng.scene_graph().get(&supprimer).unwrap().parent.as_deref(),
            Some(menubar_root.as_str()),
            "Supprimer sits under a closed Menu, not the menubar root"
        );

        // query_affordances("click"): the opener is listed, the deep item is not.
        let click = eng.query_affordances(SemanticAction::Click);
        assert!(
            click.contains(&edition),
            "top-level menu opener listed despite null bbox"
        );
        assert!(
            !click.contains(&supprimer),
            "deep submenu item stays filtered (phantom)"
        );

        // include_latent brings back the deep phantom too (superset).
        let all = eng.query_affordances_filtered(SemanticAction::Click, true);
        assert!(all.contains(&edition));
        assert!(all.contains(&supprimer));

        // get_affordances mirrors the same exemption.
        let aff = eng.affordances_view(false);
        assert!(
            aff["affordances"].get(edition.as_str()).is_some(),
            "opener kept in get_affordances"
        );
        assert!(
            aff["affordances"].get(supprimer.as_str()).is_none(),
            "deep item omitted in get_affordances"
        );

        // find_element still locates both; the gate still acts on the deep item by id.
        assert!(!eng.find_element("Édition").is_empty());
        assert!(!eng.find_element("Supprimer").is_empty());
        let gated = eng.click_element(&supprimer, Some("delete")).unwrap();
        assert_eq!(gated.result, ActionResult::PendingApproval);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "exemption never opens the gate"
        );
    }
}
