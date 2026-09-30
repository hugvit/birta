use birta::anchor::{Owner, OwnerKind};
use birta::render;
use birta::review::doc::Doc;
use birta::review::hunk;
use birta::review::ops::Hunk;

const PLAIN: &str = "# Title\n\nHello *world* here\n\n- a\n- b\n  - nested\n\n| h | i |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\n> quoted\n\nTerm\n: detail\n";

fn with_bom(text: &str) -> String {
    format!("\u{feff}{text}")
}

fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn owner_on(doc: &Doc, kind: OwnerKind, line: u32) -> &Owner {
    doc.blocks
        .owners
        .iter()
        .find(|o| o.kind == kind && o.lines.0 == line)
        .unwrap_or_else(|| panic!("no {kind:?} owner on line {line}"))
}

/// Lines with terminators, split on `\n`, `\r\n` and a lone `\r`.
fn split(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let (mut lines, mut start, mut i) = (Vec::new(), 0, 0);
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

// --- the doc and its rev ---

#[test]
fn a_doc_keeps_its_bytes_and_their_rev() {
    let text = "\u{feff}# T\n\nbody\n";
    let doc = Doc::new(text);
    assert_eq!(&*doc.text, text, "the BOM stays in the text");
    assert_eq!(doc.rev.as_str(), "545d6cb693cdcdb20450ce2ee9298ec8a711aa6c");
}

// --- blocks agree with the page ---

#[test]
fn doc_blocks_are_the_pages_blocks() {
    let docs = [
        ("plain", PLAIN.to_string()),
        ("bom", with_bom(PLAIN)),
        ("crlf", crlf(PLAIN)),
        ("bom crlf", with_bom(&crlf(PLAIN))),
        ("bom list", with_bom("- item one\n- item two\n  more\n")),
        ("bom paragraph", with_bom("Hello\nworld\n\nnext\n")),
        ("bom table", with_bom("| a | b |\n|---|---|\n| 1 | 2 |\n")),
        ("bom code", with_bom("```\ncode\n```\n")),
        ("bom only", with_bom("")),
        ("empty", String::new()),
        ("cr", PLAIN.replace('\n', "\r")),
        ("alert", "> [!NOTE]\n> x\n".to_string()),
        ("footnote", "a[^1]\n\n[^1]: n\n".to_string()),
        ("math", "$$\nx\n$$\n".to_string()),
        ("details", "<details>\n\n- in\n\n</details>\n".to_string()),
        (
            "mermaid",
            "```mermaid\ngraph TD\n```\n\nafter\n".to_string(),
        ),
    ];
    for (name, text) in &docs {
        let (_, page) = render::render_indexed(text, None);
        assert_eq!(Doc::new(text).blocks, page, "{name}");
    }
}

#[test]
fn owner_bytes_slice_the_source_whatever_the_line_endings() {
    // Both sides of the page comparison share `anchor::index`, so check bytes literally
    for (name, text) in [("cr", PLAIN.replace('\n', "\r")), ("crlf", crlf(PLAIN))] {
        let doc = Doc::new(&text);
        let para = owner_on(&doc, OwnerKind::Paragraph, 3);
        assert_eq!(
            &doc.text[para.bytes.clone()],
            "Hello *world* here",
            "{name}"
        );
        let heading = owner_on(&doc, OwnerKind::Heading, 1);
        assert_eq!(&doc.text[heading.bytes.clone()], "# Title", "{name}");
    }
}

#[test]
fn a_bom_moves_line_one_columns_and_every_byte_offset_by_three() {
    let text = with_bom(PLAIN);
    let doc = Doc::new(&text);
    // The page's own `data-sourcepos` for `# Title` after a BOM
    assert_eq!(doc.blocks.stops[0].sp, "1:4-1:10");
    let heading = owner_on(&doc, OwnerKind::Heading, 1);
    assert_eq!(heading.sp, "1:4-1:10");
    assert_eq!(heading.bytes, 3..10);
    assert_eq!(&doc.text[heading.bytes.clone()], "# Title");

    let para = owner_on(&doc, OwnerKind::Paragraph, 3);
    assert_eq!(para.sp, "3:1-3:18");
    assert_eq!(&doc.text[para.bytes.clone()], "Hello *world* here");
    let plain = Doc::new(PLAIN);
    let plain_para = owner_on(&plain, OwnerKind::Paragraph, 3);
    assert_eq!(para.bytes.start, plain_para.bytes.start + 3);
}

// --- line index ---

#[test]
fn line_index_counts_and_ranges_lines_without_terminators() {
    // Start and end bytes of each line
    let cases: &[(&str, &[(usize, usize)])] = &[
        ("", &[]),
        ("a", &[(0, 1)]),
        ("a\n", &[(0, 1)]),
        ("a\r\nb", &[(0, 1), (3, 4)]),
        ("a\rb\n", &[(0, 1), (2, 3)]),
        ("\n\n", &[(0, 0), (1, 1)]),
        ("\u{feff}", &[(3, 3)]),
        ("\u{feff}# T\r\nx\n", &[(3, 6), (8, 9)]),
        ("héllo\nñ\n", &[(0, 6), (7, 9)]),
    ];
    for (text, ranges) in cases {
        let doc = Doc::new(text);
        assert_eq!(doc.lines.count() as usize, ranges.len(), "{text:?}");
        for (i, &(start, end)) in ranges.iter().enumerate() {
            let line = i as u32 + 1;
            assert_eq!(
                doc.lines.range(line),
                Some(start..end),
                "{text:?} line {line}"
            );
        }
        assert_eq!(doc.lines.range(0), None, "{text:?}");
        assert_eq!(doc.lines.range(ranges.len() as u32 + 1), None, "{text:?}");
    }
}

#[test]
fn line_index_numbers_lines_the_way_hunks_do() {
    let texts = [
        "\u{feff}first\r\nsecond\rthird\nfourth",
        "one\rtwo\rthree\r",
        "\u{feff}# Title\n\nbody\r\n- item\n",
    ];
    for text in texts {
        let doc = Doc::new(text);
        let lines = split(text);
        assert_eq!(doc.lines.count() as usize, lines.len(), "{text:?}");
        for (i, old) in lines.iter().enumerate() {
            let k = i as u32 + 1;
            let body = old.trim_end_matches(['\r', '\n']);
            let terminator = &old[body.len()..];
            let new_line = format!("CHANGED {k}{terminator}");
            let mut new_lines = lines.clone();
            new_lines[i] = &new_line;
            let new = new_lines.concat();
            assert_eq!(
                hunk::diff(text, &new),
                vec![Hunk {
                    a: k - 1,
                    d: vec![old.to_string()],
                    i: vec![new_line.clone()],
                }],
                "{text:?} line {k}"
            );
            let range = doc.lines.range(k).unwrap();
            assert_eq!(
                &text[range],
                body.trim_start_matches('\u{feff}'),
                "{text:?} line {k}"
            );
        }
    }
}
