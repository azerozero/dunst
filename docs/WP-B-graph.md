> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-B — `visualops-graph` (pure logic)

**Owner:** Claude (tmux 9). **Crate:** `crates/visualops-graph` only.
**No macOS, no device, no network.** Test against `MockPerceptor::notes_fixture()`.

Read `docs/ARCHITECTURE.md` first. Do **not** edit `visualops-core` or
`visualops-platform`. Do **not** run `git`. Keep `cargo test -p visualops-graph`
green.

You implement four `todo!()`s. Signatures are already in place — fill the bodies.

---

## 1. `scene::map_role(ax_role) -> Role`

Map native roles. At minimum:

```
AXButton->Button  AXMenuButton->MenuButton  AXTextField->TextField
AXTextArea->TextArea  AXCheckBox->Checkbox  AXRadioButton->Radio
AXRow->Row  AXCell->Cell  AXMenuItem->MenuItem  AXMenu->Menu
AXMenuBar/AXMenuBarItem->MenuBar  AXList->List  AXTable->Table
AXOutline->Outline  AXWindow->Window  AXToolbar->Toolbar
AXStaticText->StaticText  AXImage->Image  AXGroup->Group
everything else -> Unknown
```

## 2. `scene::synth_id(role, label, path, used) -> String`

Stable, human-readable, **unique within the graph**, deterministic.

- With label: `"{role.id_prefix()}_{slug(label)}"`, e.g. `Button` + `"Nouvelle note"`
  → `btn_nouvelle_note`. Slug = lowercase ASCII; spaces/punct → `_`; strip
  accents (`é`→`e`, `à`→`a`, …); collapse repeats; trim `_`; cap ~40 chars.
- Without label: `"{prefix}_{hash}"` where `hash` is a short stable hex of the
  structural `path` (the child-index chain from the root), e.g. `text_a1b2`.
- Collision: if the candidate is already in `used`, append `_2`, `_3`, …

## 3. `scene::build_scene_graph(roots, window, now_ms) -> SceneGraph`

Recursively flatten the `RawAxNode` forest:

- DFS; track the `path` (Vec of child indices) for `synth_id` and for hashing.
- For each node: `synth_id` (passing the running `used` set), `map_role`, copy
  `label/help/value/bbox/enabled/focused/ax_actions/ax_identifier`,
  `confidence = 1.0`, `source = Accessibility`, `last_seen_ms = now_ms`.
- Wire `parent` (id of parent or `None` for roots) and `children` (ids, in
  order). Push root ids to `roots` in order.
- `captured_at_ms = now_ms`, `window = window`.

## 4. `risk::RiskEngine`

`new()` builds keyword tables; `assess(node)` matches against
`label + " " + help + " " + ax_identifier` (lowercased, accent-stripped).

- **HIGH** (`requires_approval = true`): `supprimer, delete, effacer, remove,
  éteindre, shut down, redémarrer, restart, forcer à quitter, force quit,
  réinitialiser, reset, déconnexion, log out, formater, erase, vider, empty
  trash`.
- **MEDIUM**: `envoyer, send, publier, publish, deploy, déployer, enregistrer,
  save, coller, paste, déplacer, move, renommer, rename, partager, share,
  archiver`.
- **LOW** otherwise (`requires_approval = false`).
- `reasons` lists the matched keyword(s), e.g. `["matched keyword: supprimer"]`.
- Highest tier wins if multiple match.

## 5. `affordance::map_action(ax_action) -> Option<SemanticAction>`

```
press->Click  showmenu->OpenMenu  pick->Pick  raise->Raise
confirm->Click  cancel->None  increment/decrement->None
showdefaultui/showalternateui->None  zoomwindow->None
```
(anything unmapped → `None`.)

## 6. `affordance::derive_affordances(graph, risk) -> AffordanceGraph`

For every node:

- Map each `ax_action` → semantic action; dedupe; keep stable order.
- If role is `TextField` or `TextArea`, also expose `Type` (and `Focus`).
- `drag_targets`: only for `Row`/`Cell` nodes — list sibling `Row` ids and any
  ancestor `List`/`Table`/`Outline` id (POC heuristic). Add `Drag` to actions
  when `drag_targets` is non-empty.
- Attach `risk.assess(node)`.
- Skip nodes with no actions **and** Low risk? No — include every node so the
  graph is complete; nodes with empty actions are fine.

---

## Done-criteria (write these tests in the crate)

Use `visualops_core::mock::MockPerceptor::notes_fixture()` →
`perceptor.capture(&Target{pid:1363,window_id:105})` to get roots, then build.

1. `build_scene_graph` produces a node with id `btn_nouvelle_note`, role
   `Button`, label `"Nouvelle note"`, `confidence == 1.0`.
2. All synthesised IDs are unique.
3. The text area node exposes `Type` in its affordance actions.
4. Risk: `Supprimer`, `Éteindre`, `Forcer à quitter Notes`, `Redémarrer…` →
   `High` + `requires_approval`. `Copier`, `Nouvelle note` → `Low`.
5. The `Notes` `AXCell`/`AXRow` has non-empty `drag_targets` and `Drag` action.
6. `diff`: clone the graph, change one node's `value`, assert exactly one
   `NodeChange::Changed { field: "value", .. }`; add/remove a node → `Added`/
   `Removed`. Timestamp-only differences produce **no** changes.

Run: `cargo test -p visualops-graph`. Leave it green. Ping architect when done.
