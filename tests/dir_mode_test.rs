//! Serving a folder: `/` enters at its picked doc, and the folder stays the served root.

#[allow(dead_code)]
mod common;

use std::os::unix::fs::{PermissionsExt, symlink};
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

// --- dir_state: the folder's docs with activity, over a served `birta DIR` ---

mod support {
    pub mod scenario;
    pub mod served;
}
use support::scenario::Scenario;
use support::served::{self, GATE_ON, Ws};

const A_MD: &str = "# A\n\nSome text.\n";

/// A served folder: `index.md` (the entry), `a.md` with a review file holding one
/// draft, and `c.md` with none.
fn review_site() -> served::Site {
    let site = served::Site::new();
    site.write("index.md", "# Index\n");
    site.write("a.md", A_MD);
    let mut s = Scenario::new(A_MD);
    s.note("One draft.");
    site.write("a.md.birta.jsonl", &s.log());
    site.write("c.md", "# C\n");
    site
}

/// The listing entry reachable at `path`.
fn entry<'a>(state: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    state["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["paths"].as_array().unwrap().iter().any(|p| p == path))
}

/// The first `dir_state` whose listing passes `pred`.
async fn dir_state_where(
    ws: &mut Ws,
    what: &str,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    served::until(ws, what, |f| f["type"] == "dir_state" && pred(f))
        .await
        .1
}

/// The hello's listing when it already passes `pred`, else the first push that does:
/// the startup scan summarizes in the background.
async fn listing_where(
    ws: &mut Ws,
    frames: &[serde_json::Value],
    what: &str,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let hello = hello_dir_state(frames);
    if pred(&hello) {
        return hello;
    }
    dir_state_where(ws, what, pred).await
}

fn hello_dir_state(frames: &[serde_json::Value]) -> serde_json::Value {
    // The hello's own three frames; a push may land around them
    let i = frames
        .iter()
        .position(|f| f["type"] == "content")
        .unwrap_or_else(|| panic!("no content: {frames:#?}"));
    match (frames.get(i + 1), frames.get(i + 2)) {
        (Some(r), Some(d)) if r["type"] == "review_state" && d["type"] == "dir_state" => d.clone(),
        _ => panic!("no review_state then dir_state after the hello's content: {frames:#?}"),
    }
}

#[tokio::test]
async fn hello_lists_only_the_docs_with_activity() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let state = listing_where(&mut ws, &frames, "a.md summarized", |s| {
        entry(s, "a.md").is_some_and(|a| !a["summary"].is_null())
    })
    .await;
    let a = entry(&state, "a.md").expect("a doc with a review file is listed");
    assert_eq!(a["summary"]["counts"]["drafts"], 1, "{a}");
    assert_eq!(a["news"], false);
    assert!(entry(&state, "c.md").is_none(), "{state}");
    assert!(entry(&state, "index.md").is_none(), "{state}");
    assert_eq!(state["folder"], serde_json::Value::Null);
    assert_eq!(state["truncated"], false);
    assert_eq!(state["totals"]["drafts"], 1);
}

/// Claude's replies are news as much as its changes are.
#[tokio::test]
async fn a_doc_whose_only_news_is_an_unchecked_reply_is_news() {
    let site = review_site();
    site.write("b.md", "# B\n");
    let mut s = Scenario::new("# B\n");
    let x = s.note("is this needed?");
    s.submit();
    let turn = s.claim("claude");
    s.agent_reply("claude", x, "answer", "yes, the index links it");
    s.yield_turn("claude", turn);
    site.write("b.md.birta.jsonl", &s.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let state = listing_where(&mut ws, &frames, "b.md summarized", |s| {
        entry(s, "b.md").is_some_and(|b| !b["summary"].is_null())
    })
    .await;
    let b = entry(&state, "b.md").unwrap();
    assert_eq!(b["summary"]["counts"]["unseen"], 0, "setup: {b}");
    assert_eq!(b["news"], true, "{b}");
    assert_eq!(b["summary"]["counts"]["to_check"], 1, "{b}");
}

#[tokio::test]
async fn an_unviewed_write_is_news_until_a_hello_views_it() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("b.md", "# B\n");
    let state = dir_state_where(&mut ws, "b.md as news", |s| {
        entry(s, "b.md").is_some_and(|b| b["news"] == true)
    })
    .await;
    assert_eq!(
        entry(&state, "b.md").unwrap()["summary"],
        serde_json::Value::Null
    );
    let (_other, _) = served.viewing("b.md").await;
    dir_state_where(&mut ws, "b.md no longer news", |s| {
        entry(s, "b.md").is_none_or(|b| b["news"] == false)
    })
    .await;
}

#[tokio::test]
async fn a_write_to_a_viewed_doc_is_never_news() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (_viewer, _) = served.viewing("c.md").await;
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("c.md", "# C, edited\n");
    // Written after c.md, so its news arriving means c.md's write was handled
    tokio::time::sleep(Duration::from_millis(400)).await;
    site.write("d.md", "# D\n");
    let (seen, state) = served::until(&mut ws, "d.md as news", |f| {
        f["type"] == "dir_state" && entry(f, "d.md").is_some()
    })
    .await;
    for state in seen.iter().chain([&state]) {
        if state["type"] == "dir_state" {
            assert!(
                entry(state, "c.md").is_none_or(|c| c["news"] == false),
                "{state}"
            );
        }
    }
}

#[tokio::test]
async fn a_review_file_is_summarized_again_only_when_its_signature_changes() {
    let site = review_site();
    let log = site.path("a.md.birta.jsonl");
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    let set_mode = |mode: u32| {
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // Unreadable but the same ino, length and mtime: the cached summary stands
    set_mode(0o000);
    site.write("d.md", "# D\n");
    let state = dir_state_where(&mut ws, "d.md listed", |s| entry(s, "d.md").is_some()).await;
    let summary = entry(&state, "a.md").unwrap()["summary"].clone();
    set_mode(0o644);
    assert_eq!(summary["counts"]["drafts"], 1, "{state}");
    // A new mtime makes it read again, and now it can't be
    let later = std::time::SystemTime::now() + Duration::from_secs(10);
    std::fs::File::options()
        .write(true)
        .open(&log)
        .unwrap()
        .set_modified(later)
        .unwrap();
    set_mode(0o000);
    site.write("e.md", "# E\n");
    let state = dir_state_where(&mut ws, "e.md listed", |s| entry(s, "e.md").is_some()).await;
    set_mode(0o644);
    assert_eq!(
        entry(&state, "a.md").map(|a| a["summary"].clone()),
        Some(serde_json::Value::Null),
        "{state}"
    );
}

#[tokio::test]
async fn a_deleted_doc_leaves_the_listing() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("b.md", "# B\n");
    dir_state_where(&mut ws, "b.md listed", |s| entry(s, "b.md").is_some()).await;
    std::fs::remove_file(site.path("b.md")).unwrap();
    dir_state_where(&mut ws, "b.md gone", |s| entry(s, "b.md").is_none()).await;
}

#[tokio::test]
async fn a_linked_doc_is_listed_once_under_every_path() {
    let site = review_site();
    symlink("a.md", site.path("alias.md")).unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let state = listing_where(&mut ws, &frames, "a.md listed", |s| {
        entry(s, "a.md").is_some()
    })
    .await;
    let a = entry(&state, "a.md").expect("a.md listed");
    let mut paths: Vec<&str> = a["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["a.md", "alias.md"]);
    let listed = state["files"].as_array().unwrap().len();
    assert_eq!(listed, 1, "{state}");
}

#[tokio::test]
async fn an_unchanged_listing_is_not_pushed_again() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("b.md", "# B\n");
    dir_state_where(&mut ws, "b.md as news", |s| entry(s, "b.md").is_some()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    served::drain(&mut ws, Duration::from_millis(100)).await;
    site.write("b.md", "# B\n");
    let again: Vec<serde_json::Value> = served::drain(&mut ws, Duration::from_millis(1_000))
        .await
        .into_iter()
        .filter(|f| f["type"] == "dir_state")
        .collect();
    assert!(again.is_empty(), "{again:#?}");
}

#[tokio::test]
async fn the_folder_review_file_is_the_folder_summary() {
    let site = review_site();
    let mut folder = Scenario::folder("root");
    folder.comment(
        serde_json::json!("dir"),
        None,
        Some("A note on the folder."),
    );
    site.write(".birta.jsonl", &folder.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let state = listing_where(&mut ws, &frames, "the folder summarized", |s| {
        !s["folder"].is_null()
    })
    .await;
    assert_eq!(state["folder"]["summary"]["counts"]["drafts"], 1, "{state}");
    assert!(entry(&state, ".birta.jsonl").is_none(), "{state}");
}

#[tokio::test]
async fn review_state_goes_only_to_the_viewers_of_its_doc() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut other, _) = served.viewing("c.md").await;
    let (mut ws, _) = served.viewing("a.md").await;
    let note = serde_json::json!({
        "type": "op", "cid": "v1", "path": "a.md", "rev": birta::rev::Rev::of(A_MD.as_bytes()),
        "op": "comment", "at": {"k": "doc"}, "body": "for a.md's viewers",
    });
    let n = served::acked(&mut ws, note).await["n"].as_u64();
    served::until(&mut ws, "a.md's new state", |f| {
        f["type"] == "review_state"
            && f["path"] == "a.md"
            && f["state"]["log"]["lines"].as_u64() >= n
    })
    .await;
    let leaked: Vec<serde_json::Value> = served::drain(&mut other, Duration::from_millis(1_000))
        .await
        .into_iter()
        .filter(|f| f["type"] == "review_state")
        .collect();
    assert!(leaked.is_empty(), "{leaked:#?}");
}

#[tokio::test]
async fn a_hello_clears_only_its_own_docs_news() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("b.md", "# B\n");
    site.write("d.md", "# D\n");
    dir_state_where(&mut ws, "b.md and d.md as news", |s| {
        ["b.md", "d.md"]
            .iter()
            .all(|p| entry(s, p).is_some_and(|e| e["news"] == true))
    })
    .await;
    let mut refused = served.connect().await;
    served::hello(&mut refused, "missing.md").await;
    let (_b, _) = served.viewing("b.md").await;
    let state = dir_state_where(&mut ws, "b.md seen", |s| {
        entry(s, "b.md").is_none_or(|b| b["news"] == false)
    })
    .await;
    assert_eq!(
        entry(&state, "d.md").map(|d| d["news"].clone()),
        Some(serde_json::json!(true)),
        "{state}"
    );
}

#[tokio::test]
async fn a_doc_whose_viewer_left_is_news_again() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    {
        let (_viewer, _) = served.viewing("c.md").await;
    }
    // Let the server see the close
    tokio::time::sleep(Duration::from_millis(300)).await;
    site.write("c.md", "# C, edited\n");
    dir_state_where(&mut ws, "c.md as news", |s| {
        entry(s, "c.md").is_some_and(|c| c["news"] == true)
    })
    .await;
}

#[tokio::test]
async fn a_burst_of_writes_is_throttled_and_its_last_write_is_listed() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    let start = std::time::Instant::now();
    for i in 0..30 {
        site.write(&format!("burst{i:02}.md"), "# x\n");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let (seen, _) = served::until(&mut ws, "every burst file listed", |f| {
        f["type"] == "dir_state" && (0..30).all(|i| entry(f, &format!("burst{i:02}.md")).is_some())
    })
    .await;
    let pushes = seen.iter().filter(|f| f["type"] == "dir_state").count() + 1;
    let bound = start.elapsed().as_millis() as usize / 250 + 3;
    assert!(pushes <= bound, "{pushes} pushes in {:?}", start.elapsed());
}

#[tokio::test]
async fn watcher_state_goes_only_to_viewers_and_unviewed_writes_are_recorded() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut other, _) = served.viewing("c.md").await;
    let (mut index, _) = served.viewing("index.md").await;
    let changed = format!("{A_MD}Nobody is looking.\n");
    site.write("a.md", &changed);
    let log = site.path("a.md.birta.jsonl");
    let to_rev = birta::rev::Rev::of(changed.as_bytes());
    // Nobody views a.md, and the write is still recorded
    served::eventually("the unviewed write is recorded", || {
        served::ops_named(&log, "external")
            .iter()
            .any(|l| l["to_rev"] == to_rev.as_str())
    })
    .await;
    let (mut ws, _) = served.viewing("a.md").await;
    let appended = format!(
        "{}{}\n",
        std::fs::read_to_string(&log).unwrap(),
        serde_json::json!({"ts": 1, "by": {"type": "agent", "name": "claude"}, "op": "claim", "round": 99})
    );
    std::fs::write(&log, appended).unwrap();
    served::until(&mut ws, "a.md's state after the agent's line", |f| {
        f["type"] == "review_state" && f["path"] == "a.md"
    })
    .await;
    for socket in [&mut other, &mut index] {
        let leaked: Vec<_> = served::drain(socket, Duration::from_millis(800))
            .await
            .into_iter()
            .filter(|f| f["type"] == "review_state")
            .collect();
        assert!(leaked.is_empty(), "{leaked:#?}");
    }
}

#[tokio::test]
async fn the_first_note_on_a_doc_lists_it_at_once() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut index, _) = served.viewing("index.md").await;
    let (mut ws, _) = served.viewing("c.md").await;
    let note = serde_json::json!({"type": "op", "cid": "f1", "path": "c.md",
        "rev": birta::rev::Rev::of(b"# C\n"), "op": "comment", "at": {"k": "doc"}, "body": "first"});
    served::acked(&mut ws, note).await;
    // The file may be listed a moment before the note lands in it
    dir_state_where(&mut index, "c.md listed with its note", |s| {
        entry(s, "c.md").is_some_and(|c| c["summary"]["counts"]["drafts"] == 1)
    })
    .await;
}

#[tokio::test]
async fn every_review_file_is_listed_past_the_startup_summary_cap() {
    let site = served::Site::new();
    site.write("index.md", "# Index\n");
    for i in 0..201 {
        site.write(&format!("d{i:03}.md"), A_MD);
        let mut s = Scenario::new(A_MD);
        s.note("x");
        site.write(&format!("d{i:03}.md.birta.jsonl"), &s.log());
    }
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let full = |f: &serde_json::Value| f["files"].as_array().is_some_and(|a| a.len() == 201);
    if !frames.iter().any(|f| f["type"] == "dir_state" && full(f)) {
        dir_state_where(&mut ws, "all 201 listed", full).await;
    }
}

#[tokio::test]
async fn hand_back_all_sends_every_doc_with_drafts_in_one_batch() {
    let site = review_site();
    site.write("b.md", A_MD);
    let mut b = Scenario::new(A_MD);
    b.note("Another draft.");
    site.write("b.md.birta.jsonl", &b.log());
    site.write("c.md.birta.jsonl", &Scenario::new("# C\n").log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("a.md").await;
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "a.md",
        "rev": birta::rev::Rev::of(A_MD.as_bytes()), "op": "hand_back", "scope": "all"});
    served::acked(&mut ws, hand_back).await;
    let submits =
        |doc: &str| served::ops_named(&site.path(&format!("{doc}.birta.jsonl")), "submit");
    let (a, b) = (submits("a.md"), submits("b.md"));
    assert_eq!((a.len(), b.len()), (1, 1), "{a:#?} {b:#?}");
    assert!(a[0]["batch"].is_u64(), "{a:#?}");
    assert_eq!(a[0]["batch"], b[0]["batch"]);
    assert!(
        submits("c.md").is_empty(),
        "a doc with no drafts is not sent"
    );
}

#[tokio::test]
async fn a_doc_deleted_while_its_review_file_remains_is_not_listed() {
    let site = review_site();
    std::fs::remove_file(site.path("a.md")).unwrap();
    // A doc the scan does list, so its listing marks the scan as done
    site.write("b.md", A_MD);
    let mut b = Scenario::new(A_MD);
    b.note("Kept.");
    site.write("b.md.birta.jsonl", &b.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("index.md").await;
    let state = listing_where(&mut ws, &frames, "b.md summarized", |s| {
        entry(s, "b.md").is_some_and(|b| !b["summary"].is_null())
    })
    .await;
    assert!(entry(&state, "a.md").is_none(), "{state}");
}

#[tokio::test]
async fn an_alias_viewer_gets_the_state_under_its_own_path() {
    let site = review_site();
    symlink("a.md", site.path("alias.md")).unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut alias, frames) = served.viewing("alias.md").await;
    let hello = frames
        .iter()
        .find(|f| f["type"] == "review_state")
        .expect("a state on hello");
    assert_eq!(hello["path"], "alias.md");
    let (mut ws, _) = served.viewing("a.md").await;
    let note = serde_json::json!({"type": "op", "cid": "v1", "path": "a.md",
        "rev": birta::rev::Rev::of(A_MD.as_bytes()), "op": "comment", "at": {"k": "doc"}, "body": "x"});
    let n = served::acked(&mut ws, note).await["n"].as_u64();
    let (_, frame) = served::until(&mut alias, "the alias's state", |f| {
        f["type"] == "review_state" && f["state"]["log"]["lines"].as_u64() >= n
    })
    .await;
    assert_eq!(frame["path"], "alias.md");
}

/// A note on the folder from the page at `path`, which is `text` on disk.
fn folder_note(cid: &str, path: &str, text: &str, body: &str) -> serde_json::Value {
    serde_json::json!({"type": "op", "cid": cid, "path": path,
        "rev": birta::rev::Rev::of(text.as_bytes()), "op": "comment", "at": {"k": "dir"}, "body": body})
}

#[tokio::test]
async fn a_folder_note_lands_in_the_folder_review_file() {
    let site = review_site();
    let a_log = site.read("a.md.birta.jsonl").unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut index, _) = served.viewing("index.md").await;
    let (mut ws, _) = served.viewing("c.md").await;
    served::acked(&mut ws, folder_note("f1", "c.md", "# C\n", "FOLDER-NOTE")).await;
    let notes = served::ops_named(&site.path(".birta.jsonl"), "comment");
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert_eq!(notes[0]["at"], "dir");
    assert_eq!(notes[0]["body"], "FOLDER-NOTE");
    assert!(
        !site.path("c.md.birta.jsonl").exists(),
        "the page's own doc gets no review file"
    );
    assert_eq!(site.read("a.md.birta.jsonl").unwrap(), a_log);
    dir_state_where(&mut index, "the folder's draft listed", |s| {
        s["folder"]["summary"]["counts"]["drafts"] == 1
    })
    .await;
}

#[tokio::test]
async fn a_folder_note_from_a_linked_folder_lands_at_the_served_root() {
    let site = served::Site::new();
    site.write("index.md", "# Index\n");
    let outside = site.base.join("vault");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("notes.md"), A_MD).unwrap();
    symlink("../vault", site.path("linked")).unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("linked/notes.md").await;
    served::acked(
        &mut ws,
        folder_note("f1", "linked/notes.md", A_MD, "FROM-LINKED"),
    )
    .await;
    let notes = served::ops_named(&site.path(".birta.jsonl"), "comment");
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert_eq!(notes[0]["at"], "dir");
    assert!(
        !outside.join(".birta.jsonl").exists(),
        "not the link target's folder"
    );
    assert!(!outside.join("notes.md.birta.jsonl").exists());
}

#[tokio::test]
async fn hand_back_all_takes_the_folder_note_into_the_batch() {
    let site = review_site();
    let mut folder = Scenario::folder("root");
    folder.comment(serde_json::json!("dir"), None, Some("FOLDER-NOTE"));
    site.write(".birta.jsonl", &folder.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("a.md").await;
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "a.md",
        "rev": birta::rev::Rev::of(A_MD.as_bytes()), "op": "hand_back", "scope": "all"});
    served::acked(&mut ws, hand_back).await;
    let a = served::ops_named(&site.path("a.md.birta.jsonl"), "submit");
    let f = served::ops_named(&site.path(".birta.jsonl"), "submit");
    assert_eq!((a.len(), f.len()), (1, 1), "{a:#?} {f:#?}");
    assert!(a[0]["batch"].is_u64(), "{a:#?}");
    assert_eq!(a[0]["batch"], f[0]["batch"]);
}

#[tokio::test]
async fn the_folder_summary_counts_every_live_folder_note_after_a_hand_back() {
    let site = review_site();
    let mut folder = Scenario::folder("root");
    folder.comment(serde_json::json!("dir"), None, Some("KEPT"));
    let gone = folder.comment(serde_json::json!("dir"), None, Some("TAKEN BACK"));
    folder.retract(gone);
    folder.comment(serde_json::json!("dir"), Some("ask"), Some("ASKED"));
    folder.submit();
    folder.claim("claude");
    site.write(".birta.jsonl", &folder.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("a.md").await;
    let listing = listing_where(&mut ws, &frames, "the folder's summary", |s| {
        !s["folder"]["summary"].is_null()
    })
    .await;
    let summary = &listing["folder"]["summary"];
    assert_eq!(summary["counts"]["drafts"], 0, "{summary:#?}");
    assert_eq!(
        summary["notes"], 2,
        "sent notes count, a retracted one doesn't: {summary:#?}"
    );
}

#[tokio::test]
async fn hand_back_all_with_only_a_folder_note_is_acked() {
    let site = served::Site::new();
    site.write("index.md", "# Index\n");
    let mut folder = Scenario::folder("root");
    folder.comment(serde_json::json!("dir"), None, Some("FOLDER-NOTE"));
    site.write(".birta.jsonl", &folder.log());
    let served = site.serve(&["."], &[GATE_ON]);
    let (mut ws, _) = served.viewing("index.md").await;
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "index.md",
        "rev": birta::rev::Rev::of(b"# Index\n"), "op": "hand_back", "scope": "all"});
    served::acked(&mut ws, hand_back).await;
    assert_eq!(
        served::ops_named(&site.path(".birta.jsonl"), "submit").len(),
        1
    );
    assert!(!site.path("index.md.birta.jsonl").exists());
}

#[tokio::test]
async fn withdrawing_a_hand_back_of_all_withdraws_every_file_of_its_batch() {
    let site = review_site();
    site.write("b.md", A_MD);
    let mut b = Scenario::new(A_MD);
    b.note("Another draft.");
    site.write("b.md.birta.jsonl", &b.log());
    let mut folder = Scenario::folder("root");
    folder.comment(serde_json::json!("dir"), None, Some("FOLDER-NOTE"));
    site.write(".birta.jsonl", &folder.log());
    let served = site.serve(&["."], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", "60000")]);
    let (mut ws, _) = served.viewing("a.md").await;
    let rev = birta::rev::Rev::of(A_MD.as_bytes());
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "a.md",
        "rev": rev, "op": "hand_back", "scope": "all"});
    let n = served::acked(&mut ws, hand_back).await["n"].clone();
    let withdraw = serde_json::json!({"type": "op", "cid": "w1", "path": "a.md",
        "rev": rev, "op": "withdraw", "to": n});
    served::acked(&mut ws, withdraw).await;
    for log in ["a.md.birta.jsonl", "b.md.birta.jsonl", ".birta.jsonl"] {
        let lines = served::log_lines(&site.path(log));
        let submit = lines
            .iter()
            .position(|l| l["op"] == "submit")
            .unwrap_or_else(|| panic!("{log}: no submit: {lines:#?}"));
        let withdrawn = lines
            .iter()
            .any(|l| l["op"] == "withdraw" && l["to"] == submit + 1);
        assert!(withdrawn, "{log} still sent: {lines:#?}");
    }
}

#[tokio::test]
async fn boot_says_whether_a_folder_is_served() {
    let site = review_site();
    let folder = site.serve(&["."], &[GATE_ON]);
    assert_eq!(folder.boot("a.md").await["review"]["dir"], true);
    let file = site.serve(&["a.md"], &[GATE_ON]);
    assert_eq!(file.boot("a.md").await["review"]["dir"], false);
    let off = site.serve(&["."], &[]);
    assert_eq!(
        off.boot("a.md").await["review"],
        serde_json::json!({"enabled": false})
    );
}

#[tokio::test]
async fn a_hand_back_of_all_from_a_doc_with_no_drafts_is_withdrawn_from_that_doc() {
    let site = review_site();
    let served = site.serve(&["."], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", "60000")]);
    let (mut ws, _) = served.viewing("c.md").await;
    let rev = birta::rev::Rev::of(b"# C\n");
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "c.md",
        "rev": rev, "op": "hand_back", "scope": "all"});
    let ack = served::acked(&mut ws, hand_back).await;
    assert!(ack["batch"].is_u64(), "the ack names its batch: {ack}");
    assert!(ack["not_before"].is_u64(), "and when its hold ends: {ack}");
    // Whatever line the ack names, the page can only send it back under its own path
    let withdraw = serde_json::json!({"type": "op", "cid": "w1", "path": "c.md",
        "rev": rev, "op": "withdraw", "to": ack["n"], "batch": ack["batch"]});
    served::acked(&mut ws, withdraw).await;
    let lines = served::log_lines(&site.path("a.md.birta.jsonl"));
    let submit = lines
        .iter()
        .position(|l| l["op"] == "submit")
        .expect("a.md was sent");
    assert!(
        lines
            .iter()
            .any(|l| l["op"] == "withdraw" && l["to"] == submit + 1),
        "{lines:#?}"
    );
    assert!(
        !site.path("c.md.birta.jsonl").exists(),
        "c.md never had a note"
    );
}

#[tokio::test]
async fn withdrawing_a_batch_leaves_a_hand_back_outside_it_alone() {
    let site = review_site();
    let mut c = Scenario::new("# C\n");
    c.note("Sent on its own.");
    c.submit();
    site.write("c.md.birta.jsonl", &c.log());
    let served = site.serve(&["."], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", "60000")]);
    let (mut ws, _) = served.viewing("a.md").await;
    let rev = birta::rev::Rev::of(A_MD.as_bytes());
    let hand_back = serde_json::json!({"type": "op", "cid": "h1", "path": "a.md",
        "rev": rev, "op": "hand_back", "scope": "all"});
    let n = served::acked(&mut ws, hand_back).await["n"].clone();
    let withdraw = serde_json::json!({"type": "op", "cid": "w1", "path": "a.md",
        "rev": rev, "op": "withdraw", "to": n});
    served::acked(&mut ws, withdraw).await;
    assert_eq!(
        served::ops_named(&site.path("a.md.birta.jsonl"), "withdraw").len(),
        1
    );
    let c_withdrawn = served::ops_named(&site.path("c.md.birta.jsonl"), "withdraw");
    assert!(c_withdrawn.is_empty(), "not in the batch: {c_withdrawn:#?}");
}

#[tokio::test]
async fn a_folder_note_is_refused_when_one_file_is_served() {
    let site = review_site();
    let served = site.serve(&["a.md"], &[GATE_ON]);
    let (mut ws, _) = served.viewing("a.md").await;
    let reason = served::nacked(&mut ws, folder_note("f1", "a.md", A_MD, "NOPE")).await;
    assert_eq!(reason, "bad_anchor");
    assert!(!site.path(".birta.jsonl").exists());
    assert!(!site.root.parent().unwrap().join(".birta.jsonl").exists());
}

#[tokio::test]
async fn a_replayed_hand_back_of_all_is_acked_with_its_first_batch() {
    let site = review_site();
    site.write("b.md", A_MD);
    let mut b = Scenario::new(A_MD);
    b.note("Another draft.");
    site.write("b.md.birta.jsonl", &b.log());
    let served = site.serve(&["."], &[GATE_ON, ("BIRTA_REVIEW_HOLD_MS", "60000")]);
    let (mut ws, _) = served.viewing("a.md").await;
    let rev = birta::rev::Rev::of(A_MD.as_bytes());
    let hand_back = serde_json::json!({"type": "op", "cid": "hb", "path": "a.md",
        "rev": rev, "op": "hand_back", "scope": "all"});
    let first = served::acked(&mut ws, hand_back.clone()).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let again = served::acked(&mut ws, hand_back).await;
    for key in ["n", "batch", "not_before"] {
        assert_eq!(first[key], again[key], "{key}: {first} then {again}");
    }
    let withdraw = serde_json::json!({"type": "op", "cid": "w1", "path": "a.md",
        "rev": rev, "op": "withdraw", "to": again["n"], "batch": again["batch"]});
    served::acked(&mut ws, withdraw).await;
    for log in ["a.md.birta.jsonl", "b.md.birta.jsonl"] {
        let path = site.path(log);
        assert_eq!(served::ops_named(&path, "submit").len(), 1, "{log}");
        assert_eq!(served::ops_named(&path, "withdraw").len(), 1, "{log}");
    }
}

#[tokio::test]
async fn a_doc_viewed_under_one_path_is_not_news_through_another() {
    let site = review_site();
    site.write("sub/x.md", "# X\n");
    symlink("sub", site.path("linked")).unwrap();
    let served = site.serve(&["."], &[GATE_ON]);
    let (_viewer, _) = served.viewing("sub/x.md").await;
    let (mut ws, _) = served.viewing("index.md").await;
    site.write("sub/x.md", "# X, edited\n");
    // Written after x.md, so its news arriving means x.md's write was handled
    tokio::time::sleep(Duration::from_millis(400)).await;
    site.write("d.md", "# D\n");
    let (seen, state) = served::until(&mut ws, "d.md as news", |f| {
        f["type"] == "dir_state" && entry(f, "d.md").is_some()
    })
    .await;
    for state in seen.iter().chain([&state]) {
        if state["type"] == "dir_state" {
            assert!(
                entry(state, "sub/x.md").is_none_or(|x| x["news"] == false),
                "{state}"
            );
        }
    }
}
