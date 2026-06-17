> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# FIX round 2 — `visualops-graph` (owner: Claude / tmux 9)

Triaged from `docs/review-graph.md` (Codex), `cli-forge-perf`, `cli-audit-code`.
Work **only** in `crates/visualops-graph`. Do not touch `visualops-core` /
`visualops-platform`. Do not run git. Keep `cargo test -p visualops-graph` green
and add the new tests below.

## G1 [MAJEUR] Decomposed (NFD) accents in `text::normalize`
`normalize` only folds precomposed chars, so `"E\u{301}teindre"` (combining
acute) is NOT folded to `eteindre` → false-negative HIGH risk. Fix: decompose to
NFD and drop combining marks (`U+0300..=U+036F`) before folding, then keep the
existing precomposed map as a fallback. Either add `unicode-normalization` (light
dep) or strip the combining range manually. **Test:** `assess` on a node labelled
`"E\u{301}teindre"` and `"Re\u{301}initialiser"` returns HIGH.

## G2 [MAJEUR] `audit::diff` ignores structural changes
`collect_field_changes` compares only label/value/enabled/bbox. A move/reorder
that changes hierarchy yields an empty diff. Fix: also compare `parent` and
`children` (emit `NodeChange::Changed { field: "parent" | "children", .. }`,
serialising the children list deterministically), and compare `SceneGraph::roots`
in `diff`. **Test:** reparent a node / change a children vector → exactly the
expected `Changed` entries.

## G3 [MAJEUR] Label change must read as `Changed`, not Remove+Add
Because `synth_id` is label-derived, renaming a node changes its ID, so `diff`
emits `Removed`+`Added` instead of `Changed{field:"label"}` — fragile for audit.
Keep the human-readable IDs (product requirement) but add a **reconciliation pass
in `diff`**: after the naive pass, pair a `Removed` with an `Added` when they
share `(parent, role, ax_identifier)` (and bbox is close) and rewrite the pair
into a single `Changed{field:"label", before, after}`. **Test:** rename one
node's label, rebuild, diff → one `Changed{field:"label"}`, no Add/Remove.

## G4 [MAJEUR] `drag_targets_for` is wrong for `Cell`
For a `Cell`, the direct parent is its `Row`, so "sibling rows" finds cells, not
rows. Fix: for a `Cell`, first climb to the nearest ancestor `Row`, then
enumerate that row's parent's other `Row` children; keep the ancestor
`List`/`Table`/`Outline`. **Test:** a table with ≥2 rows, each with a cell →
each cell's `drag_targets` contains the sibling row id(s).

## G5 [PERF] Pre-normalise risk keyword tables once
`match_tier` calls `normalize(kw)` for all 35 keywords per node per refresh
(invariant work + allocations). Pre-normalise the high/medium tables once in
`RiskEngine::new()` (store `Vec<String>`), match against them directly. Keep the
original keyword text for the `reasons` strings.

## G6 [PERF] `drag_targets_for` quadratic dedup
`push_unique_id` does `Vec::contains` in a loop → O(n²) on large tables. Use a
`BTreeSet` (or dedup once at the end). Preserve deterministic order.

## G7 [MINEUR] Widen `path_hash`
4 hex (16 bits) collides well within the 5000-node platform cap. Widen to 12–16
hex (48–64 bits). Keep it deterministic (FNV-1a is fine, just emit more bits).

## G8 [TESTS] Synthetic unit tests (don't rely only on the Notes fixture)
Add `synth_id` (empty label, punctuation-only label → hash fallback; forced
collision → `_2`), `normalize` (NFD cases from G1), `diff` (G2/G3 cases), and the
G4 cell-drag case. Optionally add a tiny `criterion` bench for `build_scene_graph`
+ `derive_affordances` + `assess` driven by `MockPerceptor::notes_fixture()` (the
perf GATE wants a baseline) — only if quick; tests are the priority.

Finish: print `cargo test -p visualops-graph` result + a summary of what changed.
