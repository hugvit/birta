use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use tokio::sync::broadcast;

use crate::render;
use crate::server::{AppState, ContentUpdate};

/// Watch a directory recursively and broadcast rendered HTML for changed `.md` files.
pub(crate) fn watch_dir(
    base_dir: PathBuf,
    tx: broadcast::Sender<ContentUpdate>,
    state: Arc<AppState>,
) -> anyhow::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let canonical_base = base_dir.canonicalize()?;
    let rt = tokio::runtime::Handle::current();

    let mut debouncer = new_debouncer(
        std::time::Duration::from_millis(200),
        move |events: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            let events = match events {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("birta: watcher error: {e}");
                    return;
                }
            };

            // Collect unique .md files that changed
            let mut changed: HashSet<PathBuf> = HashSet::new();
            for event in &events {
                if event.kind != DebouncedEventKind::Any {
                    continue;
                }
                let Ok(path) = event.path.canonicalize() else {
                    continue;
                };
                if !path.starts_with(&canonical_base) {
                    continue;
                }
                match path.extension().and_then(|e| e.to_str()) {
                    Some("md" | "markdown") => {
                        changed.insert(path);
                    }
                    _ => continue,
                }
            }

            if changed.is_empty() {
                return;
            }

            let syntax_theme = rt.block_on(async {
                let reg = state.registry.read().await;
                reg.active().active_data().syntax.clone()
            });

            for path in changed {
                let relpath = path
                    .strip_prefix(&canonical_base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();

                let markdown = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("birta: could not read {relpath}: {e}");
                        continue;
                    }
                };

                let html = render::render_dir(
                    &markdown,
                    render::Style::new(syntax_theme.as_ref(), state.features),
                    std::path::Path::new(&relpath),
                );
                let source = render::render_source(&markdown, syntax_theme.as_ref());
                let file_stats = render::format_file_stats(&markdown);
                let _ = tx.send(ContentUpdate {
                    relpath,
                    rendered_html: html,
                    source_html: source,
                    file_stats,
                });
            }
        },
    )?;

    if let Err(e) = debouncer
        .watcher()
        .watch(&base_dir, notify::RecursiveMode::Recursive)
    {
        eprintln!("birta: warning: directory watch failed: {e}");
        eprintln!("birta: hint: on Linux, try increasing fs.inotify.max_user_watches");
    }

    Ok(debouncer)
}

/// Watch specific files and broadcast a fresh diff when either changes.
///
/// Watches each file's parent directory, because editors that save by
/// renaming replace the file and a watch on the file itself would go stale.
pub(crate) fn watch_files(
    files: Vec<PathBuf>,
    tx: broadcast::Sender<ContentUpdate>,
    state: Arc<AppState>,
) -> anyhow::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    // Device files such as /dev/null (git difftool's side for an added file) never change
    let files: HashSet<PathBuf> = files.into_iter().filter(|f| f.is_file()).collect();
    let dirs: HashSet<PathBuf> = files
        .iter()
        .filter_map(|f| f.parent().map(PathBuf::from))
        .collect();
    let rt = tokio::runtime::Handle::current();
    let watched = files.clone();

    let mut debouncer = new_debouncer(
        std::time::Duration::from_millis(200),
        move |events: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            let events = match events {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("birta: watcher error: {e}");
                    return;
                }
            };
            let touched = events.iter().any(|event| {
                event
                    .path
                    .canonicalize()
                    .is_ok_and(|path| watched.contains(&path))
            });
            let Some(diff) = state.diff.as_ref().filter(|_| touched) else {
                return;
            };

            let html = rt.block_on(async {
                let reg = state.registry.read().await;
                diff.render(reg.active().active_data().syntax.as_ref())
            });
            let html = match html {
                Ok(html) => html,
                Err(e) => {
                    eprintln!("birta: could not re-read diff files: {e}");
                    return;
                }
            };
            rt.block_on(async {
                *state.current_html.write().await = html.clone();
            });
            let _ = tx.send(ContentUpdate {
                relpath: String::new(),
                rendered_html: html,
                source_html: String::new(),
                file_stats: String::new(),
            });
        },
    )?;

    for dir in &dirs {
        if let Err(e) = debouncer
            .watcher()
            .watch(dir, notify::RecursiveMode::NonRecursive)
        {
            eprintln!("birta: warning: watching {} failed: {e}", dir.display());
        }
    }

    Ok(debouncer)
}
