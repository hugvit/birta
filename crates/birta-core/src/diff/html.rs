use std::fmt::Write;
use std::ops::Range;

use super::{Diff, DiffOptions, Row, RowKind};
use crate::highlight::highlight_line_segments;
use crate::render::html_escape;

pub(super) fn render(
    diff: &Diff,
    old_marks: &[Vec<Range<usize>>],
    new_marks: &[Vec<Range<usize>>],
    oversized: bool,
    opts: &DiffOptions<'_>,
) -> String {
    let digits = diff
        .old_lines
        .len()
        .max(diff.new_lines.len())
        .max(1)
        .to_string()
        .len();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<div class=\"birta-diff\" data-layout=\"{}\"{} style=\"--birta-diff-digits:{digits}\">",
        opts.layout.as_str(),
        if diff.identical {
            " data-identical=\"true\""
        } else {
            ""
        },
    );
    if let Some([old_label, new_label]) = opts.labels
        && !diff.identical
    {
        let _ = writeln!(
            out,
            "<div class=\"diff-head\"><span class=\"diff-label diff-old\">{}</span><span class=\"diff-label diff-new\">{}</span></div>",
            html_escape(old_label),
            html_escape(new_label),
        );
    }
    let s = &opts.strings;
    for (set, key, text) in [
        (diff.line_endings_differ, "line-endings", s.line_endings),
        (diff.final_newline_differs, "final-newline", s.final_newline),
        (diff.bom_differs, "bom", s.bom),
        (diff.identical, "identical", s.identical),
    ] {
        if set {
            let _ = writeln!(
                out,
                "<div class=\"diff-note\" data-note=\"{key}\">{}</div>",
                html_escape(text)
            );
        }
    }
    if diff.identical {
        out.push_str("</div>");
        return out;
    }

    let old_html = side_html(&diff.old_lines, old_marks, "del", oversized, opts);
    let new_html = side_html(&diff.new_lines, new_marks, "ins", oversized, opts);
    let cells = Cells {
        old: &old_html,
        new: &new_html,
    };

    let mut i = 0;
    while i < diff.rows.len() {
        let row = &diff.rows[i];
        if row.kind == RowKind::Context && row.folded {
            let end = run_end(&diff.rows, i, |r| r.kind == RowKind::Context && r.folded);
            let run = &diff.rows[i..end];
            let _ = writeln!(
                out,
                "<details class=\"diff-fold\" data-old=\"{}\" data-new=\"{}\" data-count=\"{}\"><summary>{}</summary>",
                line_no(run[0].old),
                line_no(run[0].new),
                run.len(),
                html_escape(&s.fold.replace("{n}", &run.len().to_string())),
            );
            for r in run {
                cells.context(&mut out, r);
            }
            out.push_str("</details>\n");
            i = end;
        } else if row.kind == RowKind::Context {
            cells.context(&mut out, row);
            i += 1;
        } else {
            let id = row.hunk;
            let end = run_end(&diff.rows, i, |r| {
                r.kind != RowKind::Context && r.hunk == id
            });
            cells.hunk(&mut out, &diff.rows[i..end], diff);
            i = end;
        }
    }
    out.push_str("</div>");
    out
}

fn side_html(
    lines: &[String],
    marks: &[Vec<Range<usize>>],
    tag: &'static str,
    degraded: bool,
    opts: &DiffOptions<'_>,
) -> Vec<String> {
    if degraded {
        return lines
            .iter()
            .map(|l| html_escape(l).replace('\r', "&#13;"))
            .collect();
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    highlight_line_segments(&refs, opts.syntax_theme, marks, tag)
}

fn run_end(rows: &[Row], start: usize, same: impl Fn(&Row) -> bool) -> usize {
    rows[start..]
        .iter()
        .position(|r| !same(r))
        .map_or(rows.len(), |p| start + p)
}

fn line_no(index: Option<usize>) -> usize {
    index.map_or(0, |i| i + 1)
}

struct Cells<'a> {
    old: &'a [String],
    new: &'a [String],
}

impl Cells<'_> {
    fn context(&self, out: &mut String, row: &Row) {
        let (o, n) = (line_no(row.old), line_no(row.new));
        let _ = writeln!(
            out,
            "<div class=\"diff-ctx\" data-old=\"{o}\" data-new=\"{n}\">{}{}</div>",
            cell("diff-old", "", o, ' ', &self.old[o - 1]),
            cell("diff-new", "", n, ' ', &self.new[n - 1]),
        );
    }

    fn hunk(&self, out: &mut String, rows: &[Row], diff: &Diff) {
        let Some(hunk) = rows[0].hunk.map(|id| &diff.hunks[id]) else {
            return;
        };
        let height = hunk.old.len().max(hunk.new.len());
        let _ = writeln!(
            out,
            "<div class=\"diff-hunk\" data-hunk=\"{}\" data-kind=\"{}\" style=\"--rows:{height}\">",
            hunk.id,
            hunk.kind.as_str(),
        );
        for o in hunk.old.clone() {
            let no = o + 1;
            let attr = format!(" data-old=\"{no}\"");
            out.push_str(&cell("diff-del", &attr, no, '-', &self.old[o]));
            out.push('\n');
        }
        push_empty(out, height - hunk.old.len());
        for n in hunk.new.clone() {
            let no = n + 1;
            let attr = format!(" data-new=\"{no}\"");
            out.push_str(&cell("diff-add", &attr, no, '+', &self.new[n]));
            out.push('\n');
        }
        push_empty(out, height - hunk.new.len());
        out.push_str("</div>\n");
    }
}

fn cell(class: &str, attrs: &str, no: usize, sign: char, html: &str) -> String {
    format!(
        "<div class=\"diff-cell {class}\"{attrs}><span class=\"diff-num\">{no}</span><span class=\"diff-sign\">{sign}</span><span class=\"diff-text\">{html}</span></div>"
    )
}

fn push_empty(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push_str("<div class=\"diff-cell diff-empty\" aria-hidden=\"true\"></div>\n");
    }
}
