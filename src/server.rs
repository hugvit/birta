use std::net::SocketAddr;
use std::path::PathBuf;

use axum::response::Html;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;

use crate::render;
use crate::template;

pub async fn run(file: PathBuf, port: u16, no_open: bool) -> anyhow::Result<()> {
    let markdown = std::fs::read_to_string(&file)?;
    let content_html = render::render(&markdown);

    let filename = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());

    let page = template::render_page(&filename, &content_html);

    let app = router(page);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;

    eprintln!("sheen: serving {filename} at http://{actual_addr}");

    if !no_open {
        let url = format!("http://{actual_addr}");
        if let Err(e) = open::that(&url) {
            eprintln!("sheen: could not open browser: {e}");
        }
    }

    axum::serve(listener, app).await?;

    Ok(())
}

fn router(page: String) -> Router {
    Router::new()
        .route("/", get(move || async move { Html(page) }))
        .route("/health", get(|| async { "ok" }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    fn test_router() -> Router {
        let page = template::render_page("test.md", "<p>hello</p>");
        router(page)
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
