//! Host / Origin / Sec-Fetch guard against a real server.

#[allow(dead_code)]
mod common;

use std::time::Duration;

use common::{http_get, http_request, start_server};
use tempfile::TempDir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderName;

async fn serve_doc() -> (TempDir, u16) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("doc.md");
    std::fs::write(&file, "# Doc").unwrap();
    let port = start_server(file).await;
    (dir, port)
}

/// Upgrades `/ws` with `headers` replacing the defaults; returns the HTTP status.
async fn ws_upgrade_status(port: u16, headers: &[(&str, &str)]) -> u16 {
    let mut request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    let result = tokio::time::timeout(Duration::from_secs(30), connect_async(request))
        .await
        .expect("timed out");
    match result {
        Ok((_, response)) => response.status().as_u16(),
        Err(WsError::Http(response)) => response.status().as_u16(),
        Err(err) => panic!("unexpected ws error: {err}"),
    }
}

#[tokio::test]
async fn ws_upgrade_rejects_foreign_origin() {
    let (_dir, port) = serve_doc().await;
    let same_origin = format!("http://127.0.0.1:{port}");

    assert_eq!(ws_upgrade_status(port, &[]).await, 101, "no Origin");
    assert_eq!(
        ws_upgrade_status(port, &[("origin", &same_origin)]).await,
        101,
        "same Origin"
    );
    assert_eq!(
        ws_upgrade_status(port, &[("origin", "http://evil.com")]).await,
        403,
        "foreign Origin"
    );
}

#[tokio::test]
async fn ws_upgrade_rejects_foreign_host() {
    let (_dir, port) = serve_doc().await;
    assert_eq!(ws_upgrade_status(port, &[("host", "evil.com")]).await, 421);
}

#[tokio::test]
async fn ws_upgrade_rejects_rebinding_shape() {
    // After DNS rebinding the page's Host and Origin agree, so only Host catches it.
    let (_dir, port) = serve_doc().await;
    let host = format!("evil.com:{port}");
    let origin = format!("http://evil.com:{port}");
    assert_eq!(
        ws_upgrade_status(port, &[("host", &host), ("origin", &origin)]).await,
        421
    );
}

#[tokio::test]
async fn real_server_rejects_foreign_host() {
    let (_dir, port) = serve_doc().await;
    let local = format!("127.0.0.1:{port}");
    assert_eq!(http_get(port, "/health", &[("host", &local)]).await, 200);
    assert_eq!(
        http_get(port, "/health", &[("host", "evil.com")]).await,
        421
    );
}

#[tokio::test]
async fn real_server_rejects_missing_host() {
    // hyper accepts HTTP/1.1 without Host, so the guard is what refuses it.
    let (_dir, port) = serve_doc().await;
    assert_eq!(http_get(port, "/health", &[]).await, 421);
}

#[tokio::test]
async fn scroll_post_without_origin_still_works() {
    // The birta.nvim path: curl sends Host and no Origin.
    let (_dir, port) = serve_doc().await;
    let local = format!("127.0.0.1:{port}");
    assert_eq!(
        http_request(port, "POST", "/scroll/3", &[("host", &local)]).await,
        204
    );
}
