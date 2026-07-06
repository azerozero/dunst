# Dunst Development Guidelines

Companion to [`AGENTS.md`](AGENTS.md) (project map, domain concepts, gotchas). This file is the **how we develop here**: layout, setup, git/CI flow, and documentation standards.

## Architecture

Dunst is an AX-first macOS MCP server for background UI automation. It perceives a target window into a stable scene graph, derives a risk-annotated affordance graph, and executes semantic actions through a least-intrusive capability ladder (AX → window-scoped background event → `borrow_cursor`), auditing every step with a before/after graph diff. See `AGENTS.md` for the full picture.

### Distribution

- **Binary**: `dunst-mcp`, an MCP stdio server (`dunst-mcp serve --pid <pid> --window <id>`).
- **Releases**: GitHub Releases via release-plz (auto-bumps on `main` push).
- **Not on crates.io** — the crates are internal workspace members, not published libraries.

### Module Layout

```
crates/
  dunst-core/      # types + traits, no macOS deps (the vocabulary)
  dunst-graph/     # pure pipeline: scene graph → affordances → risk
  dunst-platform/  # macOS backend: AX, SkyLight, pointer/web events, text input
  dunst-vision/    # capture + OCR
  dunst-mcp/       # engine (src/engine/) + MCP serve loop (src/serve/)
```

The engine (`crates/dunst-mcp/src/engine/`) is split by concern: `action*` (dispatch + gating), `raw_input*` (synthetic input + approval), `read`/`ocr_read`/`scene_query` (perception & query), `window_ops`/`window_geometry` (visibility & raising), `app_ops` (launch/navigate/tab reuse). Platform primitives live behind `dunst_platform::*` with non-macOS stubs so the workspace still builds on Linux CI.

## Local Setup

```bash
# Toolchain: stable Rust; the MSRV is the workspace rust-version (1.85 in Cargo.toml).
cargo build

# Install the git hooks (prek). Required before your first commit.
brew install prek          # or: cargo install prek / curl -fsSL https://prek.sh | sh
prek install

# Run the full local gate manually
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
```

Pre-commit hooks run `cargo fmt` / `clippy` / `gitleaks` / offline link check. Pre-push hooks run `cargo deny` / `audit` / `machete` / **doc-coverage** / `sweep`. Tests are **not** in the push gate (CI runs them on macOS + Linux); keep the local gate fast-fail.

## Git Flow & CI/CD

### Branching Model (GitHub Flow)

```
feat/* or fix/* ──► PR ──► main ──► release-plz PR ──► tag v* ──► release
```

1. **Feature branches** from `main`, named `feat/<topic>` or `fix/<topic>`. PR targets `main`.
2. **`main`** is the only long-lived branch. release-plz watches it and opens a Release PR when releasable commits land.
3. **Release**: the Release PR merges, release-plz tags `v*`, which triggers the release pipeline.

### Critical Rules

- **Never commit or push directly to `main`.** All changes go through feature branches + PRs.
- **Conventional commits required**: `feat:` / `fix:` / `refactor:` / `perf:` bump the version via release-plz; `chore:` / `docs:` / `test:` / `style:` do not. The prefix is the gate. Commit messages are in **French**.
- **No AI/tool attribution in history.** Commits and PR bodies carry **zero** `Co-Authored-By` bot lines and no "Generated with …" trailers.
- **Check file overlap before parallel PRs**: if two PRs touch the same files, base the second on the first branch (`git checkout -b feat/B feat/A`), not on `main`.

### CI Pipeline (`.github/workflows/`)

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | push to `main` / PR | fmt, clippy, tests (macOS + Linux), optional live smoke against a real app |
| `codeql.yml`, `semgrep.yml` | push / PR / schedule | static security analysis |
| `docs-lint.yml`, `shellcheck.yml` | push / PR | markdown link + shape checks, shell lint |
| `audit-cron.yml`, `nightly.yml` | schedule | advisory audit + nightly build |
| `release-plz.yml` | push to `main` | Release PR (version bump + changelog), then `v*` tag |
| `auto-merge-pr.yml`, `auto-merge-release.yml` | PR opened / updated | enable auto-merge for eligible PRs and release-plz PRs |
| `auto-update-branch.yml` | push to `main` | keep open PR branches current with `main` |
| `cleanup-branches.yml` | PR closed / weekly | delete merged and stale branches |

## Documentation Standards

Enforced by the `cargo-doc-coverage` pre-push hook: **every public item must have a doc comment** (`RUSTDOCFLAGS='-W missing-docs' cargo doc --no-deps` must be warning-free).

### Doc Comment Conventions (RFC 505 + RFC 1574 + Microsoft M-DOC)

- Every public item gets a `///` doc comment.
- Summary line in third-person present indicative ("Returns", not "Return"), ≤ 15 words.
- Never start with "This function/method/struct…".
- Standard sections in order: `# Errors`, `# Panics`, `# Safety`, `# Examples` (always plural).
- Examples use `?`, never `unwrap()`.
- Link related types with intra-doc `[`TypeName`]` syntax.
- Module `//!` docs give the high-level summary; types document themselves fully.

### Inline Comment Conventions (Clean Code + Linux Kernel)

- `//` comments explain **WHY**, not WHAT.
- Tags: `// TODO:`, `// FIXME:`, `// HACK:`, `// NOTE:`, `// SAFETY:` — capitalized, colon, space, end with a period.
- `// SAFETY:` is **mandatory** before every `unsafe` block (the codebase is FFI-heavy: AX, CoreGraphics, SkyLight). It states why the FFI contract is upheld.
- No commented-out code (git has history). No closing-brace comments.

### External Documentation

`docs/` is mostly flat, with a few subdirectories: `design/` (LLDs), `reviews/` (dated cycle reports), and `fixtures/` (device-free MCP/AX fixtures). `docs/README.md` is the routing map that separates **Current** references, **Design & plans**, and **Historical** work-package / audit notes. Add a new doc under `docs/` (or the matching subdirectory) and link it from `docs/README.md`; prefer updating an existing reference over adding a near-duplicate. Behavioural invariants that a test locks live in `docs/CONTRACTS.md`.

### What Goes Where

| Content | Location |
|---------|----------|
| API contract, usage, examples | `///` doc comments |
| Why this implementation approach | `//` inline comment |
| Safety justification for `unsafe` | `// SAFETY:` before the block |
| Safety contract for callers | `# Safety` in the doc comment |
| Behavioural invariant locked by a test | `docs/CONTRACTS.md` |
| Cross-cutting design / architecture note | a `docs/*.md`, linked from `docs/README.md` |
