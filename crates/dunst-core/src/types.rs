//! The Dunst data model. These types are the contract between the
//! perception layer, the graph/logic layer, and the MCP server.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Axis-aligned bounding box in **global screen points** (top-left origin),
/// matching macOS / ScreenCaptureKit conventions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bbox {
    /// Left edge, in global screen points.
    pub x: f64,
    /// Top edge, in global screen points.
    pub y: f64,
    /// Width, in screen points.
    pub w: f64,
    /// Height, in screen points.
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
    /// Process ID of the owning application.
    pub pid: i32,
    /// Native window identifier within that process.
    pub window_id: u32,
    /// Application name, e.g. `"Notes"`.
    pub app_name: String,
    /// Window title at capture time.
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
    /// Native menu shortcut character from AXMenuItemCmdChar.
    #[serde(default)]
    pub cmd_char: Option<String>,
    /// Native menu shortcut modifier bitmask from AXMenuItemCmdModifiers.
    #[serde(default)]
    pub cmd_modifiers: Option<u64>,
    /// Native virtual key from AXMenuItemCmdVirtualKey, kept for diagnostics.
    #[serde(default)]
    pub cmd_virtual_key: Option<u16>,
    /// Native action verbs reported by AX, e.g. `["press", "showmenu"]`.
    #[serde(default)]
    pub ax_actions: Vec<String>,
    /// Global-screen frame (from `AXFrame` / `AXPosition`+`AXSize`).
    #[serde(default)]
    pub frame: Option<Bbox>,
    /// Whether the element is enabled (interactive); defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the element currently holds keyboard focus.
    #[serde(default)]
    pub focused: bool,
    /// Child AX nodes, forming the raw perception tree.
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
    /// A push button.
    Button,
    /// A button that opens a menu (pop-up / pull-down).
    MenuButton,
    /// A single-line text input.
    TextField,
    /// A multi-line text input.
    TextArea,
    /// A checkbox toggle.
    Checkbox,
    /// A radio button.
    Radio,
    /// A row in a table, list, or outline.
    Row,
    /// A cell within a row.
    Cell,
    /// An item within a menu.
    MenuItem,
    /// A menu container.
    Menu,
    /// A menu bar container.
    MenuBar,
    /// A list container.
    List,
    /// A table container.
    Table,
    /// An outline (tree) container.
    Outline,
    /// A window.
    Window,
    /// A toolbar container.
    Toolbar,
    /// Non-interactive static text.
    StaticText,
    /// An image.
    Image,
    /// A generic grouping container.
    Group,
    /// A role that could not be normalised; see `ax_role` for the original.
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
    /// Sourced from the macOS Accessibility (AX) API — highest priority.
    Accessibility,
    /// Sourced from vision-based UI detection.
    Vision,
    /// Sourced from optical character recognition.
    Ocr,
}

/// One element in the Scene Graph — the "system truth".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    /// Stable, human-readable, synthesised ID, e.g. `"btn_nouvelle_note"`.
    pub id: String,
    /// Normalised semantic role.
    pub role: Role,
    /// Original AX role string, preserved for `Role::Unknown` and debugging.
    pub ax_role: String,
    /// Display label / title, when known.
    #[serde(default)]
    pub label: Option<String>,
    /// Help / tooltip text, when known.
    #[serde(default)]
    pub help: Option<String>,
    /// Current value (e.g. text-field contents), when known.
    #[serde(default)]
    pub value: Option<String>,
    /// Bounding box in global screen points, when known.
    #[serde(default)]
    pub bbox: Option<Bbox>,
    /// Detection confidence: `1.0` for AX-sourced, lower for vision/OCR.
    pub confidence: f32,
    /// Which perception layer produced this node.
    pub source: Source,
    /// Whether the element is enabled (interactive); defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the element currently holds keyboard focus.
    #[serde(default)]
    pub focused: bool,
    /// Native AX action verbs, carried through for the executor.
    #[serde(default)]
    pub ax_actions: Vec<String>,
    /// Native AX identifier, when present.
    #[serde(default)]
    pub ax_identifier: Option<String>,
    /// Native menu shortcut character, when present.
    #[serde(default)]
    pub cmd_char: Option<String>,
    /// Native menu shortcut modifier bitmask, when present.
    #[serde(default)]
    pub cmd_modifiers: Option<u64>,
    /// Native virtual key of the menu shortcut, when present.
    #[serde(default)]
    pub cmd_virtual_key: Option<u16>,
    /// Wall-clock (`now_ms`) at which this node was last observed.
    pub last_seen_ms: u64,
    /// Structural child-index path from the capture root to this node.
    ///
    /// Human-readable ids stay stable for agents, but duplicate controls can
    /// share the same role/label/identifier. The path lets platform backends
    /// re-resolve the exact occurrence instead of falling back to first match.
    #[serde(default)]
    pub path: Vec<usize>,
    /// ID of this node's parent, if any.
    #[serde(default)]
    pub parent: Option<String>,
    /// IDs of this node's children, in order.
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
    /// IDs of the root nodes, in capture order.
    pub roots: Vec<String>,
    /// Wall-clock (`now_ms`) at which the graph was captured.
    pub captured_at_ms: u64,
    /// The window this graph was captured from.
    pub window: WindowRef,
}

impl SceneGraph {
    /// Returns the node with the given `id`, if present.
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
    /// Click / press the element.
    Click,
    /// Move the pointer over the element.
    Hover,
    /// Type text into the element.
    Type,
    /// Press a single key.
    KeyPress,
    /// Press a key combination (shortcut).
    Hotkey,
    /// Open the element's menu.
    OpenMenu,
    /// Pick / select the element (e.g. a menu item or row).
    Pick,
    /// Toggle the element's state.
    Toggle,
    /// Scroll the element.
    Scroll,
    /// Drag the element (onto a drop target).
    Drag,
    /// Raise the element's window to the front.
    Raise,
    /// Give the element keyboard focus.
    Focus,
}

/// Risk tier for an action/element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Low risk; safe to perform without approval.
    Low,
    /// Medium risk.
    Medium,
    /// High risk; typically requires operator approval.
    High,
}

/// Output of the Risk Engine for a single element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// Assessed risk tier.
    pub level: RiskLevel,
    /// Whether the action must be approved by an operator before running.
    pub requires_approval: bool,
    /// Human-readable justifications (`["matched keyword: supprimer"]`).
    #[serde(default)]
    pub reasons: Vec<String>,
}

impl RiskAssessment {
    /// Returns a low-risk assessment that requires no approval.
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
    /// ID of the element these affordances belong to.
    pub id: String,
    /// Semantic actions available on the element.
    pub actions: Vec<SemanticAction>,
    /// IDs of elements this node can be dropped onto (drag candidates).
    #[serde(default)]
    pub drag_targets: Vec<String>,
    /// Risk assessment for acting on the element.
    pub risk: RiskAssessment,
}

/// The full affordance graph derived from a [`SceneGraph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffordanceGraph {
    /// Affordances keyed by element ID.
    pub affordances: BTreeMap<String, Affordance>,
}

// ---------------------------------------------------------------------------
// Audit / diff
// ---------------------------------------------------------------------------

/// Per-node change between two scene graphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeChange {
    /// A node appeared in the newer graph.
    Added {
        /// ID of the added node.
        id: String,
        /// Label of the added node, if any.
        label: Option<String>,
    },
    /// A node disappeared from the newer graph.
    Removed {
        /// ID of the removed node.
        id: String,
        /// Label of the removed node, if any.
        label: Option<String>,
    },
    /// A field of an existing node changed.
    ///
    /// `field` such as `"label"`, `"value"`, `"bbox"`, `"enabled"`.
    Changed {
        /// ID of the changed node.
        id: String,
        /// Name of the field that changed.
        field: String,
        /// Value before the change.
        before: String,
        /// Value after the change.
        after: String,
    },
}

/// Structural diff between two scene graphs (`diff_since`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GraphDiff {
    /// The per-node changes between the two graphs.
    pub changes: Vec<NodeChange>,
}

impl GraphDiff {
    /// Returns `true` when there are no changes.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Identifies the MCP session/client responsible for an action.
///
/// This is provenance, not authentication. Future inter-process locks and
/// leases can use it to explain ownership, but risk approval remains a
/// separate operator-side gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentity {
    /// Unique identifier of the MCP session.
    pub session_id: String,
    /// Name of the connecting client, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    /// Version of the connecting client, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    /// Agent identifier, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// PID of the parent process, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    /// Name of the parent process, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_process: Option<String>,
}

/// A single audited action (the spec's audit-trail record).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Wall-clock (`now_ms`) at which the action was recorded.
    pub ts_ms: u64,
    /// ID of the element the action targeted.
    pub target_id: String,
    /// The semantic action that was requested.
    pub action: SemanticAction,
    /// Optional argument supplied with the action (e.g. typed text).
    #[serde(default)]
    pub argument: Option<String>,
    /// Risk assessment computed for the action.
    pub risk: RiskAssessment,
    /// Free-text agent reasoning supplied with the action request.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Outcome of the action.
    pub result: ActionResult,
    /// `false` when the input layer reported success but post-action evidence
    /// only showed low-signal churn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_verified: Option<bool>,
    /// Diff of the scene graph caused by the action.
    #[serde(default)]
    pub graph_diff: GraphDiff,
    /// MCP session/client that requested the action, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<SessionIdentity>,
}

/// Outcome of an audited action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionResult {
    /// The action completed successfully.
    Success,
    /// The action was attempted but failed.
    Failed,
    /// The action was denied (e.g. by policy).
    Denied,
    /// The action is waiting for operator approval.
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
