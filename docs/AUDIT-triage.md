# Audit cycle — triage & decisions (2026-06-09)

Distributed `cli-cycle`: Claude audited the 4 pure crates (`docs/AUDIT-cycle.md`),
Codex audited `visualops-platform` + the risk gate (`docs/AUDIT-platform.md`).
Headline: **0 functional/critical defect**, codebase healthy (clippy clean, 54
tests) — but Codex showed the **risk gate is not yet a real security barrier**.

## Applied now (delegated by crate)

| # | Finding | Crate | Owner |
|---|---|---|---|
| Gate-1 | Approvals: **one-shot, consumed in `act`, validated at `approve` (id exists & currently high-risk), invalidated on every `refresh`** | mcp | Claude |
| Gate-2 | `drag_element` gates **composite** risk `max(source, target)` (was source-only) | mcp | Claude |
| Plat-1 | `resolve_window` **strict** for `window_id != 0` (error if the window is gone; fallback only for the explicit `0` wildcard) | platform | Codex |
| Plat-2 | Cached fast-path **revalidates the element's window** (`_AXUIElementGetWindow == target.window_id`) + `AX_CACHE` namespaced by `(pid, window_id)` | platform | Codex |
| Plat-3 | `drag` posts a best-effort `LeftMouseUp` on cleanup if a down was emitted (no stuck mouse-down) | platform | Codex |
| Rob-1 | MCP `handle_tool_call` serialization `unwrap()` → `unwrap_or(Null)` (no server panic) + table-driven dispatcher tests | mcp | Claude |
| Vis-1 | Remove the dead `_screen_box` compute from the OCR hot path | vision | Claude |
| Vis-2 | Unify `ocr::region_to_vision_roi` onto `coords::window_rect_to_vision_roi` (kills the divergent clamp = the predicted bug #1) | vision | Claude |

## Deferred — real, but architectural / bigger than this pass
- **Out-of-band approval channel** (Codex #1): `approve` shares the agent's tool
  surface; a true barrier needs an operator-side capability/token, not just
  one-shot. Gate-1 mitigates the worst (no persistent/blind approvals) but the
  channel separation is a design change. **Next.**
- **Pre-action revalidation of a stale scene** (Codex #6): re-confirm
  id/role/label/bbox/risk against a live lookup before a mutating action.
- **`post_to_pid` has no ack** (Codex #11) + **refresh-after-action failure
  ignored** (Codex #7): a mutating action can read `Success` without observable
  effect → add `SuccessUnverified` / verify via diff.
- **Risk model beyond keywords** (Codex #14) + **typed-content risk** (Codex #13):
  gate the `Type` argument and use structural/contextual signals (menu position,
  native identifiers, app bundle), not just FR/EN label keywords.
- **Wire `confidence` into risk** (Claude #7): the "risk monotone in uncertainty"
  claim (§10.7) isn't implemented (POC is AX-only, confidence=1.0). Needed when
  vision lands.
- **`CONTRACTS.md` + stdio-serve integration test** (Claude #5, partial — we add
  dispatcher unit tests now; a full contract doc is later).

## Skipped for now — acceptable at POC / low value
- Cursor-restore vs a concurrent human moving the mouse (Codex #10) — log-only later.
- AX attr errors silently → `None` (Codex #18) — add debug counters later.
- `find_element` key-collision hardening (Codex #16), per-element AX timeout
  (Codex #8), `type_text` focus side-effect split (Codex #12).
- `find_element` projection/latent-filter parity (Claude #3) — deliberate: keep it
  exhaustive for discovery; revisit if payloads bite.
- README merged-example note (Claude #6), `Role::as_str` micro-opt (Claude #8),
  memoize `window_rect` (Claude #9), drop duplicate `graph_bench` (Claude #10),
  Cargo cross-platform note (Claude #11) — batch as polish later.
