//! Nested vision-zone tree.
//!
//! Groups flat vision primitives — detected `shapes`, OCR text runs, and
//! controls — into a **containment hierarchy** so an agent perceives
//! "section > card > control/text" instead of a flat list.
//!
//! The accessibility tree is already nested, but the vision path
//! (`detect_shapes` and OCR) returns flat vectors. On sparse-AX surfaces
//! (canvas/WebGL web apps, custom-drawn UIs) that
//! flatness forces an agent to reconstruct layout from raw boxes. This builder
//! reconstructs it once, by geometry.
//!
//! The builder is **pure** — no capture, no macOS, no color — so it unit-tests
//! offline. It reasons only about bounding boxes; the caller decides which
//! primitives to feed in and how to classify them.

use dunst_core::Bbox;

/// Coarse category of a [`Zone`], enough to guide an agent's reading order.
///
/// Serialization lives in the `dunst-mcp` layer (as for `shapes`), so this crate
/// stays serde-free; [`ZoneKind::as_str`] gives the wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    /// A large container: page section, panel, card, or list.
    Region,
    /// An interactive control: button, field, or link.
    Control,
    /// An OCR text run.
    Text,
    /// A detected graphic primitive: bar, circle, or line.
    Shape,
}

impl ZoneKind {
    /// Returns the lowercase wire form (`"region"`, `"control"`, `"text"`,
    /// `"shape"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ZoneKind::Region => "region",
            ZoneKind::Control => "control",
            ZoneKind::Text => "text",
            ZoneKind::Shape => "shape",
        }
    }
}

/// One flat region before nesting: a bounding box plus what produced it.
#[derive(Debug, Clone)]
pub struct ZoneItem {
    /// Stable identifier, echoed onto the resulting [`Zone`].
    pub id: String,
    /// Bounding box in screen points.
    pub bbox: Bbox,
    /// What kind of primitive this is.
    pub kind: ZoneKind,
    /// Human-readable text or label, when the source carries one.
    pub label: Option<String>,
    /// Source confidence in `[0, 1]`.
    pub confidence: f32,
}

/// A node in the nested zone tree: an item plus the items geometrically
/// contained within it.
#[derive(Debug, Clone)]
pub struct Zone {
    /// Identifier carried from the source [`ZoneItem`].
    pub id: String,
    /// Category of this node.
    pub kind: ZoneKind,
    /// Human-readable text or label, when present.
    pub label: Option<String>,
    /// Bounding box in screen points.
    pub bbox: Bbox,
    /// Source confidence in `[0, 1]`.
    pub confidence: f32,
    /// Zones geometrically nested inside this one, in reading order
    /// (top-to-bottom, then left-to-right).
    pub children: Vec<Zone>,
}

/// Tolerance (screen points) by which a child may spill outside a parent and
/// still count as contained — absorbs OCR/detection jitter on box edges.
const CONTAINMENT_SLACK: f64 = 2.0;

/// Builds a containment forest from flat vision primitives.
///
/// Each item nests under the **tightest** (smallest-area) other item that
/// geometrically contains it; items contained by nothing become roots. Children
/// and roots are ordered top-to-bottom, then left-to-right. Purely geometric:
/// identical or partially-overlapping boxes stay siblings.
pub fn build_zone_tree(items: Vec<ZoneItem>) -> Vec<Zone> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    // Process largest-first so every candidate parent is seen before its
    // children: any box that contains item `i` is strictly larger than it, so
    // it has already been placed by the time `i` is processed.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        area(items[b].bbox)
            .partial_cmp(&area(items[a].bbox))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut parent_of: Vec<Option<usize>> = vec![None; n];
    let mut seen: Vec<usize> = Vec::with_capacity(n);
    for &i in &order {
        let mut best: Option<usize> = None;
        for &p in &seen {
            if !contains(items[p].bbox, items[i].bbox) {
                continue;
            }
            // Keep the smaller-area container: it is the tighter parent.
            match best {
                Some(b) if area(items[b].bbox) <= area(items[p].bbox) => {}
                _ => best = Some(p),
            }
        }
        parent_of[i] = best;
        seen.push(i);
    }

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut roots: Vec<usize> = Vec::new();
    for (i, parent) in parent_of.iter().enumerate() {
        match parent {
            Some(p) => children[*p].push(i),
            None => roots.push(i),
        }
    }
    roots.sort_by(|&a, &b| reading_order(&items, a, b));
    for bucket in &mut children {
        bucket.sort_by(|&a, &b| reading_order(&items, a, b));
    }
    roots
        .into_iter()
        .map(|r| assemble(r, &items, &children))
        .collect()
}

/// Recursively turns index `i` and its children into a [`Zone`].
fn assemble(i: usize, items: &[ZoneItem], children: &[Vec<usize>]) -> Zone {
    let item = &items[i];
    Zone {
        id: item.id.clone(),
        kind: item.kind,
        label: item.label.clone(),
        bbox: item.bbox,
        confidence: item.confidence,
        children: children[i]
            .iter()
            .map(|&c| assemble(c, items, children))
            .collect(),
    }
}

/// Reading order: top-to-bottom, then left-to-right, on the items' boxes.
fn reading_order(items: &[ZoneItem], a: usize, b: usize) -> std::cmp::Ordering {
    let ba = items[a].bbox;
    let bb = items[b].bbox;
    ba.y.partial_cmp(&bb.y)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(ba.x.partial_cmp(&bb.x).unwrap_or(std::cmp::Ordering::Equal))
}

/// Area of `b` in square screen points, clamped so degenerate boxes read as `0`.
fn area(b: Bbox) -> f64 {
    b.w.max(0.0) * b.h.max(0.0)
}

/// Whether `parent` geometrically contains `child`: `child` sits inside
/// `parent` within [`CONTAINMENT_SLACK`], and `parent` is strictly larger — so
/// equal boxes never nest into each other.
fn contains(parent: Bbox, child: Bbox) -> bool {
    if area(parent) <= area(child) {
        return false;
    }
    parent.x <= child.x + CONTAINMENT_SLACK
        && parent.y <= child.y + CONTAINMENT_SLACK
        && parent.x + parent.w >= child.x + child.w - CONTAINMENT_SLACK
        && parent.y + parent.h >= child.y + child.h - CONTAINMENT_SLACK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: ZoneKind, x: f64, y: f64, w: f64, h: f64) -> ZoneItem {
        ZoneItem {
            id: id.to_string(),
            bbox: Bbox { x, y, w, h },
            kind,
            label: None,
            confidence: 1.0,
        }
    }

    #[test]
    fn nests_cards_and_text_under_a_region_in_reading_order() {
        // A page region with two stacked cards, each holding one text run.
        let items = vec![
            item("region", ZoneKind::Region, 0.0, 0.0, 100.0, 100.0),
            item("card2", ZoneKind::Region, 10.0, 60.0, 30.0, 30.0),
            item("text2", ZoneKind::Text, 12.0, 62.0, 10.0, 5.0),
            item("card1", ZoneKind::Region, 10.0, 10.0, 30.0, 30.0),
            item("text1", ZoneKind::Text, 12.0, 12.0, 10.0, 5.0),
        ];
        let tree = build_zone_tree(items);
        assert_eq!(tree.len(), 1);
        let region = &tree[0];
        assert_eq!(region.id, "region");
        // Children ordered top-to-bottom despite the shuffled input.
        assert_eq!(region.children.len(), 2);
        assert_eq!(region.children[0].id, "card1");
        assert_eq!(region.children[1].id, "card2");
        assert_eq!(region.children[0].children[0].id, "text1");
        assert_eq!(region.children[1].children[0].id, "text2");
    }

    #[test]
    fn tightest_container_wins_in_a_nested_chain() {
        // A ⊃ B ⊃ C: C must parent onto B, not A.
        let items = vec![
            item("A", ZoneKind::Region, 0.0, 0.0, 100.0, 100.0),
            item("C", ZoneKind::Text, 20.0, 20.0, 10.0, 10.0),
            item("B", ZoneKind::Region, 10.0, 10.0, 50.0, 50.0),
        ];
        let tree = build_zone_tree(items);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].id, "A");
        assert_eq!(tree[0].children.len(), 1);
        let b = &tree[0].children[0];
        assert_eq!(b.id, "B");
        assert_eq!(b.children.len(), 1);
        assert_eq!(b.children[0].id, "C");
    }

    #[test]
    fn disjoint_items_are_all_roots() {
        let items = vec![
            item("a", ZoneKind::Shape, 0.0, 0.0, 10.0, 10.0),
            item("b", ZoneKind::Shape, 50.0, 50.0, 10.0, 10.0),
        ];
        assert_eq!(build_zone_tree(items).len(), 2);
    }

    #[test]
    fn identical_boxes_stay_siblings() {
        // Same box from two sources (e.g. a Rect shape and its OCR label) must
        // not nest into each other — strict-area containment forbids it.
        let items = vec![
            item("rect", ZoneKind::Region, 0.0, 0.0, 20.0, 20.0),
            item("label", ZoneKind::Text, 0.0, 0.0, 20.0, 20.0),
        ];
        assert_eq!(build_zone_tree(items).len(), 2);
    }

    #[test]
    fn partial_overlap_stays_siblings() {
        let items = vec![
            item("a", ZoneKind::Region, 0.0, 0.0, 30.0, 30.0),
            item("b", ZoneKind::Region, 20.0, 20.0, 30.0, 30.0),
        ];
        assert_eq!(build_zone_tree(items).len(), 2);
    }

    #[test]
    fn slack_absorbs_small_edge_jitter() {
        // Child spills 1pt past the parent's right edge — within slack, so it
        // still nests.
        let items = vec![
            item("outer", ZoneKind::Region, 0.0, 0.0, 40.0, 40.0),
            item("inner", ZoneKind::Text, 5.0, 5.0, 36.0, 10.0),
        ];
        let tree = build_zone_tree(items);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].id, "inner");
    }

    #[test]
    fn empty_input_yields_empty_forest() {
        assert!(build_zone_tree(Vec::new()).is_empty());
    }
}
