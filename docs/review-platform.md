# Review — `crates/visualops-platform` (backend macOS AX)

Revue **lecture seule** du backend FFI macOS (`src/lib.rs` module `macos`,
`examples/dump.rs`). Aucun fichier `.rs` modifié. Référentiels : le contrat
`visualops-core` (gelé, non modifié) et `docs/WP-A-platform.md` (spec du WP).

Méthode : trace de l'ownership Core Foundation (Create/Copy → CFRelease),
vérification des `unsafe`, des null-checks, de l'exactitude des types
`AXValueGetValue`, de la traversée, du mapping des `SemanticAction`, des bornes
et de la conformité au contrat. La sémantique exacte des helpers
`core-foundation 0.10` (`downcast`, `downcast_into`, `wrap_under_get_rule`,
`wrap_under_create_rule`) a été relue dans la source du crate pour fonder
l'analyse retain/release. Le crate **compile sans aucun warning** sur cette
machine (`cargo build`/`clippy -p visualops-platform`), donc les types FFI
(`AXValueGetValue -> bool`, constantes `kAXValueType*`) sont cohérents.

## Synthèse de l'ownership (ce qui est correct)

Pour éviter les faux positifs, voici ce qui est **sain** :

- `attr_value` (l.346) : `AXUIElementCopyAttributeValue` (Copy) → `value`
  enveloppé par `wrap_under_create_rule` ⇒ libéré au `Drop` du `CFType`. Le
  null-check (`err == kAXErrorSuccess && !value.is_null()`) est correct.
- `attr_string`/`attr_bool` (l.359/367) : `downcast::<T>()` suit la Get Rule
  (+1 indépendant, libéré au Drop) ; le `CFType` source est libéré séparément.
  **Équilibré, pas de fuite, pas de double-free.**
- `attr_array` (l.372) : `downcast_into::<CFArray>()` consomme le `CFType` sans
  toucher le compteur (transfert) ⇒ le `CFArray` est libéré à son Drop.
  **Correct.**
- `action_names` (l.388) : `AXUIElementCopyActionNames` (Copy) →
  `wrap_under_create_rule` ⇒ libéré. `cf_strings` (l.407) utilise
  `wrap_under_get_rule` (+1) puis Drop (−1) par élément. **Équilibré.**
- `attr_ax_element`/`attr_ax_value` (l.377/500) : `mem::forget(value)` après
  test de type transfère correctement le +1 au pointeur brut renvoyé ; sur le
  chemin d'échec de type, le `CFType` est bien libéré (Drop). Le **transfert**
  est correct — le défaut est en aval (cf. BLOQUANT/MAJEUR : le destinataire ne
  libère jamais).
- Types `AXValueGetValue` : `kAXValueTypeCGRect/CGPoint/CGSize` appariés aux
  buffers `CGRect`/`CGPoint`/`CGSize` (`core-graphics`, `repr(C)`, champs
  `f64`/CGFloat) — tailles 32/16/16 octets exactes. `then_some` confirme un
  retour `bool`. **Pas d'UB.**
- Aucune panique sur attribut manquant : `Option` + `unwrap_or(_else)` partout
  (l.195, 215, 216…). Conforme au done-criteria « No panics ».
- Bornes `MAX_NODES`/`MAX_DEPTH` (l.51-52) effectivement appliquées dans
  `walk_element` (l.220, 227) et `find_element` (l.244). Profondeur ≤ 40 ⇒ pas
  de risque de débordement de pile.

**Conclusion soundness : aucun double-free, aucun use-after-free, aucune UB.**
Le seul défaut d'ownership est une **fuite** (release absent), traitée ci-dessous.

---

## MAJEUR

### M1 — Fuite Core Foundation systématique : aucun `CFRelease` n'est jamais exécuté

**Fichier:ligne :**
- `release_ax_element` — `src/lib.rs:511-516` : **seul** appelant de `CFRelease`,
  marqué `#[allow(dead_code)]`, **jamais appelé**.
- `ax_elements` — `src/lib.rs:435` : `CFRetain(cf_ref)` (+1) sur **chaque**
  élément, sans release jumelé.
- `app_element` — `src/lib.rs:132` : `AXUIElementCreateApplication` (Create, +1)
  jamais libéré par `capture` (l.62), `window_ref` (l.76), `perform` (l.93).
- `attr_ax_value` — `src/lib.rs:500-509` : `AXValueRef` (+1 via `forget`) renvoyé
  à `attr_cgrect/cgpoint/cgsize` (l.457/475/487) puis jamais libéré.
- `attr_ax_element` — `src/lib.rs:377-386` : `AXUIElementRef` (+1) de
  `AXMainWindow` renvoyé à `resolve_window`, jamais libéré.

**Problème.** Le backend **ne libère jamais** un seul `AXUIElementRef` ou
`AXValueRef` qu'il possède. Conséquences par `capture` :
- l'élément application (+1),
- la fenêtre résolue (+1),
- **chaque nœud de l'arbre** : `walk_element` (l.225-233) itère
  `ax_elements(&children)` (chaque enfant retenu +1) puis recurse sans libérer ⇒
  jusqu'à `MAX_NODES` (5 000) refs fuités par capture,
- **1 à 2 `AXValueRef` par nœud** (`AXFrame`, ou `AXPosition`+`AXSize`).

`resolve_window` (l.145-175) aggrave : `ax_elements` retient **toutes** les
fenêtres mais n'en renvoie qu'une ⇒ les autres fuient ; en cas d'échec il
re-lit `kAXWindowsAttribute` une 2ᵉ fois (l.146 puis l.163), re-retenant tout.

> ⚠️ **Nuance importante — le `CFRetain` de `ax_elements` (l.435) est correct et
> nécessaire, ce n'est PAS lui le bug.** Dans `find_element` (l.238-263) les
> enfants sont empilés sur `stack` (l.256) et déréférencés à des itérations
> *ultérieures*, **après** que le `CFArray` parent a été libéré (fin du bloc
> `if let Some(children)`). Sans le retain, ce serait un **use-after-free**. Le
> défaut n'est donc pas le retain mais l'**absence du `CFRelease` jumelé**.

**Gravité.** Sain (pas d'UB), et **inoffensif pour le livrable WP-A** : l'exemple
`dump` (`examples/dump.rs`) est un process one-shot, l'OS récupère tout à la
sortie. **Mais** le contrat sera câblé dans `visualops-mcp`, un serveur
long-running qui capture en boucle : la fuite y est **non bornée** (~5 000 refs
AX + AXValues par capture). → **À traiter avant intégration serveur (BLOQUANT en
contexte serveur).**

**Correction concrète.** Introduire un wrapper RAII propriétaire, p.ex. :

```rust
struct AxElement(AXUIElementRef);
impl Drop for AxElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}
```

puis :
- `ax_elements` renvoie `Vec<AxElement>` (le `CFRetain` actuel devient le +1 du
  wrapper) ; `walk_element`/`find_element` détiennent des `AxElement` qui se
  libèrent au Drop. Pour l'élément *retourné* par `find_element`/`resolve_window`,
  faire `mem::forget` du wrapper (transfert) ou renvoyer le `AxElement` lui-même.
- `app_element` renvoie un `AxElement` (libère l'app en fin de `capture`/
  `window_ref`/`perform`).
- `attr_ax_value` : libérer le `AXValueRef` après `AXValueGetValue` (le wrapper
  RAII, ou un `CFRelease` explicite en fin de `attr_cgrect/cgpoint/cgsize`).
- `attr_ax_element` : même traitement pour la fenêtre principale.
- Supprimer `#[allow(dead_code)]` sur `release_ax_element` une fois utilisé, ou
  le retirer au profit du `Drop`.

---

## MINEUR

### m1 — `Type` : pas de repli CGEvent alors que le spec l'exige
**`src/lib.rs:107-112`** (et `set_string_attr` l.304). Le WP-A précise pour
`Type` : « `AXUIElementSetAttributeValue(...)` ; **if that errors, fall back to
CGEvent keystrokes** ». L'implémentation renvoie directement l'erreur AX sans
repli. Beaucoup de champs refusent `setValue` sur `kAXValueAttribute` (web
views, NSTextView en lecture indirecte) ⇒ `Type` échouera là où le spec attend
un succès. **Correction :** sur `Err`, synthétiser les frappes via
`CGEvent::new_keyboard_event` (séquence de caractères) avant de renoncer.

### m2 — `element_matches` : repli de label incohérent avec `walk_element`
**`src/lib.rs:280`** vs **`src/lib.rs:197-205`**. À la capture, le label dérive
de `title → description → (value seulement si AXStaticText)`. À la résolution,
`element_matches` essaie `title → description → value` (**value pour tout
rôle**). Un élément dont le `value` (non vide) coïncide avec le `label` recherché
peut donc matcher un autre élément que celui capturé. **Correction :** aligner la
dérivation de label sur celle de `walk_element` (value en repli **uniquement**
pour `AXStaticText`).

### m3 — `attr_string` masque les chaînes vides → perte « vide vs absent »
**`src/lib.rs:364`** : `.filter(|s| !s.is_empty())`. Un champ texte réellement
vide (`AXValue == ""`) devient `value = None` au lieu de `Some("")`. Pour le
diff/audit (`visualops-graph::audit::diff`), on ne distingue plus « champ vidé »
de « pas de valeur ». **Correction :** ne filtrer le vide que pour `label`
(esthétique d'ID), pas pour `value` ; ou conserver `Some("")` pour `value`.

### m4 — Affordance `Drag` émise par WP-B mais refusée par `perform`
**`src/lib.rs:114-116`**. `perform` renvoie `Execution(...)` pour
`Drag`/`Toggle`/`Scroll` — conforme au spec (« others → Execution error »).
Mais `visualops-graph::derive_affordances` **expose `Drag`** sur les
`Row`/`Cell`. Un agent qui suit le graphe d'affordances demandera donc une
action qui échoue toujours à l'exécution (pas de panique, mais incohérence
inter-crates). **Correction (côté plateforme, optionnel POC) :** implémenter
`Drag` via deux CGEvents (mouse down au centre source → mouse up sur la cible),
ou documenter explicitement la non-prise en charge pour que la couche MCP filtre
`Drag` des actions exécutables.

### m5 — Re-résolution first-match : nœuds à suffixe de collision inatteignables
**`src/lib.rs:238-283`**. `find_element` matche `(role, identifier, label)` et
renvoie le **premier** en pré-ordre — explicitement accepté par le WP-A (« first
match wins (POC heuristic) »). Limite à documenter : quand `synth_id` (WP-B) a dû
désambiguïser par `_2/_3` (même role+label+identifier), `find_element` renvoie
**toujours le premier**. L'exécuteur agit alors sur un élément *différent* de
celui évalué par le Risk Engine — implication de sûreté à garder en tête.
**Correction (post-POC) :** propager un index structurel (le `path` de WP-B) dans
`SceneNode`/la résolution, ou matcher par ordinal parmi les éléments de même clé.

### m6 — Robustesse / efficacité (regroupé, non bloquant)
- **`src/lib.rs:146` & `163`** : `resolve_window` lit `kAXWindowsAttribute`
  **deux fois** (copie + traversée CFArray redondantes). Mémoriser la 1ʳᵉ lecture.
- **`src/lib.rs:244`** : borne `seen > MAX_NODES || depth > MAX_DEPTH` (strict)
  vs `walk_element` `>=` (l.220) — incohérence off-by-one, inoffensive.
- **`src/lib.rs:139`** : valeur de retour de `AXUIElementSetMessagingTimeout`
  ignorée (acceptable, setter non critique).
- **`src/lib.rs:57,179`** : dépendance au SPI privé `_AXUIElementGetWindow`
  (recommandé par le WP-A, avec fallbacks corrects `AXMainWindow` → 1ʳᵉ fenêtre) —
  fragile entre versions macOS ; les fallbacks couvrent l'échec, OK pour POC.

---

## Informationnel (conforme au WP-A — pas un défaut)

- **`src/lib.rs:71`** — `capture` renvoie une racine unique (la fenêtre). Conforme
  au WP-A (« Return the window element as the single root »). À noter pour les
  intégrateurs : la **barre de menus** (et donc les items destructeurs
  `Supprimer`/`Éteindre`/`Forcer à quitter`/`Redémarrer…` du scénario de risque
  d'`ARCHITECTURE.md`) **n'apparaît pas** dans une capture live — elle est un
  attribut de l'élément *application* (`AXMenuBar`), pas un descendant de la
  fenêtre. La fixture à 2 racines est mintée séparément par l'architecte ; le
  démo de risque s'appuie sur cette fixture, pas sur la capture live. Si l'on
  veut les items de menu en réel, il faudra aussi parcourir `AXMenuBar` de l'app.
- **`src/lib.rs:81`** — `window_ref.app_name` via `kAXTitleAttribute` de l'app
  (souvent vide ⇒ `""`). Conforme au WP-A, qui mentionne aussi
  `NSRunningApplication.localizedName` comme option plus fiable.
- **`src/lib.rs:103`** — `Pick → kAXPressAction` : conforme à la table du WP-A.
- **`examples/dump.rs`** — parsing pid/window_id, message d'usage, `exit(1)` sur
  erreur, JSON pretty via `serde_json` (dev-dependency) : conforme et propre.
- **Contrat `visualops-core`** : tous les champs de `RawAxNode` sont renseignés
  (`ax_role`, `label`, `help`, `value`, `ax_identifier`, `ax_actions`, `frame`,
  `enabled`, `focused`, `children`) ; `ax_actions` normalisées (strip `AX` +
  lowercase) ⇒ cohérentes avec `visualops-graph::map_action`. **Aucune
  modification du contrat.**

---

## Résumé par sévérité

| Sévérité | # | Constat |
|---|---|---|
| **BLOQUANT** | 0 | Aucun double-free, use-after-free, UB, panique ou rupture de build/contrat. |
| **MAJEUR** | 1 | **M1** Fuite CF systématique : `CFRelease` jamais exécuté (app + tout l'arbre AX + AXValues fuités par capture). Sain mais **non borné** ⇒ **BLOQUANT une fois câblé dans le serveur `visualops-mcp`**. Le `CFRetain` (l.435) est correct/nécessaire (sûreté de `find_element`) ; le défaut est l'**absence de release**. Correctif : wrapper RAII `Drop→CFRelease`. |
| **MINEUR** | 6 | **m1** `Type` sans repli CGEvent (spec non respecté) · **m2** repli label `value` incohérent capture/résolution · **m3** `attr_string` masque `""` (perte vide/absent pour le diff) · **m4** `Drag` exposé par WP-B mais refusé par `perform` · **m5** first-match inatteignable pour les ID suffixés `_2` (sûreté de l'exécuteur) · **m6** divers (double lecture fenêtres, off-by-one borne, retour timeout ignoré, SPI privé). |

**Verdict.** Code FFI **sûr** (aucune UB, null-checks corrects, types
`AXValueGetValue` exacts, pas de panique, bornes respectées, contrat respecté) et
**conforme au WP-A** sur le périmètre fonctionnel. Le seul vrai défaut est la
**fuite mémoire CF généralisée (M1)** : tolérable pour le livrable `dump`
one-shot, mais à corriger impérativement (wrapper RAII) avant branchement dans le
serveur long-running. Les points MINEUR sont des écarts de robustesse/cohérence,
non bloquants pour le POC.
