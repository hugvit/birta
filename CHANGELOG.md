# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/andresthor/birta/releases/tag/v0.1.0) - 2026-03-29

### Added

- add logo as favicon and README header
- add --light/--dark, --reading-mode flags and OS theme detection
- *(nvim)* add setup() with config for all CLI flags
- add CLI flags for theme-swap, toggle, fonts, and header visibility
- add custom system font support via config
- add file header bar and reading mode
- *(themes)* add 9 bundled themes
- *(themes)* persist variant preference across theme switches
- add theme support with catppuccin and dracula presets
- add interactive checkbox toggling with source write-back
- add scroll synchronization from editor to browser
- add neovim lua plugin
- add client-side diagram rendering with mermaid.js
- add client-side math rendering with KaTeX
- add --css flag for custom stylesheet injection
- support reading markdown from stdin
- add hover-reveal anchor links on headings
- auto-shutdown server when last browser tab closes
- add github-style alert/admonition rendering
- add server-side syntax highlighting with syntect
- add error handling and graceful shutdown
- serve local images referenced in markdown
- add file watcher and websocket live updates
- add HTTP server serving rendered markdown
- add HTML viewer template with github-markdown-css
- add markdown-to-html rendering with comrak
- scaffold project with CLI argument parsing

### Fixed

- rustfmt and release-plz action version
- *(themes)* always include syntax.css and add missing diff scopes
- *(themes)* use visible border color for catppuccin theme
- theme hot-swap, github fidelity, and alert backgrounds
- scope themed alert colors to avoid overriding github defaults
- stop sheen server when neovim exits
- detect websocket disconnect even without pending sends
- dark mode toggle now applies to markdown body
- apply dark mode theme to markdown body content

### Other

- collapse nested if in main.rs (clippy 1.94)
- collapse nested if statements (clippy 1.94)
- add CI, release, and Homebrew workflows
- add README, LICENSE, and crates.io metadata
- improve --help with grouped options and examples
- rename crate from sheen to birta
- regroup header controls and fix toggle layout shift
- [**breaking**] replace CSS-based themes with TOML format and hot-swap support
- add mermaid diagram to kitchen sink fixture
- use proper images in kitchen sink fixture
- add kitchen sink fixture covering all supported features
- verify details/summary and footnote interactive elements
- use comrak's native alerts and image URL rewriting
- add playwright e2e tests for rendering and live reload
