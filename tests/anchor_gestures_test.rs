//! Browser-derived oracles for the resolver. `e2e/scripts/gen-gestures.mjs` records real
//! selections in Chromium and WebKit, and these tests feed them through `anchor::resolve`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use birta::anchor::{self, Owner, OwnerKind, ResolveReq, Resolved};
use comrak::{Arena, parse_document};
use serde_json::Value;

const ENGINES: [&str; 2] = ["chromium", "webkit"];

enum Want {
    /// `Exact` on `source`, found at its place inside the unique `context`.
    Exact {
        source: &'static str,
        context: &'static str,
        widened: bool,
    },
    /// Anything but `Exact`: the page showed text the server can't see.
    NotExact,
    /// A selection with no owner: a whole-doc search finds this raw source once.
    Search { source: &'static str },
}

struct Expect {
    id: &'static str,
    sp: Option<&'static str>,
    want: Want,
}

const LINE_3: &str = "This PR **adds retry** with *jittered* backoff to the `upload()` client -- because S3 returns 503 &amp; drops events... See [the docs](https://example.com/retry) :tada:. He said \"we just fail\" and don't \\*panic\\*.";

#[rustfmt::skip]
fn expectations() -> Vec<Expect> {
    use Want::*;
    let exact = |source, context, widened| Exact {
        source,
        context,
        widened,
    };
    let pr = Some("3:1-3:213");
    let retry = Some("5:1-7:70");
    vec![
        Expect { id: "01-dblclick-emphasis", sp: pr, want: exact("*jittered*", "with *jittered* backoff", true) },
        Expect { id: "02-triple-click-paragraph", sp: pr, want: exact(LINE_3, LINE_3, true) },
        Expect { id: "03-triple-click-heading", sp: Some("1:1-1:26"), want: exact("Retry \"layer\" — design", "# Retry \"layer\" — design", false) },
        Expect { id: "04-triple-click-item-with-nested-list", sp: Some("10:1-11:22"), want: exact("Checked idempotency on every POST", "Checked idempotency on every POST", false) },
        Expect { id: "05-triple-click-cell", sp: Some("16:2-16:10"), want: exact("retries", "| retries |", false) },
        Expect { id: "06-triple-click-task-item", sp: Some("9:1-9:31"), want: exact("Ran the integration suite", "Ran the integration suite", false) },
        Expect {
            id: "07-drag-strong-to-link-end",
            sp: pr,
            want: exact(
                "**adds retry** with *jittered* backoff to the `upload()` client -- because S3 returns 503 &amp; drops events... See [the docs](https://example.com/retry)",
                "**adds retry** with *jittered* backoff to the `upload()` client -- because S3 returns 503 &amp; drops events... See [the docs](https://example.com/retry)",
                true,
            ),
        },
        Expect {
            id: "08-drag-into-next-paragraph",
            sp: pr,
            want: exact(&LINE_3[8..], LINE_3, true),
        },
        Expect { id: "09-drag-across-inline-math", sp: retry, want: exact("three <kbd>Ctrl</kbd> and math $e^{i\\pi}+1=0$ end", "three <kbd>Ctrl</kbd> and math $e^{i\\pi}+1=0$ end", true) },
        Expect { id: "10-drag-across-footnote-ref", sp: Some("99:1-99:39"), want: exact("Second ref[^2] with", "Second ref[^2] with", true) },
        Expect { id: "11-dblclick-after-emoji", sp: pr, want: exact("panic", "\\*panic\\*", false) },
        Expect { id: "12-drag-to-emoji-at-end", sp: Some("74:5-74:35"), want: exact("café and 🎉", "inner quote with café and 🎉", false) },
        Expect { id: "13-dblclick-repeated-word", sp: retry, want: exact("just", "we just fail again", false) },
        Expect { id: "14-dblclick-in-code", sp: Some("43:1-47:3"), want: exact("retry", "fn retry() {", false) },
        Expect { id: "15-drag-in-raw-html", sp: None, want: Search { source: "More" } },
        Expect { id: "16-triple-click-hidden-span-paragraph", sp: Some("89:1-89:46"), want: NotExact },
        Expect { id: "17-dblclick-nested-item", sp: Some("11:3-11:22"), want: exact("nested", "nested item -- one", false) },
        Expect { id: "18-dblclick-after-graphemes", sp: Some("125:1-125:34"), want: exact("ok", "\u{1F1EE}\u{1F1F8} ok", false) },
        Expect { id: "19-range-inside-footnote-ref", sp: retry, want: exact("[^1]", "end[^1].", true) },
        Expect { id: "20-range-from-inside-math", sp: retry, want: exact("$e^{i\\pi}+1=0$ end", "math $e^{i\\pi}+1=0$ end", true) },
    ]
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run `node e2e/scripts/gen-gestures.mjs`",
            path.display()
        )
    })
}

fn doc() -> String {
    read(&fixtures_dir().join("anchor.md"))
}

fn gestures() -> Value {
    serde_json::from_str(&read(&fixtures_dir().join("anchor_gestures.json"))).unwrap()
}

/// Owners whose page text the server can't reproduce, as `(fixture, sp)`.
const P1_EXCLUDED: &[(&str, &str)] = &[
    // Text inside `<span hidden>`: visibility is CSS, which the server never sees
    ("anchor.md", "89:1-89:46"),
];

/// Every `tests/fixtures/*.md` as `(name, text)`.
fn fixtures() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = fs::read_dir(fixtures_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, read(&p))
        })
        .collect();
    out.sort();
    out
}

/// `(sp, T)` per line of `sp | "T"`, with `T` as a JSON string.
fn golden(fixture: &str) -> Vec<(String, String)> {
    read(&fixtures_dir().join(format!("anchor_goldens/{fixture}.txt")))
        .lines()
        .map(|line| {
            let (sp, t) = line
                .split_once(" | ")
                .unwrap_or_else(|| panic!("bad golden line {line:?}"));
            (sp.to_string(), serde_json::from_str(t).unwrap())
        })
        .collect()
}

/// Mermaid and math fences are drawn in the browser and anchor at block level only.
fn is_diagram(md: &str, o: &Owner) -> bool {
    if o.kind != OwnerKind::Code {
        return false;
    }
    let line = md.lines().nth(o.lines.0 as usize - 1).unwrap_or("");
    let info = line.trim_start().trim_start_matches(['`', '~']).trim();
    info.starts_with("mermaid") || info.starts_with("math")
}

fn owners(md: &str) -> Vec<Owner> {
    let arena = Arena::new();
    let root = parse_document(&arena, md, &anchor::parse_options());
    anchor::index(root, md).owners
}

/// Byte range of `needle` inside the one occurrence of `context` in `text`.
fn span(text: &str, context: &str, needle: &str) -> std::ops::Range<usize> {
    let at = text.find(context).expect("context in doc");
    assert_eq!(
        text.matches(context).count(),
        1,
        "context {context:?} is not unique"
    );
    let inner = context.find(needle).expect("needle in context");
    at + inner..at + inner + needle.len()
}

struct Recorded {
    sp: Option<String>,
    quote: String,
    pre: String,
    suf: String,
    off: Option<usize>,
    tl: Option<usize>,
}

fn recorded(engine: &str, id: &str) -> Recorded {
    let all = gestures();
    let g = &all["engines"][engine][id];
    assert!(g.is_object(), "{engine} has no recording for {id}");
    let text = |k: &str| g[k].as_str().unwrap_or_default().to_string();
    let num = |k: &str| g[k].as_u64().map(|n| n as usize);
    Recorded {
        sp: g["sp"].as_str().map(str::to_string),
        quote: text("quote"),
        pre: text("pre"),
        suf: text("suf"),
        off: num("off"),
        tl: num("tl"),
    }
}

fn request(r: &Recorded, with_offset: bool) -> ResolveReq {
    ResolveReq {
        sp: r.sp.clone().unwrap_or_default(),
        quote: r.quote.clone(),
        pre: r.pre.clone(),
        suf: r.suf.clone(),
        off: r.off.filter(|_| with_offset),
        tl: r.tl.filter(|_| with_offset),
    }
}

fn check(md: &str, e: &Expect, r: &Recorded, with_offset: bool) -> Result<(), String> {
    let got = anchor::resolve(md, &request(r, with_offset));
    match (&e.want, got) {
        (
            Want::Exact {
                source,
                context,
                widened,
            },
            Resolved::Exact {
                bytes,
                source: s,
                shown,
                widened: w,
                ..
            },
        ) => {
            let want = span(md, context, source);
            if bytes != want || s != *source || w != *widened || shown != r.quote {
                return Err(format!(
                    "Exact {bytes:?} {s:?} widened {w} shown {shown:?}, want {want:?} {source:?} widened {widened}"
                ));
            }
            Ok(())
        }
        (Want::NotExact, Resolved::Candidates { .. }) => Ok(()),
        (Want::NotExact, Resolved::Block { kept_quote, .. }) if kept_quote == r.quote => Ok(()),
        (_, other) => Err(format!("got {other:?}")),
    }
}

#[test]
fn the_e2e_fixture_is_the_rust_fixture() {
    let e2e = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e/fixtures/anchor.md");
    assert!(
        read(&e2e) == doc(),
        "e2e/fixtures/anchor.md drifted from tests/fixtures/anchor.md"
    );
}

#[test]
fn every_gesture_ran_in_both_engines() {
    let all = gestures();
    assert_eq!(all["fixture"], "anchor.md");
    let engines: BTreeSet<&str> = all["engines"]
        .as_object()
        .expect("engines")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(engines, BTreeSet::from(ENGINES));
    let want: BTreeSet<&str> = expectations().iter().map(|e| e.id).collect();
    for engine in ENGINES {
        let ids: BTreeSet<&str> = all["engines"][engine]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(ids, want, "{engine}");
    }
}

#[test]
fn gestures_start_in_the_expected_owner() {
    let mut bad = Vec::new();
    for engine in ENGINES {
        for e in expectations() {
            let r = recorded(engine, e.id);
            if r.sp.as_deref() != e.sp {
                bad.push(format!("{engine} {}: sp {:?}, want {:?}", e.id, r.sp, e.sp));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn gesture_offsets_index_the_server_text() {
    let md = doc();
    let mut bad = Vec::new();
    for engine in ENGINES {
        // The page's text differs from the server's for an excluded owner, by design
        let comparable = |e: &&Expect| {
            e.sp.is_some_and(|sp| !P1_EXCLUDED.contains(&("anchor.md", sp)))
        };
        for e in expectations().iter().filter(comparable) {
            let r = recorded(engine, e.id);
            let Some(t) = anchor::owner_text(&md, r.sp.as_deref().unwrap_or_default()) else {
                bad.push(format!("{engine} {}: {:?} is not an owner", e.id, r.sp));
                continue;
            };
            let t: Vec<char> = t.chars().collect();
            let len = r.quote.chars().count();
            let (Some(off), Some(tl)) = (r.off, r.tl) else {
                bad.push(format!("{engine} {}: no off/tl", e.id));
                continue;
            };
            let slice = |a: usize, b: usize| -> String {
                t[a.min(t.len())..b.min(t.len())].iter().collect()
            };
            let got = (tl, slice(off, off + len), r.pre.clone(), r.suf.clone());
            let want = (
                t.len(),
                r.quote.clone(),
                slice(off.saturating_sub(32), off),
                slice(off + len, off + len + 32),
            );
            if got != want {
                bad.push(format!(
                    "{engine} {}: (tl, T[off..], pre, suf) {got:?}, want {want:?}",
                    e.id
                ));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn gestures_resolve_to_their_expected_source() {
    let md = doc();
    let mut bad = Vec::new();
    for engine in ENGINES {
        for e in expectations().iter().filter(|e| e.sp.is_some()) {
            let r = recorded(engine, e.id);
            for with_offset in [true, false] {
                if let Err(msg) = check(&md, e, &r, with_offset) {
                    bad.push(format!("{engine} {} (offset {with_offset}): {msg}", e.id));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_raw_html_selection_is_found_by_searching_the_doc() {
    let md = doc();
    for engine in ENGINES {
        for e in expectations() {
            let Want::Search { source } = e.want else {
                continue;
            };
            let r = recorded(engine, e.id);
            let hits = anchor::search(&md, &r.quote);
            let got: Vec<(Option<String>, String)> =
                hits.into_iter().map(|c| (c.sp, c.source)).collect();
            assert_eq!(got, vec![(None, source.to_string())], "{engine} {}", e.id);
        }
    }
}

#[test]
fn goldens_match_the_server_owner_text() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        for (sp, t) in golden(&name) {
            if P1_EXCLUDED.contains(&(name.as_str(), sp.as_str())) {
                continue;
            }
            let server = anchor::owner_text(&md, &sp);
            if server.as_deref() != Some(t.as_str()) {
                bad.push(format!(
                    "{name} {sp}\n  page:   {t:?}\n  server: {server:?}"
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatch(es):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_parity_exclusions_still_differ() {
    for &(name, sp) in P1_EXCLUDED {
        let md = read(&fixtures_dir().join(name));
        let page = golden(name)
            .into_iter()
            .find(|(s, _)| s == sp)
            .map(|(_, t)| t);
        let server = anchor::owner_text(&md, sp);
        assert!(
            page.is_some() && server.is_some(),
            "{name} {sp} left the page or the server"
        );
        assert_ne!(page, server, "{name} {sp} matches now; drop the exclusion");
    }
}

#[test]
fn goldens_cover_exactly_the_server_owners() {
    let mut bad = Vec::new();
    for (name, md) in fixtures() {
        let server: BTreeSet<String> = owners(&md)
            .into_iter()
            .filter(|o| !is_diagram(&md, o))
            .map(|o| o.sp)
            .collect();
        let lines = golden(&name);
        let page: BTreeSet<String> = lines.iter().map(|(sp, _)| sp.clone()).collect();
        if page.len() != lines.len() {
            bad.push(format!("{name}: an sp appears twice in the golden"));
        }
        let missing: Vec<_> = server.difference(&page).collect();
        let extra: Vec<_> = page.difference(&server).collect();
        if !missing.is_empty() || !extra.is_empty() {
            bad.push(format!(
                "{name}: owners the page lacks {missing:?}, sps that are not owners {extra:?}"
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_quote_across_hidden_text_never_resolves_to_a_wrong_exact() {
    let md = doc();
    let sp = "89:1-89:46";
    // The page's text, which skips the hidden span
    let page: Vec<char> = "Hidden inline text.".chars().collect();
    for (off, len) in [(0, 13), (0, 19), (5, 3), (4, 8)] {
        let quote: String = page[off..off + len].iter().collect();
        let pre: String = page[off.saturating_sub(32)..off].iter().collect();
        let suf: String = page[off + len..(off + len + 32).min(page.len())]
            .iter()
            .collect();
        for (o, t) in [(Some(off), Some(page.len())), (None, None)] {
            let req = ResolveReq {
                sp: sp.to_string(),
                quote: quote.clone(),
                pre: pre.clone(),
                suf: suf.clone(),
                off: o,
                tl: t,
            };
            let got = anchor::resolve(&md, &req);
            assert!(
                matches!(got, Resolved::Candidates { .. } | Resolved::Block { .. }),
                "{quote:?} (off {o:?}): {got:?}"
            );
        }
    }
}
