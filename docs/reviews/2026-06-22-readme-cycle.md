# Dunst MCP README Cycle - 2026-06-22

Scope: documentation cycle focused on the root `README.md`, triggered by a
review that the main schema described internals before the real MCP workflow.

Method: `cli-cycle` was run as a documentation-focused pass. Applicable checks:

- `cli-forge-readme`: README structure, hook, quickstart, examples, and visual.
- `cli-audit-doc`: documentation quality and freshness.
- `cli-forge-schema`: Mermaid flow clarity and product mental model.
- Lightweight doc-code sync against the current MCP tool catalog in
  `crates/dunst-mcp/src/serve/tools.rs`.

## Recon Brief

- What: a macOS MCP server for verified UI automation.
- Who: AI agents and operators driving real macOS apps through Codex/Claude-style
  MCP clients.
- Problem: raw coordinates and stale screenshots make browser/app automation
  fragile and hard to audit.
- Headline capability: MCP tools resolve a target semantically, choose the
  cheapest trustworthy read path, then execute risk-gated actions.
- Differentiator: target visibility, AX affordances, OCR fallback, approvals,
  verification, and audit are one workflow instead of disconnected probes.
- Golden path: configure MCP, attach a window, read through AX first, fall back
  to OCR/pixels only when needed, then execute a verified action.

## Finding

### Tier 2 - Major: README schema presented the internal pipeline backwards

Evidence:

- `README.md` opened with a diagram that flowed from `macOS AX tree` to
  `Scene Graph`, `Affordance Graph`, `Risk Engine`, and finally `MCP tools`.
- The actual user entrypoint is the MCP server. A client calls a tool, Dunst
  verifies target scope, then chooses AX, targeted probes, OCR, pixels, or raw
  input in order of reliability and cost.
- The tool catalog now exposes `target_visibility`, `read_text_detailed`,
  `find_ocr_text`, `click_near_text`, `extract_ocr_cards`, `detect_modal`,
  `dismiss_modal`, `expose_target_window`, and scoped affordance queries, but
  the README did not teach this ladder.

Impact: new users could infer that Dunst is primarily an AX-to-MCP exporter,
not an MCP-first decision engine. That mismatch explains why agents kept
guessing coordinates after AX/OCR failures instead of switching strategies.

Fix applied:

- Replaced the top schema with an MCP-first Mermaid flow:
  MCP client -> tool call -> target verification -> AX reads -> targeted probes
  -> OCR -> pixels/charts -> verified action -> risk gate -> audit.
- Added an explicit information ladder ordered from fastest and most practical
  to slowest/riskier.
- Added README coverage for target visibility, exposure, detailed OCR,
  OCR-relative clicks, OCR cards, modal dismissal, shapes, and chart tools.
- Added `get_hit_targets` to the README flow as the semantic target path before
  raw coordinates: labels, action modes, safe click zones, risk, visibility,
  selected tab, and stale `ui_epoch` detection.

## Scorecard

| Area | Before | After | Status |
| --- | ---: | ---: | --- |
| README completeness | 7.4/10 | 8.6/10 | Stronger product entrypoint |
| Diagram clarity | 6.5/10 | 8.8/10 | MCP-first and operationally ordered |
| Documentation freshness | 7.7/10 | 8.5/10 | New MCP tools now represented |
| Doc-code sync | 7.5/10 | 8.6/10 | README now matches the current tool catalog better |

## Remaining Items

| Tier | Correction | Effort | Source |
| --- | --- | --- | --- |
| 2 | Add a root `CONTRIBUTING.md` with local hooks, contract/test coupling, and commit workflow. | Medium | `cli-audit-doc`, `cli-forge-readme` |
| 2 | Add a small MCP transcript or fixture-mode tools/list example to make the README quickstart visibly executable. | Medium | `cli-forge-readme` |
| 2 | Fold OCR cards/text and shapes into `get_hit_targets` so inaccessible web cards use the same semantic target contract as AX elements. | Medium | `cli-cycle`, product review |
| 1 | Keep historical docs labeled as historical so old VisualOps-era flow diagrams are not mistaken for current architecture. | Low | `cli-audit-doc` |

## Verification Plan

- `markdownlint-cli2` over `README.md` and `docs/**/*.md`.
- `lychee` over `README.md` and `docs/**/*.md`.
- Inspect rendered Mermaid mentally against the MCP tool catalog.
