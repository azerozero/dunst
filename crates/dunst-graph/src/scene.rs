//! Raw AX tree -> normalised [`SceneGraph`].
//!
//! Responsibilities (see WP-B for detail):
//! 1. Map native `ax_role` -> [`Role`] ([`map_role`]).
//! 2. Flatten the tree into `BTreeMap<id, SceneNode>` with `parent`/`children`
//!    wired by synthesised IDs and `roots` preserved in order.
//! 3. Synthesise **stable, human-readable IDs** ([`synth_id`]): `"btn_nouvelle_note"`.
//!    Must be deterministic and collision-free within one graph.
//! 4. Set `confidence = 1.0` and `source = Source::Accessibility` for AX nodes,
//!    `last_seen_ms = now_ms`.

use std::collections::{BTreeMap, BTreeSet};

use dunst_core::{RawAxNode, Role, SceneGraph, SceneNode, Source, WindowRef};

use crate::text::normalize;

/// Map a native AX role string to a normalised [`Role`]. Unknown roles map to
/// [`Role::Unknown`] (the original string stays in `SceneNode::ax_role`).
pub fn map_role(ax_role: &str) -> Role {
    match ax_role {
        "AXButton" => Role::Button,
        "AXMenuButton" => Role::MenuButton,
        "AXTextField" => Role::TextField,
        "AXTextArea" => Role::TextArea,
        "AXCheckBox" => Role::Checkbox,
        "AXRadioButton" => Role::Radio,
        "AXRow" => Role::Row,
        "AXCell" => Role::Cell,
        "AXMenuItem" => Role::MenuItem,
        "AXMenu" => Role::Menu,
        "AXMenuBar" | "AXMenuBarItem" => Role::MenuBar,
        "AXList" => Role::List,
        "AXTable" => Role::Table,
        "AXOutline" => Role::Outline,
        "AXWindow" => Role::Window,
        "AXToolbar" => Role::Toolbar,
        "AXStaticText" => Role::StaticText,
        "AXImage" => Role::Image,
        "AXGroup" => Role::Group,
        _ => Role::Unknown,
    }
}

/// Synthesise a stable, human-readable, unique-within-graph ID for a node.
///
/// Priority of the id *source* (D1):
/// 1. A **developer-assigned** `ax_identifier` (`is_stable_identifier`) →
///    `"{role_prefix}_{slug(ax_identifier)}"`. This makes the id stable *by
///    construction*: it survives a label/value change, so a rename surfaces as a
///    single `Changed{label}` instead of Remove+Add (see [`crate::audit`] G3).
/// 2. Otherwise the node `label` → `"{role_prefix}_{slug(label)}"` (unchanged).
/// 3. Otherwise a short hash of the structural `path` (unchanged).
///
/// The role prefix is always kept so ids stay glanceable (`btn_…`, `mi_…`), and
/// `used` tracks already-emitted IDs so a numeric suffix can disambiguate
/// collisions (`btn_partager`, `btn_partager_2`) — including two siblings that
/// share one `ax_identifier`.
pub fn synth_id(
    role: Role,
    label: Option<&str>,
    ax_identifier: Option<&str>,
    path: &[usize],
    used: &std::collections::BTreeSet<String>,
) -> String {
    let prefix = role.id_prefix();

    // Prefer a developer-assigned AXIdentifier when it slugs to something
    // non-empty; fall back to the (unchanged) label-slug / path-hash scheme.
    let from_identifier = ax_identifier
        .filter(|id| is_stable_identifier(id))
        .map(slug)
        .filter(|s| !s.is_empty());

    let base = match from_identifier {
        Some(s) => format!("{prefix}_{s}"),
        None => match label.map(slug) {
            Some(ref s) if !s.is_empty() => format!("{prefix}_{s}"),
            // No label, or a label that slugs to nothing (all punctuation): use a
            // short deterministic hash of the structural path.
            _ => format!("{prefix}_{}", path_hash(path)),
        },
    };

    if !used.contains(&base) {
        return base;
    }
    // Collision: append the smallest free numeric suffix (`_2`, `_3`, ...).
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}_{n}");
        if !used.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Is `ax_identifier` a **developer-assigned** identifier we can trust as a
/// rename-invariant id source?
///
/// AppKit auto-generates ordinal `_NS:<n>` accessibility identifiers (e.g.
/// `_NS:573`) that are **not** stable across launches or framework versions —
/// using them as an id source would reintroduce the very churn stable ids are
/// meant to remove. We therefore treat the `_NS:<digits>` pattern as "no usable
/// identifier" and fall back to the label-slug / path-hash scheme. Anything else
/// non-empty (`"Note Body Text View"`, `"_forceQuitRequested:"`) is honoured.
pub(crate) fn is_stable_identifier(ax_identifier: &str) -> bool {
    !ax_identifier.is_empty() && !is_appkit_auto(ax_identifier)
}

/// Matches AppKit's auto-generated `_NS:<digits>` identifier pattern (no regex
/// dependency): a `_NS:` prefix followed by one or more ASCII digits and nothing
/// else.
fn is_appkit_auto(s: &str) -> bool {
    matches!(
        s.strip_prefix("_NS:"),
        Some(rest) if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
    )
}

/// Turn a human label into a slug: lowercase ASCII, accents folded, runs of
/// non-alphanumerics collapsed to a single `_`, trimmed, capped at ~40 chars.
fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in normalize(label).chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            out.push(c);
            pending_sep = false;
        } else {
            // Space/punctuation: remember we need a separator, but only emit it
            // before the next real char so leading/trailing/repeats collapse.
            pending_sep = true;
        }
    }
    out.chars()
        .take(40)
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Short, stable hex of a structural path (child-index chain from the root).
/// FNV-1a **64-bit** over the index bytes -> 16 hex chars (G7). 16 bits collided
/// well within the 5000-node platform cap; 64 bits makes a collision (and thus
/// an order-dependent `_2` suffix on label-less nodes) astronomically unlikely.
fn path_hash(path: &[usize]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64-bit offset basis
    for &idx in path {
        for b in (idx as u64).to_le_bytes() {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV-1a 64-bit prime
        }
    }
    format!("{hash:016x}")
}

/// Build the full scene graph from perceived roots.
pub fn build_scene_graph(roots: Vec<RawAxNode>, window: WindowRef, now_ms: u64) -> SceneGraph {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut nodes: BTreeMap<String, SceneNode> = BTreeMap::new();
    let mut root_ids = Vec::with_capacity(roots.len());

    for (i, root) in roots.iter().enumerate() {
        let mut path = vec![i];
        let id = flatten(root, &mut path, None, now_ms, &mut used, &mut nodes);
        root_ids.push(id);
    }

    SceneGraph {
        nodes,
        roots: root_ids,
        captured_at_ms: now_ms,
        window,
    }
}

/// DFS one node: synthesise its ID, recurse into children (so their IDs are
/// stable and parent-linked), then insert the normalised [`SceneNode`].
/// Returns the node's synthesised ID. `path` holds the child-index chain
/// (including this node's own index) and is restored on exit.
fn flatten(
    node: &RawAxNode,
    path: &mut Vec<usize>,
    parent: Option<String>,
    now_ms: u64,
    used: &mut BTreeSet<String>,
    nodes: &mut BTreeMap<String, SceneNode>,
) -> String {
    let role = map_role(&node.ax_role);
    let id = synth_id(
        role,
        node.label.as_deref(),
        node.ax_identifier.as_deref(),
        path,
        used,
    );
    // Reserve the ID before recursing so children see it for collision checks.
    used.insert(id.clone());

    let mut child_ids = Vec::with_capacity(node.children.len());
    for (i, child) in node.children.iter().enumerate() {
        path.push(i);
        let child_id = flatten(child, path, Some(id.clone()), now_ms, used, nodes);
        path.pop();
        child_ids.push(child_id);
    }

    nodes.insert(
        id.clone(),
        SceneNode {
            id: id.clone(),
            role,
            ax_role: node.ax_role.clone(),
            label: node.label.clone(),
            help: node.help.clone(),
            value: node.value.clone(),
            bbox: node.frame,
            confidence: 1.0,
            source: Source::Accessibility,
            enabled: node.enabled,
            focused: node.focused,
            ax_actions: node.ax_actions.clone(),
            ax_identifier: node.ax_identifier.clone(),
            last_seen_ms: now_ms,
            path: path.clone(),
            parent,
            children: child_ids,
        },
    );
    id
}
