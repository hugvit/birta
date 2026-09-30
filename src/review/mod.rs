//! The review layer: the `<doc>.birta.jsonl` log, its fold, and the gate that hides it.

pub mod chain;
pub mod doc;
pub mod fold;
pub mod hunk;
pub mod log;
pub mod ops;
pub mod state;

use crate::config::Config;

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
