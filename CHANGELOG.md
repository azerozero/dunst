# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3](https://github.com/azerozero/dunst/compare/v0.1.2...v0.1.3) - 2026-10-05

### Fixed

- *(focus)* fiabilise la saisie et les panneaux natifs ([#46](https://github.com/azerozero/dunst/pull/46))

### Other

- simplifications issues de l'audit de code ([#45](https://github.com/azerozero/dunst/pull/45))

## [0.1.2](https://github.com/azerozero/dunst/compare/v0.1.1...v0.1.2) - 2026-07-06

### Fixed

- *(raise)* activation fenêtre-seule — le raise ramenait toutes les fenêtres de l'app ([#39](https://github.com/azerozero/dunst/pull/39))

## [0.1.1](https://github.com/azerozero/dunst/compare/v0.1.0...v0.1.1) - 2026-07-06

### Added

- cycle Phoenix — CI ré-armée, doc réalignée, doctor JSON, perf vision ([#35](https://github.com/azerozero/dunst/pull/35))
- *(vision)* détecte les cartes pleines comme conteneurs (ShapeKind::Panel) ([#29](https://github.com/azerozero/dunst/pull/29))
- *(vision)* arbre de zones imbriquées (read_zones) sur la sortie vision plate ([#28](https://github.com/azerozero/dunst/pull/28))
- *(approval)* preauthorize — pré-autorisation brute bornée pour couper les allers-retours MCP↔LLM ([#14](https://github.com/azerozero/dunst/pull/14))
- *(choices)* auto scroll_scan sur surface incomplète + recommandations d'enchaînement ([#13](https://github.com/azerozero/dunst/pull/13))
- *(mcp)* enumerate_choices + apply_selections (remplissage de choix par lot) ([#4](https://github.com/azerozero/dunst/pull/4))

### Fixed

- *(release)* retire publish=false des manifests — il cassait le diff git-only ([#36](https://github.com/azerozero/dunst/pull/36))
- *(input)* clic curseur réel (borrow_cursor) pour les popups natives ([#32](https://github.com/azerozero/dunst/pull/32))
- *(coordination)* set_field_text advertise ses préconditions de mutation + test anti-dérive ([#18](https://github.com/azerozero/dunst/pull/18))
- *(type_keys)* signale quand la saisie brute peut ne pas avoir atterri ([#16](https://github.com/azerozero/dunst/pull/16))
- *(epoch)* exclut la barre de menus du fingerprint (plus de « stale UI epoch ») ([#12](https://github.com/azerozero/dunst/pull/12))
- *(perception)* classe les champs de la barre de menus en chrome, pas en page ([#11](https://github.com/azerozero/dunst/pull/11))
- *(scroll)* oriente vers enumerate_choices scroll_scan pour énumérer les feeds sparse-AX ([#10](https://github.com/azerozero/dunst/pull/10))
- *(navigate+launch+open_url)* cible navigateur, launched honnête, réutilisation d'onglet correcte ([#9](https://github.com/azerozero/dunst/pull/9))
- *(audit)* robustesse iTerm/web backgroundé — hotkey ciblé, visibilité, auto-unstick curseur idle-gaté ([#7](https://github.com/azerozero/dunst/pull/7))
- *(epoch+set_text)* epoch stable au focus + repli de saisie sur champ web ([#6](https://github.com/azerozero/dunst/pull/6))
- *(epoch)* fingerprint structurel, ne churn plus sur le texte vivant des pages web ([#5](https://github.com/azerozero/dunst/pull/5))

### Other

- *(rustdoc)* documente les conditions d'erreur des fn publiques (# Errors) ([#27](https://github.com/azerozero/dunst/pull/27))
- *(selections)* aplatit execute_selection_batch (nesting 6 → helper plat) ([#26](https://github.com/azerozero/dunst/pull/26))
- *(read)* scinde le god-module read.rs sous 1000 lignes (clôt C3-001) ([#25](https://github.com/azerozero/dunst/pull/25))
- *(read)* extrait le cluster epoch/fingerprint en sous-module (XRAY-004/T-002) ([#24](https://github.com/azerozero/dunst/pull/24))
- *(readme)* documente l'installation sur PATH + lien vers CONTRIBUTING ([#23](https://github.com/azerozero/dunst/pull/23))
- corrige le compte de nœuds du fixture Notes (427 → 22, 1 → 2 racines) ([#19](https://github.com/azerozero/dunst/pull/19))
- sort le window_id des fixtures de la plage réelle (fin des 4 flakes locaux) ([#17](https://github.com/azerozero/dunst/pull/17))
- supprime la fiction driver/Diátaxis, aligne CONTRACTS + nettoie le cruft ([#15](https://github.com/azerozero/dunst/pull/15))

## [0.1.0](https://github.com/azerozero/dunst/releases/tag/v0.1.0) - 2026-06-29

### Added

- *(mcp)* outils navigate + set_field_text, robustesse scroll/OCR, fixes plateforme & CI à iso grob ([#3](https://github.com/azerozero/dunst/pull/3))
- *(mcp)* add unstick_cursor recovery tool
- *(platform)* expose backend capabilities
- *(mcp)* coordinate mutating sessions
- *(mcp)* add session provenance
- *(mcp)* [**breaking**] harden live targets and setup flow
- *(mcp)* add semantic hit targets
- *(mcp)* add reveal_hover_click tool
- *(mcp)* harden live window actions
- *(mcp)* ship live dunst operator workflow
- launch_app args passthrough — background-paint nudge for Chromium charts
- add list_apps tool (running GUI apps, with name-substring search)
- read_text accurate-OCR option + helper unit tests + open_menu limit doc
- working AX-first pipeline + risk-gated engine + demo

### Fixed

- *(mcp)* prefer wheel scroll when background keys scroll dead
- *(mcp)* reject brand-only titles for URL verification
- *(mcp)* flag disabled approve tool in approval hint
- *(mcp)* floor OCR click affordance risk to executor gate
- *(platform)* keep cursor-bound input on target
- *(mcp)* handle sparse browser form edits
- *(mcp)* bound AX probes and OCR offset clicks
- *(mcp)* harden raw input recovery
- *(mcp)* align page scroll and URL verification
- *(mcp)* guard visual actions by target visibility
- *(mcp)* harden browser and raw approval flow
- *(setup)* start installed mcp server
- *(mcp)* reduce raw input approval churn
- *(mcp)* gate foreground and verify live actions
- *(mcp)* cap select_file chooser wait
- open_menu doc — real failure cause is a wrong menu NAME, not the node cap
- correct open_menu doc — works on backgrounded apps; real limit is the node cap
- read_shapes also uses composited capture (GPU-rendered windows)
- read_text uses composited capture so it reads GPU-rendered windows

### Other

- *(license)* relicense to Apache-2.0 only
- *(cycle)* record raw input follow-up
- add prek hooks
- *(mcp)* split read dispatch navigation
- *(mcp)* split tool catalog families
- *(mcp)* split dispatch and input slices
- split engine serve and macos modules
- *(platform)* split macos backend shell
- *(mcp)* split engine and response slices
- [**breaking**] rename crates visualops-* -> dunst-* (binary dunst-mcp)
- conform README example, add CONTRACTS.md, mark vision confidence not-yet-wired
