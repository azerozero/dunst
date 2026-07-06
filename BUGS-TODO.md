# Bugs dunst-mcp — rencontrés en pilotant un champ web sparse-AX (Collective/Firefox)

> Après correctifs : `cargo install --path crates/dunst-mcp --force` + reload du serveur MCP.

## ⚠️ 0. CRITIQUE — ne JAMAIS coder en dur un keycode pour un raccourci lettre (layout !)

Un keycode **physique** ne mappe pas la même lettre selon le clavier. Sur **AZERTY**,
keycode `0x00` (= 'A' en QWERTY) = **'Q'** → un faux « Cmd+A » devient **`Cmd+Q` =
Quitter l'app**. C'est ce qui a **fermé Firefox le 2026-06-25**, et pourquoi les
« Cmd+A / Cmd+V » codés en dur (`select_all_and_paste_background`,
`clear_field_background`) ne sélectionnaient/collaient jamais juste sur ce poste.
Le garde « layout-sensitive » de l'outil `hotkey` (qui **refuse `cmd+a`**) avait
raison ; le contourner avec un keycode brut était l'erreur.
→ Code keycode-brut **reverté** (clipboard.rs / lib.rs / text_input.rs revenus à
l'original). Pour un vrai Cmd+A/Cmd+V indépendant du layout : traduire le
**caractère** via la keymap courante (`UCKeyTranslate` /
`TISCopyCurrentKeyboardLayoutInputSource`), jamais un keycode fixe.

**✅ Fix appliqué 2026-06-25** : `set_field_text` → `set_focused_field_text` appelle
maintenant `paste_replace_field_foreground` (`clipboard.rs`) : presse-papier +
`osascript` (`set frontmost of process` + `keystroke "a"/"v" using command down`,
**traduit par le layout courant**) → layout-safe, sélection **native** (donc pas de
queue 716/729), aucun keycode lettre codé en dur. ⚠️ foregrounde la fenêtre (pas
transparent) et requiert la permission **Automation → System Events** au 1er appel.
Ça résout aussi le bug #2 (queue) et #3 (frappes globales). Bug #4 (open_menu) reste.

## 1. `set_field_text` impossible à approuver — ✅ CORRIGÉ 2026-06-25

`approve(keyboard@set_field_text:<hash>:<len>)` répondait `not a recognised
synthetic raw-input target`. Le préfixe `keyboard@set_field_text:` n'était géré ni
dans `validate_synthetic_raw_approval` ni dans `raw_approval_policy`
(`raw_input_gate.rs`). Fix : branches ajoutées + `validate_set_field_text_target_id`
(mirror de `paste_text`). **Vérifié live : approve accepté.**

## 2. `set_field_text` laisse une queue DOM (React textarea) — ✅ FIX APPLIQUÉ (à vérifier live)

Sur une textarea React (Collective), `set_field_text` appliquait bien le nouveau
texte mais laissait un **fragment résiduel en fin de champ** (ex. `…OpenBao` +
`ouverain (st4ck).`), de façon **déterministe**, tout en renvoyant `success`
(l'attribut AX `AXValue` lisait le texte propre — l'artefact est **invisible à
l'AX**, niveau DOM uniquement).

Cause : `type_text_by_replacing_selection` (`text_input.rs`) sélectionne `{0,len}`
en AX puis **tape le texte caractère par caractère** (`post_window_bound_text` →
`type_text_background_impl`, 8 ms/char). La frappe synthétique dans un input
contrôlé React laisse un artefact DOM en queue.

Fix v1 (échec) : `set AXSelectedText = text` — l'attribut n'est **pas settable**
sur la textarea web Firefox → retombait sur la frappe (queue persistante, fragment
différent).
Fix v2 (appliqué) : **coller** le texte (`paste_text_background` = presse-papier +
**Cmd+V window-bound** + restore) après la sélection `{0,len}`. La frappe
caractère-par-caractère reste en fallback si le paste échoue. Atomique → pas de
race React → pas de queue. **À vérifier live.**

## 3. Champ web en arrière-plan ne reçoit pas les frappes globales — limitation (documenté)

`hotkey`/`press_key` (chemin clavier global) **n'atteignent pas** un champ web dans
une fenêtre backgroundée : `cmd+Down` a déclenché la recherche de la *page*
Collective, pas la navigation curseur du textarea ; `Backspace` n'a rien supprimé.
Seuls **(a)** la frappe *window-bound* (`type_text_background_impl`) et **(b)** l'AX
(`set_field_text`) atteignent réellement le champ. ⇒ Pas de récupération d'édition
par touches brutes (curseur+Backspace) sur ces champs : passer par l'AX.

## 4. `open_menu` n'ouvre pas le menu Firefox (multi-fenêtres) — ouvert

`open_menu("Édition")` → `failed` (item AX visible mais l'AXPress n'ouvre pas),
même après `focus_window`. Probable : Firefox multi-fenêtres / fenêtre cible pas
*key window*. Empêche le fallback « Édition → Tout sélectionner ». Basse priorité.

## 5. Pilotage LinkedIn (édition d'expériences) — notes + gotcha « formulaire vide »

Édition des 6 expériences du profil le 2026-06-25 (sync sur le rendu-final). LinkedIn
est **sparse-AX** comme Collective : crayons par-ligne, textarea Description et scroll
du modal **absents de l'arbre AX** → tout en raw (`click_at`) + `find_ocr_text` + molette
réelle (`scroll_at borrow_cursor=true`, fallback appris Firefox+LinkedIn).

**⚠️ Gotcha « formulaire vide » (≠ bug MCP)** : au 1er clic sur le crayon d'une
expérience, le **modal d'édition peut se charger VIDE** (champs en placeholder
« Ex. : chef des ventes au détail »), ce qui ressemble à une **création**. Ce n'en
est PAS une : c'est le **même form-id** (race de chargement LinkedIn). La coordonnée
était bonne. **Recharger la page** (`Cmd+R`) règle le glitch → le modal se rouvre
pré-rempli. **NE JAMAIS sauvegarder un modal aux champs requis vides** (ça écraserait
l'expérience). *Idée d'amélioration MCP* : sur ouverture d'un edit-form, détecter des
champs requis vides + avertir/retry au lieu de laisser croire à une création.

**Méthode fiable (vérifiée ×6)** :
1. `find_ocr_text("<Titre de l'expérience>")` → centre `(tx, ty)` de la ligne.
2. Crayon = `click_at(x≈3603 [bord droit de la carte], y=ty)`. Ne PAS deviner à
   l'aveugle un y approximatif (risque de taper le « + créer » de la section ou un
   hotspot inter-cartes).
3. Modal ouvert pré-rempli → `scroll_at(down, 1, borrow_cursor)` cadre la Description.
4. `click_at` dans la textarea → `pbcopy <bloc> | osascript Cmd+A + Cmd+V` (layout-safe).
5. « Enregistrer » → puis fermer **2 pop-ups** post-save : « vérifiez l'emploi »
   (`Passer`) pour les postes **actuels**, et « personnes que vous pourriez connaître »
   (`Ignorer`) à chaque save.

**Collage = lignes vides écrasées** (idem bug About) : LinkedIn supprime les lignes
vides au collage ; les blocs d'expérience n'en ont pas (puces consécutives) donc OK.

---

# Audit 2026-07-03 — pilotage iTerm backgroundé (session reti) : B1–B5 + O1–O6

> Diagnostic vérifié sur code + trace (`export_trace` de la session, 4 mutations).
> Ordre d'implémentation recommandé : **B2, B3, B1, O2, O3, B4, B5, O1, O4, O6, O5**.

## ✅ Statut — implémenté et validé en live le 2026-07-03 (fenêtre iTerm 74 backgroundée, couverte par Firefox)

Tous les items sont implémentés sur la branche `fix/audit-2026-07-03`. Vérifié en
live contre la vraie fenêtre (non-frontmost, couverte) :
- **B3** : un seul pane `focused` (au lieu de 4). ✓
- **O2** : `find_element` = 3,6 Ko (value tronquée) au lieu de 142 Ko. ✓
- **O4** : `export_trace` mode `summary` par défaut (~10 Ko au lieu de 77 Ko). ✓
- **B1** : `expose_target_window` renvoie `raised:true` cohérent quand la cible est
  réellement dégagée (100 % visible, non couverte), et `raised_within_app_only:true`
  sans mentir quand AXRaise n'a réordonné que dans l'app. Boucle de settle +
  critère `fully_visible` ajoutés.
- **B2** : raccourci équivalent-menu (⌥⌘↑ « Select Pane Above ») **résolu vers l'item
  de menu** (par `AXMenuItemCmdChar`, ou `AXMenuItemCmdVirtualKey` pour les touches
  nommées type flèches — jamais par keycode brut, cf. §0 AZERTY), puis **cliqué via
  System Events sur la fenêtre attachée préalablement remontée en key window** (raise
  window-scoped par id CoreGraphics via `_AXUIElementGetWindow`, robuste au titre
  volatil). Le pane change réellement (vérifié : bascule up puis restauration down),
  y compris quand la fenêtre attachée n'est pas la fenêtre frontmost de l'app.
  ⚠️ Compromis assumé : foregrounde l'app ~200 ms (impossible en pur background pour
  un équivalent-menu sur fenêtre non-key ; cf. §3/§4).

## B2 — `hotkey` renvoie success sans effet (fenêtre non-key) — PRIORITÉ 1

Chemin : `keyboard.rs:693 hotkey()` → `key_web_background` (`web_events.rs:403`) →
`SLEventPostToPid` (`skylight.rs:149`). Le post est **scopé PID**, pas fenêtre :
AppKit route l'équivalent-clavier vers la *key window*, inexistante quand l'app est
backgroundée. `focus_without_raise` (`skylight.rs:77`) ne crée pas de key window.
`success` = « évènement posté » (`raw_input_gate.rs:445-449`), aucune vérif d'effet.
Preuve trace : 271 changements de graphe = revalidation des menus, zéro effet pane.

Fix :
1. **Résolveur combo→item de menu, AXPress element-bound** (marche backgroundé,
   `ax_backend.rs:281`) :
   - capter `kAXMenuItemCmdCharAttribute` + `kAXMenuItemCmdModifiersAttribute`
     (+ `kAXMenuItemCmdVirtualKeyAttribute`) dans `WalkAttributes` (`ax_tree.rs:16-29`) ;
   - propager `cmd_char`/`cmd_modifiers` dans `RawAxNode` (`dunst-core/types.rs:42`),
     `assemble_node` (`ax_tree.rs:90`), `flatten` (`dunst-graph/scene.rs:213`) ;
   - dans `hotkey()` : si un `Role::MenuItem` matche le combo → `AXPress` via le
     chemin élément ; fallback `key_web_background` sinon.
   ⚠️ matcher par **caractère** (cmd_char), jamais par keycode (cf. section 0 AZERTY).
2. **Filet** : détection low-signal post-action comme `scroll` l'a déjà
   (`scroll_result_low_signal` `keyboard.rs:788`, `low_signal_menu_id` :810) : si le
   diff n'est que du churn `mi_`/`menu_`, renvoyer `effect_verified:false`/low-signal
   au lieu de `success` sec.

## B3 — focus AX inutilisable (4 AXTextArea `focused=true`) — PRIORITÉ 1

`ax_tree.rs:160` (batch), `:192` (single), `:122` (shallow) lisent le `kAXFocused`
**auto-déclaré par élément** ; iTerm répond `true` pour chaque pane. Le seul usage
d'`AXFocusedUIElement` du codebase est `set_focused_field_text`
(`ax_backend.rs:397-402`) — jamais consulté pour le scene graph.

Fix : dans le walk (`walk_element` `ax_tree.rs:134`), lire une fois
l'`AXFocusedUIElement` de l'app (+ `kAXFocusedWindow`), et ne poser `focused=true`
que sur l'élément correspondant (comparer via `element_key`/`ElementKey`
`ax_tree.rs:326`). Résultat attendu : un seul `focused` par fenêtre, exploitable.

## B1 — `expose_target_window` ment (`raised:true`, `visible_fraction` 0.0) — PRIORITÉ 2

Ce n'est PAS SkyLight : le raise est **AXRaise** (`ax_backend.rs:243-246`), qui
réordonne dans l'app **sans activer le process** → l'app frontmost (Firefox) reste
devant, le z-order CoreGraphics ne bouge pas, `visible_fraction` (calcul correct,
`window_geometry.rs:185`) reste 0.0. `raised` = « l'appel AX a réussi »
(`window_ops.rs:355`), jamais réconcilié avec le `after` pourtant recalculé (:365).
Le fallback arrange est gaté par `arrange_if_needed=false` par défaut
(`dispatch/window_app_tools.rs:53-54`). Preuve trace : le diff du raise ne contient
que le spinner braille ⠂→⠐ et le texte vivant.

Fix :
1. `ax_backend.rs:243` : après AXRaise, activer réellement le process — le combo
   éprouvé existe déjà dans `file_chooser.rs:120` (`set frontmost of process`) +
   `:127` (AXRaise). Garder ça derrière le gating d'approbation existant (raise est
   déjà high-risk).
2. `window_ops.rs:355-366` : réconcilier — `raised = ax_ok && (after.is_frontmost
   || after.visible_fraction > before.visible_fraction)` ; sinon renvoyer
   `raised_within_app_only:true` + hint explicite.

## O2 — `find_element` renvoie le `value` entier (scrollback 142 KB) — PRIORITÉ 2

`find_matches_value` (`serve.rs:519`) sérialise les `SceneNode` entiers ;
`SceneNode.value` (`dunst-core/types.rs:186`) n'a aucun cap. La seule troncature
existante est `DIFF_SUMMARY_VALUE_LIMIT = 160` (`serve.rs:44`), diff-only.
`compact_node` (`scene_query.rs:404`) recopie aussi le value entier (:411-413).

Fix : projeter dans `find_matches_value` avec `value` tronqué (~200 chars) +
`value_len` ; param opt-in (`full_value=true`) pour l'intégral. Corriger aussi
`compact_node`.

## O3 — `graph_diff` dominé par le churn de menus (47 KB / 271 changements) — PRIORITÉ 2

Preuve trace : 206/271 changements du hotkey = menus (`parent` ×50, `enabled` ×156).
`include_diff` est déjà `false` par défaut et un résumé compact est toujours émis
(`response.rs:45-60`) — cette moitié d'O3 est faite. Mais `low_signal_diff_change`
(`response.rs:389`) ne couvre que `mi_menuitemhit_*`, `intercom`, et `bbox` sur
`grp_/el_/img_` : le re-parenting des menus compte comme *meaningful*.

Fix : étendre `low_signal_diff_change` à `Changed{field ∈ {parent, children}}` (et
`enabled`) sur ids `menu_`/`mi_` ; appliquer le filtre aussi au `graph_diff` complet
quand `include_diff=true` (response.rs:55-60), pas seulement au sample du résumé.

## B4 — `find_element` incohérent sur les ids latents — PRIORITÉ 3

L'id EST cherché (`engine.rs:324-331`), mais `normalized_contains_query`
(`query_support.rs:76`) impose une **frontière de mot** aux requêtes mono-token
alphanumériques ≥4 chars : `selectsessionatindex` est rejeté car suivi de `action`
dans `mi_selectsessionatindexaction` (frontière après = alnum), alors que « Select
Pane Above » (espaces) court-circuite le garde à `query_support.rs:84`. Cause
additionnelle : aucune rétention inter-snapshots des `mi_*` dynamiques
(`build_scene_graph` `scene.rs:163` reconstruit from scratch).

Fix : exempter le champ `id` de la contrainte mot-entier (simple `contains` sur
`normalize_match(&n.id)`, `engine.rs:325`) ; ne pas régresser
`find_element_prefers_exact_button_label_over_containing_help_text`. Optionnel :
overlay des nœuds menu récents pour la résolution d'id.

## B5 — ids dérivés des labels instables (titres de panes vivants) — PRIORITÉ 3

`synth_id` (`scene.rs:59`) : `ax_identifier` stable → sinon **slug(label)** (:77-78)
→ sinon `path_hash` (:81, seulement si label vide). Les panes iTerm n'ont pas
d'identifier stable → branche label → l'id mute avec le titre. Preuve trace :
`txt_recuperer_le_retour_de_la_session_preced` → `txt_default_dunst_mcp` dans le
diff du hotkey. L'ancre stable existe déjà : `path_hash` (FNV-1a rôle+position,
`scene.rs:151`), et le fingerprint d'epoch (`read.rs:330`, commits 64d706c/e14c9ca)
prouve que rôle+path suffit sans le label.

Fix : dans `synth_id`, préférer `{prefix}_{path_hash(path)}` pour les nœuds à label
volatil (au minimum : rôles texte/fenêtre sous une app terminal ; ou heuristique
générique). La passe G3 de réconciliation du diff (`audit.rs:50-83`) masque déjà le
churn au niveau diff mais pas au niveau id — c'est l'id qu'il faut stabiliser.

## O1 — `target_visibility` répète les objets fenêtre + display — PRIORITÉ 3

`TargetVisibility.covered_by: Vec<DesktopWindow>` (`types.rs:57/64`) où chaque
`DesktopWindow` (:477) porte un `DisplaySummary` complet (:487) ; `WindowView` a en
plus son propre `display` top-level (:453) ; `ExposeTargetWindowResult{before,after}`
(:236) = deux snapshots entiers.

Fix : struct couvrante allégée `(app, window_id, title, bounds, z_order)` construite
dans `window_geometry.rs:120-135` ; display une seule fois au top-level ; pour
expose, renvoyer `after` + delta au lieu de before+after entiers.

## O4 — `export_trace` embarque les graph_diff pleins (77 KB / 4 actions) — PRIORITÉ 3

`action_resolution.rs:156` sérialise `Vec<AuditEntry>` entier ; l'outil n'a **aucun
paramètre** (`tools.rs:689`) ; dispatch sans args (`read_tools.rs:382`).

Fix : mode `summary` par défaut (remplacer chaque `graph_diff` par l'équivalent de
`diff_summary_value`, réutilisable depuis `response.rs:316`) ; `index` (ts/action/
result seulement) ; diff complet par index à la demande (`entry=N`).

## O6 — boutons Electron sans label — PRIORITÉ 4

Dériver un label du subrole AX (`AXCloseButton` → « close », etc.) quand le label
est vide, au niveau du walk ou de `synth_id`/labeling (`scene.rs`), pour une sortie
`get_affordances` auto-descriptive.

## O5 — cycle d'approbation ×3 par action gatée — PRIORITÉ 4 (design)

Aujourd'hui : tentative → `pending_approval` → `approve` → retry (×2 latence).
Concevoir un scope de pré-autorisation par session/cible (ex. `approve` acceptant
`keyboard@*` pour la fenêtre attachée, TTL court) et/ou un `approve_and_execute`.
Point d'entrée : `raw_input_gate.rs` (`validate_synthetic_raw_approval`,
`raw_approval_policy`) + `serve/coordination.rs`. À faire en dernier, design à
documenter dans le commit.

## 6. Popup native de `<select>` inatteignable en clic synthétique — CORRIGÉ (2026-07-06)

Rencontré sur lacartedescolocs.fr/Firefox (champ « Meublée ») : le 1er clic ouvre
la popup native du `<select>`, mais le 2e clic (posté `SLEventPostToPid` vers la
fenêtre attachée) ne l'atteint jamais — la popup est une AUTRE fenêtre (parfois un
autre process, ex. « Open and Save Panel Service ») → le menu se referme sans
sélectionner. Idem `pick_option` : les items ne sont pas dans l'arbre AX de la
cible, et un AXPress sur item latent/étranger échoue (cf. #4).

**Fix** : primitive `click_at_point_cursor` (warp + hover + LeftDown/Up **globaux
HID** + restore, miroir de `right_click_at_point_impl`) dans
`pointer_events.rs`, exposée en opt-in `borrow_cursor=true` sur `click_at` et
`click_near_text` (même recette que `scroll_at borrow_cursor`). Contournement
utilisé avant le fix : ouvrir le select puis `press_key Down` + `Return` (le
clavier atteint le menu-tracking), ou AppleScript System Events.
