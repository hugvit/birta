//! Times `render` against `extract` on 984 synthetic notes:
//! `cargo run --release -p birta-core --example extract_bench`

use std::time::{Duration, Instant};

use birta_core::render::{RenderOptions, extract, render};

const NOTES: usize = 984;

fn note(i: usize) -> String {
    let mut s = format!(
        "---\ntitle: Note {i}\ntags: [area-{}]\n---\n# Note {i}\n\n",
        i % 7
    );
    for p in 0..(5 + i % 10) {
        s.push_str(&format!(
            "Paragraph {p} links [[Note {}]] and [[Topic {}|topic]], tagged #project/{} and #status-{}. \
             It mentions `inline code` and **bold** text with a [web link](https://example.com/{p}).\n\n",
            (i * 7 + p) % NOTES,
            p % 13,
            i % 5,
            p % 3
        ));
    }
    if i.is_multiple_of(2) {
        s.push_str(&format!(
            "![[diagram-{}.png|400]]\n\n![[Note {}]]\n\n",
            i % 11,
            (i + 1) % NOTES
        ));
    }
    if i.is_multiple_of(3) {
        s.push_str("```rust\nfn main() {\n    let total: u32 = (1..=10).map(|n| n * n).sum();\n    println!(\"{total}\");\n}\n```\n\n");
    }
    if i.is_multiple_of(5) {
        s.push_str("| Task | Owner |\n|---|---|\n| Draft | [[Ana]] |\n| Review | #team |\n\n");
    }
    s
}

fn best_of(runs: usize, notes: &[String], f: impl Fn(&str) -> usize) -> (Duration, usize) {
    let mut best = Duration::MAX;
    let mut found = 0;
    for _ in 0..runs {
        let start = Instant::now();
        found = notes.iter().map(|n| f(n)).sum();
        best = best.min(start.elapsed());
    }
    (best, found)
}

fn main() {
    let notes: Vec<String> = (0..NOTES).map(note).collect();
    let bytes: usize = notes.iter().map(String::len).sum();
    let opts = RenderOptions {
        wikilinks: true,
        embeds: true,
        tags: true,
        front_matter: true,
        ..RenderOptions::default()
    };
    let (rendered, a) = best_of(5, &notes, |n| {
        let r = render(n, &opts);
        r.wikilinks.len() + r.embeds.len() + r.tags.len()
    });
    let (extracted, b) = best_of(5, &notes, |n| {
        let e = extract(n, &opts);
        e.wikilinks.len() + e.embeds.len() + e.tags.len()
    });
    assert_eq!(a, b, "render and extract found different totals");
    println!("{NOTES} notes, {} KiB, {a} links/embeds/tags", bytes / 1024);
    println!("render:  {rendered:?}");
    println!(
        "extract: {extracted:?} ({:.1}x faster)",
        rendered.as_secs_f64() / extracted.as_secs_f64()
    );

    // One link-heavy note: span lookup must stay linear in its link count
    for links in [1_000, 4_000, 16_000] {
        let note = many_links(links);
        let (time, found) = best_of(3, std::slice::from_ref(&note), |n| {
            extract(n, &opts).wikilink_spans.len()
        });
        assert_eq!(found, links);
        println!("one note, {links:>6} links: extract {time:?}");
    }
}

fn many_links(links: usize) -> String {
    let mut s = String::new();
    for i in 0..links {
        s.push_str(&format!("[[Note {}]] ", i % 97));
        if i % 10 == 0 {
            s.push_str("`[[code lookalike]]` ");
        }
        if i % 25 == 24 {
            s.push('\n');
        }
    }
    s
}
