# Review `visualops-graph`

Commande lancee: `cargo test -p visualops-graph` -> vert, 6 tests passent.

Portee respectee pour la revue: lecture des sources Rust uniquement, aucune modification de `.rs`, aucun changement de `visualops-core`, aucune commande git.

## BLOQUANT

Aucun constat bloquant.

## MAJEUR

### Identifiants instables quand le label change

- **Fichier:ligne**: `crates/visualops-graph/src/scene.rs:56`
- **Probleme**: `synth_id` derive l'identite primaire du label (`prefix_slug`). Pour un noeud labelle, un changement de label modifie donc son ID. Ensuite `diff` ne peut plus emettre `Changed { field: "label" }`: il verra un `Removed` et un `Added`. Cela fragilise `diff_since`, l'audit et toute reference MCP conservee entre deux captures pour des elements dont le texte change.
- **Correction suggeree**: rendre l'identite stable independamment du label, tout en gardant le slug humain. Par exemple: preferer `ax_identifier` quand disponible, sinon ajouter un suffixe stable base sur le chemin (`btn_nouvelle_note_a1b2`). Si le format exact `btn_nouvelle_note` doit rester pour le POC, ajouter au moins dans `diff` une reconciliation des couples removed/added par `(parent, role, ax_identifier, bbox)` pour transformer un changement de label en `Changed(label)`.

### Hash de chemin trop court pour la limite de 5000 noeuds

- **Fichier:ligne**: `crates/visualops-graph/src/scene.rs:98`
- **Probleme**: `path_hash` tronque FNV-1a a 16 bits (`4 hex`). La contrainte plateforme autorise environ 5000 noeuds; a cette taille, les collisions de hash deviennent probables. Le `used` set evite l'ecrasement dans un graphe, mais les suffixes `_2`, `_3` dependent de l'ordre DFS, donc les IDs de noeuds non libelles peuvent changer quand un autre noeud collisionne.
- **Correction suggeree**: utiliser au moins 64 bits (`16 hex`) ou 48 bits (`12 hex`) pour les chemins, et ajouter un test generatif simple qui construit plusieurs milliers de chemins non libelles pour verifier l'absence de suffixes dus a collision dans une capture typique.

### Le diff ignore les changements structurels

- **Fichier:ligne**: `crates/visualops-graph/src/audit.rs:39`
- **Probleme**: `collect_field_changes` compare `label`, `value`, `enabled` et `bbox`, mais ignore `parent`, `children` et `roots`. Un drag/reorder ou un changement de hierarchie peut donc produire un diff vide si les IDs et champs compares restent identiques. C'est dangereux pour l'audit des actions de deplacement.
- **Correction suggeree**: comparer au minimum `parent` et `children` dans `collect_field_changes`, et comparer `SceneGraph::roots` dans `diff`. Encoder ces differences avec `NodeChange::Changed { field: "parent" | "children" | "roots", ... }` en serialisant les listes de facon deterministe.

### La normalisation ne gere pas les accents decomposes

- **Fichier:ligne**: `crates/visualops-graph/src/text.rs:8`
- **Probleme**: `normalize` replie seulement des caracteres precomposes (`é`, `à`, etc.). Une chaine Unicode decomposed comme `e\u{301}teindre` conserve le mark combinant, donc elle ne matche pas le mot-cle `éteindre` normalise en `eteindre`. Cela peut produire un faux negatif High dans le RiskEngine.
- **Correction suggeree**: normaliser en NFD puis supprimer les combining marks, via `unicode-normalization`, ou ajouter un traitement local des marks combinants courants (`U+0300..U+036F`) avant le matching. Ajouter un test avec `E\u{301}teindre` et `Re\u{301}initialiser`.

### Les cellules ne recuperent pas les sibling rows attendues

- **Fichier:ligne**: `crates/visualops-graph/src/affordance.rs:80`
- **Probleme**: `drag_targets_for` cherche les sibling rows parmi les enfants du parent direct. Pour un `Cell`, le parent direct est generalement une `Row`, donc ses freres directs sont d'autres cellules/contenus, pas les autres rows du container. Le test passe parce que l'ancetre `Outline`/`Table` rend la liste non vide, mais l'heuristique "sibling Row ids" est incomplete pour les cellules.
- **Correction suggeree**: pour un `Cell`, remonter d'abord a la row ancetre la plus proche, puis enumerer les autres enfants `Row` du parent de cette row. Garder ensuite les ancetres `List`/`Table`/`Outline`.

## MINEUR

### Matching de risque par sous-chaine brute

- **Fichier:ligne**: `crates/visualops-graph/src/risk.rs:117`
- **Probleme**: `hay.contains(keyword)` peut matcher un mot-cle a l'interieur d'un mot plus long, ou manquer des variantes ponctuees (`shut-down`, `force-quit`). Pour High, les faux positifs restent conservateurs; pour Medium, cela peut bruiter les affordances et les audits.
- **Correction suggeree**: normaliser la ponctuation en espaces, tokeniser, puis matcher les mots/phrases sur frontieres de tokens. Garder la priorite High > Medium.

### Couverture de tests trop centree fixture

- **Fichier:ligne**: `crates/visualops-graph/tests/wp_b.rs:52`
- **Probleme**: les tests valident la fixture Notes et les criteres WP-B minimaux, mais ne couvrent pas les cas limites demandes ici: labels vides/punctuation-only, accents decomposes, collisions de hash, changement de label, changement de parent/children, et cellule avec plusieurs sibling rows.
- **Correction suggeree**: ajouter des tests unitaires synthetiques pour `synth_id`, `diff`, `RiskEngine::assess` et `derive_affordances` sans dependre uniquement de `fixtures/notes.json`.

## Points verifies sans anomalie

- `map_role` couvre le mapping minimal de `docs/WP-B-graph.md`.
- `map_action` respecte le mapping attendu: `press`/`confirm` -> `Click`, `showmenu` -> `OpenMenu`, `pick` -> `Pick`, `raise` -> `Raise`, le reste -> `None`.
- `derive_affordances` inclut bien tous les noeuds, dedupe les actions en ordre stable, ajoute `Type`/`Focus` aux champs texte, et attache `risk.assess`.
- Le diff actuel est deterministe dans son ordre de sortie grace aux `BTreeMap` et a l'ordre fixe des champs compares.
- Les noeuds sans frame restent representes via `bbox: None`; pas de panic observe.
