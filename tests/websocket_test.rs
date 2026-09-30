use std::io::Write;
use std::time::Duration;

use futures_util::StreamExt;
use tempfile::Builder as TempBuilder;
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
            raw_mode: false,
            keybindings_json: "{}".to_string(),
            variant_explicit: false,
            drafted_by: None,
        };
        birta::server::start(path, listener, opts).await.unwrap();
    });

    // Wait for server to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

#[tokio::test]
async fn ws_sends_initial_html_on_connect() {
    // Its own folder, since the whole folder is served and watched.
    let dir = tempfile::tempdir().unwrap();
    let mut tmpfile = TempBuilder::new()
        .suffix(".md")
        .tempfile_in(dir.path())
        .unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let mut tmpfile = TempBuilder::new()
        .suffix(".md")
        .tempfile_in(dir.path())
        .unwrap();
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

#[tokio::test]
async fn ws_reconnect_sees_latest_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut tmpfile = TempBuilder::new()
        .suffix(".md")
        .tempfile_in(dir.path())
        .unwrap();
    write!(tmpfile, "# Before").unwrap();
    tmpfile.flush().unwrap();

    let port = start_server(&tmpfile).await;
    let url = format!("ws://127.0.0.1:{port}/ws");

    let (mut first, _) = connect_async(&url).await.expect("failed to connect");
    let _ = timeout(Duration::from_secs(5), first.next())
        .await
        .expect("timed out on initial message");

    std::fs::write(tmpfile.path(), "# After change").unwrap();
    let _ = timeout(Duration::from_secs(5), first.next())
        .await
        .expect("timed out waiting for update");

    // A new connection's initial frame comes from the server-side cache.
    let (mut second, _) = connect_async(&url).await.expect("failed to connect");
    let msg = timeout(Duration::from_secs(5), second.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("ws error");

    if let Message::Text(text) = msg {
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("expected JSON");
        assert_eq!(parsed["type"], "content");
        assert!(
            parsed["html"].as_str().unwrap().contains("After change"),
            "reconnect should see the updated content"
        );
    } else {
        panic!("expected text message, got {msg:?}");
    }
}

/// Reads the next text frame as JSON.
async fn next_frame(
    ws: &mut (impl StreamExt<Item = tokio_tungstenite::tungstenite::Result<Message>> + Unpin),
) -> serde_json::Value {
    loop {
        let msg = timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Text(text) = msg {
            return serde_json::from_str(&text).expect("expected JSON");
        }
    }
}

/// `(kind, sp, lines, level, section)` of each stop in a content frame.
fn stops_of(frame: &serde_json::Value) -> Vec<String> {
    assert!(frame.get("owners").is_none(), "{frame}");
    frame["stops"]
        .as_array()
        .unwrap_or_else(|| panic!("no stops: {frame}"))
        .iter()
        .map(|s| {
            format!(
                "{} {} {} {} {}",
                s["kind"], s["sp"], s["lines"], s["level"], s["section"]
            )
        })
        .collect()
}

#[tokio::test]
async fn ws_update_frames_carry_the_stops_of_the_new_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut tmpfile = TempBuilder::new()
        .suffix(".md")
        .tempfile_in(dir.path())
        .unwrap();
    write!(tmpfile, "# Before").unwrap();
    tmpfile.flush().unwrap();

    let port = start_server(&tmpfile).await;
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("failed to connect");

    let initial = next_frame(&mut ws).await;
    assert_eq!(stops_of(&initial), [r#""heading" "1:1-1:8" [1,1] 1 [1,1]"#]);

    std::fs::write(tmpfile.path(), "# After\n\nSome text.\n\n- a\n").unwrap();
    let update = next_frame(&mut ws).await;
    assert_eq!(update["type"], "content");
    assert_eq!(
        stops_of(&update),
        [
            r#""heading" "1:1-1:7" [1,1] 1 [1,5]"#,
            r#""paragraph" "3:1-3:10" [3,3] null null"#,
            r#""list_item" "5:1-5:3" [5,5] null null"#,
        ]
    );
}

#[tokio::test]
async fn a_new_connection_after_an_update_gets_the_new_stops() {
    let dir = tempfile::tempdir().unwrap();
    let mut tmpfile = TempBuilder::new()
        .suffix(".md")
        .tempfile_in(dir.path())
        .unwrap();
    write!(tmpfile, "# Before").unwrap();
    tmpfile.flush().unwrap();

    let port = start_server(&tmpfile).await;
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut first, _) = connect_async(&url).await.expect("failed to connect");
    next_frame(&mut first).await;

    std::fs::write(tmpfile.path(), "# After\n\nSome text.\n\n- a\n").unwrap();
    let update = next_frame(&mut first).await;
    assert_eq!(update["type"], "content");

    let (mut second, _) = connect_async(&url).await.expect("failed to connect");
    let initial = next_frame(&mut second).await;
    assert_eq!(initial["type"], "content");
    assert_eq!(initial["rev"], update["rev"]);
    assert_eq!(
        stops_of(&initial),
        [
            r#""heading" "1:1-1:7" [1,1] 1 [1,5]"#,
            r#""paragraph" "3:1-3:10" [3,3] null null"#,
            r#""list_item" "5:1-5:3" [5,5] null null"#,
        ]
    );
}
