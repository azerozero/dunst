//! The Dunst data model. These types are the contract between the
//! perception layer, the graph/logic layer, and the MCP server.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Axis-aligned bounding box in **global screen points** (top-left origin),
/// matching macOS / ScreenCaptureKit conventions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bbox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bbox {
    /// Serialize as the spec's `[x, y, x2, y2]` quad (used in MCP scene output).
    pub fn as_quad(&self) -> [f64; 4] {
        [self.x, self.y, self.x + self.w, self.y + self.h]
    }
}

/// Identifies the window a graph was captured from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WindowRef {
    pub pid: i32,
    pub window_id: u32,
    pub app_name: String,
    pub title: String,
}

// ---------------------------------------------------------------------------
// Raw perception output
// ---------------------------------------------------------------------------

/// A node exactly as observed by a [`Perceptor`](crate::Perceptor) — the raw
/// macOS AX element (or a vision/OCR-synthesised one in later phases). No
/// normalisation, no stable IDs yet. This is the *only* shape a perception
/// backend must produce; everything downstream is pure logic over it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawAxNode {
    /// Native AX role, e.g. `"AXButton"`, `"AXTextArea"`, `"AXMenuItem"`.
    pub ax_role: String,
    /// AX title / description, e.g. `"Nouvelle note"`.
    #[serde(default)]
    pub label: Option<String>,
    /// AX help / tooltip, e.g. `"Créer une note"`.
    #[serde(default)]
    pub help: Option<String>,
    /// AX value (text field contents, etc.).
    #[serde(default)]
    pub value: Option<String>,
    /// AX identifier when present, e.g. `"_NS:411"`, `"closeAll:"`.
    #[serde(default)]
    pub ax_identifier: Option<String>,
    /// Native action verbs reported by AX, e.g. `["press", "showmenu"]`.
    #[serde(default)]
    pub ax_actions: Vec<String>,
    /// Global-screen frame (from `AXFrame` / `AXPosition`+`AXSize`).
    #[serde(default)]
    pub frame: Option<Bbox>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub children: Vec<RawAxNode>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Scene graph
// ---------------------------------------------------------------------------

/// Normalised semantic role. `ax_role` is preserved on the node for the
/// `Unknown` case and for debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Button,
    MenuButton,
    TextField,
    TextArea,
    Checkbox,
    Radio,
    Row,
    Cell,
    MenuItem,
    Menu,
    MenuBar,
    List,
    Table,
    Outline,
    Window,
    Toolbar,
    StaticText,
    Image,
    Group,
    Unknown,
}

impl Role {
    /// Short prefix used when synthesising stable IDs (`btn_deploy`).
    pub fn id_prefix(self) -> &'static str {
        match self {
            Role::Button => "btn",
            Role::MenuButton => "mbtn",
            Role::TextField => "field",
            Role::TextArea => "text",
            Role::Checkbox => "chk",
            Role::Radio => "radio",
            Role::Row => "row",
            Role::Cell => "cell",
            Role::MenuItem => "mi",
            Role::Menu => "menu",
            Role::MenuBar => "menubar",
            Role::List => "list",
            Role::Table => "table",
            Role::Outline => "outline",
            Role::Window => "win",
            Role::Toolbar => "toolbar",
            Role::StaticText => "txt",
            Role::Image => "img",
            Role::Group => "grp",
            Role::Unknown => "el",
        }
    }

    /// The normalised role as the snake_case string used in the JSON encoding —
    /// it mirrors the `#[serde(rename_all = "snake_case")]` on [`Role`]. Lets
    /// callers (histogram keys, the compact projection) get the wire string
    /// directly, with no per-node `serde_json` round-trip. The
    /// `as_str_matches_serde_rename` test pins the two together.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::MenuButton => "menu_button",
            Role::TextField => "text_field",
            Role::TextArea => "text_area",
            Role::Checkbox => "checkbox",
            Role::Radio => "radio",
            Role::Row => "row",
            Role::Cell => "cell",
            Role::MenuItem => "menu_item",
            Role::Menu => "menu",
            Role::MenuBar => "menu_bar",
            Role::List => "list",
            Role::Table => "table",
            Role::Outline => "outline",
            Role::Window => "window",
            Role::Toolbar => "toolbar",
            Role::StaticText => "static_text",
            Role::Image => "image",
            Role::Group => "group",
            Role::Unknown => "unknown",
        }
    }
}

/// Source of truth for a node, in priority order (Accessibility First).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Accessibility,
    Vision,
    Ocr,
}

/// One element in the Scene Graph — the "system truth".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    /// Stable, human-readable, synthesised ID, e.g. `"btn_nouvelle_note"`.
    pub id: String,
    pub role: Role,
    /// Original AX role string, preserved for `Role::Unknown` and debugging.
    pub ax_role: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub bbox: Option<Bbox>,
    /// Detection confidence: `1.0` for AX-sourced, lower for vision/OCR.
    pub confidence: f32,
    pub source: Source,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    /// Native AX action verbs, carried through for the executor.
    #[serde(default)]
    pub ax_actions: Vec<String>,
    #[serde(default)]
    pub ax_identifier: Option<String>,
    /// Wall-clock (`now_ms`) at which this node was last observed.
    pub last_seen_ms: u64,
    /// Structural child-index path from the capture root to this node.
    ///
    /// Human-readable ids stay stable for agents, but duplicate controls can
    /// share the same role/label/identifier. The path lets platform backends
    /// re-resolve the exact occurrence instead of falling back to first match.
    #[serde(default)]
    pub path: Vec<usize>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

impl SceneNode {
    /// Age of the node relative to `now_ms` (the spec's `freshness_ms`).
    pub fn freshness_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_seen_ms)
    }
}

/// The flattened scene graph: id-keyed nodes plus root ordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneGraph {
    /// `BTreeMap` for deterministic iteration / stable diffs.
    pub nodes: BTreeMap<String, SceneNode>,
    pub roots: Vec<String>,
    pub captured_at_ms: u64,
    pub window: WindowRef,
}

impl SceneGraph {
    pub fn get(&self, id: &str) -> Option<&SceneNode> {
        self.nodes.get(id)
    }
}

// ---------------------------------------------------------------------------
// Affordance graph
// ---------------------------------------------------------------------------

/// A semantic action an agent may request — independent of the native AX verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticAction {
    Click,
    Hover,
    Type,
    KeyPress,
    Hotkey,
    OpenMenu,
    Pick,
    Toggle,
    Scroll,
    Drag,
    Raise,
    Focus,
}

/// Risk tier for an action/element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Output of the Risk Engine for a single element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub requires_approval: bool,
    /// Human-readable justifications (`["matched keyword: supprimer"]`).
    #[serde(default)]
    pub reasons: Vec<String>,
}

impl RiskAssessment {
    pub fn low() -> Self {
        Self {
            level: RiskLevel::Low,
            requires_approval: false,
            reasons: vec![],
        }
    }
}

/// The actions available on one element, plus its risk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Affordance {
    pub id: String,
    pub actions: Vec<SemanticAction>,
    /// IDs of elements this node can be dropped onto (drag candidates).
    #[serde(default)]
    pub drag_targets: Vec<String>,
    pub risk: RiskAssessment,
}

/// The full affordance graph derived from a [`SceneGraph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffordanceGraph {
    pub affordances: BTreeMap<String, Affordance>,
}

// ---------------------------------------------------------------------------
// Audit / diff
// ---------------------------------------------------------------------------

/// Per-node change between two scene graphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeChange {
    Added {
        id: String,
        label: Option<String>,
    },
    Removed {
        id: String,
        label: Option<String>,
    },
    /// `field` such as `"label"`, `"value"`, `"bbox"`, `"enabled"`.
    Changed {
        id: String,
        field: String,
        before: String,
        after: String,
    },
}

/// Structural diff between two scene graphs (`diff_since`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GraphDiff {
    pub changes: Vec<NodeChange>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// A single audited action (the spec's audit-trail record).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts_ms: u64,
    pub target_id: String,
    pub action: SemanticAction,
    #[serde(default)]
    pub argument: Option<String>,
    pub risk: RiskAssessment,
    /// Free-text agent reasoning supplied with the action request.
    #[serde(default)]
    pub reasoning: Option<String>,
    pub result: ActionResult,
    /// Diff of the scene graph caused by the action.
    #[serde(default)]
    pub graph_diff: GraphDiff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionResult {
    Success,
    Failed,
    Denied,
    PendingApproval,
}

#[cfg(test)]
mod role_tests {
    use super::Role;

    /// Every variant's [`Role::as_str`] must equal its serde wire string, so the
    /// hand-written table can never silently drift from the JSON encoding.
    #[test]
    fn as_str_matches_serde_rename() {
        let all = [
            Role::Button,
            Role::MenuButton,
            Role::TextField,
            Role::TextArea,
            Role::Checkbox,
            Role::Radio,
            Role::Row,
            Role::Cell,
            Role::MenuItem,
            Role::Menu,
            Role::MenuBar,
            Role::List,
            Role::Table,
            Role::Outline,
            Role::Window,
            Role::Toolbar,
            Role::StaticText,
            Role::Image,
            Role::Group,
            Role::Unknown,
        ];
        for r in all {
            let serde = serde_json::to_value(r).unwrap();
            assert_eq!(
                serde,
                serde_json::Value::String(r.as_str().to_string()),
                "as_str disagrees with serde for {r:?}"
            );
        }
    }
}
