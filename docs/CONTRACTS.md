# CONTRACTS — load-bearing invariants

Factual list of the behavioural guarantees the POC must keep. Each is locked by a
named test; if you change the behaviour, update the contract **and** the test in
the same change. Crates: `dunst-core`, `-graph`, `-mcp`, `-vision`.

## Risk gate

- **The gate is never bypassed without an explicit operator grant.** A high-risk
  mutating action returns `PendingApproval` and the executor/platform write path is
  **never** invoked on that path unless the action is covered by an explicit grant:
  a per-action `approve`, a batch approval, or a bounded raw pre-authorization.
  — `engine::tests::high_risk_click_is_gated_then_approved`,
  `find_element_and_gating_still_reach_latent_nodes`,
  `engine::tests::raw_input_gate_accepts_valid_synthetic_raw_approval`.
- **Approvals are validated, not blindly stored.** `approve(id)` errors unless `id`
  is genuinely gated: either it exists in the scene and its own risk requires
  approval, or it is the subject of a pending contextual/raw gate. A phantom id or
  a plain low-risk id is rejected.
  — `engine::tests::approve_rejects_unknown_and_non_gated_ids`,
  `engine::tests::synthetic_raw_preapproval_rejects_off_target_points`,
  `engine::tests::synthetic_raw_preapproval_rejects_unsupported_hotkeys`.
- **Element/contextual approvals are one-shot.** A grant authorises exactly one
  successful element-bound action; a second high-risk action on the same id
  re-gates.
  — `engine::tests::approval_is_one_shot_consumed_by_act`.
- **Element/contextual approvals never survive a re-perception.** Every
  `refresh()` clears element-bound grants and pending-gate markers.
  — `engine::tests::approval_is_invalidated_by_refresh`.
- **Composite drag risk.** A drag gates on `max(risk(source), risk(target))`: a
  high-risk drop target forces approval even when the source is low-risk.
  — `engine::tests::drag_onto_high_risk_target_is_gated_then_approvable`.
- **Composite type risk.** `type_into` gates on `max(risk(field), risk(typed
  text))`: a destructive payload gates an otherwise low-risk field.
  — `engine::tests::destructive_typed_text_gates_low_risk_field_and_is_approvable`.
- **Destructive and financial-commit labels gate.** The label/help/identifier
  keyword denylist classifies not only destructive/irreversible verbs
  (`supprimer`, `delete`, `révoquer`, `écraser`, `uninstall`, …) but also
  financial / external commits (`payer`/`pay`, `commander`/`checkout`,
  `acheter`/`buy`, `place order`) as HIGH → `requires_approval`, so an agent
  cannot place an order or pay on its own. The match is word-boundary-anchored so
  look-alikes (`payment`, `buyer`, `important`) stay LOW. It remains a heuristic
  denylist bounded by `approve` being off by default.
  — `risk::tests::broadened_denylist_gates_destructive_and_financial_commits`,
  `risk::tests::broadened_denylist_avoids_false_positives_on_lookalike_words`.
- **Raw mutating input risk.** Raw coordinate/key tools that can mutate UI state
  are high-risk because they are not bound to a scene element. The first call
  records `PendingApproval` and does not execute the platform input path.
  Approved raw grants are scoped, count-limited, and TTL-limited: exact pointer
  and text-entry targets (`type_keys` and `paste_text`) stay one-shot, repeated
  `press_key` approvals cover a short same-key burst, same-direction scroll
  approvals tolerate page-count changes, and hotkeys are limited to a short retry
  window. A single operator-only `keyboard@*` wildcard grant covers a short,
  event-limited burst of *any* keyboard raw action (`press_key`, `type_keys`,
  `paste_text`, `set_field_text`, hotkey, keyboard scroll) on the attached window,
  but never screen/pointer raw input. Raw grants survive ordinary `refresh()`
  calls but are cleared by `attach`, expiry, or grant exhaustion.
  — `engine::tests::raw_input_gate_accepts_valid_synthetic_raw_approval`,
  `engine::tests::raw_paste_text_approval_is_one_shot`,
  `engine::tests::raw_key_approval_allows_short_repeated_same_key_burst`,
  `engine::tests::raw_scroll_approval_covers_same_direction_count_change`,
  `engine::tests::keyboard_wildcard_approval_covers_short_keyboard_batch_only`,
  `engine::tests::keyboard_wildcard_approval_is_event_limited`,
  `engine::tests::attach_clears_raw_approval_grants`.
- **Raw pre-authorization is bounded on three axes.** `preauthorize` installs an
  operator grant that lets subsequent gated *raw* actions run without a per-action
  `approve`, but only while all three bounds hold: the attached `(pid, window_id)`, a spend
  budget (each raw action consumes one unit), and a TTL. It drops on window change,
  budget exhaustion, expiry, or `revoke_preauthorization`, and skips only the approval
  gate — geometry/target-window safety checks still run. It never clears batch,
  file-selection or high-risk element gates. Gated behind
  `DUNST_MCP_ENABLE_APPROVE_TOOL=1`, like `approve`.
  — `engine::tests::preauthorization_runs_a_bounded_burst_of_raw_actions_then_regates`,
  `engine::tests::preauthorization_is_scoped_to_the_attached_window`,
  `engine::tests::revoke_preauthorization_restores_gating`,
  `engine::tests::preauthorization_rejects_expiry_and_same_window_in_another_process`,
  `engine::tests::preauthorization_does_not_approve_batches_files_or_high_risk_elements`.
- **Batch selections are approved as one unit.** `apply_selections` records
  exactly one `PendingApproval` for a `batch@selections:<hash>:<n>` target whose
  preview carries per-step risk and an aggregate `max_risk`; a single operator
  `approve(batch_id)` authorizes the whole batch, the grant is one-shot, and the
  `BatchApprovalContext` is always cleared on exit so no later single action is
  silently authorized.
  — `engine::tests::apply_selections_first_call_is_pending_with_per_step_risk_preview`,
  `engine::tests::apply_selections_batch_grant_is_one_shot_resists_second_batch`.
- **Batch execution is epoch-guarded.** `apply_selections` refuses a plan whose
  `expected_epoch` no longer matches before mutation and re-scans only when a
  mid-batch structural reflow changes the UI fingerprint, re-resolving remaining
  steps by id then label/bbox; execution is bounded by `MAX_RESCANS` and the step
  budget.
  — `engine::tests::apply_selections_rescans_only_when_fingerprint_changes`,
  `serve::tests::stale_expected_epoch_refuses_apply_selections`.
- **Enumeration is read-only or survey-only.** `enumerate_choices` mutates no
  application data: the default mode reads without scrolling unless its single pass
  is incomplete on an AX-sparse surface, in which case it auto-runs one
  position-restoring `scroll_scan` sweep. `scroll_scan` (explicit or auto-upgraded)
  is mutation-coordinated but not operator-approval-gated and restores the original
  scroll position on live backends.
  — `engine::tests::enumerate_scroll_scan_restores_origin_and_sets_coverage_complete`.
- **Approval transport boundary.** `approve` is an operator-side interlock, not a
  default agent affordance. The MCP server does not advertise or execute the
  `approve` tool unless `DUNST_MCP_ENABLE_APPROVE_TOOL=1` is set for a controlled
  local session.
  — `serve::tests::approve_tool_is_disabled_by_default`.
- **Every attempt is audited.** Exactly one `AuditEntry` is appended per attempted
  action (gated or executed).
  — `engine::tests::every_attempt_is_audited`.
- **Known MCP sessions are carried into provenance.** When the server has a
  `SessionIdentity`, every appended `AuditEntry` carries it as `caller`, and MCP
  tool responses expose it under `_meta.dunst.session`. This is diagnostic
  provenance, not authorization.
  — `engine::tests::audited_attempts_include_session_identity_when_known`,
  `serve::tests::tool_call_results_include_session_identity_meta`,
  `serve::tests::initialize_result_includes_build_and_session_identity`.
- **Mutating/resource MCP tools are coordinated per session/window.** When the
  MCP server knows a `SessionIdentity`, mutating tools and read tools that borrow
  global UI resources acquire the global mutation lock and a TTL lease for the
  target `window_id` before dispatching. A different active session on the same
  window is refused; a stale `fencing_token` is refused; a supplied
  `expected_epoch` must match the current UI epoch before mutation. Pure
  read-only tools remain outside this coordination path.
  — `serve::tests::mutating_tool_adds_window_lease_and_fencing_meta`,
  `serve::tests::active_window_lease_blocks_other_session`,
  `serve::tests::stale_fencing_token_is_rejected_for_same_session`,
  `serve::tests::mutating_tool_rejects_stale_expected_epoch`.
- **The two mutation policies stay in lockstep.** A tool that acquires the
  mutation lock + window lease for some arguments
  (`tool_requires_mutation_coordination`) also advertises the
  `expected_epoch`/`fencing_token` preconditions in its schema
  (`tool_accepts_mutation_preconditions`), and vice versa — the only exception is a
  read-only/survey tool that coordinates without gating (`enumerate_choices`
  `scroll_scan`). Neither hand-maintained list can silently drift when a mutating
  tool is added without the other being updated.
  — `serve::tests::mutation_precondition_and_coordination_policies_agree`.
- **OS support is advertised by grouped platform capabilities, not inferred from
  the current target.** `dunst-platform` owns the platform-kind switch and
  exposes reusable groups for input, clipboard, perception/OCR/CV, windows, and
  apps. MCP callers use `platform_capabilities` instead of assuming macOS-only
  live-GUI features are present.
  — `dunst_platform::capabilities::tests::current_capabilities_match_current_platform`,
  `dunst_platform::capabilities::tests::macos_groups_related_capabilities_by_call_type`,
  `serve::tests::platform_capabilities_tool_reports_grouped_backend_surface`.
- **Native OS side effects stay behind platform adapters.** Clipboard paste,
  native file chooser driving, app launch/close, and real-cursor pointer paths
  are exposed through `dunst-platform` facades. MCP code gates, audits, and
  validates visibility, then calls the platform facade instead of owning
  OS-specific shell scripts or FFI directly.
  — `dunst_platform::clipboard::tests::paste_shortcut_uses_command_v`,
  `dunst_platform::file_chooser::tests::select_file_script_handles_native_panel_process_variants`,
  `dunst_platform::file_chooser::tests::select_file_script_compiles_as_applescript`.
- **Real-cursor actions require visible target pixels and preserve focus when
  possible.** `right_click_at` uses a real-cursor context-click because macOS
  positions context menus from the hardware cursor; borrowed-cursor reads move
  within a single borrow without re-running the user-active guard on their own
  synthetic moves; hover-reveal does not raise the target window.
  — (lock to be created) no test currently pins these behaviours: `right_click_at`'s
  real-cursor context-click, the intra-borrow skip of the user-active guard, and
  hover-reveal not raising the window are all unverified. `dunst-platform`
  `pointer_events` has no test module, and the `tools/list` schema test that was
  cited here proves none of them.

## Action verification

- **Typing verifies values, not labels or merely a diff.** Empty replacements and
  already-correct values are verified too. `effect_verified` carries the readback
  result even when nothing changed. Missing post-action perception cannot report
  a verified success.
  — `engine::tests::type_verification_handles_noops_clearing_and_rejects_matching_labels`,
  `engine::tests::failed_post_action_capture_cannot_verify_a_stale_value`,
  `serve::tests::typed_audit_reports_verified_noop_and_empty_replacement_without_a_diff`.
- **Settling does not replay actions or discard the initial state.** Clicks wait
  for delayed effects; audit diffs span pre-action to final state. An AX timeout
  (`CannotComplete`) is not a stale-handle retry signal: it may follow a completed
  action.
  — `engine::tests::click_waits_for_delayed_removal_without_replaying`,
  `engine::tests::type_into_waits_for_ax_value_to_settle`,
  `macos::ax_backend::tests::timed_out_actions_are_not_replayed_as_stale_handles`.

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
