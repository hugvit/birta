# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.3](https://github.com/hugvit/birta/compare/birta-core-v0.3.2...birta-core-v0.3.3) - 2026-11-24

### Added

- *(birta-core)* add extract for links, embeds and tags without rendering

### Fixed

- *(birta-core)* keep spans for wiki links that break across lines
- *(birta-core)* trace Obsidian syntax to its exact source in render and extract

## [0.3.2](https://github.com/hugvit/birta/compare/birta-core-v0.3.1...birta-core-v0.3.2) - 2026-11-24

### Added

- *(birta-core)* add diff module for highlighted source diffs

### Fixed

- *(birta-core)* keep table delimiter dashes plain and let theme variables beat page defaults
- *(birta-core)* close the remaining GitHub parity gaps in diffs and source view
- *(birta-core)* match GitHub's diff geometry, hunk rows and markdown colours
- *(birta-core)* take diff text and row colours from the same light/dark mode
- *(birta-core)* match GitHub's diff colours and layout in the github theme
- *(birta-core)* keep the long-line cap out of highlight_source
- *(birta-core)* harden diff view against host CSS and long lines

### Other

- Merge origin/main into feat-birta-core-diff

## [0.3.1](https://github.com/hugvit/birta/compare/birta-core-v0.3.0...birta-core-v0.3.1) - 2026-11-24

### Other

- ship THIRD-PARTY-LICENSES in release archives

## [0.3.0](https://github.com/hugvit/birta/compare/birta-core-v0.2.0...birta-core-v0.3.0) - 2026-11-23

### Fixed

- *(birta-core)* [**breaking**] resolve theme paths only when allow_paths is set

## [0.2.0](https://github.com/hugvit/birta/compare/birta-core-v0.1.0...birta-core-v0.2.0) - 2026-11-22

### Added

- *(birta-core)* [**breaking**] let resolvers mark wiki links and embeds unresolved
- *(birta-core)* render obsidian note embeds through Resolver::embed_note
- *(birta-core)* [**breaking**] render obsidian image embeds and tags

### Fixed

- *(birta-core)* match an escaped tag only at a tag boundary in the source
- *(birta-core)* keep escaped embeds and tags as text
- *(birta-core)* match embed headings case-insensitively beyond ascii

### Other

- *(birta-core)* cover embeds in table cells and after non-ascii text
