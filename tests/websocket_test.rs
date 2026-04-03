use std::io::Write;
use std::time::Duration;

use futures_util::StreamExt;
use tempfile::NamedTempFile;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server(tmpfile: &NamedTempFile) -> u16 {
    let path = tmpfile.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let theme = birta::theme::ResolvedTheme {
            name: "github".to_string(),
            variants: birta::theme::ThemeVariants::Both {
                light: Box::new(birta::theme::VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
                dark: Box::new(birta::theme::VariantData {
                    css_vars: String::new(),
                    syntax: None,
                }),
            },
            active_variant: birta::theme::Variant::Dark,
        };
        let opts = birta::server::ServerOptions {
            port: 0,
            no_open: true,
            custom_css: None,
            font_css: None,
            theme,
            enable_swap: false,
            enable_toggle: true,
            show_header: true,
            reading_mode: false,
            keybindings_json: "{}".to_string(),
        };
        birta::server::start(path, listener, opts).await.unwrap();
    });

    // Wait for server to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

#[tokio::test]
async fn ws_sends_initial_html_on_connect() {
    let mut tmpfile = NamedTempFile::new().unwrap();
    write!(tmpfile, "# Initial").unwrap();
    tmpfile.flush().unwrap();

    let port = start_server(&tmpfile).await;

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("failed to connect");

    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("ws error");

    if let Message::Text(text) = msg {
        // Messages are now JSON
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");
        assert_eq!(parsed["type"], "content");
        assert!(
            parsed["html"].as_str().unwrap().contains("Initial"),
            "initial WS message should contain rendered content"
        );
    } else {
        panic!("expected text message, got {msg:?}");
    }
}

#[tokio::test]
async fn ws_sends_update_on_file_change() {
    let mut tmpfile = NamedTempFile::new().unwrap();
    write!(tmpfile, "# Before").unwrap();
    tmpfile.flush().unwrap();

    let port = start_server(&tmpfile).await;

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("failed to connect");

    // Consume initial message
    let _ = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out on initial message");

    // Modify the file
    std::fs::write(tmpfile.path(), "# After change").unwrap();

    // Wait for the update
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for update")
        .expect("stream ended")
        .expect("ws error");

    if let Message::Text(text) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");
        assert_eq!(parsed["type"], "content");
        assert!(
            parsed["html"].as_str().unwrap().contains("After change"),
            "update should contain new content"
        );
    } else {
        panic!("expected text message, got {msg:?}");
    }
}
