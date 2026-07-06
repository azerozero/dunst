# POC -> Distributable CLI Transition Plan

Date: 2026-06-14
Project: `dunst-mcp` / `dunst-mcp`

> **Status (2026-07-06): internal planning note (English) - partially delivered.**
> Several priorities below have been delivered since 2026-06-14: the `clap` CLI
> with the `doctor`/`setup` subcommands, `release-plz`, `objc2` alignment between
> `dunst-platform` and `dunst-vision`, and the README converted into a user-facing
> landing page. Treat the remaining items as open backlog, not as a complete
> product status.

## Context

A broad project review was conducted across several audit tracks: code, documentation, packaging, shell, tangle, tests, and comparison with `grob`.

Shared conclusion: the Rust/MCP POC is technically interesting and fairly healthy, but it is not yet ready for clean Homebrew distribution. The main blockers are not a major architectural tangle; they are mostly in the product/distribution surface: action reliability, formatting, CLI contract, licenses, CI, packaging, setup/doctor, and user documentation.

The highest-priority issue discovered by the code audit is a false-success risk in background keyboard input: some functions can report `true` even if a keyboard event was only partially created or posted. This affects real MCP reliability and must come before Homebrew or README work.

## Audit Status

The audits are considered complete.

Agents that provided a usable synthesis:

- `audit_code`
- `audit_doc`
- `audit_packaging_surface`
- `audit_shell`
- `audit_tangle`
- `audit_test`
- `grob_compare`

The `idle` notifications mean that agents are available or waiting, not that they are still working.

Important: audit complete does not mean fixes complete.

## Priority 1 - Fix Background Keyboard Input Reliability

Goal: never return success if a background keyboard action partially failed.

Main files:

- `crates/dunst-platform/src/lib.rs`
- `crates/dunst-mcp/src/engine.rs`

Actions:

1. Inspect the `type_text_background` and `key_web_background` functions in `dunst-platform`.
2. Change the logic so it no longer returns success when an expected event pair was not created or posted correctly.
3. Propagate the failure on the `Engine` side as a clear MCP error instead of converting a false success into `Ok(())`.
4. Add or adapt tests if the code can be isolated without a live macOS dependency.
5. Run Rust formatting.

Verification:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Priority 2 - Clean Up Formatting

The audit confirmed that `cargo fmt --check` fails.

Action:

```bash
cargo fmt --all
```

Then verify:

```bash
cargo fmt --all -- --check
```

## Priority 3 - Stabilize the CLI Contract

Goal: make `dunst-mcp` a real installable CLI before discussing Homebrew.

Main file:

- `crates/dunst-mcp/src/main.rs`

Actions:

1. Replace manual `demo|serve` parsing with `clap`.
2. Add the following commands and help output:
   - `dunst-mcp --help`
   - `dunst-mcp --version`
   - `dunst-mcp demo`
   - `dunst-mcp serve --help`
   - `dunst-mcp doctor`
3. Preserve the existing `demo` and `serve` behavior as much as possible.
4. Add a minimal `doctor` command that diagnoses the environment and explains macOS/TCC prerequisites if not everything can be tested automatically.

Verification:

```bash
cargo run -p dunst-mcp -- --help
cargo run -p dunst-mcp -- --version
cargo run -p dunst-mcp -- serve --help
cargo run -p dunst-mcp -- doctor
```

## Priority 4 - Add Licenses and Cargo Metadata

Goal: make the manifest consistent with clean distribution.

Affected files:

- `Cargo.toml`
- `crates/dunst-mcp/Cargo.toml`
- repository root for licenses

Actions:

1. Add the license files corresponding to `MIT OR Apache-2.0`:
   - `LICENSE-MIT`
   - `LICENSE-APACHE`
   - optionally a synthetic `LICENSE` indicating the dual choice.
2. Add or complete the required Cargo metadata:
   - `description`
   - `repository`
   - `readme`
   - `keywords`
   - `categories`
   - `rust-version` if absent or incomplete.
3. Clarify the `publish = false` strategy or future publication plan.

Verification:

```bash
cargo metadata --no-deps
cargo package -p dunst-mcp --allow-dirty --no-verify
```

## Priority 5 - Add Minimal CI

Goal: turn local checks into an automatic gate.

Target file:

- `.github/workflows/ci.yml`

Minimal workflow:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --locked -p dunst-mcp
shellcheck scripts/*.sh
```

Do not add in the first foundation:

- release-plz
- Homebrew formula
- full cargo-deny/gitleaks/semgrep
- heavy mutation/fuzz testing

## Priority 6 - Separate Dev Wrapper and Installed Binary

Goal: prevent user configuration from depending on local paths such as `/Users/ludwig/workspace/...`.

Affected files:

- `scripts/mcp-dunst.sh`
- `.mcp.json`
- `.codex/config.toml`
- `README.md` later

Actions:

1. Keep `scripts/mcp-dunst.sh` as a development wrapper.
2. Harden the wrapper:
   - validate `DUNST_MCP_BIN`
   - validate `DUNST_MCP_MODE`
   - avoid silent fallbacks
3. Document that installed configuration should call the binary from `PATH`:

```json
{
  "mcpServers": {
    "dunst": {
      "command": "dunst-mcp",
      "args": ["serve"]
    }
  }
}
```

Do not yet implement a complete setup command that writes client configs, unless explicitly requested.

## Backlog After the First Foundation

Handle after the priorities above:

- Align the `objc2` versions between `dunst-platform` and `dunst-vision`.
- Replace predictable `/tmp` paths with `tempfile` or equivalent.
- Create a single declarative MCP registry to avoid schema/dispatcher/test drift.
- Rewrite the README as a user-facing landing page: quickstart, MCP config, TCC, troubleshooting.
- Add `setup --dry-run` / `setup --client codex|claude`.
- Add release-plz.
- Create a private Homebrew tap only once CLI, CI, licenses, and packaging pass.

## Immediate Non-Goals

- No massive refactor of `engine.rs` now.
- No full split of `dunst-platform/src/lib.rs` now.
- No immediate Homebrew formula.
- No immediate full release workflow.
- No complete Diataxis documentation in the first patch.

## Expected Result

After the first batch, the project should move from:

> usable local POC

to:

> reliable alpha CLI foundation, verified locally, ready to receive clean CI/packaging.
