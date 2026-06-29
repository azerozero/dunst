# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
