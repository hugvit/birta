//! File watching through symlinked folders, file links and removals, against a real server.

#[allow(dead_code)]
mod common;

use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{http_get, start_server};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `<tmp>/r/index.md` is the entry doc, so `<tmp>/r` is served; the rest of `<tmp>` is not.
struct Site {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Site {
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let site = Self { _tmp: tmp, base };
        site.write("r/index.md", "# Index\n");
        for (rel, text) in files {
            site.write(rel, text);
        }
        site
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.at(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Creates `<base>/<link>` pointing at `target` (relative to the link).
    fn link(&self, link: &str, target: impl AsRef<Path>) {
        symlink(target, self.at(link)).unwrap();
    }

    /// Starts the server and connects, consuming the initial `content` frame.
    async fn serve(&self) -> (u16, Client) {
        let port = start_server(self.at("r/index.md")).await;
        let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        let first = next_frame(&mut ws, Duration::from_secs(10)).await;
        assert_eq!(first.map(|f| f["type"].clone()), Some("content".into()));
        (port, ws)
    }
}

async fn next_frame(ws: &mut Client, wait: Duration) -> Option<serde_json::Value> {
    tokio::time::timeout(wait, async {
        loop {
            match ws.next().await.expect("socket closed").expect("ws error") {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Close(_) => panic!("socket closed"),
                _ => {}
            }
        }
    })
    .await
    .ok()
}

/// Reads frames, in any order, until `done` holds for the frames seen or 10 s pass.
/// Returns every frame seen.
async fn frames_until(
    ws: &mut Client,
    done: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let started = Instant::now();
    let mut seen = Vec::new();
    while !done(&seen) {
        let Some(left) = Duration::from_secs(10).checked_sub(started.elapsed()) else {
            break;
        };
        match next_frame(ws, left).await {
            Some(frame) => seen.push(frame),
            None => break,
        }
    }
    seen
}

fn is(frame: &serde_json::Value, kind: &str, path: &str) -> bool {
    frame["type"] == kind && frame["path"] == path
}

fn has(seen: &[serde_json::Value], kind: &str, path: &str) -> bool {
    seen.iter().any(|f| is(f, kind, path))
}

fn summary(seen: &[serde_json::Value]) -> Vec<String> {
    seen.iter()
        .map(|f| format!("{} {}", f["type"], f["path"]))
        .collect()
}

/// Writes the entry doc and waits for its `content` frame, so every event queued before
/// it has been delivered. Returns the frames seen on the way.
async fn after_sentinel(site: &Site, ws: &mut Client) -> Vec<serde_json::Value> {
    site.write("r/index.md", "# Sentinel\n");
    let seen = frames_until(ws, |seen| has(seen, "content", "index.md")).await;
    assert!(
        has(&seen, "content", "index.md"),
        "no sentinel: {:?}",
        summary(&seen)
    );
    seen
}

/// Rewrites `rel` (fresh bytes each round) until `done` holds or 10 s pass, for waits that
/// race a watch being added. Returns every frame seen.
async fn edit_until(
    site: &Site,
    ws: &mut Client,
    rel: &str,
    done: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut n = 0;
    while !done(&seen) && started.elapsed() < Duration::from_secs(10) {
        site.write(rel, &format!("# Edit {n}\n"));
        n += 1;
        let round = Instant::now();
        while !done(&seen) && round.elapsed() < Duration::from_secs(1) {
            match next_frame(ws, Duration::from_millis(200)).await {
                Some(frame) => seen.push(frame),
                None => continue,
            }
        }
    }
    seen
}

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

async fn tick(ws: &mut Client, path: &str, cid: &str, line: u32, rev: &serde_json::Value) {
    let frame = serde_json::json!({
        "type": "checkbox", "cid": cid, "path": path, "line": line, "checked": true, "rev": rev,
    });
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
}

fn reply_to<'a>(seen: &'a [serde_json::Value], cid: &str) -> Option<&'a serde_json::Value> {
    seen.iter()
        .find(|f| matches!(f["type"].as_str(), Some("ack" | "nack")) && f["cid"] == cid)
}

#[tokio::test]
async fn linked_folder_change_sends_content_under_the_link() {
    let site = Site::new(&[("vault/plan.md", "# Plan\n")]);
    site.link("r/.context", "../vault");
    let (_, mut ws) = site.serve().await;

    site.write("vault/plan.md", "# Plan edited\n");
    let seen = frames_until(&mut ws, |seen| has(seen, "content", ".context/plan.md")).await;

    let frame = seen.iter().find(|f| is(f, "content", ".context/plan.md"));
    let frame = frame.unwrap_or_else(|| panic!("no update: {:?}", summary(&seen)));
    assert!(frame["html"].as_str().unwrap().contains("Plan edited"));
    assert_eq!(frame["rev"], git_rev(b"# Plan edited\n"));
}

#[tokio::test]
async fn link_created_after_start_is_watched() {
    let site = Site::new(&[("outside/x.md", "# X\n")]);
    let (_, mut ws) = site.serve().await;

    site.link("r/late", "../outside");
    // Registration races the first edit, so keep editing until one lands.
    let seen = edit_until(&site, &mut ws, "outside/x.md", |seen| {
        has(seen, "content", "late/x.md")
    })
    .await;

    assert!(has(&seen, "content", "late/x.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn two_links_to_one_target_update_both_aliases() {
    let site = Site::new(&[("outside/x.md", "# X\n")]);
    site.link("r/one", "../outside");
    site.link("r/two", "../outside");
    let (_, mut ws) = site.serve().await;

    site.write("outside/x.md", "# X edited\n");
    let seen = frames_until(&mut ws, |seen| {
        has(seen, "content", "one/x.md") && has(seen, "content", "two/x.md")
    })
    .await;

    assert!(has(&seen, "content", "one/x.md"), "{:?}", summary(&seen));
    assert!(has(&seen, "content", "two/x.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn file_link_out_of_root_is_served_and_updates_its_alias() {
    let site = Site::new(&[
        ("outside/doc.md", "# Doc\n"),
        ("outside/other.md", "# Other\n"),
    ]);
    site.link("r/ext.md", "../outside/doc.md");
    let (port, mut ws) = site.serve().await;
    let host = format!("127.0.0.1:{port}");
    assert_eq!(
        http_get(port, "/view/ext.md", &[("host", &host)]).await,
        200
    );

    site.write("outside/doc.md", "# Doc edited\n");
    let seen = frames_until(&mut ws, |seen| has(seen, "content", "ext.md")).await;
    assert!(has(&seen, "content", "ext.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn file_link_sibling_edit_never_surfaces() {
    let site = Site::new(&[
        ("outside/doc.md", "# Doc\n"),
        ("outside/other.md", "# Other\n"),
    ]);
    site.link("r/ext.md", "../outside/doc.md");
    let (_, mut ws) = site.serve().await;

    // Its folder is not served, so a sibling edit must not surface under any name. The
    // sentinel is the linked file itself, in the same folder, so it can't overtake.
    site.write("outside/other.md", "# Other edited\n");
    let seen = edit_until(&site, &mut ws, "outside/doc.md", |seen| {
        has(seen, "content", "ext.md")
    })
    .await;

    assert!(has(&seen, "content", "ext.md"), "{:?}", summary(&seen));
    let leaked: Vec<_> = seen
        .iter()
        .filter(|f| f["path"].as_str().is_some_and(|p| p.contains("other")))
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
}

#[tokio::test]
async fn deleting_a_doc_sends_gone() {
    let site = Site::new(&[("r/other.md", "# Other\n")]);
    let (_, mut ws) = site.serve().await;

    std::fs::remove_file(site.at("r/other.md")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", "other.md")).await;

    assert!(has(&seen, "gone", "other.md"), "{:?}", summary(&seen));
    assert!(!has(&seen, "content", "other.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn deleting_the_entry_doc_sends_gone() {
    let site = Site::new(&[]);
    let (_, mut ws) = site.serve().await;

    std::fs::remove_file(site.at("r/index.md")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", "index.md")).await;

    assert!(has(&seen, "gone", "index.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn renaming_a_doc_away_sends_gone() {
    let site = Site::new(&[("r/a.md", "# A\n"), ("r/b.md", "# B\n")]);
    let (_, mut ws) = site.serve().await;

    std::fs::rename(site.at("r/a.md"), site.at("r/a.txt")).unwrap();
    std::fs::rename(site.at("r/b.md"), site.at("elsewhere.md")).unwrap();
    let seen = frames_until(&mut ws, |seen| {
        has(seen, "gone", "a.md") && has(seen, "gone", "b.md")
    })
    .await;

    assert!(has(&seen, "gone", "a.md"), "{:?}", summary(&seen));
    assert!(has(&seen, "gone", "b.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn deleting_in_a_linked_folder_sends_gone_under_the_link() {
    let site = Site::new(&[("vault/plan.md", "# Plan\n")]);
    site.link("r/.context", "../vault");
    let (_, mut ws) = site.serve().await;

    std::fs::remove_file(site.at("vault/plan.md")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", ".context/plan.md")).await;

    assert!(
        has(&seen, "gone", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );
}

#[tokio::test]
async fn atomic_replace_sends_content_not_gone() {
    let site = Site::new(&[("r/doc.md", "# Doc\n"), ("vault/plan.md", "# Plan\n")]);
    site.link("r/.context", "../vault");
    let (_, mut ws) = site.serve().await;

    // How editors save: write a temp file beside the doc, then rename it over.
    site.write("r/.doc.md.tmp", "# Doc saved\n");
    std::fs::rename(site.at("r/.doc.md.tmp"), site.at("r/doc.md")).unwrap();
    site.write("vault/.plan.md.tmp", "# Plan saved\n");
    std::fs::rename(site.at("vault/.plan.md.tmp"), site.at("vault/plan.md")).unwrap();
    let mut seen = frames_until(&mut ws, |seen| {
        has(seen, "content", "doc.md") && has(seen, "content", ".context/plan.md")
    })
    .await;
    seen.extend(after_sentinel(&site, &mut ws).await);

    assert!(has(&seen, "content", "doc.md"), "{:?}", summary(&seen));
    assert!(
        has(&seen, "content", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );
    let gone: Vec<_> = seen.iter().filter(|f| f["type"] == "gone").collect();
    assert!(gone.is_empty(), "{gone:?}");
}

#[tokio::test]
async fn edits_in_skipped_folders_send_nothing() {
    let site = Site::new(&[
        ("r/node_modules/pkg/skipped.md", "# X\n"),
        ("r/.git/skipped.md", "# X\n"),
    ]);
    let (_, mut ws) = site.serve().await;

    site.write("r/node_modules/pkg/skipped.md", "# X edited\n");
    site.write("r/.git/skipped.md", "# X edited\n");
    let seen = after_sentinel(&site, &mut ws).await;

    let leaked: Vec<_> = seen
        .iter()
        .filter(|f| f["path"].as_str().is_some_and(|p| p.contains("skipped")))
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
}

#[tokio::test]
async fn ticks_in_a_linked_folder_live_reload() {
    let before: &[u8] = b"- [ ] a\n- [ ] b\n";
    let after: &[u8] = b"- [x] a\n- [x] b\n";
    let site = Site::new(&[("vault/plan.md", std::str::from_utf8(before).unwrap())]);
    site.link("r/.context", "../vault");
    let (_, mut ws) = site.serve().await;

    tick(
        &mut ws,
        ".context/plan.md",
        "c1",
        1,
        &git_rev(before).into(),
    )
    .await;
    let seen = frames_until(&mut ws, |seen| {
        reply_to(seen, "c1").is_some() && has(seen, "content", ".context/plan.md")
    })
    .await;
    let ack = reply_to(&seen, "c1").unwrap_or_else(|| panic!("no reply: {:?}", summary(&seen)));
    assert_eq!(ack["type"], "ack", "{ack}");
    let update = seen.iter().find(|f| is(f, "content", ".context/plan.md"));
    let update = update.unwrap_or_else(|| panic!("no live reload: {:?}", summary(&seen)));
    assert_eq!(
        update["rev"], ack["rev"],
        "the reload renders the bytes the tick wrote"
    );

    tick(&mut ws, ".context/plan.md", "c2", 2, &update["rev"]).await;
    let mut seen2 = frames_until(&mut ws, |seen| reply_to(seen, "c2").is_some()).await;
    let ack = reply_to(&seen2, "c2").unwrap_or_else(|| panic!("no reply: {:?}", summary(&seen2)));
    assert_eq!(ack["type"], "ack", "{ack}");
    assert_eq!(std::fs::read(site.at("vault/plan.md")).unwrap(), after);

    seen2.extend(seen);
    let gone: Vec<_> = seen2.iter().filter(|f| f["type"] == "gone").collect();
    assert!(gone.is_empty(), "{gone:?}");
}

#[tokio::test]
async fn removing_a_folder_link_sends_gone_and_unserves_it() {
    let site = Site::new(&[("vault/plan.md", "# Plan\n")]);
    site.link("r/.context", "../vault");
    // A second alias keeps the folder watched, so its frames order the negative check.
    site.link("r/keep", "../vault");
    let (port, mut ws) = site.serve().await;

    std::fs::remove_file(site.at("r/.context")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", ".context/plan.md")).await;
    assert!(
        has(&seen, "gone", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );

    let seen = edit_until(&site, &mut ws, "vault/plan.md", |seen| {
        has(seen, "content", "keep/plan.md")
    })
    .await;
    assert!(
        has(&seen, "content", "keep/plan.md"),
        "{:?}",
        summary(&seen)
    );
    assert!(
        !has(&seen, "content", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );
    let host = format!("127.0.0.1:{port}");
    assert_eq!(
        http_get(port, "/view/.context/plan.md", &[("host", &host)]).await,
        404
    );
}

#[tokio::test]
async fn retargeted_link_follows_the_new_target() {
    let site = Site::new(&[("vault/plan.md", "# Plan\n"), ("v2/plan.md", "# V2\n")]);
    site.link("r/.context", "../vault");
    site.link("r/keep", "../vault");
    let (_, mut ws) = site.serve().await;

    // Swapped in one rename, the way `ln -sfn` behaves on Linux.
    site.link("r/.context.new", "../v2");
    std::fs::rename(site.at("r/.context.new"), site.at("r/.context")).unwrap();
    let seen = edit_until(&site, &mut ws, "v2/plan.md", |seen| {
        seen.iter().any(|f| {
            is(f, "content", ".context/plan.md")
                && f["html"].as_str().is_some_and(|h| h.contains("Edit"))
        })
    })
    .await;
    assert!(
        has(&seen, "content", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );

    let seen = edit_until(&site, &mut ws, "vault/plan.md", |seen| {
        has(seen, "content", "keep/plan.md")
    })
    .await;
    assert!(
        has(&seen, "content", "keep/plan.md"),
        "{:?}",
        summary(&seen)
    );
    assert!(
        !has(&seen, "content", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );
}

#[tokio::test]
async fn review_log_append_sends_no_content() {
    let site = Site::new(&[("r/index.md.birta.jsonl", "{}\n")]);
    let (_, mut ws) = site.serve().await;

    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(site.at("r/index.md.birta.jsonl"))
        .unwrap();
    log.write_all(b"{}\n").unwrap();
    drop(log);
    let seen = after_sentinel(&site, &mut ws).await;

    let leaked: Vec<_> = seen
        .iter()
        .filter(|f| f["path"].as_str().is_some_and(|p| p.ends_with(".jsonl")))
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
}

#[tokio::test]
async fn renaming_a_folder_away_sends_gone_for_its_docs() {
    let site = Site::new(&[("r/sub/a.md", "# A\n")]);
    let (_, mut ws) = site.serve().await;

    std::fs::rename(site.at("r/sub"), site.at("away")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", "sub/a.md")).await;

    assert!(has(&seen, "gone", "sub/a.md"), "{:?}", summary(&seen));
}

#[tokio::test]
async fn removed_folder_link_no_longer_makes_a_file_link_writable() {
    let task: &[u8] = b"- [ ] a\n";
    let site = Site::new(&[
        ("vault/plan.md", std::str::from_utf8(task).unwrap()),
        ("other/x.md", "# X\n"),
    ]);
    site.link("r/.context", "../vault");
    site.link("r/ext.md", "../vault/plan.md");
    let (_, mut ws) = site.serve().await;

    std::fs::remove_file(site.at("r/.context")).unwrap();
    let seen = frames_until(&mut ws, |seen| has(seen, "gone", ".context/plan.md")).await;
    assert!(
        has(&seen, "gone", ".context/plan.md"),
        "{:?}",
        summary(&seen)
    );
    // Link changes apply in order, so once this new link is live the removal has landed.
    site.link("r/probe", "../other");
    let seen = edit_until(&site, &mut ws, "other/x.md", |seen| {
        has(seen, "content", "probe/x.md")
    })
    .await;
    assert!(has(&seen, "content", "probe/x.md"), "{:?}", summary(&seen));

    tick(&mut ws, "ext.md", "c1", 1, &git_rev(task).into()).await;
    let seen = frames_until(&mut ws, |seen| reply_to(seen, "c1").is_some()).await;

    let reply = reply_to(&seen, "c1").unwrap_or_else(|| panic!("no reply: {:?}", summary(&seen)));
    assert_eq!(reply["type"], "nack", "{reply}");
    assert_eq!(reply["reason"], "read_only", "{reply}");
    assert_eq!(std::fs::read(site.at("vault/plan.md")).unwrap(), task);
}

#[tokio::test]
async fn reading_a_doc_sends_nothing() {
    let site = Site::new(&[("r/other.md", "# Other\n")]);
    let (port, mut ws) = site.serve().await;

    // inotify reports opens; a read must not look like an edit, or each render re-reads.
    std::fs::read(site.at("r/other.md")).unwrap();
    let host = format!("127.0.0.1:{port}");
    assert_eq!(
        http_get(port, "/view/other.md", &[("host", &host)]).await,
        200
    );
    let seen = after_sentinel(&site, &mut ws).await;

    assert!(!has(&seen, "content", "other.md"), "{:?}", summary(&seen));
    // Rendering the sentinel reads it too; that read must not come back as another render.
    let echo = next_frame(&mut ws, Duration::from_secs(1)).await;
    assert!(echo.is_none(), "{echo:?}");
}
