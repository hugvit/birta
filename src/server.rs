use std::net::SocketAddr;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::{Notify, RwLock, broadcast};

use crate::security::{HostGuard, base_headers, guard};
use crate::theme::{ResolvedTheme, ThemeRegistry, Variant};
use crate::{render, template, watcher};

/// A content update broadcast from the watcher or theme change.
#[derive(Clone, Debug)]
pub(crate) struct ContentUpdate {
    pub(crate) relpath: String,
    pub(crate) rendered_html: String,
    pub(crate) source_html: String,
    pub(crate) file_stats: String,
}

/// An outbound event fanned out to every WebSocket connection.
#[derive(Clone, Debug)]
pub(crate) enum WsEvent {
    Content(ContentUpdate),
    /// Ready-to-send JSON (e.g. `theme_update` messages).
    Raw(Arc<str>),
    Scroll(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Stdin,
    Serve,
}

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);
const HUB_CAPACITY: usize = 64;
const FAVICON: &[u8] = include_bytes!("../assets/favicon.png");

/// Options for starting the server.
pub struct ServerOptions {
    pub port: u16,
    pub no_open: bool,
    pub custom_css: Option<String>,
    pub font_css: Option<String>,
    pub theme: ResolvedTheme,
    pub enable_swap: bool,
    pub enable_toggle: bool,
    pub show_header: bool,
    pub reading_mode: bool,
    pub raw_mode: bool,
    pub keybindings_json: String,
    /// Variant was explicitly chosen via `--light`/`--dark` or config.
    pub variant_explicit: bool,
}

pub(crate) struct AppState {
    /// Bound port, from the listener rather than the requested (possibly 0) port.
    #[cfg_attr(not(test), expect(dead_code, reason = "not read by any handler yet"))]
    pub(crate) port: u16,
    #[cfg_attr(not(test), expect(dead_code, reason = "not read by any handler yet"))]
    pub(crate) mode: Mode,
    pub(crate) base_dir: PathBuf,
    pub(crate) source_file: Option<PathBuf>,
    pub(crate) filename: String,
    /// Relative path of the initial file from base_dir (e.g. "README.md").
    pub(crate) initial_relpath: Option<String>,
    pub(crate) custom_css: Option<String>,
    /// Raw rendered HTML (not JSON-wrapped).
    pub(crate) current_html: RwLock<String>,
    /// Syntax-highlighted source HTML for the code view.
    pub(crate) current_source_html: RwLock<String>,
    /// Formatted file stats string (e.g. "215 lines (154 loc) · 5.25 KB") for the raw-mode header.
    pub(crate) current_file_stats: RwLock<String>,
    pub(crate) hub: broadcast::Sender<WsEvent>,
    pub(crate) connections: AtomicUsize,
    pub(crate) all_disconnected: Notify,
    pub(crate) registry: RwLock<ThemeRegistry>,
    pub(crate) enable_toggle: bool,
    pub(crate) font_css: Option<String>,
    pub(crate) show_header: bool,
    pub(crate) reading_mode: bool,
    pub(crate) raw_mode: bool,
    pub(crate) keybindings_json: String,
    /// Variant was explicitly chosen via `--light`/`--dark` or config.
    pub(crate) variant_explicit: bool,
    /// Epoch seconds of the last HTTP request (prevents premature auto-shutdown).
    pub(crate) last_request: AtomicU64,
}

/// Everything [`AppState::new`] needs; shared channels and counters are created there.
pub(crate) struct AppInit {
    pub(crate) port: u16,
    pub(crate) mode: Mode,
    pub(crate) base_dir: PathBuf,
    pub(crate) source_file: Option<PathBuf>,
    pub(crate) filename: String,
    pub(crate) initial_relpath: Option<String>,
    pub(crate) html: String,
    pub(crate) source_html: String,
    pub(crate) file_stats: String,
    pub(crate) registry: ThemeRegistry,
    pub(crate) custom_css: Option<String>,
    pub(crate) font_css: Option<String>,
    pub(crate) enable_toggle: bool,
    pub(crate) show_header: bool,
    pub(crate) reading_mode: bool,
    pub(crate) raw_mode: bool,
    pub(crate) keybindings_json: String,
    pub(crate) variant_explicit: bool,
}

impl AppInit {
    fn from_opts(opts: ServerOptions, listener: &TcpListener, mode: Mode) -> anyhow::Result<Self> {
        let mut registry = ThemeRegistry::new(opts.theme);
        if opts.enable_swap {
            registry.discover_all();
        }
        Ok(Self {
            port: listener.local_addr()?.port(),
            mode,
            base_dir: PathBuf::new(),
            source_file: None,
            filename: String::new(),
            initial_relpath: None,
            html: String::new(),
            source_html: String::new(),
            file_stats: String::new(),
            registry,
            custom_css: opts.custom_css,
            font_css: opts.font_css,
            enable_toggle: opts.enable_toggle,
            show_header: opts.show_header,
            reading_mode: opts.reading_mode,
            raw_mode: opts.raw_mode,
            keybindings_json: opts.keybindings_json,
            variant_explicit: opts.variant_explicit,
        })
    }

    pub(crate) fn stdin(
        markdown: &str,
        listener: &TcpListener,
        opts: ServerOptions,
    ) -> anyhow::Result<Self> {
        let mut init = Self::from_opts(opts, listener, Mode::Stdin)?;
        let syntax = init.registry.active().active_data().syntax.as_ref();
        init.html = render::render(markdown, syntax);
        init.source_html = render::render_source(markdown, syntax);
        init.file_stats = render::format_file_stats(markdown);
        init.base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        init.filename = "stdin".to_string();
        Ok(init)
    }

    pub(crate) fn serve(
        file: &StdPath,
        listener: &TcpListener,
        opts: ServerOptions,
    ) -> anyhow::Result<Self> {
        let markdown = std::fs::read_to_string(file)?;

        let canonical_file = file.canonicalize()?;
        let base_dir = canonical_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let initial_relpath = canonical_file
            .strip_prefix(&base_dir)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());

        let mut init = Self::from_opts(opts, listener, Mode::Serve)?;
        let syntax = init.registry.active().active_data().syntax.as_ref();
        init.html = if let Some(ref relpath) = initial_relpath {
            render::render_dir(&markdown, syntax, StdPath::new(relpath))
        } else {
            render::render(&markdown, syntax)
        };
        init.source_html = render::render_source(&markdown, syntax);
        init.file_stats = render::format_file_stats(&markdown);
        init.filename = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());
        init.base_dir = base_dir;
        init.source_file = Some(canonical_file);
        init.initial_relpath = initial_relpath;
        Ok(init)
    }
}

impl AppState {
    pub(crate) fn new(init: AppInit) -> Arc<Self> {
        let (hub, _) = broadcast::channel(HUB_CAPACITY);
        Arc::new(Self {
            port: init.port,
            mode: init.mode,
            base_dir: init.base_dir,
            source_file: init.source_file,
            filename: init.filename,
            initial_relpath: init.initial_relpath,
            custom_css: init.custom_css,
            current_html: RwLock::new(init.html),
            current_source_html: RwLock::new(init.source_html),
            current_file_stats: RwLock::new(init.file_stats),
            hub,
            connections: AtomicUsize::new(0),
            all_disconnected: Notify::new(),
            registry: RwLock::new(init.registry),
            enable_toggle: init.enable_toggle,
            font_css: init.font_css,
            show_header: init.show_header,
            reading_mode: init.reading_mode,
            raw_mode: init.raw_mode,
            keybindings_json: init.keybindings_json,
            variant_explicit: init.variant_explicit,
            last_request: AtomicU64::new(now_secs()),
        })
    }

    /// Broadcast a rendered update, refreshing the cache first when it is the initial file.
    /// The cache is written here because a lagging hub subscriber would lose the update.
    pub(crate) async fn publish_content(&self, update: ContentUpdate) {
        if self.initial_relpath.as_deref() == Some(&update.relpath) {
            *self.current_html.write().await = update.rendered_html.clone();
            *self.current_source_html.write().await = update.source_html.clone();
            *self.current_file_stats.write().await = update.file_stats.clone();
        }
        let _ = self.hub.send(WsEvent::Content(update));
    }
}

pub async fn run(file: PathBuf, opts: ServerOptions) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], opts.port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    eprintln!("birta: serving {filename} at http://{actual_addr}");

    if !opts.no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("birta: could not open browser: {e}");
        }
    }

    start(file, listener, opts).await
}

/// Serve markdown read from stdin (no file watching).
pub async fn run_stdin(markdown: &str, opts: ServerOptions) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], opts.port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    eprintln!("birta: serving stdin at http://{actual_addr}");

    if !opts.no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("birta: could not open browser: {e}");
        }
    }

    let state = AppState::new(AppInit::stdin(markdown, &listener, opts)?);

    let app = router(state.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;

    Ok(())
}

/// Start serving a markdown file on the given listener.
pub async fn start(
    file: PathBuf,
    listener: TcpListener,
    opts: ServerOptions,
) -> anyhow::Result<()> {
    let state = AppState::new(AppInit::serve(&file, &listener, opts)?);

    let _debouncer = watcher::watch_dir(state.base_dir.clone(), Arc::clone(&state))?;

    let app = router(state.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;

    Ok(())
}

async fn shutdown_signal(state: Arc<AppState>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl-c");
        eprintln!("\nbirta: shutting down...");
    };

    let auto_shutdown = async {
        loop {
            state.all_disconnected.notified().await;

            if state.connections.load(Ordering::Relaxed) == 0 {
                break;
            }
        }

        tokio::time::sleep(SHUTDOWN_GRACE_PERIOD).await;

        if state.connections.load(Ordering::Relaxed) == 0
            && now_secs().saturating_sub(state.last_request.load(Ordering::Relaxed))
                >= SHUTDOWN_GRACE_PERIOD.as_secs()
        {
            eprintln!("birta: all tabs closed, shutting down...");
        } else {
            Box::pin(auto_shutdown_loop(state)).await;
        }
    };

    tokio::select! {
        () = ctrl_c => {},
        () = auto_shutdown => {},
    }
}

async fn auto_shutdown_loop(state: Arc<AppState>) {
    loop {
        state.all_disconnected.notified().await;

        if state.connections.load(Ordering::Relaxed) > 0 {
            continue;
        }

        tokio::time::sleep(SHUTDOWN_GRACE_PERIOD).await;

        if state.connections.load(Ordering::Relaxed) == 0
            && now_secs().saturating_sub(state.last_request.load(Ordering::Relaxed))
                >= SHUTDOWN_GRACE_PERIOD.as_secs()
        {
            eprintln!("birta: all tabs closed, shutting down...");
            return;
        }
    }
}

fn router(state: Arc<AppState>) -> Router {
    let pages = Router::new()
        .route("/", get(index_handler))
        .route("/view/{*path}", get(view_handler));
    let frags = Router::new()
        .route("/render/{*path}", get(render_handler))
        .route("/local/{*path}", get(local_file_handler))
        .route("/favicon.png", get(favicon_handler))
        .route("/favicon.ico", get(favicon_handler));
    let live = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_handler))
        .route("/scroll/{line}", post(scroll_handler));
    let assets = Router::new().route("/_birta/{*asset}", get(asset_handler));

    pages
        .merge(frags)
        .merge(live)
        .merge(assets)
        .with_state(state)
        // The guard wraps every group; base headers sit outside it so rejections carry them too.
        .layer(from_fn_with_state(HostGuard::new(), guard))
        .layer(from_fn(base_headers))
}

/// Reserved for embedded assets; nothing is served yet.
async fn asset_handler() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn favicon_handler() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], FAVICON).into_response()
}

async fn index_handler(State(state): State<Arc<AppState>>) -> Response {
    if let Some(relpath) = &state.initial_relpath {
        return Redirect::temporary(&format!("/view/{relpath}")).into_response();
    }

    // stdin mode — render inline
    let registry = state.registry.read().await;
    let theme = registry.active();
    let theme_names: Vec<&str> = registry.theme_names();
    let content_html = state.current_html.read().await;
    let source_html = state.current_source_html.read().await;
    let file_stats = state.current_file_stats.read().await;
    let page = template::render_page(&template::PageOptions {
        filename: &state.filename,
        file_stats: &file_stats,
        content_html: &content_html,
        source_html: Some(&source_html),
        custom_css: state.custom_css.as_deref(),
        font_css: state.font_css.as_deref(),
        show_header: state.show_header,
        reading_mode: state.reading_mode,
        raw_mode: state.raw_mode,
        theme,
        theme_names: &theme_names,
        variant_explicit: state.variant_explicit,
        static_mode: false,
        keybindings_json: &state.keybindings_json,
        current_path: None,
    });
    Html(page).into_response()
}

async fn scroll_handler(Path(line): Path<u32>, State(state): State<Arc<AppState>>) -> StatusCode {
    let _ = state.hub.send(WsEvent::Scroll(line));
    StatusCode::NO_CONTENT
}

/// Canonicalize a relative path and verify it stays within `base_dir`.
async fn resolve_safe_path(base_dir: &StdPath, relative: &str) -> Result<PathBuf, StatusCode> {
    let joined = base_dir.join(relative);
    let canonical = tokio::fs::canonicalize(&joined)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !canonical.starts_with(base_dir) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(canonical)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Render and serve a markdown file from the served directory.
async fn view_handler(Path(path): Path<String>, State(state): State<Arc<AppState>>) -> Response {
    state.last_request.store(now_secs(), Ordering::Relaxed);

    let canonical = match resolve_safe_path(&state.base_dir, &path).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };

    match canonical.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => {}
        _ => return (StatusCode::BAD_REQUEST, "not a markdown file").into_response(),
    }

    let markdown = match tokio::fs::read_to_string(&canonical).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "could not read file").into_response(),
    };

    let syntax_theme = {
        let reg = state.registry.read().await;
        reg.active().active_data().syntax.clone()
    };

    let html = render::render_dir(&markdown, syntax_theme.as_ref(), StdPath::new(&path));
    let source_html = render::render_source(&markdown, syntax_theme.as_ref());
    let file_stats = render::format_file_stats(&markdown);

    let filename = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let registry = state.registry.read().await;
    let theme = registry.active();
    let theme_names: Vec<&str> = registry.theme_names();
    let page = template::render_page(&template::PageOptions {
        filename: &filename,
        file_stats: &file_stats,
        content_html: &html,
        source_html: Some(&source_html),
        custom_css: state.custom_css.as_deref(),
        font_css: state.font_css.as_deref(),
        show_header: state.show_header,
        reading_mode: state.reading_mode,
        raw_mode: state.raw_mode,
        theme,
        theme_names: &theme_names,
        variant_explicit: state.variant_explicit,
        static_mode: false,
        keybindings_json: &state.keybindings_json,
        current_path: Some(&path),
    });
    Html(page).into_response()
}

/// Return just the rendered HTML fragment for a markdown file (no page chrome).
/// Used by the client to re-render content after theme changes.
/// Accepts `?mode=source` to return highlighted source instead of rendered HTML.
async fn render_handler(
    Path(path): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let canonical = match resolve_safe_path(&state.base_dir, &path).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };

    match canonical.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => {}
        _ => return (StatusCode::BAD_REQUEST, "not a markdown file").into_response(),
    }

    let markdown = match tokio::fs::read_to_string(&canonical).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "could not read file").into_response(),
    };

    let syntax_theme = {
        let reg = state.registry.read().await;
        reg.active().active_data().syntax.clone()
    };

    if params.get("mode").map(|m| m.as_str()) == Some("source") {
        let source = render::render_source(&markdown, syntax_theme.as_ref());
        return Html(source).into_response();
    }

    let html = render::render_dir(&markdown, syntax_theme.as_ref(), StdPath::new(&path));
    Html(html).into_response()
}

/// Handle incoming WebSocket JSON messages from the browser.
async fn handle_ws_message(text: &str, state: &AppState) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    match msg.get("type").and_then(|t| t.as_str()) {
        Some("checkbox") => {
            let line = msg.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
            let checked = msg
                .get("checked")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            let relpath = msg.get("path").and_then(|p| p.as_str()).unwrap_or("");
            if let Err(e) = toggle_checkbox(state, line, checked, relpath) {
                eprintln!("birta: checkbox toggle failed: {e}");
            }
        }
        Some("theme_change") => {
            if let Some(theme_name) = msg.get("theme").and_then(|t| t.as_str()) {
                handle_theme_change(state, theme_name).await;
            }
        }
        Some("variant_change") => {
            if let Some(variant_str) = msg.get("variant").and_then(|v| v.as_str())
                && let Some(variant) = Variant::parse(variant_str)
            {
                handle_variant_change(state, variant).await;
            }
        }
        _ => {}
    }
}

/// Re-render and broadcast a theme update to all clients.
async fn broadcast_theme_update(state: &AppState) {
    let registry = state.registry.read().await;
    let theme = registry.active();
    let active = theme.active_data();

    // Re-render markdown with new syntax theme
    let (html, source) = if let Some(source_file) = &state.source_file {
        match std::fs::read_to_string(source_file) {
            Ok(markdown) => {
                let rendered = if let Some(relpath) = &state.initial_relpath {
                    render::render_dir(&markdown, active.syntax.as_ref(), StdPath::new(relpath))
                } else {
                    render::render(&markdown, active.syntax.as_ref())
                };
                let source = render::render_source(&markdown, active.syntax.as_ref());
                (rendered, source)
            }
            Err(e) => {
                eprintln!("birta: failed to re-read file for theme change: {e}");
                return;
            }
        }
    } else {
        // stdin mode — use current HTML since we can't re-render
        let html = state.current_html.read().await.clone();
        let source = state.current_source_html.read().await.clone();
        (html, source)
    };

    let (css_vars, theme_attr) = if theme.is_github() {
        (String::new(), String::new())
    } else {
        (active.css_vars.clone(), theme.name.clone())
    };

    let has_toggle = theme.has_toggle() && state.enable_toggle;

    let msg = serde_json::json!({
        "type": "theme_update",
        "css_vars": css_vars,
        "html": html,
        "source": source,
        "path": state.initial_relpath.as_deref().unwrap_or(""),
        "theme_name": theme.name,
        "theme_attr": theme_attr,
        "variants": theme.variant_names(),
        "active_variant": theme.active_variant.as_str(),
        "has_toggle": has_toggle,
    });

    // Update stored content
    *state.current_html.write().await = html;
    *state.current_source_html.write().await = source;

    let _ = state.hub.send(WsEvent::Raw(msg.to_string().into()));
}

async fn handle_theme_change(state: &AppState, theme_name: &str) {
    let mut registry = state.registry.write().await;
    if let Err(e) = registry.set_active(theme_name) {
        eprintln!("birta: theme change failed: {e}");
        return;
    }
    drop(registry);
    broadcast_theme_update(state).await;
}

async fn handle_variant_change(state: &AppState, variant: Variant) {
    let mut registry = state.registry.write().await;
    let changed = registry.active().active_variant != variant;
    registry.set_variant(variant);
    drop(registry);
    if changed {
        broadcast_theme_update(state).await;
    }
}

/// Toggle a checkbox in a source file at the given line.
fn toggle_checkbox(
    state: &AppState,
    line: usize,
    checked: bool,
    relpath: &str,
) -> anyhow::Result<()> {
    let path = if !relpath.is_empty() {
        let joined = state.base_dir.join(relpath);
        let canonical = joined
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("invalid path: {e}"))?;
        if !canonical.starts_with(&state.base_dir) {
            anyhow::bail!("path traversal not allowed");
        }
        canonical
    } else {
        state
            .source_file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no source file (stdin mode)"))?
    };
    let path = &path;

    let content = std::fs::read_to_string(path)?;
    let mut lines: Vec<&str> = content.lines().collect();

    if line == 0 || line > lines.len() {
        anyhow::bail!("line {line} out of range");
    }

    let target = lines[line - 1];
    let new_line = if checked {
        target.replacen("[ ]", "[x]", 1)
    } else {
        target.replacen("[x]", "[ ]", 1)
    };

    if new_line == target {
        return Ok(());
    }

    lines[line - 1] = &new_line;

    let mut output = lines.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }

    std::fs::write(path, output)?;
    Ok(())
}

async fn local_file_handler(
    Path(path): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let file_path = match resolve_safe_path(&state.base_dir, &path).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };

    let content = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let content_type = match file_path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };

    ([(header::CONTENT_TYPE, content_type)], content).into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

fn content_frame(path: &str, html: &str, source: &str, file_stats: &str) -> String {
    serde_json::json!({
        "type": "content",
        "html": html,
        "source": source,
        "file_stats": file_stats,
        "path": path,
    })
    .to_string()
}

async fn cached_content_frame(state: &AppState) -> String {
    let html = state.current_html.read().await.clone();
    let source = state.current_source_html.read().await.clone();
    let file_stats = state.current_file_stats.read().await.clone();
    let path = state.initial_relpath.as_deref().unwrap_or("");
    content_frame(path, &html, &source, &file_stats)
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    state.connections.fetch_add(1, Ordering::Relaxed);

    let init_msg = cached_content_frame(&state).await;
    if socket.send(Message::Text(init_msg.into())).await.is_err() {
        state.connections.fetch_sub(1, Ordering::Relaxed);
        state.all_disconnected.notify_one();
        return;
    }

    let mut rx = state.hub.subscribe();

    loop {
        let frame = tokio::select! {
            result = rx.recv() => match result {
                Ok(WsEvent::Content(update)) => content_frame(
                    &update.relpath,
                    &update.rendered_html,
                    &update.source_html,
                    &update.file_stats,
                ),
                Ok(WsEvent::Raw(json)) => json.to_string(),
                Ok(WsEvent::Scroll(line)) => {
                    serde_json::json!({"type": "scroll", "line": line}).to_string()
                }
                // Dropped events may include content, so resync this connection from the cache.
                Err(broadcast::error::RecvError::Lagged(_)) => cached_content_frame(&state).await,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_ws_message(&text, &state).await;
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
                continue;
            }
        };
        if socket.send(Message::Text(frame.into())).await.is_err() {
            break;
        }
    }

    state.connections.fetch_sub(1, Ordering::Relaxed);
    state.all_disconnected.notify_one();
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::theme::{self, ThemeVariants, VariantData};

    fn github_theme() -> theme::ResolvedTheme {
        theme::ResolvedTheme {
            name: "github".to_string(),
            variants: ThemeVariants::Both {
                light: Box::new(VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
                dark: Box::new(VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
            },
            active_variant: Variant::Dark,
        }
    }

    fn test_opts() -> ServerOptions {
        ServerOptions {
            port: 0,
            no_open: true,
            custom_css: None,
            font_css: None,
            theme: github_theme(),
            enable_swap: false,
            enable_toggle: true,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            keybindings_json: "{}".to_string(),
            variant_explicit: false,
        }
    }

    fn test_state(customize: impl FnOnce(&mut AppInit)) -> Arc<AppState> {
        let mut init = AppInit {
            port: 0,
            mode: Mode::Stdin,
            base_dir: PathBuf::from("."),
            source_file: None,
            filename: "test.md".to_string(),
            initial_relpath: None,
            html: "<p>hello</p>".to_string(),
            source_html: String::new(),
            file_stats: String::new(),
            registry: ThemeRegistry::new(github_theme()),
            custom_css: None,
            font_css: None,
            enable_toggle: true,
            show_header: true,
            reading_mode: false,
            raw_mode: false,
            keybindings_json: "{}".to_string(),
            variant_explicit: false,
        };
        customize(&mut init);
        AppState::new(init)
    }

    fn test_router() -> Router {
        router(test_state(|_| {}))
    }

    #[tokio::test]
    async fn index_returns_200_with_markdown_body() {
        let app = test_router();

        let response = app.oneshot(get("/")).await.unwrap();

        assert_eq!(response.status(), 200);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("markdown-body"),
            "response should contain markdown-body class"
        );
        assert!(
            html.contains("<p>hello</p>"),
            "response should contain rendered content"
        );
    }

    /// Every registered path with the method it is routed for.
    const ROUTES: [(&str, &str); 11] = [
        ("GET", "/"),
        ("GET", "/view/x.md"),
        ("GET", "/render/x.md"),
        ("GET", "/local/a.png"),
        ("GET", "/_birta/x"),
        ("GET", "/_birta/nested/review.css"),
        ("GET", "/health"),
        ("GET", "/favicon.png"),
        ("GET", "/favicon.ico"),
        ("POST", "/scroll/1"),
        ("GET", "/ws"),
    ];

    fn wrong_method(method: &str) -> &'static str {
        if method == "GET" { "POST" } else { "GET" }
    }

    const LOCAL_HOST: &str = "127.0.0.1:1";

    fn raw_request(method: &str, uri: &str, headers: &[(&str, &str)]) -> axum::http::Request<Body> {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    /// A request as a local client sends it; the guard refuses requests without Host.
    fn request(method: &str, uri: &str) -> axum::http::Request<Body> {
        raw_request(method, uri, &[("host", LOCAL_HOST)])
    }

    fn get(uri: &str) -> axum::http::Request<Body> {
        request("GET", uri)
    }

    #[tokio::test]
    async fn router_builds_with_all_groups() {
        // A wrong method gets 405 only when the path is routed, so it proves registration.
        let mut cases = vec![
            ("GET", "/", 200),
            ("GET", "/health", 200),
            ("GET", "/favicon.png", 200),
            ("GET", "/favicon.ico", 200),
            ("GET", "/_birta/review.js", 404),
            ("POST", "/not-a-route", 404),
        ];
        cases.extend(ROUTES.map(|(method, uri)| (wrong_method(method), uri, 405)));
        let app = test_router();
        for (method, uri, expected) in cases {
            let response = app.clone().oneshot(request(method, uri)).await.unwrap();
            assert_eq!(response.status(), expected, "{method} {uri}");
        }
    }

    type Rows<'a> = [(&'a [(&'a str, &'a str)], u16)];
    type RouteRows<'a> = [(&'a str, &'a str, &'a [(&'a str, &'a str)], u16)];

    /// Sends each row with exactly its headers; returns every status mismatch.
    async fn route_mismatches(rows: &RouteRows<'_>) -> Vec<String> {
        let app = test_router();
        let mut mismatches = Vec::new();
        for (method, uri, headers, want) in rows {
            let got = app
                .clone()
                .oneshot(raw_request(method, uri, headers))
                .await
                .unwrap()
                .status();
            if got != *want {
                mismatches.push(format!(
                    "{method} {uri} {headers:?}: got {got}, want {want}"
                ));
            }
        }
        mismatches
    }

    async fn health_mismatches(rows: &Rows<'_>) -> Vec<String> {
        let rows: Vec<_> = rows
            .iter()
            .map(|(headers, want)| ("GET", "/health", *headers, *want))
            .collect();
        route_mismatches(&rows).await
    }

    #[tokio::test]
    async fn guard_rejects_foreign_host() {
        let rows: &Rows = &[
            (&[("host", "evil.com")], 421),
            (&[("host", "evil.com:1")], 421),
            (&[("host", "127.0.0.1.evil.com")], 421),
            (&[("host", "127.0.0.1.evil.com:1")], 421),
            (&[("host", "localhost.")], 421),
            (&[("host", "localhost.:1")], 421),
            (&[("host", "127.0.0.1:99999")], 421),
            (&[("host", "127.0.0.1:abc")], 421),
            (&[("host", "127.0.0.2:1")], 421),
            (&[("host", "0.0.0.0:1")], 421),
            (&[("host", "[::2]:1")], 421),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_missing_host() {
        let rows: &Rows = &[(&[], 421), (&[("host", "")], 421)];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_allows_loopback_hosts() {
        let rows: &Rows = &[
            (&[("host", "127.0.0.1")], 200),
            (&[("host", "127.0.0.1:1")], 200),
            (&[("host", "localhost")], 200),
            (&[("host", "localhost:3456")], 200),
            (&[("host", "LOCALHOST:5")], 200),
            (&[("host", "[::1]")], 200),
            (&[("host", "[::1]:9")], 200),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_foreign_origin() {
        let rows: &Rows = &[
            (
                &[("host", LOCAL_HOST), ("origin", "http://127.0.0.1:2")],
                403,
            ),
            (&[("host", LOCAL_HOST), ("origin", "http://127.0.0.1")], 403),
            (&[("host", LOCAL_HOST), ("origin", "null")], 403),
            (
                &[("host", LOCAL_HOST), ("origin", "https://127.0.0.1:1")],
                403,
            ),
            (
                &[("host", LOCAL_HOST), ("origin", "http://localhost:1")],
                403,
            ),
            (&[("host", LOCAL_HOST), ("origin", "http://evil.com")], 403),
            (&[("host", LOCAL_HOST), ("origin", "")], 403),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_allows_same_origin() {
        let rows: &Rows = &[
            (&[("host", LOCAL_HOST)], 200),
            (
                &[("host", LOCAL_HOST), ("origin", "http://127.0.0.1:1")],
                200,
            ),
            (
                &[("host", "localhost:5"), ("origin", "http://localhost:5")],
                200,
            ),
            (&[("host", "[::1]:9"), ("origin", "http://[::1]:9")], 200),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_cross_site_fetch() {
        let fetch = |site, mode| {
            [
                ("host", LOCAL_HOST),
                ("sec-fetch-site", site),
                ("sec-fetch-mode", mode),
            ]
        };
        let rows: &Rows = &[
            (
                &[("host", LOCAL_HOST), ("sec-fetch-site", "cross-site")],
                403,
            ),
            (&fetch("cross-site", "navigate"), 403),
            (&fetch("cross-site", "cors"), 403),
            (&fetch("cross-site", "no-cors"), 403),
            (&fetch("same-site", "cors"), 403),
            (&fetch("same-site", "no-cors"), 403),
            (&fetch("same-site", "websocket"), 403),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_allows_same_site_navigation_and_same_origin() {
        let fetch = |site, mode| {
            [
                ("host", LOCAL_HOST),
                ("sec-fetch-site", site),
                ("sec-fetch-mode", mode),
            ]
        };
        let rows: &Rows = &[
            (&fetch("same-site", "navigate"), 200),
            (&fetch("same-origin", "cors"), 200),
            (&fetch("same-origin", "websocket"), 200),
            (&fetch("none", "navigate"), 200),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_rebinding_shape() {
        // A rebinding page sends a Host and Origin that agree; Host must fail first.
        let rebound: &[(&str, &str)] = &[("host", "evil.com:1"), ("origin", "http://evil.com:1")];
        let rows: &RouteRows = &[
            ("GET", "/health", rebound, 421),
            ("GET", "/ws", rebound, 421),
            ("POST", "/scroll/1", rebound, 421),
        ];
        let mismatches = route_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_sec_fetch_does_not_bypass_origin_or_host() {
        let rows: &RouteRows = &[
            (
                "GET",
                "/health",
                &[
                    ("host", LOCAL_HOST),
                    ("sec-fetch-site", "same-origin"),
                    ("sec-fetch-mode", "cors"),
                    ("origin", "http://evil.com"),
                ],
                403,
            ),
            (
                "GET",
                "/health",
                &[
                    ("host", LOCAL_HOST),
                    ("sec-fetch-site", "none"),
                    ("sec-fetch-mode", "navigate"),
                    ("origin", "null"),
                ],
                403,
            ),
            (
                "POST",
                "/scroll/1",
                &[
                    ("host", "localhost:1"),
                    ("origin", "http://localhost:3000"),
                    ("sec-fetch-site", "same-site"),
                    ("sec-fetch-mode", "navigate"),
                ],
                403,
            ),
            (
                "GET",
                "/health",
                &[("host", "evil.com"), ("sec-fetch-site", "same-origin")],
                421,
            ),
        ];
        let mismatches = route_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_origin_prefix_tricks() {
        let rows: &Rows = &[
            (
                &[("host", LOCAL_HOST), ("origin", "http://127.0.0.1:10")],
                403,
            ),
            (
                &[
                    ("host", LOCAL_HOST),
                    ("origin", "http://127.0.0.1:1.evil.com"),
                ],
                403,
            ),
            (
                &[("host", LOCAL_HOST), ("origin", "http://127.0.0.1:1/")],
                403,
            ),
            (
                &[("host", LOCAL_HOST), ("origin", "http://127.0.0.1:1:1")],
                403,
            ),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_host_lookalikes() {
        let rows: &Rows = &[
            (&[("host", "evil.localhost:1")], 421),
            (&[("host", "localhost.evil.com:1")], 421),
            (&[("host", "localhostevil.com")], 421),
            (&[("host", "127.0.0.1.:1")], 421),
            (&[("host", "[0:0:0:0:0:0:0:1]:1")], 421),
            (&[("host", "[::ffff:127.0.0.1]:1")], 421),
            (&[("host", "::1")], 421),
            (&[("host", "::1:9")], 421),
            (&[("host", "[::1")], 421),
            (&[("host", "[::1]x")], 421),
            (&[("host", "evil.com@127.0.0.1:1")], 421),
            (&[("host", "127.0.0.1:+1")], 421),
            (&[("host", "127.0.0.1:")], 421),
            (&[("host", "127.0.0.1:1:2")], 421),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_rejects_duplicate_host() {
        let rows: &Rows = &[
            (&[("host", LOCAL_HOST), ("host", "evil.com")], 421),
            (&[("host", "evil.com"), ("host", LOCAL_HOST)], 421),
            (&[("host", LOCAL_HOST), ("host", LOCAL_HOST)], 421),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_ignores_forwarded_headers() {
        let rows: &Rows = &[
            (
                &[("host", "evil.com"), ("x-forwarded-host", LOCAL_HOST)],
                421,
            ),
            (
                &[("host", "evil.com"), ("forwarded", "host=127.0.0.1:1")],
                421,
            ),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_absolute_form() {
        let rows: &RouteRows = &[
            (
                "GET",
                "http://evil.com/health",
                &[("host", LOCAL_HOST)],
                421,
            ),
            ("GET", "http://127.0.0.1:1/health", &[], 421),
            (
                "GET",
                "http://127.0.0.1:1/health",
                &[("host", LOCAL_HOST)],
                200,
            ),
        ];
        let mismatches = route_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_same_site_without_mode() {
        let rows: &Rows = &[
            (
                &[("host", LOCAL_HOST), ("sec-fetch-site", "same-site")],
                403,
            ),
            (
                &[("host", LOCAL_HOST), ("sec-fetch-site", "same-origin")],
                200,
            ),
            (&[("host", LOCAL_HOST), ("sec-fetch-site", "none")], 200),
        ];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_allows_node_fetch_shape() {
        // Node's fetch (the e2e /health poll) sends a mode but no site and no Origin.
        let rows: &Rows = &[(&[("host", LOCAL_HOST), ("sec-fetch-mode", "cors")], 200)];
        let mismatches = health_mismatches(rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn guard_covers_every_route() {
        // Includes wrong-method (405), OPTIONS/HEAD and unrouted (fallback 404) responses.
        let mut targets: Vec<(&str, &str)> = Vec::new();
        for (method, uri) in ROUTES {
            targets.extend([(method, uri), (wrong_method(method), uri)]);
            targets.extend([("OPTIONS", uri), ("HEAD", uri)]);
        }
        targets.extend([("GET", "/not-a-route"), ("POST", "/not-a-route")]);
        let rejections: [(&[(&str, &str)], u16); 4] = [
            (&[("host", "evil.com")], 421),
            (&[], 421),
            (&[("host", LOCAL_HOST), ("origin", "http://evil.com")], 403),
            (
                &[("host", LOCAL_HOST), ("sec-fetch-site", "cross-site")],
                403,
            ),
        ];
        let rows: Vec<_> = targets
            .iter()
            .flat_map(|(method, uri)| {
                rejections.map(|(headers, want)| (*method, *uri, headers, want))
            })
            .collect();
        let mismatches = route_mismatches(&rows).await;
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[tokio::test]
    async fn base_headers_on_every_route() {
        let expected = [
            ("x-content-type-options", "nosniff"),
            ("referrer-policy", "no-referrer"),
            ("x-frame-options", "DENY"),
        ];
        let mut requests: Vec<_> = ROUTES.map(|(method, uri)| request(method, uri)).into();
        // Guard rejections carry them too.
        requests.push(raw_request("GET", "/health", &[("host", "evil.com")]));
        requests.push(raw_request(
            "GET",
            "/health",
            &[("host", LOCAL_HOST), ("origin", "http://evil.com")],
        ));

        let app = test_router();
        let mut missing = Vec::new();
        for req in requests {
            let label = format!("{} {} {:?}", req.method(), req.uri(), req.headers());
            let response = app.clone().oneshot(req).await.unwrap();
            for (name, value) in expected {
                if response.headers().get(name).is_none_or(|v| v != value) {
                    missing.push(format!("{label} ({}): {name}", response.status()));
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    const BARRIER: u32 = 1_000_001;
    const SENTINEL: u32 = 1_000_002;

    async fn serve(state: &Arc<AppState>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = router(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    /// Connects and consumes the initial `content` frame.
    async fn connect(port: u16) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        assert_eq!(next_json(&mut ws).await["type"], "content");
        ws
    }

    async fn next_json(ws: &mut Client) -> serde_json::Value {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        loop {
            match ws.next().await.expect("socket closed").expect("ws error") {
                TMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
                TMessage::Close(_) => panic!("socket closed"),
                _ => {}
            }
        }
    }

    /// Reads until scroll `line` arrives, returning the `content` frames seen on the way.
    async fn contents_until(ws: &mut Client, line: u32) -> Vec<serde_json::Value> {
        let mut contents = Vec::new();
        loop {
            let msg = next_json(ws).await;
            if msg["type"] == "content" {
                contents.push(msg);
            } else if msg["type"] == "scroll" && msg["line"] == line {
                return contents;
            }
        }
    }

    async fn sync(state: &AppState, ws: &mut Client, line: u32) -> Vec<serde_json::Value> {
        state.hub.send(WsEvent::Scroll(line)).unwrap();
        contents_until(ws, line).await
    }

    // Tests run on a current_thread runtime, so the server can't drain the hub mid-flood.
    fn flood(state: &AppState, count: u32) {
        for line in 0..count {
            state.hub.send(WsEvent::Scroll(line)).unwrap();
        }
    }

    async fn within<F: std::future::Future<Output = ()>>(fut: F) {
        tokio::time::timeout(Duration::from_secs(5), fut)
            .await
            .expect("timed out");
    }

    fn doc_state() -> Arc<AppState> {
        test_state(|init| init.initial_relpath = Some("doc.md".to_string()))
    }

    #[tokio::test]
    async fn ws_survives_hub_lag() {
        let state = doc_state();
        let port = serve(&state).await;
        within(async {
            let mut ws = connect(port).await;
            sync(&state, &mut ws, BARRIER).await;

            *state.current_html.write().await = "<p>after lag</p>".to_string();
            flood(&state, 256);

            let resent = sync(&state, &mut ws, SENTINEL).await;
            assert_eq!(resent.len(), 1, "{resent:?}");
            assert_eq!(resent[0]["html"], "<p>after lag</p>");
            assert_eq!(resent[0]["path"], "doc.md");
            assert_eq!(sync(&state, &mut ws, BARRIER).await.len(), 0);
            assert_eq!(state.connections.load(Ordering::Relaxed), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn lag_resend_carries_source_and_stats() {
        let state = doc_state();
        let port = serve(&state).await;
        within(async {
            let mut ws = connect(port).await;
            sync(&state, &mut ws, BARRIER).await;

            *state.current_html.write().await = "<p>html</p>".to_string();
            *state.current_source_html.write().await = "<pre>source</pre>".to_string();
            *state.current_file_stats.write().await = "3 lines".to_string();
            flood(&state, 256);

            let resent = sync(&state, &mut ws, SENTINEL).await;
            assert_eq!(resent.len(), 1, "{resent:?}");
            assert_eq!(resent[0]["html"], "<p>html</p>");
            assert_eq!(resent[0]["source"], "<pre>source</pre>");
            assert_eq!(resent[0]["file_stats"], "3 lines");
        })
        .await;
    }

    #[tokio::test]
    async fn ws_resyncs_on_every_lag() {
        let state = doc_state();
        let port = serve(&state).await;
        within(async {
            let mut ws = connect(port).await;
            sync(&state, &mut ws, BARRIER).await;

            for round in 0..3 {
                let html = format!("<p>round {round}</p>");
                *state.current_html.write().await = html.clone();
                flood(&state, 256);
                let resent = sync(&state, &mut ws, SENTINEL).await;
                assert_eq!(resent.len(), 1, "round {round}: {resent:?}");
                assert_eq!(resent[0]["html"], html.as_str());
            }
        })
        .await;
    }

    #[tokio::test]
    async fn lag_resend_is_local() {
        let state = doc_state();
        let port = serve(&state).await;
        within(async {
            let mut a = connect(port).await;
            let mut b = connect(port).await;
            state.hub.send(WsEvent::Scroll(BARRIER)).unwrap();
            contents_until(&mut a, BARRIER).await;
            contents_until(&mut b, BARRIER).await;

            flood(&state, 256);
            let mut spy = state.hub.subscribe();
            state.hub.send(WsEvent::Scroll(SENTINEL)).unwrap();
            assert_eq!(contents_until(&mut a, SENTINEL).await.len(), 1);
            assert_eq!(contents_until(&mut b, SENTINEL).await.len(), 1);

            while let Ok(event) = spy.try_recv() {
                assert!(
                    !matches!(event, WsEvent::Content(_)),
                    "resend must not go through the hub"
                );
            }
            assert_eq!(state.connections.load(Ordering::Relaxed), 2);
        })
        .await;
    }

    #[tokio::test]
    async fn html_cache_survives_hub_lag() {
        let state = doc_state();
        let mut spy = state.hub.subscribe();

        state
            .publish_content(ContentUpdate {
                relpath: "doc.md".to_string(),
                rendered_html: "<p>new</p>".to_string(),
                source_html: "<pre>new</pre>".to_string(),
                file_stats: "1 line".to_string(),
            })
            .await;
        assert!(matches!(spy.try_recv(), Ok(WsEvent::Content(_))));
        flood(&state, 100);

        within(async {
            while *state.current_html.read().await != "<p>new</p>" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert_eq!(*state.current_source_html.read().await, "<pre>new</pre>");
        assert_eq!(*state.current_file_stats.read().await, "1 line");
    }

    #[tokio::test]
    async fn publish_content_leaves_cache_for_other_paths() {
        let state = doc_state();
        state
            .publish_content(ContentUpdate {
                relpath: "other.md".to_string(),
                rendered_html: "<p>other</p>".to_string(),
                source_html: String::new(),
                file_stats: String::new(),
            })
            .await;
        assert_eq!(*state.current_html.read().await, "<p>hello</p>");
    }

    #[tokio::test]
    async fn scroll_post_reaches_hub() {
        let state = test_state(|_| {});
        let mut rx = state.hub.subscribe();
        let response = router(state.clone())
            .oneshot(request("POST", "/scroll/42"))
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
        assert!(matches!(rx.try_recv(), Ok(WsEvent::Scroll(42))));
    }

    #[tokio::test]
    async fn state_port_and_mode_from_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 0);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.md");
        std::fs::write(&file, "# Doc").unwrap();

        let served = AppState::new(AppInit::serve(&file, &listener, test_opts()).unwrap());
        assert_eq!(served.port, port);
        assert_eq!(served.mode, Mode::Serve);

        let piped = AppState::new(AppInit::stdin("# Doc", &listener, test_opts()).unwrap());
        assert_eq!(piped.port, port);
        assert_eq!(piped.mode, Mode::Stdin);
    }

    #[tokio::test]
    async fn health_returns_200() {
        let app = test_router();

        let response = app.oneshot(get("/health")).await.unwrap();

        assert_eq!(response.status(), 200);
    }

    fn test_router_with_base_dir(base_dir: PathBuf) -> Router {
        router(test_state(|init| init.base_dir = base_dir))
    }

    #[tokio::test]
    async fn local_file_serves_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();

        let app = test_router_with_base_dir(dir.path().canonicalize().unwrap());

        let response = app.oneshot(get("/local/photo.png")).await.unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/png");
    }

    #[tokio::test]
    async fn local_file_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_router_with_base_dir(dir.path().canonicalize().unwrap());

        let response = app
            .oneshot(get("/local/../../../etc/passwd"))
            .await
            .unwrap();

        // canonicalize fails for nonexistent traversal paths → 404
        let status = response.status().as_u16();
        assert!(
            status == 403 || status == 404,
            "path traversal should be blocked, got {status}"
        );
    }

    #[tokio::test]
    async fn local_file_returns_404_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_router_with_base_dir(dir.path().canonicalize().unwrap());

        let response = app.oneshot(get("/local/nonexistent.png")).await.unwrap();

        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn view_handler_renders_md() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        std::fs::write(dir_path.join("test.md"), "# Hello").unwrap();

        let app = test_router_with_base_dir(dir_path);

        let response = app.oneshot(get("/view/test.md")).await.unwrap();

        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Hello"), "should contain rendered content");
    }

    #[tokio::test]
    async fn view_handler_rejects_non_md() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        std::fs::write(dir_path.join("photo.png"), b"fake").unwrap();

        let app = test_router_with_base_dir(dir_path);

        let response = app.oneshot(get("/view/photo.png")).await.unwrap();

        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn view_handler_returns_404_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        let app = test_router_with_base_dir(dir_path);

        let response = app.oneshot(get("/view/nonexistent.md")).await.unwrap();

        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn index_redirects_when_initial_relpath_set() {
        let state = test_state(|init| init.initial_relpath = Some("test.md".to_string()));
        let app = router(state);

        let response = app.oneshot(get("/")).await.unwrap();

        assert_eq!(response.status(), 307);
        assert_eq!(response.headers()["location"], "/view/test.md");
    }
}
