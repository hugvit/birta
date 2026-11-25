//! `render` on documents without line-broken wiki links must match birta-core
//! 0.3.4 exactly. The snapshot was recorded with that release's render.

use birta_core::render::{RenderOptions, render};

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
    "\\![[e.png]]",
    "#t/x",
    "[^1]",
    "[^2]",
    "日本",
    "\n\n[^1]: ![[a.png]] #fn [[F]]\n",
    "\n\n[^2]: `![[c]]` ![[b.png]]\n",
    "\n[^1]:![[Note]]#x",
    "\n```md\n![[Note]] #no\n```\n",
    "\n2.    [[d]] #li ![[n.png]]",
    "*#em*",
    "==#hl==",
];

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[test]
fn render_matches_the_last_release_on_normal_documents() {
    let opts = RenderOptions {
        wikilinks: true,
        embeds: true,
        tags: true,
        front_matter: true,
        ..Default::default()
    };
    let mut seed: u64 = 314;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let mut summary = String::new();
    for i in 0..400 {
        let mut doc = String::new();
        if next() % 10 == 0 {
            doc.push('\u{feff}');
        }
        for _ in 0..(2 + next() % 14) {
            doc.push_str(TOKENS[next() % TOKENS.len()]);
        }
        let r = render(&doc, &opts);
        summary.push_str(&format!(
            "{i} {:?} {:?} {:?} {:016x}\n",
            r.embeds,
            r.tags,
            r.wikilinks,
            fnv(&r.html)
        ));
    }
    insta::assert_snapshot!(summary);
}
