use dunst_core::{AuditEntry, Bbox, SemanticAction};

/// Projection requested for [`Engine::scene_graph_view`](super::Engine::scene_graph_view)
/// (WP-J / J1). The MCP server defaults to [`Compact`](SceneView::Compact) so a
/// real client can take the graph inline; [`Full`](SceneView::Full) is the
/// unchanged escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneView {
    /// Per-node `{id, role, label, value?, bbox, enabled, focused, parent,
    /// n_children}` — the heavy/derivable AX fields are dropped (~5-10x lighter).
    Compact,
    /// Today's behaviour: the full scene graph, every field.
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

/// One OCR'd line returned by [`Engine::read_text`](super::Engine::read_text):
/// the recognised `text`, its bounding box in screen points, and Vision's
/// `confidence` in `[0,1]`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextHit {
    pub text: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

/// One geometric primitive returned by [`Engine::read_shapes`](super::Engine::read_shapes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShapeHit {
    pub kind: String,
    pub bbox: Bbox,
    pub confidence: f32,
}

/// One traversal point of [`Engine::scan_chart`](super::Engine::scan_chart).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChartSample {
    pub x: f64,
    pub value: Option<String>,
    pub time: Option<String>,
    pub raw: Vec<String>,
}

/// Result of [`Engine::scan_chart`](super::Engine::scan_chart).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanResult {
    pub present: bool,
    /// Whether SkyLight focus-without-raise activated the window before the scan.
    pub focused: bool,
    pub fill_ratio: f32,
    pub region: Option<Bbox>,
    pub samples: Vec<ChartSample>,
}

/// One top-level window, for [`Engine::list_windows`](super::Engine::list_windows).
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowSummary {
    pub window_id: u32,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub bounds: Bbox,
    pub on_screen: bool,
}

/// One running GUI app, for [`Engine::list_apps`](super::Engine::list_apps).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppSummary {
    pub app: String,
    pub pid: i32,
    /// Number of top-level windows this app owns.
    pub windows: usize,
    /// At least one of its windows is currently on-screen.
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

/// Result of launching/opening a URL in an app, including enough scope context
/// for a caller to re-attach or verify the correct browser window/tab.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LaunchAppResult {
    pub launched: bool,
    pub app: String,
    pub url: Option<String>,
    pub target: TargetState,
    pub target_window_title: String,
    pub matching_windows: Vec<WindowSummary>,
    pub verification_hint: Option<String>,
}

/// Lightweight page/window state for orientation without a screenshot or full scene graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageState {
    pub target: TargetState,
    pub title: String,
    pub url: Option<String>,
    /// Selected browser-tab title, when the target app exposes a tab strip.
    /// This lets callers detect stale/incoherent browser window titles after
    /// background URL opens or tab switches.
    pub browser_tab: Option<BrowserTab>,
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

/// Where [`Engine::select_file`](super::Engine::select_file) should click before
/// filling the native file chooser. `None` means the chooser is already open.
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

/// A scoped "enter the window" view: enough geometry and orientation to act on
/// the target without returning the full AX scene graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowView {
    pub target: TargetState,
    pub title: String,
    pub url: Option<String>,
    pub browser_tab: Option<BrowserTab>,
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
