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

## 7. Scroll clavier de fond = activation Firefox (toutes fenêtres remontées) — CORRIGÉ (2026-07-06)

`scroll` (pseudo-cible `page@scroll:*`, chemin `keyboard@scroll:...`) →
`key_web_background()` appelait `focus_without_raise()` avant CHAQUE frappe : la
recette SLPS défocus(0x02)+focus(0x01) active l'app cible au niveau window server,
et AppKit remonte alors TOUTES ses fenêtres au-dessus du premier plan de
l'utilisateur (observé : toutes les fenêtres Firefox raised pendant un simple
scroll top).

**Fix** : `skylight::focus_key_only()` (poste uniquement le record focus 0x01,
sans défocaliser le front process) utilisé par `key_web_background` à la place de
`focus_without_raise`.

**Complément (même jour)** : le symptôme persistait via les chemins souris/frappe
de fond — `click_web_background`, `hover_web_background_impl`,
`scroll_web_background_impl`, `type_text_background_impl` appelaient encore
`focus_without_raise` (observé pendant une session Collective : activation à
chaque clic/set_field_text). Les 4 sites sont passés à `focus_key_only` aussi.
`focus_without_raise` ne reste utilisé que par le chemin volontaire de raise/focus
(`dunst_platform::focus_without_raise`, outil focus_window).

**Audit approfondi (2026-07-06, les 2 fenêtres remontaient toujours)** — deux
causes réelles identifiées, `focus_key_only` seul ne suffisait pas :

1. `set_field_text` → `set_focused_field_text` → `paste_replace_field_foreground`
   exécutait un osascript `set frontmost … to true` : activation APP explicite à
   chaque appel, indépendante de tout focus record. **Fix** : reroute sur le
   chemin AX `type_text` (sélection via kAXSelectedTextRange — pas de Cmd+A,
   donc pas de piège keycode AZERTY — puis livraison background auth-signée).
2. Le record de focus 0x01 est posté au **PSN du process** (pas de la fenêtre) :
   même « key-only », chaque post peut réactiver l'app entière → AppKit remonte
   toutes ses fenêtres. **Fix** : `ensure_window_key_focus(pid, window_id)` ne
   poste le record que si `AXFocusedWindow` ≠ fenêtre cible — one-shot par
   changement de cible au lieu d'un effet de bord par événement.

Reste connu (hors scope, à traiter si gênant) : `scan_chart` et l'outil
`focus_window` utilisent volontairement `focus_without_raise` (activation
attendue) ; `navigate` active Firefox par design.
À VALIDER en live après rebuild + reload MCP : set_field_text et press_key en
rafale ne doivent plus faire remonter les fenêtres Firefox ; la livraison
(clics Chromium gate, Page/Home/End, frappe) doit toujours atteindre la cible.

---

# Audit 2026-07-22 — pilotage Firefox multi-fenêtres (saisine médiateur, formulaire web réel)

> Contexte : dépôt d'un dossier juridique sur `formulaire.mediation-assurance.org`.
> Firefox avec 2 fenêtres, l'une couverte par iTerm + Zen en plein écran.
> Session interrompue volontairement après corruption d'un champ (voir C1).

## C1 — `set_field_text` déclenche Cmd+Shift+A (gestionnaire de modules) — PRIORITÉ 0

Sur un `<input type=text>` d'un formulaire web (Firefox, AZERTY), l'appel a :
1. **ouvert un onglet « Gestionnaire de modules complémentaires »** → le repli clavier
   a produit **Cmd+Shift+A**, pas une sélection ;
2. **corrompu le champ** : `LIARDCLÉMENT` est devenu `LIARDCLÉMENTE` — pas de
   remplacement, un caractère ajouté.

C'est la famille du bug §0 (keycode lettre non mappé sur AZERTY), qui était censée
être corrigée par le reroutage sur `paste_replace_field_foreground` puis sur le
chemin AX `type_text`. Le repli clavier subsiste et reste dangereux.
→ Sur échec de la voie AX (`kAXSelectedTextRange` absent), **ne pas retomber sur un
raccourci clavier** : renvoyer une erreur explicite. Un remplacement raté est moins
coûteux qu'un raccourci imprévisible dans l'app hôte.
→ Cas de test : champ texte simple d'un formulaire web classique, clavier AZERTY.

## C2 — `type_keys` avale les tabulations — PRIORITÉ 2

`type_keys("LIARD\tClément")` a produit `LIARDCLÉMENT` **dans le même champ** : la
tabulation n'est pas convertie en frappe Tab, elle disparaît.
Conséquence : impossible d'enchaîner les champs d'un formulaire en un appel, il faut
un clic + une frappe par champ, soit 4 appels par champ avec le cycle d'approbation
(voir C6). Sur un formulaire de 8 champs : 32 aller-retours.
→ Soit convertir `\t` (et `\n`) en vraies frappes, soit le documenter et exposer un
`fill_fields([{selector|label, text}])`.

## C3 — `navigate` ignore la fenêtre attachée — PRIORITÉ 1

Attaché à la fenêtre 1106, `navigate` a systématiquement chargé l'URL dans la
fenêtre 100 (dernière fenêtre Firefox active), puis s'y est ré-attaché tout seul.
Testé aussi avec une URL rendue unique (`&zx=a1`) pour éviter la resélection d'onglet :
même résultat. La doc de l'outil affirme pourtant qu'il « force toujours un chargement
neuf, sans jamais resélectionner un onglet existant ».
→ Router l'ouverture vers `window_id` attaché, ou documenter que `navigate` cible
l'app et non la fenêtre.

## C4 — `list_browser_tabs` vide sur Firefox — PRIORITÉ 2

Retourne `[]` systématiquement ; `selected_tab` retombe sur
`tab_fallback_window_title`. Conséquence : aucun moyen de changer d'onglet par id,
donc le seul levier reste `navigate`… qui a le défaut C3. Les deux combinés rendent
le ciblage d'un onglet précis impossible dans une session multi-fenêtres.

## C5 — `expose_target_window` impuissant face au plein écran — confirme B1

`raised_within_app_only: true`, `visible_fraction: 0.0` inchangé, quand la cible est
couverte par des fenêtres **plein écran** d'autres apps (iTerm, Zen en 2560×1326).
`move_window_to_display(2)` déplace bien la fenêtre mais elle reste couverte par les
fenêtres de ce second écran.
→ Contournement trouvé et fiable : **ne pas chercher à exposer**. Le screenshot
composité et les clics ciblés fenêtre fonctionnent à 0 % de visibilité (validé sur
une dizaine d'actions). Voir C7.

## C6 — approbation à usage unique = ×2 appels par action — aggrave O5

Chaque action gatée exige : appel → `pending_approval` → `approve(target_id)` →
rappel identique. L'approbation ne couvre ni l'action suivante ni le même
`target_id` réutilisé. Mesuré sur ce formulaire : **~1 champ par minute**.
→ Proposer une portée d'approbation par session/fenêtre, ou un TTL, ou un mode
« formulaire » où l'opérateur pré-autorise une liste d'actions.

## C7 — `target_visibility` décourage à tort les clics fenêtre — PRIORITÉ 3

L'avertissement « target window is covered […] verify OCR/screenshot came from the
target before using visible coordinates » apparaît même quand l'action visée est un
clic **ciblé fenêtre**, qui fonctionne parfaitement en arrière-plan. Il m'a fait
perdre beaucoup de temps à tenter d'exposer la fenêtre (C5) alors que les clics
passaient déjà.
→ Distinguer dans le message : clic ciblé fenêtre = OK même couvert ; panneau natif
(sélecteur de fichiers, feuille d'enregistrement) = curseur réel requis.

## C8 — panneaux natifs : frontière à documenter — PRIORITÉ 3

Bloquant réel rencontré : impression Gmail → panneau d'impression Firefox
(« Enregistrer au format PDF ») puis feuille d'enregistrement macOS. Ni l'un ni
l'autre atteignable sans fenêtre visible + curseur réel. Idem pour le téléversement
de fichiers (`<input type=file>`).
→ Le documenter en tête d'`AGENT_GUIDE.md` comme limite dure, avec la parade :
faire faire ces étapes-là à l'opérateur.

## C9 — `get_hit_targets` dépasse la limite de tokens — confirme O3

Sortie par défaut : **80 KB / 2929 lignes** sur une page Gmail, tronquée par le
client MCP et déversée dans un fichier. Chaque cible répète bbox, zones de clic
sûres et modes d'action.
→ Défaut `limit` plus bas (20 ?) et champs verbeux derrière un `verbose: true`.

## C10 — `read_text(region)` muet là où l'OCR pleine fenêtre voit — PRIORITÉ 3

`read_text` avec une `region` de 260×120 sur une zone contenant des icônes a
renvoyé `[]`, alors que l'OCR pleine fenêtre trouvait bien du texte à proximité.
Mapping de région à vérifier (origine écran vs origine fenêtre ?).

## Ce qui a bien marché

- Screenshot composité sur fenêtre 0 % visible : impeccable, y compris multi-écrans.
- `click_near_text` avec `offset_x/offset_y` pour viser un champ à partir de son
  label : fiable, y compris sur un champ date segmenté.
- `find_ocr_text` + `occurrence` pour désambiguïser deux boutons « Oui » identiques.
- Champ date segmenté : `18/02/2026` ne remplit que l'année, **`18022026` marche**.
  À mettre dans le guide.

## C11 — `select_file` expire (12 s) et laisse un panneau natif orphelin — PRIORITÉ 1

Contexte : `<input type=file>` d'un formulaire web, Firefox sur l'écran secondaire
(fenêtre 1106 à x=2560), déclenchement par `x/y` sur le bouton IMPORTER.

Résultat : `action execution failed: select_file timed out after 12000 ms`.
Aucun fichier monté côté page. `list_windows(all=true)` montre ensuite un
**« Open and Save Panel Service » (pid 31852, window 1287, 304×330 à 3272,733,
`on_screen: false`)** : le panneau s'est ouvert mais n'a jamais été affiché ni
piloté, et il survit à l'échec.

Pistes :
- délai de 12 s trop court pour l'ouverture d'un panneau sur écran secondaire ;
- le panneau naît hors de la zone visible → le backend ne le trouve pas ;
- pas de nettoyage sur timeout : le service reste, ce qui peut gêner l'essai suivant.

Attendu : délai configurable, recherche du panneau par **pid de l'app hôte** plutôt
que par position, et fermeture du panneau (Échap) si le timeout est atteint.
Un message d'erreur indiquant « panneau ouvert mais introuvable » aiderait aussi —
le timeout seul laisse penser que rien ne s'est passé.

**Correction à C8** : la frontière « panneaux natifs hors de portée » était fausse,
`select_file` est prévu pour ça. Le blocage est un bug d'implémentation, pas une
limite de conception. Reformuler C8 en conséquence une fois C11 corrigé.
