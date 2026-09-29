//! Checkbox writes against a real server with its file watcher.

#[allow(dead_code)]
mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::start_server;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `git hash-object --no-filters`, as an oracle independent of the server's hashing.
fn git_rev(bytes: &[u8]) -> String {
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

/// The next JSON frame whose `type` is one of `types`.
async fn next_of(ws: &mut Client, types: &[&str]) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await.expect("socket closed").expect("ws error") {
                Message::Text(text) => {
                    let msg: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if types.iter().any(|t| msg["type"] == *t) {
                        return msg;
                    }
                }
                Message::Close(_) => panic!("socket closed"),
                _ => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {types:?}"))
}

async fn tick(
    ws: &mut Client,
    name: &str,
    cid: &str,
    line: u32,
    rev: &serde_json::Value,
) -> serde_json::Value {
    let frame = serde_json::json!({
        "type": "checkbox", "cid": cid, "path": name, "line": line, "checked": true, "rev": rev,
    });
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
    next_of(ws, &["ack", "nack"]).await
}

#[tokio::test]
async fn content_rev_follows_each_write() {
    rev_follows_each_write("doc.md").await;
}

#[tokio::test]
async fn content_rev_follows_each_write_for_upper_case_extension() {
    rev_follows_each_write("doc.MD").await;
}

async fn rev_follows_each_write(name: &str) {
    let before: &[u8] = b"# Rev\n\n- [ ] a\n- [ ] b\n";
    let middle: &[u8] = b"# Rev\n\n- [x] a\n- [ ] b\n";
    let after: &[u8] = b"# Rev\n\n- [x] a\n- [x] b\n";
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(name);
    std::fs::write(&file, before).unwrap();
    let port = start_server(file.clone()).await;

    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let initial = next_of(&mut ws, &["content"]).await;
    assert_eq!(initial["rev"], git_rev(before), "initial content frame");

    let reply = tick(&mut ws, name, "c1", 3, &initial["rev"]).await;
    assert_eq!(reply["type"], "ack", "{reply}");
    assert_eq!(std::fs::read(&file).unwrap(), middle);

    // The watcher's update carries the rev of the bytes it rendered, so a second tick works.
    let update = next_of(&mut ws, &["content"]).await;
    assert_eq!(
        update["rev"],
        git_rev(middle),
        "content frame after the write"
    );

    let reply = tick(&mut ws, name, "c2", 4, &update["rev"]).await;
    assert_eq!(reply["type"], "ack", "{reply}");
    assert_eq!(std::fs::read(&file).unwrap(), after);
}
