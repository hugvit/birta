//! Multi-file static export: crawl the relative `.md` link graph from an entry
//! file and write a self-contained bundle of `.html` pages plus copied assets,
//! mirroring the source directory tree.
//!
//! Path safety: every source path is validated with `canonicalize` +
//! `starts_with(base_dir)` (the same model as `server::resolve_safe_path`), so
//! links/assets that escape the tree via `..` or symlinks are rejected rather
//! than read or written outside the bundle. Output paths are derived from the
//! canonicalized source, so writes always stay inside `out_dir`.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::render;
use crate::template::{self, PageOptions};
use crate::theme::ResolvedTheme;

/// Page-rendering options for a static bundle, mirroring the relevant subset of
/// the server's per-page options.
pub struct BundleOptions<'a> {
    pub theme: &'a ResolvedTheme,
    pub custom_css: Option<&'a str>,
    pub font_css: Option<&'a str>,
    pub show_header: bool,
    pub reading_mode: bool,
    pub raw_mode: bool,
    pub variant_explicit: bool,
    pub keybindings_json: &'a str,
}

/// Result of a bundle export.
#[derive(Debug)]
pub struct BundleResult {
    /// Absolute path of the entry file's `.html` (to open in a browser).
    pub entry_html: PathBuf,
    /// Number of pages written.
    pub pages: usize,
    /// Number of assets copied.
    pub assets: usize,
}

/// Export `entry` and every reachable relative `.md` link into `out_dir`.
///
/// `base_dir` bounds the crawl; nothing outside it is read or written.
pub fn export_bundle(
    entry: &Path,
    base_dir: &Path,
    out_dir: &Path,
    opts: &BundleOptions<'_>,
) -> anyhow::Result<BundleResult> {
    let base_dir = std::fs::canonicalize(base_dir)
        .with_context(|| format!("base directory not found: {}", base_dir.display()))?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("could not create output directory: {}", out_dir.display()))?;
    let out_dir = std::fs::canonicalize(out_dir)
        .with_context(|| format!("could not resolve output directory: {}", out_dir.display()))?;

    if dir_is_non_empty(&out_dir) {
        eprintln!(
            "birta: warning: output directory {} is not empty; existing files are overwritten, not removed",
            out_dir.display()
        );
    }

    let syntax = opts.theme.active_data().syntax.as_ref();

    // Resolve the entry relative to base_dir (using on-disk casing) so the
    // returned `.html` path matches what is actually written.
    let entry_canonical = std::fs::canonicalize(entry)
        .with_context(|| format!("entry file not found: {}", entry.display()))?;
    let entry_rel = entry_canonical
        .strip_prefix(&base_dir)
        .map_err(|_| anyhow::anyhow!("entry is not inside its base directory"))?
        .to_path_buf();
    let entry_out_rel = entry_rel.with_extension("html");

    // BFS over base-relative page paths (may contain `..`; resolved on dequeue).
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    let mut enqueued: HashSet<PathBuf> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // Output relpath (case-folded) -> source relpath, for collision detection.
    let mut out_paths: std::collections::HashMap<String, PathBuf> =
        std::collections::HashMap::new();
    let mut asset_candidates: Vec<PathBuf> = Vec::new();
    let mut pages = 0usize;

    queue.push_back(entry_rel.clone());
    enqueued.insert(entry_rel);

    while let Some(rel) = queue.pop_front() {
        // Resolve + validate against base_dir.
        let canonical = match std::fs::canonicalize(base_dir.join(&rel)) {
            Ok(c) => c,
            Err(_) => {
                eprintln!(
                    "birta: warning: skipping missing link target: {}",
                    rel.display()
                );
                continue;
            }
        };
        let Ok(safe_rel) = canonical.strip_prefix(&base_dir) else {
            eprintln!(
                "birta: warning: skipping link outside source tree: {}",
                rel.display()
            );
            continue;
        };
        let safe_rel = safe_rel.to_path_buf();
        if !visited.insert(safe_rel.clone()) {
            continue;
        }

        // Output path + collision check.
        let out_rel = safe_rel.with_extension("html");
        let key = out_rel.to_string_lossy().to_ascii_lowercase();
        if let Some(prev) = out_paths.get(&key) {
            if prev != &safe_rel {
                anyhow::bail!(
                    "output path collision: '{}' and '{}' both map to '{}'",
                    prev.display(),
                    safe_rel.display(),
                    out_rel.display()
                );
            }
        } else {
            out_paths.insert(key, safe_rel.clone());
        }

        // Render.
        let markdown = std::fs::read_to_string(&canonical)
            .with_context(|| format!("could not read {}", canonical.display()))?;
        let (content_html, references) = render::render_bundle(&markdown, syntax);
        let source_html = render::render_source(&markdown, syntax);
        let file_stats = render::format_file_stats(&markdown);
        let filename = safe_rel
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());

        let page = template::render_page(&PageOptions {
            filename: &filename,
            file_stats: &file_stats,
            content_html: &content_html,
            source_html: Some(&source_html),
            custom_css: opts.custom_css,
            font_css: opts.font_css,
            show_header: opts.show_header,
            reading_mode: opts.reading_mode,
            raw_mode: opts.raw_mode,
            theme: opts.theme,
            theme_names: &[],
            variant_explicit: opts.variant_explicit,
            static_mode: true,
            keybindings_json: opts.keybindings_json,
            current_path: None,
        });

        // Write (output stays inside out_dir since safe_rel is `..`-free).
        let out_file = out_dir.join(&out_rel);
        if let Some(parent) = out_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(&out_file, &page)
            .with_context(|| format!("could not write {}", out_file.display()))?;
        pages += 1;

        // Enqueue linked pages and gather assets, resolved relative to this file's dir.
        let cur_dir = safe_rel.parent().unwrap_or(Path::new(""));
        for link in references.md_links {
            let target = cur_dir.join(&link);
            if enqueued.insert(target.clone()) {
                queue.push_back(target);
            }
        }
        for asset in references.assets {
            asset_candidates.push(cur_dir.join(&asset));
        }
    }

    // Copy assets (deduped by canonical path, validated against base_dir).
    let mut copied: HashSet<PathBuf> = HashSet::new();
    let mut assets = 0usize;
    for rel in asset_candidates {
        let canonical = match std::fs::canonicalize(base_dir.join(&rel)) {
            Ok(c) => c,
            Err(_) => {
                eprintln!("birta: warning: skipping missing asset: {}", rel.display());
                continue;
            }
        };
        let Ok(safe_rel) = canonical.strip_prefix(&base_dir) else {
            eprintln!(
                "birta: warning: skipping asset outside source tree: {}",
                rel.display()
            );
            continue;
        };
        // Only copy regular files — a link/image pointing at a directory (e.g.
        // `[x](docs/)`) is not a copyable asset and must not abort the export.
        if !canonical.is_file() {
            eprintln!("birta: warning: skipping non-file asset: {}", rel.display());
            continue;
        }
        let safe_rel = safe_rel.to_path_buf();
        if !copied.insert(safe_rel.clone()) {
            continue;
        }
        let dst = out_dir.join(&safe_rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        // Asset copy is best-effort: a single unreadable asset warns rather than
        // aborting the whole bundle.
        if let Err(e) = std::fs::copy(&canonical, &dst) {
            eprintln!(
                "birta: warning: could not copy asset {}: {e}",
                rel.display()
            );
            continue;
        }
        assets += 1;
    }

    Ok(BundleResult {
        entry_html: out_dir.join(entry_out_rel),
        pages,
        assets,
    })
}

fn dir_is_non_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}
