# Cycle — VisualOps MCP / audit des 4 crates pures

> Audit STRICT (lecture seule). Aucune correction, aucun `git`.
> Périmètre : `visualops-core`, `visualops-graph`, `visualops-mcp`, `visualops-vision`.
> `visualops-platform` est **exclu** (revu en parallèle).
> Date : 2026-06-09 · Branche : `poc/visualops`

## Signaux de référence (mesurés, pas estimés)

| Vérification | Résultat |
|---|---|
| `cargo build` (core+graph+mcp) | OK |
| `cargo clippy --all-targets` (les 4 crates) | **0 warning, 0 erreur** |
| `cargo test` core+graph+mcp | **40 tests OK** (0 échec) |
| `cargo test -p visualops-vision` (coords) | **14 tests OK** |
| LOC périmètre | ~4 300 (core 488 / graph 1609 / mcp 1298 / vision 907) |

Base de code remarquablement saine pour un POC : frontières de modules nettes,
`core` gelé et sans dépendance macOS, logique pure entièrement testée contre une
fixture device-free. Les constats ci-dessous sont donc majoritairement du
**durcissement (P1)** et du **polissage (P2)** — pas de P0.

---

## Carte de score (par dimension)

| Dimension | Note | Commentaire |
|---|---|---|
| Qualité code (`audit-code`) | A− | Idiomatique, commenté avec justification (G1…G7, D1…D3, WP-J). Quelques `unwrap()` de sérialisation. |
| Tests (`audit-test`) | A | 54 tests, invariants ciblés, snapshots de régression (ids, round-trips coords). Trou : pyramide sans test d'intégration du serveur stdio. |
| Couplage (`audit-tangle`) | A | DAG strict `core ← graph/vision`, `mcp` agrège. Aucun cycle. `engine.rs` = seul gros fichier (844 L). |
| Drift (`audit-drift`) | B+ | Pas de `CONTRACTS.md`. 2 écarts doc↔code (confidence OCR « monotone », exemple README). |
| Doc (`audit-doc`) | A− | ARCHITECTURE/README/WP excellents. Pas de rustdoc d'erreur sur les API publiques `mcp`. |
| Sync doc↔code (`audit-sync`) | B+ | Exemple JSON README non conforme au schéma `Affordance` réel. |
| Perf (`forge-perf`) | A | Pré-normalisation des keywords (G5), `BTreeSet` anti-O(n²) (G6), FNV-1a 64-bit (G7). 1 recalcul jetable + 1 recalcul O(N) par listing. |

---

## Triage 3-2-1 (tous les constats, non tronqué)

### 🔴 Tier 3 — Critique (0 item)

Aucun. Pas de faille de sécurité, pas de fonctionnalité cassée, pas de risque de
perte de données. Le gating de risque (action high → `PendingApproval`, exécuteur
jamais appelé) est correct et testé (`high_risk_click_is_gated_then_approved`,
`find_element_and_gating_still_reach_latent_nodes`).

### 🟡 Tier 2 — Majeur (5 items)

| # | Constat | Fichier:ligne | Dim. | Effort | Action concrète |
|---|---|---|---|---|---|
| 1 | **Transform OCR dupliqué et non testé.** `region_to_vision_roi` réimplémente à la main le Y-flip + clamp déjà fournis, prouvés et testés par `coords::window_rect_to_vision_roi`. Le clamp y est aussi **différent** (`w.clamp(0, 1-x)` vs clamp par arêtes), donc deux comportements de bord divergents pour « la même » conversion — exactement le bug n°1 prédit (§10.8). | `vision/src/ocr.rs:115-141` | drift / code | M | Remplacer le corps par un appel à `coords::window_rect_to_vision_roi` (en convertissant l'entrée screen-pt → window-local), supprimer la copie. |
| 2 | **Calcul mort dans le hot path OCR.** `vision_norm_to_screen_pt` est appelé puis le résultat `_screen_box` est jeté à chaque observation. Travail inutile par ligne OCR + intention masquée (le `OcrBox` ne porte que `norm`). | `vision/src/ocr.rs:107` | perf / code | L | Soit supprimer l'appel, soit ajouter le `Bbox` screen-pt au `OcrBox` si les consommateurs en ont besoin (sinon le supprimer). |
| 3 | **`find_element` n'applique pas le filtre latent ni la projection compacte.** Les autres listings (`query_affordances`, `get_affordances`, `get_scene_graph`) filtrent les nœuds latents et compactent (WP-J). `find_element` renvoie le `SceneNode` **complet** de chaque match, latents inclus — incohérence de surface MCP et payload lourd. | `mcp/src/serve.rs:170-172`, `engine.rs:105-116` | sync / code | M | Décider la politique (ex. projeter en compact + drapeau `include_latent`), documenter l'écart si volontaire. |
| 4 | **`unwrap()` de sérialisation sur le chemin MCP.** Plusieurs `serde_json::to_value(...).unwrap()` dans `handle_tool_call` (l.171,181,188,195,202,220) : un échec de sérialisation panique le serveur au lieu de renvoyer une erreur JSON-RPC, alors que `engine.rs` utilise déjà `.unwrap_or(Value::Null)` ailleurs. | `mcp/src/serve.rs:171-220` | code | L | Remplacer par `.unwrap_or(Value::Null)` ou propager en `isError:true`, par cohérence avec `engine.rs`. |
| 5 | **Pas de `CONTRACTS.md` ni de test d'intégration du serveur stdio.** Les invariants forts (gating jamais contourné, `full` byte-identique, `actionable_only` ⊆ total) vivent dans des commentaires et tests unitaires, mais le binaire `serve` (parse JSON-RPC → dispatch) n'a aucun test ; un harness « live serve smoke » existe (commit `9ba75e2`) hors périmètre. | `mcp/src/serve.rs` (global) | test / drift | M | Ajouter 2-3 tests JSON-RPC table-driven sur `handle_tool_call` (tool inconnu, arg manquant, `view` invalide) ; figer les contrats clés. |

### 🟢 Tier 1 — Mineur (6 items)

| # | Constat | Fichier:ligne | Dim. | Effort | Action concrète |
|---|---|---|---|---|---|
| 6 | **Exemple README non conforme au schéma réel.** Le JSON montre `{id, role, label, actions, confidence, risk}` sur un seul objet, mais `confidence` est sur `SceneNode` et `actions`/`risk` sur `Affordance` (deux types distincts). Aucun objet sérialisé n'a cette forme. | `README.md:8-11` | sync | L | Marquer l'exemple comme « vue fusionnée illustrative » ou le scinder en scene-node + affordance. |
| 7 | **Claim « risk monotone en incertitude » non implémenté.** `OcrBox.confidence` est documenté comme devant remonter le gate (§10.7), mais `RiskEngine::assess` ne consomme jamais la confidence (POC AX-only, `confidence=1.0`). Drift latent à tracer pour P1. | `vision/src/lib.rs:50-57` | drift / doc | L | Ajouter un `// TODO P1` explicite ou une note « non encore câblé » pour éviter une fausse garantie. |
| 8 | **`role_key` re-sérialise un enum via `serde_json` par nœud** pour obtenir la string du rôle (histogramme summary + compact). Allocation + passage serde évitables. | `mcp/src/engine.rs:466-471` | perf | L | Exposer un `Role::as_str(&self) -> &'static str` dans `core` (déjà voisin de `id_prefix`) et l'utiliser. |
| 9 | **Recalcul O(N) de `window_rect`/`menubar_root_id` à chaque listing.** Chaque appel de `scene_graph_view` / `affordances_view` / `query_affordances_filtered` re-balaye tous les nœuds pour retrouver la fenêtre et le menubar root. Acceptable au POC (graphes petits), à mémoïser si le cap 5000 nœuds est atteint. | `mcp/src/engine.rs:179-198` | perf | M | Cacher `(window_rect, menubar_root_id)` au moment du `refresh()`. |
| 10 | **Deux benches `pipeline`/`graph_bench` quasi identiques.** `graph_bench.rs` est un sous-ensemble de `pipeline.rs` (mêmes 3 fonctions, sans le groupe « full »). Duplication. | `graph/benches/graph_bench.rs` | code (DRY) | L | Supprimer `graph_bench.rs` ou le réduire au seul cas non couvert, et ajuster `Cargo.toml`. |
| 11 | **`Cargo.lock` versionne des deps de seconde main lourdes pour le spike vision** (`objc2-vision`, etc.) sans `rust-toolchain`/CI épinglant la cible macOS. Risque de build cassé hors macOS si `coords` n'était pas correctement `cfg`-gardé (il l'est — vérifié). Purement préventif. | `vision/Cargo.toml` | infra | L | Documenter dans le README que seul `coords` compile cross-platform ; le reste est `cfg(target_os="macos")`. |

---

## Top-5 des recommandations les plus rentables

Classées par (impact × confiance) ÷ effort :

1. **#2 — Supprimer le `_screen_box` mort dans `ocr.rs`** (effort L, gain immédiat).
   Travail inutile par ligne OCR retiré du hot path < 100 ms, et l'intention
   redevient lisible. Zéro risque.

2. **#1 — Unifier la transform OCR sur `coords::window_rect_to_vision_roi`**
   (effort M). Élimine la *deuxième* implémentation divergente du Y-flip/clamp —
   la source de bug n°1 explicitement anticipée par la doc — et fait bénéficier
   `ocr.rs` des 14 tests de `coords` au lieu de zéro.

3. **#4 — Remplacer les `unwrap()` de sérialisation MCP par un fallback**
   (effort L). Transforme un panic-serveur potentiel en erreur JSON-RPC propre,
   et aligne `serve.rs` sur la convention déjà tenue dans `engine.rs`.

4. **#5 — Tests table-driven sur `handle_tool_call`** (effort M). Couvre le
   dispatcher (le seul gros morceau non testé du périmètre) et fige les
   invariants de surface MCP — fort retour pour peu de lignes.

5. **#3 — Aligner `find_element` sur la politique latent/compact** (effort M).
   Supprime l'incohérence entre les 4 outils de listing et allège le plus gros
   payload non projeté exposé à l'agent.

---

## Points forts (à préserver)

- **Frontières de crates exemplaires** : `core` gelé, `graph`/`vision` purs et
  device-free, `mcp` agrégateur. DAG sans cycle.
- **Discipline perf déjà appliquée** : G5 (keywords pré-normalisés), G6 (anti
  O(n²)), G7 (hash 64-bit), commentés avec leur justification.
- **Tests d'invariants, pas de surface** : round-trips de coordonnées, snapshot
  d'ids de régression, gating jamais contournable même sur nœuds latents.
- **Politique `_NS:` stable-id délibérée et testée** (`is_appkit_auto`) — à ne
  PAS « corriger » : c'est une déviation WP-D assumée.
- **Clippy clean + 54 tests verts** sur tout le périmètre.

## Statut Phoenix

🟢 **Convergé sur le critère santé** : 0 item 🔴, et les 🟡 sont du durcissement,
pas des défauts bloquants. Le POC est sain. Prochain passage utile : après
câblage du backend live (`visualops-platform`, hors périmètre) et de P1 vision.
