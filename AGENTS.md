# Dunst

AX-first macOS MCP server for **background UI automation**: it exposes a macOS Accessibility affordance graph over the Model Context Protocol, gates mutating actions by risk, and keeps an audit trail — so an agent can perceive and drive a backgrounded/occluded window without stealing the user's foreground.

## Stack

- **Language**: Rust 2021 edition, Cargo **workspace** of 5 crates.
- **Protocol**: MCP over stdio (JSON-RPC 2.0). Tools expose perception, query and risk-gated actions.
- **CLI**: clap 4 (derive). Binary `dunst-mcp`; `serve` runs the MCP loop, the default command runs the device-free Notes fixture demo.
- **macOS backend**: Accessibility (`accessibility-sys`), CoreGraphics, and the private **SkyLight** framework (`SLEventPostToPid`, `SLSGetGlobalCursorData`, focus-without-raise) via `objc2`.
- **Vision**: screenshot capture + OCR for surfaces the AX tree does not expose (sparse-AX web content, terminals).
- **CI**: GitHub Actions (fmt, clippy, tests on macOS + Linux, cargo-audit, cargo-deny, cargo-machete, docs lint, codeql, semgrep, shellcheck). Releases via release-plz.
- **License**: see `LICENSE`.

## Architecture

Dunst perceives a target window and normalizes it into a stable **scene graph**, derives an **affordance graph** (what an agent may do, plus per-element risk), then executes requested `SemanticAction`s through a capability ladder, re-perceiving after each to produce a **graph diff** for the audit trail.

```
Perceptor (AX + vision) ─► SceneGraph ─► AffordanceGraph (+RiskEngine)
                                                  │
                        risk gate / approval ◄────┤
                                                  ▼
             AppDriver (native) ─► AX ─► background SkyLight event ─► borrow_cursor
                                                  │
                                          refresh + diff_since ─► AuditEntry
```

The executor prefers the least-intrusive rung that works: a **native app driver** (if one claims the window and is reachable), else **AX** (press/set-value/scrollbar), else a **window-scoped background event** (SkyLight auth-signed, no cursor, no foreground), and only as a last resort **`borrow_cursor`** (briefly move and restore the real OS cursor). Every mutating action is risk-gated and audited.

## Crate Layout

- **dunst-core**: shared types (`Target`, `WindowRef`, `SceneGraph`/`SceneNode`, `Affordance`/`AffordanceGraph`, `SemanticAction`, `RiskAssessment`, `AuditEntry`, `Bbox`), the `Perceptor` / `ActionExecutor` traits, and error types. No macOS deps.
- **dunst-graph**: the pure pipeline `build_scene_graph → derive_affordances → risk`. Deterministic, unit-benchable (`--features bench`).
- **dunst-platform**: the macOS backend — AX (`ax_backend`, `ax_tree`, `ax_actions`), SkyLight (`skylight`: focus-without-raise, cursor fingerprint, event posting), `pointer_events` (click/scroll/`borrow_cursor`/unstick), `web_events` (background click/scroll/type/key), `text_input`, `file_chooser`. Non-macOS builds get stubs.
- **dunst-vision**: `capture` (screenshots, window bounds) and OCR.
- **dunst-mcp**: the `Engine` (perception + affordances + risk gating + audit + execution, under `src/engine/`) and the MCP `serve` loop + tool schemas (`src/serve/`). The `drivers/` module hosts the per-app driver layer.

## Domain Concepts

- **Target**: the `(pid, window_id)` an engine is bound to. App identity lives in `WindowRef` (`app_name`, `title`), not `Target`.
- **SceneGraph / SceneNode**: id-keyed, deterministically-ordered nodes with `role`, `bbox`, `label`, `value`, `ax_actions`, `focused`, structural `path`. Synthesised, stable ids (e.g. `btn_nouvelle_note`) survive re-perception.
- **AffordanceGraph / Affordance**: per-node `SemanticAction`s (`Click`, `Type`, `Scroll`, `Hotkey`, `Raise`, `Pick`, …) plus its `RiskAssessment`. Actions are semantic, independent of the native AX verb.
- **RiskAssessment**: `RiskLevel` (Low/Medium/High) + `requires_approval` + human-readable `reasons`. High-risk / destructive actions block until the operator approves the exact gated id.
- **AuditEntry / GraphDiff**: every action pushes an entry with the before/after scene diff, so `graph_diff.changes` is the ground truth for "did this move anything" (see `scroll_result_low_signal`).
- **Background events**: `*_web_background` post CGEvents tagged with the target window + pid via SkyLight — window-scoped, cursor-less. The default for clicks/hover/scroll/type into a backgrounded window.
- **`focus_without_raise`**: makes a window AppKit-active without raising it (yabai recipe). Needed so a background web canvas paints — but note it **moves keyboard focus** (see Gotchas).
- **`borrow_cursor`**: real-cursor wheel scroll. The only path that scrolls some sparse-AX web feeds (LinkedIn/Firefox). Moves the shared cursor briefly, then restores it. Guarded by the user-idle check and auto-unstick.
- **User-idle guard**: synthetic input is refused while the operator was active < `DEFAULT_USER_IDLE_GUARD_MS` (150 ms) ago; `retry_user_active_guard` backs off and retries so automation waits for idle rather than fighting the user.
- **unstick_cursor**: recovery maneuver for the macOS bug that freezes the cursor shape (e.g. an I-beam) after driving a backgrounded window. Manual tool is immediate; the automatic post-scroll variant is idle-gated.
- **AppDriver / DriverRegistry**: per-app native driver layer (`crates/dunst-mcp/src/engine/drivers`). A driver claims a window by app identity and can serve `SemanticAction`s through the app's native channel (CDP, iTerm2 Python API, AppleScript); it declines (`NotApplicable`) to fall back to the generic ladder.

## Key Patterns

- **AX-first, vision-fallback**: the affordance graph is built from the AX tree; OCR/screenshot fills gaps for sparse-AX surfaces. AX ids are `confidence 1.0`, vision lower.
- **Least-intrusive execution ladder**: driver → AX → background event → `borrow_cursor`. Session memory (`scroll_strategy_cache`, `scroll_background_low_signal`) learns per app/page which rung actually works and skips dead ones.
- **Window-scoped, not pid-scoped, when possible**: mouse/scroll background events route by `MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER`. Keyboard is inherently focus-scoped (a hard limit — see Gotchas).
- **Risk gating + audit**: mutating actions pass through `gate_raw_input` / `evaluate_action_gate`; approvals are element- or raw-scoped and count-limited. `audit_raw_input` re-perceives and diffs.
- **Idle-gated synthetic input**: anything that could fight the operator (cursor warp, background keys, the unstick maneuver) goes through `retry_user_active_guard`.
- **Deterministic graphs**: `BTreeMap`-backed nodes for stable diffs; structural `path` re-resolves duplicate controls.

## Git Flow

```
feat/* or fix/* ──► PR ──► main ──► (release-plz PR) ──► main ──► tag v*
```

- **Never commit or push directly to `main`.** All changes go through `feat/<topic>` or `fix/<topic>` branches + PRs.
- `main` is the only long-lived branch (GitHub Flow). release-plz watches `main` and opens a Release PR when releasable commits land.
- **Conventional commits**: `feat:` / `fix:` / `refactor:` / `perf:` bump versions via release-plz; `chore:` / `docs:` / `test:` / `style:` do not. Commit messages are in French.
- **Commits carry no AI/tool attribution trailers** — no `Co-Authored-By` bots, no "Generated with" lines, anywhere (commits or PR bodies).
- **Pre-commit hooks** via [prek](https://github.com/j178/prek): run `prek install` after cloning. `cargo fmt` / `clippy` / `gitleaks` on commit; `cargo deny` / `audit` / `machete` / doc-coverage / sweep on push.

## Commands

```bash
# Build
cargo build
cargo build --release
cargo install --path crates/dunst-mcp --force   # install the dunst-mcp binary

# Test / lint
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo machete                                   # unused deps
cargo deny check                                # licenses + advisories

# Run the MCP server against a live window (stdio JSON-RPC)
dunst-mcp serve --pid <pid> --window <window_id>
dunst-mcp                                       # default: device-free Notes fixture demo

# Doc coverage (enforced by the pre-push hook)
RUSTDOCFLAGS='-W missing-docs' cargo doc --no-deps
```

## Gotchas

- **Keyboard is focus-scoped, not window-scoped.** Posting keys to a pid routes them to the app's *focused* window. For a multi-window app (e.g. Firefox with your window + a target window), keyboard scroll/type on the background target **steals focus from your window**. There is no window-tagged keyboard path (the `windowNumber` NSEvent bridge routes the mouse but not the keyboard — tested, negative). Scroll such targets with `borrow_cursor`.
- **`borrow_cursor` moves the shared OS cursor** but does *not* touch keyboard focus. It is idle-gated and restores the cursor; a post-gesture **auto-unstick** (idle-gated) fixes the macOS shape-freeze bug. The manual `unstick_cursor` tool stays immediate for on-demand recovery.
- **Background wheel doesn't scroll every web surface.** Sparse-AX feeds (LinkedIn in Firefox) ignore a pid-routed wheel — even trackpad-phase/momentum variants — because the browser hit-tests the wheel under the *real* cursor. `borrow_cursor` is required there and is the accepted trade-off.
- **`focus_without_raise` shifts keyboard focus** even though it doesn't raise the window. Background *mouse* paths use it to make a web canvas paint; be aware it can disturb a sibling window's focus.
- **4 raw-approval tests are environmental flakes**, not regressions: a fixture `window_id 105` collides with whatever live desktop widget currently owns CG window 105. They fail identically on `main` and come/go with desktop state. Don't chase them.
- **The Python JSON-RPC driver scripts** in a scratchpad are only for live validation when the session's `mcp__dunst__*` tools are disconnected (e.g. after killing the server). Prefer the MCP tools; reconnect with `/mcp`.
