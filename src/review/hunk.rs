//! Line hunks: how one text became another, byte for byte.

use std::fmt;

use similar::{Algorithm, DiffTag, TextDiff};

use super::ops::Hunk;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// Hunks must be sorted and must not overlap.
    Unordered { a: u32 },
    /// The hunk's old lines are not the text's lines at `a`.
    Mismatch { a: u32 },
    /// The result would join or split lines, so its hunks would no longer line up.
    NotLines,
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unordered { a } => write!(f, "hunk at line {a} is out of order"),
            Self::Mismatch { a } => write!(f, "hunk at line {a} does not match the text"),
            Self::NotLines => f.write_str("hunk lines are not whole lines"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// The hunks that turn `old` into `new`. Myers with no deadline, so the same texts
/// always give the same hunks and change ids stay stable.
pub fn diff(old: &str, new: &str) -> Vec<Hunk> {
    let (old, new) = (lines(old), lines(new));
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&old, &new);
    diff.ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| Hunk {
            a: op.old_range().start as u32,
            d: old[op.old_range()].iter().map(|s| s.to_string()).collect(),
            i: new[op.new_range()].iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}

/// Applies `hunks`, all in `text`'s line numbers, checking every old line.
pub fn apply(text: &str, hunks: &[Hunk]) -> Result<String, ApplyError> {
    let old = lines(text);
    let mut out: Vec<&str> = Vec::with_capacity(old.len());
    let mut next = 0;
    for hunk in hunks {
        let a = hunk.a as usize;
        if a < next {
            return Err(ApplyError::Unordered { a: hunk.a });
        }
        let kept = old
            .get(a..a + hunk.d.len())
            .ok_or(ApplyError::Mismatch { a: hunk.a })?;
        if !kept.iter().eq(hunk.d.iter()) {
            return Err(ApplyError::Mismatch { a: hunk.a });
        }
        out.extend(&old[next..a]);
        out.extend(hunk.i.iter().map(String::as_str));
        next = a + hunk.d.len();
    }
    out.extend(&old[next..]);
    let result = out.concat();
    // A new line without its terminator, or a lone CR meeting an LF, would change the split
    if lines(&result) != out {
        return Err(ApplyError::NotLines);
    }
    Ok(result)
}

/// The hunks that undo `hunks`, in the new text's line numbers.
pub fn reverse(hunks: &[Hunk]) -> Vec<Hunk> {
    let mut shift = 0i64;
    hunks
        .iter()
        .map(|hunk| {
            let a = (i64::from(hunk.a) + shift) as u32;
            shift += hunk.i.len() as i64 - hunk.d.len() as i64;
            Hunk {
                a,
                d: hunk.i.clone(),
                i: hunk.d.clone(),
            }
        })
        .collect()
}

/// Lines with their terminators, split at `\n`, `\r\n` and a lone `\r` as CommonMark does.
pub(crate) fn lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => i + 2,
            b'\r' | b'\n' => i + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        lines.push(&text[start..end]);
        (start, i) = (end, end);
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}
