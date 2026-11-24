use std::fs;

use birta_core::render::{Extracted, RenderOptions, Resolved, Resolver, extract, render};

/// Expands every note embed and links every tag, so `render` takes the paths
/// that rewrite the tree and render nested notes.
struct Expanding;

impl Resolver for Expanding {
    fn wikilink(&self, target: &str) -> Resolved {
        Resolved::Missing(format!("/new/{target}"))
    }
    fn embed_note(&self, name: &str) -> Option<String> {
        Some(format!(
            "# {name}\n\nInner [[inner-{name}]] #inner ![[inner.png]]\n"
        ))
    }
    fn tag(&self, name: &str) -> Option<String> {
        Some(format!("/tags/{name}"))
    }
}

fn fixtures() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = [
        "tests/fixtures",
        "tests/fixtures/diff",
        "tests/fixtures/extract",
    ]
    .iter()
    .flat_map(|dir| fs::read_dir(dir).unwrap())
    .map(|e| e.unwrap().path())
    .filter(|p| p.extension().is_some_and(|e| e == "md"))
    .collect();
    paths.sort();
    assert!(paths.len() > 30, "fixtures not found: {paths:?}");
    paths
}

fn all_on(resolver: Option<&dyn Resolver>) -> RenderOptions<'_> {
    RenderOptions {
        resolver,
        wikilinks: true,
        embeds: true,
        tags: true,
        front_matter: true,
        highlight: true,
        ..Default::default()
    }
}

fn assert_matches_render(markdown: &str, opts: &RenderOptions<'_>, name: &str) -> Extracted {
    let rendered = render(markdown, opts);
    let extracted = extract(markdown, opts);
    assert_eq!(extracted.wikilinks, rendered.wikilinks, "{name}: wikilinks");
    assert_eq!(extracted.embeds, rendered.embeds, "{name}: embeds");
    assert_eq!(extracted.tags, rendered.tags, "{name}: tags");
    assert_eq!(
        extracted.front_matter, rendered.front_matter,
        "{name}: front matter"
    );
    extracted
}

#[test]
fn extract_matches_render_on_every_fixture() {
    let expanding = Expanding;
    for path in fixtures() {
        let name = path.display().to_string();
        let markdown = fs::read_to_string(&path).unwrap();
        assert_matches_render(&markdown, &all_on(None), &name);
        assert_matches_render(
            &markdown,
            &all_on(Some(&expanding)),
            &format!("{name} (resolver)"),
        );
        let off = extract(&markdown, &RenderOptions::default());
        assert!(
            off.wikilinks.is_empty() && off.embeds.is_empty() && off.tags.is_empty(),
            "{name}: off"
        );
        assert!(
            off.wikilink_spans.is_empty() && off.embed_spans.is_empty(),
            "{name}: off spans"
        );
    }
}

#[test]
fn spans_point_at_each_occurrence_in_the_source() {
    for path in fixtures() {
        let name = path.display().to_string();
        let markdown = fs::read_to_string(&path).unwrap();
        let extracted = extract(&markdown, &all_on(None));
        for (spans, open, list) in [
            (&extracted.wikilink_spans, "[[", &extracted.wikilinks),
            (&extracted.embed_spans, "![[", &extracted.embeds),
        ] {
            for span in spans {
                let text = &markdown[span.range.clone()];
                assert!(
                    text.starts_with(open) && text.ends_with("]]"),
                    "{name}: {text:?}"
                );
                assert!(
                    text.contains(span.target.as_str()) || text.contains("\\|"),
                    "{name}: {text:?} vs {}",
                    span.target
                );
            }
            assert!(
                spans.windows(2).all(|w| w[0].range.end <= w[1].range.start),
                "{name}: order"
            );
            // Every reported target has at least one span, and every span's target is reported
            for target in list {
                assert!(
                    spans.iter().any(|s| &s.target == target),
                    "{name}: no span for {target}"
                );
            }
            for span in spans {
                assert!(
                    list.contains(&span.target),
                    "{name}: unreported {}",
                    span.target
                );
            }
        }
    }
}

#[test]
fn edge_case_fixture_finds_what_obsidian_would() {
    let markdown = fs::read_to_string("tests/fixtures/extract/edge-cases.md").unwrap();
    let e = extract(&markdown, &all_on(None));
    // Paragraph-only note embeds come first, as render reports them
    assert_eq!(
        e.embeds.first().map(String::as_str),
        Some("Standalone Note")
    );
    assert!(
        e.embeds.contains(&"pic.png".to_string()) && e.embeds.contains(&"cell.png".to_string())
    );
    assert!(
        !e.embeds
            .iter()
            .any(|x| x.contains("not-embed") || x.contains("entity") || x == "no")
    );
    // comrak reads `[[Table\|Pipe]]` as target `Table` titled `Pipe`
    assert!(e.wikilinks.contains(&"Table".to_string()));
    assert!(!e.tags.iter().any(|t| {
        ["not-tag", "entity", "no", "1", "2026", "front", "matter"].contains(&t.as_str())
    }));
    for tag in [
        "tag-in-heading",
        "first",
        "second/nested",
        "celltag",
        "ok-1",
        "quoted",
        "listtag",
        "ünïcode-täg",
    ] {
        assert!(
            e.tags.contains(&tag.to_string()),
            "missing #{tag}: {:?}",
            e.tags
        );
    }
    // Duplicates are reported once, but each occurrence has a span
    assert_eq!(
        e.wikilink_spans
            .iter()
            .filter(|s| s.target == "Target")
            .count(),
        3
    );
    assert_eq!(e.wikilinks.iter().filter(|t| *t == "Target").count(), 1);
}

fn spans_of<'m>(markdown: &'m str, extracted: &Extracted) -> (Vec<&'m str>, Vec<&'m str>) {
    let slice = |spans: &[birta_core::render::TargetSpan]| {
        spans
            .iter()
            .map(|s| &markdown[s.range.clone()])
            .collect::<Vec<_>>()
    };
    (
        slice(&extracted.wikilink_spans),
        slice(&extracted.embed_spans),
    )
}

#[test]
fn wikilink_spans_survive_table_column_drift() {
    for (markdown, expected) in [
        (
            "a | b\n--|--\n[[A\\|B]] [[C]] | x",
            vec!["[[A\\|B]]", "[[C]]"],
        ),
        ("a | b\n--|--\n`x\\|y` [[C]] | x", vec!["[[C]]"]),
        ("a | b\n--|--\n![[q.png\\|20]] [[C]] | x", vec!["[[C]]"]),
        ("\u{feff}a | b\n--|--\n![[p.png]] [[C]] | x", vec!["[[C]]"]),
        (
            "\u{feff}a | [[H]]\n--|--\n[[C]] | [[D]]",
            vec!["[[H]]", "[[C]]", "[[D]]"],
        ),
    ] {
        let e = assert_matches_render(markdown, &all_on(None), markdown);
        assert_eq!(spans_of(markdown, &e).0, expected, "{markdown:?}");
    }
}

#[test]
fn wikilink_spans_are_in_source_order() {
    let markdown = "x[^1]\n\n[^1]: [[B]]\n\n[[A]]";
    let e = extract(markdown, &all_on(None));
    assert_eq!(spans_of(markdown, &e).0, ["[[B]]", "[[A]]"]);
}

#[test]
fn a_lone_embed_with_an_escaped_pipe_is_one_target() {
    let expanding = Expanding;
    for markdown in [
        "![[q.png\\|20]]",
        "![[Note\\|x]]",
        "a | b\n--|--\n![[Note\\|x]] | y",
    ] {
        for resolver in [None, Some(&expanding as &dyn Resolver)] {
            let e = assert_matches_render(markdown, &all_on(resolver), markdown);
            assert_eq!(e.embeds.len(), 1, "{markdown:?}: {:?}", e.embeds);
            assert!(!e.embeds[0].ends_with('\\'), "{markdown:?}: {:?}", e.embeds);
            assert_eq!(e.embed_spans.len(), 1, "{markdown:?}");
        }
    }
}

fn embeds_only() -> RenderOptions<'static> {
    RenderOptions {
        embeds: true,
        tags: true,
        ..Default::default()
    }
}

#[test]
fn escaped_embeds_in_table_cells_stay_text() {
    for markdown in [
        "a | b\n--|--\n\\![[e.png]] | y",
        "a | b\n--|--\nx\\|y \\![[e.png]] | z",
        "a | b\n--|--\n`p\\|q` \\![[e.png]] ![[f.png]] | z",
    ] {
        let rendered = render(markdown, &embeds_only());
        assert!(
            !rendered.embeds.contains(&"e.png".to_string()),
            "{markdown:?}: {:?}",
            rendered.embeds
        );
        assert!(
            !rendered.html.contains("src=\"e.png\""),
            "{markdown:?}: {}",
            rendered.html
        );
        assert_matches_render(markdown, &embeds_only(), markdown);
    }
    let markdown = "a | b\n--|--\n`p\\|q` \\![[e.png]] ![[f.png]] | z";
    assert_eq!(
        render(markdown, &embeds_only())
            .html
            .matches("<img")
            .count(),
        1
    );
}

#[test]
fn entity_spelled_embeds_do_not_hide_or_duplicate_real_ones() {
    for (markdown, expected) in [
        ("&#33;[[E]] ![[p.png]]", vec!["p.png"]),
        ("![[a.png]] &#33;[[E]] ![[p.png]]", vec!["a.png", "p.png"]),
        (
            "&#33;[[e.png]] ![[p.png]] ![[q.png]]",
            vec!["p.png", "q.png"],
        ),
        ("!\\[[x.png]] ![[p.png]]", vec!["p.png"]),
        (
            "it's ![[p.png]] -- and ![[q.png]]...",
            vec!["p.png", "q.png"],
        ),
    ] {
        let rendered = render(markdown, &embeds_only());
        assert_eq!(rendered.embeds, expected, "{markdown:?}");
        assert_eq!(
            rendered.html.matches("<img").count(),
            expected.len(),
            "{markdown:?}: {}",
            rendered.html
        );
        let e = assert_matches_render(markdown, &embeds_only(), markdown);
        assert_eq!(e.embed_spans.len(), expected.len(), "{markdown:?}");
    }
}

#[test]
fn a_bom_keeps_table_cell_embeds_and_tags() {
    let markdown = "\u{feff}a | b\n--|--\n![[p.png]] #celltag | y";
    let rendered = render(markdown, &embeds_only());
    assert_eq!(rendered.embeds, ["p.png"]);
    assert_eq!(rendered.tags, ["celltag"]);
    assert!(
        rendered.html.contains("<img src=\"p.png\""),
        "{}",
        rendered.html
    );
    let e = assert_matches_render(markdown, &embeds_only(), "bom");
    assert_eq!(spans_of(markdown, &e).1, ["![[p.png]]"]);
}

#[test]
fn wikilinks_broken_across_lines_keep_their_spans() {
    for (markdown, expected) in [
        ("[[x\n]] [[B]]", vec!["[[x\n]]", "[[B]]"]),
        ("a [[x\ny]] [[B]]", vec!["[[x\ny]]", "[[B]]"]),
        ("> a [[x\n> y]] [[B]]", vec!["[[x\n> y]]", "[[B]]"]),
        ("[[x\r\n]] [[B]]", vec!["[[x\r\n]]", "[[B]]"]),
        (
            "p [[a\nb]] [[c]]\nnext [[d]] and [[e\nf]]",
            vec!["[[a\nb]]", "[[c]]", "[[d]]", "[[e\nf]]"],
        ),
    ] {
        let e = assert_matches_render(markdown, &all_on(None), markdown);
        assert_eq!(spans_of(markdown, &e).0, expected, "{markdown:?}");
    }
}

/// Every wiki link `render` reports has as many spans as it has occurrences,
/// across randomly built documents mixing line breaks, quotes, tables and escapes.
#[test]
fn every_wikilink_occurrence_gets_a_span_in_generated_documents() {
    let tokens = [
        "[[a]]",
        "[[b|B]]",
        "[[c\n]]",
        "[[d\ne]]",
        "\n",
        "\n\n",
        "> ",
        "x ",
        "`[[no]]` ",
        "\\[[esc]] ",
        "![[img.png]] ",
        "| ",
        "\\| ",
        "&#33;[[f]] ",
        "[[lamb&#32;soup]] ",
        "`[[lamb soup]]` ",
        "`x\\|y` ",
        "\r\n",
    ];
    let mut seed: u64 = 7;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..300 {
        let mut doc = String::new();
        if next() % 3 == 0 {
            doc.push_str("h | k\n--|--\n");
        }
        for _ in 0..(1 + next() % 12) {
            doc.push_str(tokens[next() % tokens.len()]);
        }
        let rendered = render(&doc, &all_on(None));
        let e = extract(&doc, &all_on(None));
        assert_eq!(e.wikilinks, rendered.wikilinks, "{doc:?}");
        assert_eq!(e.embeds, rendered.embeds, "{doc:?}");
        assert_eq!(e.tags, rendered.tags, "{doc:?}");
        for span in &e.wikilink_spans {
            let text = &doc[span.range.clone()];
            assert!(
                text.starts_with("[[") && text.ends_with("]]"),
                "{doc:?}: {text:?}"
            );
        }
        // A literal `[[…]]` in a code span is never a link's span
        for span in &e.wikilink_spans {
            assert!(
                !doc[..span.range.start].ends_with('`'),
                "{doc:?}: {:?}",
                e.wikilink_spans
            );
        }
        let anchors = rendered.html.matches("data-wikilink=\"true\"").count();
        assert_eq!(
            e.wikilink_spans.len(),
            anchors,
            "{doc:?}: {:?}",
            e.wikilink_spans
        );
    }
}

#[test]
fn spelled_links_beat_literal_lookalikes_in_code_spans() {
    for (markdown, expected) in [
        ("[[lamb&#32;soup]] and `[[lamb soup]]`", "[[lamb&#32;soup]]"),
        (
            "`[[lamb soup]]` then [[lamb&#32;soup]]",
            "[[lamb&#32;soup]]",
        ),
        ("[[a\\_b]] and `[[a_b]]`", "[[a\\_b]]"),
        ("a | b\n--|--\nx\\|y `[[c]]` [[c]] | z", "[[c]]"),
        ("``x ` [[d]]`` [[d]]", "[[d]]"),
    ] {
        let e = assert_matches_render(markdown, &all_on(None), markdown);
        let spans = spans_of(markdown, &e).0;
        assert_eq!(spans, [expected], "{markdown:?}");
        // The span is the link itself, never the code span's text
        let span = &e.wikilink_spans[0];
        assert!(
            markdown.rfind(expected) == Some(span.range.start)
                || markdown.find(expected) == Some(span.range.start)
        );
        assert!(!markdown[..span.range.start].ends_with('`'), "{markdown:?}");
    }
}

#[test]
fn many_links_in_one_note_all_get_spans() {
    // 4,000 links, many sharing targets, with code-span lookalikes between them
    let mut doc = String::new();
    for i in 0..4000 {
        doc.push_str(&format!("[[n{}]] ", i % 97));
        if i % 10 == 0 {
            doc.push_str(&format!("`[[n{}]]` ", i % 97));
        }
        if i % 25 == 0 {
            doc.push('\n');
        }
    }
    let e = extract(&doc, &all_on(None));
    assert_eq!(e.wikilink_spans.len(), 4000);
    assert_eq!(e.wikilinks.len(), 97);
    assert!(
        e.wikilink_spans
            .iter()
            .all(|s| !doc[..s.range.start].ends_with('`'))
    );
}
