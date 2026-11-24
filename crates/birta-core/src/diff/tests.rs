use proptest::prelude::*;

use super::*;

fn d(old: &str, new: &str) -> Diff {
    diff(old, new, &DiffOptions::default())
}

fn exact(old: &str, new: &str) -> Diff {
    diff(
        old,
        new,
        &DiffOptions {
            line_endings: LineEndings::Exact,
            ..Default::default()
        },
    )
}

#[test]
fn identical_inputs_have_no_hunks() {
    let r = d("a\nb\n", "a\nb\n");
    assert!(r.identical);
    assert!(r.hunks.is_empty());
    assert!(r.html.contains("data-identical=\"true\""));
    assert!(r.html.contains("data-note=\"identical\""));
}

#[test]
fn empty_to_one_line_is_a_pure_insertion() {
    let r = d("", "a\n");
    assert_eq!(r.hunks.len(), 1);
    assert_eq!(r.hunks[0].old, 0..0);
    assert_eq!(r.hunks[0].new, 0..1);
    assert_eq!(r.hunks[0].kind, HunkKind::Added);
}

#[test]
fn crlf_against_lf_is_identical_and_flagged() {
    let r = d("a\r\nb\r\n", "a\nb\n");
    assert!(r.identical && r.line_endings_differ);
    assert!(r.html.contains("data-note=\"line-endings\""));
    assert_eq!(exact("a\r\nb\r\n", "a\nb\n").hunks.len(), 1);
}

#[test]
fn missing_final_newline_is_identical_and_flagged() {
    let r = d("a\n", "a");
    assert!(r.identical && r.final_newline_differs);
    let r = d("", "\n");
    assert_eq!(r.hunks.len(), 1);
    assert_eq!(r.hunks[0].kind, HunkKind::Added);
    assert!(!r.final_newline_differs);
    assert_eq!(exact("a\n", "a").hunks.len(), 1);
}

#[test]
fn bom_is_ignored_and_flagged() {
    let r = d("\u{FEFF}a\n", "a\n");
    assert!(r.identical && r.bom_differs);
    assert!(r.html.contains("data-note=\"bom\""));
}

#[test]
fn lone_carriage_return_is_content() {
    let r = d("a\rb\n", "a\nb\n");
    assert_eq!(r.hunks.len(), 1);
    assert_eq!(r.old_lines, vec!["a\rb"]);
    assert!(r.html.contains("&#13;"));
}

#[test]
fn final_lone_carriage_return_reads_as_crlf() {
    assert!(d("a\r", "a\r\n").identical);
    let r = d("a\r\nb\r", "a\nb\n");
    assert!(r.identical && r.final_newline_differs && r.line_endings_differ);
}

#[test]
fn context_before_an_appended_line_stays_visible() {
    let r = d("a\nb\nc\n", "a\nb\nc\nd\n");
    assert!(r.rows.iter().all(|row| !row.folded));
    let old: String = (0..20).map(|i| format!("l{i}\n")).collect();
    let r = d(&old, &format!("{old}x\n"));
    let shown: Vec<usize> = r
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Context && !row.folded)
        .filter_map(|row| row.old)
        .collect();
    assert_eq!(shown, [17, 18, 19]);
}

#[test]
fn context_after_a_prepended_line_stays_visible() {
    let old: String = (0..20).map(|i| format!("l{i}\n")).collect();
    let r = d(&old, &format!("x\n{old}"));
    let shown: Vec<usize> = r
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Context && !row.folded)
        .filter_map(|row| row.old)
        .collect();
    assert_eq!(shown, [0, 1, 2]);
}

#[test]
fn separate_edits_stay_separate_hunks() {
    let old: String = (1..=12).map(|i| format!("line {i}\n")).collect();
    let new = old
        .replace("line 2\n", "line two\n")
        .replace("line 9\n", "line nine\n");
    let r = d(&old, &new);
    assert_eq!(r.hunks.len(), 2);
    assert_eq!(
        (r.hunks[0].old.clone(), r.hunks[1].old.clone()),
        (1..2, 8..9)
    );
    assert_eq!(
        r.stats,
        DiffStats {
            added: 2,
            removed: 2
        }
    );
}

#[test]
fn rows_reference_hunk_lines() {
    let r = d("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
    for hunk in &r.hunks {
        let rows: Vec<&Row> = r
            .rows
            .iter()
            .filter(|row| row.hunk == Some(hunk.id))
            .collect();
        let deleted: Vec<usize> = rows.iter().filter_map(|row| row.old).collect();
        let added: Vec<usize> = rows.iter().filter_map(|row| row.new).collect();
        assert_eq!(deleted, hunk.old.clone().collect::<Vec<_>>());
        assert_eq!(added, hunk.new.clone().collect::<Vec<_>>());
    }
    for row in r.rows.iter().filter(|row| row.kind == RowKind::Context) {
        assert_eq!(r.old_lines[row.old.unwrap()], r.new_lines[row.new.unwrap()]);
    }
}

#[test]
fn word_marks_cover_the_changed_word() {
    let r = d("the quick brown fox\n", "the quick red fox\n");
    let deleted = r
        .rows
        .iter()
        .find(|row| row.kind == RowKind::Deleted)
        .unwrap();
    let added = r
        .rows
        .iter()
        .find(|row| row.kind == RowKind::Added)
        .unwrap();
    assert_eq!(&r.old_lines[0][deleted.marks[0].clone()], "brown");
    assert_eq!(&r.new_lines[0][added.marks[0].clone()], "red");
    assert!(r.html.contains("<del class=\"diff-word\">brown</del>"));
    assert!(r.html.contains("<ins class=\"diff-word\">red</ins>"));
}

#[test]
fn dissimilar_lines_get_no_word_marks() {
    let r = d("alpha beta gamma\n", "one two three\n");
    assert!(r.rows.iter().all(|row| row.marks.is_empty()));
}

#[test]
fn oversized_input_is_degraded_but_correct() {
    let opts = DiffOptions {
        highlight_limit: 4,
        ..Default::default()
    };
    let r = diff("hello\n", "help\n", &opts);
    assert!(r.degraded);
    assert_eq!(r.hunks.len(), 1);
    assert!(!r.html.contains("diff-word"));
}

#[test]
fn html_escapes_input_and_strings() {
    let opts = DiffOptions {
        labels: Some(["<a>", "\"b\""]),
        ..Default::default()
    };
    let r = diff("<script>x</script>\n", "<b>\n", &opts);
    // Highlighting may split the escaped tag across spans; no raw tag may survive
    assert!(!r.html.contains("<script") && !r.html.contains("<b>"));
    assert!(r.html.contains("&lt;"));
    assert!(r.html.contains("&lt;a&gt;") && r.html.contains("&quot;b&quot;"));
}

#[test]
fn layout_attribute_follows_options() {
    let opts = DiffOptions {
        layout: Layout::Split,
        ..Default::default()
    };
    assert!(
        diff("a\n", "b\n", &opts)
            .html
            .contains("data-layout=\"split\"")
    );
    assert!(d("a\n", "b\n").html.contains("data-layout=\"auto\""));
}

#[test]
fn hunk_cells_are_in_unified_order_with_padding() {
    let r = d("x\na\nb\nc\ny\n", "x\nA\ny\n");
    let hunk = &r.hunks[0];
    assert_eq!((hunk.old.len(), hunk.new.len()), (3, 1));
    let start = r.html.find("<div class=\"diff-hunk\"").unwrap();
    let body = &r.html[start..r.html[start..].find("</div>\n</div>").unwrap() + start];
    assert!(body.contains("style=\"--rows:3\""));
    let order: Vec<&str> = body
        .match_indices("diff-cell diff-")
        .map(|(i, _)| {
            let rest = &body[i + "diff-cell diff-".len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(order, ["del", "del", "del", "add", "empty", "empty"]);
}

#[test]
fn long_unchanged_runs_fold() {
    let old: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    let new = old.replace("line 20\n", "line twenty\n");
    let r = d(&old, &new);
    // 19 lines before the edit keep 3 visible, 20 after keep 3
    assert!(r.html.contains("data-count=\"16\""), "{}", r.html);
    assert!(r.html.contains("data-count=\"17\""));
    assert!(r.html.contains("<summary>16 unchanged lines</summary>"));
    assert_eq!(r.rows.iter().filter(|row| row.folded).count(), 33);
    let all = diff(
        &old,
        &new,
        &DiffOptions {
            context: None,
            ..Default::default()
        },
    );
    assert!(!all.html.contains("diff-fold"));
}

#[test]
fn fold_text_comes_from_strings() {
    let opts = DiffOptions {
        strings: DiffStrings {
            fold: "{n} óbreyttar línur",
            ..Default::default()
        },
        ..Default::default()
    };
    let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
    let r = diff(&old, &old.replace("10\n", "tíu\n"), &opts);
    assert!(r.html.contains("óbreyttar línur</summary>"));
}

/// Rebuild one side's lines from the HTML: its changed cells plus its half of each context row.
fn side_from_html(html: &str, changed: &str, context: &str) -> Vec<String> {
    let mut found = Vec::new();
    for class in [changed, context] {
        let marker = format!("<div class=\"diff-cell {class}\"");
        for (at, _) in html.match_indices(&marker) {
            let text_at = at + html[at..].find("<span class=\"diff-text\">").unwrap();
            let start = text_at + "<span class=\"diff-text\">".len();
            let end = start + html[start..].find("</span></div>").unwrap();
            found.push((at, unescape(&strip_tags(&html[start..end]))));
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, text)| text).collect()
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}

const PAIRS: &[(&str, &str)] = &[
    (
        "Hæ þú, hvað segirðu?\nÆðislegt veður.\n",
        "Hæ þú, hvað segir þú?\nÆðislegt veður!\n",
    ),
    ("emoji 👋🏽 here\nsame\n", "emoji 👋 here\nsame\n"),
    ("cafe\u{301} au lait\n", "café au lait\n"),
    ("<b>&amp;</b> \"q\"\n", "<i>&amp;</i> \"q\"\n"),
    (
        "---\ntitle: a\n---\n# H\n\n```rust\nfn a() {}\n```\n",
        "---\ntitle: b\n---\n# H\n\n```rust\nfn b() {}\n```\n",
    ),
    ("a\r\nb\r\n", "a\nc\n"),
    (
        "| Dagur | Hlaup |\n|---|---|\n| Mán | 5 km |\n",
        "| Dagur | Hlaup |\n|---|---|\n| Mán | 6 km |\n| Þri | 3 km |\n",
    ),
    ("", "only new\n"),
    ("only old\n", ""),
];

#[test]
fn html_reconstructs_both_sides() {
    let theme = crate::highlight::tests_support::default_syntax_theme();
    for &(old, new) in PAIRS {
        for syntax_theme in [None, Some(&theme)] {
            let opts = DiffOptions {
                syntax_theme,
                context: Some(1),
                ..Default::default()
            };
            let r = diff(old, new, &opts);
            if r.identical {
                continue;
            }
            assert_eq!(
                side_from_html(&r.html, "diff-del", "diff-old"),
                r.old_lines,
                "{}",
                r.html
            );
            assert_eq!(
                side_from_html(&r.html, "diff-add", "diff-new"),
                r.new_lines,
                "{}",
                r.html
            );
        }
    }
}

fn splice(old: &[String], new: &[String], hunks: &[Hunk]) -> Vec<String> {
    let mut lines = old.to_vec();
    for hunk in hunks.iter().rev() {
        lines.splice(hunk.old.clone(), new[hunk.new.clone()].iter().cloned());
    }
    lines
}

#[test]
fn applying_every_hunk_gives_the_new_side() {
    for &(old, new) in PAIRS {
        for mode in [LineEndings::Normalize, LineEndings::Exact] {
            let r = diff(
                old,
                new,
                &DiffOptions {
                    line_endings: mode,
                    ..Default::default()
                },
            );
            assert_eq!(splice(&r.old_lines, &r.new_lines, &r.hunks), r.new_lines);
        }
    }
}

proptest! {
    #[test]
    fn applying_every_hunk_gives_the_new_side_for_random_lines(
        old in prop::collection::vec("[abc]{0,2}", 0..12),
        new in prop::collection::vec("[abc]{0,2}", 0..12),
        crlf in any::<bool>(),
    ) {
        let sep = if crlf { "\r\n" } else { "\n" };
        let old_text: String = old.iter().map(|l| format!("{l}{sep}")).collect();
        let new_text: String = new.iter().map(|l| format!("{l}\n")).collect();
        let r = d(&old_text, &new_text);
        prop_assert_eq!(splice(&r.old_lines, &r.new_lines, &r.hunks), r.new_lines.clone());
        prop_assert_eq!(r.identical, old == new);
        prop_assert_eq!(r.old_lines.clone(), old);
    }
}
