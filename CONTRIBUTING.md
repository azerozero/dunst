# Contributing

This repository is a Rust workspace for the Dunst MCP server and its macOS
automation backends. Keep changes small, contract-driven, and covered by the
closest test layer.

## Local Checks

Run the focused checks first:

```bash
cargo fmt --check
cargo test -p dunst-core -p dunst-graph -p dunst-mcp
```

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
and `--config PATH` for tests or non-standard client paths.

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

## Branch Policy

`main` should be protected before release distribution. The current GitHub
repository cannot enforce rulesets from this environment because the rulesets API
returns `403`, and `main` is reported as unprotected. Until branch protection is
available, treat green CI plus manual review as the required merge gate:

- do not merge with failing required checks;
- do not bypass review for MCP schema, raw input, setup, release, or platform
  backend changes;
- record any intentional policy exception in the PR or review notes.
