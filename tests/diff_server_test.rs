use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

fn options() -> birta::server::ServerOptions {
    let variant = || {
        Box::new(birta::theme::VariantData {
            css_vars: String::new(),
            syntax: None,
        })
    };
    birta::server::ServerOptions {
        port: 0,
        no_open: true,
        custom_css: None,
        font_css: None,
        theme: birta::theme::ResolvedTheme {
            name: "github".to_string(),
            variants: birta::theme::ThemeVariants::Both {
                light: variant(),
                dark: variant(),
            },
            active_variant: birta::theme::Variant::Dark,
        },
        enable_swap: false,
        enable_toggle: true,
        show_header: true,
        reading_mode: false,
        raw_mode: true,
        features: birta::render::Features::CLI,
        keybindings_json: "{}".to_string(),
        variant_explicit: false,
    }
}

async fn start_diff_server(old: &Path, new: &Path) -> u16 {
    let (old, new) = (old.to_path_buf(), new.to_path_buf());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        birta::server::start_diff(
            old,
            new,
            birta_core::diff::Layout::Auto,
            listener,
            options(),
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

async fn get(port: u16, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

fn write_pair(dir: &Path, old: &str, new: &str) -> (PathBuf, PathBuf) {
    let (old_path, new_path) = (dir.join("old.md"), dir.join("new.md"));
    std::fs::write(&old_path, old).unwrap();
    std::fs::write(&new_path, new).unwrap();
    (old_path, new_path)
}

#[tokio::test]
async fn index_serves_the_diff_page() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = write_pair(dir.path(), "# Title\n\nold line\n", "# Title\n\nnew line\n");
    let port = start_diff_server(&old, &new).await;

    let page = get(port, "/").await;
    assert!(page.starts_with("HTTP/1.0 200"), "{}", &page[..40]);
    assert!(page.contains("class=\"birta-diff\""));
    assert_eq!(page.matches("data-hunk=\"0\"").count(), 1);
    assert!(!page.contains("data-hunk=\"1\""));
    assert!(page.contains("<title>old.md \u{2194} new.md — birta</title>"));
    assert!(page.contains(".birta-diff"), "diff stylesheet is included");
    // --raw has nothing to show for a diff, so the page starts in preview
    assert!(page.contains("var INITIAL_VIEW = 'preview';"));
    assert!(page.contains("var DIFF_MODE = true;"));
}

#[tokio::test]
async fn dev_null_reads_as_an_empty_side() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("added.md");
    std::fs::write(&new, "# Added\n").unwrap();
    let port = start_diff_server(Path::new("/dev/null"), &new).await;

    let page = get(port, "/").await;
    assert!(page.contains("class=\"birta-diff\""));
    assert!(page.contains("Added"));
}

#[tokio::test]
async fn editing_a_side_pushes_a_fresh_diff() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = write_pair(dir.path(), "# Title\n", "# Title\n\nfirst\n");
    let port = start_diff_server(&old, &new).await;

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("failed to connect");
    let _ = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out on initial message");

    std::fs::write(&new, "# Title\n\nsecond edit\n").unwrap();

    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for update")
        .expect("stream ended")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text message, got {msg:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["type"], "content");
    let html = parsed["html"].as_str().unwrap();
    assert!(
        html.contains("birta-diff") && html.contains("second"),
        "{html}"
    );

    // A reload gets the new diff too, not the one from startup
    assert!(get(port, "/").await.contains("second"));
}

#[tokio::test]
async fn a_variant_switch_re_diffs_the_files() {
    use futures_util::SinkExt;

    let dir = tempfile::tempdir().unwrap();
    let (old, new) = write_pair(dir.path(), "# Title\n", "# Title\n\nadded\n");
    let port = start_diff_server(&old, &new).await;

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("failed to connect");
    let _ = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out on initial message");

    let switch = r#"{"type":"variant_change","variant":"light"}"#;
    ws.send(Message::Text(switch.into())).await.unwrap();

    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for theme update")
        .expect("stream ended")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text message, got {msg:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["type"], "theme_update");
    let html = parsed["html"].as_str().unwrap();
    assert!(
        html.contains("birta-diff") && html.contains("added"),
        "{html}"
    );
}

async fn next_content(
    ws: &mut (impl StreamExt<Item = Result<Message, impl std::fmt::Debug>> + Unpin),
) -> String {
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for update")
        .expect("stream ended")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text message, got {msg:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["type"], "content");
    parsed["html"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn saves_by_rename_and_edits_to_old_are_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = write_pair(dir.path(), "# Title\n", "# Title\n\nfirst\n");
    let port = start_diff_server(&old, &new).await;

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("failed to connect");
    let _ = timeout(Duration::from_secs(5), ws.next()).await.unwrap();

    // Editors that save atomically write a temp file and rename it over the original
    let temp = dir.path().join(".new.md.swp");
    std::fs::write(&temp, "# Title\n\nrenamed in\n").unwrap();
    std::fs::rename(&temp, &new).unwrap();
    let html = next_content(&mut ws).await;
    assert!(html.contains("renamed in"), "{html}");

    std::fs::write(&old, "# Old title\n").unwrap();
    let html = next_content(&mut ws).await;
    assert!(html.contains("Old title"), "{html}");
}

#[tokio::test]
async fn diff_mode_serves_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = write_pair(dir.path(), "a\n", "b\n");
    std::fs::write(dir.path().join("photo.png"), b"fake").unwrap();
    let port = start_diff_server(&old, &new).await;

    for path in ["/local/photo.png", "/view/old.md", "/render/old.md"] {
        let response = get(port, path).await;
        assert!(
            response.starts_with("HTTP/1.0 404"),
            "{path}: {}",
            &response[..20]
        );
    }
}
