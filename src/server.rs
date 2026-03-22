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

use crate::{render, template, watcher};

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

struct AppState {
    base_dir: PathBuf,
    source_file: Option<PathBuf>,
    current_html: RwLock<String>,
    tx: broadcast::Sender<String>,
    scroll_tx: broadcast::Sender<u32>,
    connections: AtomicUsize,
    all_disconnected: Notify,
}

pub async fn run(
    file: PathBuf,
    port: u16,
    no_open: bool,
    custom_css: Option<&str>,
) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    eprintln!("sheen: serving {filename} at http://{actual_addr}");

    if !no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("sheen: could not open browser: {e}");
        }
    }

    start(file, listener, custom_css).await
}

/// Serve markdown read from stdin (no file watching).
pub async fn run_stdin(
    markdown: &str,
    port: u16,
    no_open: bool,
    custom_css: Option<&str>,
) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    eprintln!("sheen: serving stdin at http://{actual_addr}");

    if !no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("sheen: could not open browser: {e}");
        }
    }

    let content_html = render::render(markdown);
    let page = template::render_page("stdin", &content_html, custom_css);
    let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let (tx, _rx) = broadcast::channel::<String>(16);
    let (scroll_tx, _) = broadcast::channel::<u32>(16);

    let state = Arc::new(AppState {
        base_dir,
        source_file: None,
        current_html: RwLock::new(content_html),
        tx,
        scroll_tx,
        connections: AtomicUsize::new(0),
        all_disconnected: Notify::new(),
    });

    let app = router(page, state.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;

    Ok(())
}

/// Start serving a markdown file on the given listener.
///
/// Watches the file for changes and pushes updates over WebSocket.
/// Shuts down automatically when the last browser tab disconnects.
pub async fn start(
    file: PathBuf,
    listener: TcpListener,
    custom_css: Option<&str>,
) -> anyhow::Result<()> {
    let markdown = std::fs::read_to_string(&file)?;
    let content_html = render::render(&markdown);

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let page = template::render_page(&filename, &content_html, custom_css);

    let base_dir = file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let (tx, _rx) = broadcast::channel::<String>(16);
    let (scroll_tx, _) = broadcast::channel::<u32>(16);

    let state = Arc::new(AppState {
        base_dir,
        source_file: Some(file.clone()),
        current_html: RwLock::new(content_html),
        tx: tx.clone(),
        scroll_tx,
        connections: AtomicUsize::new(0),
        all_disconnected: Notify::new(),
    });

    let state_for_task = Arc::clone(&state);
    let mut rx = tx.subscribe();
    tokio::spawn(async move {
        while let Ok(html) = rx.recv().await {
            *state_for_task.current_html.write().await = html;
        }
    });

    let _debouncer = watcher::watch(file, tx)?;

    let app = router(page, state.clone());
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
        eprintln!("\nsheen: shutting down...");
    };

    let auto_shutdown = async {
        // Wait for at least one connection before monitoring disconnects
        loop {
            state.all_disconnected.notified().await;

            if state.connections.load(Ordering::Relaxed) == 0 {
                break;
            }
        }

        // Grace period — allow reconnects (page refresh, etc.)
        tokio::time::sleep(SHUTDOWN_GRACE_PERIOD).await;

        if state.connections.load(Ordering::Relaxed) == 0 {
            eprintln!("sheen: all tabs closed, shutting down...");
        } else {
            // Reconnected during grace period, keep waiting
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
            eprintln!("sheen: all tabs closed, shutting down...");
            return;
        }
    }
}

fn router(page: String, state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(move || async move { Html(page) }))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_handler))
        .route("/scroll/{line}", post(scroll_handler))
        .route("/local/{*path}", get(local_file_handler))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .with_state(state)
}

async fn scroll_handler(Path(line): Path<u32>, State(state): State<Arc<AppState>>) -> StatusCode {
    let _ = state.scroll_tx.send(line);
    StatusCode::NO_CONTENT
}

/// Handle incoming WebSocket text messages from the browser.
fn handle_ws_message(text: &str, state: &AppState) {
    // Simple JSON parsing without serde — messages are {"type":"checkbox","line":N,"checked":B}
    if !text.starts_with('{') {
        return;
    }

    if let Some(rest) = text.strip_prefix(r#"{"type":"checkbox","line":"#) {
        // Parse: N,"checked":true/false}
        if let Some(comma_pos) = rest.find(',') {
            let line_str = &rest[..comma_pos];
            let checked = rest.contains(r#""checked":true"#);
            if let Ok(line) = line_str.parse::<usize>() {
                if let Err(e) = toggle_checkbox(state, line, checked) {
                    eprintln!("sheen: checkbox toggle failed: {e}");
                }
            }
        }
    }
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
        return Ok(()); // no change needed
    }

    lines[line - 1] = &new_line;

    // Preserve trailing newline if original had one
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

    let current = state.current_html.read().await.clone();
    if socket.send(Message::Text(current.into())).await.is_err() {
        state.connections.fetch_sub(1, Ordering::Relaxed);
        state.all_disconnected.notify_one();
        return;
    }

    let mut rx = state.tx.subscribe();
    let mut scroll_rx = state.scroll_tx.subscribe();

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(html) => {
                        if socket.send(Message::Text(html.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            result = scroll_rx.recv() => {
                if let Ok(line) = result {
                    let msg = format!(r#"{{"type":"scroll","line":{line}}}"#);
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_ws_message(&text, &state);
                    }
                    Some(Ok(_)) => {} // ignore binary/ping/pong
                    _ => break,       // disconnected or error
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

    fn test_router() -> Router {
        let page = template::render_page("test.md", "<p>hello</p>", None);
        let (tx, _rx) = broadcast::channel(16);
        let (scroll_tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState {
            base_dir: PathBuf::from("."),
            source_file: None,
            current_html: RwLock::new("<p>hello</p>".to_string()),
            tx,
            scroll_tx,
            connections: AtomicUsize::new(0),
            all_disconnected: Notify::new(),
        });
        router(page, state)
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
        let page = template::render_page("test.md", "<p>hello</p>", None);
        let (tx, _rx) = broadcast::channel(16);
        let (scroll_tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState {
            base_dir,
            source_file: None,
            current_html: RwLock::new("<p>hello</p>".to_string()),
            tx,
            scroll_tx,
            connections: AtomicUsize::new(0),
            all_disconnected: Notify::new(),
        });
        router(page, state)
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
