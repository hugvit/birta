//! Browser review over the socket: page ops into the review file, the folded state and
//! folder listing pushed back, half-typed notes, and the beacon a closing page sends.

use std::collections::HashMap;
use std::ffi::CString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::anchor::StopKind;
use crate::dirindex::{DirIndex, FileSig};
use crate::paths::{Access, DocRef, PathError};
use crate::rev::Rev;
use crate::review::chain::Chain;
use crate::review::doc::Doc;
use crate::review::log::{self, AppendCtx, AppendError, Held, Log};
use crate::review::ops::{
    self, AnchorReq, At, BlockScope, By, ByKind, CommentKind, Header, Line, Op, Parsed, Verdict,
};
use crate::review::state::{HeldState, ItemStatus, ReviewState, Stage};
use crate::review::{FOLDER_REVIEW, Reject, fold, held, review_path_for, sync, undo};
use crate::server::{AppState, WsEvent};

const DEFAULT_HOLD_MS: u64 = 5_000;
/// At most one folder listing per this long; a burst ends with one more.
const DIR_PUSH_EVERY: Duration = Duration::from_millis(250);

/// Ops a closing page may still send: seen marks and notes it had not heard back about.
const BEACON_OPS: [&str; 5] = ["comment", "amend", "retract", "reply", "seen"];

/// Why an op was refused, as the page's nack names it.
type Nack = &'static str;

/// Review for one server, present only when the gate is on and a folder is served.
pub(crate) struct ReviewHub {
    /// Per-server secret the page's beacon must carry.
    pub(crate) token: String,
    drafted_by: Option<String>,
    hold_ms: u64,
    /// Where half-typed notes live; `None` when no state folder can be named.
    drafts: Option<PathBuf>,
    git_exclude: bool,
    /// A hand-back of "all" sends every doc under the root only when a folder is served.
    pub(crate) dir_mode: bool,
    pub(crate) dir: DirIndex,
    /// The canonical path this server was named, and that path as typed.
    pub(crate) served_as: Option<(PathBuf, String)>,
    folds: Mutex<HashMap<PathBuf, (FoldKey, Arc<Folded>)>>,
    /// What the pages last saw of each held doc, so a write that changes none of it isn't pushed.
    pushed: Mutex<HashMap<PathBuf, Value>>,
    /// The pending hold each doc has a timer for, by its end.
    holds: Mutex<HashMap<PathBuf, u64>>,
    dir_push: Mutex<DirPush>,
    /// Runs the hold and listing timers; none outside a runtime, as in unit tests.
    rt: Option<tokio::runtime::Handle>,
}

/// The doc's and its review file's signatures a fold was made from.
type FoldKey = (Option<FileSig>, Option<FileSig>);

/// A doc's state for the page, and the text it shows while the doc is held.
struct Folded {
    state: ReviewState,
    held: Option<Arc<str>>,
}

#[derive(Debug, Default)]
struct DirPush {
    last: Option<Instant>,
    trailing: bool,
    hash: Option<u64>,
}

/// A doc's new state, for every page viewing it under any of its paths.
#[derive(Debug)]
pub(crate) struct ReviewPush {
    pub(crate) canonical: PathBuf,
    pub(crate) state: ReviewState,
}

impl ReviewHub {
    pub(crate) fn new(
        root: &Path,
        drafted_by: Option<String>,
        git_exclude: bool,
        dir_mode: bool,
    ) -> Self {
        Self {
            token: new_token(),
            drafted_by,
            hold_ms: hold_ms(),
            drafts: drafts_dir(),
            git_exclude,
            dir_mode,
            dir: DirIndex::new(root),
            served_as: None,
            folds: Mutex::default(),
            pushed: Mutex::default(),
            holds: Mutex::default(),
            dir_push: Mutex::default(),
            rt: tokio::runtime::Handle::try_current().ok(),
        }
    }

    /// The doc's review, folded from its file or, with none yet, from the file a first
    /// note would create. `None` when the doc can't be read as text.
    pub(crate) fn state_of(&self, canonical: &Path, folder_root: bool) -> Option<ReviewState> {
        let mut state = self.folded(canonical)?.state.clone();
        state.server_now = now_ms();
        state.writable = folder_root && can_write(canonical);
        state.log_writable = log_writable(folder_root, canonical);
        state.hand_off = fold::hand_off(&self.typed_path(canonical));
        Some(state)
    }

    /// The text a held doc shows the reviewer, at its held rev; `None` when not held.
    pub(crate) fn held_text(&self, canonical: &Path) -> Option<(Rev, Arc<str>)> {
        let folded = self.folded(canonical)?;
        let rev = Rev::parse(&folded.state.held.as_ref()?.rev)?;
        Some((rev, folded.held.clone()?))
    }

    /// Whether the doc is held: what the page shows and what may write it agree on this.
    pub(crate) fn is_held(&self, canonical: &Path) -> bool {
        self.folded(canonical).is_some_and(|f| f.held.is_some())
    }

    fn folded(&self, canonical: &Path) -> Option<Arc<Folded>> {
        let log_path = review_path_for(canonical);
        let key = (FileSig::of(canonical), FileSig::of(&log_path));
        let cached = lock(&self.folds)
            .get(canonical)
            .filter(|(k, _)| *k == key)
            .map(|(_, folded)| Arc::clone(folded));
        if let Some(folded) = cached {
            return Some(folded);
        }
        let bytes = sync::read_doc(canonical).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let parsed = Log::open(&log_path).read().ok().flatten();
        let folded = match &parsed {
            Some(parsed) => match held::view(parsed) {
                Some(view) => {
                    let mut state = fold_text(&view.log, &view.text);
                    state.held = Some(HeldState {
                        rev: view.at.rev.as_str().to_string(),
                        n: view.at.n,
                    });
                    // The file's own bookkeeping is not held back
                    state.log.lines = fold::last_line(parsed);
                    Folded {
                        state,
                        held: Some(view.text),
                    }
                }
                None => Folded {
                    state: fold_text(parsed, &text),
                    held: None,
                },
            },
            None => {
                let mut state = fold_text(&self.unwritten(canonical, &text), &text);
                state.log.lines = 0;
                Folded { state, held: None }
            }
        };
        let folded = Arc::new(folded);
        lock(&self.folds).insert(canonical.to_path_buf(), (key, Arc::clone(&folded)));
        Some(folded)
    }

    /// The path an agent would type for `canonical`: the served folder as typed, or the
    /// served doc as typed, or another doc beside it through the same typed folder.
    fn typed_path(&self, canonical: &Path) -> PathBuf {
        let Some((served, typed)) = &self.served_as else {
            return canonical.to_path_buf();
        };
        if self.dir_mode || canonical == served {
            return PathBuf::from(typed);
        }
        let beside = served
            .parent()
            .and_then(|dir| canonical.strip_prefix(dir).ok());
        match (beside, Path::new(typed).parent()) {
            (Some(rel), Some(dir)) => dir.join(rel),
            _ => canonical.to_path_buf(),
        }
    }

    /// The review file a first note on the doc would create, parsed but never written.
    fn unwritten(&self, canonical: &Path, text: &str) -> Parsed {
        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let by = By {
            kind: ByKind::Birta,
            ..By::default()
        };
        let line = |n, op| Line {
            n,
            ts: Some(now_ms()),
            by: by.clone(),
            cid: None,
            op,
        };
        let lines = [
            serde_json::to_string(&Header::new(&name)),
            serde_json::to_string(&line(
                2,
                Op::Snap {
                    rev: Rev::of(text.as_bytes()),
                    text: text.to_string(),
                    checkpoint: false,
                },
            )),
            serde_json::to_string(&line(
                3,
                Op::Owner {
                    drafted_by: self.drafted_by.clone(),
                },
            )),
        ];
        let jsonl: String = lines.into_iter().flatten().map(|l| l + "\n").collect();
        ops::parse(jsonl)
    }

    /// Where a half-typed note on the doc is kept, by its key.
    fn draft_file(&self, canonical: &Path, key: &str) -> Option<PathBuf> {
        let dir = self
            .drafts
            .as_ref()?
            .join(sha1_hex(canonical.as_os_str().as_bytes()));
        Some(dir.join(format!("{}.json", sha1_hex(key.as_bytes()))))
    }

    /// The half-typed notes kept for the doc, sorted by key.
    pub(crate) fn drafts_of(&self, canonical: &Path) -> Vec<Value> {
        let Some(dir) = self
            .drafts
            .as_ref()
            .map(|d| d.join(sha1_hex(canonical.as_os_str().as_bytes())))
        else {
            return Vec::new();
        };
        let mut items: Vec<Value> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
            .filter(|v| v["key"].is_string() && v["text"].is_string())
            .map(|v| json!({"key": v["key"], "text": v["text"]}))
            .collect();
        items.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        items
    }

    /// Keeps, replaces or, for empty text, deletes a half-typed note.
    fn compose(&self, canonical: &Path, key: &str, text: &str) -> io::Result<()> {
        let Some(file) = self.draft_file(canonical, key) else {
            return Ok(());
        };
        if text.is_empty() {
            return match std::fs::remove_file(&file) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = json!({"key": key, "text": text}).to_string();
        crate::atomic_write::write_atomic(&file, body.as_bytes())
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn fold_text(parsed: &Parsed, text: &str) -> ReviewState {
    let chain = Chain::build(parsed);
    let doc = Doc::new(text);
    fold::fold(parsed, &chain, &doc).state
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `BIRTA_REVIEW_HOLD_MS`, or the default when it isn't a whole number of ms.
fn hold_ms() -> u64 {
    std::env::var("BIRTA_REVIEW_HOLD_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_HOLD_MS)
}

/// `$XDG_STATE_HOME/birta/drafts`, else `~/.local/state/birta/drafts`.
fn drafts_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let state = xdg.or_else(|| Some(dirs::home_dir()?.join(".local/state")))?;
    Some(state.join("birta/drafts"))
}

pub(crate) fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source failed");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha1_hex(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

/// `access(W_OK)`: ownership and mounts count, not only the mode bits.
fn can_write(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the duration of the call
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Whether the doc's review file may be written: it sits beside a doc in a folder root,
/// in a folder birta can write. Every review write checks this, not only the first.
fn log_writable(folder_root: bool, canonical: &Path) -> bool {
    folder_root && canonical.parent().is_some_and(can_write)
}

fn human() -> By {
    By {
        kind: ByKind::Human,
        name: None,
        via: Some(ops::Via::Browser),
    }
}

// --- pushes ---

fn review_frame(path: &str, state: &ReviewState) -> String {
    json!({"type": "review_state", "path": path, "rev": state.rev, "state": state}).to_string()
}

/// What follows `content` on a hello: the doc's state, the listing, and any
/// half-typed notes.
pub(crate) fn hello_frames(app: &Arc<AppState>, hub: &ReviewHub, doc: &DocRef) -> Vec<String> {
    let mut frames = Vec::new();
    if let Some(state) = hub.state_of(&doc.canonical, doc.log_writable) {
        frames.push(review_frame(&doc.rel, &state));
        arm_hold(app, hub, &doc.canonical, &state);
    }
    let read = hub.dir.viewed(&doc.canonical);
    let joined = hub.dir.opened(&doc.canonical, &doc.rel);
    if joined {
        hub.dir.refresh(&doc.canonical);
    }
    if read || joined {
        push_dir(app);
    }
    frames.push(hub.dir.frame().to_string());
    let items = hub.drafts_of(&doc.canonical);
    if !items.is_empty() {
        frames
            .push(json!({"type": "compose_restore", "path": doc.rel, "items": items}).to_string());
    }
    frames
}

/// The doc's state and the listing again, for a page that missed pushes.
pub(crate) fn resync_frames(app: &AppState, canonical: &Path, rel: &str) -> Vec<String> {
    let Some(hub) = &app.review else {
        return Vec::new();
    };
    let state = hub.state_of(canonical, app.roots.folder_root(canonical));
    let review = state.map(|state| review_frame(rel, &state));
    review
        .into_iter()
        .chain([hub.dir.frame().to_string()])
        .collect()
}

/// A page's own `review_state` for a push, under the path it views.
pub(crate) fn push_frame(push: &ReviewPush, rel: &str) -> String {
    review_frame(rel, &push.state)
}

/// Folds the doc again and sends its state to every page viewing it.
pub(crate) fn push_state(app: &Arc<AppState>, canonical: &Path) {
    push(app, canonical, false);
}

/// [`push_state`] after a write to the doc or its review file. While the doc is held,
/// the agent's writes change nothing the pages show, so the same state isn't sent again.
fn push_changed(app: &Arc<AppState>, canonical: &Path) {
    push(app, canonical, true);
}

fn push(app: &Arc<AppState>, canonical: &Path, unless_same: bool) {
    let Some(hub) = &app.review else {
        return;
    };
    let Some(state) = hub.state_of(canonical, app.roots.folder_root(canonical)) else {
        return;
    };
    let shown = state.held.as_ref().and_then(|_| {
        let mut v = serde_json::to_value(&state).ok()?;
        // The clock and the file's line count move with every write; the page shows neither
        v.as_object_mut()?
            .retain(|k, _| k != "server_now" && k != "log");
        Some(v)
    });
    {
        let mut pushed = lock(&hub.pushed);
        let same = shown.is_some() && pushed.get(canonical) == shown.as_ref();
        if unless_same && same {
            return;
        }
        match shown {
            Some(v) => pushed.insert(canonical.to_path_buf(), v),
            None => pushed.remove(canonical),
        };
    }
    arm_hold(app, hub, canonical, &state);
    let _ = app.hub.send(WsEvent::Review(Arc::new(ReviewPush {
        canonical: canonical.to_path_buf(),
        state,
    })));
}

/// One timer per pending hold, so pages learn the moment it passes.
fn arm_hold(app: &Arc<AppState>, hub: &ReviewHub, canonical: &Path, state: &ReviewState) {
    let Some(hold) = &state.hold else {
        return;
    };
    let now = state.server_now;
    if now >= hold.not_before {
        return;
    }
    let Some(rt) = hub.rt.as_ref() else {
        return;
    };
    let end = hold.not_before;
    if lock(&hub.holds).insert(canonical.to_path_buf(), end) == Some(end) {
        return;
    }
    let (app, canonical) = (Arc::clone(app), canonical.to_path_buf());
    // A little past the end, so the state built on waking is on the far side of it
    let wait = Duration::from_millis(end - now + 5);
    rt.spawn(async move {
        tokio::time::sleep(wait).await;
        if let Some(hub) = &app.review {
            let mut holds = lock(&hub.holds);
            if holds.get(&canonical) == Some(&end) {
                holds.remove(&canonical);
            }
        }
        let _ = tokio::task::spawn_blocking(move || push_state(&app, &canonical)).await;
    });
}

/// Sends the folder listing when it changed, at most once per [`DIR_PUSH_EVERY`].
pub(crate) fn push_dir(app: &Arc<AppState>) {
    let Some(hub) = &app.review else {
        return;
    };
    let mut push = lock(&hub.dir_push);
    if let Some(last) = push.last
        && last.elapsed() < DIR_PUSH_EVERY
    {
        if !push.trailing
            && let Some(rt) = hub.rt.as_ref()
        {
            push.trailing = true;
            let wait = DIR_PUSH_EVERY - last.elapsed();
            let app = Arc::clone(app);
            rt.spawn(async move {
                tokio::time::sleep(wait).await;
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(hub) = &app.review {
                        lock(&hub.dir_push).trailing = false;
                    }
                    push_dir(&app);
                })
                .await;
            });
        }
        return;
    }
    let frame = hub.dir.frame().to_string();
    let mut hasher = DefaultHasher::new();
    frame.hash(&mut hasher);
    let hash = hasher.finish();
    if push.hash == Some(hash) {
        return;
    }
    push.hash = Some(hash);
    push.last = Some(Instant::now());
    drop(push);
    let _ = app.hub.send(WsEvent::DirState(Arc::from(frame)));
}

/// Walks what the startup scan found into the listing and summarizes it in the
/// background, so serving starts at once; the listing is pushed when it is done.
pub(crate) fn seed(app: &Arc<AppState>, md_files: Vec<String>, truncated: bool) {
    if app.review.is_none() {
        return;
    }
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        if let Some(hub) = &app.review {
            let logged = hub.dir.seed(&md_files, truncated);
            hub.dir.summarize_all(&logged);
        }
        push_dir(&app);
    });
}

/// A doc was written or removed on disk, under the lexical path `rel`.
pub(crate) fn doc_event(app: &Arc<AppState>, rel: &str, canonical: &Path, removed: bool) {
    let Some(hub) = &app.review else {
        return;
    };
    if removed {
        if hub.dir.removed(canonical, rel) {
            push_dir(app);
        }
        return;
    }
    let log = Log::open(&review_path_for(canonical)).git_exclude(hub.git_exclude);
    let mut synced = false;
    if log_writable(app.roots.folder_root(canonical), canonical)
        && let Ok(mut held) = log.hold()
        && !held.parsed().read_only
    {
        sync::sync_external(&mut held, canonical, now_ms());
        synced = true;
    }
    // A doc with no review file still has a state on its new text
    if synced || app.viewers.wants(rel) {
        push_changed(app, canonical);
    }
    if hub
        .dir
        .wrote(canonical, rel, |path| app.viewers.count(path) > 0, now_ms())
    {
        push_dir(app);
    }
}

/// A page opened the doc at `rel`, which joins what a one-file server lists.
pub(crate) fn opened(app: &Arc<AppState>, canonical: &Path, rel: &str) {
    if let Some(hub) = &app.review
        && hub.dir.opened(canonical, rel)
    {
        hub.dir.refresh(canonical);
        push_dir(app);
    }
}

/// Whether a doc this server serves is out with the agent: handed back, or claimed and
/// awaiting its respond.
pub(crate) fn round_open(app: &AppState) -> bool {
    let Some(hub) = &app.review else {
        return false;
    };
    let open = |stage: &Stage| matches!(stage, Stage::Sent | Stage::PickedUp);
    hub.dir.served().iter().any(|doc| {
        hub.state_of(doc, true)
            .is_some_and(|s| open(&s.phrase.stage))
    }) || (hub.dir_mode && hub.dir.folder_stage().is_some_and(|s| open(&s)))
}

/// A review file changed on disk: an agent's line, or a write of our own.
pub(crate) fn review_file_event(app: &Arc<AppState>, review: &Path) {
    let Some(hub) = &app.review else {
        return;
    };
    // A round may have opened or closed while no tab is open
    if app.held.swap(false, Ordering::Relaxed) {
        app.all_disconnected.notify_one();
    }
    let Some(name) = review.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Some(doc) = name
        .strip_suffix(".birta.jsonl")
        .filter(|d| !d.is_empty())
        .map(|d| review.with_file_name(d))
    else {
        if hub.dir.refresh_folder() {
            push_dir(app);
        }
        return;
    };
    if doc.is_file() {
        push_changed(app, &doc);
    }
    if hub.dir.refresh(&doc) {
        push_dir(app);
    }
}

/// After an op landed: the doc's state to its pages, and the listing.
fn landed(app: &Arc<AppState>, doc: &Path, rel: &str) {
    push_state(app, doc);
    if let Some(hub) = &app.review {
        hub.dir.met(doc, rel);
        if hub.dir.refresh(doc) {
            push_dir(app);
        }
    }
}

// --- ops ---

#[derive(Debug, Deserialize)]
pub(crate) struct OpFrame {
    #[serde(default)]
    cid: Option<String>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    rev: Option<String>,
    #[serde(flatten)]
    op: WsOp,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WsOp {
    Comment {
        at: AnchorReq,
        #[serde(default)]
        verdict: Option<Verdict>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        kind: Option<CommentKind>,
        #[serde(default)]
        to: Option<u32>,
    },
    Amend {
        to: u32,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        verdict: Option<Verdict>,
        #[serde(default)]
        drop_verdict: bool,
    },
    Retract {
        to: u32,
        /// `{"k":"dir"}` takes back a note on the served folder, in the folder's review file.
        #[serde(default)]
        at: Option<AnchorReq>,
    },
    Reply {
        to: u32,
        #[serde(default)]
        verdict: Option<Verdict>,
        #[serde(default)]
        body: Option<String>,
    },
    Seen {
        ids: Vec<String>,
    },
    Unkeep {
        ids: Vec<String>,
    },
    HandBack {
        scope: HandBackScope,
    },
    Approve {},
    Withdraw {
        to: u32,
        /// The batch a hand-back of all was acked with, so a doc outside it can take it back.
        #[serde(default)]
        batch: Option<u64>,
    },
    Release {
        claim: u32,
    },
    Owner {
        drafted_by: Option<String>,
    },
    /// Pages may still send `confirm`; a revert never needs it.
    UndoChange {
        change: String,
    },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum HandBackScope {
    Doc,
    All,
}

impl WsOp {
    /// Whether the op stands on the doc as handed back while the doc is held.
    fn takes_held(&self) -> bool {
        matches!(
            self,
            Self::Comment { .. }
                | Self::Amend { .. }
                | Self::Retract { .. }
                | Self::Reply { .. }
                | Self::Seen { .. }
                | Self::Unkeep { .. }
        )
    }

    /// Whether the op means something only on the text the page saw.
    fn reads_text(&self) -> bool {
        match self {
            Self::Comment { at, .. } => !matches!(at, AnchorReq::Doc | AnchorReq::Dir),
            Self::Approve {} | Self::UndoChange { .. } => true,
            _ => false,
        }
    }
}

/// The reply to an `op` frame: `ack` or `nack`. `None` when the frame isn't one.
pub(crate) async fn op(app: &Arc<AppState>, msg: Value) -> Option<String> {
    let cid = msg.get("cid").cloned().unwrap_or(Value::Null);
    if app.review.is_none() {
        return Some(nack(&cid, "disabled"));
    }
    let Ok(frame) = serde_json::from_value::<OpFrame>(msg) else {
        #[cfg(debug_assertions)]
        eprintln!("birta: dropped an op frame that does not parse");
        return None;
    };
    let app = Arc::clone(app);
    let reply = tokio::task::spawn_blocking(move || apply(&app, frame, false))
        .await
        .unwrap_or(Err("read_only"));
    Some(match reply {
        Ok(ack) => {
            let mut frame = json!({"type": "ack", "cid": cid, "n": ack.n});
            if let Some(rev) = ack.rev {
                frame["rev"] = json!(rev.as_str());
            }
            if let Some(batch) = ack.batch {
                frame["batch"] = json!(batch);
                frame["not_before"] = json!(ack.not_before);
            }
            frame.to_string()
        }
        Err(reason) => nack(&cid, reason),
    })
}

fn nack(cid: &Value, reason: &str) -> String {
    json!({"type": "nack", "cid": cid, "reason": reason}).to_string()
}

#[derive(Default)]
struct Acked {
    n: u32,
    rev: Option<Rev>,
    /// A hand-back of all: the batch its submits share, and when its hold ends.
    batch: Option<u64>,
    not_before: Option<u64>,
}

fn apply(app: &Arc<AppState>, frame: OpFrame, beacon: bool) -> Result<Acked, Nack> {
    let hub = app.review.as_ref().ok_or("disabled")?;
    if beacon && !BEACON_OPS.contains(&frame.op_name()) {
        return Err("disabled");
    }
    let rel = match frame.path.as_str() {
        "" => app.initial_relpath.clone().ok_or("gone")?,
        rel => rel.to_string(),
    };
    let doc = app
        .roots
        .resolve_doc(&rel, Access::Write)
        .map_err(|e| match e {
            PathError::NotFound => "gone",
            _ => "read_only",
        })?;
    if !log_writable(doc.log_writable, &doc.canonical) {
        return Err("read_only");
    }
    let acked = match &frame.op {
        WsOp::Comment {
            at: AnchorReq::Dir, ..
        } => return folder_note(app, hub, &frame),
        WsOp::Retract {
            to,
            at: Some(AnchorReq::Dir),
        } => return folder_retract(app, hub, &frame, *to),
        WsOp::UndoChange { .. } if hub.is_held(&doc.canonical) => return Err("held"),
        WsOp::UndoChange { change } => undo_change(&doc, &frame, change)?,
        WsOp::HandBack {
            scope: HandBackScope::All,
        } if hub.dir_mode => hand_back_all(app, hub, &doc, &frame)?,
        WsOp::Withdraw { to, batch } if hub.dir_mode => {
            match batch.or_else(|| batch_of(&doc.review_path, *to)) {
                Some(batch) => withdraw_batch(app, hub, &doc, batch, &frame)?,
                None => keep(hub, &doc, &frame)?,
            }
        }
        _ => keep(hub, &doc, &frame)?,
    };
    landed(app, &doc.canonical, &doc.rel);
    Ok(acked)
}

impl OpFrame {
    fn op_name(&self) -> &'static str {
        match self.op {
            WsOp::Comment { .. } => "comment",
            WsOp::Amend { .. } => "amend",
            WsOp::Retract { .. } => "retract",
            WsOp::Reply { .. } => "reply",
            WsOp::Seen { .. } => "seen",
            WsOp::Unkeep { .. } => "unkeep",
            WsOp::HandBack { .. } => "hand_back",
            WsOp::Approve {} => "approve",
            WsOp::Withdraw { .. } => "withdraw",
            WsOp::Release { .. } => "release",
            WsOp::Owner { .. } => "owner",
            WsOp::UndoChange { .. } => "undo_change",
        }
    }

    fn client_rev(&self) -> Option<Rev> {
        self.rev.as_deref().and_then(Rev::parse)
    }
}

/// Appends the op to the doc's review file, creating the file for a first note.
fn keep(hub: &ReviewHub, doc: &DocRef, frame: &OpFrame) -> Result<Acked, Nack> {
    let log = Log::open(&doc.review_path).git_exclude(hub.git_exclude);
    let mut held = match log.hold() {
        Ok(held) => held,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            create_for(hub, doc, frame, &log)?;
            log.hold().map_err(|_| "read_only")?
        }
        Err(_) => return Err("read_only"),
    };
    append(hub, doc, frame, &mut held, None)
}

/// Makes the review file for an op that may start one, once the op is known to stand,
/// so a refused op leaves nothing behind.
fn create_for(hub: &ReviewHub, doc: &DocRef, frame: &OpFrame, log: &Log) -> Result<(), Nack> {
    match &frame.op {
        WsOp::Comment { to: Some(_), .. } => return Err("no_such_note"),
        WsOp::Comment { .. } | WsOp::Approve {} | WsOp::Owner { .. } => {}
        WsOp::HandBack { .. } => return Err("nothing_to_send"),
        _ => return Err("no_such_note"),
    }
    let bytes = sync::read_doc(&doc.canonical).map_err(|_| "gone")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "read_only")?;
    let disk = Rev::of(&bytes);
    if frame.op.reads_text() && frame.client_rev().as_ref() != Some(&disk) {
        return Err("changed_since");
    }
    if let WsOp::Comment { at, .. } = &frame.op {
        place(at, &Doc::new(text), &disk, None)?;
    }
    if !doc.log_writable {
        return Err("read_only");
    }
    let name = doc
        .canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    log.create_if_missing(&name, text, hub.drafted_by.as_deref())
        .map_err(|_| "read_only")?;
    Ok(())
}

/// The op's lines under the lock: the page's own replay first, then any write nobody
/// recorded, then the rev check and the op's guard.
fn append(
    hub: &ReviewHub,
    doc: &DocRef,
    frame: &OpFrame,
    held: &mut Held,
    batch: Option<u64>,
) -> Result<Acked, Nack> {
    if let Some(n) = frame
        .cid
        .as_deref()
        .and_then(|c| replayed(held.parsed(), c))
    {
        return Ok(Acked {
            n,
            ..Acked::default()
        });
    }
    if held.parsed().read_only {
        return Err("read_only");
    }
    let now = now_ms();
    let bytes = sync::read_doc(&doc.canonical).map_err(|_| "gone")?;
    sync::sync_bytes(held, &doc.canonical, &bytes, now);
    let text = String::from_utf8(bytes).map_err(|_| "read_only")?;
    let disk = Rev::of(text.as_bytes());
    let rev = frame.client_rev();
    // While held, the page shows the doc as handed back, so notes stand on that text
    let at_disk = rev.as_ref() == Some(&disk);
    let view = (frame.op.takes_held() && !(frame.op.reads_text() && at_disk))
        .then(|| held::view(held.parsed()))
        .flatten();
    if frame.op.reads_text() && view.is_none() {
        let rev = rev.as_ref().ok_or("changed_since")?;
        if *rev != disk {
            return Err(if recorded(held.parsed(), rev) {
                "changed_since"
            } else {
                "stale_rev"
            });
        }
    }
    let ops = match &view {
        Some(v) => {
            if frame.op.reads_text() && rev.as_ref() != Some(&v.at.rev) {
                return Err("changed_since");
            }
            build(hub, &frame.op, &v.log, &v.text, &v.at.rev, now, batch)?
        }
        None => build(hub, &frame.op, held.parsed(), &text, &disk, now, batch)?,
    };
    // A doc out with the agent is sent again only after its reply, never at its half-done text
    if matches!(frame.op, WsOp::HandBack { .. }) && held::at(held.parsed()).is_some() {
        return Err("held");
    }
    let ctx = AppendCtx {
        now_ms: now,
        by: human(),
        cid: frame.cid.clone(),
    };
    let lines = held.append_with(ctx, |_, _| Ok(ops)).map_err(|e| match e {
        AppendError::Rejected(r) => reject(&r),
        AppendError::Io(_) => "read_only",
    })?;
    let n = lines.last().copied().ok_or("read_only")?;
    Ok(Acked {
        n,
        ..Acked::default()
    })
}

/// The line an earlier send of this cid landed on, so a resend writes nothing.
fn replayed(parsed: &Parsed, cid: &str) -> Option<u32> {
    parsed
        .lines
        .iter()
        .filter(|l| l.cid.as_deref() == Some(cid) && l.by.kind == ByKind::Human)
        .filter(|l| !matches!(l.op, Op::Snap { .. }))
        .map(|l| l.n)
        .max()
}

/// Whether the log ever knew the doc at `rev`.
fn recorded(parsed: &Parsed, rev: &Rev) -> bool {
    parsed.lines.iter().any(|l| match &l.op {
        Op::Snap { rev: r, .. } | Op::Approve { rev: r, .. } => r == rev,
        Op::Edit {
            rev: a, to_rev: b, ..
        }
        | Op::External {
            rev: a, to_rev: b, ..
        } => a == rev || b == rev,
        _ => false,
    })
}

fn reject(r: &Reject) -> Nack {
    match r {
        Reject::AlreadyClaimed => "claimed",
        Reject::NotWithdrawable => "already_picked_up",
        Reject::NotYet | Reject::HoldPending => "hold_pending",
        Reject::NothingToSend => "nothing_to_send",
        Reject::StaleRev => "stale_rev",
        Reject::ReadOnly => "read_only",
        Reject::BadAnchor | Reject::Invalid(_) => "bad_anchor",
    }
}

fn build(
    hub: &ReviewHub,
    op: &WsOp,
    parsed: &Parsed,
    text: &str,
    disk: &Rev,
    now: u64,
    batch: Option<u64>,
) -> Result<Vec<Op>, Nack> {
    let hold = hub.hold_ms;
    Ok(match op {
        WsOp::Comment {
            at,
            verdict,
            body,
            kind,
            to,
        } => {
            if let Some(to) = to
                && !live_note(parsed, *to)
            {
                return Err("no_such_note");
            }
            let at = place(at, &Doc::new(text), disk, Some(parsed))?;
            vec![Op::Comment {
                at,
                verdict: *verdict,
                body: body.clone(),
                to: *to,
                kind: *kind,
                turn: None,
            }]
        }
        WsOp::Amend {
            to,
            body,
            verdict,
            drop_verdict,
        } => {
            if verdict.is_some() && *drop_verdict {
                return Err("bad_op");
            }
            editable(parsed, text, *to)?;
            vec![Op::Amend {
                to: *to,
                body: body.clone(),
                verdict: *verdict,
                drop_verdict: *drop_verdict,
            }]
        }
        WsOp::Retract { to, .. } => {
            editable(parsed, text, *to)?;
            vec![Op::Retract { to: *to }]
        }
        WsOp::Reply { to, verdict, body } => {
            if !live_note(parsed, *to) && !turn_note(parsed, *to) {
                return Err("no_such_note");
            }
            vec![Op::Reply {
                to: *to,
                verdict: *verdict,
                body: body.clone(),
                kind: None,
                late: false,
            }]
        }
        WsOp::Seen { ids } => vec![Op::Seen { ids: ids.clone() }],
        WsOp::Unkeep { ids } => {
            // A hand-back or approve settled what it checked, for good
            let news = fold_text(parsed, text).news;
            if ids.is_empty() {
                return Err("bad_op");
            }
            if !ids.iter().all(|id| news.iter().any(|n| &n.id == id)) {
                return Err("not_editable");
            }
            vec![Op::Unkeep { ids: ids.clone() }]
        }
        WsOp::HandBack { .. } => {
            let upto = parsed
                .lines
                .iter()
                .filter(|l| l.by.kind == ByKind::Human)
                .map(|l| l.n)
                .max()
                .unwrap_or(0);
            fold::scan(parsed, now)
                .check_submit(upto, now)
                .map_err(|r| reject(&r))?;
            let mut ops = Vec::with_capacity(2);
            // The doc stays held at this text until the reply, so the log must rebuild it
            if Chain::build(parsed).text(disk).is_none() {
                ops.push(Op::Snap {
                    rev: disk.clone(),
                    text: text.to_string(),
                    checkpoint: true,
                });
            }
            ops.push(Op::Submit {
                upto,
                not_before: now + hold,
                hold_ms: hold,
                to: None,
                batch,
            });
            ops
        }
        WsOp::Approve {} => log::approve_ops(&Chain::build(parsed), text, now + hold, hold),
        WsOp::Withdraw { to, .. } => {
            withdrawable(parsed, now, *to)?;
            vec![Op::Withdraw { to: *to }]
        }
        WsOp::Release { claim } => {
            let live = fold::scan(parsed, now)
                .rounds
                .iter()
                .any(|r| r.claim == Some(*claim) && r.yielded.is_none() && !r.released);
            if !live {
                return Err("claim_moved");
            }
            vec![Op::Yield {
                turn: Some(*claim),
                release: true,
            }]
        }
        WsOp::Owner { drafted_by } => vec![Op::Owner {
            drafted_by: drafted_by.clone(),
        }],
        WsOp::UndoChange { .. } => return Err("not_reversible"),
    })
}

/// Whether line `n` is a note nobody took back.
fn live_note(parsed: &Parsed, n: u32) -> bool {
    let note = parsed
        .lines
        .iter()
        .any(|l| l.n == n && matches!(l.op, Op::Comment { .. }));
    let retracted = parsed
        .lines
        .iter()
        .any(|l| matches!(l.op, Op::Retract { to } if to == n));
    note && !retracted
}

/// Whether line `n` is an agent's turn note, which takes replies like a note.
fn turn_note(parsed: &Parsed, n: u32) -> bool {
    parsed
        .lines
        .iter()
        .any(|l| l.n == n && l.by.kind == ByKind::Agent && matches!(l.op, Op::TurnNote { .. }))
}

/// A human note an agent hasn't picked up, or a human reply no hand-back has sent: only
/// those can be amended or taken back.
fn editable(parsed: &Parsed, text: &str, n: u32) -> Result<(), Nack> {
    let line = parsed
        .lines
        .iter()
        .find(|l| l.n == n && matches!(l.op, Op::Comment { .. } | Op::Reply { .. }))
        .ok_or("no_such_note")?;
    if matches!(line.op, Op::Reply { .. }) {
        return editable_reply(parsed, line);
    }
    if line.by.kind != ByKind::Human {
        return Err("not_editable");
    }
    let state = fold_text(parsed, text);
    let item = state
        .items
        .iter()
        .find(|i| i.n == n)
        .ok_or("no_such_note")?;
    match item.status {
        ItemStatus::Draft | ItemStatus::Sent | ItemStatus::Withdrawn => Ok(()),
        ItemStatus::Retracted => Err("no_such_note"),
        _ => Err("not_editable"),
    }
}

/// A reply the page can change: its own, as the nameless human, until a hand-back sends it.
fn editable_reply(parsed: &Parsed, line: &Line) -> Result<(), Nack> {
    let retracted = parsed
        .lines
        .iter()
        .any(|l| matches!(l.op, Op::Retract { to } if to == line.n));
    if retracted {
        return Err("no_such_note");
    }
    let sent = fold::scan(parsed, 0)
        .rounds
        .iter()
        .any(|r| r.withdrawn.is_none() && r.n > line.n && r.upto >= line.n);
    if line.by.kind != ByKind::Human || line.by.name != human().name || sent {
        return Err("not_editable");
    }
    Ok(())
}

/// A hand-back or approve can be taken back until an agent claims it or a later one.
fn withdrawable(parsed: &Parsed, now: u64, to: u32) -> Result<(), Nack> {
    let index = fold::scan(parsed, now);
    let i = index
        .rounds
        .iter()
        .position(|r| r.n == to && r.withdrawn.is_none())
        .ok_or("no_such_note")?;
    let round = &index.rounds[i];
    if round.claim.is_some() {
        return Err("claimed");
    }
    if round.released || index.rounds[i..].iter().any(|r| r.claim.is_some()) {
        return Err("already_picked_up");
    }
    Ok(())
}

/// What the page pointed at, as the log stores it. `parsed` places a note on a change.
fn place(req: &AnchorReq, doc: &Doc, rev: &Rev, parsed: Option<&Parsed>) -> Result<At, Nack> {
    match req {
        AnchorReq::Change { id } => {
            let parsed = parsed.ok_or("bad_anchor")?;
            change_anchor(parsed, doc, id)
        }
        AnchorReq::Block {
            sp,
            scope: BlockScope::Section,
        } if !doc
            .blocks
            .stops
            .iter()
            .any(|s| s.sp == *sp && s.kind == StopKind::Heading) =>
        {
            Err("bad_anchor")
        }
        req => crate::review::anchor_from_req(req, doc, rev).map_err(|r| match r {
            Reject::StaleRev => "changed_since",
            _ => "bad_anchor",
        }),
    }
}

/// A note on a change sits on the block holding the start of the change's text, or of the
/// text your revert put back; on the doc once that text is gone.
fn change_anchor(parsed: &Parsed, doc: &Doc, id: &str) -> Result<At, Nack> {
    let state = fold_text(parsed, &doc.text);
    let change = state
        .changes
        .iter()
        .find(|c| c.id == id)
        .ok_or("bad_anchor")?;
    let Some(stop) = fold::change_stop(&state.changes, doc, change) else {
        return Ok(At::Doc);
    };
    let req = AnchorReq::Block {
        sp: stop.sp.clone(),
        scope: BlockScope::Block,
    };
    crate::review::anchor_from_req(&req, doc, &doc.rev).map_err(|_| "bad_anchor")
}

/// `x` on a change: the strict write reverts it and records the undo under the lock.
fn undo_change(doc: &DocRef, frame: &OpFrame, change: &str) -> Result<Acked, Nack> {
    let rev = frame.client_rev().ok_or("changed_since")?;
    let cid = frame.cid.as_deref();
    let written = undo::reverse_change(doc, change, &rev, cid).map_err(|e| match e {
        undo::UndoErr::ChangedSince => "changed_since",
        undo::UndoErr::NotReversible => "not_reversible",
        undo::UndoErr::ReadOnly => "read_only",
        undo::UndoErr::Io(e) if e.kind() == io::ErrorKind::NotFound => "gone",
        undo::UndoErr::Io(_) => "read_only",
    })?;
    let parsed = Log::open(&doc.review_path)
        .read()
        .ok()
        .flatten()
        .ok_or("read_only")?;
    let n = parsed
        .lines
        .iter()
        .rev()
        .filter(|l| l.cid.as_deref() == cid)
        .find(|l| matches!(&l.op, Op::Edit { of: Some(of), .. } if of == change))
        .map(|l| l.n)
        .ok_or("read_only")?;
    Ok(Acked {
        n,
        rev: Some(written.rev),
        ..Acked::default()
    })
}

/// A hand-back of every doc under the root that has drafts, as one batch. Acked with
/// this doc's line when it had drafts, else the first that did.
fn hand_back_all(
    app: &Arc<AppState>,
    hub: &ReviewHub,
    doc: &DocRef,
    frame: &OpFrame,
) -> Result<Acked, Nack> {
    if let Some(acked) = frame
        .cid
        .as_deref()
        .and_then(|cid| replayed_batch(app, hub, doc, cid))
    {
        return Ok(acked);
    }
    let batch = now_ms();
    let mut own = Err("nothing_to_send");
    let mut first = None;
    let log = Log::open(&doc.review_path).git_exclude(hub.git_exclude);
    if let Ok(mut held) = log.hold() {
        own = append(hub, doc, frame, &mut held, Some(batch));
    }
    for other in hub.dir.logged() {
        if other == doc.canonical {
            continue;
        }
        let Some(rel) = hub.dir.path_of(&other) else {
            continue;
        };
        let Ok(other_doc) = app.roots.resolve_doc(&rel, Access::Write) else {
            continue;
        };
        if !log_writable(other_doc.log_writable, &other_doc.canonical) {
            continue;
        }
        let Ok(mut held) = Log::open(&other_doc.review_path).hold() else {
            continue;
        };
        let theirs = OpFrame {
            cid: frame.cid.clone(),
            path: rel.clone(),
            rev: None,
            op: WsOp::HandBack {
                scope: HandBackScope::All,
            },
        };
        if let Ok(acked) = append(hub, &other_doc, &theirs, &mut held, Some(batch)) {
            first.get_or_insert(acked.n);
            drop(held);
            landed(app, &other_doc.canonical, &rel);
        }
    }
    if let Some(n) = folder_hand_back(app, hub, frame, batch) {
        first.get_or_insert(n);
    }
    let n = match (own, first) {
        (Ok(acked), _) => acked.n,
        (Err(_), Some(n)) => n,
        (Err(reason), None) => return Err(reason),
    };
    Ok(Acked {
        n,
        batch: Some(batch),
        not_before: Some(batch + hub.hold_ms),
        ..Acked::default()
    })
}

/// What a hand-back of all sent with `cid` was acked with: its batch, read from a submit
/// it wrote, and this doc's line when it had one, else the first found.
fn replayed_batch(app: &AppState, hub: &ReviewHub, doc: &DocRef, cid: &str) -> Option<Acked> {
    let others = hub
        .dir
        .logged()
        .into_iter()
        .filter(|c| *c != doc.canonical)
        .map(|c| review_path_for(&c));
    let reviews = std::iter::once(doc.review_path.clone())
        .chain(others)
        .chain([app.roots.root().join(FOLDER_REVIEW)]);
    reviews
        .filter_map(|review| Log::open(&review).read().ok().flatten())
        .find_map(|parsed| {
            let n = replayed(&parsed, cid)?;
            parsed
                .lines
                .iter()
                .find(|l| l.n == n)
                .and_then(|l| match l.op {
                    Op::Submit {
                        batch: Some(batch),
                        hold_ms,
                        ..
                    } => Some(Acked {
                        n,
                        batch: Some(batch),
                        not_before: Some(batch + hold_ms),
                        ..Acked::default()
                    }),
                    _ => None,
                })
        })
}

/// The folder's own drafts, handed back in `batch`; the submit's line when there were any.
fn folder_hand_back(
    app: &Arc<AppState>,
    hub: &ReviewHub,
    frame: &OpFrame,
    batch: u64,
) -> Option<u32> {
    let mut held = Log::open(&app.roots.root().join(FOLDER_REVIEW))
        .hold()
        .ok()?;
    let acked = append_plain(&mut held, frame, |parsed| {
        build(
            hub,
            &frame.op,
            parsed,
            "",
            &Rev::of(b""),
            now_ms(),
            Some(batch),
        )
    })
    .ok()?;
    drop(held);
    if hub.dir.refresh_folder() {
        push_dir(app);
    }
    Some(acked.n)
}

/// A note on the whole folder, in the folder's own review file. Folder mode only: with
/// one file served, nothing would ever hand it back.
fn folder_note(app: &Arc<AppState>, hub: &ReviewHub, frame: &OpFrame) -> Result<Acked, Nack> {
    if !hub.dir_mode {
        return Err("bad_anchor");
    }
    let WsOp::Comment {
        verdict,
        body,
        kind,
        to,
        ..
    } = &frame.op
    else {
        return Err("bad_anchor");
    };
    if to.is_some() {
        return Err("no_such_note");
    }
    let root = app.roots.root();
    let log = Log::open(&root.join(FOLDER_REVIEW)).git_exclude(hub.git_exclude);
    let mut held = match log.hold() {
        Ok(held) => held,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !can_write(root) {
                return Err("read_only");
            }
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            log.create_if_missing(&name, "", None)
                .map_err(|_| "read_only")?;
            log.hold().map_err(|_| "read_only")?
        }
        Err(_) => return Err("read_only"),
    };
    let acked = append_plain(&mut held, frame, |_| {
        Ok(vec![Op::Comment {
            at: At::Dir,
            verdict: *verdict,
            body: body.clone(),
            to: None,
            kind: *kind,
            turn: None,
        }])
    })?;
    drop(held);
    if hub.dir.refresh_folder() {
        push_dir(app);
    }
    Ok(acked)
}

/// Takes back the page's note on line `to` of the folder's review file, as z does.
fn folder_retract(
    app: &Arc<AppState>,
    hub: &ReviewHub,
    frame: &OpFrame,
    to: u32,
) -> Result<Acked, Nack> {
    if !hub.dir_mode {
        return Err("bad_anchor");
    }
    let log = Log::open(&app.roots.root().join(FOLDER_REVIEW)).git_exclude(hub.git_exclude);
    let mut held = log.hold().map_err(|_| "no_such_note")?;
    let acked = append_plain(&mut held, frame, |parsed| {
        let note = parsed
            .lines
            .iter()
            .find(|l| l.n == to && matches!(l.op, Op::Comment { at: At::Dir, .. }))
            .ok_or("no_such_note")?;
        if note.by.kind != ByKind::Human {
            return Err("not_editable");
        }
        if !live_note(parsed, to) {
            return Err("no_such_note");
        }
        // A hand-back took it, until that hand-back is withdrawn
        let sent = fold::scan(parsed, 0)
            .rounds
            .iter()
            .any(|r| r.withdrawn.is_none() && r.n > to && r.upto >= to);
        if sent {
            return Err("not_editable");
        }
        Ok(vec![Op::Retract { to }])
    })?;
    drop(held);
    if hub.dir.refresh_folder() {
        push_dir(app);
    }
    Ok(acked)
}

/// Appends what `ops` builds as the page's op, with no doc to sync; a resent cid gets
/// the line it landed on before.
fn append_plain(
    held: &mut Held,
    frame: &OpFrame,
    ops: impl FnOnce(&Parsed) -> Result<Vec<Op>, Nack>,
) -> Result<Acked, Nack> {
    if let Some(n) = frame
        .cid
        .as_deref()
        .and_then(|c| replayed(held.parsed(), c))
    {
        return Ok(Acked {
            n,
            ..Acked::default()
        });
    }
    if held.parsed().read_only {
        return Err("read_only");
    }
    let ops = ops(held.parsed())?;
    let ctx = AppendCtx {
        now_ms: now_ms(),
        by: human(),
        cid: frame.cid.clone(),
    };
    let lines = held.append_with(ctx, |_, _| Ok(ops)).map_err(|e| match e {
        AppendError::Rejected(r) => reject(&r),
        AppendError::Io(_) => "read_only",
    })?;
    let n = lines.last().copied().ok_or("read_only")?;
    Ok(Acked {
        n,
        ..Acked::default()
    })
}

/// The batch of the hand-back on line `to` of the review file, if it had one.
fn batch_of(review: &Path, to: u32) -> Option<u64> {
    let parsed = Log::open(review).read().ok().flatten()?;
    parsed
        .lines
        .iter()
        .find(|l| l.n == to)
        .and_then(|l| match l.op {
            Op::Submit { batch, .. } => batch,
            _ => None,
        })
}

/// Takes back every hand-back of `batch`, the folder's and every doc's, and only those.
/// Acked with this doc's line when it had one in the batch, else the first.
fn withdraw_batch(
    app: &Arc<AppState>,
    hub: &ReviewHub,
    doc: &DocRef,
    batch: u64,
    frame: &OpFrame,
) -> Result<Acked, Nack> {
    let others: Vec<DocRef> = hub
        .dir
        .logged()
        .iter()
        .filter(|c| **c != doc.canonical)
        .filter_map(|c| hub.dir.path_of(c))
        .filter_map(|rel| app.roots.resolve_doc(&rel, Access::Write).ok())
        .filter(|d| log_writable(d.log_writable, &d.canonical))
        .collect();
    let docs: Vec<&DocRef> = std::iter::once(doc).chain(&others).collect();
    let folder = app.roots.root().join(FOLDER_REVIEW);
    let mut own = None;
    let mut first = None;
    let mut refused = None;
    let mut landed_docs = Vec::new();
    let targets = docs
        .iter()
        .map(|d| (d.review_path.clone(), Some(d)))
        .chain([(folder, None)]);
    for (review, of) in targets {
        let Ok(mut held) = Log::open(&review).hold() else {
            continue;
        };
        let now = now_ms();
        if let Some(d) = of {
            sync::sync_external(&mut held, &d.canonical, now);
        }
        let rounds: Vec<u32> = held
            .parsed()
            .lines
            .iter()
            .filter(|l| matches!(l.op, Op::Submit { batch: Some(b), .. } if b == batch))
            .map(|l| l.n)
            .collect();
        let mut wrote = None;
        for to in rounds {
            if let Some(n) = frame
                .cid
                .as_deref()
                .and_then(|c| replayed(held.parsed(), c))
            {
                wrote = Some(n);
                break;
            }
            match withdrawable(held.parsed(), now, to) {
                Ok(()) => {
                    let result = append_plain(&mut held, frame, |_| Ok(vec![Op::Withdraw { to }]));
                    match result {
                        Ok(acked) => wrote = Some(acked.n),
                        Err(reason) => {
                            refused.get_or_insert(reason);
                        }
                    }
                }
                Err("no_such_note") => {}
                Err(reason) => {
                    refused.get_or_insert(reason);
                }
            }
        }
        drop(held);
        let Some(n) = wrote else {
            continue;
        };
        first.get_or_insert(n);
        match of {
            Some(d) if d.canonical == doc.canonical => own = Some(n),
            Some(d) => landed_docs.push((d.canonical.clone(), d.rel.clone())),
            None => {
                if hub.dir.refresh_folder() {
                    push_dir(app);
                }
            }
        }
    }
    for (canonical, rel) in landed_docs {
        landed(app, &canonical, &rel);
    }
    match (own.or(first), refused) {
        (Some(n), _) => Ok(Acked {
            n,
            ..Acked::default()
        }),
        (None, Some(reason)) => Err(reason),
        (None, None) => Err("no_such_note"),
    }
}

/// The places a typed quote could mean, for the composer's preview.
pub(crate) async fn resolve(app: &Arc<AppState>, msg: &Value) -> String {
    let cid = msg.get("cid").cloned().unwrap_or(Value::Null);
    if app.review.is_none() {
        return nack(&cid, "disabled");
    }
    let rel = msg.get("path").and_then(Value::as_str).unwrap_or("");
    let text = msg
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Ok(doc) = app.roots.resolve_doc(rel, Access::Read) else {
        return nack(&cid, "gone");
    };
    let found = tokio::task::spawn_blocking(move || {
        let bytes = sync::read_doc(&doc.canonical).ok()?;
        let markdown = String::from_utf8(bytes).ok()?;
        Some(
            crate::anchor::search(&markdown, &text)
                .into_iter()
                .map(|c| {
                    let mut at = json!({"k": "quote", "q": text, "pre": "", "suf": ""});
                    if let Some(sp) = c.sp {
                        at["sp"] = json!(sp);
                    }
                    if let Some(off) = c.off {
                        at["off"] = json!(off);
                    }
                    json!({"at": at, "preview": c.preview})
                })
                .collect::<Vec<_>>(),
        )
    })
    .await
    .ok()
    .flatten();
    match found {
        Some(candidates) => {
            json!({"type": "resolved", "cid": cid, "candidates": candidates}).to_string()
        }
        None => nack(&cid, "gone"),
    }
}

// --- compose and the beacon ---

/// Keeps a half-typed note in the drafts folder, never beside the doc; never replies.
pub(crate) async fn compose(app: &Arc<AppState>, msg: &Value) {
    if app.review.is_none() {
        return;
    }
    let (Some(rel), Some(key), Some(text)) = (
        msg.get("path").and_then(Value::as_str),
        msg.get("key").and_then(Value::as_str),
        msg.get("text").and_then(Value::as_str),
    ) else {
        return;
    };
    // A doc reached only through a link out of the root keeps no drafts either
    let Ok(doc) = app
        .roots
        .resolve_doc(rel, Access::Read)
        .map_err(drop)
        .and_then(|d| if d.log_writable { Ok(d) } else { Err(()) })
    else {
        return;
    };
    let (app, key, text) = (Arc::clone(app), key.to_string(), text.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(hub) = &app.review
            && let Err(e) = hub.compose(&doc.canonical, &key, &text)
        {
            eprintln!("birta: could not keep a half-typed note: {e}");
        }
    })
    .await;
}

/// What a closing page could not send over its socket: seen marks and draft notes.
/// Returns the status to answer with.
pub(crate) async fn flush(app: &Arc<AppState>, body: &[u8]) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    let Some(hub) = &app.review else {
        return StatusCode::NOT_FOUND;
    };
    let Ok(msg) = serde_json::from_slice::<Value>(body) else {
        return StatusCode::BAD_REQUEST;
    };
    let token = msg.get("token").and_then(Value::as_str).unwrap_or("");
    if !same(token.as_bytes(), hub.token.as_bytes()) {
        return StatusCode::FORBIDDEN;
    }
    let path = msg.get("path").and_then(Value::as_str).unwrap_or("");
    let mut frames = Vec::new();
    let seen = msg.get("seen").and_then(Value::as_array);
    if let Some(ids) = seen.filter(|ids| !ids.is_empty()) {
        frames.push(json!({"path": path, "op": "seen", "ids": ids}));
    }
    for op in msg
        .get("ops")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if op.get("type").and_then(Value::as_str) == Some("op") {
            frames.push(op.clone());
        }
    }
    let app = Arc::clone(app);
    let _ = tokio::task::spawn_blocking(move || {
        for frame in frames {
            if let Ok(frame) = serde_json::from_value::<OpFrame>(frame) {
                let _ = apply(&app, frame, true);
            }
        }
    })
    .await;
    StatusCode::NO_CONTENT
}

/// Compares in time that doesn't depend on where the bytes first differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// --- assets ---

/// The review script: the modules in load order, then the shadow CSS as a string.
pub(crate) fn review_js() -> &'static str {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| {
        let parts = [
            include_str!("../assets/review/quote.js"),
            include_str!("../assets/review/00-core.js"),
            include_str!("../assets/review/10-keys.js"),
            include_str!("../assets/review/20-pointing.js"),
            include_str!("../assets/review/30-flow.js"),
            include_str!("../assets/review/40-lane.js"),
            include_str!("../assets/review/60-chrome.js"),
            include_str!("../assets/review/70-dir.js"),
            include_str!("../assets/review/80-life.js"),
        ];
        let mut js = parts.join("\n");
        let css = include_str!("../assets/review/shadow.css");
        js.push_str(&format!(
            "\nBR.shadowCss = {};\n",
            serde_json::to_string(css).unwrap_or_default()
        ));
        js
    })
}

pub(crate) const REVIEW_CSS: &str = include_str!("../assets/review/page.css");
