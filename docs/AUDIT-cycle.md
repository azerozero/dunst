> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# Cycle - VisualOps MCP / audit of the 4 pure crates

> STRICT audit (read-only). No fixes, no `git`.
> Scope: `visualops-core`, `visualops-graph`, `visualops-mcp`, `visualops-vision`.
> `visualops-platform` is **excluded** (reviewed in parallel).
> Date: 2026-06-09 · Branch: `poc/visualops`

## Reference Signals (Measured, Not Estimated)

| Check | Result |
|---|---|
| `cargo build` (core+graph+mcp) | OK |
| `cargo clippy --all-targets` (the 4 crates) | **0 warnings, 0 errors** |
| `cargo test` core+graph+mcp | **40 tests OK** (0 failures) |
| `cargo test -p visualops-vision` (coords) | **14 tests OK** |
| Scope LOC | ~4 300 (core 488 / graph 1609 / mcp 1298 / vision 907) |

Remarkably healthy codebase for a POC: clean module boundaries, frozen `core`
with no macOS dependency, and pure logic fully tested against a device-free
fixture. The findings below are therefore mostly **hardening (P1)** and
**polish (P2)** - not P0.

---

## Scorecard (by Dimension)

| Dimension | Grade | Comment |
|---|---|---|
| Code quality (`audit-code`) | A− | Idiomatic, commented with justification (G1…G7, D1…D3, WP-J). A few serialization `unwrap()` calls. |
| Tests (`audit-test`) | A | 54 tests, targeted invariants, regression snapshots (ids, coord round-trips). Gap: pyramid has no stdio server integration test. |
| Coupling (`audit-tangle`) | A | Strict DAG `core ← graph/vision`, `mcp` aggregates. No cycle. `engine.rs` = only large file (844 L). |
| Drift (`audit-drift`) | B+ | No `CONTRACTS.md`. 2 doc↔code gaps (OCR confidence "monotone", README example). |
| Docs (`audit-doc`) | A− | Excellent ARCHITECTURE/README/WP. No rustdoc error on public `mcp` APIs. |
| Doc↔code sync (`audit-sync`) | B+ | README JSON example does not match the real `Affordance` schema. |
| Perf (`forge-perf`) | A | Keyword pre-normalization (G5), anti-O(n²) `BTreeSet` (G6), FNV-1a 64-bit (G7). 1 disposable recomputation + 1 O(N) recomputation per listing. |

---

## 3-2-1 Triage (all findings, unabridged)

### 🔴 Tier 3 - Critical (0 items)

None. No security vulnerability, no broken feature, no data-loss risk. Risk
gating (high-risk action → `PendingApproval`, executor never called) is correct
and tested (`high_risk_click_is_gated_then_approved`,
`find_element_and_gating_still_reach_latent_nodes`).

### 🟡 Tier 2 - Major (5 items)

| # | Finding | File:line | Dim. | Effort | Concrete action |
|---|---|---|---|---|---|
| 1 | **Duplicated and untested OCR transform.** `region_to_vision_roi` manually reimplements the Y-flip + clamp already provided, proven, and tested by `coords::window_rect_to_vision_roi`. Its clamp is also **different** (`w.clamp(0, 1-x)` vs edge-based clamp), so there are two divergent edge behaviors for "the same" conversion - exactly the #1 bug predicted (§10.8). | `vision/src/ocr.rs:115-141` | drift / code | M | Replace the body with a call to `coords::window_rect_to_vision_roi` (converting the input from screen-pt to window-local), and remove the copy. |
| 2 | **Dead computation in the OCR hot path.** `vision_norm_to_screen_pt` is called and then the `_screen_box` result is discarded for every observation. Unneeded work per OCR line + masked intent (the `OcrBox` only carries `norm`). | `vision/src/ocr.rs:107` | perf / code | L | Either remove the call, or add the screen-pt `Bbox` to `OcrBox` if consumers need it (otherwise remove it). |
| 3 | **`find_element` does not apply the latent filter or compact projection.** The other listings (`query_affordances`, `get_affordances`, `get_scene_graph`) filter latent nodes and compact (WP-J). `find_element` returns the **full** `SceneNode` for each match, including latent nodes - inconsistent MCP surface and heavy payload. | `mcp/src/serve.rs:170-172`, `engine.rs:105-116` | sync / code | M | Decide the policy (for example, project to compact + `include_latent` flag), or document the gap if intentional. |
| 4 | **Serialization `unwrap()` on the MCP path.** Multiple `serde_json::to_value(...).unwrap()` calls in `handle_tool_call` (l.171,181,188,195,202,220): a serialization failure panics the server instead of returning a JSON-RPC error, while `engine.rs` already uses `.unwrap_or(Value::Null)` elsewhere. | `mcp/src/serve.rs:171-220` | code | L | Replace with `.unwrap_or(Value::Null)` or propagate as `isError:true`, consistent with `engine.rs`. |
| 5 | **No `CONTRACTS.md` and no stdio server integration test.** The strong invariants (gating never bypassed, `full` byte-identical, `actionable_only` ⊆ total) live in comments and unit tests, but the `serve` binary (parse JSON-RPC → dispatch) has no test; a "live serve smoke" harness exists (commit `9ba75e2`) outside this scope. | `mcp/src/serve.rs` (global) | test / drift | M | Add 2-3 table-driven JSON-RPC tests on `handle_tool_call` (unknown tool, missing arg, invalid `view`); freeze the key contracts. |

### 🟢 Tier 1 - Minor (6 items)

| # | Finding | File:line | Dim. | Effort | Concrete action |
|---|---|---|---|---|---|
| 6 | **README example does not match the real schema.** The JSON shows `{id, role, label, actions, confidence, risk}` on a single object, but `confidence` is on `SceneNode` and `actions`/`risk` are on `Affordance` (two distinct types). No serialized object has that shape. | `README.md:8-11` | sync | L | Mark the example as an "illustrative merged view" or split it into scene-node + affordance. |
| 7 | **"Risk monotone with uncertainty" claim not implemented.** `OcrBox.confidence` is documented as needing to raise the gate (§10.7), but `RiskEngine::assess` never consumes confidence (AX-only POC, `confidence=1.0`). Latent drift to track for P1. | `vision/src/lib.rs:50-57` | drift / doc | L | Add an explicit `// TODO P1` or a "not wired yet" note to avoid a false guarantee. |
| 8 | **`role_key` re-serializes an enum through `serde_json` per node** to get the role string (summary histogram + compact). Allocation + serde pass are avoidable. | `mcp/src/engine.rs:466-471` | perf | L | Expose a `Role::as_str(&self) -> &'static str` in `core` (already near `id_prefix`) and use it. |
| 9 | **O(N) recomputation of `window_rect`/`menubar_root_id` on every listing.** Each `scene_graph_view` / `affordances_view` / `query_affordances_filtered` call rescans all nodes to find the window and menubar root. Acceptable for the POC (small graphs), should be memoized if the 5000-node cap is reached. | `mcp/src/engine.rs:179-198` | perf | M | Cache `(window_rect, menubar_root_id)` during `refresh()`. |
| 10 | **Two nearly identical `pipeline`/`graph_bench` benches.** `graph_bench.rs` is a subset of `pipeline.rs` (same 3 functions, without the "full" group). Duplication. | `graph/benches/graph_bench.rs` | code (DRY) | L | Delete `graph_bench.rs` or reduce it to the only case not covered, and adjust `Cargo.toml`. |
| 11 | **`Cargo.lock` versions heavy second-hand deps for the vision spike** (`objc2-vision`, etc.) without a `rust-toolchain`/CI pinning the macOS target. Risk of broken non-macOS build if `coords` were not correctly `cfg`-guarded (it is - verified). Purely preventive. | `vision/Cargo.toml` | infra | L | Document in the README that only `coords` compiles cross-platform; the rest is `cfg(target_os="macos")`. |

---

## Top 5 Highest-ROI Recommendations

Ranked by (impact × confidence) ÷ effort:

1. **#2 - Remove the dead `_screen_box` from `ocr.rs`** (effort L, immediate gain).
   Unneeded work per OCR line is removed from the < 100 ms hot path, and the
   intent becomes readable again. Zero risk.

2. **#1 - Unify the OCR transform on `coords::window_rect_to_vision_roi`**
   (effort M). Eliminates the *second* divergent implementation of Y-flip/clamp -
   the #1 bug source explicitly anticipated by the docs - and lets `ocr.rs`
   benefit from the 14 `coords` tests instead of zero.

3. **#4 - Replace MCP serialization `unwrap()` calls with a fallback**
   (effort L). Turns a potential server panic into a clean JSON-RPC error,
   and aligns `serve.rs` with the convention already held in `engine.rs`.

4. **#5 - Table-driven tests on `handle_tool_call`** (effort M). Covers the
   dispatcher (the only large untested piece in scope) and freezes MCP surface
   invariants - high return for few lines.

5. **#3 - Align `find_element` with the latent/compact policy** (effort M).
   Removes the inconsistency between the 4 listing tools and lightens the largest
   unprojected payload exposed to the agent.

---

## Strengths (Preserve)

- **Exemplary crate boundaries**: frozen `core`, pure and device-free
  `graph`/`vision`, aggregator `mcp`. DAG with no cycle.
- **Perf discipline already applied**: G5 (pre-normalized keywords), G6 (anti
  O(n²)), G7 (64-bit hash), commented with their justification.
- **Invariant tests, not surface tests**: coordinate round-trips, regression id
  snapshot, gating never bypassable even on latent nodes.
- **Deliberate and tested `_NS:` stable-id policy** (`is_appkit_auto`) - do NOT
  "fix" it: it is an accepted WP-D deviation.
- **Clippy clean + 54 green tests** across the whole scope.

## Phoenix Status

🟢 **Converged on the health criterion**: 0 🔴 items, and the 🟡 items are
hardening, not blocking defects. The POC is healthy. Next useful pass: after
the live backend (`visualops-platform`, out of scope) and P1 vision are wired.
