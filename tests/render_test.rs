use std::fs;

#[test]
fn snapshot_all_fixtures() {
    insta::glob!("fixtures/*.md", |path| {
        let markdown = fs::read_to_string(path).unwrap();
        let html = birta::render::render(&markdown, None);
        insta::assert_snapshot!(html);
    });
}

/// Opening `<pre` tags in `html`, each up to its `>`.
fn pre_tags(html: &str) -> Vec<&str> {
    html.match_indices("<pre")
        .map(|(i, _)| &html[i..i + html[i..].find('>').unwrap() + 1])
        .filter(|tag| tag[4..].starts_with([' ', '>']))
        .collect()
}

#[test]
fn every_code_pre_carries_one_sourcepos_in_every_mode() {
    let catppuccin = birta::theme::resolve_by_name("catppuccin").unwrap();
    let themed = catppuccin.active_data().syntax.clone();
    assert!(themed.is_some(), "catppuccin ships a tmTheme");
    let mut bad = Vec::new();
    let mut seen = 0;
    insta::glob!("fixtures/*.md", |path| {
        let md = fs::read_to_string(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for syntax in [None, themed.as_ref()] {
            let renders = [
                ("render", birta::render::render(&md, syntax)),
                (
                    "render_dir",
                    birta::render::render_dir(&md, syntax, std::path::Path::new("sub/doc.md")),
                ),
                ("render_bundle", birta::render::render_bundle(&md, syntax).0),
            ];
            for (how, html) in renders {
                for tag in pre_tags(&html) {
                    seen += 1;
                    if tag.matches(" data-sourcepos=\"").count() != 1 {
                        bad.push(format!("{name} {how} themed={}: {tag}", syntax.is_some()));
                    }
                }
            }
        }
    });
    assert!(seen > 0, "the fixtures have code blocks");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_highlighted_pre_tag_is_class_then_sourcepos() {
    let html = birta::render::render("# Code\n\n```rust\nfn x() {}\n```\n", None);
    assert!(
        html.contains("<pre class=\"syntax-highlighting\" data-sourcepos=\"3:1-5:3\"><code>"),
        "{html}"
    );

    let html = birta::render::render("    indented\n    code\n", None);
    assert!(
        html.contains("<pre class=\"syntax-highlighting\" data-sourcepos=\"1:5-2:8\"><code>"),
        "{html}"
    );
}

#[test]
fn a_themed_pre_keeps_its_background_and_gains_nothing_twice() {
    let catppuccin = birta::theme::resolve_by_name("catppuccin").unwrap();
    let syntax = catppuccin.active_data().syntax.clone();
    let html = birta::render::render("```rust\nfn x() {}\n```\n", syntax.as_ref());
    let tags = pre_tags(&html);
    assert_eq!(tags.len(), 1, "{html}");
    assert!(tags[0].contains(" style=\"background-color:#"), "{html}");
    assert_eq!(
        tags[0].matches(" data-sourcepos=\"1:1-3:3\"").count(),
        1,
        "{html}"
    );
}

#[test]
fn a_mermaid_pre_carries_its_sourcepos() {
    let html = birta::render::render("Intro\n\n```mermaid\ngraph TD\n  A --> B\n```\n", None);
    assert!(
        html.contains(
            "<pre class=\"mermaid\" data-sourcepos=\"3:1-6:3\">graph TD\n  A --&gt; B\n</pre>"
        ),
        "{html}"
    );
}
