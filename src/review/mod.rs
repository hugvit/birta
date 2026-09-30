//! The review layer: the `<doc>.birta.jsonl` log, its fold, and the gate that hides it.

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
