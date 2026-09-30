//! Serving a folder: `/` enters at its picked doc, and the folder stays the served root.

#[allow(dead_code)]
mod common;

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::time::Duration;

use common::{header, http_exchange, http_get, start_server};
use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// `<tmp>/root` is served; the rest of `<tmp>` is not.
struct Site {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Site {
    fn new(files: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("root");
        std::fs::create_dir(&root).unwrap();
        for rel in files {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("# {rel}\n")).unwrap();
        }
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn link(&self, link: &str, target: &str) {
        symlink(target, self.base.join(link)).unwrap();
    }
}

/// Status and `Location` of `GET path`.
async fn get(port: u16, path: &str) -> (u16, Option<String>) {
    let host = format!("127.0.0.1:{port}");
    let response = http_exchange(port, "GET", path, &[("host", &host)]).await;
    let status = response
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("bad response: {response:?}"));
    (status, header(&response, "location").map(str::to_string))
}

async fn status(port: u16, path: &str) -> u16 {
    let host = format!("127.0.0.1:{port}");
    http_get(port, path, &[("host", &host)]).await
}

#[tokio::test]
async fn a_directory_redirects_to_its_nested_entry() {
    let site = Site::new(&["root/notes/a.md", "root/notes/b.md"]);
    let port = start_server(site.root.clone()).await;

    assert_eq!(get(port, "/").await, (307, Some("/view/notes/a.md".into())));
    assert_eq!(status(port, "/view/notes/a.md").await, 200);
    assert_eq!(status(port, "/view/notes/b.md").await, 200);
}

#[tokio::test]
async fn a_directory_prefers_index_over_readme() {
    let site = Site::new(&["root/a.md", "root/README.md", "root/index.md"]);
    let port = start_server(site.root.clone()).await;

    assert_eq!(get(port, "/").await, (307, Some("/view/index.md".into())));
    assert_eq!(status(port, "/view/README.md").await, 200);
}

#[tokio::test]
async fn a_directory_sends_its_entry_under_the_lexical_path() {
    let site = Site::new(&["root/notes/a.md"]);
    let port = start_server(site.root.clone()).await;

    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let frame = timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Text(text) = ws.next().await.unwrap().unwrap() {
                return serde_json::from_str::<serde_json::Value>(&text).unwrap();
            }
        }
    })
    .await
    .expect("no frame");

    assert_eq!(frame["type"], "content", "{frame}");
    assert_eq!(frame["path"], "notes/a.md", "{frame}");
    assert!(
        frame["html"].as_str().unwrap().contains("notes/a.md"),
        "{frame}"
    );
}

#[tokio::test]
async fn a_linked_entry_keeps_the_directory_as_the_root() {
    let site = Site::new(&["root/b.md", "outside/doc.md", "outside/other.md"]);
    site.link("root/index.md", "../outside/doc.md");
    let port = start_server(site.root.clone()).await;

    assert_eq!(get(port, "/").await, (307, Some("/view/index.md".into())));
    assert_eq!(status(port, "/view/index.md").await, 200);
    assert_eq!(status(port, "/view/b.md").await, 200);
    assert_ne!(
        status(port, "/view/other.md").await,
        200,
        "the link's folder must not become the root"
    );
}

#[tokio::test]
async fn a_folder_linked_entry_is_served() {
    let site = Site::new(&["outside/doc.md"]);
    site.link("root/docs", "../outside");
    let port = start_server(site.root.clone()).await;

    assert_eq!(
        get(port, "/").await,
        (307, Some("/view/docs/doc.md".into()))
    );
    assert_eq!(status(port, "/view/docs/doc.md").await, 200);
}

#[tokio::test]
async fn a_directory_redirect_encodes_the_entry() {
    let site = Site::new(&["root/a #1.md"]);
    let port = start_server(site.root.clone()).await;

    let (code, location) = get(port, "/").await;
    assert_eq!(
        (code, location.as_deref()),
        (307, Some("/view/a%20%231.md"))
    );
    assert_eq!(status(port, "/view/a%20%231.md").await, 200);
}

#[tokio::test]
async fn a_file_redirect_encodes_its_name() {
    let site = Site::new(&["root/über #1?.md"]);
    let port = start_server(site.root.join("über #1?.md")).await;

    let (code, location) = get(port, "/").await;
    let location = location.unwrap();
    assert_eq!(
        (code, location.as_str()),
        (307, "/view/%C3%BCber%20%231%3F.md")
    );
    assert_eq!(status(port, &location).await, 200);
}
