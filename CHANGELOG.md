# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.6](https://github.com/hugvit/birta/compare/v0.10.5...v0.10.6) - 2026-11-25

### Fixed

- *(birta-core)* restore spans 0.3.4 placed for entity-spelled spaces and suspect blocks

## [0.10.5](https://github.com/hugvit/birta/compare/v0.10.4...v0.10.5) - 2026-11-25

### Fixed

- *(birta-core)* treat a backslash-escaped comment opener as text
- *(birta-core)* match code spans across CRLF line breaks as comrak reads them
- *(birta-core)* keep code spans out of code blocks and never slice inside a character
- *(birta-core)* keep footnote embeds and give suspect line-broken blocks exact spans or none
- *(birta-core)* find Obsidian syntax in source order and skip escaped link brackets
- *(birta-core)* find Obsidian syntax after line-broken wiki links without misreading code spans
- *(birta-core)* give line-broken wiki links exact spans or none

## [0.10.4](https://github.com/hugvit/birta/compare/v0.10.3...v0.10.4) - 2026-11-25

### Fixed

- *(birta-core)* never let one wiki link take another's span
- *(birta-core)* keep wiki link spans off code-span lookalikes and linear in link count

## [0.10.3](https://github.com/hugvit/birta/compare/v0.10.2...v0.10.3) - 2026-11-24

### Added

- *(birta-core)* add extract for links, embeds and tags without rendering

### Fixed

- *(birta-core)* keep spans for wiki links that break across lines
- *(birta-core)* trace Obsidian syntax to its exact source in render and extract

## [0.10.2](https://github.com/hugvit/birta/compare/v0.10.1...v0.10.2) - 2026-11-24

### Added

- *(birta-core)* add diff module for highlighted source diffs

### Fixed

- *(birta-core)* close the remaining GitHub parity gaps in diffs and source view
- *(birta-core)* match GitHub's diff geometry, hunk rows and markdown colours
- *(birta-core)* harden diff view against host CSS and long lines

### Other

- credit every bundled third-party asset in THIRD-PARTY-LICENSES
- list the bundled Primer Octicons in THIRD-PARTY-LICENSES
- Merge origin/main into feat-birta-core-diff

## [0.10.1](https://github.com/hugvit/birta/compare/v0.10.0...v0.10.1) - 2026-11-24

### Other

- ship THIRD-PARTY-LICENSES in release archives

## [0.10.0](https://github.com/hugvit/birta/compare/v0.9.0...v0.10.0) - 2026-11-23

### Fixed

- *(birta-core)* [**breaking**] resolve theme paths only when allow_paths is set

## [0.9.0](https://github.com/hugvit/birta/compare/v0.8.1...v0.9.0) - 2026-11-22

### Added

- add --no-embeds and --tags with matching config keys
- render obsidian image embeds in the viewer and static export

## [0.8.1](https://github.com/hugvit/birta/compare/v0.8.0...v0.8.1) - 2026-11-22

### Other

- *(release)* build the Apple Silicon binary on macos-15

## [0.8.0](https://github.com/hugvit/birta/compare/v0.7.1...v0.8.0) - 2026-11-22

### Added

- show YAML front matter as a table above the document

### Other

- *(release)* keep v-prefixed tags for birta and skip tags for birta-core
- check, lint and test every workspace member
- [**breaking**] extract rendering, themes and stylesheets into birta-core

## [0.7.1](https://github.com/hugvit/birta/compare/v0.7.0...v0.7.1) - 2026-09-29

### Added

- add copy markdown button to file header

### Fixed

- *(highlight)* escape carriage returns in raw source view
- *(template)* fill page placeholders in a single pass

## [0.7.0](https://github.com/hugvit/birta/compare/v0.6.0...v0.7.0) - 2026-06-14

### Added

- update docs for new release
- *(static)* export linked files as a portable bundle

### Fixed

- honour --light/--dark over OS color-scheme preference

### Other

- *(render)* drop dead single-file static render path
- ignore .DS_Store

## [0.6.0](https://github.com/hugvit/birta/compare/v0.5.0...v0.6.0) - 2026-04-12

### Added

- *(raw-mode)* show file stats in file-header

### Other

- update readme

## [0.5.0](https://github.com/hugvit/birta/compare/v0.4.0...v0.5.0) - 2026-04-11

### Added

- add raw mode with syntax-highlighted source view

### Other

- add directory navigation to readme features
- *(readme)* missing word

## [0.4.0](https://github.com/hugvit/birta/compare/v0.3.0...v0.4.0) - 2026-04-06

### Added

- add directory serving with navigable markdown links

### Fixed

- hide sticky header in visual regression screenshots

### Other

- add e2e tests for directory navigation and theme persistence

## [0.3.0](https://github.com/hugvit/birta/compare/v0.2.1...v0.3.0) - 2026-04-03

### Added

- add configurable keybindings with --bind flag
- add reading_mode, syntax_theme, and variant to config

### Other

- extract merge logic and improve test coverage
- update release-plz pr template

## [0.2.1](https://github.com/hugvit/birta/compare/v0.2.0...v0.2.1) - 2026-04-03

### Other

- migrate repo references from andresthor to hugvit

## [0.2.0](https://github.com/hugvit/birta/compare/v0.1.1...v0.2.0) - 2026-03-30

### Added

- add --static flag for self-contained HTML output

### Fixed

- formatting

## [0.1.1](https://github.com/hugvit/birta/compare/v0.1.0...v0.1.1) - 2026-03-29

### Other

- add Homebrew badge and install instructions
