> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# Review `visualops-graph`

Command run: `cargo test -p visualops-graph` -> green, 6 tests pass.

Review scope respected: Rust sources read only, no `.rs` modifications, no `visualops-core` changes, no git command.

## BLOCKING

No blocking finding.

## MAJOR

### Unstable identifiers when the label changes

- **File:line**: `crates/visualops-graph/src/scene.rs:56`
- **Problem**: `synth_id` derives the primary identity from the label (`prefix_slug`). For a labeled node, a label change therefore changes its ID. Then `diff` can no longer emit `Changed { field: "label" }`: it will see a `Removed` and an `Added`. This weakens `diff_since`, the audit, and any MCP reference kept between two captures for elements whose text changes.
- **Suggested fix**: make identity stable independently of the label, while keeping the human slug. For example: prefer `ax_identifier` when available, otherwise add a stable suffix based on the path (`btn_nouvelle_note_a1b2`). If the exact `btn_nouvelle_note` format must remain for the POC, at least add reconciliation in `diff` for removed/added pairs by `(parent, role, ax_identifier, bbox)` to turn a label change into `Changed(label)`.

### Path hash too short for the 5000-node limit

- **File:line**: `crates/visualops-graph/src/scene.rs:98`
- **Problem**: `path_hash` truncates FNV-1a to 16 bits (`4 hex`). The platform constraint allows around 5000 nodes; at that size, hash collisions become likely. The `used` set prevents overwrites within a graph, but suffixes `_2`, `_3` depend on DFS order, so the IDs of unlabeled nodes can change when another node collides.
- **Suggested fix**: use at least 64 bits (`16 hex`) or 48 bits (`12 hex`) for paths, and add a simple generative test that builds several thousand unlabeled paths to verify there are no collision-induced suffixes in a typical capture.

### Diff ignores structural changes

- **File:line**: `crates/visualops-graph/src/audit.rs:39`
- **Problem**: `collect_field_changes` compares `label`, `value`, `enabled`, and `bbox`, but ignores `parent`, `children`, and `roots`. A drag/reorder or hierarchy change can therefore produce an empty diff if the IDs and compared fields remain identical. This is dangerous for auditing movement actions.
- **Suggested fix**: compare at least `parent` and `children` in `collect_field_changes`, and compare `SceneGraph::roots` in `diff`. Encode these differences with `NodeChange::Changed { field: "parent" | "children" | "roots", ... }` by serializing lists deterministically.

### Normalization does not handle decomposed accents

- **File:line**: `crates/visualops-graph/src/text.rs:8`
- **Problem**: `normalize` folds only precomposed characters (`é`, `à`, etc.). A decomposed Unicode string such as `e\u{301}teindre` keeps the combining mark, so it does not match the keyword `éteindre` normalized to `eteindre`. This can produce a High false negative in the RiskEngine.
- **Suggested fix**: normalize to NFD and remove combining marks via `unicode-normalization`, or add local handling for common combining marks (`U+0300..U+036F`) before matching. Add a test with `E\u{301}teindre` and `Re\u{301}initialiser`.

### Cells do not recover the expected sibling rows

- **File:line**: `crates/visualops-graph/src/affordance.rs:80`
- **Problem**: `drag_targets_for` looks for sibling rows among the direct parent's children. For a `Cell`, the direct parent is generally a `Row`, so its direct siblings are other cells/content, not the other rows in the container. The test passes because the `Outline`/`Table` ancestor makes the list non-empty, but the "sibling Row ids" heuristic is incomplete for cells.
- **Suggested fix**: for a `Cell`, first walk up to the closest ancestor row, then enumerate the other `Row` children of that row's parent. Then keep the `List`/`Table`/`Outline` ancestors.

## MINOR

### Risk matching by raw substring

- **File:line**: `crates/visualops-graph/src/risk.rs:117`
- **Problem**: `hay.contains(keyword)` can match a keyword inside a longer word, or miss punctuated variants (`shut-down`, `force-quit`). For High, false positives remain conservative; for Medium, this can add noise to affordances and audits.
- **Suggested fix**: normalize punctuation to spaces, tokenize, then match words/phrases on token boundaries. Keep High > Medium priority.

### Test coverage too fixture-centered

- **File:line**: `crates/visualops-graph/tests/wp_b.rs:52`
- **Problem**: tests validate the Notes fixture and the minimal WP-B criteria, but do not cover the edge cases requested here: empty/punctuation-only labels, decomposed accents, hash collisions, label change, parent/children change, and cell with multiple sibling rows.
- **Suggested fix**: add synthetic unit tests for `synth_id`, `diff`, `RiskEngine::assess`, and `derive_affordances` without relying only on `fixtures/notes.json`.

## Points Verified Without Anomaly

- `map_role` covers the minimal mapping from `docs/WP-B-graph.md`.
- `map_action` respects the expected mapping: `press`/`confirm` -> `Click`, `showmenu` -> `OpenMenu`, `pick` -> `Pick`, `raise` -> `Raise`, the rest -> `None`.
- `derive_affordances` correctly includes all nodes, dedupes actions in stable order, adds `Type`/`Focus` to text fields, and attaches `risk.assess`.
- The current diff is deterministic in output order thanks to `BTreeMap` and the fixed order of compared fields.
- Nodes without frames remain represented via `bbox: None`; no panic observed.
