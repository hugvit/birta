//! `review-state/1`: the folded view of a review file that the page, the CLI and
//! `birta review --json` all read. Pure data; only the fold builds it.
//!
//! Fields documented as `| null` in the schema serialize as `null`; every other
//! optional field is left out when absent.

use serde::Serialize;

use super::ops::{By, CommentKind, Hunk, ReplyKind, Scope, To, Verdict};

#[derive(Clone, Debug, Serialize)]
pub struct ReviewState {
    pub schema: String,
    pub doc: String,
    pub rev: String,
    /// Epoch ms when the server built this, so clients can correct their clock for `hold`.
    pub server_now: u64,
    /// False when the doc can't be written: notes only.
    pub writable: bool,
    /// False when the doc's folder can't be written, so no review file can exist.
    pub log_writable: bool,
    pub hand_off: String,
    pub log: LogInfo,
    pub owner: OwnerState,
    pub phrase: Phrase,
    /// The phrase as if the pending hold were absent, shown until `hold.not_before`.
    pub phrase_before_hold: Option<Phrase>,
    pub hold: Option<Hold>,
    /// The hand-back the page's view is held at until its reply; the server sets it.
    pub held: Option<HeldState>,
    pub counts: Counts,
    pub items: Vec<Item>,
    pub changes: Vec<Change>,
    pub deletions: Vec<Deletion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<Rewrite>,
    pub later: Vec<u32>,
    pub lost: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approve: Option<ApproveState>,
    pub rounds: Vec<Round>,
    pub warnings: Vec<String>,
    /// Claude's news, in walk order, until each item is settled.
    pub news: Vec<News>,
}

/// One item of Claude's news: a change, a reply, a question or a row the reviewer checks.
#[derive(Clone, Debug, Serialize)]
pub struct News {
    /// As a `seen` op names it: `12.0`, `#15`, `nh:12`.
    pub id: String,
    pub kind: NewsKind,
    /// The stop it sits on, `null` for the whole doc and for a removal.
    pub sp: Option<String>,
    /// A removal's gap: the stop above it, `null` at the top. Only removals carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_sp: Option<Option<String>>,
    pub check: Option<Check>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewsKind {
    Change,
    Removal,
    Reply,
    Answer,
    Question,
    NotHandled,
    TurnNote,
}

/// How the reviewer checked an item of news.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Kept,
    Ok,
    Reverted,
    Restored,
    Replied,
    Answered,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogInfo {
    pub lines: u32,
    pub bad: u32,
    pub unknown_ops: u32,
    pub read_only: bool,
    pub baseline_available: bool,
    /// The snap line used as baseline when the original one is unusable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_from: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnerState {
    /// `null` means the doc is the human's own.
    pub drafted_by: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Phrase {
    pub stage: Stage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent: Option<Sent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picked_up: Option<PickedUp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied: Option<Replied>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    Drafts,
    Sent,
    PickedUp,
    Replied,
    Approved,
}

#[derive(Clone, Debug, Serialize)]
pub struct Sent {
    pub n: u32,
    pub ts: u64,
    pub items: u32,
    pub can_withdraw: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PickedUp {
    pub n: u32,
    pub ts: u64,
    pub by: String,
    pub silence: Silence,
}

#[derive(Clone, Debug, Serialize)]
pub struct Silence {
    pub since_ts: u64,
    pub threshold_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Replied {
    pub turn: u32,
    pub ts: u64,
    pub by: String,
    pub done: u32,
    pub answered: u32,
    pub didnt: u32,
    pub asked: u32,
    pub unasked: u32,
    pub not_handled: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The doc as handed back: the chain head `rev` at submit line `n`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HeldState {
    pub rev: String,
    pub n: u32,
}

/// A hand-back or approve written but not yet claimable.
#[derive(Clone, Debug, Serialize)]
pub struct Hold {
    pub n: u32,
    pub kind: HoldKind,
    pub count: u32,
    pub not_before: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldKind {
    HandBack,
    Approve,
}

#[derive(Clone, Debug, Serialize)]
pub struct Counts {
    pub drafts: u32,
    pub sent_unclaimed: u32,
    pub unseen: u32,
    pub unasked: u32,
    pub not_handled: u32,
    pub open_asks: u32,
    pub later: u32,
    pub lost: u32,
    pub items: u32,
    pub decided: u32,
    pub attention: u32,
    /// Stops holding news the reviewer hasn't checked.
    pub to_check: u32,
}

/// A note (human or agent) and what became of it.
#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub n: u32,
    /// `nh:<n>` on a not-handled row: the id a `seen` op marks it with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub op: String,
    pub by: By,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    pub ts: u64,
    pub at: AtState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CommentKind>,
    pub status: ItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub amended: Vec<u32>,
    pub replies: Vec<ReplyState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ItemOutcome>,
    /// Seen by its own id (`#<n>`) since the agent last answered or handled it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub seen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Draft,
    Sent,
    PickedUp,
    Handled,
    NotHandled,
    Open,
    Answered,
    Later,
    Retracted,
    Withdrawn,
}

/// Where a note points, re-located on the current text.
#[derive(Clone, Debug, Serialize)]
pub struct AtState {
    pub kind: AtKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<AnchorStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<(u32, u32)>,
    /// Source bytes in the current text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(usize, usize)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<(u32, u32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The change an orphaned note's text went into, which the page places it at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// `null` for doc and dir notes, and when the anchor can't be placed.
    pub loc: Option<Loc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AtKind {
    Doc,
    Dir,
    Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    Ok,
    Moved,
    Fuzzy,
    Orphaned,
    Suspended,
}

/// A place the page can paint: an owner's sourcepos and a code-point range in its text.
#[derive(Clone, Debug, Serialize)]
pub struct Loc {
    pub sp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off: Option<(usize, usize)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tl: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplyState {
    pub n: u32,
    /// `#<n>`, the id a `seen` op marks it with.
    pub id: String,
    pub by: By,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ReplyKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub ts: u64,
    pub seen: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemOutcome {
    pub done: bool,
    pub no_change: bool,
    pub changes: Vec<String>,
}

/// One hunk of a recorded write, with who made it and whether it was asked for.
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    /// `<op n>.<hunk index>`
    pub id: String,
    pub op: u32,
    pub idx: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub attr: Attr,
    pub by: By,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    pub old: (u32, u32),
    pub new: (u32, u32),
    pub range: ChangeRange,
    pub asked: Asked,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes_your_edit: Option<String>,
    pub reverts_your_edit: bool,
    pub breaks_stet: Vec<u32>,
    pub seen: bool,
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undone_by: Option<u32>,
    pub what: What,
    /// What kind of change it is and how big, as the page words it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<String>,
    pub is_rewrite: bool,
    /// The recorded hunk's removed and written lines, as raw markdown.
    pub old_text: String,
    pub new_text: String,
    pub check: Option<Check>,
    /// The body of the `unasked` line that names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unasked_why: Option<String>,
    /// An `unasked` line of its turn names it, so it was flagged rather than forgotten.
    #[serde(skip)]
    pub flagged: bool,
    /// The current text's lines holding what it wrote, or what later writes put over it.
    #[serde(skip)]
    pub span: Option<(u32, u32)>,
    /// The hunk that undoes this one, in the current text's lines; set only when reversible.
    #[serde(skip)]
    pub reverse: Option<Hunk>,
    pub reversible: Reversible,
    /// Hunks the op recorded, those that render the same included.
    pub op_hunks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attr {
    Agent,
    Human,
    Outside,
}

/// Where the change sits in the current text.
#[derive(Clone, Debug, Serialize)]
pub struct ChangeRange {
    pub status: RangeStatus,
    pub lines: (u32, u32),
    pub bytes: (usize, usize),
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sp: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeStatus {
    Ok,
    Moved,
    Superseded,
}

#[derive(Clone, Debug, Serialize)]
pub struct Asked {
    pub kind: AskedKind,
    pub notes: Vec<u32>,
    /// The reviewer's answers a `covers` cited it for, by reply line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AskedKind {
    Location,
    Cited,
    Unasked,
    Human,
    Outside,
}

#[derive(Clone, Debug, Serialize)]
pub struct What {
    pub old_lines: u32,
    pub new_lines: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<u32>,
    pub touches_html: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reversible {
    pub ok: bool,
    pub reason: ReversibleReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReversibleReason {
    LaterOverlap,
    Snap,
    Outside,
    None,
}

#[derive(Clone, Debug, Serialize)]
pub struct Deletion {
    pub change: String,
    pub kind: DeletionKind,
    pub at_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_byte: Option<usize>,
    pub removed: String,
    /// The surviving stop above the gap, `null` at the top of the doc.
    pub after_sp: Option<String>,
    /// A removed block's first line with text, markers kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_line: Option<String>,
    /// What whole blocks went, e.g. `section · 2 blocks`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The removed heading and its first block, or the first removed block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Removed stops after the head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionKind {
    Block,
    Words,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rewrite {
    pub turn: u32,
    /// Fraction of top-level blocks the turn touched.
    pub pct: f64,
    pub blocks_changed: u32,
    pub blocks_total: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApproveState {
    pub n: u32,
    pub ts: u64,
    pub rev: String,
    pub current_matches: bool,
    pub changed_since: bool,
    pub later: u32,
    pub can_withdraw: bool,
}

/// A review file in brief, for a directory listing: the phrase and the counts.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub stage: Stage,
    pub phrase: Phrase,
    pub counts: Counts,
    /// Human notes still standing, whether drafts, sent or handled (1.x).
    pub notes: u32,
}

impl Summary {
    pub fn of(state: ReviewState) -> Self {
        let notes = state
            .items
            .iter()
            .filter(|i| i.by.kind == crate::review::ops::ByKind::Human)
            .filter(|i| !matches!(i.status, ItemStatus::Retracted | ItemStatus::Withdrawn))
            .count() as u32;
        Summary {
            stage: state.phrase.stage,
            phrase: state.phrase,
            counts: state.counts,
            notes,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Round {
    pub submit: u32,
    pub ts: u64,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<To>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<u64>,
    pub items: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#yield: Option<u32>,
    pub released: bool,
}
