# CONTRACTS — load-bearing invariants

Factual list of the behavioural guarantees the POC must keep. Each is locked by a
named test; if you change the behaviour, update the contract **and** the test in
the same change. Crates: `dunst-core`, `-graph`, `-mcp`, `-vision`.

## Risk gate

- **The gate is never bypassed for mutating actions.** A high-risk mutating action
  returns `PendingApproval` and the executor/platform write path is **never**
  invoked on that path.
  — `engine::tests::high_risk_click_is_gated_then_approved`,
  `find_element_and_gating_still_reach_latent_nodes`,
  `engine::tests::raw_input_gate_requires_pending_synthetic_approval`.
- **Approvals are validated, not blindly stored.** `approve(id)` errors unless `id`
  is genuinely gated: either it exists in the scene and its own risk requires
  approval, or it is the subject of a pending contextual/raw gate. A phantom id or
  a plain low-risk id is rejected.
  — `engine::tests::approve_rejects_unknown_and_non_gated_ids`,
  `engine::tests::raw_input_gate_requires_pending_synthetic_approval`.
- **Approvals are one-shot.** A grant authorises exactly one successful action; a
  second high-risk action on the same id re-gates.
  — `engine::tests::approval_is_one_shot_consumed_by_act`.
- **Approvals never survive a re-perception.** Every `refresh()` clears all grants
  and pending-gate markers.
  — `engine::tests::approval_is_invalidated_by_refresh`.
- **Composite drag risk.** A drag gates on `max(risk(source), risk(target))`: a
  high-risk drop target forces approval even when the source is low-risk.
  — `engine::tests::drag_onto_high_risk_target_is_gated_then_approvable`.
- **Composite type risk.** `type_into` gates on `max(risk(field), risk(typed
  text))`: a destructive payload gates an otherwise low-risk field.
  — `engine::tests::destructive_typed_text_gates_low_risk_field_and_is_approvable`.
- **Raw mutating input risk.** Raw coordinate/key tools that can mutate UI state
  are high-risk because they are not bound to a scene element. The first call
  records `PendingApproval` and does not execute the platform input path; approval
  is one-shot for the synthetic raw target id.
  — `engine::tests::raw_input_gate_requires_pending_synthetic_approval`.
- **Approval transport boundary.** `approve` is an operator-side interlock, not a
  default agent affordance. The MCP server does not advertise or execute the
  `approve` tool unless `DUNST_MCP_ENABLE_APPROVE_TOOL=1` is set for a controlled
  local session.
  — `serve::tests::approve_tool_is_disabled_by_default`.
- **Every attempt is audited.** Exactly one `AuditEntry` is appended per attempted
  action (gated or executed).
  — `engine::tests::every_attempt_is_audited`.

## Scene-graph projection (WP-J)

- **`get_scene_graph` `full` (without `actionable_only`) is byte-identical** to the
  raw `SceneGraph` serialisation — the unchanged escape hatch.
  — `engine::tests::full_view_is_byte_identical_to_raw_scene_graph`.
- **`actionable_only` ⊆ total**, and `summary.n_actionable ≤ n_nodes`.
  — `engine::tests::summary_view_has_counts_and_roots_but_no_nodes`,
  `actionable_only_drops_latent_menu_items`.
- **Latent filter.** Listings omit latent (off-screen / zero-bbox) nodes by default,
  **except top-level menu openers** (direct children of the menubar root). The
  filter is read-only: `find_element` and the by-id risk gate still reach latent
  nodes; only the *listings* hide them. `include_latent` is a strict superset.
  — `engine::tests::query_affordances_excludes_latent_by_default_but_include_latent_keeps_them`,
  `top_level_menu_opener_listed_but_deep_submenu_item_filtered`.
- **`Role::as_str` equals the serde wire string** for every variant (so histogram
  keys / compact `role` never drift from the JSON encoding).
  — `core::types::role_tests::as_str_matches_serde_rename`.

## Coordinate transforms (`dunst-vision::coords`, pure / cross-platform)

- **Round-trip identity.** `vision_norm_to_screen_pt` and `screen_pt_to_vision_norm`
  are exact inverses (modulo f64 epsilon), both directions.
  — `coords::tests::round_trip_norm_screen_norm`, `round_trip_screen_norm_screen`.
- **Scale invariance.** The point-space result is independent of `backing_scale`
  (Retina 2× and non-Retina 1× agree).
  — `coords::tests::retina_and_non_retina_agree`.
- **ROI is always a valid unit-square sub-rectangle** (edge-clamped, never
  origin-shifted). OCR's `region_to_vision_roi` delegates here — one owner for the
  Y-flip + clamp.
  — `coords::tests::roi_clamps_partly_outside`, `ocr::tests::roi_delegates_to_coords_transform`.

## Known drift / not-yet-wired

- **Risk monotone in uncertainty (§10.7) is documented intent, not a guarantee.**
  The POC is AX-only (`OcrBox.confidence ≈ 1.0`) and `RiskEngine` does not read
  `confidence`. See the `TODO P1` on `dunst-vision::OcrBox`.
- **`_NS:` stable-id policy is a deliberate WP-D deviation** (`scene::is_appkit_auto`
  excludes AppKit auto identifiers from synth ids) — not a bug; do not "fix".
