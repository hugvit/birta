//! Line diffs of markdown source, as data and as themed HTML.
//!
//! [`diff`] compares two texts line by line, marks the changed words inside
//! changed lines, and renders the result with the same syntax highlighting as
//! [`render_source`](crate::render::render_source). The HTML lays out as a
//! unified or a side-by-side view from the same markup (see [`Layout`]); include
//! [`css::DIFF`](crate::css::DIFF) to style it.
//!
//! ```
//! use birta_core::diff::{DiffOptions, diff};
//!
//! let d = diff("one\ntwo\n", "one\n2\n", &DiffOptions::default());
//! assert_eq!(d.hunks.len(), 1);
//! assert_eq!(d.hunks[0].old, 1..2);
//! assert!(d.html.contains("data-hunk=\"0\""));
//! ```

mod html;

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, ChangeTag, DiffTag, TextDiff};

use crate::theme::SyntaxTheme;

/// Lines longer than this get no word marks.
const WORD_DIFF_MAX_LINE: usize = 10_000;
/// Line pairs less similar than this get no word marks; the whole row is coloured already.
const WORD_DIFF_MIN_RATIO: f32 = 0.5;

/// How [`diff`] compares and renders.
#[derive(Clone, Copy)]
pub struct DiffOptions<'a> {
    /// Inline colours from this theme; `None` emits CSS classes for
    /// [`css::SYNTAX`](crate::css::SYNTAX) or a host's own syntax CSS. Hosts that
    /// switch light/dark in the browser want `None`.
    pub syntax_theme: Option<&'a SyntaxTheme>,
    /// Unchanged lines to show around each change; the rest fold away.
    /// `None` shows every line.
    pub context: Option<usize>,
    /// Mark the changed words inside changed lines.
    pub words: bool,
    pub line_endings: LineEndings,
    /// Time budget for the line diff and the word marks. When it runs out the
    /// diff is still correct, but may not be minimal.
    pub deadline: Option<Duration>,
    /// Inputs larger than this many bytes (either side) skip highlighting and word marks.
    pub highlight_limit: usize,
    /// The layout the HTML starts in; a host can change `data-layout` later.
    pub layout: Layout,
    /// Column labels for the old and the new side, e.g. `["Theirs", "Yours"]`.
    pub labels: Option<[&'a str; 2]>,
    pub strings: DiffStrings<'a>,
}

impl Default for DiffOptions<'_> {
    fn default() -> Self {
        DiffOptions {
            syntax_theme: None,
            context: Some(3),
            words: true,
            line_endings: LineEndings::Normalize,
            deadline: Some(Duration::from_millis(250)),
            highlight_limit: 512 * 1024,
            layout: Layout::Auto,
            labels: None,
            strings: DiffStrings::default(),
        }
    }
}

/// The text the HTML shows. `fold` has `{n}` replaced by the number of lines.
#[derive(Clone, Copy, Debug)]
pub struct DiffStrings<'a> {
    pub fold: &'a str,
    pub identical: &'a str,
    pub line_endings: &'a str,
    pub final_newline: &'a str,
    pub bom: &'a str,
}

impl Default for DiffStrings<'_> {
    fn default() -> Self {
        DiffStrings {
            fold: "{n} unchanged lines",
            identical: "No differences.",
            line_endings: "Line endings differ; compared without them.",
            final_newline: "Only one side ends with a newline.",
            bom: "Only one side starts with a byte order mark.",
        }
    }
}

/// How the diff HTML is laid out. `Auto` is side by side when the diff is at
/// least 48rem wide and unified below that.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Auto,
    Unified,
    Split,
}

impl Layout {
    fn as_str(self) -> &'static str {
        match self {
            Layout::Auto => "auto",
            Layout::Unified => "unified",
            Layout::Split => "split",
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEndings {
    /// Ignore CRLF vs LF, a leading byte order mark and a missing final
    /// newline, and report each through a flag on [`Diff`].
    #[default]
    Normalize,
    /// Compare bytes as given.
    Exact,
}

/// The result of [`diff`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub html: String,
    pub hunks: Vec<Hunk>,
    /// Every line in unified display order, folded ones included.
    pub rows: Vec<Row>,
    /// The compared lines, without line endings; [`Hunk`] and [`Row`] index into these.
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub stats: DiffStats,
    /// Equal after normalisation.
    pub identical: bool,
    pub line_endings_differ: bool,
    /// Set only when both sides are non-empty.
    pub final_newline_differs: bool,
    pub bom_differs: bool,
    /// Highlighting and word marks were skipped because an input was too large.
    pub degraded: bool,
    /// The deadline had passed by the time the diff finished, so the line diff or
    /// some word marks may be less precise than they could be.
    pub timed_out: bool,
}

/// One change: a maximal run of changed lines. Ranges are 0-based, half-open
/// line ranges into [`Diff::old_lines`] and [`Diff::new_lines`]; an empty range
/// is the position of a pure insertion or deletion.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub id: usize,
    pub old: Range<usize>,
    pub new: Range<usize>,
    pub kind: HunkKind,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Removed,
    Changed,
}

impl HunkKind {
    fn as_str(self) -> &'static str {
        match self {
            HunkKind::Added => "added",
            HunkKind::Removed => "removed",
            HunkKind::Changed => "changed",
        }
    }
}

/// One displayed line.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    /// The [`Hunk::id`] of a changed line.
    pub hunk: Option<usize>,
    /// 0-based index into [`Diff::old_lines`].
    pub old: Option<usize>,
    /// 0-based index into [`Diff::new_lines`].
    pub new: Option<usize>,
    /// Changed byte ranges within the line.
    pub marks: Vec<Range<usize>>,
    /// Hidden in a fold.
    pub folded: bool,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Deleted,
    Added,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
}

/// Compare `old` with `new` line by line.
pub fn diff(old: &str, new: &str, opts: &DiffOptions<'_>) -> Diff {
    let deadline = opts.deadline.map(|d| Instant::now() + d);
    let norm = normalise(old, new, opts.line_endings);

    // Compare with line endings, so `Exact` sees a missing final newline
    let old_keys: Vec<&str> = norm.old.split_inclusive('\n').collect();
    let new_keys: Vec<&str> = norm.new.split_inclusive('\n').collect();
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Myers);
    if let Some(d) = deadline {
        config.deadline(d);
    }
    let text_diff = config.diff_slices(&old_keys, &new_keys);
    let ops = text_diff.ops();

    let old_lines: Vec<String> = old_keys
        .iter()
        .map(|l| strip_newline(l).to_string())
        .collect();
    let new_lines: Vec<String> = new_keys
        .iter()
        .map(|l| strip_newline(l).to_string())
        .collect();

    let hunks = build_hunks(ops);
    let stats = DiffStats {
        added: hunks.iter().map(|h| h.new.len()).sum(),
        removed: hunks.iter().map(|h| h.old.len()).sum(),
    };
    let degraded = old.len() > opts.highlight_limit || new.len() > opts.highlight_limit;
    let (old_marks, new_marks) = if opts.words && !degraded {
        word_marks(&hunks, &old_lines, &new_lines, deadline)
    } else {
        (
            vec![Vec::new(); old_lines.len()],
            vec![Vec::new(); new_lines.len()],
        )
    };
    let timed_out = deadline.is_some_and(|d| Instant::now() >= d);
    let rows = build_rows(ops, &hunks, &old_marks, &new_marks, opts.context);

    let mut result = Diff {
        html: String::new(),
        identical: hunks.is_empty(),
        hunks,
        rows,
        old_lines,
        new_lines,
        stats,
        line_endings_differ: norm.line_endings_differ,
        final_newline_differs: norm.final_newline_differs,
        bom_differs: norm.bom_differs,
        degraded,
        timed_out,
    };
    result.html = html::render(&result, &old_marks, &new_marks, opts);
    result
}

struct Normalised {
    old: String,
    new: String,
    line_endings_differ: bool,
    final_newline_differs: bool,
    bom_differs: bool,
}

fn normalise(old: &str, new: &str, mode: LineEndings) -> Normalised {
    if mode == LineEndings::Exact {
        return Normalised {
            old: old.to_string(),
            new: new.to_string(),
            line_endings_differ: false,
            final_newline_differs: false,
            bom_differs: false,
        };
    }
    let (old, old_bom) = strip_bom(old);
    let (new, new_bom) = strip_bom(new);
    let (old_crlf, new_crlf) = (old.contains("\r\n"), new.contains("\r\n"));
    let (mut old, mut new) = (old.to_string(), new.to_string());
    let final_newline_differs =
        !old.is_empty() && !new.is_empty() && old.ends_with('\n') != new.ends_with('\n');
    // Before the CRLF pass, so a final lone `\r` reads as a CRLF missing its `\n`
    if final_newline_differs {
        for text in [&mut old, &mut new] {
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
    }
    let old = old.replace("\r\n", "\n");
    let new = new.replace("\r\n", "\n");
    Normalised {
        old,
        new,
        line_endings_differ: old_crlf != new_crlf,
        final_newline_differs,
        bom_differs: old_bom != new_bom,
    }
}

fn strip_bom(text: &str) -> (&str, bool) {
    match text.strip_prefix('\u{FEFF}') {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

fn strip_newline(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

fn build_hunks(ops: &[similar::DiffOp]) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut open: Option<(Range<usize>, Range<usize>)> = None;
    for op in ops {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            if let Some((o, n)) = open.take() {
                hunks.push(make_hunk(hunks.len(), o, n));
            }
            continue;
        }
        open = Some(match open {
            Some((o, n)) => (o.start..old.end, n.start..new.end),
            None => (old, new),
        });
    }
    if let Some((o, n)) = open {
        hunks.push(make_hunk(hunks.len(), o, n));
    }
    hunks
}

fn make_hunk(id: usize, old: Range<usize>, new: Range<usize>) -> Hunk {
    let kind = match (old.is_empty(), new.is_empty()) {
        (true, _) => HunkKind::Added,
        (_, true) => HunkKind::Removed,
        _ => HunkKind::Changed,
    };
    Hunk { id, old, new, kind }
}

type Marks = Vec<Vec<Range<usize>>>;

/// Word marks for line pairs, paired by position within each hunk as the split view shows them.
fn word_marks(
    hunks: &[Hunk],
    old_lines: &[String],
    new_lines: &[String],
    deadline: Option<Instant>,
) -> (Marks, Marks) {
    let mut old_marks = vec![Vec::new(); old_lines.len()];
    let mut new_marks = vec![Vec::new(); new_lines.len()];
    for hunk in hunks {
        for (o, n) in hunk.old.clone().zip(hunk.new.clone()) {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return (old_marks, new_marks);
            }
            let (a, b) = (old_lines[o].as_str(), new_lines[n].as_str());
            if a.len() > WORD_DIFF_MAX_LINE || b.len() > WORD_DIFF_MAX_LINE {
                continue;
            }
            let mut config = TextDiff::configure();
            if let Some(d) = deadline {
                config.deadline(d);
            }
            let words = config.diff_unicode_words(a, b);
            if words.ratio() < WORD_DIFF_MIN_RATIO {
                continue;
            }
            let (mut old_at, mut new_at) = (0, 0);
            for change in words.iter_all_changes() {
                let len = change.value().len();
                match change.tag() {
                    ChangeTag::Equal => {
                        old_at += len;
                        new_at += len;
                    }
                    ChangeTag::Delete => {
                        push_range(&mut old_marks[o], old_at..old_at + len);
                        old_at += len;
                    }
                    ChangeTag::Insert => {
                        push_range(&mut new_marks[n], new_at..new_at + len);
                        new_at += len;
                    }
                }
            }
        }
    }
    (old_marks, new_marks)
}

fn push_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}

fn build_rows(
    ops: &[similar::DiffOp],
    hunks: &[Hunk],
    old_marks: &[Vec<Range<usize>>],
    new_marks: &[Vec<Range<usize>>],
    context: Option<usize>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut hunk_iter = hunks.iter().peekable();
    let mut hunks_done = 0;
    for op in ops {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            // A run is a file edge when no change lies on that side of it
            let first = hunks_done == 0;
            let last = hunks_done == hunks.len();
            let len = old.len();
            let shown = visible_context(len, first, last, context);
            for (i, (o, n)) in old.zip(new).enumerate() {
                rows.push(Row {
                    kind: RowKind::Context,
                    hunk: None,
                    old: Some(o),
                    new: Some(n),
                    marks: Vec::new(),
                    folded: !shown(i),
                });
            }
            continue;
        }
        // A hunk spans one or more consecutive non-equal ops; emit it once, at its first op
        let Some(hunk) =
            hunk_iter.next_if(|h| h.old.start == old.start && h.new.start == new.start)
        else {
            continue;
        };
        hunks_done += 1;
        for o in hunk.old.clone() {
            rows.push(changed_row(
                RowKind::Deleted,
                hunk.id,
                Some(o),
                None,
                &old_marks[o],
            ));
        }
        for n in hunk.new.clone() {
            rows.push(changed_row(
                RowKind::Added,
                hunk.id,
                None,
                Some(n),
                &new_marks[n],
            ));
        }
    }
    rows
}

fn changed_row(
    kind: RowKind,
    hunk: usize,
    old: Option<usize>,
    new: Option<usize>,
    marks: &[Range<usize>],
) -> Row {
    Row {
        kind,
        hunk: Some(hunk),
        old,
        new,
        marks: marks.to_vec(),
        folded: false,
    }
}

/// Which lines of an unchanged run of `len` lines stay visible.
fn visible_context(
    len: usize,
    at_start: bool,
    at_end: bool,
    context: Option<usize>,
) -> impl Fn(usize) -> bool {
    move |i| {
        let Some(n) = context else {
            return true;
        };
        match (at_start, at_end) {
            (true, true) => false,
            (true, false) => len <= n || i >= len - n,
            (false, true) => i < n,
            (false, false) => len <= 2 * n || i < n || i >= len - n,
        }
    }
}

#[cfg(test)]
mod tests;
