use birta::anchor;
use birta::rev::Rev;
use birta::review::chain::Chain;
use birta::review::doc::Doc;
use birta::review::ladder;
use birta::review::ops::{self, Anchor, AnchorReq, At, BlockScope, Scope};
use birta::review::state::AnchorStatus;
use birta::review::{Reject, anchor_from_req};
use serde_json::json;

const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;

const DOC: &str = "\
# Retry policy

The client **adds retry** with *jittered* backoff -- because S3 drops events.

- first item
- second item
  - nested item

| key | value |
|-----|-------|
| mode | fast |

> quoted words here

- loose one

- loose two

## Stacked on

Plain closing paragraph.
";

/// A definition list, a code block, a diagram, and a heading inside a blockquote.
const MIXED: &str = "\
Term
: Detail text

```rust
let x = 1;
```

```mermaid
graph TD
```

> ## Inner

We said hello.
";

const SP_PARA: &str = "3:1-3:77";

/// Doc, owner sp, the selection, the quote it stores, and that quote's lines.
type Case<'a> = (&'a str, &'a str, &'a str, &'a str, (u32, u32));

fn rev(text: &str) -> Rev {
    Rev::of(text.as_bytes())
}

fn make(md: &str, req: &AnchorReq) -> Result<At, Reject> {
    let doc = Doc::new(md);
    anchor_from_req(req, &doc, &doc.rev)
}

fn anchored(md: &str, req: &AnchorReq) -> Anchor {
    match make(md, req) {
        Ok(At::Anchor(anchor)) => anchor,
        other => panic!("{req:?} should anchor, got {other:?}"),
    }
}

/// A selection in owner `sp` at code-point offset `off`, with the owner's real text length.
fn selection(md: &str, sp: &str, q: &str, off: usize) -> AnchorReq {
    let t: Vec<char> = anchor::owner_text(md, sp)
        .unwrap_or_else(|| panic!("{sp} is not an owner"))
        .chars()
        .collect();
    let len = q.chars().count();
    let at = |r: std::ops::Range<usize>| t[r].iter().collect::<String>();
    AnchorReq::Quote {
        sp: Some(sp.to_string()),
        q: q.to_string(),
        pre: at(off.saturating_sub(32)..off),
        suf: at(off + len..(off + len + 32).min(t.len())),
        off: Some(off),
        tl: Some(t.len()),
    }
}

/// The first occurrence of `q` in the owner's text.
fn first(md: &str, sp: &str, q: &str) -> AnchorReq {
    let t = anchor::owner_text(md, sp).unwrap();
    let byte = t.find(q).unwrap_or_else(|| panic!("{q:?} not in {t:?}"));
    selection(md, sp, q, t[..byte].chars().count())
}

/// A selection the page could not place: no offset and no context.
fn bare(sp: &str, q: &str) -> AnchorReq {
    AnchorReq::Quote {
        sp: Some(sp.to_string()),
        q: q.to_string(),
        pre: String::new(),
        suf: String::new(),
        off: None,
        tl: None,
    }
}

/// A selection inside raw HTML, which has no owner.
fn in_html(q: &str) -> AnchorReq {
    AnchorReq::Quote {
        sp: None,
        q: q.to_string(),
        pre: String::new(),
        suf: String::new(),
        off: None,
        tl: None,
    }
}

fn block(sp: &str) -> AnchorReq {
    AnchorReq::Block {
        sp: sp.to_string(),
        scope: BlockScope::Block,
    }
}

fn section(sp: &str) -> AnchorReq {
    AnchorReq::Block {
        sp: sp.to_string(),
        scope: BlockScope::Section,
    }
}

fn quote_anchor(
    md: &str,
    quote: &str,
    ctx: (&str, &str),
    lines: (u32, u32),
    shown: &str,
) -> Anchor {
    Anchor {
        quote: quote.to_string(),
        pre: ctx.0.to_string(),
        suf: ctx.1.to_string(),
        lines,
        rev: rev(md),
        scope: Scope::Quote,
        shown: Some(shown.to_string()),
    }
}

fn block_anchor(md: &str, quote: &str, lines: (u32, u32), shown: Option<&str>) -> Anchor {
    Anchor {
        quote: quote.to_string(),
        pre: String::new(),
        suf: String::new(),
        lines,
        rev: rev(md),
        scope: Scope::Block,
        shown: shown.map(str::to_string),
    }
}

/// A log holding only the baseline, so the doc is the chain's head.
fn baseline(md: &str) -> Chain {
    let snap = json!({"by": {"type": "birta"}, "op": "snap", "rev": rev(md), "text": md});
    Chain::build(&ops::parse(format!("{HEADER}\n{snap}\n")))
}

#[test]
fn a_quote_anchor_keeps_the_source_and_shows_the_selection() {
    // `adds` sits inside `**adds retry**`, so the stored quote is the whole emphasis
    let got = make(DOC, &selection(DOC, SP_PARA, "adds", 11)).unwrap();
    let want = quote_anchor(
        DOC,
        "**adds retry**",
        (
            "# Retry policy The client ",
            " with *jittered* backoff -- beca",
        ),
        (3, 3),
        "adds",
    );
    assert_eq!(got, At::Anchor(want));
}

#[test]
fn a_quote_in_any_owner_stores_its_source_text_and_lines() {
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (DOC, "1:1-1:14", "Retry policy", "Retry policy", (1, 1)),
        (DOC, SP_PARA, "backoff – because", "backoff -- because", (3, 3)),
        (DOC, SP_PARA, "S3 drops", "S3 drops", (3, 3)),
        (DOC, "6:1-7:15", "second", "second", (6, 6)),
        (DOC, "7:3-7:15", "nested", "nested", (7, 7)),
        (DOC, "11:9-11:14", "fast", "fast", (11, 11)),
        (DOC, "13:3-13:19", "quoted words", "quoted words", (13, 13)),
        (DOC, "17:3-17:11", "loose two", "loose two", (17, 17)),
        (DOC, "19:1-19:13", "Stacked", "Stacked", (19, 19)),
        (MIXED, "2:1-2:13", "Detail", "Detail", (2, 2)),
        (MIXED, "4:1-6:3", "x = 1", "x = 1", (5, 5)),
        (MIXED, "12:3-12:10", "Inner", "Inner", (12, 12)),
    ];
    for &(md, sp, q, source, lines) in cases {
        let got = anchored(md, &first(md, sp, q));
        assert_eq!(
            (
                got.quote.as_str(),
                got.lines,
                got.scope,
                got.shown.as_deref()
            ),
            (source, lines, Scope::Quote, Some(q)),
            "{sp} {q:?}"
        );
        assert_eq!(got.rev, rev(md), "{sp} {q:?}");
    }
}

#[test]
fn the_offset_picks_the_occurrence_and_the_context_comes_from_its_source() {
    let md = "retry and retry now\n";
    let got = anchored(md, &selection(md, "1:1-1:19", "retry", 10));
    assert_eq!(
        got,
        quote_anchor(md, "retry", ("retry and ", " now "), (1, 1), "retry")
    );
}

#[test]
fn crlf_and_bom_docs_anchor_in_their_own_bytes() {
    let crlf = "line one\r\nline two\r\n";
    let got = anchored(crlf, &selection(crlf, "1:1-2:8", "one line", 5));
    assert_eq!((got.quote.as_str(), got.lines), ("one\r\nline", (1, 2)));
    assert_eq!(got.shown.as_deref(), Some("one line"));

    let bom = "\u{feff}# Title\n\nBody text here.\n";
    let got = anchored(bom, &selection(bom, "1:4-1:10", "Title", 0));
    assert_eq!((got.quote.as_str(), got.lines), ("Title", (1, 1)));
    let at = ladder::relocate(&got, &Doc::new(bom), &baseline(bom));
    assert_eq!(at.bytes, Some(5..10), "bytes count the BOM");
}

#[test]
fn a_block_anchor_keeps_the_first_60_chars_of_its_stop() {
    let long = "é".repeat(70) + "\n";
    let sixty = "é".repeat(60);
    let bom = "\u{feff}# Title\n\nBody text here.\n";
    #[rustfmt::skip]
    let cases: &[(&str, &str, &str, (u32, u32))] = &[
        (DOC, SP_PARA, "The client **adds retry** with *jittered* backoff -- because", (3, 3)),
        (DOC, "6:1-7:15", "- second item\n  - nested item", (6, 7)),
        (DOC, "7:3-7:15", "- nested item", (7, 7)),
        (DOC, "11:1-11:15", "| mode | fast |", (11, 11)),
        (DOC, "17:1-18:0", "- loose two", (17, 17)),
        (DOC, "19:1-19:13", "## Stacked on", (19, 19)),
        (MIXED, "4:1-6:3", "```rust\nlet x = 1;\n```", (4, 6)),
        (MIXED, "8:1-10:3", "```mermaid\ngraph TD\n```", (8, 10)),
        (bom, "1:4-1:10", "# Title", (1, 1)),
        (&long, "1:1-1:140", &sixty, (1, 1)),
    ];
    for &(md, sp, quote, lines) in cases {
        assert_eq!(
            make(md, &block(sp)),
            Ok(At::Anchor(block_anchor(md, quote, lines, None))),
            "{sp}"
        );
    }
}

#[test]
fn a_section_anchor_spans_its_heading_section() {
    for (sp, quote, lines) in [
        ("1:1-1:14", "# Retry policy", (1, 21)),
        ("19:1-19:13", "## Stacked on", (19, 21)),
    ] {
        let want = Anchor {
            scope: Scope::Section,
            ..block_anchor(DOC, quote, lines, None)
        };
        assert_eq!(make(DOC, &section(sp)), Ok(At::Anchor(want)), "{sp}");
    }
}

#[test]
fn a_forged_or_misplaced_sp_is_bad_anchor() {
    #[rustfmt::skip]
    let blocks: &[(&str, &str)] = &[
        (DOC, "11:9-11:14"),  // a cell: an owner, not a stop
        (DOC, "15:3-15:11"),  // the paragraph of a loose item
        (DOC, "9:1-9:15"),    // the header row
        (DOC, "9:1-11:15"),   // the table
        (DOC, "13:1-13:19"),  // the blockquote
        (DOC, "5:1-7:15"),    // the list
        (DOC, "3:1-3:76"),    // one column short
        (DOC, "3:1-3:77 "),
        (DOC, "99:1-99:5"),
        (DOC, ""),
        (MIXED, "2:1-2:13"),  // a description detail
    ];
    for &(md, sp) in blocks {
        assert_eq!(make(md, &block(sp)), Err(Reject::BadAnchor), "block {sp}");
    }

    #[rustfmt::skip]
    let sections: &[(&str, &str)] = &[
        (DOC, SP_PARA),        // not a heading
        (DOC, "6:1-7:15"),
        (MIXED, "12:3-12:10"), // a heading in a blockquote has no section
    ];
    for &(md, sp) in sections {
        assert_eq!(
            make(md, &section(sp)),
            Err(Reject::BadAnchor),
            "section {sp}"
        );
    }

    let html = "Intro para.\n\n<div>\n<p>Only here</p>\n</div>\n";
    #[rustfmt::skip]
    let quotes: &[(&str, &str, &str)] = &[
        (DOC, "15:1-15:11", "loose one"),        // a loose item: a stop, not an owner
        (DOC, "11:1-11:15", "fast"),             // a row
        (DOC, "9:1-11:15", "fast"),
        (DOC, "13:1-13:19", "quoted"),
        (DOC, "3:1-3:78", "adds"),               // one column long
        (html, "3:1-5:6", "Only here"),          // the raw HTML block
    ];
    for &(md, sp, q) in quotes {
        let req = AnchorReq::Quote {
            sp: Some(sp.to_string()),
            q: q.to_string(),
            pre: String::new(),
            suf: String::new(),
            off: Some(0),
            tl: Some(q.chars().count()),
        };
        assert_eq!(make(md, &req), Err(Reject::BadAnchor), "quote {sp}");
    }
}

#[test]
fn a_quote_that_cannot_be_placed_falls_back_to_its_enclosing_stop() {
    let twice = "retry and retry\n";
    #[rustfmt::skip]
    let cases: &[Case] = &[
        (DOC, SP_PARA, "not in here", "The client **adds retry** with *jittered* backoff -- because", (3, 3)),
        (DOC, "11:9-11:14", "nope", "| mode | fast |", (11, 11)),
        (DOC, "15:3-15:11", "nope", "- loose one", (15, 15)),
        (DOC, "7:3-7:15", "nope", "- nested item", (7, 7)),
        // Two equal occurrences: never a silent pick of the first
        (twice, "1:1-1:15", "retry", "retry and retry", (1, 1)),
        // Code text is not collapsed, so a doubled space doesn't match
        (MIXED, "4:1-6:3", "let  x", "```rust\nlet x = 1;\n```", (4, 6)),
        // Diagrams stay block-level
        (MIXED, "8:1-10:3", "graph", "```mermaid\ngraph TD\n```", (8, 10)),
    ];
    for &(md, sp, q, quote, lines) in cases {
        assert_eq!(
            make(md, &bare(sp, q)),
            Ok(At::Anchor(block_anchor(md, quote, lines, Some(q)))),
            "{sp} {q:?}"
        );
    }
}

#[test]
fn a_selection_in_raw_html_is_searched_in_the_whole_doc() {
    let html = "Intro para.\n\n<div>\n<p>Only here</p>\n<p>Twice here</p>\n<p>Twice here</p>\n<p>a &amp; b</p>\n</div>\n";

    // One hit: a quote anchor on the raw source
    assert_eq!(
        make(html, &in_html("Only here")),
        Ok(At::Anchor(quote_anchor(
            html,
            "Only here",
            ("Intro para. <div> <p>", "</p> <p>Twice here</p> <p>Twice "),
            (4, 4),
            "Only here",
        )))
    );

    // Two hits in one HTML block: the stop before it, with the quote kept
    assert_eq!(
        make(html, &in_html("Twice here")),
        Ok(At::Anchor(block_anchor(
            html,
            "Intro para.",
            (1, 1),
            Some("Twice here")
        )))
    );

    // The page shows `&` where the source has `&amp;`: nothing to point at
    assert_eq!(make(html, &in_html("a & b")), Ok(At::Doc));

    // No stop precedes the HTML block
    let first = "<div>\n<p>Twice here</p>\n<p>Twice here</p>\n</div>\n\nAfter para.\n";
    assert_eq!(make(first, &in_html("Twice here")), Ok(At::Doc));

    // Hits under different stops: nothing to prefer one by
    let apart = "Para one.\n\n<div>dup</div>\n\nPara two.\n\n<div>dup</div>\n";
    assert_eq!(make(apart, &in_html("dup")), Ok(At::Doc));

    // A unique hit in an owner's text is an ordinary quote anchor
    let owned = "Some words here.\n\n<div>x</div>\n";
    let got = anchored(owned, &in_html("words"));
    assert_eq!(
        (
            got.quote.as_str(),
            got.lines,
            got.scope,
            got.shown.as_deref()
        ),
        ("words", (1, 1), Scope::Quote, Some("words"))
    );
}

#[test]
fn doc_and_dir_requests_need_no_text() {
    assert_eq!(make(DOC, &AnchorReq::Doc), Ok(At::Doc));
    assert_eq!(make(DOC, &AnchorReq::Dir), Ok(At::Dir));
}

#[test]
fn a_change_request_is_left_to_the_caller_that_has_the_fold() {
    let got = make(
        DOC,
        &AnchorReq::Change {
            id: "12.0".to_string(),
        },
    );
    assert!(matches!(got, Err(Reject::Invalid(_))), "{got:?}");

    // Only the change form is refused: the block that change sits on anchors fine
    assert!(matches!(make(DOC, &block(SP_PARA)), Ok(At::Anchor(_))));
}

#[test]
fn a_request_made_on_another_rev_is_refused() {
    let doc = Doc::new(DOC);
    let older = rev("# Retry policy\n");
    for req in [
        first(DOC, SP_PARA, "adds"),
        block(SP_PARA),
        section("1:1-1:14"),
    ] {
        assert_eq!(
            anchor_from_req(&req, &doc, &older),
            Err(Reject::StaleRev),
            "{req:?}"
        );
    }
}

#[test]
fn every_anchor_made_relocates_ok_on_its_own_rev() {
    for md in [DOC, MIXED, "line one\r\nline two\r\n"] {
        let doc = Doc::new(md);
        let chain = baseline(md);
        let mut reqs: Vec<AnchorReq> = doc.blocks.stops.iter().map(|s| block(&s.sp)).collect();
        for owner in &doc.blocks.owners {
            let t = anchor::owner_text(md, &owner.sp).unwrap();
            let word = t.split_whitespace().last().unwrap();
            let byte = t.rfind(word).unwrap();
            reqs.push(selection(md, &owner.sp, word, t[..byte].chars().count()));
        }
        for req in reqs {
            let Ok(At::Anchor(anchor)) = anchor_from_req(&req, &doc, &doc.rev) else {
                panic!("{req:?} should anchor");
            };
            let at = ladder::relocate(&anchor, &doc, &chain);
            assert_eq!(
                (at.status, at.lines),
                (AnchorStatus::Ok, Some(anchor.lines)),
                "{req:?}"
            );
            if anchor.scope == Scope::Quote {
                let bytes = at.bytes.clone().expect("a found quote has bytes");
                assert_eq!(&md[bytes], anchor.quote, "{req:?}");
            }
        }
    }
}
