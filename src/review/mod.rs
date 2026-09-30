//! The review layer: the `<doc>.birta.jsonl` log, its fold, and the gate that hides it.

pub mod chain;
pub mod doc;
pub mod fold;
pub mod hunk;
pub mod ladder;
pub mod log;
pub mod ops;
pub mod state;

use crate::anchor::{self, Reason, ResolveReq, Resolved};
use crate::config::Config;
use crate::rev::Rev;
use doc::Doc;
use ops::{Anchor, AnchorReq, At, BlockScope, Scope};

/// Code points of a block's source a block anchor keeps.
const BLOCK_QUOTE: usize = 60;

/// Line 1 of every review file names this schema.
pub const SCHEMA_LOG: &str = "review/1";
pub const SCHEMA_STATE: &str = "review-state/1";

/// Review is on when `[review] enabled = true`, or forced by `BIRTA_REVIEW=1`.
pub fn enabled(config: &Config) -> bool {
    config.review.enabled || std::env::var_os("BIRTA_REVIEW").is_some_and(|v| v == "1")
}

/// Why a review op was refused. Checked under the log lock, so a refusal writes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    /// The round, or a later one that took its notes, has a claim.
    AlreadyClaimed,
    /// A claim names the round, or it was never a round or is already withdrawn.
    NotWithdrawable,
    /// The round's hold hasn't passed.
    NotYet,
    /// No human note since the last hand-back.
    NothingToSend,
    /// A hand-back is still in its hold.
    HoldPending,
    /// The op's rev isn't in the log's chain.
    StaleRev,
    /// The log is from a later schema version, or has no header.
    ReadOnly,
    BadAnchor,
    Invalid(String),
}

/// Turns what the page or CLI points at into what the log stores. `rev` is the rev the
/// request was made on; the anchor's lines are in `doc`, so the two must agree.
pub fn anchor_from_req(req: &AnchorReq, doc: &Doc, rev: &Rev) -> Result<At, Reject> {
    let stops = &doc.blocks.stops;
    match req {
        AnchorReq::Doc => Ok(At::Doc),
        AnchorReq::Dir => Ok(At::Dir),
        AnchorReq::Change { .. } => Err(Reject::Invalid(
            "a change anchor is placed by the fold".to_string(),
        )),
        _ if *rev != doc.rev => Err(Reject::StaleRev),
        AnchorReq::Block { sp, scope } => {
            let i = stops
                .iter()
                .position(|s| s.sp == *sp)
                .ok_or(Reject::BadAnchor)?;
            block_anchor(doc, i, *scope == BlockScope::Section, None)
                .map(At::Anchor)
                .ok_or(Reject::BadAnchor)
        }
        AnchorReq::Quote {
            sp: Some(sp),
            q,
            pre,
            suf,
            off,
            tl,
        } => {
            let req = ResolveReq {
                sp: sp.clone(),
                quote: q.clone(),
                pre: pre.clone(),
                suf: suf.clone(),
                off: *off,
                tl: *tl,
            };
            match anchor::resolve(&doc.text, &req) {
                Resolved::Exact { lines, bytes, .. } => {
                    Ok(At::Anchor(quote_anchor(doc, bytes, lines, q)))
                }
                Resolved::Block {
                    reason: Reason::BadSp,
                    ..
                } => Err(Reject::BadAnchor),
                // Never a guess between occurrences: the block, with what was selected
                Resolved::Block { .. } | Resolved::Candidates { .. } => {
                    let owner = doc
                        .blocks
                        .owners
                        .iter()
                        .find(|o| o.sp == *sp)
                        .ok_or(Reject::BadAnchor)?;
                    let (first, last) = owner.lines;
                    let holder = (0..stops.len())
                        .rev()
                        .find(|&i| stops[i].lines.0 <= first && stops[i].lines.1 >= last)
                        .or_else(|| preceding(doc, first));
                    Ok(holder
                        .and_then(|i| block_anchor(doc, i, false, Some(q)))
                        .map_or(At::Doc, At::Anchor))
                }
            }
        }
        // Raw HTML has no owner, so only a search of the whole doc can place it
        AnchorReq::Quote { sp: None, q, .. } => {
            let hits = anchor::search(&doc.text, q);
            if let [hit] = &hits[..] {
                let last = hit.bytes.end.max(hit.bytes.start + 1) - 1;
                let lines = (doc.lines.line_of(hit.bytes.start), doc.lines.line_of(last));
                return Ok(At::Anchor(quote_anchor(doc, hit.bytes.clone(), lines, q)));
            }
            let mut before = hits
                .iter()
                .map(|hit| preceding(doc, doc.lines.line_of(hit.bytes.start)));
            let first = before.next().flatten();
            let shared = first.filter(|&i| before.all(|other| other == Some(i)));
            Ok(shared
                .and_then(|i| block_anchor(doc, i, false, Some(q)))
                .map_or(At::Doc, At::Anchor))
        }
    }
}

fn quote_anchor(
    doc: &Doc,
    bytes: std::ops::Range<usize>,
    lines: (u32, u32),
    shown: &str,
) -> Anchor {
    let (pre, suf) = ladder::context(&doc.text, &bytes);
    Anchor {
        quote: doc.text[bytes].to_string(),
        pre,
        suf,
        lines,
        rev: doc.rev.clone(),
        scope: Scope::Quote,
        shown: Some(shown.to_string()),
    }
}

/// An anchor on stop `i`: its lines, or its heading's section, and its opening source.
fn block_anchor(doc: &Doc, i: usize, section: bool, shown: Option<&str>) -> Option<Anchor> {
    let stop = &doc.blocks.stops[i];
    let lines = if section { stop.section? } else { stop.lines };
    let bytes = anchor::sp_bytes(&doc.text, [stop.sp.as_str()])
        .pop()
        .flatten()?;
    Some(Anchor {
        quote: doc.text[bytes].chars().take(BLOCK_QUOTE).collect(),
        pre: String::new(),
        suf: String::new(),
        lines,
        rev: doc.rev.clone(),
        scope: if section {
            Scope::Section
        } else {
            Scope::Block
        },
        shown: shown.map(str::to_string),
    })
}

/// The last stop starting at or before `line`.
fn preceding(doc: &Doc, line: u32) -> Option<usize> {
    doc.blocks
        .stops
        .partition_point(|s| s.lines.0 <= line)
        .checked_sub(1)
}
