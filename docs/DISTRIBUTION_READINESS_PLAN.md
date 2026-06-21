# Plan de passage POC → CLI distribuable

Date : 2026-06-14
Projet : `dunst-mcp` / `dunst-mcp`

## Contexte

Une revue large du projet a été menée avec plusieurs axes d'audit : code, documentation, packaging, shell, tangle, tests et comparaison avec `grob`.

Conclusion commune : le POC Rust/MCP est techniquement intéressant et plutôt sain, mais il n'est pas encore prêt pour une distribution Homebrew propre. Les blocages principaux ne sont pas un gros tangle architectural ; ils concernent surtout la surface produit/distribution : fiabilité des actions, formatage, contrat CLI, licences, CI, packaging, setup/doctor et documentation utilisateur.

Le point le plus prioritaire découvert par l'audit code est un risque de faux succès sur l'entrée clavier background : certaines fonctions peuvent signaler `true` même si une création ou un post d'événement clavier échoue partiellement. Ce point touche la fiabilité MCP réelle et doit passer avant Homebrew ou README.

## État de l'audit

Les audits sont considérés comme terminés.

Agents ayant fourni une synthèse exploitable :

- `audit_code`
- `audit_doc`
- `audit_packaging_surface`
- `audit_shell`
- `audit_tangle`
- `audit_test`
- `grob_compare`

Les notifications `idle` signifient que les agents sont disponibles ou en attente, pas qu'ils continuent à travailler.

Important : audit terminé ne veut pas dire corrections terminées.

## Priorité 1 — Corriger la fiabilité de l'entrée clavier background

Objectif : ne jamais retourner un succès si une action clavier background a échoué partiellement.

Fichiers principaux :

- `crates/dunst-platform/src/lib.rs`
- `crates/dunst-mcp/src/engine.rs`

Actions :

1. Inspecter les fonctions `type_text_background` et `key_web_background` dans `dunst-platform`.
2. Modifier la logique pour ne plus retourner succès si une paire d'événements attendue n'a pas été créée ou postée correctement.
3. Propager l'échec côté `Engine` en erreur MCP claire au lieu de convertir un faux succès en `Ok(())`.
4. Ajouter ou adapter des tests si le code peut être isolé sans dépendance macOS live.
5. Lancer le formatage Rust.

Vérification :

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Priorité 2 — Nettoyer le formatage

L'audit a confirmé que `cargo fmt --check` échoue.

Action :

```bash
cargo fmt --all
```

Puis vérifier :

```bash
cargo fmt --all -- --check
```

## Priorité 3 — Stabiliser le contrat CLI

Objectif : faire de `dunst-mcp` un vrai CLI installable avant de parler Homebrew.

Fichier principal :

- `crates/dunst-mcp/src/main.rs`

Actions :

1. Remplacer le parsing manuel `demo|serve` par `clap`.
2. Ajouter les commandes et aides suivantes :
   - `dunst-mcp --help`
   - `dunst-mcp --version`
   - `dunst-mcp demo`
   - `dunst-mcp serve --help`
   - `dunst-mcp doctor`
3. Garder le comportement existant de `demo` et `serve` autant que possible.
4. Ajouter un `doctor` minimal qui diagnostique l'environnement et explique les prérequis macOS/TCC si tout ne peut pas être testé automatiquement.

Vérification :

```bash
cargo run -p dunst-mcp -- --help
cargo run -p dunst-mcp -- --version
cargo run -p dunst-mcp -- serve --help
cargo run -p dunst-mcp -- doctor
```

## Priorité 4 — Ajouter licences et métadonnées Cargo

Objectif : rendre le manifeste cohérent avec une distribution propre.

Fichiers concernés :

- `Cargo.toml`
- `crates/dunst-mcp/Cargo.toml`
- racine du repo pour les licences

Actions :

1. Ajouter les fichiers licence correspondant à `MIT OR Apache-2.0` :
   - `LICENSE-MIT`
   - `LICENSE-APACHE`
   - éventuellement `LICENSE` synthétique indiquant le double choix.
2. Ajouter ou compléter les métadonnées Cargo nécessaires :
   - `description`
   - `repository`
   - `readme`
   - `keywords`
   - `categories`
   - `rust-version` si absent ou incomplet.
3. Clarifier la stratégie `publish = false` ou publication future.

Vérification :

```bash
cargo metadata --no-deps
cargo package -p dunst-mcp --allow-dirty --no-verify
```

## Priorité 5 — Ajouter une CI minimale

Objectif : transformer les vérifications locales en gate automatique.

Fichier cible :

- `.github/workflows/ci.yml`

Workflow minimal :

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --locked -p dunst-mcp
shellcheck scripts/*.sh
```

À ne pas ajouter dans le premier socle :

- release-plz
- formule Homebrew
- cargo-deny/gitleaks/semgrep complets
- mutation/fuzz lourds

## Priorité 6 — Séparer wrapper dev et binaire installé

Objectif : éviter que la configuration utilisateur dépende de chemins locaux comme `/Users/ludwig/workspace/...`.

Fichiers concernés :

- `scripts/mcp-dunst.sh`
- `.mcp.json`
- `.codex/config.toml`
- `README.md` plus tard

Actions :

1. Garder `scripts/mcp-dunst.sh` comme wrapper de développement.
2. Durcir le wrapper :
   - valider `DUNST_MCP_BIN`
   - valider `DUNST_MCP_MODE`
   - éviter les fallback silencieux
3. Documenter que la configuration installée devra appeler le binaire depuis le `PATH` :

```json
{
  "mcpServers": {
    "dunst": {
      "command": "dunst-mcp",
      "args": ["serve"]
    }
  }
}
```

Ne pas faire encore un setup complet qui écrit les configs client, sauf demande explicite.

## Backlog après le premier socle

À traiter après les priorités ci-dessus :

- Aligner les versions `objc2` entre `dunst-platform` et `dunst-vision`.
- Remplacer les chemins `/tmp` prévisibles par `tempfile` ou équivalent.
- Créer un registre MCP déclaratif unique pour éviter la dérive schéma/dispatcher/tests.
- Réécrire le README en landing page utilisateur : quickstart, config MCP, TCC, troubleshooting.
- Ajouter `setup --dry-run` / `setup --client codex|claude`.
- Ajouter release-plz.
- Créer un tap Homebrew privé seulement quand CLI, CI, licences et packaging passent.

## Non-objectifs immédiats

- Pas de refactor massif de `engine.rs` maintenant.
- Pas de split complet de `dunst-platform/src/lib.rs` maintenant.
- Pas de formule Homebrew immédiate.
- Pas de workflow release complet immédiat.
- Pas de documentation Diátaxis complète dans le premier patch.

## Résultat attendu

Après le premier lot, le projet doit passer de :

> POC local utilisable

à :

> base CLI alpha fiable, vérifiée localement, prête à recevoir CI/packaging propre.
