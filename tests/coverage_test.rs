//! Every span birta-core 0.3.4's `extract` placed correctly must still be
//! placed. `tests/fixtures/coverage-0.3.4.txt` holds those spans; regenerate it
//! with 0.3.4's render.rs and obsidian.rs and `BIRTA_RECORD_COVERAGE=1`.

use birta_core::render::{RenderOptions, extract, render};

fn opts() -> RenderOptions<'static> {
    RenderOptions {
        wikilinks: true,
        embeds: true,
        tags: true,
        front_matter: true,
        ..Default::default()
    }
}

const CASES: &[&str] = &[
    "see [[Note&#32;]] and [[E]]",
    "> [[a\n> b]] [[Note&#32;]]",
    "see [[Note&#32;]] and [[E]]\r\n",
    "> [[a\r\n> b]] [[Note&#32;]]\r\n",
    "[[Note]][[ \n\tNote]][[  [[ \n\tNote]] [[E]] [[F]]",
    "[[Note]][[ \r\n\tNote]][[  [[ \r\n\tNote]] [[E]] [[F]]",
];

const TOKENS: &[&str] = &[
    "[[a]]",
    "[[ g]]",
    "[[&#65;]]",
    "[[A]]",
    "[[h\\|i]]",
    "\\[[j]]",
    "![[p.png]]",
    "`![[c]]`",
    "`[[a]]`",
    "#tag",
    "\n",
    "\n> ",
    "> ",
    " ",
    "x",
    "[[a&amp;b]]",
    "é",
    "``",
    "&#33;[[q.png]]",
    "it's",
    "| ",
    "--|--\n",
    "[![[p.png]]](u)",
    "[^1]",
    "\n\n[^1]: ![[a.png]] #fn [[F]]\n",
    "[[Note&#32;]]",
    "[[ \n\tNote]]",
    "[[  ",
    "[[b\n]]",
    "[[e\n>f]]",
    "<!-- [[C]] -->",
    "\n\n\t   `x\n",
    "日本",
];

fn documents() -> Vec<String> {
    let mut seed: u64 = 2718;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let mut docs: Vec<String> = CASES.iter().map(|c| c.to_string()).collect();
    for _ in 0..400 {
        let mut doc = String::new();
        for _ in 0..(2 + next() % 12) {
            doc.push_str(TOKENS[next() % TOKENS.len()]);
        }
        if next() % 3 == 0 {
            doc = doc.replace('\n', "\r\n");
        }
        docs.push(doc);
    }
    docs
}

fn wikilink_hrefs(markdown: &str) -> Vec<String> {
    let html = render(markdown, &opts()).html;
    html.split("<a ")
        .skip(1)
        .filter_map(|tag| {
            let tag = &tag[..tag.find('>')?];
            tag.contains("data-wikilink=\"true\"").then_some(())?;
            let href = &tag[tag.find("href=\"")? + 6..];
            Some(href[..href.find('"')?].to_string())
        })
        .collect()
}

/// Spans that provably point at their own link or embed: replacing one with a
/// fresh link removes exactly that target's href, or with a fresh embed adds it.
fn verified_spans(doc: &str) -> Vec<String> {
    let e = extract(doc, &opts());
    let before = wikilink_hrefs(doc);
    let mut out = Vec::new();
    for span in &e.wikilink_spans {
        let slice = &doc[span.range.clone()];
        if slice.contains('`') || (doc.contains("--|") && slice.replace("\\|", "").contains('|')) {
            continue;
        }
        let replaced = format!(
            "{}[[ZZQ]]{}",
            &doc[..span.range.start],
            &doc[span.range.end..]
        );
        let mut after = wikilink_hrefs(&replaced);
        let Some(at) = after.iter().position(|h| h == "ZZQ") else {
            continue;
        };
        after.remove(at);
        let mut removed = before.clone();
        for href in &after {
            if let Some(i) = removed.iter().position(|h| h == href) {
                removed.remove(i);
            }
        }
        // Spaces spelled as entities survive comrak's trimming, as in the original
        let rebuilt = span.target.replace('|', "\\|").replace(' ', "&#32;");
        if removed == wikilink_hrefs(&format!("[[{rebuilt}]]")) {
            out.push(format!(
                "link {:?} {}..{}",
                span.target, span.range.start, span.range.end
            ));
        }
    }
    for span in &e.embed_spans {
        let replaced = format!(
            "{}![[ZZQ.png]]{}",
            &doc[..span.range.start],
            &doc[span.range.end..]
        );
        if render(&replaced, &opts())
            .embeds
            .iter()
            .any(|t| t == "ZZQ.png")
        {
            out.push(format!(
                "embed {:?} {}..{}",
                span.target, span.range.start, span.range.end
            ));
        }
    }
    out
}

const FIXTURE: &str = "tests/fixtures/coverage-0.3.4.txt";

#[test]
fn every_span_0_3_4_placed_correctly_is_still_placed() {
    let docs = documents();
    if std::env::var("BIRTA_RECORD_COVERAGE").is_ok() {
        let mut out = String::new();
        for (i, doc) in docs.iter().enumerate() {
            for line in verified_spans(doc) {
                out.push_str(&format!("{i} {line}\n"));
            }
        }
        std::fs::write(FIXTURE, out).unwrap();
        return;
    }
    let recorded = std::fs::read_to_string(FIXTURE).unwrap();
    assert!(recorded.lines().count() > 300, "fixture looks empty");
    let mut current: Vec<Vec<String>> = vec![Vec::new(); docs.len()];
    for (i, doc) in docs.iter().enumerate() {
        let e = extract(doc, &opts());
        current[i].extend(
            e.wikilink_spans
                .iter()
                .map(|s| format!("link {:?} {}..{}", s.target, s.range.start, s.range.end)),
        );
        current[i].extend(
            e.embed_spans
                .iter()
                .map(|s| format!("embed {:?} {}..{}", s.target, s.range.start, s.range.end)),
        );
    }
    let mut missing = Vec::new();
    for line in recorded.lines() {
        let (index, span) = line.split_once(' ').unwrap();
        let index: usize = index.parse().unwrap();
        if !current[index].iter().any(|s| s == span) {
            missing.push(format!("{:?}: {span}", docs[index]));
        }
    }
    assert!(
        missing.is_empty(),
        "{} spans lost:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
