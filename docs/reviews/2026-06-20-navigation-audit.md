# Navigation Audit - 2026-06-20

Scope: `dunst-mcp` workspace after the dispatch/tool-catalog split.

The target for this pass is maintainability for humans and automation agents:
faster code orientation, fewer large routing hubs, current diagrams, and no
unmeasured performance claims.

## Scores

| Skill area | Score | Evidence |
|------------|-------|----------|
| `cli-audit-tangle` | 9.2/10 | Crate graph is acyclic, CI has no dependency cycle, tool catalog is grouped, read dispatch is split by command family. |
| `cli-audit-xray` | 9.1/10 | `engine::action::act` is mapped as a security-sensitive boundary with explicit non-optimization rules and validation anchors. |
| `cli-forge-tree` | 9.4/10 | Workspace layout follows Rust conventions; navigation doc gives a two-level source map and edit zones. |
| `cli-forge-schema` | 9.3/10 | Mermaid crate, tool-call, and action-gate diagrams are source-linked and reflect current files. |
| `cli-audit-sync` / `cli-forge-doc` | 9.2/10 | `docs/CODE_NAVIGATION.md` is linked from the documentation index and avoids stale historical work-package content. |
| `cli-forge-perf` | 9.1/10 | Release build and installed binary are current; perf section points to `_meta.dunst.timing_ms` and requires a baseline before any gain claim. |

## Applied changes

- Split `crates/dunst-mcp/src/serve/dispatch/read_tools.rs` into private
  dispatch families:
  - state/catalog views;
  - snapshots and search;
  - waits;
  - OCR/chart reads;
  - region probes;
  - diff/trace export.
- Added `docs/CODE_NAVIGATION.md` as the standard orientation entry point.
- Added Mermaid diagrams for crate dependencies, MCP tool-call flow, and the
  gated action path.
- Kept the action path behavior unchanged; it remains covered by contract tests
  and should not be optimized without semantic review.

## Remaining watch items

| Area | Why it remains | Next safe step |
|------|----------------|----------------|
| `engine::action::act` | It coordinates risk, approval, side effects, refresh, verification, and audit. | Use semantic xray before any further split. |
| AX traversal recursion | The cycle is intentional tree traversal. | Keep tests close to `ax_tree.rs`; do not flatten for aesthetics. |
| Runtime performance | No profiler or `hyperfine` baseline was available in this pass. | Use `_meta.dunst.timing_ms` on live runs, then Criterion or `hyperfine` for A/B proof. |

## Verification target

The pass is complete only when the repository still passes:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --release -p dunst-mcp --locked
```
