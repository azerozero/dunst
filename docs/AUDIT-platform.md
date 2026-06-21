> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# AUDIT platform — visualops-platform + risk gate

Date: 2026-06-09
Scope: `crates/visualops-platform` en profondeur, passe transverse sur `crates/visualops-mcp/src/engine.rs::act`.
Mode: audit uniquement, aucun correctif appliqué.

## Findings priorisés

### 1. `crates/visualops-mcp/src/serve.rs:206` — BLOQUANT — `approve` est exposé comme outil libre, donc le gate est contournable par le même agent

`approve` est un outil MCP ordinaire qui appelle directement `engine.approve(&eid)` sans preuve d'approbation externe, capability séparée, challenge opérateur, ni restriction de rôle. Un agent qui reçoit `PendingApproval` peut simplement appeler `approve` puis réessayer l'action. Le commentaire `High-risk elements return pending_approval until approve() is called` décrit le mécanisme, mais pas une barrière de sécurité réelle.

Fix proposé: séparer l'approbation du canal d'action agent. Exiger un token/capability délivré hors modèle, lier l'approbation à `(scene_generation, id, action, risk_hash, argument_hash)`, et refuser `approve` depuis la même surface autonome que `click_element`/`drag_element`.

### 2. `crates/visualops-mcp/src/engine.rs:290` — BLOQUANT — les approbations sont persistantes, non consommées, et non validées

`approve(&mut self, id)` insère n'importe quelle chaîne dans `BTreeSet<String>`. `act` teste ensuite `self.approvals.contains(id)` mais ne consomme jamais l'approbation. Une approbation reste valable pour toutes les actions futures sur le même id, après refresh, après changement de label/risque, et peut être pré-positionnée pour un id inexistant qui apparaîtrait ensuite.

Fix proposé: rendre l'approbation one-shot et la consommer dans `act`; valider que l'id existe et que son risque courant nécessite vraiment approbation au moment de `approve`; stocker l'action, l'argument, la génération de scène, le label/identifier/bbox hashés, puis invalider à chaque `refresh`.

### 3. `crates/visualops-mcp/src/engine.rs:319` — BLOQUANT — `drag_element` gate uniquement le risque de la source, pas celui de la cible

`drag_element` calcule le centre bbox de `target_id`, puis appelle `act(source_id, Drag, Some("x,y"))`. Le gate de `act` charge seulement l'affordance et le risque du `source_id`. Un drag depuis une source bénigne vers une cible destructrice ou irréversible peut donc passer si la source est low-risk.

Fix proposé: évaluer un risque composite pour `Drag`: `max(risk(source), risk(target), risk(action+target_role+target_label))`. L'approbation doit être liée au couple `(source_id, target_id, Drag, drop_point)`, pas à la source seule.

### 4. `crates/visualops-platform/src/lib.rs:297` — BLOQUANT — si la fenêtre demandée disparaît, le backend peut agir sur `AXMainWindow` ou la première fenêtre du process

`resolve_window` cherche `requested_window_id`, puis retombe sur `AXMainWindow`, puis sur la première fenêtre AX. Pour une action, cela peut envoyer un click/type/drag dans une autre fenêtre du même process si la fenêtre ciblée a été fermée ou remplacée. C'est un problème de sécurité et d'intégrité de cible.

Fix proposé: rendre la résolution stricte pour `requested_window_id != 0`: si l'id n'est pas trouvé, retourner une erreur `WindowNotFound/WindowGone`. Réserver le fallback `AXMainWindow` uniquement à une cible explicitement wildcard (`window_id == 0`) et l'auditer comme tel.

### 5. `crates/visualops-platform/src/lib.rs:71` — BLOQUANT — `AX_CACHE` est thread-local global mais non namespacé par `pid/window_id`

Le cache stocke `ElementKey -> AxElement` globalement par thread. `ElementKey` ne contient ni `pid`, ni `window_id`, ni génération de capture. Deux `Engine`/targets dans le même thread peuvent se polluer: une capture B efface/remplit le cache, puis une action A peut toucher un élément B si la clé `(role,label,bbox,identifier)` collisionne.

Fix proposé: inclure `(pid, window_id, capture_generation, ElementKey)` dans la clé, ou déplacer le cache dans une instance liée au backend/target. Avant tout fast-path cached, revalider `_AXUIElementGetWindow(element) == target.window_id`.

### 6. `crates/visualops-mcp/src/engine.rs:343` — MAJEUR — `act` exécute sur une scène potentiellement stale sans refresh/revalidation pré-action

`act` clone le `SceneNode` et l'affordance depuis `current`, puis exécute immédiatement. Si l'UI change entre le dernier `refresh` et l'action, le risque, la disponibilité d'action, la bbox de drag, et le mapping AX peuvent ne plus correspondre à la cible réelle. La re-perception arrive après l'exécution, trop tard pour protéger.

Fix proposé: avant une action non purement read/probe, revalider la cible: refresh léger ou lookup AX live, confirmer id/role/label/identifier/bbox/risk, puis exécuter. Pour les actions approuvées, invalider si la génération ou le hash de cible a changé.

### 7. `crates/visualops-mcp/src/engine.rs:385` — MAJEUR — l'échec de `refresh()` après action est ignoré

Après `executor.perform`, le code fait `let _ = self.refresh();` puis calcule `diff_since`. Si la re-perception échoue, l'audit peut quand même retourner `Success` avec un diff faux ou obsolète. Cela affaiblit la traçabilité et peut masquer une action qui a changé l'état.

Fix proposé: propager l'erreur de refresh dans l'`AuditEntry` ou introduire un résultat `SuccessUnverified/VerificationFailed`; ne pas produire un diff comme s'il était fiable.

### 8. `crates/visualops-platform/src/lib.rs:271` — MAJEUR — le timeout AX de 1s n'est appliqué qu'à l'application, pas aux éléments retenus ensuite

`AXUIElementSetMessagingTimeout(app, 1.0)` est appelé sur l'élément application. Les `AXUIElementRef` extraits via attributs, arrays, cache ou fallback ne reçoivent pas explicitement ce timeout. Si le timeout est par élément, des appels sur enfants peuvent bloquer plus longtemps que prévu.

Fix proposé: appliquer le timeout à chaque `AxElement` construit (`app_element`, `attr_ax_element`, `ax_elements`, `retain_clone`) via un helper `AxElement::new_retained/non_null` qui centralise `AXUIElementSetMessagingTimeout`.

### 9. `crates/visualops-platform/src/lib.rs:773` — MAJEUR — un drag partiel peut laisser l'app cible dans un état "mouse down"

`drag` poste `LeftMouseDown`, puis plusieurs `LeftMouseDragged`, puis `LeftMouseUp`. Si `post_mouse` échoue après le down, la closure retourne avant le `LeftMouseUp`; le curseur est restauré, mais l'app cible peut avoir reçu un down sans up. `CGEventPostToPid` ne renvoie pas de statut, mais la création d'événements peut échouer.

Fix proposé: suivre `mouse_down_posted` et poster un `LeftMouseUp` best-effort dans un cleanup/defer dès qu'un down a été émis. Auditer séparément l'échec de cleanup.

### 10. `crates/visualops-platform/src/lib.rs:827` — MAJEUR — la restauration curseur peut déplacer le curseur d'un utilisateur humain concurrent

`hover`/`drag` sauvegardent la position puis appellent toujours `CGDisplay::warp_mouse_cursor_position(saved)`. Si l'utilisateur bouge physiquement la souris pendant les ~64ms du drag, le backend la ramène à l'ancienne position. C'est non-intrusif pour le geste synthétique, mais intrusif pour un humain concurrent.

Fix proposé: restaurer seulement si la position courante est proche de la trajectoire synthétique ou si un déplacement synthétique a été observé. Sinon, ne pas warper et journaliser `cursor_not_restored_user_moved`.

### 11. `crates/visualops-platform/src/lib.rs:715` — MAJEUR — `post_to_pid` ne fournit pas d'accusé de réception; le backend peut retourner `Ok` pour un événement ignoré

Les chemins clavier, hover et drag postent au PID avec `CGEventPostToPid`, qui retourne `void`. Si l'app ignore les événements parce qu'elle est inactive, sandboxée, non key-window, ou qu'un contrôle ne consomme pas les événements background, `perform` retourne quand même `Ok(())`. Le no-foreground est préservé, mais l'audit peut marquer `Success` sans effet.

Fix proposé: pour les actions mutantes, vérifier l'effet attendu via refresh/diff ou un probe AX ciblé. Pour hover/drag, retourner un statut "posted_unverified" ou ajouter une confirmation optionnelle par changement de graph/état.

### 12. `crates/visualops-platform/src/lib.rs:677` — MAJEUR — le fallback `type_text` peut modifier le focus AX avant de poster au PID

Quand `AXValue` n'est pas settable ou ne prend pas l'effet attendu, le code fait `set_bool_attr(element, kAXFocusedAttribute, true)` avant les keystrokes. Le WP considère Focus non-intrusif, mais selon l'app et le contrôle, ce focus AX peut changer l'état interne, faire défiler, ouvrir un champ, voire provoquer une activation indirecte.

Fix proposé: séparer `TypeSetValue` et `TypeKeystrokes`; rendre le fallback clavier opt-in par politique, revalider que l'app est restée background, et auditer explicitement le focus side-effect.

### 13. `crates/visualops-mcp/src/engine.rs:300` — MAJEUR — le risk gate ignore le contenu tapé

`type_into` gate seulement le risque de l'élément cible. Le texte `argument` n'est jamais évalué. Sur un champ low-risk mais sémantiquement dangereux (terminal, champ de commande, prompt admin, URL, recherche avec raccourcis), du contenu destructif peut passer sans approbation.

Fix proposé: intégrer `argument` dans l'évaluation de risque pour `Type`, avec politiques par rôle/app: commandes shell, AppleScript, URLs sensibles, mots clés destructifs, secrets, ou texte multi-ligne avec entrée implicite.

### 14. `crates/visualops-graph/src/risk.rs:95` — MAJEUR — la classification de risque est heuristique label/help/identifier uniquement

Le risque dépend de mots clés dans label/help/identifier. Un élément destructif sans mot clé reconnu, icon-only, localisé hors FR/EN, ou renommé par l'app sera low-risk. C'est acceptable pour un POC, pas pour un gate "infranchissable".

Fix proposé: compléter avec signaux structurels et contextuels: rôle menu item sous menus système, native action/identifier allow/deny-lists, app bundle, position dans menus "File/Edit", confirmation OS, historique de diff, et politiques par action.

### 15. `crates/visualops-platform/src/lib.rs:561` — MAJEUR — le fast-path cached ne revalide pas que l'élément retenu correspond encore au `SceneNode`

`cached_element(key)` retourne un `AXUIElementRef` retenu et `perform_on_element` l'utilise directement. La seule protection est le fallback sur erreurs stale (`kAXErrorInvalidUIElement`/`CannotComplete`). Un élément AX encore valide mais sémantiquement différent peut recevoir l'action.

Fix proposé: avant action cached, recalculer `element_key(element)` et comparer à `key`, vérifier `_AXUIElementGetWindow`, et idéalement comparer un hash minimal `(role,label,identifier,bbox,enabled)` au snapshot courant.

### 16. `crates/visualops-platform/src/lib.rs:516` — MINEUR — `find_element` peut faire un fallback vers une collision de clé

Le fallback live search utilise `element_key(element) == ElementKey::from_scene(wanted)`. La clé est utile mais pas unique: beaucoup d'éléments peuvent partager rôle, label vide, identifier absent et bbox arrondie. Le premier match DFS gagne.

Fix proposé: enrichir `ElementKey` avec parent path/id stable, index sibling, window id, et/ou score multi-critères plutôt qu'égalité stricte sur peu de champs.

### 17. `crates/visualops-platform/src/lib.rs:134` — MINEUR — `clear_cache()` global à chaque capture peut invalider des actions concurrentes

Le cache est thread-local, mais `capture` efface tout le cache de ce thread sans tenir compte du target. Dans un usage multi-engine synchrone, une capture d'un target peut dégrader ou détourner le fast-path d'un autre target.

Fix proposé: namespacer le cache par target/génération, ou l'attacher à l'instance `MacosBackend` au lieu d'un `thread_local`.

### 18. `crates/visualops-platform/src/lib.rs:841` — MINEUR — les erreurs AX d'attribut sont silencieusement écrasées en `None`

`attr_value` retourne `None` pour toute erreur AX. Cela simplifie la capture, mais masque les différences entre attribut absent, timeout, permission, stale element, et erreur système. La capture peut produire un graphe incomplet sans signal fiable hors `MAX_NODES/MAX_DEPTH`.

Fix proposé: en mode debug ou métrique, compter les erreurs par code AX et les exposer dans stderr/trace; traiter certains codes (`CannotComplete`, timeout) comme capture dégradée explicite.

## Points FFI sans finding bloquant

- `AxElement::retain_clone`/`Drop` (`crates/visualops-platform/src/lib.rs:111`, `:119`) équilibrent bien `CFRetain`/`CFRelease` pour les refs non nulles.
- `attr_ax_element` (`crates/visualops-platform/src/lib.rs:873`) transfère correctement l'ownership du `CFType` create-rule vers `AxElement` via `mem::forget` après type-check.
- `ax_elements` (`crates/visualops-platform/src/lib.rs:921`) retient explicitement les valeurs borrowed d'un `CFArray` avant de construire des `AxElement`.
- `BatchValues` (`crates/visualops-platform/src/lib.rs:368`) garde le `CFArray` vivant pendant l'utilisation des `CFTypeRef` borrowed; pas de use-after-free visible dans ce chemin.
- `retain_core_graphics_image` n'existe pas dans `visualops-platform`; le risque précédemment lié à cette conversion n'est pas applicable à ce crate.

## Verdict synthétique

Le backend FFI est globalement prudent sur retain/release, mais la sécurité système repose sur des invariants non encodés: cache AX global, résolution de fenêtre permissive, scène stale, et approbation trop large. Le risk gate n'est pas infranchissable aujourd'hui: un agent peut appeler `approve`, un drag peut contourner le risque de la cible, et une approbation persiste sans être consommée.
