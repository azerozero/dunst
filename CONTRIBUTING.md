# Contributing

This repository is a Rust workspace for the Dunst MCP server and its macOS
automation backends. Keep changes small, contract-driven, and covered by the
closest test layer.

Read [`AGENTS.md`](AGENTS.md) (project map, domain concepts, gotchas) and
[`CLAUDE.md`](CLAUDE.md) (git/CI flow, documentation standards) first — they are
the canonical development guidelines.

## Local Checks

Install the git hooks once (they gate every commit and push):

```bash
brew install prek   # or: cargo install prek
prek install
```

Run the focused checks first:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p dunst-core -p dunst-graph -p dunst-mcp
```

Pre-commit runs fmt/clippy/gitleaks; pre-push runs cargo deny/audit/machete,
artifact sweep, and **doc coverage** — every public item needs a doc comment, so
`RUSTDOCFLAGS='-W missing-docs' cargo doc --no-deps` must be warning-free. Tests
are not in the push gate (CI runs them on macOS + Linux).

For platform work that touches macOS AX, ScreenCaptureKit, OCR, raw input, or
SkyLight routing, also run the relevant live smoke command from `scripts/` on a
machine with Accessibility and Screen Recording permissions enabled.

## Contract and Tests

Public behavior flows through:

```text
docs/CONTRACTS.md -> core/engine/platform code -> MCP schema -> tests/docs
```

When adding or changing an MCP tool, update the tool schema, dispatcher/registry,
engine behavior, tests, and README/operator docs together. For risky actions,
preserve approval gating and audit entries; raw pointer/keyboard paths must remain
explicitly gated.

## Setup Lifecycle

Use `setup` in dry-run mode before writing config:

```bash
cargo run -p dunst-mcp -- setup --client codex --dry-run
cargo run -p dunst-mcp -- setup --client claude --dry-run --dev-wrapper
```

Write project-local config only when the diff is expected:

```bash
cargo run -p dunst-mcp -- setup --client codex --apply
cargo run -p dunst-mcp -- setup --client claude --migrate
```

Use `--edit` to inspect the current file and the merged result without writing,
and `--config PATH` for tests or non-standard client paths. `--apply` writes the
`scripts/mcp-dunst.sh` wrapper when run inside a checkout (or with `--dev-wrapper`)
and `dunst-mcp serve` otherwise. Both `setup` and `doctor` take `--json` for
machine-readable output; `doctor` exits `0` (pass), `1` (warn), or `2` (fail).

## MCP Fixture Transcript

`docs/fixtures/mcp-transcript.jsonl` is a minimal device-free MCP transcript for
the bundled Notes fixture. Keep it in sync when initialization metadata, tool
names, or core response shapes change.

## Commit Style

Use Conventional Commits:

```text
feat: add scroll_at MCP tool
fix: preserve raw approval on user-active retry
test: cover setup apply lifecycle
docs: document branch protection limitation
refactor: route MCP tools through typed registry
```

`feat` / `fix` / `refactor` / `perf` trigger a release-plz version bump; use
`chore` / `docs` / `test` / `style` for non-release changes. Commit messages are
written in French. **Commits and PR bodies carry no AI/tool attribution** — no
`Co-Authored-By` bot lines, no "Generated with …" trailers.

## Branch Policy

`main` should be protected before release distribution. The current GitHub
repository cannot enforce rulesets from this environment because the rulesets API
returns `403`, and `main` is reported as unprotected. Until branch protection is
available, treat green CI plus manual review as the required merge gate:

- do not merge with failing required checks;
- do not bypass review for MCP schema, raw input, setup, release, or platform
  backend changes;
- record any intentional policy exception in the PR or review notes.
