# VisualOps MCP — POC

> From pixels to verified actions. **AX-first slice.**

A macOS daemon that turns a window into a **verifiable affordance graph** for AI
agents. Instead of `Click(x=842, y=661)`, an agent resolves a target by meaning —
a **scene-graph node** (system truth) and its **affordance** (semantic actions +
risk), two distinct objects keyed by the same stable id:

```json
// get_scene_graph (compact projection) — one node
{ "id": "btn_nouvelle_note", "role": "button", "label": "Nouvelle note",
  "bbox": { "x": 1815, "y": 391, "w": 45, "h": 52 },
  "enabled": true, "focused": false, "parent": "toolbar_692558b0", "n_children": 0 }

// get_affordances — the affordance for that same id
{ "id": "btn_nouvelle_note", "actions": ["click"], "drag_targets": [],
  "risk": { "level": "low", "requires_approval": false, "reasons": [] } }
```

(The node carries `confidence`/`source` in the `full` view; the compact projection
drops them. `risk` is the structured `RiskAssessment`, not a bare string.)

## Why this POC is small (and still proves the point)

The full vision (Tile/Foveal/OCR/ScreenCaptureKit, drag&drop, replay…) is large.
This POC proves the **load-bearing hypothesis**: *the macOS Accessibility tree is
rich enough to build the affordance graph without pixels or OCR.*

Validated on Notes (pure AX, no screenshot): 427 elements, each actionable one
already carrying `role`, native `actions`, `label`/`help`, an identifier, and
risk signals in the label text (`Supprimer`, `Éteindre`, …). So the
Tile/Foveal/OCR half is deferred to P1 — only needed for non-AX surfaces.

```
macOS AX tree
  -> Scene Graph        (stable IDs, role, bbox, confidence, freshness)
  -> Affordance Graph   (semantic actions, drag targets)
  -> Risk Engine        (low/medium/high + requires_approval)
  -> MCP tools + Audit   (verify_state, diff_since, export_trace)
```

## Workspace

| Crate                | Role                                                     |
|----------------------|----------------------------------------------------------|
| `visualops-core`     | Frozen contract: types, traits, `MockPerceptor`, fixtures|
| `visualops-graph`    | Pure logic: scene graph, affordances, risk, diff         |
| `visualops-platform` | macOS AX backend: `Perceptor` + `ActionExecutor`         |
| `visualops-vision`   | P1a spike: window capture + Apple Vision OCR + coord math |
| `visualops-mcp`      | Engine (risk gating + audit) + demo + MCP server         |

`graph` and `platform` depend only on `core`. See `docs/ARCHITECTURE.md`.

**Cross-platform compilation:** only `visualops-vision::coords` (pure coordinate
math) builds on any target; the rest of `visualops-vision` (capture, OCR) and all
of `visualops-platform` are `#[cfg(target_os = "macos")]`. So `cargo test` runs the
full logic/coords suite everywhere, and the macOS-only backends compile on macOS.

## Run

```bash
# Device-free demo on the Notes fixture: scene -> affordance -> risk gating -> audit
cargo run -p visualops-mcp -- demo

# Dump a live window's AX tree as JSON (find the pid/window via the MCP host)
cargo run -p visualops-platform --example dump -- <pid> <window_id>
```

The `demo` narrates: resolve "Nouvelle note" by **label** → click → a destructive
`Supprimer`/`Éteindre` is **denied pending approval** → approve → proceed →
audit trail exported as JSON.

## Status

POC / work in progress. Differentiator vs raw computer-use drivers: the semantic
layer — stable IDs, affordance normalisation, **risk-based approval gating**,
verify-loop and audit trail — not pixel OCR.
