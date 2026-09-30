//! A doc's text as the review fold sees it: its rev, its lines and its blocks.

use std::ops::Range;
use std::sync::Arc;

use comrak::{Arena, parse_document};

use super::hunk;
use crate::anchor::{self, BlockIndex};
use crate::rev::Rev;

pub struct Doc {
    /// The bytes as on disk, BOM included.
    pub text: Arc<str>,
    pub rev: Rev,
    pub lines: LineIndex,
    pub blocks: BlockIndex,
}

impl Doc {
    pub fn new(text: &str) -> Self {
        let arena = Arena::new();
        // comrak skips a leading BOM itself, so sourcepos match the page's
        let root = parse_document(&arena, text, &anchor::parse_options());
        Self {
            text: Arc::from(text),
            rev: Rev::of(text.as_bytes()),
            lines: LineIndex::new(text),
            blocks: anchor::index(root, text),
        }
    }
}

/// Lines numbered as hunks and sourcepos number them.
pub struct LineIndex {
    /// Bytes of each line, without its terminator or a leading BOM.
    ranges: Vec<Range<usize>>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut start = 0;
        let ranges = hunk::lines(text)
            .into_iter()
            .map(|line| {
                let body = line
                    .strip_suffix("\r\n")
                    .or_else(|| line.strip_suffix(['\n', '\r']))
                    .unwrap_or(line);
                let bom = if start == 0 && body.starts_with('\u{feff}') {
                    '\u{feff}'.len_utf8()
                } else {
                    0
                };
                let range = start + bom..start + body.len();
                start += line.len();
                range
            })
            .collect();
        Self { ranges }
    }

    pub fn count(&self) -> u32 {
        self.ranges.len() as u32
    }

    /// Bytes of 1-based `line`, without its terminator.
    pub fn range(&self, line: u32) -> Option<Range<usize>> {
        let i = line.checked_sub(1)?;
        self.ranges.get(i as usize).cloned()
    }
}
