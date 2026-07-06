# Documentation Index

This directory mixes current project documentation with historical work-package
notes from the VisualOps-to-Dunst bring-up. Treat this file as the routing map.

## Current

- `ARCHITECTURE.md` - current Dunst crate architecture and dataflow.
- `AGENT_GUIDE.md` - agent reading order, RTK commands, MCP/raw-input rules,
  and Firefox/AX live-debug gotchas.
- `CODE_NAVIGATION.md` - short reading path, module map, and edit zones.
- `CONTRACTS.md` - load-bearing behavioral invariants tied to tests.
- `BINARY_USAGE_REX.md` - return on experience for binary and live MCP usage.
- `P1-vision-surfaces.md` - current P1 vision surface plan.
- `reviews/` - dated cycle reports.

## Design & plans

Design docs live under `design/`; plans carry a dated status banner.

- `design/LLD-batch-choice-enumeration-selection.md` - as-built low-level design
  for the `enumerate_choices` / `apply_selections` batch tools (implemented, PR #4).
- `DISTRIBUTION_READINESS_PLAN.md` - distribution and release hardening plan
  (French, internal; partially delivered — see its status banner).
- `DUNST-UI-REMEDIATION-PLAN.md` - raw-input ergonomics remediation tracker
  (largely delivered — see its status banner).
- `P1-vision-rust-feasibility.md` - superseded pre-build feasibility study; the
  vision crate now ships on Core Graphics (see its status banner).

## Language

User-facing documentation is English. Internal planning and design notes may be
written in French and say so in their status banner (e.g.
`DISTRIBUTION_READINESS_PLAN.md`). Historical notes remain in the language they
were originally written in, including French, to preserve review context.

## Historical

The following files preserve implementation history. They may mention old
`visualops-*` crate names, old branch names, or findings that have already been
fixed. Use them for context only; do not treat their commands as current setup
instructions.

- `AUDIT-*.md`
- `FIX-*.md`
- `WP-*.md`
- `review-*.md`
