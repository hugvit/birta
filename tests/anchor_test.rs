use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use birta::anchor::{self, BlockIndex, Owner, OwnerKind, Stop, StopKind};
use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document};

fn index_of(md: &str) -> BlockIndex {
    let arena = Arena::new();
    let root = parse_document(&arena, md, &anchor::parse_options());
    anchor::index(root, md)
}

/// Every `tests/fixtures/*.md`, plus a CRLF copy of the anchor fixture.
fn fixtures() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect();
    out.sort();
    let anchor = fs::read_to_string(dir.join("anchor.md")).unwrap();
    out.push(("anchor.md (CRLF)".to_string(), anchor.replace('\n', "\r\n")));
    out
}

fn kind_name<T: serde::Serialize>(kind: &T) -> String {
    serde_json::to_value(kind)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn stop_line(s: &Stop) -> String {
    let level = s.level.map(|l| format!(" h{l}")).unwrap_or_default();
    let section = s
        .section
        .map(|(a, b)| format!(" section {a}-{b}"))
        .unwrap_or_default();
    format!(
        "{} {} {} {}-{}{level}{section}",
        s.n,
        kind_name(&s.kind),
        s.sp,
        s.lines.0,
        s.lines.1
    )
}

fn owner_line(o: &Owner) -> String {
    format!(
        "{} {} {}-{}",
        kind_name(&o.kind),
        o.sp,
        o.lines.0,
        o.lines.1
    )
}

/// `(line, column)` of the start of a `data-sourcepos` value.
fn sp_start(sp: &str) -> (u32, u32) {
    let (start, _) = sp.split_once('-').unwrap();
    let (l, c) = start.split_once(':').unwrap();
    (l.parse().unwrap(), c.parse().unwrap())
}

/// An opening tag in rendered HTML that carries `data-sourcepos`.
struct Tagged {
    name: String,
    sp: String,
    attrs: String,
    in_thead: bool,
    /// Inside the footnotes section, which renders after the doc.
    in_footnotes: bool,
    /// The start of the HTML right after the opening tag.
    rest: String,
}

/// Opening tags with a `data-sourcepos` attribute, in document order.
fn tagged(html: &str) -> Vec<Tagged> {
    let mut out = Vec::new();
    let mut in_thead = false;
    let mut in_footnotes = false;
    let mut at = 0;
    while let Some(lt) = html[at..].find('<').map(|i| at + i) {
        let gt = lt + html[lt..].find('>').unwrap();
        let tag = &html[lt + 1..gt];
        at = gt + 1;
        if tag == "thead" {
            in_thead = true;
        } else if tag == "/thead" {
            in_thead = false;
        } else if tag.starts_with("section ") && tag.contains(" class=\"footnotes\"") {
            in_footnotes = true;
        } else if tag == "/section" {
            in_footnotes = false;
        }
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let Some(i) = tag.find(" data-sourcepos=\"") else {
            continue;
        };
        let value = &tag[i + " data-sourcepos=\"".len()..];
        out.push(Tagged {
            name,
            sp: value[..value.find('"').unwrap()].to_string(),
            attrs: tag.to_string(),
            in_thead,
            in_footnotes,
            rest: html[at..].chars().take(200).collect(),
        });
    }
    out
}

/// A `li` or `dd` that holds `<p>` blocks (loose item, footnote) owns no text of its own.
fn holds_paragraphs(t: &Tagged) -> bool {
    let mut rest = t.rest.trim_start();
    if rest.starts_with("<input ") {
        rest = rest[rest.find('>').unwrap() + 1..].trim_start();
    }
    rest.starts_with("<p ") || rest.starts_with("<p>")
}

fn is_heading_tag(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn owner_tag_fits(kind: &OwnerKind, tag: &str) -> bool {
    match kind {
        OwnerKind::Paragraph => tag == "p",
        OwnerKind::Item => tag == "li",
        OwnerKind::Heading => is_heading_tag(tag),
        OwnerKind::Cell => matches!(tag, "td" | "th"),
        OwnerKind::Term => tag == "dt",
        OwnerKind::Detail => tag == "dd",
        OwnerKind::Code => tag == "pre",
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn stop_sps(idx: &BlockIndex, kind: StopKind) -> Vec<String> {
    idx.stops
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| s.sp.clone())
        .collect()
}

#[test]
fn stop_list_per_fixture() {
    insta::glob!("fixtures/*.md", |path| {
        let md = fs::read_to_string(path).unwrap();
        let idx = index_of(&md);
        let lines: Vec<String> = idx.stops.iter().map(stop_line).collect();
        insta::assert_snapshot!(lines.join("\n"));
    });
}

#[test]
fn owner_list_per_fixture() {
    insta::glob!("fixtures/*.md", |path| {
        let md = fs::read_to_string(path).unwrap();
        let idx = index_of(&md);
        let lines: Vec<String> = idx.owners.iter().map(owner_line).collect();
        insta::assert_snapshot!(lines.join("\n"));
    });
}

#[test]
fn owners_are_exactly_the_rendered_text_blocks() {
    const OWNER_TAGS: [&str; 13] = [
        "p", "li", "h1", "h2", "h3", "h4", "h5", "h6", "td", "th", "dt", "dd", "pre",
    ];
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let idx = index_of(&md);
        let html = birta::render::render(&md, None);
        let rendered: Vec<Tagged> = tagged(&html)
            .into_iter()
            .filter(|t| OWNER_TAGS.contains(&t.name.as_str()))
            .filter(|t| !(matches!(t.name.as_str(), "li" | "dd") && holds_paragraphs(t)))
            .collect();

        let want = sorted(rendered.iter().map(|t| t.sp.clone()).collect());
        let got = sorted(idx.owners.iter().map(|o| o.sp.clone()).collect());
        if want != got {
            bad.push(format!("{name}: rendered {want:?}\n  owners {got:?}"));
        }
        let unique: BTreeSet<&str> = idx.owners.iter().map(|o| o.sp.as_str()).collect();
        if unique.len() != idx.owners.len() {
            bad.push(format!("{name}: duplicate owner sp"));
        }
        for o in &idx.owners {
            let fits = rendered
                .iter()
                .any(|t| t.sp == o.sp && owner_tag_fits(&o.kind, &t.name));
            if !fits {
                bad.push(format!("{name}: {} is not a rendered {:?}", o.sp, o.kind));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn every_stop_names_one_rendered_block_of_its_kind() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let idx = index_of(&md);
        let html = birta::render::render(&md, None);
        let tags = tagged(&html);
        let rendered = |pick: &dyn Fn(&Tagged) -> bool| -> Vec<String> {
            sorted(
                tags.iter()
                    .filter(|t| !t.in_footnotes && pick(t))
                    .map(|t| t.sp.clone())
                    .collect(),
            )
        };

        let rows = [
            (
                StopKind::ListItem,
                rendered(&|t| t.name == "li" && !t.attrs.contains(" id=\"fn-")),
            ),
            (
                StopKind::TableRow,
                rendered(&|t| t.name == "tr" && !t.in_thead),
            ),
            (StopKind::Heading, rendered(&|t| is_heading_tag(&t.name))),
            (StopKind::CodeBlock, rendered(&|t| t.name == "pre")),
        ];
        for (kind, want) in rows {
            let got = sorted(stop_sps(&idx, kind.clone()));
            if got != want {
                bad.push(format!(
                    "{name} {kind:?}: rendered {want:?}\n  stops {got:?}"
                ));
            }
        }

        let paragraphs = rendered(&|t| t.name == "p");
        for sp in stop_sps(&idx, StopKind::Paragraph) {
            if !paragraphs.contains(&sp) {
                bad.push(format!("{name}: paragraph stop {sp} is not a rendered <p>"));
            }
        }
        for s in idx.stops.iter().filter(|s| s.kind == StopKind::Heading) {
            let level = tags
                .iter()
                .find(|t| t.sp == s.sp && is_heading_tag(&t.name))
                .map(|t| t.name[1..].parse::<u8>().unwrap());
            if level != s.level {
                bad.push(format!("{name}: heading {} level {:?}", s.sp, s.level));
            }
        }
        for kind in [
            StopKind::ListItem,
            StopKind::TableRow,
            StopKind::Heading,
            StopKind::Paragraph,
            StopKind::CodeBlock,
        ] {
            let sps = stop_sps(&idx, kind.clone());
            let unique: BTreeSet<&String> = sps.iter().collect();
            if unique.len() != sps.len() {
                bad.push(format!("{name}: duplicate {kind:?} sp in {sps:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Paragraphs that an item renders as its own text are not stops, nor is anything in a
/// footnote, which renders after the doc.
fn paragraph_is_a_stop<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    let parent = node.parent().unwrap();
    let in_item = matches!(
        parent.data.borrow().value,
        NodeValue::Item(_)
            | NodeValue::TaskItem(_)
            | NodeValue::DescriptionTerm
            | NodeValue::DescriptionDetails
    );
    let in_footnote = node
        .ancestors()
        .any(|a| matches!(a.data.borrow().value, NodeValue::FootnoteDefinition(_)));
    !in_item && !in_footnote
}

#[test]
fn paragraph_stops_are_the_paragraphs_outside_items_and_footnotes() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let idx = index_of(&md);
        let got = sorted(stop_sps(&idx, StopKind::Paragraph));
        let arena = Arena::new();
        let root = parse_document(&arena, &md, &anchor::parse_options());
        let mut want = Vec::new();
        for node in root.descendants() {
            if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
                continue;
            }
            if paragraph_is_a_stop(node) {
                let pos = node.data.borrow().sourcepos;
                want.push(format!(
                    "{}:{}-{}:{}",
                    pos.start.line, pos.start.column, pos.end.line, pos.end.column
                ));
            }
        }
        if got != sorted(want.clone()) {
            bad.push(format!("{name}: want {want:?}\n  stops {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn quote_and_alert_paragraphs_are_stops_and_footnote_paragraphs_only_owners() {
    let md = "> quoted\n\n> [!NOTE]\n> alert body\n\nText[^1].\n\n[^1]: Foot body.\n";
    let idx = index_of(md);
    let starts: Vec<u32> = stop_sps(&idx, StopKind::Paragraph)
        .iter()
        .map(|sp| sp_start(sp).0)
        .collect();
    assert_eq!(starts, [1, 4, 6]);
    let footnote = idx
        .owners
        .iter()
        .find(|o| sp_start(&o.sp).0 == 8)
        .expect("the footnote paragraph is an owner");
    assert_eq!(footnote.kind, OwnerKind::Paragraph);
    assert!(
        idx.stops.iter().all(|s| sp_start(&s.sp).0 != 8),
        "{:?}",
        idx.stops
    );
}

#[test]
fn nothing_inside_a_footnote_is_a_stop_but_its_text_has_owners() {
    let md = "Text[^1].\n\n[^1]: Foot body.\n\n    - foot item\n\n    ## Foot heading\n\n    ```\n    foot code\n    ```\n\n    | a |\n    |---|\n    | 1 |\n\nAfter.\n";
    let idx = index_of(md);
    assert_eq!(
        idx.stops.iter().map(stop_line).collect::<Vec<_>>(),
        ["0 paragraph 1:1-1:9 1-1", "1 paragraph 17:1-17:6 17-17"]
    );
    // Owners run in render order, where footnotes come last
    let owners: Vec<(String, u32)> = idx
        .owners
        .iter()
        .map(|o| (kind_name(&o.kind), sp_start(&o.sp).0))
        .collect();
    let row = |k: &str, line| (k.to_string(), line);
    assert_eq!(
        owners,
        [
            row("paragraph", 1),
            row("paragraph", 17),
            row("paragraph", 3),
            row("item", 5),
            row("heading", 7),
            row("code", 9),
            row("cell", 13),
            row("cell", 15),
        ]
    );
}

#[test]
fn stops_run_in_document_order_numbered_from_zero() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let idx = index_of(&md);
        if idx.stops.first().is_some_and(|s| s.n != 0) {
            bad.push(format!("{name}: first n is {}", idx.stops[0].n));
        }
        for pair in idx.stops.windows(2) {
            if sp_start(&pair[0].sp) >= sp_start(&pair[1].sp) {
                bad.push(format!("{name}: {} before {}", pair[0].sp, pair[1].sp));
            }
            if pair[1].n != pair[0].n + 1 {
                bad.push(format!("{name}: n {} then {}", pair[0].n, pair[1].n));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_nested_list_and_its_first_item_are_told_apart() {
    let md = "- parent\n  - only child\n";
    let html = birta::render::render(md, None);
    assert!(
        html.contains("<ul data-sourcepos=\"2:3-2:14\">\n<li data-sourcepos=\"2:3-2:14\">"),
        "the nested ul and its li share a sourcepos: {html}"
    );

    let idx = index_of(md);
    assert_eq!(
        idx.stops.iter().map(stop_line).collect::<Vec<_>>()[..],
        ["0 list_item 1:1-2:14 1-2", "1 list_item 2:3-2:14 2-2",]
    );
    assert_eq!(
        idx.owners.iter().map(owner_line).collect::<Vec<_>>(),
        ["item 1:1-2:14 1-2", "item 2:3-2:14 2-2"]
    );
}

#[test]
fn a_column_zero_end_stops_on_the_line_before() {
    let md = "- a\n- b\n\n1. c\n";
    let idx = index_of(md);
    let b = idx
        .stops
        .iter()
        .find(|s| s.sp == "2:1-3:0")
        .expect("item b");
    assert_eq!(b.lines, (2, 2));
    let owner = idx.owners.iter().find(|o| o.sp == "2:1-3:0").unwrap();
    assert_eq!(owner.lines, (2, 2));
    assert_eq!(md[owner.bytes.clone()].trim_end(), "- b");

    let md = "    x\n    y\n\npara\n";
    let idx = index_of(md);
    let code = &idx.stops[0];
    assert_eq!(
        (code.kind.clone(), code.sp.as_str()),
        (StopKind::CodeBlock, "1:5-3:0")
    );
    assert_eq!(code.lines, (1, 2));
    let owner = &idx.owners[0];
    assert_eq!((owner.kind.clone(), owner.lines), (OwnerKind::Code, (1, 2)));
    assert_eq!(md[owner.bytes.clone()].trim_end(), "x\n    y");
}

const SECTIONS: &str = "# One\n\ntext\n\n## Two\n\n### Three\n\n## Four\n\n> ## Quoted\n\n- ## Listed\n\n# Five\n\nSetext\n------\n\nend";

/// `(level, lines, section)` of a heading stop.
type HeadingRow = (Option<u8>, (u32, u32), Option<(u32, u32)>);

fn headings(md: &str) -> Vec<HeadingRow> {
    index_of(md)
        .stops
        .iter()
        .filter(|s| s.kind == StopKind::Heading)
        .map(|s| (s.level, s.lines, s.section))
        .collect()
}

#[test]
fn a_top_level_heading_section_runs_to_the_next_heading_of_its_level_or_above() {
    let expect = |last: u32| {
        vec![
            (Some(1), (1, 1), Some((1, 14))),
            (Some(2), (5, 5), Some((5, 8))),
            (Some(3), (7, 7), Some((7, 8))),
            (Some(2), (9, 9), Some((9, 14))),
            (Some(2), (11, 11), None),
            (Some(2), (13, 13), None),
            (Some(1), (15, 15), Some((15, last))),
            (Some(2), (17, 18), Some((17, last))),
        ]
    };
    assert_eq!(headings(SECTIONS), expect(20), "no trailing newline");
    assert_eq!(
        headings(&format!("{SECTIONS}\n")),
        expect(20),
        "trailing newline"
    );
    assert_eq!(
        headings(&format!("{SECTIONS}\n\n\n")),
        expect(22),
        "trailing blank lines"
    );
}

#[test]
fn owner_bytes_slice_the_source() {
    let md = "# Café 🎉\n\nnaïve *text* here\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nTerm\n: def\n\n```\ncode\n```\n";
    let idx = index_of(md);
    let got: Vec<(String, String, (u32, u32))> = idx
        .owners
        .iter()
        .map(|o| (kind_name(&o.kind), o.sp.clone(), o.lines))
        .collect();
    let row = |k: &str, sp: &str, lines| (k.to_string(), sp.to_string(), lines);
    assert_eq!(
        got,
        [
            row("heading", "1:1-1:12", (1, 1)),
            row("paragraph", "3:1-3:18", (3, 3)),
            row("item", "5:1-5:5", (5, 5)),
            row("item", "6:1-6:5", (6, 6)),
            row("cell", "8:2-8:4", (8, 8)),
            row("cell", "8:6-8:8", (8, 8)),
            row("cell", "10:2-10:4", (10, 10)),
            row("cell", "10:6-10:8", (10, 10)),
            row("term", "12:1-12:1", (12, 12)),
            row("detail", "13:1-13:5", (13, 13)),
            row("code", "15:1-17:3", (15, 17)),
        ]
    );

    let slices: Vec<&str> = idx
        .owners
        .iter()
        .filter(|o| o.kind != OwnerKind::Term)
        .map(|o| md[o.bytes.clone()].trim())
        .collect();
    assert_eq!(
        slices,
        [
            "# Café 🎉",
            "naïve *text* here",
            "- one",
            "- two",
            "a",
            "b",
            "1",
            "2",
            ": def",
            "```\ncode\n```",
        ]
    );
}

#[test]
fn owner_bytes_start_on_the_sourcepos_column() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let line_starts: Vec<usize> = std::iter::once(0)
            .chain(md.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        for o in index_of(&md).owners {
            let (line, col) = sp_start(&o.sp);
            let start = line_starts[line as usize - 1] + col as usize - 1;
            if o.bytes.start != start || o.bytes.end > md.len() || o.bytes.end <= o.bytes.start {
                bad.push(format!("{name}: {} bytes {:?}", o.sp, o.bytes));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn crlf_gives_the_same_index_as_lf() {
    let lf =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anchor.md"))
            .unwrap();
    let crlf = lf.replace('\n', "\r\n");
    let (a, b) = (index_of(&lf), index_of(&crlf));

    let stops = |i: &BlockIndex| i.stops.iter().map(stop_line).collect::<Vec<_>>();
    assert_eq!(stops(&a), stops(&b));
    let owners = |i: &BlockIndex| i.owners.iter().map(owner_line).collect::<Vec<_>>();
    assert_eq!(owners(&a), owners(&b));
    for (x, y) in a.owners.iter().zip(&b.owners) {
        assert_eq!(
            lf[x.bytes.clone()].trim_end(),
            crlf[y.bytes.clone()].replace("\r\n", "\n").trim_end(),
            "{}",
            x.sp
        );
    }
}

#[test]
fn stops_and_owners_serialize_to_the_wire_shape() {
    let md =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anchor.md"))
            .unwrap();
    let idx = index_of(&md);
    let stop_keys = ["n", "kind", "sp", "lines", "level", "section"];
    let stop_kinds = [
        "list_item",
        "table_row",
        "heading",
        "paragraph",
        "code_block",
    ];
    let mut seen = BTreeSet::new();
    for s in &idx.stops {
        let v = serde_json::to_value(s).unwrap();
        let obj = v.as_object().unwrap();
        assert!(obj.keys().all(|k| stop_keys.contains(&k.as_str())), "{v}");
        let kind = v["kind"].as_str().unwrap();
        assert!(stop_kinds.contains(&kind), "{v}");
        seen.insert(kind.to_string());
        assert_eq!(v["lines"], serde_json::json!([s.lines.0, s.lines.1]), "{v}");
        if kind != "heading" {
            assert!(v.get("level").is_none_or(|l| l.is_null()), "{v}");
            assert!(v.get("section").is_none_or(|l| l.is_null()), "{v}");
        }
    }
    assert_eq!(
        seen.len(),
        stop_kinds.len(),
        "the fixture has every stop kind"
    );

    let owner_kinds = [
        "paragraph",
        "item",
        "heading",
        "cell",
        "term",
        "detail",
        "code",
    ];
    let kinds: BTreeSet<String> = idx.owners.iter().map(|o| kind_name(&o.kind)).collect();
    assert!(
        kinds.iter().all(|k| owner_kinds.contains(&k.as_str())),
        "{kinds:?}"
    );
    assert_eq!(
        kinds.len(),
        owner_kinds.len(),
        "the fixture has every owner kind: {kinds:?}"
    );
}

#[test]
fn parse_options_enable_every_render_extension() {
    let md = "- [ ] t\n\n\"q\" :tada: ~~s~~ www.x.org $m$\n\n> [!NOTE]\n> a\n\nT\n: d\n\n|a|\n|-|\n\nx[^1]\n\n[^1]: f\n";
    let arena = Arena::new();
    let root = parse_document(&arena, md, &anchor::parse_options());
    let mut seen = BTreeSet::new();
    for node in root.descendants() {
        let found = match &node.data.borrow().value {
            NodeValue::TaskItem(_) => "task item",
            NodeValue::Text(t) if t.contains('\u{201c}') => "smart quotes",
            NodeValue::ShortCode(_) => "shortcode",
            NodeValue::Strikethrough => "strikethrough",
            NodeValue::Link(_) => "autolink",
            NodeValue::Math(_) => "math",
            NodeValue::Alert(_) => "alert",
            NodeValue::DescriptionList => "description list",
            NodeValue::Table(_) => "table",
            NodeValue::FootnoteDefinition(_) => "footnote",
            _ => continue,
        };
        seen.insert(found);
    }
    assert_eq!(
        seen,
        BTreeSet::from([
            "alert",
            "autolink",
            "description list",
            "footnote",
            "math",
            "shortcode",
            "smart quotes",
            "strikethrough",
            "table",
            "task item",
        ])
    );
}

#[test]
fn pre_sps_equal_code_stops_in_every_mode() {
    let catppuccin = birta::theme::resolve_by_name("catppuccin").unwrap();
    let themed = catppuccin.active_data().syntax.clone();
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let want = sorted(stop_sps(&index_of(&md), StopKind::CodeBlock));
        for syntax in [None, themed.as_ref()] {
            let renders = [
                ("render", birta::render::render(&md, syntax)),
                (
                    "render_dir",
                    birta::render::render_dir(&md, syntax, Path::new("sub/doc.md")),
                ),
                ("render_bundle", birta::render::render_bundle(&md, syntax).0),
            ];
            for (how, html) in renders {
                let tagged_pres: Vec<Tagged> = tagged(&html)
                    .into_iter()
                    .filter(|t| t.name == "pre")
                    .collect();
                let pres = html.matches("<pre ").count() + html.matches("<pre>").count();
                let got = sorted(
                    tagged_pres
                        .iter()
                        .filter(|t| !t.in_footnotes)
                        .map(|t| t.sp.clone())
                        .collect(),
                );
                if got != want || pres != tagged_pres.len() {
                    bad.push(format!(
                        "{name} {how} themed={}: pre {got:?} ({pres} tags), stops {want:?}",
                        syntax.is_some()
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Fastest of three runs of parsing and indexing `md`.
fn index_time(md: &str) -> std::time::Duration {
    (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            let idx = index_of(md);
            let took = start.elapsed();
            assert!(!idx.stops.is_empty());
            took
        })
        .min()
        .unwrap()
}

#[test]
fn index_scales_linearly() {
    let chunk = |copies: usize| {
        let mut md = String::new();
        for i in 0..copies {
            md.push_str(&format!("## Section {i}\n\n"));
            for j in 0..10 {
                md.push_str(&format!("- item {i}.{j} with *some* text\n"));
            }
            md.push('\n');
        }
        md
    };
    let (small, large) = (chunk(2_000), chunk(4_000));
    let (t1, t2) = (index_time(&small), index_time(&large));
    assert!(t2 < t1 * 3, "{t1:?} then {t2:?} for twice the doc");
    assert!(t2 < std::time::Duration::from_secs(2), "{t2:?}");
}

#[test]
fn a_bom_doc_indexes_like_render() {
    let md = "\u{feff}# Title\n\nText.\n";
    let html = birta::render::render(md, None);
    let rendered = tagged(&html);
    let h1 = rendered.iter().find(|t| t.name == "h1").expect("an h1");
    assert_eq!(h1.sp, "1:4-1:10");

    let idx = index_of(md);
    assert_eq!(idx.stops[0].sp, h1.sp);
    assert_eq!(idx.stops[0].lines, (1, 1));
    let owner = idx.owners.iter().find(|o| o.sp == h1.sp).unwrap();
    assert_eq!(&md[owner.bytes.clone()], "# Title");
}

#[test]
fn owner_bytes_never_split_a_character() {
    let md = "Été\n: dé\n\n| ü | ö |\n|---|---|\n";
    let idx = index_of(md);
    assert!(
        idx.owners.iter().any(|o| o.kind == OwnerKind::Term),
        "{:?}",
        idx.owners
    );
    for (name, md) in fixtures()
        .into_iter()
        .chain([("dt".to_string(), md.to_string())])
    {
        for o in index_of(&md).owners {
            assert!(
                md.is_char_boundary(o.bytes.start) && md.is_char_boundary(o.bytes.end),
                "{name}: {} bytes {:?}",
                o.sp,
                o.bytes
            );
        }
    }
}
