use std::fs;

use birta_core::diff::{DiffOptions, diff};

#[test]
fn snapshot_diff_fixtures() {
    insta::glob!("fixtures/diff/*.old.md", |old_path| {
        let new_path = old_path.with_file_name(
            old_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace(".old.md", ".new.md"),
        );
        let old = fs::read_to_string(old_path).unwrap();
        let new = fs::read_to_string(&new_path).unwrap();
        let result = diff(&old, &new, &DiffOptions::default());
        if !result.identical {
            assert_eq!(side(&result.html, "diff-del", "diff-old"), result.old_lines);
            assert_eq!(side(&result.html, "diff-add", "diff-new"), result.new_lines);
        }
        insta::assert_snapshot!("html", result.html);
        insta::assert_debug_snapshot!(
            "data",
            (
                &result.hunks,
                &result.stats,
                result.identical,
                result.line_endings_differ,
                result.final_newline_differs,
                result.bom_differs,
                result.degraded,
            )
        );
    });
}

/// One side's lines rebuilt from the HTML: its changed cells plus its half of each context row.
fn side(html: &str, changed: &str, context: &str) -> Vec<String> {
    let mut found = Vec::new();
    for class in [changed, context] {
        let marker = format!("<div class=\"diff-cell {class}\"");
        for (at, _) in html.match_indices(&marker) {
            let open = "<span class=\"diff-text\">";
            let start = at + html[at..].find(open).unwrap() + open.len();
            let end = start + html[start..].find("</span></div>").unwrap();
            found.push((at, text_of(&html[start..end])));
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, text)| text).collect()
}

fn text_of(html: &str) -> String {
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
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}
