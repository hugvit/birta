use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast};

use crate::{render, template, watcher};

struct AppState {
    current_html: RwLock<String>,
    tx: broadcast::Sender<String>,
}

pub async fn run(file: PathBuf, port: u16, no_open: bool) -> anyhow::Result<()> {
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

    start(file, listener).await
}

/// Start serving a markdown file on the given listener.
///
/// Watches the file for changes and pushes updates over WebSocket.
pub async fn start(file: PathBuf, listener: TcpListener) -> anyhow::Result<()> {
    let markdown = std::fs::read_to_string(&file)?;
    let content_html = render::render(&markdown);

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let page = template::render_page(&filename, &content_html);

    let (tx, _rx) = broadcast::channel::<String>(16);

    let state = Arc::new(AppState {
        current_html: RwLock::new(content_html),
        tx: tx.clone(),
    });

    let state_for_task = Arc::clone(&state);
    let mut rx = tx.subscribe();
    tokio::spawn(async move {
        while let Ok(html) = rx.recv().await {
            *state_for_task.current_html.write().await = html;
        }
    });

    let _debouncer = watcher::watch(file, tx)?;

    let app = router(page, state);
    axum::serve(listener, app).await?;

    Ok(())
}

fn router(page: String, state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(move || async move { Html(page) }))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let current = state.current_html.read().await.clone();
    if socket.send(Message::Text(current.into())).await.is_err() {
        return;
    }

    let mut rx = state.tx.subscribe();

    while let Ok(html) = rx.recv().await {
        if socket.send(Message::Text(html.into())).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    fn test_router() -> Router {
        let page = template::render_page("test.md", "<p>hello</p>");
        let (tx, _rx) = broadcast::channel(16);
        let state = Arc::new(AppState {
            current_html: RwLock::new("<p>hello</p>".to_string()),
            tx,
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
}
