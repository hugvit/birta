# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
