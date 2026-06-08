# WP-D — `visualops-graph`: ax_identifier-stable IDs + bench (owner: Claude / tmux 3)

Work **only** in `crates/visualops-graph`. Do **not** touch `visualops-core` /
`visualops-platform` / `visualops-mcp`. Do **not** run git. Keep
`cargo test -p visualops-graph` green and add the new tests below.

## Context
`synth_id` (scene.rs) derives the node id from the **label** (`{role_prefix}_{slug(label)}`,
path-hash fallback). Consequence: renaming a node changes its id, so `diff` sees
Remove+Add and a fragile reconciliation pass (G3, audit.rs) has to pair them back
into a `Changed{field:"label"}`. But `RawAxNode`/`Node` **already carry
`ax_identifier: Option<String>`** (AXIdentifier — a developer-assigned, stable,
rename-invariant string), populated by the platform. We should make the id
**stable by construction** when that identifier exists. No core/contract change:
the field is already there.

## D1 [MAJEUR, main task] Prefer `ax_identifier` in `synth_id`
Change id synthesis so that when a node has a non-empty `ax_identifier`, the id is
derived from it (stable across label/value changes); otherwise fall back to the
current label-slug / path-hash scheme **unchanged**.

- Thread `ax_identifier: Option<&str>` into `synth_id` (and through `flatten` /
  `build_scene_graph` which already has the `RawAxNode`).
- When present and non-empty: `id = format!("{prefix}_{slug(ax_identifier)}")`
  (slug it the same way so ids stay human-readable and collision-handling/`_2`
  suffixing still applies). Keep the role prefix so ids remain
  glanceable (`btn_…`, `mi_…`).
- When absent/empty: **exactly today's behaviour** — do not regress label-derived
  ids for the Notes fixture (it has few/no AXIdentifiers, so most fixture ids must
  be byte-identical to now; assert this).
- Keep ids unique-within-graph via the existing `used` set + numeric suffix.

## D2 [MAJEUR] Simplify the diff given stable ids
With D1, a node that has an `ax_identifier` keeps its id across a rename, so `diff`
naturally emits `Changed{field:"label"}` for it — the G3 Remove+Add reconciliation
is only needed for the **label-derived** (no-identifier) nodes.

- Keep the reconciliation pass (still correct for identifier-less nodes) but verify
  it is **not** triggered for identifier-backed nodes (their ids are already
  stable). Add an assertion/comment to that effect.
- Do **not** delete the pass — fixtures without AXIdentifier still rely on it.

## D3 [TESTS] Cover the new behaviour
- `synth_id`/`build_scene_graph`: a node **with** `ax_identifier` → id derives from
  it; rename its label, rebuild → **same id**, and `diff` yields exactly one
  `Changed{field:"label"}` (no Add/Remove).
- a node **without** `ax_identifier` → id unchanged vs current scheme (lock the
  Notes-fixture ids: snapshot a couple of known ids like `btn_nouvelle_note` and
  assert they are still produced).
- collision: two siblings sharing the same `ax_identifier` → `_2` suffix applies.

## D4 [PERF/BASELINE] criterion bench harness
`criterion` is already an optional dep behind feature `bench` in `Cargo.toml`, but
there is no `benches/` yet. Add one so the platform batch-read work (WP-C) has a
**pure-pipeline** baseline to compare against.

- `crates/visualops-graph/benches/pipeline.rs`: drive
  `MockPerceptor::notes_fixture()` (from `visualops-core`) through
  `build_scene_graph` → `derive_affordances` → risk `assess` (the full pure
  pipeline, no AX/IO). One `criterion_group`/`criterion_main`, gated so
  `cargo bench -p visualops-graph --features bench` runs it.
- Add the `[[bench]] name = "pipeline" harness = false` entry to `Cargo.toml`.
- Report the baseline numbers (mean/median) in your finish summary. This measures
  the **pure** cost; capture/AX latency is WP-C's separate measurement.

Finish: print `cargo test -p visualops-graph` result, the `cargo bench` baseline
numbers, and a summary of what changed (esp. confirm Notes-fixture ids unchanged).
