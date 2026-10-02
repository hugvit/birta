//! Where a served `birta` binds and how long it lives: the port derived from the served
//! path, handing over to a birta already serving that path, and staying up while a
//! review round is open.

use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

mod support {
    pub mod scenario;
    pub mod served;
}
use support::scenario::{DOC, Scenario};
use support::served::{GATE_ON, Served, Site, Ws, body_of, drain, until};

/// The server's wait between its last tab closing and shutting down.
const GRACE: Duration = Duration::from_secs(5);
/// Past the grace, for a loaded machine.
const MARGIN: Duration = Duration::from_secs(3);
const LOG: &str = "pr.md.birta.jsonl";
/// How long a launch that hands over to a running birta may take to exit, on a loaded machine.
const REUSE_WAIT: Duration = Duration::from_secs(10);

/// The port `birta` derives for a canonical served path: the first four bytes of the
/// path's SHA-1, big-endian, mod 8000, above 41000.
fn derived(served: &Path) -> u16 {
    let digest = sha1_smol::Sha1::from(served.as_os_str().as_encoded_bytes())
        .digest()
        .bytes();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    41000 + (n % 8000) as u16
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Writes `text` as `<stem>.md`, or `<stem>-N.md` when an unrelated listener holds the
/// first name's derived port; returns the name.
fn free_doc(site: &Site, stem: &str, text: &str) -> String {
    (0..)
        .map(|i| match i {
            0 => format!("{stem}.md"),
            i => format!("{stem}-{i}.md"),
        })
        .find(|name| port_free(derived(&site.path(name))))
        .inspect(|name| site.write(name, text))
        .unwrap()
}

/// A folder `<stem>[-N]` holding `index.md`, whose derived port nothing holds.
fn free_folder(site: &Site, stem: &str) -> String {
    (0..)
        .map(|i| match i {
            0 => stem.to_string(),
            i => format!("{stem}-{i}"),
        })
        .find(|name| port_free(derived(&site.path(name))))
        .inspect(|name| site.write(&format!("{name}/index.md"), "# Index\n"))
        .unwrap()
}

async fn whoami(served: &Served) -> Value {
    let (status, response) = served.get("/birta/whoami").await;
    assert_eq!(status, 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap_or_else(|e| panic!("{e}: {response}"))
}

fn serving_line(served: &Served) -> &str {
    served
        .stderr
        .iter()
        .find(|l| l.contains(" at http://"))
        .unwrap()
}

// --- The derived port ---------------------------------------------------------------------

#[tokio::test]
async fn with_review_on_and_no_port_a_file_is_served_on_the_port_its_path_derives() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let served = site.launch(&[&name], &[GATE_ON]);
    assert_eq!(
        served.port,
        derived(&site.path(&name)),
        "{:?}",
        served.stderr
    );
}

#[tokio::test]
async fn a_folder_is_served_on_the_port_the_folder_derives() {
    let site = Site::new();
    let folder = free_folder(&site, "docs");
    let served = site.launch(&[&folder], &[GATE_ON]);
    assert_eq!(
        served.port,
        derived(&site.path(&folder)),
        "{:?}",
        served.stderr
    );
}

#[tokio::test]
async fn a_server_killed_with_a_tab_open_comes_back_on_the_same_port() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.launch(&[&name], &[GATE_ON]);
    // The open socket leaves the old connection to linger as the process dies
    let (ws, _) = first.viewing(&name).await;
    first.kill();
    let again = site.launch(&[&name], &[GATE_ON]);
    assert_eq!(again.port, first.port, "{:?}", again.stderr);
    drop(ws);
}

#[tokio::test]
async fn two_files_in_one_folder_each_get_their_own_server() {
    let site = Site::new();
    let a = free_doc(&site, "a", "# A\n");
    let b = free_doc(&site, "b", "# B\n");
    let mut first = site.launch(&[&a], &[GATE_ON]);
    let mut second = site.launch_tty(&[&b], &[GATE_ON]);
    assert_eq!(
        second.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        second.stderr
    );
    assert!(first.running());
    assert_ne!(first.port, second.port);
    assert_eq!(
        whoami(&first).await["served"],
        site.path(&a).to_str().unwrap()
    );
    assert_eq!(
        whoami(&second).await["served"],
        site.path(&b).to_str().unwrap()
    );
}

#[tokio::test]
async fn with_review_on_an_explicit_port_0_neither_derives_nor_reuses() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.serve(&[&name], &[GATE_ON]);
    let mut second = site.launch_tty(&[&name, "--port", "0"], &[GATE_ON]);
    assert_eq!(
        second.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        second.stderr
    );
    assert!(first.running());
    assert_ne!(first.port, second.port);
    let d = derived(&site.path(&name));
    assert!(first.port != d && second.port != d);
}

#[tokio::test]
async fn with_review_off_no_port_still_auto_assigns_and_never_reuses() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.launch(&[&name], &[]);
    let mut second = site.launch_tty(&[&name], &[]);
    assert_eq!(
        second.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        second.stderr
    );
    assert!(first.running());
    assert_ne!(first.port, second.port);
    assert_ne!(first.port, derived(&site.path(&name)));
}

#[tokio::test]
async fn a_config_port_counts_as_given() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let config = site.base.join("home/.config/birta/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // A free port can be taken again before birta binds it, so a few tries
    for _ in 0..5 {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        std::fs::write(&config, format!("port = {port}\n")).unwrap();
        match site.start(&[&name], &[GATE_ON], false).try_serving() {
            Ok(served) => return assert_eq!(served.port, port, "{:?}", served.stderr),
            Err(stderr) if stderr.iter().any(|l| l.contains("Address already in use")) => {}
            Err(stderr) => panic!("no `serving` line: {stderr:#?}"),
        }
    }
    panic!("every free port was taken before birta bound it");
}

// --- Fallback ------------------------------------------------------------------------------

#[tokio::test]
async fn a_derived_port_held_by_a_birta_serving_another_path_falls_back_to_auto_assign() {
    let site = Site::new();
    let a = free_doc(&site, "a", "# A\n");
    let b = free_doc(&site, "b", "# B\n");
    let d = derived(&site.path(&a));
    let other = site.launch(&[&b, "--port", &d.to_string()], &[GATE_ON]);
    assert_eq!(other.port, d);
    let mut served = site.launch_tty(&[&a], &[GATE_ON]);
    assert_eq!(
        served.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        served.stderr
    );
    assert_ne!(served.port, d);
    assert_eq!(
        whoami(&served).await["served"],
        site.path(&a).to_str().unwrap()
    );
    assert_eq!(
        whoami(&other).await["served"],
        site.path(&b).to_str().unwrap()
    );
}

#[tokio::test]
async fn a_derived_port_held_by_a_listener_that_never_answers_falls_back_within_seconds() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let d = derived(&site.path(&name));
    let hog = TcpListener::bind(("127.0.0.1", d)).unwrap();
    std::thread::spawn(move || {
        let held: Vec<_> = hog.incoming().collect();
        drop(held);
    });
    let started = Instant::now();
    let mut served = site.launch_tty(&[&name], &[GATE_ON]);
    // Two tries of the probe's 2 s, plus a loaded machine's startup
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert_ne!(served.port, d);
    assert!(served.running());
}

// --- Reuse ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_second_birta_for_the_same_path_prints_the_first_ones_url_and_exits_0() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.launch(&[&name], &[GATE_ON]);
    let lexical = format!("./{name}");
    let mut second = site.launch_tty(&[&lexical], &[GATE_ON]);
    let status = second.exit_within(REUSE_WAIT);
    assert!(
        status.is_some_and(|s| s.success()),
        "{status:?}: {:?}",
        second.stderr
    );
    assert_eq!(second.port, first.port);
    assert_eq!(
        serving_line(&second),
        format!("birta: serving {name} at http://127.0.0.1:{}", first.port)
    );
    assert!(first.running());
}

#[tokio::test]
async fn a_launcher_without_a_terminal_never_reuses_and_auto_assigns() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.launch(&[&name], &[GATE_ON]);
    let mut second = site.launch(&[&name], &[GATE_ON]);
    assert_eq!(
        second.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        second.stderr
    );
    assert!(first.running());
    assert_eq!(first.port, derived(&site.path(&name)));
    assert_ne!(second.port, first.port);
    assert_eq!(
        whoami(&second).await["served"],
        site.path(&name).to_str().unwrap()
    );
}

#[tokio::test]
async fn reuse_follows_the_canonical_path_through_a_symlink() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    std::os::unix::fs::symlink(site.path(&name), site.path("alias.md")).unwrap();
    let first = site.launch(&[&name], &[GATE_ON]);
    let mut second = site.launch_tty(&["alias.md"], &[GATE_ON]);
    let status = second.exit_within(REUSE_WAIT);
    assert!(
        status.is_some_and(|s| s.success()),
        "{status:?}: {:?}",
        second.stderr
    );
    assert_eq!(second.port, first.port);
}

#[tokio::test]
async fn a_folder_reuses_its_own_server_but_a_file_inside_it_does_not() {
    let site = Site::new();
    let folder = free_folder(&site, "docs");
    let first = site.launch(&[&folder], &[GATE_ON]);
    let mut again = site.launch_tty(&[&folder], &[GATE_ON]);
    let status = again.exit_within(REUSE_WAIT);
    assert!(
        status.is_some_and(|s| s.success()),
        "{status:?}: {:?}",
        again.stderr
    );
    assert_eq!(again.port, first.port);
    let slashed = format!("{folder}/");
    let mut again = site.launch_tty(&[&slashed], &[GATE_ON]);
    let status = again.exit_within(REUSE_WAIT);
    assert!(
        status.is_some_and(|s| s.success()),
        "{status:?}: {:?}",
        again.stderr
    );

    let inside = format!("{folder}/index.md");
    let mut file = site.launch_tty(&[&inside], &[GATE_ON]);
    assert_eq!(
        file.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        file.stderr
    );
    assert_ne!(file.port, first.port);
}

#[tokio::test]
async fn a_terminal_launch_reuses_under_every_spelling_of_the_path() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    std::fs::create_dir_all(site.path("sub")).unwrap();
    std::os::unix::fs::symlink(&site.root, site.path("link")).unwrap();
    let first = site.launch(&[&name], &[GATE_ON]);
    let absolute = site.path(&name);
    let mut spellings = vec![
        format!("./{name}"),
        format!("sub/../{name}"),
        format!("link/{name}"),
        absolute.to_str().unwrap().to_string(),
    ];
    // macOS temp dirs sit behind the /var -> /private/var link
    if let Ok(rest) = absolute.strip_prefix("/private") {
        spellings.push(Path::new("/").join(rest).to_str().unwrap().to_string());
    }
    for spelling in spellings {
        let mut again = site.launch_tty(&[&spelling], &[GATE_ON]);
        let status = again.exit_within(REUSE_WAIT);
        assert!(
            status.is_some_and(|s| s.success()),
            "{spelling}: {status:?}: {:?}",
            again.stderr
        );
        assert_eq!(again.port, first.port, "{spelling}");
    }
}

#[tokio::test]
async fn an_explicit_port_never_reuses_even_when_it_is_the_derived_one() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let mut first = site.launch(&[&name], &[GATE_ON]);
    let d = derived(&site.path(&name));
    assert_eq!(first.port, d);
    let mut again = site.start(&[&name, "--port", &d.to_string()], &[GATE_ON], true);
    let status = again.exit_within(REUSE_WAIT);
    assert!(status.is_some_and(|s| !s.success()), "{status:?}");
    assert!(first.running());
}

#[tokio::test]
async fn a_birta_serving_a_same_named_doc_in_another_folder_is_not_reused() {
    let site = Site::new();
    let y = free_doc(&site, "y/pr", DOC);
    let x = format!("x/{}", y.rsplit('/').next().unwrap());
    site.write(&x, DOC);
    let d = derived(&site.path(&y));
    let other = site.launch(&[&x, "--port", &d.to_string()], &[GATE_ON]);
    assert_eq!(other.port, d);
    let mut served = site.launch_tty(&[&y], &[GATE_ON]);
    assert_eq!(
        served.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        served.stderr
    );
    assert_ne!(served.port, d);
    assert_eq!(
        whoami(&served).await["served"],
        site.path(&y).to_str().unwrap()
    );
}

#[tokio::test]
async fn a_derived_port_held_by_some_other_http_server_falls_back() {
    for reply in [
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        "HTTP/1.1 200 OK\r\ncontent-length: 8\r\nconnection: close\r\n\r\nnot json",
    ] {
        let site = Site::new();
        let name = free_doc(&site, "pr", DOC);
        let d = derived(&site.path(&name));
        let hog = TcpListener::bind(("127.0.0.1", d)).unwrap();
        std::thread::spawn(move || {
            for mut conn in hog.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut conn, &mut buf);
                let _ = conn.write_all(reply.as_bytes());
            }
        });
        let started = Instant::now();
        let mut served = site.launch_tty(&[&name], &[GATE_ON]);
        assert!(started.elapsed() < Duration::from_secs(5), "{reply:?}");
        assert_ne!(served.port, d);
        assert_eq!(
            served.exit_within(Duration::from_secs(1)),
            None,
            "{reply:?}: {:?}",
            served.stderr
        );
    }
}

#[tokio::test]
async fn two_terminal_launches_at_once_leave_one_server() {
    let site = Site::new();
    let name = free_doc(&site, "pr", DOC);
    let a = site.start(&[&name], &[GATE_ON], true);
    let b = site.start(&[&name], &[GATE_ON], true);
    let (mut a, mut b) = (a.serving(), b.serving());
    let d = derived(&site.path(&name));
    assert_eq!((a.port, b.port), (d, d));
    let (ea, eb) = (
        a.exit_within(Duration::from_secs(3)),
        b.exit_within(Duration::from_secs(1)),
    );
    let exits: Vec<_> = [ea, eb].into_iter().flatten().collect();
    assert_eq!(exits.len(), 1, "{ea:?} {eb:?}");
    assert!(exits[0].success(), "{exits:?}");
}

// --- whoami --------------------------------------------------------------------------------

#[tokio::test]
async fn whoami_names_the_served_file_and_a_per_server_id_but_never_the_review_token() {
    let site = Site::new();
    site.write("pr.md", DOC);
    let first = site.serve(&["pr.md"], &[GATE_ON]);
    let second = site.serve(&["pr.md"], &[GATE_ON]);
    let (status, response) = first.get("/birta/whoami").await;
    assert_eq!(status, 200, "{response}");
    let me: Value = serde_json::from_str(body_of(&response)).unwrap();
    assert_eq!(me["served"], site.path("pr.md").to_str().unwrap());
    let id = me["id"].as_str().expect("an id");
    assert!(!id.is_empty());
    assert_ne!(whoami(&second).await["id"], me["id"]);
    let token = first.boot("pr.md").await["review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!response.contains(&token), "{response}");
}

#[tokio::test]
async fn whoami_names_the_served_folder_in_folder_mode() {
    let site = Site::new();
    site.write("docs/index.md", "# Index\n");
    let served = site.serve(&["docs"], &[GATE_ON]);
    assert_eq!(
        whoami(&served).await["served"],
        site.path("docs").to_str().unwrap()
    );
}

#[tokio::test]
async fn whoami_refuses_a_foreign_host() {
    let site = Site::new();
    site.write("pr.md", DOC);
    let served = site.serve(&["pr.md"], &[GATE_ON]);
    let (status, _) = served
        .request("GET", "/birta/whoami", &[("host", "evil.com")], None)
        .await;
    assert_eq!(status, 421);
}

// --- Lifetime ------------------------------------------------------------------------------

/// A doc's review file at each stage, by name. `working` and `replied` both extend
/// `claimed`: the agent replies inside its open turn, or yields it.
fn stage(name: &str) -> Scenario {
    let mut s = Scenario::new(DOC);
    let note = s.note("Tighten the intro.");
    if name == "drafts" {
        return s;
    }
    s.submit();
    if name == "sent" {
        return s;
    }
    let turn = s.claim("claude");
    match name {
        "claimed" => return s,
        "working" => {
            s.agent_reply("claude", note, "note", "On it.");
            return s;
        }
        _ => {}
    }
    s.yield_turn("claude", turn);
    if name == "replied" {
        return s;
    }
    s.approve();
    assert_eq!(name, "approved");
    s
}

/// Appends the lines stage `to` has past stage `from`, as a writer appending to the log.
fn advance(site: &Site, from: &str, to: &str) {
    let (old, new) = (stage(from).log(), stage(to).log());
    let tail = new
        .strip_prefix(&old)
        .expect("a later stage of the same log");
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(site.path(LOG))
        .unwrap();
    log.write_all(tail.as_bytes()).unwrap();
}

#[test]
fn each_stage_folds_to_its_name() {
    for (name, want) in [
        ("drafts", "drafts"),
        ("sent", "sent"),
        ("claimed", "picked_up"),
        ("working", "picked_up"),
        ("replied", "replied"),
        ("approved", "approved"),
    ] {
        let stage = serde_json::to_value(stage(name).fold().phrase.stage).unwrap();
        assert_eq!(stage, want, "{name}");
    }
}

/// `pr.md` with its review file at `name`'s stage.
fn staged(name: &str) -> Site {
    let site = Site::new();
    site.write("pr.md", DOC);
    site.write(LOG, &stage(name).log());
    site
}

/// Opens a tab on `path` and closes it again.
async fn visit(served: &Served, path: &str) {
    let (ws, _) = served.viewing(path).await;
    drop(ws);
}

/// Closes the only tab, then reports whether the server outlived the grace.
async fn outlives_last_tab(served: &mut Served, path: &str) -> bool {
    visit(served, path).await;
    tokio::time::sleep(GRACE + MARGIN).await;
    served.running()
}

#[tokio::test]
async fn with_no_round_open_the_server_exits_once_its_last_tab_closes() {
    let names = ["drafts", "replied", "approved"];
    let sites = names.map(staged);
    let mut all = Vec::new();
    for site in &sites {
        let served = site.serve(&["pr.md"], &[GATE_ON]);
        visit(&served, "pr.md").await;
        all.push(served);
    }
    for (name, served) in names.iter().zip(&mut all) {
        let status = served.exit_within(GRACE + MARGIN);
        assert!(status.is_some_and(|s| s.success()), "{name}: {status:?}");
    }
}

#[tokio::test]
async fn a_handed_back_round_keeps_the_server_up_after_its_last_tab_closes() {
    let site = staged("sent");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "pr.md").await);
    // A hold, not a longer grace
    tokio::time::sleep(Duration::from_secs(17)).await;
    assert!(served.running());
}

#[tokio::test]
async fn a_claimed_round_awaiting_its_respond_keeps_the_server_up() {
    let site = staged("claimed");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "pr.md").await);
}

#[tokio::test]
async fn once_the_round_closes_with_no_tab_open_the_server_exits() {
    let site = staged("claimed");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "pr.md").await);
    advance(&site, "claimed", "replied");
    let status = served.exit_within(GRACE + MARGIN);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

#[tokio::test]
async fn a_write_to_the_log_that_leaves_the_round_open_keeps_the_server_up() {
    let site = staged("claimed");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "pr.md").await);
    advance(&site, "claimed", "working");
    assert_eq!(served.exit_within(GRACE + MARGIN), None);
}

#[tokio::test]
async fn with_the_gate_off_an_open_round_holds_nothing() {
    let site = staged("sent");
    let mut served = site.serve(&["pr.md"], &[]);
    visit(&served, "pr.md").await;
    let status = served.exit_within(GRACE + MARGIN);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

#[tokio::test]
async fn in_a_folder_an_open_round_on_a_doc_other_than_the_entry_keeps_the_server_up() {
    let site = Site::new();
    site.write("index.md", "# Index\n\n[checklist](linked/checklist.md)\n");
    site.write("linked/checklist.md", DOC);
    site.write("linked/checklist.md.birta.jsonl", &stage("sent").log());
    let mut served = site.serve(&["."], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "index.md").await);
}

#[tokio::test]
async fn in_single_file_mode_a_linked_siblings_open_round_holds_nothing_until_opened() {
    let site = Site::new();
    site.write("pr.md", "# PR\n\nSee [the plan](docs/plan.md).\n");
    site.write(
        LOG,
        &Scenario::new("# PR\n\nSee [the plan](docs/plan.md).\n").log(),
    );
    site.write("docs/plan.md", DOC);
    site.write("docs/plan.md.birta.jsonl", &stage("sent").log());
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("pr.md").await;
    let listing = listing_naming(&mut ws, &frames, "pr.md").await;
    assert!(!lists(&listing, "docs/plan.md"), "{listing}");
    drop(ws);
    let status = served.exit_within(GRACE + MARGIN);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

#[tokio::test]
async fn in_single_file_mode_an_open_round_on_a_doc_a_page_opened_keeps_the_server_up() {
    let site = Site::new();
    site.write("pr.md", "# PR\n\nSee [the plan](docs/plan.md).\n");
    site.write("docs/plan.md", DOC);
    site.write("docs/plan.md.birta.jsonl", &stage("sent").log());
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "docs/plan.md").await);
}

#[tokio::test]
async fn a_round_opened_while_no_tab_is_open_keeps_the_server_up() {
    let site = staged("drafts");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    visit(&served, "pr.md").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    advance(&site, "drafts", "sent");
    tokio::time::sleep(GRACE + MARGIN).await;
    assert!(served.running());
}

#[tokio::test]
async fn once_the_round_closes_the_server_still_waits_out_the_grace() {
    let site = staged("claimed");
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    assert!(outlives_last_tab(&mut served, "pr.md").await);
    advance(&site, "claimed", "replied");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(served.running(), "exited with no grace");
    visit(&served, "pr.md").await;
    let status = served.exit_within(GRACE + MARGIN);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

// --- The listing in single-file mode ---------------------------------------------------------

fn lists(frame: &Value, path: &str) -> bool {
    frame["files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|f| f["paths"].as_array().unwrap().iter().any(|p| p == path))
    })
}

/// The hello's listing when it already names `path`, else the first push that does.
async fn listing_naming(ws: &mut Ws, frames: &[Value], path: &str) -> Value {
    let is = |f: &Value| f["type"] == "dir_state" && lists(f, path);
    match frames.iter().find(|f| is(f)) {
        Some(f) => f.clone(),
        None => until(ws, "a listing naming the doc", is).await.1,
    }
}

#[tokio::test]
async fn in_single_file_mode_the_listing_leaves_out_reviewed_docs_this_server_never_served() {
    let site = staged("drafts");
    site.write("docs/plan.md", DOC);
    site.write("docs/plan.md.birta.jsonl", &stage("drafts").log());
    site.write("notes.md", DOC);
    site.write("notes.md.birta.jsonl", &stage("sent").log());
    let served = site.serve(&["pr.md"], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("pr.md").await;
    let listing = listing_naming(&mut ws, &frames, "pr.md").await;
    assert!(!lists(&listing, "docs/plan.md"), "{listing}");
    assert!(!lists(&listing, "notes.md"), "{listing}");
    for frame in drain(&mut ws, Duration::from_secs(1)).await {
        assert!(
            !lists(&frame, "docs/plan.md") && !lists(&frame, "notes.md"),
            "{frame}"
        );
    }
}

#[tokio::test]
async fn in_single_file_mode_a_doc_a_page_opened_joins_the_listing() {
    for via_view in [true, false] {
        let site = staged("drafts");
        site.write("notes.md", DOC);
        site.write("notes.md.birta.jsonl", &stage("drafts").log());
        let served = site.serve(&["pr.md"], &[GATE_ON]);
        let (mut ws, frames) = served.viewing("pr.md").await;
        let listing = listing_naming(&mut ws, &frames, "pr.md").await;
        assert!(!lists(&listing, "notes.md"), "{listing}");
        if via_view {
            let (status, page) = served.get("/view/notes.md").await;
            assert_eq!(status, 200, "{page}");
        } else {
            let (other, _) = served.viewing("notes.md").await;
            drop(other);
        }
        let listing = until(&mut ws, "a listing naming the opened doc", |f| {
            f["type"] == "dir_state" && lists(f, "notes.md")
        })
        .await
        .1;
        assert!(lists(&listing, "pr.md"), "via_view {via_view}: {listing}");
    }
}

#[tokio::test]
async fn in_single_file_mode_a_write_to_a_doc_this_server_never_served_is_no_news() {
    let site = staged("drafts");
    let served = site.serve(&["pr.md"], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("pr.md").await;
    listing_naming(&mut ws, &frames, "pr.md").await;
    // A socket that never says hello gets every doc's content, so it shows the write landed
    let mut all = served.connect().await;
    site.write("other.md", "# Other\n");
    until(&mut all, "the write's content", |f| {
        f["type"] == "content" && f["path"] == "other.md"
    })
    .await;
    for frame in drain(&mut ws, Duration::from_secs(1)).await {
        assert!(!lists(&frame, "other.md"), "{frame}");
    }
}

#[tokio::test]
async fn in_single_file_mode_a_path_no_page_could_open_never_joins_the_scope() {
    let site = staged("drafts");
    site.write("../outside.md", DOC);
    std::fs::write(
        site.base.join("outside.md.birta.jsonl"),
        stage("sent").log(),
    )
    .unwrap();
    let mut served = site.serve(&["pr.md"], &[GATE_ON]);
    let (mut ws, frames) = served.viewing("pr.md").await;
    listing_naming(&mut ws, &frames, "pr.md").await;
    let (status, _) = served.get("/view/ghost.md").await;
    assert_eq!(status, 404);
    let (outside, _) = served.viewing("../outside.md").await;
    drop(outside);
    let mut all = served.connect().await;
    site.write("ghost.md", DOC);
    site.write("ghost.md.birta.jsonl", &stage("sent").log());
    until(&mut all, "the ghost's content", |f| {
        f["type"] == "content" && f["path"] == "ghost.md"
    })
    .await;
    for frame in drain(&mut ws, Duration::from_secs(1)).await {
        assert!(!lists(&frame, "ghost.md"), "{frame}");
    }
    drop((ws, all));
    let status = served.exit_within(GRACE + MARGIN);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}

// --- The folder breadcrumb ------------------------------------------------------------------

fn header_name(page: &str) -> &str {
    let open = "<span class=\"file-header-name\">";
    let start = page.find(open).expect("a header name") + open.len();
    &page[start..start + page[start..].find("</span>").unwrap()]
}

async fn header_of(served: &Served, rel: &str) -> String {
    let (status, page) = served.get(&format!("/view/{rel}")).await;
    assert_eq!(status, 200, "{page}");
    header_name(&page).to_string()
}

#[tokio::test]
async fn in_folder_mode_the_header_names_the_doc_by_its_served_relative_path() {
    for envs in [&[GATE_ON][..], &[][..]] {
        let site = Site::new();
        site.write("index.md", "# Index\n");
        site.write("linked/checklist.md", "# Checklist\n");
        site.write("real/doc.md", "# Doc\n");
        std::os::unix::fs::symlink(site.path("real/doc.md"), site.path("alias.md")).unwrap();
        let served = site.serve(&["."], envs);
        assert_eq!(
            header_of(&served, "linked/checklist.md").await,
            "linked/checklist.md"
        );
        assert_eq!(header_of(&served, "index.md").await, "index.md");
        // The lexical path the page was opened by, not where the link resolves
        assert_eq!(header_of(&served, "alias.md").await, "alias.md");
    }
}

#[tokio::test]
async fn in_single_file_mode_the_header_keeps_the_file_name() {
    let site = Site::new();
    site.write("pr.md", DOC);
    let served = site.serve(&["pr.md"], &[GATE_ON]);
    assert_eq!(header_of(&served, "pr.md").await, "pr.md");
}

// --- The restart command ------------------------------------------------------------------------

/// A path as a POSIX shell reads it back.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[tokio::test]
async fn the_restart_names_birta_only_when_path_finds_this_binary() {
    let site = Site::new();
    site.write("pr.md", DOC);
    let doc = quoted(&site.path("pr.md"));
    let me = assert_cmd::cargo::cargo_bin("birta")
        .canonicalize()
        .unwrap();
    let own = std::env::join_paths([me.parent().unwrap().to_path_buf()]).unwrap();
    // An unrelated `birta` first on PATH, as an older install would be
    let other = site.base.join("bin");
    std::fs::create_dir_all(&other).unwrap();
    let fake = other.join("birta");
    std::fs::write(&fake, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let elsewhere =
        std::env::join_paths([other.clone(), me.parent().unwrap().to_path_buf()]).unwrap();
    for (path, program) in [
        (own.to_str().unwrap(), "birta".to_string()),
        (elsewhere.to_str().unwrap(), quoted(&me)),
    ] {
        let served = site.serve(&["pr.md"], &[GATE_ON, ("PATH", path)]);
        let review = served.boot("pr.md").await["review"].clone();
        // `--port 0` binds off the derived port, which the restart then has to name
        assert_eq!(
            review["restart"],
            format!("BIRTA_REVIEW=1 {program} {doc} --port {}", served.port),
            "PATH {path}"
        );
        assert_eq!(review["served"], site.path("pr.md").to_str().unwrap());
    }
}

#[tokio::test]
async fn the_restart_carries_no_env_when_config_turned_review_on() {
    let site = Site::new();
    site.write("pr.md", DOC);
    let config = site.base.join("home/.config/birta/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[review]\nenabled = true\n").unwrap();
    let me = assert_cmd::cargo::cargo_bin("birta")
        .canonicalize()
        .unwrap();
    let own = me.parent().unwrap().to_str().unwrap().to_string();
    let served = site.serve(&["pr.md"], &[("PATH", &own)]);
    assert_eq!(
        served.boot("pr.md").await["review"]["restart"],
        format!(
            "birta {} --port {}",
            quoted(&site.path("pr.md")),
            served.port
        )
    );
}

/// A restart binds the derived port unless it names one, so the restart names the port
/// whenever the server fell back from the derived one.
#[tokio::test]
async fn the_restart_names_the_port_only_off_the_derived_one() {
    let site = Site::new();
    let a = free_doc(&site, "a", "# A\n");
    let b = free_doc(&site, "b", "# B\n");
    let on = site.launch(&[&a], &[GATE_ON]);
    let restart = |review: &Value| review["restart"].as_str().unwrap().to_string();
    let review = on.boot(&a).await["review"].clone();
    assert!(
        restart(&review).ends_with(&quoted(&site.path(&a))),
        "{review}"
    );
    // b's derived port taken by a birta serving another path: b falls back
    let d = derived(&site.path(&b));
    let _holder = site.launch(&[&a, "--port", &d.to_string()], &[GATE_ON]);
    let mut fell = site.launch_tty(&[&b], &[GATE_ON]);
    assert_eq!(
        fell.exit_within(Duration::from_secs(2)),
        None,
        "{:?}",
        fell.stderr
    );
    assert_ne!(fell.port, d);
    let review = fell.boot(&b).await["review"].clone();
    assert!(
        restart(&review).ends_with(&format!("{} --port {}", quoted(&site.path(&b)), fell.port)),
        "{review}"
    );
}
