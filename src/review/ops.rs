//! The `review/1` log schema: one JSON op per line after a header line.
//!
//! Lines are parsed in two steps (strict JSON value, then the named op) so one bad
//! or future line never hides the rest of the log.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::SCHEMA_LOG;
use crate::rev::Rev;

const ABOUT: &str = "ops after the last hand-back are drafts; don't act on them";

/// Every op name this schema knows. Anything else is a future op, counted and skipped.
const OPS: &[&str] = &[
    "snap",
    "comment",
    "reply",
    "amend",
    "retract",
    "submit",
    "withdraw",
    "claim",
    "done",
    "covers",
    "unasked",
    "turn_note",
    "yield",
    "read",
    "suggest",
    "apply",
    "reject",
    "seen",
    "owner",
    "approve",
    "edit",
    "external",
];

/// Line 1 of a review file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub birta: String,
    pub doc: String,
    pub about: String,
}

impl Header {
    pub fn new(doc: &str) -> Self {
        Self {
            birta: SCHEMA_LOG.to_string(),
            doc: doc.to_string(),
            about: ABOUT.to_string(),
        }
    }

    /// Editors and sync tools may prepend a BOM; a header with a malformed version is `None`.
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let Strict(value) = serde_json::from_str(raw).ok()?;
        let header = Self::deserialize(&value).ok()?;
        header.major().is_some().then_some(header)
    }

    /// `review/<major>` or `review/<major>.<minor>`, plain digits without leading zeros.
    fn major(&self) -> Option<u32> {
        let number = |s: &str| {
            let plain = !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s == "0" || !s.starts_with('0'));
            plain.then(|| s.parse::<u32>().ok()).flatten()
        };
        let version = self.birta.strip_prefix("review/")?;
        let (major, minor) = match version.split_once('.') {
            Some((major, minor)) => (major, Some(minor)),
            None => (version, None),
        };
        if let Some(minor) = minor {
            number(minor)?;
        }
        number(major)
    }
}

/// One op line: the envelope plus the op. `n` is always the physical line number.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Line {
    pub n: u32,
    /// Epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<u64>,
    pub by: By,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct By {
    #[serde(rename = "type")]
    pub kind: ByKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByKind {
    Human,
    Agent,
    Birta,
    /// A hand-appended line with no `by`.
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Browser,
    Nvim,
    Cli,
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The baseline on line 2, or a checkpoint of the full text.
    Snap {
        rev: Rev,
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        checkpoint: bool,
    },
    /// A note; with no verdict it is a plain note. An agent `ask` is a blank for the human.
    Comment {
        at: At,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verdict: Option<Verdict>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// The op this note answers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<CommentKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u32>,
    },
    Reply {
        to: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verdict: Option<Verdict>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ReplyKind>,
        /// Written by a released claim after the human took the round back.
        #[serde(default, skip_serializing_if = "is_false")]
        late: bool,
    },
    Amend {
        to: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verdict: Option<Verdict>,
    },
    Retract {
        to: u32,
    },
    /// A hand-back of human ops up to line `upto`, claimable from `not_before`.
    Submit {
        upto: u32,
        not_before: u64,
        hold_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<To>,
        /// Shared by the submits of one directory-wide hand-back.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch: Option<u64>,
    },
    /// Takes back a submit or approve; valid only while no claim names it.
    Withdraw {
        to: u32,
    },
    Claim {
        round: u32,
    },
    Done {
        to: u32,
        turn: u32,
    },
    Covers {
        to: u32,
        quote: String,
        turn: u32,
    },
    Unasked {
        quote: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        turn: u32,
    },
    TurnNote {
        body: String,
        turn: u32,
    },
    /// Ends an agent turn, or with `release` a human takes a stalled round back.
    Yield {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u32>,
        #[serde(default, skip_serializing_if = "is_false")]
        release: bool,
    },
    /// An agent's read cursor.
    Read {
        #[serde(rename = "ref")]
        cursor: ReadRef,
        rev: Rev,
    },
    /// Reserved: parsed and kept, not produced yet.
    Suggest {
        at: At,
        with: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<String>,
    },
    /// Reserved.
    Apply {
        to: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    /// Reserved.
    Reject {
        to: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    /// Ids: `"12.0"` a change, `"#15"` a reply or blank, `"nh:12"` a not-handled row.
    Seen {
        ids: Vec<String>,
    },
    /// `null` means the doc is the human's own.
    Owner {
        drafted_by: Option<String>,
    },
    Approve {
        rev: Rev,
        not_before: u64,
        hold_ms: u64,
    },
    /// A write birta made itself. `patch` is `null` when the doc is too large to diff.
    Edit {
        rev: Rev,
        to_rev: Rev,
        patch: Option<Vec<Hunk>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<String>,
        act: Act,
    },
    /// A write birta observed but did not make.
    External {
        rev: Rev,
        to_rev: Rev,
        patch: Option<Vec<Hunk>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<String>,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRef {
    pub n: u32,
    /// Short hash of line `n`, so a rewritten log is noticed.
    pub h: String,
}

/// What a note points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "AtRepr", into = "AtRepr")]
pub enum At {
    Doc,
    Dir,
    Anchor(Anchor),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum AtRepr {
    Name(AtName),
    Anchor(Anchor),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AtName {
    Doc,
    Dir,
}

impl From<AtRepr> for At {
    fn from(repr: AtRepr) -> Self {
        match repr {
            AtRepr::Name(AtName::Doc) => At::Doc,
            AtRepr::Name(AtName::Dir) => At::Dir,
            AtRepr::Anchor(anchor) => At::Anchor(anchor),
        }
    }
}

impl From<At> for AtRepr {
    fn from(at: At) -> Self {
        match at {
            At::Doc => AtRepr::Name(AtName::Doc),
            At::Dir => AtRepr::Name(AtName::Dir),
            At::Anchor(anchor) => AtRepr::Anchor(anchor),
        }
    }
}

/// A place in the doc, stored as source text so it can be found again after edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub quote: String,
    pub pre: String,
    pub suf: String,
    /// 1-based inclusive lines in the text of `rev`.
    pub lines: (u32, u32),
    pub rev: Rev,
    pub scope: Scope,
    /// The rendered text the reader selected, for display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Quote,
    Block,
    Section,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Yes,
    No,
    Ask,
    Later,
    YourCall,
    Rewrite,
    Stet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    Aside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyKind {
    Answer,
    Didnt,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    Undo,
    Checkbox,
    Edit,
}

/// Who a hand-back is for: `"*"`, one name, or several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum To {
    One(String),
    Many(Vec<String>),
}

/// A line hunk: at 0-based line `a` of the old text, lines `d` become `i`.
/// Lines keep their terminators, so text round-trips byte for byte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub a: u32,
    pub d: Vec<String>,
    pub i: Vec<String>,
}

/// What the page asks to anchor a note to. The server turns it into an [`Anchor`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum AnchorReq {
    Doc,
    Dir,
    Block {
        sp: String,
        scope: BlockScope,
    },
    /// No `sp` when the selection sits inside raw HTML.
    Quote {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sp: Option<String>,
        q: String,
        pre: String,
        suf: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        off: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tl: Option<usize>,
    },
    Change {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockScope {
    Block,
    Section,
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "short-lived, and nearly every line is `Ok`"
)]
pub enum ParsedLine {
    Ok(Line),
    Unknown(UnknownLine),
    Bad(BadLine),
}

/// A line naming an op this schema doesn't know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownLine {
    pub n: u32,
    pub op: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadLine {
    pub n: u32,
    pub err: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub header: Option<Header>,
    pub lines: Vec<Line>,
    pub bad: Vec<BadLine>,
    pub unknown: Vec<UnknownLine>,
    /// Set when the header is missing, malformed or from a later major version.
    pub read_only: bool,
    /// The last line has no newline and doesn't parse: a write that was cut short.
    pub torn_tail: bool,
}

#[derive(Deserialize)]
struct Envelope {
    /// Checked for type only: the physical position is the line's number.
    #[serde(default, rename = "n")]
    _n: Option<u32>,
    #[serde(default)]
    ts: Option<u64>,
    #[serde(default)]
    by: Option<By>,
    #[serde(default)]
    cid: Option<String>,
}

/// Parses op line `n`. Never fails: a line is an op, a future op, or bad.
pub fn parse_line(n: u32, raw: &str) -> ParsedLine {
    let bad = |err: String| ParsedLine::Bad(BadLine { n, err });
    let value = match serde_json::from_str::<Strict>(raw) {
        Ok(Strict(value)) => value,
        Err(e) => return bad(e.to_string()),
    };
    let op = match value.get("op") {
        Some(Value::String(op)) if !op.is_empty() => op.clone(),
        _ => return bad("no op name".to_string()),
    };
    let envelope = match Envelope::deserialize(&value) {
        Ok(envelope) => envelope,
        Err(e) => return bad(e.to_string()),
    };
    if !OPS.contains(&op.as_str()) {
        return ParsedLine::Unknown(UnknownLine { n, op });
    }
    match Op::deserialize(&value) {
        Ok(parsed) => ParsedLine::Ok(Line {
            n,
            ts: envelope.ts,
            by: envelope.by.unwrap_or_default(),
            cid: envelope.cid,
            op: parsed,
        }),
        Err(e) => bad(e.to_string()),
    }
}

/// Parses a whole review file. Invalid UTF-8 spoils only the line it is on.
pub fn parse(bytes: impl AsRef<[u8]>) -> Parsed {
    let bytes = bytes.as_ref();
    let mut parsed = Parsed::default();
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let mut last = 0;
    if !bytes.is_empty() {
        for (i, raw) in body.split(|&b| b == b'\n').enumerate() {
            let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
            last = n;
            if n == 1 {
                match line_str(raw).and_then(Header::parse) {
                    Some(header) => parsed.header = Some(header),
                    None => parsed.bad.push(BadLine {
                        n,
                        err: "not a review header".to_string(),
                    }),
                }
                continue;
            }
            match parse_raw_line(n, raw) {
                ParsedLine::Ok(line) => parsed.lines.push(line),
                ParsedLine::Unknown(unknown) => parsed.unknown.push(unknown),
                ParsedLine::Bad(bad) => parsed.bad.push(bad),
            }
        }
    }
    parsed.read_only = parsed.header.as_ref().and_then(Header::major) != Some(1);
    parsed.torn_tail =
        !bytes.ends_with(b"\n") && parsed.bad.last().is_some_and(|bad| bad.n == last);
    parsed
}

/// Parses op line `n` as it sits on disk, without its `\n`.
pub(crate) fn parse_raw_line(n: u32, raw: &[u8]) -> ParsedLine {
    match line_str(raw) {
        Some(raw) => parse_line(n, raw),
        None => ParsedLine::Bad(BadLine {
            n,
            err: "invalid UTF-8".to_string(),
        }),
    }
}

fn line_str(raw: &[u8]) -> Option<&str> {
    std::str::from_utf8(raw.strip_suffix(b"\r").unwrap_or(raw)).ok()
}

/// A file of at most one line that is not a later version's header: what a creation
/// cut short leaves behind, so it counts as no file at all.
pub(crate) fn is_unfinished(bytes: &[u8]) -> bool {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if body.contains(&b'\n') {
        return false;
    }
    line_str(body)
        .and_then(Header::parse)
        .is_none_or(|header| header.major() == Some(1))
}

/// A JSON value that refuses duplicate object keys, which `Value` would resolve last-wins.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(v.into())
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut fields = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let Strict(value) = map.next_value()?;
            if fields.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }
}
