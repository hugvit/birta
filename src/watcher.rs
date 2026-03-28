use std::path::PathBuf;
use std::sync::Arc;

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use tokio::sync::broadcast;

use crate::render;
use crate::server::AppState;

pub(crate) fn watch(
    path: PathBuf,
    tx: broadcast::Sender<String>,
    state: Arc<AppState>,
) -> anyhow::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let canonical = path.canonicalize()?;
    let watch_dir = canonical.parent().map(|p| p.to_path_buf());

    let rt = tokio::runtime::Handle::current();

    let mut debouncer = new_debouncer(
        std::time::Duration::from_millis(200),
        move |events: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            let events = match events {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("sheen: watcher error: {e}");
                    return;
                }
            };

            let dominated = events.iter().any(|e| {
                e.kind == DebouncedEventKind::Any
                    && e.path
                        .canonicalize()
                        .map(|p| p == canonical)
                        .unwrap_or(false)
            });

            if !dominated {
                return;
            }

            let markdown = match std::fs::read_to_string(&canonical) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("sheen: could not read file: {e}");
                    return;
                }
            };

            // Read current syntax theme from the registry
            let syntax_theme = rt.block_on(async {
                let reg = state.registry.read().await;
                reg.active().active_data().syntax.clone()
            });

            let html = render::render(&markdown, syntax_theme.as_ref());
            let _ = tx.send(html);
        },
    )?;

    let dir = watch_dir.as_deref().unwrap_or(path.as_ref());
    debouncer
        .watcher()
        .watch(dir, notify::RecursiveMode::NonRecursive)?;

    Ok(debouncer)
}
