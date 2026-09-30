use std::net::SocketAddr;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, State};
use axum::http::{StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::{Notify, RwLock, broadcast};

use crate::anchor::Stop;
use crate::doc_write::{self, Expect, Via, WriteErr, WriteTarget};
use crate::paths::{Access, FileKind, Roots};
use crate::rev::Rev;
use crate::scan::{self, NoEntry};
use crate::security::{HostGuard, Nonce, base_headers, guard, page_headers, sandbox_headers};
use crate::theme::{ResolvedTheme, ThemeRegistry, Variant};
use crate::{render, template, watcher};

/// A content update broadcast from the watcher or theme change.
#[derive(Clone, Debug)]
pub(crate) struct ContentUpdate {
    pub(crate) relpath: String,
    pub(crate) rendered_html: String,
    pub(crate) source_html: String,
    pub(crate) file_stats: String,
    /// Rev of the bytes that were rendered.
    pub(crate) rev: Rev,
    pub(crate) stops: Vec<Stop>,
}

/// One render of the entry doc, cached for new and lagging connections.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rendered {
    /// Raw rendered HTML (not JSON-wrapped).
    pub(crate) html: String,
    /// Syntax-highlighted source HTML for the code view.
    pub(crate) source_html: String,
    /// Formatted file stats string (e.g. "215 lines (154 loc) · 5.25 KB") for the raw-mode header.
    pub(crate) file_stats: String,
    /// Rev of the bytes behind `html`; `None` when it did not come from a file.
    pub(crate) rev: Option<Rev>,
    /// Focus stops of `html`.
    pub(crate) stops: Vec<Stop>,
}

/// An outbound event fanned out to every WebSocket connection.
#[derive(Clone, Debug)]
pub(crate) enum WsEvent {
    Content(ContentUpdate),
    /// Ready-to-send JSON (e.g. `theme_update` messages).
    Raw(Arc<str>),
    Scroll(u32),
    /// A doc at this lexical path was deleted or moved away.
    Gone(String),
}

/// How many connections view each doc, by lexical path.
#[derive(Debug, Default)]
pub(crate) struct Viewers {
    counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    /// Connections that have not said hello, and so still get every doc.
    everything: AtomicUsize,
}

impl Viewers {
    /// Counts one more viewer of `path` until the guard drops.
    pub(crate) fn view(self: &Arc<Self>, path: &str) -> ViewGuard {
        *self.lock().entry(path.to_string()).or_default() += 1;
        ViewGuard {
            viewers: Arc::clone(self),
            path: Some(path.to_string()),
        }
    }

    /// Counts a viewer of every doc until the guard drops.
    pub(crate) fn view_all(self: &Arc<Self>) -> ViewGuard {
        self.everything.fetch_add(1, Ordering::SeqCst);
        ViewGuard {
            viewers: Arc::clone(self),
            path: None,
        }
    }

    pub(crate) fn count(&self, path: &str) -> usize {
        self.lock().get(path).copied().unwrap_or(0)
    }

    /// Whether some connection would be sent an update to `path`.
    pub(crate) fn wants(&self, path: &str) -> bool {
        self.everything.load(Ordering::SeqCst) > 0 || self.count(path) > 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, usize>> {
        self.counts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One connection's view of a doc, or of every doc; dropping it uncounts the viewer.
#[derive(Debug)]
pub(crate) struct ViewGuard {
    viewers: Arc<Viewers>,
    path: Option<String>,
}

impl ViewGuard {
    /// Whether an update to `path` goes to this viewer.
    fn sees(&self, path: &str) -> bool {
        self.path.as_deref().is_none_or(|p| p == path)
    }
}

impl Drop for ViewGuard {
    fn drop(&mut self) {
        let Some(path) = &self.path else {
            self.viewers.everything.fetch_sub(1, Ordering::SeqCst);
            return;
        };
        let mut counts = self.viewers.lock();
        if let Some(n) = counts.get_mut(path) {
            *n -= 1;
            if *n == 0 {
                counts.remove(path);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Stdin,
    Serve,
}

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);
const HUB_CAPACITY: usize = 64;
const FAVICON: &[u8] = include_bytes!("../assets/favicon.png");
/// Inbound WebSocket cap, per message and per frame.
const WS_MAX_INBOUND: usize = 256 * 1024;

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
    /// The doc's drafter from `--drafted-by`, for a review file this server creates.
    pub drafted_by: Option<String>,
}

pub(crate) struct AppState {
    /// Bound port, from the listener rather than the requested (possibly 0) port.
    #[cfg_attr(not(test), expect(dead_code, reason = "not read by any handler yet"))]
    pub(crate) port: u16,
    pub(crate) mode: Mode,
    pub(crate) source_file: Option<PathBuf>,
    pub(crate) filename: String,
    /// Relative path of the initial file from the served root (e.g. "README.md").
    pub(crate) initial_relpath: Option<String>,
    pub(crate) custom_css: Option<String>,
    /// The entry doc's latest render, behind one lock so a resend never mixes versions.
    pub(crate) current: RwLock<Rendered>,
    pub(crate) roots: Arc<Roots>,
    pub(crate) hub: broadcast::Sender<WsEvent>,
    pub(crate) connections: AtomicUsize,
    pub(crate) viewers: Arc<Viewers>,
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
    pub(crate) rev: Option<Rev>,
    pub(crate) stops: Vec<Stop>,
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
            rev: None,
            stops: Vec::new(),
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
        let (html, index) = render::render_indexed(markdown, syntax);
        (init.html, init.stops) = (html, index.stops);
        init.source_html = render::render_source(markdown, syntax);
        init.file_stats = render::format_file_stats(markdown);
        init.rev = Some(Rev::of(markdown.as_bytes()));
        init.base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        init.filename = "stdin".to_string();
        Ok(init)
    }

    /// Serves `path`, a markdown file or a folder.
    #[cfg(test)]
    pub(crate) fn serve(
        path: &StdPath,
        listener: &TcpListener,
        opts: ServerOptions,
    ) -> anyhow::Result<Self> {
        Self::serve_target(&Target::resolve(path)?, listener, opts)
    }

    fn serve_target(
        target: &Target,
        listener: &TcpListener,
        opts: ServerOptions,
    ) -> anyhow::Result<Self> {
        let canonical_file = target.root.join(&target.entry).canonicalize()?;
        let bytes = std::fs::read(&canonical_file)?;
        let rev = Rev::of(&bytes);
        let markdown = String::from_utf8(bytes)?;

        let mut init = Self::from_opts(opts, listener, Mode::Serve)?;
        let syntax = init.registry.active().active_data().syntax.as_ref();
        let (html, index) =
            render::render_dir_indexed(&markdown, syntax, StdPath::new(&target.entry));
        (init.html, init.stops) = (html, index.stops);
        init.source_html = render::render_source(&markdown, syntax);
        init.file_stats = render::format_file_stats(&markdown);
        init.rev = Some(rev);
        init.filename = target.filename.clone();
        init.base_dir = target.root.clone();
        init.source_file = Some(canonical_file);
        init.initial_relpath = Some(target.entry.clone());
        Ok(init)
    }
}

/// What `birta PATH` serves: a folder and the entry doc's place in it.
pub struct Target {
    /// The canonical served folder.
    pub root: PathBuf,
    /// Lexical, `/`-separated path of the entry doc under `root`.
    pub entry: String,
    /// The header's file name.
    filename: String,
}

impl Target {
    /// A file is served from its canonical folder; a folder is entered at its picked doc.
    pub fn resolve(path: &StdPath) -> anyhow::Result<Self> {
        Self::resolve_with(path, scan::pick_entry)
    }

    pub(crate) fn resolve_with(
        path: &StdPath,
        pick: impl FnOnce(&StdPath) -> Result<String, NoEntry>,
    ) -> anyhow::Result<Self> {
        if path.is_dir() {
            let root = path.canonicalize()?;
            let entry = pick(&root).map_err(|none| {
                let dir = path.display();
                if none.truncated {
                    anyhow::anyhow!("no markdown files found under {dir} (search stopped early)")
                } else {
                    anyhow::anyhow!("no markdown files under {dir}")
                }
            })?;
            let filename = entry.rsplit('/').next().unwrap_or(&entry).to_string();
            return Ok(Self {
                root,
                entry,
                filename,
            });
        }
        let canonical = path.canonicalize()?;
        let root = canonical
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let entry = canonical
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());
        Ok(Self {
            root,
            entry,
            filename,
        })
    }
}

impl AppState {
    pub(crate) fn new(init: AppInit) -> Arc<Self> {
        let (hub, _) = broadcast::channel(HUB_CAPACITY);
        let roots = Roots::new(init.base_dir, dirs::home_dir());
        // The entry may sit behind a link the budgeted walk never reaches.
        if let Some(entry) = &init.initial_relpath {
            roots.register_path(StdPath::new(entry));
        }
        Arc::new(Self {
            port: init.port,
            mode: init.mode,
            roots: Arc::new(roots),
            source_file: init.source_file,
            filename: init.filename,
            initial_relpath: init.initial_relpath,
            custom_css: init.custom_css,
            current: RwLock::new(Rendered {
                html: init.html,
                source_html: init.source_html,
                file_stats: init.file_stats,
                rev: init.rev,
                stops: init.stops,
            }),
            hub,
            connections: AtomicUsize::new(0),
            viewers: Arc::default(),
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
            *self.current.write().await = Rendered {
                html: update.rendered_html.clone(),
                source_html: update.source_html.clone(),
                file_stats: update.file_stats.clone(),
                rev: Some(update.rev.clone()),
                stops: update.stops.clone(),
            };
        }
        let _ = self.hub.send(WsEvent::Content(update));
    }
}

pub async fn run(file: PathBuf, opts: ServerOptions) -> anyhow::Result<()> {
    // Before binding, so a folder without docs never opens a browser.
    let target = Target::resolve(&file)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], opts.port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.display().to_string());

    eprintln!("birta: serving {filename} at http://{actual_addr}");

    if !opts.no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("birta: could not open browser: {e}");
        }
    }

    start_target(&target, listener, opts).await
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

/// Start serving a markdown file, or a folder, on the given listener.
pub async fn start(
    file: PathBuf,
    listener: TcpListener,
    opts: ServerOptions,
) -> anyhow::Result<()> {
    start_target(&Target::resolve(&file)?, listener, opts).await
}

async fn start_target(
    target: &Target,
    listener: TcpListener,
    opts: ServerOptions,
) -> anyhow::Result<()> {
    let state = AppState::new(AppInit::serve_target(target, &listener, opts)?);

    let _watch = watcher::watch(Arc::clone(&state))?;

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
        .route("/view/{*path}", get(view_handler))
        .layer(from_fn(page_headers));
    let frags = Router::new()
        .route("/render/{*path}", get(render_handler))
        .route("/local/{*path}", get(local_file_handler))
        .route("/favicon.png", get(favicon_handler))
        .route("/favicon.ico", get(favicon_handler))
        // Doc HTML and SVG here are shown by the page, never run as documents themselves.
        .layer(from_fn(sandbox_headers));
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

/// `/view/<rel>`, percent-encoded so names with `#`, `?`, `%` or spaces stay in the path.
fn view_url(rel: &str) -> String {
    let mut url = String::from("/view/");
    for b in rel.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            url.push(char::from(b));
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

async fn index_handler(
    State(state): State<Arc<AppState>>,
    Extension(nonce): Extension<Nonce>,
) -> Response {
    if let Some(relpath) = &state.initial_relpath {
        return Redirect::temporary(&view_url(relpath)).into_response();
    }

    // stdin mode — render inline
    let registry = state.registry.read().await;
    let theme = registry.active();
    let theme_names: Vec<&str> = registry.theme_names();
    let current = state.current.read().await;
    let page = template::render_page(&template::PageOptions {
        filename: &state.filename,
        file_stats: &current.file_stats,
        content_html: &current.html,
        source_html: Some(&current.source_html),
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
        nonce: nonce.as_str(),
        extra_boot: None,
        stops: Some(&current.stops),
        rev: None,
    });
    Html(page).into_response()
}

async fn scroll_handler(Path(line): Path<u32>, State(state): State<Arc<AppState>>) -> StatusCode {
    let _ = state.hub.send(WsEvent::Scroll(line));
    StatusCode::NO_CONTENT
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Resolves and reads a markdown file for `/view` and `/render`; stdin mode serves none.
async fn read_doc(state: &AppState, rel: &str) -> Result<(PathBuf, Vec<u8>), Response> {
    if state.mode == Mode::Stdin {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    let canonical = state
        .roots
        .resolve_file(rel, FileKind::Markdown)
        .map_err(|e| e.status().into_response())?;
    match tokio::fs::read(&canonical).await {
        Ok(bytes) => Ok((canonical, bytes)),
        Err(_) => Err((StatusCode::NOT_FOUND, "could not read file").into_response()),
    }
}

/// Render and serve a markdown file from the served directory.
async fn view_handler(
    Path(path): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(nonce): Extension<Nonce>,
) -> Response {
    state.last_request.store(now_secs(), Ordering::Relaxed);

    let (canonical, bytes) = match read_doc(&state, &path).await {
        Ok(doc) => doc,
        Err(response) => return response,
    };
    let rev = Rev::of(&bytes);
    let Ok(markdown) = String::from_utf8(bytes) else {
        return (StatusCode::NOT_FOUND, "could not read file").into_response();
    };

    let syntax_theme = {
        let reg = state.registry.read().await;
        reg.active().active_data().syntax.clone()
    };

    let (html, index) =
        render::render_dir_indexed(&markdown, syntax_theme.as_ref(), StdPath::new(&path));
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
        nonce: nonce.as_str(),
        extra_boot: None,
        stops: Some(&index.stops),
        rev: Some(rev.as_str()),
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
    let markdown = match read_doc(&state, &path).await {
        Ok((_, bytes)) => match String::from_utf8(bytes) {
            Ok(markdown) => markdown,
            Err(_) => return (StatusCode::NOT_FOUND, "could not read file").into_response(),
        },
        Err(response) => return response,
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
/// Returns the reply for this connection only, if the message has one.
async fn handle_ws_message(text: &str, state: &AppState, ctx: &mut ConnCtx) -> Option<String> {
    let msg: serde_json::Value = serde_json::from_str(text).ok()?;

    match msg.get("type").and_then(|t| t.as_str()) {
        Some("hello") => return hello(state, ctx, &msg).await,
        Some("checkbox") => {
            let cid = msg.get("cid").cloned().unwrap_or_default();
            let reply = match toggle_checkbox(state, &msg).await {
                // The new rev lets the page send its next tick before the refresh arrives.
                Ok(rev) => serde_json::json!({"type": "ack", "cid": cid, "rev": rev.as_str()}),
                Err(reason) => serde_json::json!({"type": "nack", "cid": cid, "reason": reason}),
            };
            return Some(reply.to_string());
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
    None
}

/// Re-render and broadcast a theme update to all clients.
async fn broadcast_theme_update(state: &AppState) {
    let registry = state.registry.read().await;
    let theme = registry.active();
    let active = theme.active_data();

    // Re-render markdown with new syntax theme
    let rendered = if let Some(source_file) = &state.source_file {
        match std::fs::read(source_file) {
            Ok(bytes) => {
                let rev = Rev::of(&bytes);
                let markdown = String::from_utf8_lossy(&bytes);
                let (html, index) = if let Some(relpath) = &state.initial_relpath {
                    let relpath = StdPath::new(relpath);
                    render::render_dir_indexed(&markdown, active.syntax.as_ref(), relpath)
                } else {
                    render::render_indexed(&markdown, active.syntax.as_ref())
                };
                Rendered {
                    html,
                    source_html: render::render_source(&markdown, active.syntax.as_ref()),
                    file_stats: render::format_file_stats(&markdown),
                    rev: Some(rev),
                    stops: index.stops,
                }
            }
            // In a folder the entry can move away; other pages still need the new theme.
            Err(_) => state.current.read().await.clone(),
        }
    } else {
        // stdin mode — use current HTML since we can't re-render
        state.current.read().await.clone()
    };

    let mut msg = theme_update(theme, state.enable_toggle);
    msg["html"] = rendered.html.clone().into();
    msg["source"] = rendered.source_html.clone().into();
    msg["path"] = state.initial_relpath.as_deref().unwrap_or("").into();
    msg["rev"] = rendered.rev.as_ref().map(Rev::as_str).into();
    msg["stops"] = serde_json::json!(rendered.stops);

    *state.current.write().await = rendered;

    let _ = state.hub.send(WsEvent::Raw(msg.to_string().into()));
}

/// A `theme_update` carrying only the theme; the page keeps its content.
fn theme_update(theme: &ResolvedTheme, enable_toggle: bool) -> serde_json::Value {
    let (css_vars, theme_attr) = if theme.is_github() {
        (String::new(), String::new())
    } else {
        (theme.active_data().css_vars.clone(), theme.name.clone())
    };
    serde_json::json!({
        "type": "theme_update",
        "css_vars": css_vars,
        "theme_name": theme.name,
        "theme_attr": theme_attr,
        "variants": theme.variant_names(),
        "active_variant": theme.active_variant.as_str(),
        "has_toggle": theme.has_toggle() && enable_toggle,
    })
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

/// Toggles the task on `line` of `path` (the entry doc when empty) through the strict
/// write, returning the rev written. The error is the nack reason.
async fn toggle_checkbox(state: &AppState, msg: &serde_json::Value) -> Result<Rev, &'static str> {
    if state.mode == Mode::Stdin {
        return Err("read_only");
    }
    let rel = match msg.get("path").and_then(|p| p.as_str()).unwrap_or("") {
        "" => state.initial_relpath.as_deref().ok_or("read_only")?,
        rel => rel,
    };
    let doc = state
        .roots
        .resolve_doc(rel, Access::Write)
        .map_err(|_| "read_only")?;
    if !doc.writable {
        return Err("read_only");
    }
    let line = msg
        .get("line")
        .and_then(|l| l.as_u64())
        .and_then(|l| u32::try_from(l).ok())
        .ok_or("not_editable")?;
    let checked = msg
        .get("checked")
        .and_then(|c| c.as_bool())
        .ok_or("not_editable")?;
    let rev = msg
        .get("rev")
        .and_then(|r| r.as_str())
        .and_then(Rev::parse)
        .ok_or("changed_since")?;

    let target = WriteTarget {
        canonical: doc.canonical,
        rel: doc.rel,
    };
    let written = tokio::task::spawn_blocking(move || {
        doc_write::write_doc(
            &target,
            None,
            Expect::Rev(rev),
            Via::Checkbox { line },
            |bytes| {
                doc_write::toggle_task_line(bytes, line as usize, checked)
                    .ok_or(WriteErr::NotApplicable)
            },
        )
    })
    .await
    .map_err(|_| "read_only")?;
    match written {
        Ok(written) => Ok(written.rev),
        Err(WriteErr::ChangedSince) => Err("changed_since"),
        Err(WriteErr::NotApplicable) => Err("not_editable"),
        Err(WriteErr::ReadOnly) => Err("read_only"),
        Err(WriteErr::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Err("gone"),
        Err(WriteErr::Io(e)) => {
            eprintln!("birta: checkbox write failed: {e}");
            Err("read_only")
        }
    }
}

async fn local_file_handler(
    Path(path): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let file_path = match state.roots.resolve_file(&path, FileKind::Image) {
        Ok(p) => p,
        Err(e) => return e.status().into_response(),
    };

    let content = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let content_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    ([(header::CONTENT_TYPE, content_type)], content).into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.max_message_size(WS_MAX_INBOUND)
        .max_frame_size(WS_MAX_INBOUND)
        .on_upgrade(|socket| handle_ws(socket, state))
}

fn content_frame(
    path: &str,
    html: &str,
    source: &str,
    file_stats: &str,
    rev: Option<&Rev>,
    stops: &[Stop],
) -> String {
    serde_json::json!({
        "type": "content",
        "html": html,
        "source": source,
        "file_stats": file_stats,
        "path": path,
        "rev": rev.map(Rev::as_str),
        "stops": stops,
    })
    .to_string()
}

async fn cached_content_frame(state: &AppState) -> String {
    let current = state.current.read().await;
    let path = state.initial_relpath.as_deref().unwrap_or("");
    content_frame(
        path,
        &current.html,
        &current.source_html,
        &current.file_stats,
        current.rev.as_ref(),
        &current.stops,
    )
}

fn gone_frame(path: &str) -> String {
    serde_json::json!({"type": "gone", "path": path}).to_string()
}

/// What one connection views: every doc until it says hello, then the doc it named.
struct ConnCtx {
    /// `None` once a hello names a doc the page can't view.
    viewing: Option<ViewGuard>,
    tab: Option<String>,
}

/// A hello's tab id, when it fits `[A-Za-z0-9_-]{1,64}`.
fn tab_id(msg: &serde_json::Value) -> Option<String> {
    let tab = msg.get("tab")?.as_str()?;
    let fits = (1..=64).contains(&tab.len())
        && tab
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    fits.then(|| tab.to_string())
}

fn viewable(state: &AppState, rel: &str) -> bool {
    match state.mode {
        Mode::Stdin => rel.is_empty(),
        Mode::Serve => state.roots.resolve_file(rel, FileKind::Markdown).is_ok(),
    }
}

/// A fresh `content` frame for `rel`, rendered from one read of the disk.
async fn doc_frame(state: &AppState, rel: &str) -> Option<String> {
    if state.mode == Mode::Stdin {
        return if rel.is_empty() {
            Some(cached_content_frame(state).await)
        } else {
            None
        };
    }
    let syntax = {
        let reg = state.registry.read().await;
        reg.active().active_data().syntax.clone()
    };
    let canonical = state.roots.resolve_file(rel, FileKind::Markdown).ok()?;
    let bytes = tokio::fs::read(&canonical).await.ok()?;
    let rev = Rev::of(&bytes);
    let markdown = String::from_utf8(bytes).ok()?;
    let (html, index) = render::render_dir_indexed(&markdown, syntax.as_ref(), StdPath::new(rel));
    Some(content_frame(
        rel,
        &html,
        &render::render_source(&markdown, syntax.as_ref()),
        &render::format_file_stats(&markdown),
        Some(&rev),
        &index.stops,
    ))
}

/// Points the connection at the doc it names and replies with that doc, or with `gone`
/// when the page can't view it, missing or refused alike.
async fn hello(state: &AppState, ctx: &mut ConnCtx, msg: &serde_json::Value) -> Option<String> {
    let rel = msg.get("path")?.as_str()?;
    ctx.tab = tab_id(msg);
    // Counted before the read, so a write landing in between is rendered for this page.
    ctx.viewing = viewable(state, rel).then(|| state.viewers.view(rel));
    if ctx.viewing.is_some()
        && let Some(frame) = doc_frame(state, rel).await
    {
        return Some(frame);
    }
    ctx.viewing = None;
    Some(gone_frame(rel))
}

/// Frames that bring a connection that missed hub events back up to date.
async fn resync_frames(state: &AppState, ctx: &ConnCtx) -> Vec<String> {
    let theme = {
        let registry = state.registry.read().await;
        theme_update(registry.active(), state.enable_toggle).to_string()
    };
    let content = match ctx.viewing.as_ref().map(|v| v.path.as_deref()) {
        None => return vec![theme],
        Some(None) => cached_content_frame(state).await,
        Some(Some(rel)) => match doc_frame(state, rel).await {
            Some(frame) => frame,
            None => gone_frame(rel),
        },
    };
    vec![theme, content]
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    state.connections.fetch_add(1, Ordering::Relaxed);
    let mut ctx = ConnCtx {
        viewing: Some(state.viewers.view_all()),
        tab: None,
    };

    // Subscribe first: an update landing between the two is then sent after the snapshot.
    let mut rx = state.hub.subscribe();
    let init_msg = cached_content_frame(&state).await;
    if socket.send(Message::Text(init_msg.into())).await.is_err() {
        drop(ctx);
        state.connections.fetch_sub(1, Ordering::Relaxed);
        state.all_disconnected.notify_one();
        return;
    }

    'conn: loop {
        let sees = |path: &str| ctx.viewing.as_ref().is_some_and(|v| v.sees(path));
        let frames = tokio::select! {
            // Pending broadcasts go out before a slow request, such as a hello, is handled.
            biased;
            result = rx.recv() => match result {
                Ok(WsEvent::Content(update)) if sees(&update.relpath) => vec![content_frame(
                    &update.relpath,
                    &update.rendered_html,
                    &update.source_html,
                    &update.file_stats,
                    Some(&update.rev),
                    &update.stops,
                )],
                Ok(WsEvent::Gone(path)) if sees(&path) => vec![gone_frame(&path)],
                Ok(WsEvent::Content(_) | WsEvent::Gone(_)) => continue,
                Ok(WsEvent::Raw(json)) => vec![json.to_string()],
                Ok(WsEvent::Scroll(line)) => {
                    vec![serde_json::json!({"type": "scroll", "line": line}).to_string()]
                }
                // Dropped events may include content or a theme, so resync this connection.
                Err(broadcast::error::RecvError::Lagged(_)) => resync_frames(&state, &ctx).await,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    match handle_ws_message(&text, &state, &mut ctx).await {
                        Some(reply) => vec![reply],
                        None => continue,
                    }
                }
                Some(Ok(_)) => continue,
                _ => break,
            },
        };
        for frame in frames {
            if socket.send(Message::Text(frame.into())).await.is_err() {
                break 'conn;
            }
        }
    }

    // Uncounted before the connection, so a caller seeing the drop sees the view gone too.
    drop(ctx);
    state.connections.fetch_sub(1, Ordering::Relaxed);
    state.all_disconnected.notify_one();
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::scan::ScanBudget;
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
            drafted_by: None,
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
            rev: None,
            stops: Vec::new(),
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

            state.current.write().await.html = "<p>after lag</p>".to_string();
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

            *state.current.write().await = Rendered {
                html: "<p>html</p>".to_string(),
                source_html: "<pre>source</pre>".to_string(),
                file_stats: "3 lines".to_string(),
                rev: None,
                stops: Vec::new(),
            };
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
                state.current.write().await.html = html.clone();
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
                rev: Rev::of(b"new"),
                stops: Vec::new(),
            })
            .await;
        assert!(matches!(spy.try_recv(), Ok(WsEvent::Content(_))));
        flood(&state, 100);

        within(async {
            while state.current.read().await.html != "<p>new</p>" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let current = state.current.read().await;
        assert_eq!(current.source_html, "<pre>new</pre>");
        assert_eq!(current.file_stats, "1 line");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cached_frame_is_never_torn() {
        let state = doc_state();
        let writer = {
            let state = state.clone();
            tokio::spawn(async move {
                for i in 0..20_000u32 {
                    let n = i.to_string();
                    state
                        .publish_content(ContentUpdate {
                            relpath: "doc.md".to_string(),
                            rendered_html: n.clone(),
                            source_html: n.clone(),
                            file_stats: n.clone(),
                            rev: Rev::of(n.as_bytes()),
                            stops: Vec::new(),
                        })
                        .await;
                }
            })
        };

        let mut torn = Vec::new();
        while !writer.is_finished() {
            let frame: serde_json::Value =
                serde_json::from_str(&cached_content_frame(&state).await).unwrap();
            let n = frame["html"].as_str().unwrap().to_string();
            if n == "<p>hello</p>" {
                continue;
            }
            let rev = Rev::of(n.as_bytes());
            if frame["source"] != n.as_str()
                || frame["file_stats"] != n.as_str()
                || frame["rev"] != rev.as_str()
            {
                torn.push(frame);
            }
        }
        writer.await.unwrap();
        assert!(
            torn.is_empty(),
            "{} torn frames, first {}",
            torn.len(),
            torn[0]
        );
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
                rev: Rev::of(b"other"),
                stops: Vec::new(),
            })
            .await;
        assert_eq!(state.current.read().await.html, "<p>hello</p>");
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
        router(test_state(|init| {
            init.mode = Mode::Serve;
            init.base_dir = base_dir;
        }))
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

    #[test]
    fn view_url_percent_encodes_each_name() {
        for (rel, want) in [
            ("notes/a.md", "/view/notes/a.md"),
            ("a #1.md", "/view/a%20%231.md"),
            ("q?/100%.md", "/view/q%3F/100%25.md"),
            ("ü-_.~.md", "/view/%C3%BC-_.~.md"),
        ] {
            assert_eq!(view_url(rel), want, "{rel}");
        }
    }

    #[tokio::test]
    async fn a_linked_entry_is_served_before_any_walk() {
        let site = Site::new(&[("root/b.md", b"# B\n"), ("outside/doc.md", b"# Doc\n")]);
        site.link("root/index.md", "../outside/doc.md");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = AppState::new(AppInit::serve(&site.root, &listener, test_opts()).unwrap());

        assert_eq!(state.initial_relpath.as_deref(), Some("index.md"));
        assert_eq!(state.roots.root(), site.root);
        let app = router(state);
        let response = app.clone().oneshot(get("/view/index.md")).await.unwrap();
        assert_eq!(response.status(), 200);
        let response = app.oneshot(get("/view/b.md")).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn a_folder_without_docs_says_whether_the_search_stopped_early() {
        let site = Site::new(&[("root/a/b/c/x.md", b"# X\n")]);
        let small = ScanBudget {
            entries: 2,
            ..ScanBudget::default()
        };

        let stopped = Target::resolve_with(&site.root, |root| scan::pick_entry_with(root, small));
        assert_eq!(
            stopped.err().map(|e| e.to_string()),
            Some(format!(
                "no markdown files found under {} (search stopped early)",
                site.root.display()
            ))
        );

        let empty = site.root.join("a/b/c");
        std::fs::remove_file(empty.join("x.md")).unwrap();
        let none = Target::resolve(&empty);
        assert_eq!(
            none.err().map(|e| e.to_string()),
            Some(format!("no markdown files under {}", empty.display()))
        );
    }

    /// `<tmp>/root` is served; `<tmp>/outside` and `<tmp>/secret.*` are not.
    struct Site {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
    }

    impl Site {
        fn new(files: &[(&str, &[u8])]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let root = base.join("root");
            std::fs::create_dir(&root).unwrap();
            for (rel, bytes) in files {
                let path = base.join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            Self {
                _tmp: tmp,
                base,
                root,
            }
        }

        fn link(&self, link: &str, target: impl AsRef<StdPath>) {
            std::os::unix::fs::symlink(target, self.base.join(link)).unwrap();
        }

        fn read(&self, rel: &str) -> Vec<u8> {
            std::fs::read(self.root.join(rel)).unwrap()
        }

        /// Names (relative to `<tmp>`) of `files` no longer equal to `original`, each restored
        /// so the next row starts clean.
        fn rewritten(&self, files: &[&str], original: &[u8]) -> Vec<String> {
            let mut changed = Vec::new();
            for rel in files {
                let path = self.base.join(rel);
                let now = std::fs::read(&path).unwrap();
                if now != original {
                    changed.push(format!("{rel} -> {:?}", String::from_utf8_lossy(&now)));
                    std::fs::write(&path, original).unwrap();
                }
            }
            changed
        }

        /// Serve mode with `root/doc.md` as the entry doc.
        fn state(&self) -> Arc<AppState> {
            let root = self.root.clone();
            test_state(|init| {
                init.mode = Mode::Serve;
                init.source_file = Some(root.join("doc.md"));
                init.initial_relpath = Some("doc.md".to_string());
                init.base_dir = root;
            })
        }

        fn stdin_state(&self) -> Arc<AppState> {
            let root = self.root.clone();
            test_state(|init| init.base_dir = root)
        }
    }

    /// GETs each uri; returns every status mismatch.
    async fn status_mismatches(app: &Router, rows: &[(&str, u16)]) -> Vec<String> {
        let mut bad = Vec::new();
        for (uri, want) in rows {
            let got = app.clone().oneshot(get(uri)).await.unwrap().status();
            if got != *want {
                bad.push(format!("{uri}: got {got}, want {want}"));
            }
        }
        bad
    }

    const TASK: &[u8] = b"- [ ] a\n";

    fn local_site() -> Site {
        Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("root/upper.MD", b"# Upper\n"),
            ("root/a.png", b"png"),
            ("root/b.PNG", b"png"),
            ("root/pic.svg", b"<svg/>"),
            ("root/c.avif", b"avif"),
            ("root/d.bmp", b"bmp"),
            ("root/notes.txt", b"notes"),
            ("root/.env", b"SECRET=1"),
            ("root/x.birta.jsonl", b"{}"),
            ("root/y.BIRTA.JSONL", b"{}"),
            ("root/.birta.jsonl", b"{}"),
            ("root/dir/.git/config", b"[core]"),
            ("root/dir/.git/a.png", b"png"),
            ("root/dir2/.GIT/a.png", b"png"),
            ("root/.git/x.md", b"# Git\n"),
            ("root/.context/x.md", b"# Context\n"),
            ("root/.context/img/a.png", b"png"),
            ("root/.context/.env", b"SECRET=1"),
            ("root/x/.secret.png", b"png"),
            ("outside/a.png", b"png"),
            ("outside/doc.md", b"# Outside\n"),
            ("secret.png", b"png"),
        ])
    }

    #[tokio::test]
    async fn local_refuses_traversal_dotfiles_and_review_files() {
        let site = local_site();
        let app = router(site.state());
        let rows = [
            ("/local/%2e%2e/etc/passwd", 403),
            ("/local/%2e%2e/secret.png", 403),
            ("/local//etc/passwd", 403),
            ("/local/x/%2e%2e/a.png", 403),
            ("/local/x%2F%2E%2E%2Fsecret.png", 403),
            ("/local/a.png%00.txt", 403),
            // Decodes once, to a literal `%2e%2e` folder that doesn't exist.
            ("/local/%252e%252e/secret.png", 404),
            ("/local/.env", 403),
            ("/local/dir/.git/config", 403),
            ("/local/dir/.git/a.png", 403),
            ("/local/dir2/.GIT/a.png", 403),
            ("/local/x.birta.jsonl", 403),
            ("/local/y.BIRTA.JSONL", 403),
            ("/local/.birta.jsonl", 403),
            ("/local/.context/.env", 403),
            ("/local/x/.secret.png", 403),
            ("/local/notes.txt", 404),
            ("/local/doc.md", 404),
            ("/local/missing.png", 404),
            ("/local/a.png", 200),
            ("/local/b.PNG", 200),
            ("/local/.context/img/a.png", 200),
        ];
        let bad = status_mismatches(&app, &rows).await;
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn local_sends_image_types_only() {
        let site = local_site();
        let app = router(site.state());
        let rows = [
            ("/local/a.png", "image/png"),
            ("/local/b.PNG", "image/png"),
            ("/local/pic.svg", "image/svg+xml"),
            ("/local/c.avif", "image/avif"),
            ("/local/d.bmp", "image/bmp"),
        ];
        let mut bad = Vec::new();
        for (uri, want) in rows {
            let response = app.clone().oneshot(get(uri)).await.unwrap();
            let got = response.headers().get("content-type").cloned();
            if response.status() != 200 || got.as_ref().is_none_or(|v| v != want) {
                bad.push(format!("{uri}: {} {got:?}, want {want}", response.status()));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn fragment_routes_carry_sandbox_headers() {
        let site = local_site();
        let app = router(site.state());
        let sandbox = "sandbox; default-src 'none'; style-src 'unsafe-inline'";
        let mut bad = Vec::new();
        for uri in [
            "/local/pic.svg",
            "/render/doc.md",
            "/favicon.png",
            "/favicon.ico",
        ] {
            let response = app.clone().oneshot(get(uri)).await.unwrap();
            let headers = response.headers();
            if response.status() != 200
                || headers
                    .get("content-security-policy")
                    .is_none_or(|v| v != sandbox)
                || headers
                    .get("x-content-type-options")
                    .is_none_or(|v| v != "nosniff")
            {
                bad.push(format!("{uri}: {} {headers:?}", response.status()));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");

        let page = app.clone().oneshot(get("/view/doc.md")).await.unwrap();
        let csp = page.headers().get("content-security-policy");
        assert!(
            csp.is_none_or(|v| !v.to_str().unwrap().contains("sandbox")),
            "pages must not be sandboxed: {csp:?}"
        );
    }

    #[tokio::test]
    async fn view_and_render_path_rules() {
        let site = local_site();
        let app = router(site.state());
        let rows = [
            ("/view/.git/x.md", 403),
            ("/render/.git/x.md", 403),
            ("/view/x.birta.jsonl", 403),
            ("/view/%2e%2e/outside/doc.md", 403),
            ("/view/.context/x.md", 200),
            ("/render/.context/x.md", 200),
            ("/view/upper.MD", 200),
            ("/view/missing.txt", 400),
            ("/view/a.png", 400),
            ("/view/missing.md", 404),
        ];
        let bad = status_mismatches(&app, &rows).await;
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn stdin_mode_serves_images_but_no_docs() {
        let site = local_site();
        let app = router(site.stdin_state());
        let rows = [
            ("/view/doc.md", 404),
            ("/render/doc.md", 404),
            ("/local/a.png", 200),
        ];
        let bad = status_mismatches(&app, &rows).await;
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn symlinked_dir_created_after_start_is_served() {
        let site = local_site();
        let app = router(site.state());
        site.link("root/linked", site.base.join("outside"));

        let rows = [
            ("/view/linked/doc.md", 200),
            ("/render/linked/doc.md", 200),
            ("/local/linked/a.png", 200),
        ];
        let bad = status_mismatches(&app, &rows).await;
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn link_targets_obey_dot_and_deny_rules() {
        let site = local_site();
        let app = router(site.state());
        site.link("root/pic.png", ".env");
        site.link("root/git.png", "dir/.git/a.png");
        site.link("root/jsonl.png", "x.birta.jsonl");
        site.link("root/ok.png", ".context/img/a.png");
        site.link("root/g.md", ".git/x.md");

        let rows = [
            ("/local/pic.png", 403),
            ("/local/git.png", 403),
            ("/local/jsonl.png", 403),
            ("/view/g.md", 403),
            ("/render/g.md", 403),
            ("/local/ok.png", 200),
        ];
        let bad = status_mismatches(&app, &rows).await;
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn symlinks_to_root_or_loops_are_refused() {
        let site = local_site();
        let app = router(site.state());
        site.link("root/slash", "/");
        site.link("root/self", "self");
        site.link("root/pic.png", site.base.join("secret.png"));
        // Through `/` and back down to an image outside the root.
        let via_slash = format!("/local/slash{}", site.base.join("secret.png").display());

        let rows = [
            (via_slash.as_str(), 403),
            ("/local/pic.png", 403),
            ("/local/self/a.png", 404),
        ];
        let bad = tokio::time::timeout(Duration::from_secs(5), status_mismatches(&app, &rows))
            .await
            .expect("symlink resolution did not terminate");
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// `git hash-object --no-filters`, as an oracle independent of the server's hashing.
    fn git_rev(bytes: &[u8]) -> String {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("git")
            .args(["hash-object", "--stdin", "--no-filters"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn checkbox_frame(
        cid: &str,
        path: &str,
        line: u64,
        checked: bool,
        rev: Option<&str>,
    ) -> String {
        let mut frame = serde_json::json!({
            "type": "checkbox", "cid": cid, "path": path, "line": line, "checked": checked,
        });
        if let Some(rev) = rev {
            frame["rev"] = rev.into();
        }
        frame.to_string()
    }

    /// Sends `frame` and waits briefly for the ack or nack naming `cid`.
    async fn send_checkbox(ws: &mut Client, cid: &str, frame: String) -> Option<serde_json::Value> {
        use futures_util::SinkExt;
        ws.send(tokio_tungstenite::tungstenite::Message::Text(frame.into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let msg = next_json(ws).await;
                if matches!(msg["type"].as_str(), Some("ack" | "nack")) && msg["cid"] == cid {
                    return msg;
                }
            }
        })
        .await
        .ok()
    }

    fn is_nack(reply: &Option<serde_json::Value>, reason: &str) -> bool {
        reply
            .as_ref()
            .is_some_and(|r| r["type"] == "nack" && r["reason"] == reason)
    }

    fn is_ack(reply: &Option<serde_json::Value>) -> bool {
        reply.as_ref().is_some_and(|r| r["type"] == "ack")
    }

    #[tokio::test]
    async fn checkbox_refuses_non_markdown_denied_and_links_out_of_root() {
        let site = Site::new(&[
            ("root/doc.md", TASK),
            ("root/a.txt", TASK),
            ("root/b.birta.jsonl", TASK),
            ("root/.git/x.md", TASK),
            ("secret.md", TASK),
        ]);
        site.link("root/evil.md", "../secret.md");
        site.link("root/fake.md", "a.txt");
        site.link("root/gitlink.md", ".git/x.md");
        let port = serve(&site.state()).await;
        let rev = git_rev(TASK);
        let targets = [
            "root/doc.md",
            "root/a.txt",
            "root/b.birta.jsonl",
            "root/.git/x.md",
            "secret.md",
        ];

        let mut ws = connect(port).await;
        let mut bad = Vec::new();
        for (i, path) in [
            "a.txt",
            "b.birta.jsonl",
            "evil.md",
            "fake.md",
            "gitlink.md",
            ".git/x.md",
            "../secret.md",
        ]
        .into_iter()
        .enumerate()
        {
            let cid = format!("c{i}");
            let reply = send_checkbox(
                &mut ws,
                &cid,
                checkbox_frame(&cid, path, 1, true, Some(&rev)),
            )
            .await;
            let changed = site.rewritten(&targets, TASK);
            if !changed.is_empty() {
                bad.push(format!("{path}: rewrote {changed:?}"));
            }
            if !is_nack(&reply, "read_only") {
                bad.push(format!("{path}: reply {reply:?}, want nack read_only"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn checkbox_refused_in_stdin_mode() {
        let site = Site::new(&[("root/doc.md", TASK)]);
        let port = serve(&site.stdin_state()).await;
        let rev = git_rev(TASK);

        let mut ws = connect(port).await;
        let mut bad = Vec::new();
        for (cid, path) in [("c1", "doc.md"), ("c2", "")] {
            let reply =
                send_checkbox(&mut ws, cid, checkbox_frame(cid, path, 1, true, Some(&rev))).await;
            let changed = site.rewritten(&["root/doc.md"], TASK);
            if !changed.is_empty() {
                bad.push(format!("{path:?}: rewrote {changed:?}"));
            }
            if !is_nack(&reply, "read_only") {
                bad.push(format!("{path:?}: reply {reply:?}, want nack read_only"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn checkbox_through_link_writes_target_and_keeps_link() {
        let site = Site::new(&[("root/doc.md", TASK), ("root/inside.md", TASK)]);
        site.link("root/alias.md", "inside.md");
        let port = serve(&site.state()).await;

        let mut ws = connect(port).await;
        let frame = checkbox_frame("c1", "alias.md", 1, true, Some(&git_rev(TASK)));
        let reply = send_checkbox(&mut ws, "c1", frame).await;

        assert_eq!(site.read("inside.md"), b"- [x] a\n");
        let link = std::fs::symlink_metadata(site.root.join("alias.md")).unwrap();
        assert!(link.file_type().is_symlink(), "the link was replaced");
        assert!(is_ack(&reply), "{reply:?}");
    }

    #[tokio::test]
    async fn back_to_back_ticks_both_land() {
        let before: &[u8] = b"- [ ] a\n- [ ] b\n";
        let middle: &[u8] = b"- [x] a\n- [ ] b\n";
        let site = Site::new(&[("root/doc.md", before)]);
        // No watcher here, so no `content` frame ever brings a newer rev.
        let port = serve(&site.state()).await;

        let mut ws = connect(port).await;
        let frame = checkbox_frame("c1", "doc.md", 1, true, Some(&git_rev(before)));
        let first = send_checkbox(&mut ws, "c1", frame).await;
        assert!(is_ack(&first), "{first:?}");
        let rev = first.as_ref().unwrap()["rev"].clone();
        assert_eq!(rev, git_rev(middle), "the ack names the rev it wrote");

        let frame = checkbox_frame("c2", "doc.md", 2, true, rev.as_str());
        let second = send_checkbox(&mut ws, "c2", frame).await;
        assert!(is_ack(&second), "{second:?}");
        assert_eq!(site.read("doc.md"), b"- [x] a\n- [x] b\n");
    }

    #[tokio::test]
    async fn checkbox_empty_path_is_the_entry_doc() {
        let site = Site::new(&[("root/doc.md", TASK)]);
        let port = serve(&site.state()).await;

        let mut ws = connect(port).await;
        let reply = send_checkbox(
            &mut ws,
            "c1",
            checkbox_frame("c1", "", 1, true, Some(&git_rev(TASK))),
        )
        .await;

        assert_eq!(site.read("doc.md"), b"- [x] a\n");
        assert!(is_ack(&reply), "{reply:?}");
    }

    #[tokio::test]
    async fn checkbox_needs_the_current_rev() {
        let site = Site::new(&[("root/doc.md", TASK)]);
        let port = serve(&site.state()).await;
        let stale = git_rev(b"- [ ] older\n");

        let mut ws = connect(port).await;
        let mut bad = Vec::new();
        for (cid, rev) in [
            ("c1", None),
            ("c2", Some(stale.as_str())),
            ("c3", Some("zzz")),
        ] {
            let reply =
                send_checkbox(&mut ws, cid, checkbox_frame(cid, "doc.md", 1, true, rev)).await;
            let changed = site.rewritten(&["root/doc.md"], TASK);
            if !changed.is_empty() {
                bad.push(format!("rev {rev:?}: rewrote {changed:?}"));
            }
            if !is_nack(&reply, "changed_since") {
                bad.push(format!(
                    "rev {rev:?}: reply {reply:?}, want nack changed_since"
                ));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");

        let rev0 = git_rev(TASK);
        let reply = send_checkbox(
            &mut ws,
            "c4",
            checkbox_frame("c4", "doc.md", 1, true, Some(&rev0)),
        )
        .await;
        assert_eq!(site.read("doc.md"), b"- [x] a\n");
        assert!(is_ack(&reply), "{reply:?}");

        // The page still holds rev0 until new content arrives.
        let reply = send_checkbox(
            &mut ws,
            "c5",
            checkbox_frame("c5", "doc.md", 1, false, Some(&rev0)),
        )
        .await;
        assert_eq!(site.read("doc.md"), b"- [x] a\n");
        assert!(is_nack(&reply, "changed_since"), "{reply:?}");
    }

    #[tokio::test]
    async fn checkbox_on_a_line_that_is_not_a_task_is_refused() {
        let text: &[u8] = b"# T [ ]\n- [ ] a\n- [x] b\n";
        let site = Site::new(&[("root/doc.md", text)]);
        let port = serve(&site.state()).await;
        let rev = git_rev(text);

        let mut ws = connect(port).await;
        let mut bad = Vec::new();
        let rows = [
            (1, true),
            (0, true),
            (4, true),
            (99, true),
            (u64::from(u32::MAX) + 2, true),
            // Tasks, but already in the requested state.
            (2, false),
            (3, true),
        ];
        for (i, (line, checked)) in rows.into_iter().enumerate() {
            let cid = format!("c{i}");
            let reply = send_checkbox(
                &mut ws,
                &cid,
                checkbox_frame(&cid, "doc.md", line, checked, Some(&rev)),
            )
            .await;
            let changed = site.rewritten(&["root/doc.md"], text);
            if !changed.is_empty() {
                bad.push(format!("line {line} -> {checked}: rewrote {changed:?}"));
            }
            if !is_nack(&reply, "not_editable") {
                bad.push(format!(
                    "line {line} -> {checked}: reply {reply:?}, want nack not_editable"
                ));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn checkbox_keeps_crlf_bom_and_missing_final_newline() {
        let cases: [(&str, &[u8], u64, &[u8]); 3] = [
            (
                "crlf.md",
                b"- [ ] a\r\n- [ ] b\r\n",
                2,
                b"- [ ] a\r\n- [x] b\r\n",
            ),
            (
                "bom.md",
                b"\xef\xbb\xbf- [ ] a\r\n",
                1,
                b"\xef\xbb\xbf- [x] a\r\n",
            ),
            ("noeol.md", b"- [ ] a\n- [ ] b", 2, b"- [ ] a\n- [x] b"),
        ];
        let mut files: Vec<(&str, &[u8])> = vec![("root/doc.md", TASK)];
        let names: Vec<String> = cases
            .iter()
            .map(|(name, ..)| format!("root/{name}"))
            .collect();
        for ((_, before, ..), name) in cases.iter().zip(&names) {
            files.push((name, before));
        }
        let site = Site::new(&files);
        let port = serve(&site.state()).await;

        let mut ws = connect(port).await;
        let mut bad = Vec::new();
        for (name, before, line, want) in cases {
            let frame = checkbox_frame(name, name, line, true, Some(&git_rev(before)));
            let reply = send_checkbox(&mut ws, name, frame).await;
            let got = site.read(name);
            if got != want {
                bad.push(format!("{name}: got {:?}", String::from_utf8_lossy(&got)));
            }
            if !is_ack(&reply) {
                bad.push(format!("{name}: reply {reply:?}, want ack"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn checkbox_on_read_only_doc_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let site = Site::new(&[("root/doc.md", TASK)]);
        let doc = site.root.join("doc.md");
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o444)).unwrap();
        let port = serve(&site.state()).await;

        let mut ws = connect(port).await;
        let reply = send_checkbox(
            &mut ws,
            "c1",
            checkbox_frame("c1", "doc.md", 1, true, Some(&git_rev(TASK))),
        )
        .await;

        assert_eq!(site.read("doc.md"), TASK);
        let mode = std::fs::metadata(&doc).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444);
        assert!(is_nack(&reply, "read_only"), "{reply:?}");
    }

    #[tokio::test]
    async fn walked_file_link_out_of_root_is_served_but_read_only() {
        let site = Site::new(&[("root/doc.md", TASK), ("outside/doc.md", TASK)]);
        site.link("root/ext.md", site.base.join("outside/doc.md"));
        let state = site.state();
        let app = router(state.clone());
        crate::scan::walk(&site.root, &state.roots, crate::scan::ScanBudget::default());
        let bad = status_mismatches(&app, &[("/view/ext.md", 200), ("/render/ext.md", 200)]).await;
        assert!(bad.is_empty(), "{bad:#?}");
        let port = serve(&state).await;

        let mut ws = connect(port).await;
        let reply = send_checkbox(
            &mut ws,
            "c1",
            checkbox_frame("c1", "ext.md", 1, true, Some(&git_rev(TASK))),
        )
        .await;

        assert_eq!(
            std::fs::read(site.base.join("outside/doc.md")).unwrap(),
            TASK
        );
        assert!(is_nack(&reply, "read_only"), "{reply:?}");
    }

    /// The JSON in the page's `birta-boot` block.
    fn boot_of(page: &str) -> serde_json::Value {
        let start = page.find("id=\"birta-boot\"").expect("no birta-boot block");
        let body = &page[start..];
        let open = body.find('>').unwrap() + 1;
        let close = body.find("</script>").unwrap();
        serde_json::from_str(&body[open..close]).unwrap()
    }

    #[tokio::test]
    async fn view_boot_carries_rev_of_rendered_bytes() {
        let text: &[u8] = b"# Rev\n\n- [ ] a\n";
        let site = Site::new(&[("root/doc.md", text)]);
        let app = router(site.state());

        let response = app.oneshot(get("/view/doc.md")).await.unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let boot = boot_of(std::str::from_utf8(&body).unwrap());

        assert_eq!(boot["rev"], git_rev(text), "{boot}");
    }

    #[tokio::test]
    async fn content_frames_carry_rev_including_lag_resend() {
        let text: &[u8] = b"# Rev\n\n- [ ] a\n";
        let site = Site::new(&[("root/doc.md", text)]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let init = AppInit::serve(&site.root.join("doc.md"), &listener, test_opts()).unwrap();
        let state = AppState::new(init);
        let port = serve(&state).await;
        let rev = git_rev(text);

        within(async {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                .await
                .unwrap();
            let initial = next_json(&mut ws).await;
            assert_eq!(initial["rev"], rev, "initial content frame");

            sync(&state, &mut ws, BARRIER).await;
            flood(&state, 256);
            let resent = sync(&state, &mut ws, SENTINEL).await;
            assert_eq!(resent.len(), 1, "{resent:?}");
            assert_eq!(resent[0]["rev"], rev, "lag resend");
        })
        .await;
    }

    /// True when the server closes the socket within a second.
    async fn closes_soon(ws: &mut Client) -> bool {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) | Some(Ok(TMessage::Close(_))) => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn ws_closes_on_oversized_message() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        let state = doc_state();
        let port = serve(&state).await;

        let mut ws = connect(port).await;
        let _ = ws.send(TMessage::Text("x".repeat(300 * 1024).into())).await;

        assert!(
            closes_soon(&mut ws).await,
            "a 300 KiB message must close the socket"
        );
    }

    #[tokio::test]
    async fn ws_closes_on_oversized_fragmented_message() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        use tokio_tungstenite::tungstenite::protocol::frame::Frame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
        let state = doc_state();
        let port = serve(&state).await;

        let mut ws = connect(port).await;
        let half = "x".repeat(150 * 1024);
        let first = Frame::message(half.clone(), OpCode::Data(Data::Text), false);
        let last = Frame::message(half, OpCode::Data(Data::Continue), true);
        let _ = ws.send(TMessage::Frame(first)).await;
        let _ = ws.send(TMessage::Frame(last)).await;

        assert!(
            closes_soon(&mut ws).await,
            "2 x 150 KiB fragments must close the socket"
        );
    }

    #[tokio::test]
    async fn checkbox_replies_go_only_to_the_sender() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        let site = Site::new(&[("root/doc.md", TASK)]);
        let state = site.state();
        let port = serve(&state).await;
        let mut a = connect(port).await;
        let mut b = connect(port).await;

        let nack = send_checkbox(&mut a, "c1", checkbox_frame("c1", "doc.md", 1, true, None)).await;
        let frame = checkbox_frame("c2", "doc.md", 1, true, Some(&git_rev(TASK)));
        let ack = send_checkbox(&mut a, "c2", frame).await;
        assert!(is_nack(&nack, "changed_since"), "{nack:?}");
        assert!(is_ack(&ack), "{ack:?}");

        state.hub.send(WsEvent::Scroll(SENTINEL)).unwrap();
        within(async {
            loop {
                let TMessage::Text(text) = b.next().await.unwrap().unwrap() else {
                    continue;
                };
                let msg: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert!(
                    !matches!(msg["type"].as_str(), Some("ack" | "nack")),
                    "second client got {msg}"
                );
                if msg["type"] == "scroll" && msg["line"] == SENTINEL {
                    break;
                }
            }
        })
        .await;
    }

    #[tokio::test]
    async fn ws_accepts_message_under_cap() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        let state = doc_state();
        let port = serve(&state).await;

        let mut ws = connect(port).await;
        ws.send(TMessage::Text("x".repeat(200 * 1024).into()))
            .await
            .unwrap();

        within(async {
            assert_eq!(sync(&state, &mut ws, SENTINEL).await.len(), 0);
        })
        .await;
    }

    const DOC_SCRIPT: &str = "<script>window.pwned=1</script>";
    const DOC_MD: &[u8] = b"# Doc\n\n<script>window.pwned=1</script>\n";

    /// Serve mode with `/view/doc.md`, and stdin mode with `/`; both render `DOC_SCRIPT`.
    fn page_apps() -> (Site, [(Router, &'static str); 2]) {
        let site = Site::new(&[("root/doc.md", DOC_MD)]);
        let serve = router(site.state());
        let stdin = router(test_state(|init| {
            init.html = format!("<p>hello</p>\n{DOC_SCRIPT}");
        }));
        (site, [(serve, "/view/doc.md"), (stdin, "/")])
    }

    /// The page's single CSP header and its body.
    async fn page_with_csp(app: &Router, req: axum::http::Request<Body>) -> (String, String) {
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
        let csp: Vec<String> = response
            .headers()
            .get_all(header::CONTENT_SECURITY_POLICY)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        let [csp] = <[String; 1]>::try_from(csp)
            .unwrap_or_else(|all| panic!("want exactly one CSP header, got {all:?}"));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (csp, String::from_utf8(body.to_vec()).unwrap())
    }

    fn directive<'a>(csp: &'a str, name: &str) -> Option<&'a str> {
        csp.split(';')
            .map(str::trim)
            .find(|d| d.split_whitespace().next() == Some(name))
    }

    /// The value of the single `'nonce-…'` source in `script-src`.
    fn nonce_of(csp: &str) -> &str {
        let script = directive(csp, "script-src").unwrap_or_else(|| panic!("no script-src: {csp}"));
        let nonces: Vec<&str> = script
            .split_whitespace()
            .filter_map(|s| s.strip_prefix("'nonce-")?.strip_suffix('\''))
            .collect();
        assert_eq!(nonces.len(), 1, "want one nonce source: {script}");
        nonces[0]
    }

    /// Open tags of every `<script` in the page.
    fn script_tags(page: &str) -> Vec<&str> {
        page.match_indices("<script")
            .map(|(i, _)| &page[i..i + page[i..].find('>').unwrap() + 1])
            .collect()
    }

    #[tokio::test]
    async fn pages_carry_a_nonce_csp_matching_their_scripts() {
        let (_site, apps) = page_apps();
        for (app, uri) in &apps {
            let (csp, page) = page_with_csp(app, get(uri)).await;
            assert!(!csp.contains("sandbox"), "{uri}: {csp}");
            let script_src: Vec<&str> = directive(&csp, "script-src")
                .unwrap_or_else(|| panic!("{uri}: no script-src: {csp}"))
                .split_whitespace()
                .collect();
            assert!(script_src.contains(&"'strict-dynamic'"), "{uri}: {csp}");
            for banned in [
                "'unsafe-inline'",
                "'unsafe-eval'",
                "'self'",
                "*",
                "http:",
                "https:",
                "data:",
            ] {
                assert!(!script_src.contains(&banned), "{uri}: {banned} in {csp}");
            }

            // CSP base64-value grammar, carrying at least 128 bits.
            let nonce = nonce_of(&csp);
            let digits = nonce.trim_end_matches('=');
            assert!(nonce.len() - digits.len() <= 2, "{uri}: {nonce}");
            assert!(
                digits
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/-_".contains(&b)),
                "{uri}: not base64: {nonce}"
            );
            assert!(digits.len() >= 22, "{uri}: under 128 bits: {nonce}");

            let attr = format!("nonce=\"{nonce}\"");
            let tags = script_tags(&page);
            let (doc, template): (Vec<&str>, Vec<&str>) =
                tags.iter().partition(|t| **t == "<script>");
            assert_eq!(
                doc.len(),
                1,
                "{uri}: doc script not rendered once: {tags:?}"
            );
            let missing: Vec<&&str> = template.iter().filter(|t| !t.contains(&attr)).collect();
            assert!(
                missing.is_empty(),
                "{uri}: scripts without {attr}: {missing:?}"
            );
            assert!(template.len() >= 4, "{uri}: {template:?}");
            assert_eq!(
                page.matches(nonce).count(),
                template.len(),
                "{uri}: the nonce appears outside the template's script tags"
            );
            assert!(
                !page
                    .to_ascii_lowercase()
                    .contains("http-equiv=\"content-security-policy\""),
                "{uri}: served pages get the header, not the static meta"
            );
        }
    }

    #[tokio::test]
    async fn page_nonce_is_fresh_per_request() {
        let (_site, apps) = page_apps();
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let (app, uri) = &apps[i % 2];
            let (csp, page) = page_with_csp(app, get(uri)).await;
            let nonce = nonce_of(&csp).to_string();
            assert!(
                page.contains(&format!("nonce=\"{nonce}\"")),
                "{uri}: body does not carry the header nonce {nonce}"
            );
            assert!(seen.insert(nonce.clone()), "{uri}: nonce repeated: {nonce}");
        }
    }

    #[tokio::test]
    async fn page_csp_connect_src_is_the_request_host() {
        let (_site, apps) = page_apps();
        for host in ["localhost:9999", "127.0.0.1:3456", "localhost"] {
            for (app, uri) in &apps {
                let req = raw_request("GET", uri, &[("host", host)]);
                let (csp, _) = page_with_csp(app, req).await;
                assert_eq!(
                    directive(&csp, "connect-src"),
                    Some(format!("connect-src 'self' ws://{host}").as_str()),
                    "{uri} via {host}: {csp}"
                );
            }
        }

        // The Host reaches the header only after the guard has validated it.
        for (app, uri) in &apps {
            let req = raw_request("GET", uri, &[("host", "localhost:1; script-src *")]);
            let response = app.clone().oneshot(req).await.unwrap();
            assert_ne!(response.status(), 200, "{uri}");
            assert!(
                response
                    .headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .is_none_or(|v| !v.to_str().unwrap().contains("script-src *")),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn doc_cannot_borrow_page_nonce() {
        let borrow = "<script nonce=\"{{NONCE}}\">window.pwned=1</script>";
        let md = format!("# Doc\n\n{borrow}\n");
        let site = Site::new(&[("root/doc.md", md.as_bytes())]);
        let serve = router(site.state());
        let stdin = router(test_state(|init| {
            init.html = format!("<p>hello</p>\n{borrow}")
        }));
        for (app, uri) in [(serve, "/view/doc.md"), (stdin, "/")] {
            let (csp, page) = page_with_csp(&app, get(uri)).await;
            let nonce = nonce_of(&csp);
            assert!(page.contains(borrow), "{uri}: doc tag was rewritten");
            let template = script_tags(&page)
                .into_iter()
                .filter(|t| !t.contains("{{NONCE}}"))
                .count();
            assert_eq!(
                page.matches(nonce).count(),
                template,
                "{uri}: the doc got the page nonce"
            );
        }
    }

    #[tokio::test]
    async fn page_csp_ignores_forwarded_host() {
        let (_site, apps) = page_apps();
        for forwarded in [
            ("x-forwarded-host", "evil.example:1; script-src *"),
            ("forwarded", "host=evil.example"),
            ("x-forwarded-proto", "https"),
        ] {
            for (app, uri) in &apps {
                let req = raw_request("GET", uri, &[("host", LOCAL_HOST), forwarded]);
                let (csp, _) = page_with_csp(app, req).await;
                assert_eq!(
                    directive(&csp, "connect-src"),
                    Some("connect-src 'self' ws://127.0.0.1:1"),
                    "{uri} with {forwarded:?}: {csp}"
                );
                assert!(!csp.contains("evil"), "{uri}: {csp}");
            }
        }
    }

    #[tokio::test]
    async fn page_nonce_is_unpredictable() {
        let (_site, apps) = page_apps();
        let mut nonces = Vec::new();
        for i in 0..32 {
            let (app, uri) = &apps[i % 2];
            let (csp, _) = page_with_csp(app, get(uri)).await;
            nonces.push(nonce_of(&csp).to_string());
        }
        assert!(
            nonces.iter().all(|n| n.len() == nonces[0].len()),
            "{nonces:?}"
        );
        // A counter or clock leaves leading positions fixed across requests.
        for pos in 0..21 {
            let values: std::collections::HashSet<u8> =
                nonces.iter().map(|n| n.as_bytes()[pos]).collect();
            assert!(values.len() >= 2, "position {pos} never varies: {nonces:?}");
        }
    }

    #[tokio::test]
    async fn every_html_response_carries_a_csp() {
        let site = local_site();
        let serve = router(site.state());
        let stdin = router(test_state(|_| {}));
        let mut targets: Vec<(&str, &str)> = ROUTES.to_vec();
        targets.extend([
            ("GET", "/view/doc.md"),
            ("GET", "/view/missing.md"),
            ("GET", "/view/notes.txt"),
            ("GET", "/view/.git/x.md"),
            ("GET", "/render/doc.md"),
            ("GET", "/render/doc.md?mode=source"),
            ("GET", "/render/missing.md"),
            ("GET", "/local/pic.svg"),
            ("GET", "/local/missing.png"),
            ("GET", "/not-a-route"),
            ("POST", "/"),
        ]);
        let mut bad = Vec::new();
        let mut html_seen = 0;
        for (mode, app) in [("serve", &serve), ("stdin", &stdin)] {
            for (method, uri) in &targets {
                let response = app.clone().oneshot(request(method, uri)).await.unwrap();
                let headers = response.headers();
                let is_html = headers
                    .get(header::CONTENT_TYPE)
                    .is_some_and(|v| v.to_str().unwrap().starts_with("text/html"));
                if !is_html {
                    continue;
                }
                html_seen += 1;
                let csp: Vec<&str> = headers
                    .get_all(header::CONTENT_SECURITY_POLICY)
                    .iter()
                    .map(|v| v.to_str().unwrap())
                    .collect();
                let ok = match csp.as_slice() {
                    [one] => *one == crate::security::SANDBOX_CSP || one.contains("'nonce-"),
                    _ => false,
                };
                if !ok {
                    bad.push(format!("{mode} {method} {uri}: {csp:?}"));
                }
            }
        }
        assert!(html_seen >= 4, "too few HTML responses to mean anything");
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[tokio::test]
    async fn page_csp_leaves_ipv6_hosts_to_self() {
        // Browsers reject an IPv6 literal as a CSP source, so only 'self' can match it.
        let (_site, apps) = page_apps();
        for host in ["[::1]:1", "[::1]", "[::1]:3456"] {
            for (app, uri) in &apps {
                let req = raw_request("GET", uri, &[("host", host)]);
                let (csp, _) = page_with_csp(app, req).await;
                assert_eq!(
                    directive(&csp, "connect-src"),
                    Some("connect-src 'self'"),
                    "{uri} via {host}: {csp}"
                );
                assert!(!csp.contains("ws://["), "{uri} via {host}: {csp}");
            }
        }
    }

    #[tokio::test]
    async fn page_csp_snapshot() {
        let (_site, apps) = page_apps();
        let mut policies = Vec::new();
        for (app, uri) in &apps {
            let (csp, _) = page_with_csp(app, get(uri)).await;
            let nonce = nonce_of(&csp).to_string();
            policies.push(csp.replace(&nonce, "[nonce]"));
        }
        assert_eq!(policies[0], policies[1], "serve and stdin pages differ");
        insta::assert_snapshot!(policies[0].split("; ").collect::<Vec<_>>().join("\n"));
    }

    async fn send_json(ws: &mut Client, frame: serde_json::Value) {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as TMessage;
        ws.send(TMessage::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    /// Sends `hello` for `path`, then a checkbox probe, and returns the frames that arrive
    /// before the probe's reply. One socket's messages are handled in order.
    async fn hello_frames(ws: &mut Client, path: &str) -> Vec<serde_json::Value> {
        send_json(
            ws,
            serde_json::json!({"type": "hello", "path": path, "tab": "t1"}),
        )
        .await;
        send_json(ws, serde_json::json!({"type": "checkbox", "cid": "probe"})).await;
        let mut frames = Vec::new();
        loop {
            let msg = next_json(ws).await;
            if msg["cid"] == "probe" {
                return frames;
            }
            frames.push(msg);
        }
    }

    fn contents(frames: &[serde_json::Value]) -> Vec<&serde_json::Value> {
        frames.iter().filter(|f| f["type"] == "content").collect()
    }

    fn paths(frames: &[serde_json::Value]) -> Vec<String> {
        contents(frames)
            .iter()
            .map(|f| f["path"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    /// An update for `path` whose html is the path itself.
    fn update_for(path: &str) -> ContentUpdate {
        ContentUpdate {
            relpath: path.to_string(),
            rendered_html: path.to_string(),
            source_html: String::new(),
            file_stats: String::new(),
            rev: Rev::of(path.as_bytes()),
            stops: Vec::new(),
        }
    }

    /// Reads until scroll `line` arrives, returning every frame seen on the way.
    async fn frames_until(ws: &mut Client, line: u32) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        loop {
            let msg = next_json(ws).await;
            if msg["type"] == "scroll" && msg["line"] == line {
                return frames;
            }
            frames.push(msg);
        }
    }

    async fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(std::time::Instant::now() < deadline, "never: {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    const PAGE: &[u8] = b"# Page\n\n[next](next.md)\n";

    #[tokio::test]
    async fn hello_returns_content_for_requested_path() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/sub/page.md", PAGE)]);
        let state = site.state();
        let port = serve(&state).await;
        let md = std::str::from_utf8(PAGE).unwrap();
        let want = render::render_dir(md, None, StdPath::new("sub/page.md"));
        assert_ne!(
            want,
            render::render_dir(md, None, StdPath::new("page.md")),
            "the fixture must render differently per path"
        );

        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "sub/page.md").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert_eq!(got[0]["path"], "sub/page.md");
            assert_eq!(got[0]["html"], want.as_str());
            assert_eq!(got[0]["source"], render::render_source(md, None).as_str());
            assert_eq!(got[0]["file_stats"], render::format_file_stats(md).as_str());
            assert_eq!(got[0]["rev"], git_rev(PAGE));
        })
        .await;
    }

    /// Highlighted `<pre>` attributes come out in no fixed order, so `lang` is set aside.
    fn unordered(html: &str) -> String {
        html.replace(r#" lang="rust""#, "")
    }

    #[tokio::test]
    async fn hello_renders_with_the_active_syntax_theme() {
        let text: &[u8] = b"# Code\n\n```rust\nfn main() { let x = 1; }\n```\n";
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/code.md", text)]);
        let root = site.root.clone();
        let catppuccin = theme::resolve_by_name("catppuccin").unwrap();
        let syntax = catppuccin.active_data().syntax.clone();
        assert!(syntax.is_some(), "catppuccin ships a tmTheme");
        let state = test_state(|init| {
            init.mode = Mode::Serve;
            init.source_file = Some(root.join("doc.md"));
            init.initial_relpath = Some("doc.md".to_string());
            init.base_dir = root;
            init.registry = ThemeRegistry::new(catppuccin);
        });
        let port = serve(&state).await;
        let md = std::str::from_utf8(text).unwrap();
        let want = render::render_dir(md, syntax.as_ref(), StdPath::new("code.md"));
        assert_ne!(want, render::render_dir(md, None, StdPath::new("code.md")));
        assert!(want.contains("style=\"color:"), "{want}");

        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "code.md").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert_eq!(
                unordered(got[0]["html"].as_str().unwrap()),
                unordered(&want)
            );
            assert_eq!(
                got[0]["source"],
                render::render_source(md, syntax.as_ref()).as_str()
            );
        })
        .await;
    }

    #[tokio::test]
    async fn hello_reads_the_doc_from_disk() {
        let site = Site::new(&[("root/doc.md", b"# Old\n")]);
        let state = site.state();
        let port = serve(&state).await;
        let text: &[u8] = b"# New\n";
        std::fs::write(site.root.join("doc.md"), text).unwrap();

        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "doc.md").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert!(
                got[0]["html"].as_str().unwrap().contains("New"),
                "{frames:?}"
            );
            assert_eq!(got[0]["rev"], git_rev(text));
        })
        .await;
    }

    #[tokio::test]
    async fn hello_for_a_doc_the_page_cannot_view_is_gone() {
        let site = Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("secret.md", b"# Secret\n"),
            ("root/.git/x.md", b"# Git\n"),
            ("root/x.birta.jsonl", b"{}\n"),
            ("root/a.txt", b"# Text\n"),
        ]);
        let state = site.state();
        let port = serve(&state).await;
        let absolute = site.base.join("secret.md").display().to_string();
        let rows = [
            "../secret.md",
            ".git/x.md",
            "x.birta.jsonl",
            "a.txt",
            absolute.as_str(),
            "missing.md",
        ];

        within(async {
            let mut ws = connect(port).await;
            let mut bad = Vec::new();
            for row in rows {
                // Identical to a missing doc's reply, so a refusal never confirms a file.
                let frames = hello_frames(&mut ws, row).await;
                if frames != [serde_json::json!({"type": "gone", "path": row})] {
                    bad.push(format!("{row}: {frames:?}"));
                }
                if state.viewers.count(row) != 0 {
                    bad.push(format!("{row}: counted as a viewer"));
                }
            }
            assert!(bad.is_empty(), "{bad:#?}");
            assert_eq!(paths(&hello_frames(&mut ws, "doc.md").await), ["doc.md"]);
        })
        .await;
    }

    #[tokio::test]
    async fn hello_in_stdin_mode_gets_only_the_piped_doc() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n")]);
        let state = site.stdin_state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "doc.md").await;
            assert_eq!(
                frames,
                [serde_json::json!({"type": "gone", "path": "doc.md"})]
            );
            let frames = hello_frames(&mut ws, "").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert_eq!(got[0]["html"], "<p>hello</p>");
            assert_eq!(got[0]["path"], "");
        })
        .await;
    }

    #[tokio::test]
    async fn content_goes_only_to_its_viewers() {
        let site = Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("root/a.md", b"# A\n"),
            ("root/b.md", b"# B\n"),
        ]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut a = connect(port).await;
            let mut b = connect(port).await;
            let mut legacy = connect(port).await;
            hello_frames(&mut a, "a.md").await;
            hello_frames(&mut b, "b.md").await;

            for path in ["a.md", "b.md", "doc.md"] {
                state.publish_content(update_for(path)).await;
            }
            state.hub.send(WsEvent::Scroll(SENTINEL)).unwrap();
            assert_eq!(paths(&frames_until(&mut a, SENTINEL).await), ["a.md"]);
            assert_eq!(paths(&frames_until(&mut b, SENTINEL).await), ["b.md"]);
            assert_eq!(
                paths(&frames_until(&mut legacy, SENTINEL).await),
                ["a.md", "b.md", "doc.md"],
                "a connection that never says hello gets every doc"
            );
        })
        .await;
    }

    #[test]
    fn viewers_refcount() {
        let viewers = Arc::new(Viewers::default());
        let a1 = viewers.view("a.md");
        let a2 = viewers.view("a.md");
        let b = viewers.view("b.md");
        assert_eq!(viewers.count("a.md"), 2);
        assert_eq!(viewers.count("b.md"), 1);
        assert_eq!(viewers.count("c.md"), 0);

        drop(a1);
        assert_eq!(viewers.count("a.md"), 1);
        drop(b);
        assert_eq!(viewers.count("b.md"), 0);
        drop(a2);
        assert_eq!(viewers.count("a.md"), 0);
        assert!(
            viewers.counts.lock().unwrap().is_empty(),
            "no empty entries"
        );
    }

    #[tokio::test]
    async fn viewers_follow_hello_and_disconnect() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/a.md", b"# A\n")]);
        let state = site.state();
        let port = serve(&state).await;
        let count = |path: &str| state.viewers.count(path);

        within(async {
            let mut a = connect(port).await;
            let mut b = connect(port).await;
            assert_eq!(count("doc.md"), 0, "connecting alone views nothing");

            hello_frames(&mut a, "a.md").await;
            hello_frames(&mut a, "a.md").await;
            assert_eq!(count("a.md"), 1, "a repeated hello counts once");
            hello_frames(&mut b, "a.md").await;
            assert_eq!(count("a.md"), 2);

            hello_frames(&mut a, "doc.md").await;
            assert_eq!((count("a.md"), count("doc.md")), (1, 1), "path change");

            drop(b);
            eventually("b's disconnect uncounts a.md", || count("a.md") == 0).await;
            drop(a);
            eventually("a's disconnect uncounts doc.md", || count("doc.md") == 0).await;
        })
        .await;
    }

    #[tokio::test]
    async fn lag_resend_uses_viewed_path() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/b.md", b"# B\n")]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            hello_frames(&mut ws, "b.md").await;
            sync(&state, &mut ws, BARRIER).await;

            // The dropped update exists only on disk.
            let text: &[u8] = b"# B after lag\n\n[x](x.md)\n";
            std::fs::write(site.root.join("b.md"), text).unwrap();
            flood(&state, 256);

            let resent = sync(&state, &mut ws, SENTINEL).await;
            assert_eq!(resent.len(), 1, "{resent:?}");
            let md = std::str::from_utf8(text).unwrap();
            assert_eq!(resent[0]["path"], "b.md");
            assert_eq!(
                resent[0]["html"],
                render::render_dir(md, None, StdPath::new("b.md")).as_str()
            );
            assert_eq!(resent[0]["rev"], git_rev(text));
        })
        .await;
    }

    fn two_tone_theme() -> theme::ResolvedTheme {
        let variant = |css: &str| {
            Box::new(VariantData {
                css_vars: css.to_string(),
                syntax: None,
            })
        };
        theme::ResolvedTheme {
            name: "two-tone".to_string(),
            variants: ThemeVariants::Both {
                light: variant("--x: light"),
                dark: variant("--x: dark"),
            },
            active_variant: Variant::Dark,
        }
    }

    #[tokio::test]
    async fn lag_resend_restores_theme_state() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n")]);
        let root = site.root.clone();
        let state = test_state(|init| {
            init.mode = Mode::Serve;
            init.source_file = Some(root.join("doc.md"));
            init.initial_relpath = Some("doc.md".to_string());
            init.base_dir = root;
            init.registry = ThemeRegistry::new(two_tone_theme());
        });
        let port = serve(&state).await;

        within(async {
            let mut legacy = connect(port).await;
            let mut viewer = connect(port).await;
            hello_frames(&mut viewer, "doc.md").await;
            state.hub.send(WsEvent::Scroll(BARRIER)).unwrap();
            frames_until(&mut legacy, BARRIER).await;
            frames_until(&mut viewer, BARRIER).await;

            handle_variant_change(&state, Variant::Light).await;
            flood(&state, 256);
            state.hub.send(WsEvent::Scroll(SENTINEL)).unwrap();

            for (name, ws) in [("legacy", &mut legacy), ("viewer", &mut viewer)] {
                let frames = frames_until(ws, SENTINEL).await;
                let restored = frames.iter().any(|f| {
                    f["theme_name"] == "two-tone"
                        && f["css_vars"] == "--x: light"
                        && f["active_variant"] == "light"
                });
                assert!(
                    restored,
                    "{name} never got the light theme back: {frames:?}"
                );
            }
        })
        .await;
    }

    /// Writes each `(rel, text)` under `root` in order, then returns the relpaths the hub
    /// renders until every doc in `want` has been, plus a 500 ms drain.
    async fn rendered_after(
        spy: &mut broadcast::Receiver<WsEvent>,
        root: &StdPath,
        writes: &[(&str, &str)],
        want: &[&str],
    ) -> Vec<String> {
        for (rel, text) in writes {
            std::fs::write(root.join(rel), text).unwrap();
        }
        let mut rendered = Vec::new();
        let waited = tokio::time::timeout(Duration::from_secs(10), async {
            while !want.iter().all(|w| rendered.iter().any(|r| r == w)) {
                if let Ok(WsEvent::Content(update)) = spy.recv().await {
                    rendered.push(update.relpath);
                }
            }
        })
        .await;
        // A batch need not publish in write order.
        while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(500), spy.recv()).await
        {
            if let WsEvent::Content(update) = event {
                rendered.push(update.relpath);
            }
        }
        assert!(waited.is_ok(), "{want:?} not all rendered: {rendered:?}");
        rendered
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_renders_only_viewed_and_entry_docs() {
        let site = Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("root/a.md", b"# A\n"),
            ("root/b.md", b"# B\n"),
        ]);
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let mut spy = state.hub.subscribe();
        let port = serve(&state).await;
        let root = site.root.as_path();
        let b = "b.md".to_string();

        let mut viewer = connect(port).await;
        hello_frames(&mut viewer, "a.md").await;
        // `b.md` goes first in each round, so its event is never in a later batch.
        let writes = [
            ("b.md", "# B2\n"),
            ("a.md", "# A2\n"),
            ("doc.md", "# Doc2\n"),
        ];
        let rendered = rendered_after(&mut spy, root, &writes, &["a.md", "doc.md"]).await;
        assert!(!rendered.contains(&b), "only hello'd sockets: {rendered:?}");

        let mut legacy = connect(port).await;
        let writes = [("b.md", "# B3\n"), ("a.md", "# A3\n")];
        let rendered = rendered_after(&mut spy, root, &writes, &["a.md", "b.md"]).await;
        assert!(rendered.contains(&b), "a no-hello socket views everything");

        hello_frames(&mut legacy, "a.md").await;
        let writes = [("b.md", "# B4\n"), ("a.md", "# A4\n")];
        let rendered = rendered_after(&mut spy, root, &writes, &["a.md"]).await;
        assert!(!rendered.contains(&b), "after its hello: {rendered:?}");

        let legacy = connect(port).await;
        let writes = [("b.md", "# B5\n"), ("a.md", "# A5\n")];
        rendered_after(&mut spy, root, &writes, &["a.md", "b.md"]).await;
        drop(legacy);
        eventually("the no-hello socket disconnects", || {
            state.connections.load(Ordering::Relaxed) == 2
        })
        .await;
        let writes = [("b.md", "# B6\n"), ("a.md", "# A6\n")];
        let rendered = rendered_after(&mut spy, root, &writes, &["a.md"]).await;
        assert!(!rendered.contains(&b), "after it disconnects: {rendered:?}");
    }

    #[test]
    fn hello_tab_ids_outside_the_grammar_are_absent() {
        let long = "x".repeat(65);
        let max = "x".repeat(64);
        let huge = "x".repeat(100 * 1024);
        let tab = |v: serde_json::Value| tab_id(&serde_json::json!({"type": "hello", "tab": v}));
        assert_eq!(tab("aZ09-_".into()).as_deref(), Some("aZ09-_"));
        assert_eq!(tab(max.clone().into()), Some(max));
        for bad in [
            serde_json::json!(""),
            serde_json::json!(long),
            serde_json::json!(huge),
            serde_json::json!("a b"),
            serde_json::json!("a/b"),
            serde_json::json!("é"),
            serde_json::json!("a\n"),
            serde_json::json!(7),
            serde_json::json!(null),
        ] {
            assert_eq!(tab(bad.clone()), None, "{bad}");
        }
        assert_eq!(tab_id(&serde_json::json!({"type": "hello"})), None);
    }

    #[tokio::test]
    async fn malformed_hello_leaves_the_socket_as_it_was() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/a.md", b"# A\n")]);
        let state = site.state();
        let port = serve(&state).await;
        let huge = "x".repeat(100 * 1024);

        within(async {
            let mut ws = connect(port).await;
            hello_frames(&mut ws, "a.md").await;
            for frame in [
                serde_json::json!({"type": "hello"}),
                serde_json::json!({"type": "hello", "path": 7}),
                serde_json::json!({"type": "hello", "path": null, "tab": "t"}),
                serde_json::json!({"type": "hello", "path": "a.md", "tab": huge}),
            ] {
                send_json(&mut ws, frame).await;
            }
            // The probe is still answered, and the socket still views a.md.
            hello_frames(&mut ws, "a.md").await;
            assert_eq!(state.viewers.count("a.md"), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn refused_hello_after_a_valid_one_views_nothing() {
        let site = Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("root/a.md", b"# A\n"),
            ("secret.md", b"# Secret\n"),
        ]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            assert_eq!(paths(&hello_frames(&mut ws, "a.md").await), ["a.md"]);
            let frames = hello_frames(&mut ws, "../secret.md").await;
            assert_eq!(
                frames,
                [serde_json::json!({"type": "gone", "path": "../secret.md"})]
            );
            assert_eq!(state.viewers.count("a.md"), 0);

            for path in ["a.md", "doc.md", "../secret.md"] {
                state.publish_content(update_for(path)).await;
            }
            state.hub.send(WsEvent::Gone("a.md".to_string())).unwrap();
            let frames = sync_frames(&state, &mut ws, SENTINEL).await;
            assert!(frames.is_empty(), "a socket viewing nothing got {frames:?}");
        })
        .await;
    }

    async fn sync_frames(state: &AppState, ws: &mut Client, line: u32) -> Vec<serde_json::Value> {
        state.hub.send(WsEvent::Scroll(line)).unwrap();
        frames_until(ws, line).await
    }

    #[tokio::test]
    async fn gone_goes_only_to_its_viewers() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/a.md", b"# A\n")]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut viewer = connect(port).await;
            let mut legacy = connect(port).await;
            hello_frames(&mut viewer, "a.md").await;
            for path in ["b.md", "a.md"] {
                state.hub.send(WsEvent::Gone(path.to_string())).unwrap();
            }
            let gone = |frames: Vec<serde_json::Value>| -> Vec<String> {
                frames
                    .iter()
                    .filter(|f| f["type"] == "gone")
                    .map(|f| f["path"].as_str().unwrap().to_string())
                    .collect()
            };
            assert_eq!(
                gone(sync_frames(&state, &mut viewer, SENTINEL).await),
                ["a.md"]
            );
            assert_eq!(
                gone(frames_until(&mut legacy, SENTINEL).await),
                ["b.md", "a.md"]
            );
        })
        .await;
    }

    #[tokio::test]
    async fn lag_resend_for_a_removed_viewed_doc_is_gone() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/b.md", b"# B\n")]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            hello_frames(&mut ws, "b.md").await;
            sync(&state, &mut ws, BARRIER).await;
            std::fs::remove_file(site.root.join("b.md")).unwrap();
            flood(&state, 256);

            let frames = sync_frames(&state, &mut ws, SENTINEL).await;
            assert!(contents(&frames).is_empty(), "{frames:?}");
            let gone = serde_json::json!({"type": "gone", "path": "b.md"});
            assert!(frames.contains(&gone), "{frames:?}");
        })
        .await;
    }

    const CODE: &[u8] = b"# Code\n\n```rust\nfn main() { let x = 1; }\n```\n";

    #[tokio::test]
    async fn hello_uses_the_theme_after_a_switch() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/code.md", CODE)]);
        let root = site.root.clone();
        let catppuccin = theme::resolve_by_name("catppuccin").unwrap();
        let ThemeVariants::Both { light, .. } = &catppuccin.variants else {
            panic!("catppuccin has two variants");
        };
        let light = light.syntax.clone();
        let dark = catppuccin.active_data().syntax.clone();
        let state = test_state(|init| {
            init.mode = Mode::Serve;
            init.source_file = Some(root.join("doc.md"));
            init.initial_relpath = Some("doc.md".to_string());
            init.base_dir = root;
            init.registry = ThemeRegistry::new(catppuccin);
        });
        let port = serve(&state).await;
        let md = std::str::from_utf8(CODE).unwrap();
        let want = render::render_dir(md, light.as_ref(), StdPath::new("code.md"));
        assert_ne!(
            want,
            render::render_dir(md, dark.as_ref(), StdPath::new("code.md"))
        );

        handle_variant_change(&state, Variant::Light).await;
        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "code.md").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert_eq!(
                unordered(got[0]["html"].as_str().unwrap()),
                unordered(&want)
            );
        })
        .await;
    }

    #[tokio::test]
    async fn hello_rev_matches_its_html_under_writes() {
        use std::sync::atomic::AtomicBool;
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/code.md", b"# V0\n")]);
        let state = site.state();
        let port = serve(&state).await;
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (stop, file) = (stop.clone(), site.root.join("code.md"));
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    std::fs::write(&file, format!("# V{n}\n")).unwrap();
                }
            })
        };

        let mut revs = std::collections::HashMap::new();
        let mut checked = 0;
        let mut ws = connect(port).await;
        for _ in 0..200 {
            let frames = hello_frames(&mut ws, "code.md").await;
            let Some(frame) = contents(&frames).first().copied() else {
                continue;
            };
            let html = frame["html"].as_str().unwrap();
            let Some(k) = html
                .split_once("</a>V")
                .and_then(|(_, rest)| rest.split_once("</h1>"))
                .map(|(k, _)| k.to_string())
            else {
                continue;
            };
            let want = revs
                .entry(k.clone())
                .or_insert_with(|| git_rev(format!("# V{k}\n").as_bytes()));
            assert_eq!(frame["rev"], want.as_str(), "html says V{k}");
            checked += 1;
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert!(checked > 100, "only {checked} replies carried a version");
        assert!(revs.len() > 1, "the writer never raced the reads");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn render_filter_survives_repeat_hello_and_hello_disconnect() {
        let site = Site::new(&[
            ("root/doc.md", b"# Doc\n"),
            ("root/a.md", b"# A\n"),
            ("root/b.md", b"# B\n"),
        ]);
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let mut spy = state.hub.subscribe();
        let port = serve(&state).await;

        let mut x = connect(port).await;
        let mut y = connect(port).await;
        hello_frames(&mut x, "a.md").await;
        hello_frames(&mut x, "a.md").await;
        hello_frames(&mut y, "a.md").await;
        drop(y);
        eventually("y disconnects", || {
            state.connections.load(Ordering::Relaxed) == 1
        })
        .await;

        let writes = [("b.md", "# B2\n"), ("a.md", "# A2\n")];
        let rendered = rendered_after(&mut spy, &site.root, &writes, &["a.md"]).await;
        assert!(!rendered.iter().any(|r| r == "b.md"), "{rendered:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_renders_a_change_once_however_many_view_it() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/a.md", b"# A\n")]);
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let mut spy = state.hub.subscribe();
        let port = serve(&state).await;
        let mut x = connect(port).await;
        let mut y = connect(port).await;
        hello_frames(&mut x, "a.md").await;
        hello_frames(&mut y, "a.md").await;

        let writes = [("a.md", "# A2\n")];
        let rendered = rendered_after(&mut spy, &site.root, &writes, &["a.md"]).await;
        assert_eq!(rendered, ["a.md"]);
    }

    /// The race below rarely lands in the gap, so this pins the order directly: the reply
    /// waits on the registry, before its disk read, and the viewer must already count.
    #[tokio::test]
    async fn hello_counts_the_viewer_before_it_reads() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/b.md", b"# B\n")]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            let registry = state.registry.write().await;
            send_json(
                &mut ws,
                serde_json::json!({"type": "hello", "path": "b.md"}),
            )
            .await;
            eventually("b.md is counted while its render waits", || {
                state.viewers.count("b.md") == 1
            })
            .await;
            drop(registry);
            loop {
                let frame = next_json(&mut ws).await;
                if frame["type"] == "content" && frame["path"] == "b.md" {
                    break;
                }
            }
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hello_racing_a_write_converges() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/b.md", b"# B\n")]);
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let port = serve(&state).await;
        let file = site.root.join("b.md");

        for round in 0..50 {
            let text = format!("# Round {round}\n");
            let want = git_rev(text.as_bytes());
            let mut ws = connect(port).await;
            // Views another doc first, so only this hello makes b.md wanted.
            hello_frames(&mut ws, "doc.md").await;
            let writer = {
                let (file, text) = (file.clone(), text.clone());
                std::thread::spawn(move || std::fs::write(file, text).unwrap())
            };
            send_json(
                &mut ws,
                serde_json::json!({"type": "hello", "path": "b.md"}),
            )
            .await;
            writer.join().unwrap();

            let mut last = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let quiet = Duration::from_millis(200);
            loop {
                let wait = if last.as_ref() == Some(&want) {
                    quiet
                } else {
                    deadline.saturating_duration_since(tokio::time::Instant::now())
                };
                match tokio::time::timeout(wait, next_json(&mut ws)).await {
                    Ok(frame) if frame["type"] == "content" && frame["path"] == "b.md" => {
                        last = frame["rev"].as_str().map(str::to_string);
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            assert_eq!(last, Some(want), "round {round}");
            drop(ws);
            eventually("the round's socket closes", || {
                state.connections.load(Ordering::Relaxed) == 0
            })
            .await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alias_viewer_gets_change_made_through_target() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/real.md", b"# Real\n")]);
        site.link("root/alias.md", "real.md");
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let port = serve(&state).await;

        let mut ws = connect(port).await;
        assert_eq!(
            paths(&hello_frames(&mut ws, "alias.md").await),
            ["alias.md"]
        );
        let text = "# Real edited\n\n[x](x.md)\n";
        std::fs::write(site.root.join("real.md"), text).unwrap();

        let frame = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = next_json(&mut ws).await;
                if frame["type"] == "content" {
                    return frame;
                }
            }
        })
        .await
        .expect("no content for the alias");
        assert_eq!(frame["path"], "alias.md");
        assert_eq!(
            frame["html"],
            render::render_dir(text, None, StdPath::new("alias.md")).as_str()
        );
    }

    #[tokio::test]
    async fn theme_change_survives_a_missing_entry_doc() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/b.md", b"# B\n")]);
        let state = site.state();
        state.current.write().await.html = "<p>cached entry</p>".to_string();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            hello_frames(&mut ws, "b.md").await;
            std::fs::rename(site.root.join("doc.md"), site.root.join("home.md")).unwrap();
            send_json(
                &mut ws,
                serde_json::json!({"type": "variant_change", "variant": "light"}),
            )
            .await;

            let update = loop {
                let frame = next_json(&mut ws).await;
                if frame["type"] == "theme_update" {
                    break frame;
                }
            };
            assert_eq!(update["active_variant"], "light", "{update}");
            assert_eq!(update["path"], "doc.md", "{update}");
            assert_eq!(update["html"], "<p>cached entry</p>", "{update}");
        })
        .await;
    }

    const STOPS_MD: &[u8] = b"# Title\n\nIntro text.\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD\n```\n\n```math\nx^2\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    /// The stops of `STOPS_MD` as sent, without `n`.
    fn stops_md_stops() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"kind": "heading", "sp": "1:1-1:7", "lines": [1, 1], "level": 1, "section": [1, 22]}),
            serde_json::json!({"kind": "paragraph", "sp": "3:1-3:11", "lines": [3, 3]}),
            serde_json::json!({"kind": "list_item", "sp": "5:1-5:5", "lines": [5, 5]}),
            serde_json::json!({"kind": "list_item", "sp": "6:1-7:0", "lines": [6, 6]}),
            serde_json::json!({"kind": "code_block", "sp": "8:1-10:3", "lines": [8, 10]}),
            serde_json::json!({"kind": "code_block", "sp": "12:1-14:3", "lines": [12, 14]}),
            serde_json::json!({"kind": "code_block", "sp": "16:1-18:3", "lines": [16, 18]}),
            serde_json::json!({"kind": "table_row", "sp": "22:1-22:9", "lines": [22, 22]}),
        ]
    }

    /// `holder["stops"]` with `n` checked (0, 1, …) and dropped, and null fields left out.
    fn wire_stops(holder: &serde_json::Value) -> Vec<serde_json::Value> {
        assert!(
            holder.get("owners").is_none(),
            "owners stay server-side: {holder}"
        );
        let stops = holder["stops"]
            .as_array()
            .unwrap_or_else(|| panic!("no stops: {holder}"));
        let mut prev: Option<u64> = None;
        let mut out = Vec::new();
        for stop in stops {
            let mut obj = stop.as_object().unwrap().clone();
            let keys = ["n", "kind", "sp", "lines", "level", "section"];
            assert!(obj.keys().all(|k| keys.contains(&k.as_str())), "{stop}");
            let n = obj.remove("n").and_then(|n| n.as_u64()).expect("n");
            assert_eq!(n, prev.map_or(0, |p| p + 1), "{stops:?}");
            prev = Some(n);
            obj.retain(|_, v| !v.is_null());
            out.push(serde_json::Value::Object(obj));
        }
        out
    }

    async fn page_boot(app: Router, uri: &str) -> serde_json::Value {
        let response = app.oneshot(get(uri)).await.unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        boot_of(std::str::from_utf8(&body).unwrap())
    }

    #[tokio::test]
    async fn view_boot_carries_stops() {
        let site = Site::new(&[("root/doc.md", STOPS_MD)]);
        let boot = page_boot(router(site.state()), "/view/doc.md").await;
        assert_eq!(wire_stops(&boot), stops_md_stops(), "{boot}");
    }

    #[tokio::test]
    async fn hello_content_carries_stops() {
        let site = Site::new(&[("root/doc.md", b"# Doc\n"), ("root/sub/stops.md", STOPS_MD)]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            let frames = hello_frames(&mut ws, "sub/stops.md").await;
            let got = contents(&frames);
            assert_eq!(got.len(), 1, "{frames:?}");
            assert_eq!(wire_stops(got[0]), stops_md_stops());
        })
        .await;
    }

    #[tokio::test]
    async fn the_first_frame_and_the_stdin_page_carry_stops() {
        let site = Site::new(&[("root/doc.md", STOPS_MD)]);
        let md = std::str::from_utf8(STOPS_MD).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let served = AppInit::serve(&site.root.join("doc.md"), &listener, test_opts()).unwrap();
        let stdin = AppInit::stdin(md, &listener, test_opts()).unwrap();

        for (mode, init) in [("serve", served), ("stdin", stdin)] {
            let state = AppState::new(init);
            let port = serve(&state).await;
            within(async {
                let (mut ws, _) =
                    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                        .await
                        .unwrap();
                let initial = next_json(&mut ws).await;
                assert_eq!(initial["type"], "content", "{mode}");
                assert_eq!(wire_stops(&initial), stops_md_stops(), "{mode}");
            })
            .await;
            if mode == "stdin" {
                let boot = page_boot(router(state.clone()), "/").await;
                assert_eq!(wire_stops(&boot), stops_md_stops(), "stdin page");
            }
        }
    }

    #[tokio::test]
    async fn the_cached_frame_keeps_its_stops_after_a_theme_switch() {
        let site = Site::new(&[("root/doc.md", STOPS_MD)]);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let init = AppInit::serve(&site.root.join("doc.md"), &listener, test_opts()).unwrap();
        let state = AppState::new(init);
        handle_variant_change(&state, Variant::Light).await;
        let port = serve(&state).await;

        within(async {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                .await
                .unwrap();
            let initial = next_json(&mut ws).await;
            assert_eq!(initial["type"], "content");
            assert_eq!(wire_stops(&initial), stops_md_stops());
        })
        .await;
    }

    #[tokio::test]
    async fn theme_update_carries_the_stops_of_its_html() {
        let site = Site::new(&[("root/doc.md", STOPS_MD)]);
        let state = site.state();
        let port = serve(&state).await;

        within(async {
            let mut ws = connect(port).await;
            send_json(
                &mut ws,
                serde_json::json!({"type": "variant_change", "variant": "light"}),
            )
            .await;
            let update = loop {
                let frame = next_json(&mut ws).await;
                if frame["type"] == "theme_update" {
                    break frame;
                }
            };
            assert_eq!(update["rev"], git_rev(STOPS_MD), "{update}");
            assert_eq!(wire_stops(&update), stops_md_stops(), "{update}");
        })
        .await;
    }

    #[tokio::test]
    async fn a_lag_resync_after_an_update_sends_the_new_stops() {
        let site = Site::new(&[("root/doc.md", b"# Before\n")]);
        let state = site.state();
        let _watch = watcher::watch(state.clone()).unwrap();
        let port = serve(&state).await;

        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
                .await
                .unwrap();
            next_json(&mut ws).await;
            std::fs::write(site.root.join("doc.md"), STOPS_MD).unwrap();
            while next_json(&mut ws).await["type"] != "content" {}

            sync(&state, &mut ws, BARRIER).await;
            flood(&state, 256);
            let resent = sync(&state, &mut ws, SENTINEL).await;
            assert_eq!(resent.len(), 1, "{resent:?}");
            assert_eq!(resent[0]["rev"], git_rev(STOPS_MD));
            assert_eq!(wire_stops(&resent[0]), stops_md_stops());
        })
        .await
        .expect("timed out");
    }
}
