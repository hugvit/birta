use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::{Notify, RwLock, broadcast};

use crate::theme::{ResolvedTheme, ThemeRegistry, Variant};
use crate::{render, template, watcher};

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);
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
    pub keybindings_json: String,
}

pub(crate) struct AppState {
    pub(crate) base_dir: PathBuf,
    pub(crate) source_file: Option<PathBuf>,
    pub(crate) filename: String,
    pub(crate) custom_css: Option<String>,
    /// Raw rendered HTML (not JSON-wrapped).
    pub(crate) current_html: RwLock<String>,
    /// Sends raw HTML content updates (from watcher file changes).
    pub(crate) tx: broadcast::Sender<String>,
    /// Sends ready-to-send JSON strings (theme_update messages).
    pub(crate) theme_tx: broadcast::Sender<String>,
    pub(crate) scroll_tx: broadcast::Sender<u32>,
    pub(crate) connections: AtomicUsize,
    pub(crate) all_disconnected: Notify,
    pub(crate) registry: RwLock<ThemeRegistry>,
    pub(crate) enable_toggle: bool,
    pub(crate) font_css: Option<String>,
    pub(crate) show_header: bool,
    pub(crate) reading_mode: bool,
    pub(crate) keybindings_json: String,
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

    let content_html = render::render(markdown, opts.theme.active_data().syntax.as_ref());

    let mut registry = ThemeRegistry::new(opts.theme);
    if opts.enable_swap {
        registry.discover_all();
    }
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let (tx, _rx) = broadcast::channel::<String>(16);
    let (theme_tx, _) = broadcast::channel::<String>(16);
    let (scroll_tx, _) = broadcast::channel::<u32>(16);

    let state = Arc::new(AppState {
        base_dir,
        source_file: None,
        filename: "stdin".to_string(),
        custom_css: opts.custom_css,
        current_html: RwLock::new(content_html),
        tx,
        theme_tx,
        scroll_tx,
        connections: AtomicUsize::new(0),
        all_disconnected: Notify::new(),
        registry: RwLock::new(registry),
        enable_toggle: opts.enable_toggle,
        font_css: opts.font_css,
        show_header: opts.show_header,
        reading_mode: opts.reading_mode,
        keybindings_json: opts.keybindings_json,
    });

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
    let markdown = std::fs::read_to_string(&file)?;
    let content_html = render::render(&markdown, opts.theme.active_data().syntax.as_ref());

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let mut registry = ThemeRegistry::new(opts.theme);
    if opts.enable_swap {
        registry.discover_all();
    }

    let base_dir = file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let (tx, _rx) = broadcast::channel::<String>(16);
    let (theme_tx, _) = broadcast::channel::<String>(16);
    let (scroll_tx, _) = broadcast::channel::<u32>(16);

    let state = Arc::new(AppState {
        base_dir,
        source_file: Some(file.clone()),
        filename,
        custom_css: opts.custom_css,
        current_html: RwLock::new(content_html),
        tx: tx.clone(),
        theme_tx,
        scroll_tx,
        connections: AtomicUsize::new(0),
        all_disconnected: Notify::new(),
        registry: RwLock::new(registry),
        enable_toggle: opts.enable_toggle,
        font_css: opts.font_css,
        show_header: opts.show_header,
        reading_mode: opts.reading_mode,
        keybindings_json: opts.keybindings_json,
    });

    let state_for_task = Arc::clone(&state);
    let mut rx = tx.subscribe();
    tokio::spawn(async move {
        while let Ok(html) = rx.recv().await {
            *state_for_task.current_html.write().await = html;
        }
    });

    let state_for_watcher = Arc::clone(&state);
    let _debouncer = watcher::watch(file, tx, state_for_watcher)?;

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

        if state.connections.load(Ordering::Relaxed) == 0 {
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

        if state.connections.load(Ordering::Relaxed) == 0 {
            eprintln!("birta: all tabs closed, shutting down...");
            return;
        }
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_handler))
        .route("/scroll/{line}", post(scroll_handler))
        .route("/local/{*path}", get(local_file_handler))
        .route("/favicon.png", get(favicon_handler))
        .route("/favicon.ico", get(favicon_handler))
        .with_state(state)
}

async fn favicon_handler() -> Response {
    ([(header::CONTENT_TYPE, "image/png")], FAVICON).into_response()
}

async fn index_handler(State(state): State<Arc<AppState>>) -> Html<String> {
    let registry = state.registry.read().await;
    let theme = registry.active();
    let theme_names: Vec<&str> = registry.theme_names();
    let content_html = state.current_html.read().await;
    let page = template::render_page(&template::PageOptions {
        filename: &state.filename,
        content_html: &content_html,
        custom_css: state.custom_css.as_deref(),
        font_css: state.font_css.as_deref(),
        show_header: state.show_header,
        reading_mode: state.reading_mode,
        theme,
        theme_names: &theme_names,
        static_mode: false,
        keybindings_json: &state.keybindings_json,
    });
    Html(page)
}

async fn scroll_handler(Path(line): Path<u32>, State(state): State<Arc<AppState>>) -> StatusCode {
    let _ = state.scroll_tx.send(line);
    StatusCode::NO_CONTENT
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
            if let Err(e) = toggle_checkbox(state, line, checked) {
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
    let html = if let Some(source_file) = &state.source_file {
        match std::fs::read_to_string(source_file) {
            Ok(markdown) => render::render(&markdown, active.syntax.as_ref()),
            Err(e) => {
                eprintln!("birta: failed to re-read file for theme change: {e}");
                return;
            }
        }
    } else {
        // stdin mode — use current HTML since we can't re-render
        state.current_html.read().await.clone()
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
        "theme_name": theme.name,
        "theme_attr": theme_attr,
        "variants": theme.variant_names(),
        "active_variant": theme.active_variant.as_str(),
        "has_toggle": has_toggle,
    });

    // Update stored content HTML (raw HTML, not JSON-wrapped)
    *state.current_html.write().await = html;

    let _ = state.theme_tx.send(msg.to_string());
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
    registry.set_variant(variant);
    drop(registry);
    broadcast_theme_update(state).await;
}

/// Toggle a checkbox in the source file at the given line.
fn toggle_checkbox(state: &AppState, line: usize, checked: bool) -> anyhow::Result<()> {
    let path = state
        .source_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no source file (stdin mode)"))?;

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
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "path traversal not allowed").into_response();
    }

    let file_path = state.base_dir.join(&path);

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

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    state.connections.fetch_add(1, Ordering::Relaxed);

    // Send initial content as JSON
    let current = state.current_html.read().await.clone();
    let init_msg = serde_json::json!({
        "type": "content",
        "html": current,
    });
    if socket
        .send(Message::Text(init_msg.to_string().into()))
        .await
        .is_err()
    {
        state.connections.fetch_sub(1, Ordering::Relaxed);
        state.all_disconnected.notify_one();
        return;
    }

    let mut rx = state.tx.subscribe();
    let mut theme_rx = state.theme_tx.subscribe();
    let mut scroll_rx = state.scroll_tx.subscribe();

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(html) => {
                        // tx carries raw HTML — wrap in JSON content message
                        let msg = serde_json::json!({"type": "content", "html": html});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            result = theme_rx.recv() => {
                match result {
                    Ok(json_str) => {
                        // theme_tx carries ready-to-send JSON (theme_update messages)
                        if socket.send(Message::Text(json_str.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            result = scroll_rx.recv() => {
                if let Ok(line) = result {
                    let msg = serde_json::json!({"type": "scroll", "line": line});
                    if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_ws_message(&text, &state).await;
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
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

    fn test_state() -> Arc<AppState> {
        let theme = github_theme();
        let registry = ThemeRegistry::new(theme);
        let (tx, _rx) = broadcast::channel(16);
        let (theme_tx, _) = broadcast::channel(16);
        let (scroll_tx, _) = broadcast::channel(16);
        Arc::new(AppState {
            base_dir: PathBuf::from("."),
            source_file: None,
            filename: "test.md".to_string(),
            custom_css: None,
            current_html: RwLock::new("<p>hello</p>".to_string()),
            tx,
            theme_tx,
            scroll_tx,
            connections: AtomicUsize::new(0),
            all_disconnected: Notify::new(),
            registry: RwLock::new(registry),
            enable_toggle: true,
            font_css: None,
            show_header: true,
            reading_mode: false,
            keybindings_json: "{}".to_string(),
        })
    }

    fn test_router() -> Router {
        let state = test_state();
        router(state)
    }

    #[tokio::test]
    async fn index_returns_200_with_markdown_body() {
        let app = test_router();

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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

    #[tokio::test]
    async fn health_returns_200() {
        let app = test_router();

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }

    fn test_router_with_base_dir(base_dir: PathBuf) -> Router {
        let theme = github_theme();
        let registry = ThemeRegistry::new(theme);
        let (tx, _rx) = broadcast::channel(16);
        let (theme_tx, _) = broadcast::channel(16);
        let (scroll_tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState {
            base_dir,
            source_file: None,
            filename: "test.md".to_string(),
            custom_css: None,
            current_html: RwLock::new("<p>hello</p>".to_string()),
            tx,
            theme_tx,
            scroll_tx,
            connections: AtomicUsize::new(0),
            all_disconnected: Notify::new(),
            registry: RwLock::new(registry),
            enable_toggle: true,
            font_css: None,
            show_header: true,
            reading_mode: false,
            keybindings_json: "{}".to_string(),
        });
        router(state)
    }

    #[tokio::test]
    async fn local_file_serves_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();

        let app = test_router_with_base_dir(dir.path().to_path_buf());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/local/photo.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/png");
    }

    #[tokio::test]
    async fn local_file_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_router_with_base_dir(dir.path().to_path_buf());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/local/../../../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn local_file_returns_404_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_router_with_base_dir(dir.path().to_path_buf());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/local/nonexistent.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }
}
