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
