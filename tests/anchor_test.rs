use std::collections::BTreeSet;
use std::fs;
use std::ops::Range;
use std::path::Path;

use birta::anchor::{
    self, BlockIndex, Cand, Loc, Owner, OwnerKind, Reason, ResolveReq, Resolved, Stop, StopKind,
};
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
    assert_eq!((owner.kind, owner.lines), (OwnerKind::Code, (1, 2)));
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
#[ignore = "timing ratio; noisy under load, run in CI with --ignored"]
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

// Resolver: a selection's rendered text back to source, and source back to a location.

const A: &str = include_str!("fixtures/anchor.md");

// Owner texts of the anchor fixture, written out by hand from the rendered page.
const SP_H1: &str = "1:1-1:26";
const T_H1: &str = "Retry “layer” — design";
const SP_PR: &str = "3:1-3:213";
const T_PR: &str = "This PR adds retry with jittered backoff to the upload() client – because S3 returns 503 & drops events… See the docs 🎉. He said “we just fail” and don’t *panic*.";
const SP_RETRY: &str = "5:1-7:70";
const T_RETRY: &str = "We just fail hard here, and we just fail again. Soft break line two with spaces hard break line three Ctrl and math \u{fffc} end\u{fffc}.";
const SP_TASK: &str = "9:1-9:31";
const T_TASK: &str = "Ran the integration suite";
const SP_CHECKED: &str = "10:1-11:22";
const T_CHECKED: &str = "Checked idempotency on every POST";
const SP_NESTED: &str = "11:3-11:22";
const T_NESTED: &str = "nested item – one";
const SP_PLAIN: &str = "12:1-12:37";
const T_PLAIN: &str = "plain item with code and gone";
const SP_KNOB: &str = "14:2-14:7";
const T_KNOB: &str = "Knob";
const SP_RETRIES: &str = "16:2-16:10";
const T_RETRIES: &str = "retries";
const SP_TIMES: &str = "16:16-16:30";
const T_TIMES: &str = "5 × 8s = 40s";
const SP_ALERT: &str = "20:3-20:56";
const T_ALERT: &str = "Retries are capped at 5 to fit the 60s job budget.";
const SP_AUTO: &str = "22:1-22:52";
const T_AUTO: &str = "https://example.com/auto and www.example.org bare.";
const SP_TERM: &str = "30:1-30:1";
const T_TERM: &str = "Term one";
const SP_DETAIL: &str = "31:1-31:32";
const T_DETAIL: &str = "Definition with emph – dash";
const SP_SETEXT: &str = "33:1-34:14";
const T_SETEXT: &str = "Setext Heading";
const SP_RUST: &str = "43:1-47:3";
const T_RUST: &str = "fn retry() {\n\tlet n = 5;\n}\n";
const SP_FENCE: &str = "49:3-52:5";
const T_FENCE: &str = "indented fence\n  kept indent\n";
const SP_INDENTED: &str = "54:5-56:0";
const T_INDENTED: &str = "indented code block\nsecond line\n";
const SP_MERMAID: &str = "61:1-64:3";
const SP_MATH: &str = "66:1-68:3";
const SP_INNER: &str = "74:5-74:35";
const T_INNER: &str = "inner quote with café and 🎉";
const SP_QHEAD: &str = "76:3-76:20";
const T_QHEAD: &str = "Quoted heading";
const SP_BODY: &str = "81:1-81:30";
const T_BODY: &str = "Body paragraph inside details.";
const SP_TADA: &str = "87:2-87:9";
const T_TADA: &str = "🎉";
const SP_NAIVE: &str = "87:11-87:23";
const T_NAIVE: &str = "naïve 🎉";
const SP_FOOT: &str = "24:7-24:24";
const T_FOOT: &str = "The footnote body.";
const SP_PIPE: &str = "117:2-117:9";
const T_PIPE: &str = "a | b";
const SP_FOOT_CODE: &str = "107:5-109:7";
const T_FOOT_CODE: &str = "foot code\n";

fn crlf(md: &str) -> String {
    md.replace('\n', "\r\n")
}

fn req(
    sp: &str,
    quote: &str,
    pre: &str,
    suf: &str,
    off: Option<usize>,
    tl: Option<usize>,
) -> ResolveReq {
    ResolveReq {
        sp: sp.to_string(),
        quote: quote.to_string(),
        pre: pre.to_string(),
        suf: suf.to_string(),
        off,
        tl,
    }
}

/// Byte range of `needle` inside the one occurrence of `context` in `text`.
fn span(text: &str, context: &str, needle: &str) -> Range<usize> {
    let at: Vec<usize> = text.match_indices(context).map(|(i, _)| i).collect();
    assert_eq!(at.len(), 1, "{context:?} occurs {} times", at.len());
    let inner: Vec<usize> = context.match_indices(needle).map(|(i, _)| i).collect();
    assert_eq!(inner.len(), 1, "{needle:?} in {context:?}");
    let start = at[0] + inner[0];
    start..start + needle.len()
}

/// [`span`] in code points.
fn cp_span(text: &str, context: &str, needle: &str) -> Range<usize> {
    let bytes = span(text, context, needle);
    let start = text[..bytes.start].chars().count();
    start..start + needle.chars().count()
}

fn cp_len(t: &str) -> usize {
    t.chars().count()
}

fn line_of(md: &str, byte: usize) -> u32 {
    md.as_bytes()[..byte]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
}

/// Bytes from the start of line `a` to the end of line `b`.
fn line_range(md: &str, (a, b): (u32, u32)) -> Range<usize> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(md.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let start = starts[a as usize - 1];
    start..starts.get(b as usize).copied().unwrap_or(md.len())
}

fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// The one owner of `kind` in `md`.
fn sole_owner(md: &str, kind: OwnerKind) -> String {
    let sps: Vec<String> = index_of(md)
        .owners
        .into_iter()
        .filter(|o| o.kind == kind)
        .map(|o| o.sp)
        .collect();
    assert_eq!(sps.len(), 1, "{kind:?} owners in {md:?}: {sps:?}");
    sps[0].clone()
}

/// A selection as the client sends it: `len` code points at `off` in owner text `t`.
struct Sel {
    quote: String,
    pre: String,
    suf: String,
    off: usize,
    tl: usize,
}

fn select(t: &str, off: usize, len: usize) -> Sel {
    let chars: Vec<char> = t.chars().collect();
    let end = off + len;
    Sel {
        quote: chars[off..end].iter().collect(),
        pre: chars[off.saturating_sub(32)..off].iter().collect(),
        suf: chars[end..(end + 32).min(chars.len())].iter().collect(),
        off,
        tl: chars.len(),
    }
}

fn select_in(t: &str, context: &str, quote: &str) -> Sel {
    let at = cp_span(t, context, quote);
    select(t, at.start, at.len())
}

impl Sel {
    fn with_off(&self, sp: &str) -> ResolveReq {
        let (off, tl) = (Some(self.off), Some(self.tl));
        req(sp, &self.quote, &self.pre, &self.suf, off, tl)
    }

    fn by_context(&self, sp: &str) -> ResolveReq {
        req(sp, &self.quote, &self.pre, &self.suf, None, None)
    }
}

#[derive(Debug)]
struct Hit {
    lines: (u32, u32),
    bytes: Range<usize>,
    source: String,
    shown: String,
    widened: bool,
    text_len: usize,
}

fn exact(r: Resolved) -> Result<Hit, String> {
    match r {
        Resolved::Exact {
            lines,
            bytes,
            source,
            shown,
            widened,
            text_len,
            ..
        } => Ok(Hit {
            lines,
            bytes,
            source,
            shown,
            widened,
            text_len,
        }),
        other => Err(format!("not exact: {other:?}")),
    }
}

fn candidates(r: Resolved) -> (Vec<Cand>, usize) {
    match r {
        Resolved::Candidates {
            items, text_len, ..
        } => (items, text_len),
        other => panic!("not candidates: {other:?}"),
    }
}

fn block(r: Resolved) -> (Reason, String, usize) {
    match r {
        Resolved::Block {
            reason,
            kept_quote,
            text_len,
            ..
        } => (reason, kept_quote, text_len),
        other => panic!("not a block: {other:?}"),
    }
}

fn loc_parts(loc: Option<Loc>) -> Option<(String, (usize, usize), usize)> {
    loc.map(|l| (l.sp, l.off, l.tl))
}

/// `(sp, owner text, context in text, quote, expected source, context in doc, widened)`.
/// The quote is cut from its context in the owner text, the source from its context in the doc.
type Case<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<bool>,
);

/// Resolves every case with its offset and by context only; both must give the exact source.
fn check_cases(md: &str, cases: &[Case]) -> Vec<String> {
    let mut bad = Vec::new();
    for &(sp, t, in_t, quote, source, in_md, widened) in cases {
        let sel = select_in(t, in_t, quote);
        let want = span(md, in_md, source);
        let want_lines = (line_of(md, want.start), line_of(md, want.end - 1));
        for (mode, r) in [
            ("off", anchor::resolve(md, &sel.with_off(sp))),
            ("context", anchor::resolve(md, &sel.by_context(sp))),
        ] {
            let what = format!("{sp} {quote:?} by {mode}");
            let hit = match exact(r) {
                Ok(hit) => hit,
                Err(e) => {
                    bad.push(format!("{what}: {e}"));
                    continue;
                }
            };
            if hit.source != source || hit.bytes != want {
                bad.push(format!(
                    "{what}: {:?} at {:?}, want {source:?} at {want:?}",
                    hit.source, hit.bytes
                ));
                continue;
            }
            if hit.lines != want_lines {
                bad.push(format!(
                    "{what}: lines {:?}, want {want_lines:?}",
                    hit.lines
                ));
            }
            if hit.shown != quote {
                bad.push(format!("{what}: shown {:?}", hit.shown));
            }
            if hit.text_len != sel.tl {
                bad.push(format!(
                    "{what}: text_len {}, want {}",
                    hit.text_len, sel.tl
                ));
            }
            if widened.is_some_and(|w| w != hit.widened) {
                bad.push(format!("{what}: widened {}", hit.widened));
            }
            let quoted = (sel.off, sel.off + cp_len(quote));
            match loc_parts(anchor::locate(md, hit.bytes.clone())) {
                Some((loc_sp, off, tl))
                    if loc_sp == sp
                        && tl == sel.tl
                        && off.0 <= quoted.0
                        && off.1 >= quoted.1
                        && (hit.widened || off == quoted) => {}
                other => bad.push(format!("{what}: locate gave {other:?}, want {quoted:?}")),
            }
        }
    }
    bad
}

#[test]
fn a_selection_resolves_to_its_exact_source() {
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (SP_PR, T_PR, "“we just fail”", "we just fail", "we just fail", "\"we just fail\"", Some(false)),
        (SP_PR, T_PR, "client – because S3", "client – because S3", "client -- because S3", "client -- because S3", Some(true)),
        (SP_PR, T_PR, "503 & drops events… See", "503 & drops events… See", "503 &amp; drops events... See", "503 &amp; drops events... See", Some(true)),
        (SP_PR, T_PR, "the docs 🎉", "the docs 🎉", "[the docs](https://example.com/retry) :tada:", "[the docs](https://example.com/retry) :tada:", Some(true)),
        (SP_PR, T_PR, "don’t *panic*", "don’t *panic*", "don't \\*panic\\*", "don't \\*panic\\*", Some(false)),
        (SP_PR, T_PR, "adds retry", "adds retry", "**adds retry**", "**adds retry**", Some(true)),
        (SP_PR, T_PR, "adds retry", "adds", "**adds retry**", "**adds retry**", Some(true)),
        (SP_PR, T_PR, "the docs", "docs", "[the docs](https://example.com/retry)", "[the docs](https://example.com/retry)", Some(true)),
        (SP_PR, T_PR, " jittered ", "jittered", "*jittered*", "*jittered*", Some(true)),
        (SP_PR, T_PR, "jittered", "itter", "itter", "*jittered*", Some(false)),
        (SP_PR, T_PR, "the upload() client", "the upload() client", "the `upload()` client", "the `upload()` client", Some(false)),
        (SP_PR, T_PR, "upload()", "upload", "upload", "`upload()`", Some(false)),
        (SP_RETRY, T_RETRY, "and we just fail again", "just fail", "just fail", "we just fail again", Some(false)),
        (SP_RETRY, T_RETRY, "We just fail hard", "just fail", "just **fail**", "We just **fail** hard", Some(true)),
        (SP_RETRY, T_RETRY, "line two with spaces", "line two with spaces", "line two  with  spaces", "line two  with  spaces", Some(false)),
        (SP_RETRY, T_RETRY, "line two with", "two ", "two ", "line two  with", Some(false)),
        (SP_RETRY, T_RETRY, "spaces hard break", "spaces hard break", "spaces  \nhard break", "spaces  \nhard break", Some(false)),
        (SP_RETRY, T_RETRY, "again. Soft", "again. Soft", "again.\nSoft", "again.\nSoft", Some(false)),
        (SP_RETRY, T_RETRY, "three Ctrl and", "three Ctrl and", "three <kbd>Ctrl</kbd> and", "three <kbd>Ctrl</kbd> and", Some(false)),
        (SP_RETRY, T_RETRY, "math \u{fffc} end", "math \u{fffc} end", "math $e^{i\\pi}+1=0$ end", "math $e^{i\\pi}+1=0$ end", Some(true)),
        (SP_RETRY, T_RETRY, "end\u{fffc}.", "end\u{fffc}", "end[^1]", "end[^1].", Some(true)),
        (SP_TASK, T_TASK, "Ran the", "Ran the", "Ran the", "Ran the", Some(false)),
        (SP_CHECKED, T_CHECKED, "every POST", "every POST", "every POST", "every POST", Some(false)),
        (SP_NESTED, T_NESTED, "nested item", "nested item", "nested item", "nested item", Some(false)),
        (SP_NESTED, T_NESTED, "– one", "– one", "-- one", "-- one", Some(true)),
        (SP_PLAIN, T_PLAIN, "and gone", "and gone", "and ~~gone~~", "and ~~gone~~", Some(true)),
        (SP_PLAIN, T_PLAIN, "with code and", "with code and", "with `code` and", "with `code` and", Some(false)),
        (SP_KNOB, T_KNOB, "Knob", "Knob", "Knob", "| Knob |", Some(false)),
        (SP_RETRIES, T_RETRIES, "retries", "retries", "retries", "| retries |", Some(false)),
        (SP_TIMES, T_TIMES, "8s = 40s", "8s = 40s", "8s = 40s", "8s = 40s", Some(false)),
        (SP_ALERT, T_ALERT, "at 5 to", "at 5 to", "at **5** to", "at **5** to", Some(true)),
        (SP_AUTO, T_AUTO, "example.com/auto and", "example.com/auto and", "<https://example.com/auto> and", "<https://example.com/auto> and", Some(true)),
        (SP_AUTO, T_AUTO, "example.com/auto", "example", "<https://example.com/auto>", "<https://example.com/auto>", Some(true)),
        (SP_AUTO, T_AUTO, "www.example.org bare", "www.example.org bare", "www.example.org bare", "www.example.org bare", None),
        (SP_H1, T_H1, "“layer” — design", "“layer” — design", "\"layer\" — design", "\"layer\" — design", Some(false)),
        (SP_SETEXT, T_SETEXT, "Setext Heading", "Setext Heading", "Setext Heading", "Setext Heading\n-", Some(false)),
        (SP_TERM, T_TERM, "Term one", "Term one", "Term one", "Term one\n: Definition", Some(false)),
        (SP_TERM, T_TERM, "Term one", "one", "one", "Term one\n: Definition", Some(false)),
        (SP_DETAIL, T_DETAIL, "emph – dash", "emph – dash", "*emph* -- dash", "*emph* -- dash", Some(true)),
        (SP_INNER, T_INNER, "café and 🎉", "café and 🎉", "café and 🎉", "café and 🎉", Some(false)),
        (SP_QHEAD, T_QHEAD, "Quoted heading", "Quoted heading", "Quoted heading", "### Quoted heading", Some(false)),
        (SP_BODY, T_BODY, "inside details", "inside details", "inside details", "inside details", Some(false)),
        (SP_TADA, T_TADA, "🎉", "🎉", ":tada:", "| :tada: |", Some(true)),
        (SP_NAIVE, T_NAIVE, "naïve 🎉", "naïve 🎉", "naïve 🎉", "naïve 🎉", Some(false)),
        (SP_FOOT, T_FOOT, "footnote body", "footnote body", "footnote body", "footnote body", Some(false)),
        (SP_PIPE, T_PIPE, "a | b", "a | b", "a \\| b", "a \\| b", Some(false)),
        (SP_PIPE, T_PIPE, "| b", "b", "b", "\\| b |", Some(false)),
    ];
    let bad = check_cases(A, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_code_selection_maps_line_by_line_without_collapsing() {
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (SP_RUST, T_RUST, "\tlet n = 5;", "let n = 5;", "let n = 5;", "\tlet n = 5;", Some(false)),
        (SP_RUST, T_RUST, "\tlet n = 5;", "\tlet n = 5;", "\tlet n = 5;", "{\n\tlet n = 5;", Some(false)),
        (SP_RUST, T_RUST, "fn retry() {\n\tlet n", "fn retry() {\n\tlet n", "fn retry() {\n\tlet n", "fn retry() {\n\tlet n", Some(false)),
        (SP_FENCE, T_FENCE, "  kept indent", "  kept indent", "  kept indent", "    kept indent", Some(false)),
        (SP_FENCE, T_FENCE, "fence\n  kept", "fence\n  kept", "fence\n    kept", "fence\n    kept", Some(false)),
        (SP_INDENTED, T_INDENTED, "second line", "second line", "second line", "second line", Some(false)),
        (SP_FOOT_CODE, T_FOOT_CODE, "foot code", "foot code", "foot code", "    foot code\n", Some(false)),
    ];
    let mut bad = check_cases(A, cases);

    // A tab the fence indent cuts into renders as the columns it has left
    let tab = "  ```\n\tx = 1\n x\n  ```\n";
    let sp = sole_owner(tab, OwnerKind::Code);
    let t = "  x = 1\nx\n";
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "  x = 1", "x = 1", "x = 1", "\tx = 1", Some(false)),
        (&sp, t, "  x = 1", "  x = 1", "\tx = 1", "\tx = 1", None),
        (&sp, t, "1\nx", "1\nx", "1\n x", "1\n x\n", Some(false)),
    ];
    bad.extend(check_cases(tab, cases));

    // Indented code drops four columns, tabs included
    let tabbed = "\t\tcode\n  \tcode2\n";
    let sp = sole_owner(tabbed, OwnerKind::Code);
    let t = "\tcode\ncode2\n";
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "\tcode\n", "\tcode", "\tcode", "\t\tcode\n", Some(false)),
        (&sp, t, "code2", "code2", "code2", "code2", Some(false)),
        (&sp, t, "code\ncode2", "code\ncode2", "code\n  \tcode2", "code\n  \tcode2", Some(false)),
    ];
    bad.extend(check_cases(tabbed, cases));

    // Container prefixes are not part of the code text
    let quoted = "> ```\n> quoted code\n>  two\n> ```\n";
    let sp = sole_owner(quoted, OwnerKind::Code);
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, "quoted code\n two\n", "code\n two", "code\n two", "code\n>  two", "code\n>  two", Some(false)),
    ];
    bad.extend(check_cases(quoted, cases));
    let indented_in_quote = ">   ```\n>  x\n>   ```\n";
    let sp = sole_owner(indented_in_quote, OwnerKind::Code);
    let cases: &[Case] = &[(&sp, "x\n", "x\n", "x", "x", ">  x\n", Some(false))];
    bad.extend(check_cases(indented_in_quote, cases));
    let in_item = "- item\n\n  ```\n  code in item\n  ```\n";
    let sp = sole_owner(in_item, OwnerKind::Code);
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, "code in item\n", "in item", "in item", "in item", "code in item", Some(false)),
    ];
    bad.extend(check_cases(in_item, cases));
    assert!(bad.is_empty(), "{}", bad.join("\n"));

    // Code text is not whitespace-collapsed, so the collapsed form is not in it
    let collapsed = req(SP_RUST, "{ let n", "", "", None, None);
    let (reason, kept, _) = block(anchor::resolve(A, &collapsed));
    assert_eq!((reason, kept.as_str()), (Reason::NoMatch, "{ let n"));
}

#[test]
fn diagram_and_math_fences_stay_block_level() {
    for (sp, quote) in [(SP_MERMAID, "graph TD"), (SP_MATH, "x^2")] {
        let r = anchor::resolve(A, &req(sp, quote, "", "", None, None));
        let (_, kept, _) = block(r);
        assert_eq!(kept, quote, "{sp}");
    }
}

#[test]
fn a_crlf_doc_resolves_to_its_crlf_source() {
    let md = crlf(A);
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (SP_RETRY, T_RETRY, "again. Soft", "again. Soft", "again.\r\nSoft", "again.\r\nSoft", Some(false)),
        (SP_RETRY, T_RETRY, "spaces hard break", "spaces hard break", "spaces  \r\nhard break", "spaces  \r\nhard break", Some(false)),
        (SP_PR, T_PR, "client – because S3", "client – because S3", "client -- because S3", "client -- because S3", Some(true)),
        (SP_TERM, T_TERM, "Term one", "Term one", "Term one", "Term one\r\n:", Some(false)),
        (SP_RUST, T_RUST, "fn retry() {\n\tlet n", "fn retry() {\n\tlet n", "fn retry() {\r\n\tlet n", "fn retry() {\r\n\tlet n", Some(false)),
        (SP_FENCE, T_FENCE, "fence\n  kept", "fence\n  kept", "fence\r\n    kept", "fence\r\n    kept", Some(false)),
    ];
    let bad = check_cases(&md, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn smart_punctuation_runs_widen_to_the_whole_source_run() {
    let md = "a -- b --- c ---- d ----- e ...... f .... g .. h\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    let t = "a – b — c –– d —– e …… f …. g .. h";
    assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some(t));
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "b — c", "b — c", "b --- c", "b --- c", Some(true)),
        (&sp, t, "c –", "–", "----", "c ---- d", Some(true)),
        (&sp, t, "– d", "–", "----", "c ---- d", Some(true)),
        (&sp, t, "d —", "—", "-----", "d ----- e", Some(true)),
        (&sp, t, "e …", "…", "......", "e ...... f", Some(true)),
        (&sp, t, "f …", "…", "....", "f .... g", Some(true)),
        (&sp, t, "…. g", ".", "....", "f .... g", Some(true)),
        (&sp, t, ".. h", ".. h", ".. h", ".. h", Some(false)),
    ];
    let bad = check_cases(md, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn entities_escapes_and_unicode_whitespace_map_to_their_source() {
    let entities = "a&nbsp;&nbsp;b &#x1F389; &bogus; &ngE; x\n";
    let sp = sole_owner(entities, OwnerKind::Paragraph);
    // Two no-break spaces collapse like any whitespace run
    let t = "a b 🎉 &bogus; ≧\u{338} x";
    assert_eq!(anchor::owner_text(entities, &sp).as_deref(), Some(t));
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "a b", "a b", "a&nbsp;&nbsp;b", "a&nbsp;&nbsp;b", Some(true)),
        (&sp, t, "🎉", "🎉", "&#x1F389;", "&#x1F389;", Some(true)),
        (&sp, t, "&bogus;", "&bogus;", "&bogus;", "&bogus;", Some(false)),
        (&sp, t, "≧\u{338}", "≧", "&ngE;", "&ngE;", Some(true)),
    ];
    let mut bad = check_cases(entities, cases);

    // JS `\s` keeps U+0085 and collapses U+FEFF, unlike `char::is_whitespace`
    let spaces = "a\u{85}b a\u{feff}b c\n";
    let sp = sole_owner(spaces, OwnerKind::Paragraph);
    let t = "a\u{85}b a b c";
    assert_eq!(anchor::owner_text(spaces, &sp).as_deref(), Some(t));
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "a b c", "a b", "a\u{feff}b", "a\u{feff}b", Some(false)),
        (&sp, t, "a\u{85}b", "a\u{85}b", "a\u{85}b", "a\u{85}b", Some(false)),
    ];
    bad.extend(check_cases(spaces, cases));
    match exact(anchor::resolve(
        spaces,
        &req(&sp, "a b", "", "", None, None),
    )) {
        Ok(hit) if hit.source == "a\u{feff}b" => {}
        other => bad.push(format!("\"a b\" without context: {other:?}")),
    }

    let escapes = "a \\\\ b \\# c\n";
    let sp = sole_owner(escapes, OwnerKind::Paragraph);
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, "a \\ b # c", "\\ b #", "\\ b #", "\\\\ b \\#", "\\\\ b \\#", Some(false)),
    ];
    bad.extend(check_cases(escapes, cases));
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_repeated_quote_without_position_or_context_gives_both_candidates() {
    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", None, None));
    let (mut items, text_len) = candidates(r);
    assert_eq!(text_len, cp_len(T_RETRY));
    items.sort_by_key(|c| c.bytes.start);
    let got: Vec<_> = items
        .iter()
        .map(|c| (c.sp.as_deref(), c.off, c.source.as_str(), c.bytes.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Some(SP_RETRY),
                Some(cp_span(T_RETRY, "We just fail", "just fail").start),
                "just **fail**",
                span(A, "We just **fail** hard", "just **fail**"),
            ),
            (
                Some(SP_RETRY),
                Some(cp_span(T_RETRY, "we just fail again", "just fail").start),
                "just fail",
                span(A, "we just fail again", "just fail"),
            ),
        ]
    );
}

#[test]
fn the_offset_picks_the_occurrence_when_the_text_length_matches() {
    let tl = Some(cp_len(T_RETRY));
    let first = cp_span(T_RETRY, "We just fail", "just fail").start;
    let second = cp_span(T_RETRY, "we just fail again", "just fail").start;

    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", Some(second), tl));
    let hit = exact(r).unwrap();
    assert_eq!(hit.source, "just fail");
    assert_eq!(hit.bytes, span(A, "we just fail again", "just fail"));

    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", Some(first), tl));
    let hit = exact(r).unwrap();
    assert_eq!(hit.source, "just **fail**");
    assert_eq!(hit.bytes, span(A, "We just **fail** hard", "just **fail**"));
}

#[test]
fn a_wrong_text_length_or_a_stale_offset_falls_back_to_context() {
    let tl = cp_len(T_RETRY);
    let first = cp_span(T_RETRY, "We just fail", "just fail").start;
    let second = cp_span(T_RETRY, "we just fail again", "just fail").start;
    let second_bytes = span(A, "we just fail again", "just fail");

    // The page's text differs, so its offset can't be trusted and context decides
    let wrong_tl = req(
        SP_RETRY,
        "just fail",
        "and we ",
        " again",
        Some(first),
        Some(tl + 1),
    );
    assert_eq!(
        exact(anchor::resolve(A, &wrong_tl)).unwrap().bytes,
        second_bytes
    );

    // An offset where the quote isn't is ignored
    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", Some(0), Some(tl)));
    assert_eq!(candidates(r).0.len(), 2);

    // The fast path needs both the offset and the text length
    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", Some(second), None));
    assert_eq!(candidates(r).0.len(), 2);
    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", None, Some(tl)));
    assert_eq!(candidates(r).0.len(), 2);
}

#[test]
fn context_alone_picks_the_occurrence_it_matches() {
    let first = span(A, "We just **fail** hard", "just **fail**");
    let second = span(A, "we just fail again", "just fail");
    let cases = [
        ("We ", "", &first),
        ("", " hard", &first),
        ("and we ", "", &second),
        ("", " again. Soft", &second),
        // Context scores by how much matches, not by an exact match
        ("Xand we ", "", &second),
    ];
    for (pre, suf, want) in cases {
        let r = anchor::resolve(A, &req(SP_RETRY, "just fail", pre, suf, None, None));
        let hit = exact(r).unwrap_or_else(|e| panic!("{pre:?} {suf:?}: {e}"));
        assert_eq!(&hit.bytes, want, "{pre:?} {suf:?}");
    }

    // Both score 3, so neither wins
    let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "e ", " ", None, None));
    assert_eq!(candidates(r).0.len(), 2);
}

#[test]
fn offsets_count_code_points_not_bytes() {
    let md = "é x é x\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some("é x é x"));

    let r = anchor::resolve(md, &req(&sp, "x", "", "", Some(6), Some(7)));
    assert_eq!(exact(r).unwrap().bytes, 8..9);
    let r = anchor::resolve(md, &req(&sp, "x", "", "", Some(2), Some(7)));
    assert_eq!(exact(r).unwrap().bytes, 3..4);
    let r = anchor::resolve(md, &req(&sp, "é x", "", "", Some(4), Some(7)));
    assert_eq!(exact(r).unwrap().bytes, 5..9);

    let r = anchor::resolve(md, &req(&sp, "x", "", "", None, None));
    let (items, text_len) = candidates(r);
    assert_eq!(text_len, 7);
    let mut got: Vec<(Option<usize>, Range<usize>)> =
        items.into_iter().map(|c| (c.off, c.bytes)).collect();
    got.sort_by_key(|(_, b)| b.start);
    assert_eq!(got, [(Some(2), 3..4), (Some(6), 8..9)]);
}

#[test]
fn every_candidate_resolves_exactly_from_its_own_offset() {
    for quote in ["just fail", "fail"] {
        let r = anchor::resolve(A, &req(SP_RETRY, quote, "", "", None, None));
        let (items, text_len) = candidates(r);
        assert!(items.len() >= 2, "{quote:?}: {items:?}");
        for c in items {
            let at = req(
                c.sp.as_deref().unwrap(),
                quote,
                "",
                "",
                c.off,
                Some(text_len),
            );
            let hit = exact(anchor::resolve(A, &at)).unwrap();
            assert_eq!(hit.bytes, c.bytes, "{quote:?}");
            assert_eq!(A[c.bytes.clone()], c.source, "{quote:?}");
        }
    }
}

#[test]
fn a_sourcepos_that_is_not_a_text_owner_is_bad_sp() {
    let not_owners = [
        "500:1-500:9", // past the end
        "garbage",
        "",
        "3:1-3:212",  // a paragraph's sp, one column short
        "36:1-38:21", // loose list item: its paragraphs own the text
        "9:1-12:37",  // list
        "16:1-16:31", // table row
        "14:1-17:31", // table
        "26:1-28:20", // blockquote
        "19:1-20:56", // alert
        "78:1-79:23", // raw HTML block
        "30:1-31:32", // description item
        // Paragraphs rendered bare inside a tight item or dd
        "9:7-9:31",
        "12:3-12:37",
        "11:5-11:22",
        "31:3-31:32",
    ];
    for sp in not_owners {
        let r = anchor::resolve(A, &req(sp, "we just fail", "", "", Some(0), Some(3)));
        let (reason, kept, _) = block(r);
        assert_eq!(
            (reason, kept.as_str()),
            (Reason::BadSp, "we just fail"),
            "{sp:?}"
        );
    }
}

#[test]
fn a_quote_missing_from_the_text_keeps_the_quote_as_no_match() {
    let r = anchor::resolve(A, &req(SP_PR, "not there", "", "", None, None));
    assert_eq!(
        block(r),
        (Reason::NoMatch, "not there".to_string(), cp_len(T_PR))
    );

    // Text in another block is not a match for this one
    let r = anchor::resolve(A, &req(SP_PR, "We just", "", "", None, None));
    assert_eq!(
        block(r),
        (Reason::NoMatch, "We just".to_string(), cp_len(T_PR))
    );

    // Markup the page doesn't show is not in the text
    let r = anchor::resolve(A, &req(SP_PR, "**adds retry**", "", "", None, None));
    assert_eq!(block(r).0, Reason::NoMatch);

    let r = anchor::resolve(A, &req(SP_PR, "", "", "", Some(0), Some(cp_len(T_PR))));
    assert_eq!(block(r).1, "");
    let r = anchor::resolve(A, &req(SP_PR, "", "", "", None, None));
    assert_eq!(block(r).1, "");
}

#[test]
fn an_offset_out_of_range_falls_back_without_panicking() {
    let tl = cp_len(T_RETRY);
    for off in [tl, tl - 1, usize::MAX] {
        let r = anchor::resolve(A, &req(SP_RETRY, "just fail", "", "", Some(off), Some(tl)));
        assert!(
            matches!(r, Resolved::Candidates { .. } | Resolved::Block { .. }),
            "{off}: {r:?}"
        );
    }
}

#[test]
fn resolve_lists_at_most_five_candidates() {
    let md = "x x x x x x x\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    let (items, text_len) = candidates(anchor::resolve(md, &req(&sp, "x", "", "", None, None)));
    assert_eq!((items.len(), text_len), (5, 13));
}

#[test]
fn an_escaped_pipe_in_a_cell_maps_to_its_source() {
    // comrak's text sourcepos in a cell ends one byte short per escaped pipe
    let md = "| a \\| b |\n|---|\n| x |\n";
    let sp = index_of(md).owners[0].sp.clone();
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, "a | b", "| b", "b", "b", "\\| b |", Some(false)),
        (&sp, "a | b", "a | b", "a | b", "a \\| b", "a \\| b", Some(false)),
    ];
    let bad = check_cases(md, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn an_image_is_one_object_replacement_character() {
    let md = "a ![alt text](i.png) w\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some("a \u{fffc} w"));
    let r = anchor::resolve(md, &req(&sp, "alt", "", "", None, None));
    assert_eq!(block(r).0, Reason::NoMatch);
    let hit = exact(anchor::resolve(
        md,
        &req(&sp, "\u{fffc}", "", "", None, None),
    ))
    .unwrap();
    assert_eq!(
        (hit.source.as_str(), hit.widened),
        ("![alt text](i.png)", true)
    );
}

#[test]
fn nested_emphasis_balances_every_level() {
    let md = "a ***b*** c\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    let hit = exact(anchor::resolve(md, &req(&sp, "b", "", "", None, None))).unwrap();
    assert_eq!((hit.source.as_str(), hit.widened), ("***b***", true));
}

#[test]
fn a_code_span_never_maps_to_a_partial_delimiter() {
    let md = "x `` a`b `` y\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some("x a`b y"));
    let hit = exact(anchor::resolve(md, &req(&sp, "a`b", "", "", None, None))).unwrap();
    assert!(
        ["a`b", "`` a`b ``"].contains(&hit.source.as_str()),
        "{hit:?}"
    );
}

#[test]
fn owner_text_is_the_rendered_text_of_the_owner() {
    let want = [
        (SP_H1, T_H1),
        (SP_PR, T_PR),
        (SP_RETRY, T_RETRY),
        (SP_TASK, T_TASK),
        (SP_CHECKED, T_CHECKED),
        (SP_NESTED, T_NESTED),
        (SP_PLAIN, T_PLAIN),
        (SP_KNOB, T_KNOB),
        (SP_RETRIES, T_RETRIES),
        (SP_TIMES, T_TIMES),
        (SP_ALERT, T_ALERT),
        (SP_AUTO, T_AUTO),
        (SP_TERM, T_TERM),
        (SP_DETAIL, T_DETAIL),
        (SP_SETEXT, T_SETEXT),
        (SP_RUST, T_RUST),
        (SP_FENCE, T_FENCE),
        (SP_INDENTED, T_INDENTED),
        (SP_INNER, T_INNER),
        (SP_QHEAD, T_QHEAD),
        (SP_BODY, T_BODY),
        (SP_TADA, T_TADA),
        (SP_NAIVE, T_NAIVE),
        (SP_FOOT, T_FOOT),
        (SP_FOOT_CODE, T_FOOT_CODE),
        (SP_PIPE, T_PIPE),
        ("96:1-97:14", "parent item"),
        ("97:3-97:14", "only child"),
        ("99:1-99:39", "Second ref\u{fffc} with blocks in its note."),
        ("101:7-101:22", "Second footnote."),
    ];
    let mut bad = Vec::new();
    for (sp, t) in want {
        let got = anchor::owner_text(A, sp);
        if got.as_deref() != Some(t) {
            bad.push(format!("{sp}: {got:?}, want {t:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn owner_text_is_none_for_anything_but_an_owner() {
    for sp in [
        "36:1-38:21",
        "16:1-16:31",
        "26:1-28:20",
        "78:1-79:23",
        "",
        "x",
    ] {
        assert_eq!(anchor::owner_text(A, sp), None, "{sp:?}");
    }
}

#[test]
fn a_crlf_doc_has_the_same_owner_texts() {
    let md = crlf(A);
    let mut bad = Vec::new();
    for o in index_of(A).owners {
        let (lf, cr) = (anchor::owner_text(A, &o.sp), anchor::owner_text(&md, &o.sp));
        if lf.is_none() || lf != cr {
            bad.push(format!("{}: {lf:?} vs {cr:?}", o.sp));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn locate_maps_source_bytes_to_the_owner_offset() {
    let md_crlf = crlf(A);
    #[rustfmt::skip]
    let cases = vec![
        (A, span(A, "\"we just fail\"", "we just fail"), SP_PR, cp_span(T_PR, "“we just fail”", "we just fail"), T_PR),
        (A, span(A, "client -- because", "--"), SP_PR, cp_span(T_PR, "client – because", "–"), T_PR),
        (A, span(A, "spaces  \nhard", "spaces  \nhard"), SP_RETRY, cp_span(T_RETRY, "spaces hard", "spaces hard"), T_RETRY),
        (A, span(A, "nested item", "nested item"), SP_NESTED, 0..11, T_NESTED),
        (A, span(A, "Term one\n:", "Term one"), SP_TERM, 0..8, T_TERM),
        (A, span(A, "café and 🎉", "and 🎉"), SP_INNER, cp_span(T_INNER, "and 🎉", "and 🎉"), T_INNER),
        (A, span(A, "\tlet n = 5;", "let n = 5;"), SP_RUST, cp_span(T_RUST, "let n = 5;", "let n = 5;"), T_RUST),
        (A, span(A, "    foot code", "foot code"), SP_FOOT_CODE, 0..9, T_FOOT_CODE),
        (md_crlf.as_str(), span(&md_crlf, "fn retry() {\r\n\tlet", "fn retry() {\r\n\tlet"), SP_RUST, 0..17, T_RUST),
    ];
    for (md, bytes, sp, off, t) in cases {
        let got = loc_parts(anchor::locate(md, bytes.clone()));
        let want = Some((sp.to_string(), (off.start, off.end), cp_len(t)));
        assert_eq!(got, want, "{:?}", &md[bytes]);
    }
}

#[test]
fn locate_is_none_when_the_range_leaves_one_owner() {
    let ranges = [
        // An item's own text into its nested item
        span(A, "POST\n  - nested", "POST\n  - nested"),
        span(A, "\\*panic\\*.\n\nWe just", "\\*panic\\*.\n\nWe just"),
        span(A, "retries | 5", "retries | 5"),
        span(A, "<summary>More</summary>", "More"),
        A.len()..A.len() + 3,
    ];
    for bytes in ranges {
        let got = loc_parts(anchor::locate(A, bytes.clone()));
        assert_eq!(got, None, "{bytes:?}");
    }
}

/// `search` results as `(sp, source, bytes)`, in source order.
fn found(md: &str, text: &str) -> Vec<(Option<String>, String, Range<usize>)> {
    let mut v: Vec<_> = anchor::search(md, text)
        .into_iter()
        .map(|c| (c.sp, c.source, c.bytes))
        .collect();
    v.sort_by_key(|(_, _, b)| b.start);
    v
}

fn owner_hit(
    sp: &str,
    source: &str,
    bytes: Range<usize>,
) -> (Option<String>, String, Range<usize>) {
    (Some(sp.to_string()), source.to_string(), bytes)
}

#[test]
fn search_finds_typed_text_in_every_owner() {
    assert_eq!(
        found(A, "loose one"),
        [
            owner_hit(
                "36:3-36:11",
                "loose one",
                span(A, "- loose one\n", "loose one")
            ),
            owner_hit(
                "38:3-38:21",
                "loose one",
                span(A, "loose one continued", "loose one")
            ),
        ]
    );
    assert_eq!(
        found(A, "client – because"),
        [owner_hit(
            SP_PR,
            "client -- because",
            span(A, "client -- because", "client -- because")
        )]
    );
    // Typed whitespace collapses like the page's
    assert_eq!(
        found(A, "line  two   with"),
        [owner_hit(
            SP_RETRY,
            "line two  with",
            span(A, "line two  with", "line two  with")
        )]
    );
    assert_eq!(
        found(A, "let n = 5"),
        [owner_hit(
            SP_RUST,
            "let n = 5",
            span(A, "let n = 5", "let n = 5")
        )]
    );
    assert_eq!(found(A, "zzz nowhere"), []);
    assert_eq!(found(A, ""), []);
    assert_eq!(found(A, "   "), []);
}

#[test]
fn search_falls_back_to_raw_source_only_when_no_owner_matches() {
    let more = span(A, "<summary>More</summary>", "More");
    assert_eq!(found(A, "More"), [(None, "More".to_string(), more)]);

    // `details` is also in the raw `<details>` tags, but owners match first
    let sps: Vec<Option<String>> = found(A, "details").into_iter().map(|h| h.0).collect();
    let want = [Some("70:1-70:22".to_string()), Some(SP_BODY.to_string())];
    assert_eq!(sps, want);
}

#[test]
fn search_returns_at_most_five() {
    let md = "x\n\nx\n\nx\n\nx\n\nx\n\nx\n\nx\n";
    assert_eq!(anchor::search(md, "x").len(), 5);
}

#[test]
fn search_previews_name_the_heading_or_quote_the_text() {
    let previews = |text: &str| -> Vec<String> {
        anchor::search(A, text)
            .into_iter()
            .map(|c| c.preview)
            .collect()
    };
    assert_eq!(previews("Quotes and"), ["→ ### Quotes and details"]);

    let quote = previews("Body paragraph");
    assert_eq!(quote.len(), 1);
    let q = &quote[0];
    assert!(
        q.starts_with("→ \"") && q.ends_with('"') && q.contains("Body paragraph"),
        "{q:?}"
    );
}

#[test]
fn search_hits_resolve_exactly_from_their_offset() {
    for text in [
        "loose one",
        "client – because",
        "details",
        "let n = 5",
        "Term one",
    ] {
        let hits = anchor::search(A, text);
        assert!(!hits.is_empty(), "{text:?}");
        for c in hits {
            let sp = c.sp.clone().unwrap();
            let tl = cp_len(&anchor::owner_text(A, &sp).unwrap());
            let r = anchor::resolve(A, &req(&sp, text, "", "", c.off, Some(tl)));
            let hit = exact(r).unwrap_or_else(|e| panic!("{text:?} in {sp}: {e}"));
            assert_eq!(
                (hit.bytes, hit.source),
                (c.bytes, c.source),
                "{text:?} in {sp}"
            );
        }
    }
}

// The property sweep: every owner in every fixture, every window of its text.

/// Window lengths the default sweep takes at every start; the ignored sweep takes all of 1..=40.
const WINDOW_LENS: [usize; 5] = [1, 2, 5, 13, 40];

fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// Drops the delimiters a quote never shows, straightens quotes and collapses whitespace.
fn unmark(s: &str) -> String {
    let s: String = s
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '~' | '`'))
        .map(|c| match c {
            '“' | '”' => '"',
            '‘' | '’' => '\'',
            c => c,
        })
        .collect();
    s.split(is_js_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Source text as the page shows it, minus escapes and inline tags.
fn plain(source: &str) -> String {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|n| n.is_ascii_punctuation()) => {
                out.push(chars.next().unwrap())
            }
            '<' if chars
                .peek()
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/') =>
            {
                for n in chars.by_ref() {
                    if n == '>' {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    unmark(&out)
}

fn unescape(source: &str) -> String {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' || !chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            out.push(c);
        }
    }
    out
}

fn alnum_runs(quote: &str) -> impl Iterator<Item = &str> {
    quote
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|run| !run.is_empty())
}

/// Mermaid and math fences render as pictures, so they have no selectable text.
fn is_diagram(md: &str, o: &Owner) -> bool {
    if o.kind != OwnerKind::Code {
        return false;
    }
    let line = md.lines().nth(o.lines.0 as usize - 1).unwrap_or("");
    let info = line.trim_start().trim_start_matches(['`', '~']).trim();
    info.starts_with("mermaid") || info.starts_with("math")
}

fn sweep(lens: &[usize]) {
    let mut bad = Vec::new();
    let mut windows = 0usize;
    'fixtures: for (name, md) in fixtures() {
        for o in index_of(&md).owners {
            if is_diagram(&md, &o) {
                continue;
            }
            let Some(t) = anchor::owner_text(&md, &o.sp) else {
                bad.push(format!("{name} {}: no owner text", o.sp));
                continue;
            };
            let chars: Vec<char> = t.chars().collect();
            let lines = line_range(&md, o.lines);
            for a in 0..chars.len() {
                for &len in lens {
                    let b = a + len;
                    if b > chars.len() {
                        break;
                    }
                    if chars[a].is_whitespace() {
                        continue;
                    }
                    windows += 1;
                    let sel = select(&t, a, len);
                    if let Err(e) = check_window(&md, &o, &lines, &sel) {
                        bad.push(format!("{name} {} [{a},{b}) {:?}: {e}", o.sp, sel.quote));
                        if bad.len() >= 40 {
                            break 'fixtures;
                        }
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(windows > 1000, "only {windows} windows");
}

/// With its offset a window resolves exactly and locates back to itself; by context alone it
/// resolves to the same bytes or lists them among the candidates.
fn check_window(md: &str, o: &Owner, lines: &Range<usize>, sel: &Sel) -> Result<(), String> {
    let hit = exact(anchor::resolve(md, &sel.with_off(&o.sp)))?;
    let quoted = (sel.off, sel.off + cp_len(&sel.quote));
    if !md.is_char_boundary(hit.bytes.start) || !md.is_char_boundary(hit.bytes.end) {
        return Err(format!("bytes {:?} split a character", hit.bytes));
    }
    // comrak's dt sourcepos covers one byte, and an indented code block's stops before its
    // last newline, so those are bounded by their lines
    let by_lines = matches!(o.kind, OwnerKind::Term | OwnerKind::Code);
    let inside = contains(lines, &hit.bytes) && (by_lines || contains(&o.bytes, &hit.bytes));
    if !inside {
        return Err(format!("bytes {:?} outside owner {:?}", hit.bytes, o.bytes));
    }
    if md.get(hit.bytes.clone()) != Some(hit.source.as_str()) {
        return Err(format!(
            "source {:?} is not md[{:?}]",
            hit.source, hit.bytes
        ));
    }
    if hit.shown != sel.quote || hit.text_len != sel.tl {
        return Err(format!("shown {:?}, text_len {}", hit.shown, hit.text_len));
    }
    if !hit.widened && o.kind != OwnerKind::Code && plain(&hit.source) != unmark(&sel.quote) {
        return Err(format!(
            "unwidened {:?} doesn't read as the quote",
            hit.source
        ));
    }
    let source = unescape(&hit.source);
    if let Some(run) = alnum_runs(&sel.quote).find(|run| !source.contains(run)) {
        return Err(format!("{:?} is missing {run:?}", hit.source));
    }
    match loc_parts(anchor::locate(md, hit.bytes.clone())) {
        Some((sp, off, tl))
            if sp == o.sp
                && tl == sel.tl
                && off.0 <= quoted.0
                && off.1 >= quoted.1
                && (hit.widened || off == quoted) => {}
        other => return Err(format!("locate({:?}) gave {other:?}", hit.bytes)),
    }

    match anchor::resolve(md, &sel.by_context(&o.sp)) {
        Resolved::Exact { bytes, .. } if bytes == hit.bytes => Ok(()),
        Resolved::Candidates { items, .. }
            if items.len() == 5
                || items
                    .iter()
                    .any(|c| c.sp.as_deref() == Some(o.sp.as_str()) && c.bytes == hit.bytes) =>
        {
            Ok(())
        }
        other => Err(format!("by context: {other:?}, want {:?}", hit.bytes)),
    }
}

#[test]
#[ignore = "sweeps every owner; slow in a debug build, run in CI with --ignored"]
fn every_window_resolves_back_to_itself() {
    sweep(&WINDOW_LENS);
}

#[test]
#[ignore = "every window up to 40 code points; slow in a debug build"]
fn every_window_resolves_back_to_itself_full() {
    sweep(&(1..=40).collect::<Vec<_>>());
}

const SP_RAW_BR: &str = "121:2-121:9";
const SP_RAW_IMG: &str = "123:1-123:65";
const T_RAW_IMG: &str = "Raw \u{fffc} image and upper break.";

#[test]
fn raw_inline_breaks_and_images_read_as_the_page_shows_them() {
    assert_eq!(anchor::owner_text(A, SP_RAW_BR).as_deref(), Some("a b"));
    assert_eq!(
        anchor::owner_text(A, SP_RAW_IMG).as_deref(),
        Some(T_RAW_IMG)
    );
    for (md, t) in [
        ("x <br/> y\n", "x y"),
        ("a<BR class=\"q\">b\n", "a b"),
        ("a<br\n  />b\n", "a b"),
        ("x <img src=y> z\n", "x \u{fffc} z"),
        ("x <IMG\tsrc=\"y\"/>z\n", "x \u{fffc}z"),
        // Only the tag names themselves; other raw tags keep just their text
        ("a<bra>b</bra>c\n", "abc"),
        ("a<imgx>b</imgx>c\n", "abc"),
        ("a <span hidden>b</span> c\n", "a b c"),
    ] {
        let sp = sole_owner(md, OwnerKind::Paragraph);
        assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some(t), "{md:?}");
    }
}

#[test]
fn raw_inline_breaks_and_images_resolve_to_their_tags() {
    // (sp, T, quote, source, unique context of the source, widened)
    #[rustfmt::skip]
    let cases: &[(&str, &str, &str, &str, &str, bool)] = &[
        (SP_RAW_BR, "a b", "a b", "a<br>b", "| a<br>b |", true),
        (SP_RAW_BR, "a b", "b", "b", "b | raw break", false),
        (SP_RAW_IMG, T_RAW_IMG, "Raw \u{fffc} image", "Raw <img src=\"nowhere.png\" alt=\"pic\"> image", "Raw <img", true),
        (SP_RAW_IMG, T_RAW_IMG, "\u{fffc}", "<img src=\"nowhere.png\" alt=\"pic\">", "Raw <img", true),
        (SP_RAW_IMG, T_RAW_IMG, "and upper", "and <BR/>upper", "and <BR/>upper", false),
    ];
    let mut bad = Vec::new();
    for &(sp, t, quote, source, context, widened) in cases {
        let at = A.find(context).expect("context")
            + context.find(source.get(..4).unwrap_or(source)).unwrap();
        let want = at..at + source.len();
        assert_eq!(&A[want.clone()], source);
        let sel = select_in(t, t, quote);
        for r in [sel.with_off(sp), sel.by_context(sp)] {
            match exact(anchor::resolve(A, &r)) {
                Ok(hit) if hit.bytes == want && hit.source == source && hit.widened == widened => {}
                other => bad.push(format!(
                    "{sp} {quote:?}: {other:?}, want {source:?} widened {widened}"
                )),
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

// comrak misplaces some inline sourcepos; the resolver must neither panic nor quote wrong text.

/// Resolves, searches and locates everything in `md`, returning the Exact results as
/// `(quote, source, widened)`. Any panic fails the caller.
fn exercise(md: &str) -> Vec<(String, String, bool)> {
    let mut exacts = Vec::new();
    for o in index_of(md).owners {
        let t = anchor::owner_text(md, &o.sp).unwrap();
        let n = cp_len(&t);
        for (a, b) in [(0, n), (0, n / 2), (n / 2, n), (n / 3, n - n / 3)] {
            if a >= b {
                continue;
            }
            let sel = select(&t, a, b - a);
            for r in [
                anchor::resolve(md, &sel.with_off(&o.sp)),
                anchor::resolve(md, &sel.by_context(&o.sp)),
            ] {
                if let Resolved::Exact {
                    source, widened, ..
                } = r
                {
                    exacts.push((sel.quote.clone(), source, widened));
                }
            }
            anchor::search(md, &sel.quote);
        }
    }
    for a in 0..=md.len() + 1 {
        anchor::locate(md, a..md.len() + 1);
        anchor::locate(md, 0..a);
    }
    exacts
}

/// Letters and digits in order, so markup, URLs and shortcode names drop out.
fn alnum(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// An Exact source says what the quote says: the same letters, or more of them when widened.
fn reads_as_quote(quote: &str, source: &str, widened: bool) -> bool {
    // Entities and raw HTML carry letters the page doesn't show
    if source.contains('&') || source.contains('<') {
        return true;
    }
    let (q, s) = (alnum(quote), alnum(source));
    let mut rest = s.chars();
    q.chars().all(|c| rest.any(|d| d == c)) && (widened || q == s)
}

#[test]
fn misplaced_sourcepos_never_panics() {
    let docs = [
        "\\|文http://.||\n-|-",
        "See\n  the `flag\nvalue`— here",
        " a||\n-|-\n$z$\u{feff}",
    ];
    for md in docs {
        for (quote, source, widened) in exercise(md) {
            assert!(
                reads_as_quote(&quote, &source, widened),
                "{md:?}: {quote:?} gave {source:?}"
            );
        }
    }
}

#[test]
fn a_multi_line_code_span_maps_past_its_misplaced_end() {
    // comrak ends this span two bytes late, inside the em dash
    let md = "See\n  the `flag\nvalue`— here\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    let t = "See the flag value— here";
    assert_eq!(anchor::owner_text(md, &sp).as_deref(), Some(t));
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (&sp, t, "flag value", "flag value", "flag\nvalue", "flag\nvalue", Some(false)),
        (&sp, t, "value— here", "value— here", "value`— here", "value`— here", Some(false)),
        (&sp, t, "the flag", "the flag", "the `flag", "the `flag", Some(false)),
    ];
    let bad = check_cases(md, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn inlines_after_an_escaped_pipe_map_to_their_source() {
    // comrak places a cell's inlines one byte early for each `\|` before them
    let md = "| a | b |\n|---|---|\n| x \\| see [docs](u) | y |\n| p \\| q \\| **é** r | <https://a.b> \\| `c\\|d` z |\n";
    let cells: Vec<String> = index_of(md)
        .owners
        .into_iter()
        .filter(|o| o.lines.0 >= 3 && o.kind == OwnerKind::Cell)
        .map(|o| o.sp)
        .collect();
    let (link, emph, auto) = (&cells[0], &cells[2], &cells[3]);
    let (t_link, t_emph, t_auto) = ("x | see docs", "p | q | é r", "https://a.b | c|d z");
    for (sp, t) in [(link, t_link), (emph, t_emph), (auto, t_auto)] {
        assert_eq!(anchor::owner_text(md, sp).as_deref(), Some(t));
    }
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (link, t_link, "docs", "docs", "[docs](u)", "[docs](u)", Some(true)),
        (link, t_link, "x | see", "x | see", "x \\| see", "x \\| see", Some(false)),
        (emph, t_emph, "é", "é", "**é**", "**é**", Some(true)),
        (emph, t_emph, "q | é r", "q | é r", "q \\| **é** r", "q \\| **é** r", Some(true)),
        (auto, t_auto, "a.b", "a.b", "<https://a.b>", "<https://a.b>", Some(true)),
        (auto, t_auto, "c|d", "c|d", "c\\|d", "`c\\|d`", Some(false)),
        (auto, t_auto, "d z", "d z", "d` z", "d` z", Some(false)),
    ];
    let bad = check_cases(md, cases);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_text_that_opens_with_an_escape_keeps_its_backslash() {
    let md = "\\*x y\n";
    let sp = sole_owner(md, OwnerKind::Paragraph);
    let hit = exact(anchor::resolve(md, &req(&sp, "*x", "", "", None, None))).unwrap();
    assert_eq!(hit.source, "\\*x");
}

#[test]
fn rows_under_an_indented_header_stay_block_level() {
    // comrak shifts these rows by the header's indent; the checks catch text and tokens alike
    for (md, quote) in [
        (" a|b\n-|-\nxy|z\n", "xy"),
        (" a|b\n-|-\n$x$|z\n", "\u{fffc}"),
        (" a|b\n-|-\n:tada:|z\n", "🎉"),
    ] {
        let sp = index_of(md)
            .owners
            .into_iter()
            .find(|o| o.lines.0 == 3)
            .unwrap()
            .sp;
        let r = anchor::resolve(md, &req(&sp, quote, "", "", None, None));
        assert_eq!(block(r).0, Reason::NoMatch, "{md:?}");
    }
}

/// A small xorshift generator, so the fuzz loop is the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if n == 0 {
            0
        } else {
            (self.0 % n as u64) as usize
        }
    }
}

#[rustfmt::skip]
const FRAGMENTS: &[&str] = &[
    "é", "🎉", "Été", "中文", "\t", "  ", "\n", "\r\n", "\r", "\n\n", "\\|", "|", "\\", "`", "``",
    "*", "_", "**", "~~", "&amp;", "&#35;", "&#x1F389;", "&nbsp;", "&copy", "--", "---", "...",
    "\"", "'", ":tada:", "$x$", "$$y$$", "$`z`$", "[l](u)", "[^1]", "[^1]: note ", "<br>",
    "<span hidden>h</span>", "<img src=x>", "<b>", "</b>", "# ", "Title\n===\n", "- ", "1. ",
    "- [ ] ", "> ", ">\t", "    ", "```\n", "```mermaid\n", "~~~\n", "| a | b |\n|---|---|\n",
    "| é\\|x | `c\\|d` |\n", "Term\n: detail\n", "<details>\n<summary>More</summary>\n", "word",
    "text ", "x", "https://example.com", "<https://a.b>", "\u{feff}", "\u{85}", "\u{a0}",
    "a\u{301}", " see [docs](u) ", " **é** ",
];

#[test]
fn generated_docs_never_panic_or_quote_the_wrong_text() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut bad = Vec::new();
    for _ in 0..600 {
        let md: String = (0..1 + rng.below(30))
            .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
            .collect();
        for (quote, source, widened) in exercise(&md) {
            if !reads_as_quote(&quote, &source, widened) {
                bad.push(format!("{md:?}: {quote:?} gave {source:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// How long one parse of `md` takes, the part of every call the resolver doesn't add.
fn parse_time(md: &str) -> std::time::Duration {
    let start = std::time::Instant::now();
    let arena = Arena::new();
    parse_document(&arena, md, &anchor::parse_options());
    start.elapsed()
}

fn timed<R>(f: impl FnOnce() -> R) -> (std::time::Duration, R) {
    let start = std::time::Instant::now();
    let out = f();
    (start.elapsed(), out)
}

#[test]
fn long_context_costs_no_more_than_the_page_sends() {
    let md = "ab ".repeat(70_000);
    let sp = sole_owner(&md, OwnerKind::Paragraph);
    let huge = "ab ".repeat(70_000);
    let r = req(&sp, "ab", &huge, &huge, None, None);
    let (took, r) = timed(|| anchor::resolve(&md, &r));
    assert_eq!(candidates(r).0.len(), 5);
    let beyond = took.saturating_sub(parse_time(&md));
    assert!(beyond < std::time::Duration::from_secs(1), "{beyond:?}");
}

#[test]
#[ignore = "timing on 1 MB; noisy under load, run in CI with --ignored"]
fn balancing_stays_linear_in_the_emphasis_count() {
    // 1 MB of emphasis; debug comrak alone takes about a second to parse it
    let md = "*a* ".repeat(250_000);
    let sp = sole_owner(&md, OwnerKind::Paragraph);
    let t = vec!["a"; 250_000].join(" ");
    let tl = cp_len(&t);
    let parse = parse_time(&md);
    let within = |what: &str, took: std::time::Duration| {
        let beyond = took.saturating_sub(parse);
        assert!(
            beyond < std::time::Duration::from_secs(1),
            "{what}: {beyond:?}"
        );
    };

    let (took, r) = timed(|| anchor::resolve(&md, &req(&sp, &t, "", "", Some(0), Some(tl))));
    assert_eq!(exact(r).unwrap().bytes, 0..md.len() - 1);
    within("whole", took);

    // Even offsets hold an `a`, odd ones a space
    let middle = req(&sp, "a a", "", "", Some((tl / 2) & !1), Some(tl));
    let (took, r) = timed(|| anchor::resolve(&md, &middle));
    assert_eq!(exact(r).unwrap().source, "*a* *a*");
    within("middle", took);

    let (took, found) = timed(|| anchor::search(&md, "a a"));
    assert_eq!(found.len(), 5);
    within("search", took);

    let (took, loc) = timed(|| anchor::locate(&md, 4..7));
    assert_eq!(loc.map(|l| l.off), Some((2, 3)));
    within("locate", took);
}
