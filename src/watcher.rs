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
                    syntax_theme.as_ref(),
                    std::path::Path::new(&relpath),
                );
                let source = render::render_source(&markdown, syntax_theme.as_ref());
                let _ = tx.send(ContentUpdate {
                    relpath,
                    rendered_html: html,
                    source_html: source,
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
