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
