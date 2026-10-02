//! The `respond` heredoc grammar, parsed as a library: every line form, the tolerance for
//! a pasted block (indent, `EOF`, `\r`, a code fence), and the errors, all collected.

use birta::agent::respond::{self, Entry, ParseError};

fn ok(input: &str) -> Vec<Entry> {
    match respond::parse(input) {
        Ok(entries) => entries.into_iter().map(|(_, e)| e).collect(),
        Err(errors) => panic!("{input:?} must parse: {errors:?}"),
    }
}

/// The 1-based input lines the errors name, in order.
fn err_lines(input: &str) -> Vec<usize> {
    errors(input).iter().map(|e| e.line).collect()
}

fn errors(input: &str) -> Vec<ParseError> {
    match respond::parse(input) {
        Ok(entries) => panic!("{input:?} must be refused, got {entries:?}"),
        Err(errors) => errors,
    }
}

fn s(text: &str) -> String {
    text.to_string()
}

#[test]
fn every_line_form_parses() {
    let cases: Vec<(&str, Entry)> = vec![
        ("#12 done", Entry::Done { n: 12, note: None }),
        (
            "#12 done: kept the list, cut the table",
            Entry::Done {
                n: 12,
                note: Some(s("kept the list, cut the table")),
            },
        ),
        (
            "#14 answer: five retries is plenty",
            Entry::Answer {
                n: 14,
                body: s("five retries is plenty"),
            },
        ),
        (
            "#9 didnt: the section is referenced elsewhere",
            Entry::Didnt {
                n: 9,
                body: s("the section is referenced elsewhere"),
            },
        ),
        (
            "#1 covers \"Retries now use jitter\"",
            Entry::Covers {
                n: 1,
                quote: s("Retries now use jitter"),
            },
        ),
        (
            "ask \"detail two\": should this stay?",
            Entry::Ask {
                quote: s("detail two"),
                body: s("should this stay?"),
            },
        ),
        (
            "unasked \"more items\": fixed a typo on the way",
            Entry::Unasked {
                quote: s("more items"),
                body: Some(s("fixed a typo on the way")),
            },
        ),
        (
            "unasked \"more items\"",
            Entry::Unasked {
                quote: s("more items"),
                body: None,
            },
        ),
        (
            "@: explained the retry; cut nothing else",
            Entry::TurnNote {
                body: s("explained the retry; cut nothing else"),
            },
        ),
    ];
    for (input, want) in cases {
        assert_eq!(ok(input), vec![want], "{input:?}");
    }
}

#[test]
fn entries_carry_their_input_line() {
    let parsed = respond::parse("#1 done\n\n#2 answer: yes\n@: that's all\n").unwrap();
    let lines: Vec<usize> = parsed.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, [1, 3, 4]);
}

#[test]
fn a_line_that_is_no_item_start_continues_the_item_before() {
    let input = "\
#14 answer: two things.
#12 is fine as it stands
## and a heading survives

  indented stays indented
#15 done
@: first line
second line";
    assert_eq!(
        ok(input),
        vec![
            Entry::Answer {
                n: 14,
                body: s(
                    "two things.\n#12 is fine as it stands\n## and a heading survives\n\n  indented stays indented"
                ),
            },
            Entry::Done { n: 15, note: None },
            Entry::TurnNote {
                body: s("first line\nsecond line"),
            },
        ]
    );
}

#[test]
fn blank_lines_between_items_are_not_part_of_a_body() {
    assert_eq!(
        ok("#1 answer: yes\n\n\n#2 done\n\n"),
        vec![
            Entry::Answer {
                n: 1,
                body: s("yes")
            },
            Entry::Done { n: 2, note: None },
        ]
    );
}

#[test]
fn done_and_covers_take_no_continuation() {
    assert_eq!(err_lines("#1 done\nand also this\n#2 done"), [2]);
    assert_eq!(
        err_lines("#1 covers \"x\"\nmore words\n#2 done: why\nstray"),
        [2, 4]
    );
}

#[test]
fn a_line_before_any_item_is_refused() {
    assert_eq!(err_lines("thanks for the notes!\n#1 done"), [1]);
    assert_eq!(err_lines("#12 finished\n#13 done"), [1]);
}

#[test]
fn a_note_number_must_be_a_positive_u32() {
    for bad in [
        "#0 done",
        "#99999999999 done",
        "#-1 done",
        "# 1 done",
        "#1a done",
    ] {
        assert_eq!(err_lines(bad), [1], "{bad:?}");
    }
}

#[test]
fn one_outcome_per_note_but_any_number_of_covers() {
    for dup in [
        "#1 done\n#1 done",
        "#1 done\n#1 answer: also",
        "#1 answer: a\n#1 didnt: b",
    ] {
        assert_eq!(err_lines(dup), [2], "{dup:?}");
    }
    assert_eq!(
        ok("#1 covers \"a\"\n#1 covers \"b\"\n#1 done").len(),
        3,
        "covers repeat, and sit beside the outcome"
    );
}

#[test]
fn texts_that_need_something_after_the_colon_refuse_nothing() {
    for bad in [
        "#1 answer:",
        "#1 answer:   ",
        "#1 didnt:",
        "ask \"q\":",
        "@:",
    ] {
        assert_eq!(err_lines(bad), [1], "{bad:?}");
    }
    // A body on the next line is still a body.
    assert_eq!(
        ok("#1 answer:\nit is on this line"),
        vec![Entry::Answer {
            n: 1,
            body: s("it is on this line")
        }]
    );
}

#[test]
fn ask_needs_its_colon_and_quotes_are_closed() {
    assert_eq!(err_lines("ask \"q\" no colon"), [1]);
    assert_eq!(err_lines("#1 covers \"never closed"), [1]);
    assert_eq!(err_lines("#1 covers \"x\" trailing"), [1]);
    assert_eq!(err_lines("#1 covers \"\""), [1]);
}

#[test]
fn crlf_is_read_as_lf() {
    assert_eq!(
        ok("#1 answer: one\r\ntwo\r\n@: done\r\n"),
        vec![
            Entry::Answer {
                n: 1,
                body: s("one\ntwo")
            },
            Entry::TurnNote { body: s("done") },
        ]
    );
}

#[test]
fn quotes_may_be_curly_or_escaped() {
    assert_eq!(
        ok("#1 covers \u{201c}Retries now use jitter\u{201d}"),
        vec![Entry::Covers {
            n: 1,
            quote: s("Retries now use jitter")
        }]
    );
    assert_eq!(
        ok(r#"unasked "the \"fast\" path": renamed"#),
        vec![Entry::Unasked {
            quote: s("the \"fast\" path"),
            body: Some(s("renamed")),
        }]
    );
}

#[test]
fn a_quote_is_at_most_200_chars() {
    let at = "é".repeat(200);
    assert_eq!(
        ok(&format!("#1 covers \"{at}\"")),
        vec![Entry::Covers { n: 1, quote: at }]
    );
    assert_eq!(
        err_lines(&format!("#1 covers \"{}\"", "é".repeat(201))),
        [1]
    );
}

#[test]
fn a_pasted_indented_block_is_dedented_and_its_eof_dropped() {
    // Exactly what `birta review` prints under `next:`, answered.
    let pasted = "  #6 done
  #8 answer: yes
    and a deeper line
  #5 covers \"Retries now use jitter\"
  @: explained the retry
  EOF
";
    assert_eq!(
        ok(pasted),
        vec![
            Entry::Done { n: 6, note: None },
            Entry::Answer {
                n: 8,
                body: s("yes\n  and a deeper line"),
            },
            Entry::Covers {
                n: 5,
                quote: s("Retries now use jitter"),
            },
            Entry::TurnNote {
                body: s("explained the retry"),
            },
        ]
    );
    assert_eq!(ok("#1 done\nEOF"), vec![Entry::Done { n: 1, note: None }]);
    assert_eq!(
        ok("@: a\nEOF\r\n\n"),
        vec![Entry::TurnNote { body: s("a") }]
    );
}

#[test]
fn a_wrapping_code_fence_is_stripped_and_lines_count_from_the_input() {
    let fenced = "```\n#1 done\n#2 answer: yes\n```\n";
    assert_eq!(
        ok(fenced),
        vec![
            Entry::Done { n: 1, note: None },
            Entry::Answer {
                n: 2,
                body: s("yes")
            },
        ]
    );
    let with_lang = "```text\n  #1 done\n  EOF\n```";
    assert_eq!(ok(with_lang), vec![Entry::Done { n: 1, note: None }]);
    // Line 3 of what was piped in, fence included.
    assert_eq!(err_lines("```\n#1 done\n#1 done\n```"), [3]);
}

#[test]
fn empty_input_is_refused() {
    for empty in ["", "\n", "   \n\t\n", "EOF\n", "```\n```\n"] {
        assert!(respond::parse(empty).is_err(), "{empty:?}");
    }
}

#[test]
fn only_the_exact_template_placeholders_are_refused() {
    for left in [
        "#14 answer: <your answer>",
        "#1 covers \"<new text you wrote for the whole-doc note>\"",
        "@: <two lines: what you did, didn't, ask>",
        "#14 answer: see below\n<your answer>",
        "#1 covers \"<new text>\"",
    ] {
        let errs = errors(left);
        assert_eq!(errs.len(), 1, "{left:?}: {errs:?}");
        assert!(
            errs[0].msg.contains("placeholder"),
            "{left:?}: {}",
            errs[0].msg
        );
    }
    for fine in [
        "#14 answer: use Vec<T> here",
        "@: kept the <details> block",
        "#14 answer: <your own answer>",
        "#1 covers \"<new text> here\"",
    ] {
        assert!(respond::parse(fine).is_ok(), "{fine:?}");
    }
}

/// A doc about markup quotes tags that read like the template's placeholders.
#[test]
fn a_placeholder_inside_real_text_is_not_refused() {
    for fine in [
        "ask \"<question> element\": is it still needed?",
        "unasked \"the <why> tag\": it was missing",
        "#14 answer: wrap it in <reply> as before",
        "#14 didnt: the <why> block stays",
    ] {
        assert!(respond::parse(fine).is_ok(), "{fine:?}");
    }
}

#[test]
fn every_error_is_reported_not_only_the_first() {
    let input = "\
#1 done
#1 done
stray after done
#2 answer: <your answer>
ask \"q\" missing colon
#3 done";
    assert_eq!(err_lines(input), [2, 3, 4, 5]);
    for e in errors(input) {
        assert!(!e.msg.is_empty());
        assert_eq!(
            e.to_string(),
            format!("line {}: {}", e.line, e.msg),
            "the CLI prints errors as `respond: <this>`"
        );
    }
}

#[test]
fn only_a_last_eof_is_dropped() {
    assert_eq!(err_lines("#1 done\nEOF\n#2 done"), [2]);
    assert_eq!(
        ok("#1 answer: a heredoc ends at\nEOF\nand goes on"),
        vec![Entry::Answer {
            n: 1,
            body: s("a heredoc ends at\nEOF\nand goes on")
        }]
    );
    assert_eq!(
        ok("#1 answer: x\nEOF # end"),
        vec![Entry::Answer {
            n: 1,
            body: s("x\nEOF # end")
        }]
    );
    // The heredoc's own terminator sits at column 0 under an indented block
    assert_eq!(
        ok("  #1 done\n  #2 answer: yes\nEOF\n"),
        vec![
            Entry::Done { n: 1, note: None },
            Entry::Answer {
                n: 2,
                body: s("yes")
            },
        ]
    );
}

#[test]
fn the_dedent_is_the_common_prefix_of_lines_with_text() {
    assert_eq!(
        ok("\t#1 answer: a\n\t\tcode\n\t#2 done"),
        vec![
            Entry::Answer {
                n: 1,
                body: s("a\n\tcode")
            },
            Entry::Done { n: 2, note: None },
        ]
    );
    assert_eq!(
        ok("  #1 answer: a\n\n  b\n  #2 done\n"),
        vec![
            Entry::Answer {
                n: 1,
                body: s("a\n\nb")
            },
            Entry::Done { n: 2, note: None },
        ]
    );
    let got = ok("  #1 answer: a\n \n  b");
    assert!(
        matches!(&got[..], [Entry::Answer { n: 1, body }] if body.starts_with("a\n") && body.ends_with("\nb")),
        "a whitespace-only line sets no indent: {got:?}"
    );
}

#[test]
fn a_fence_inside_a_body_is_kept() {
    assert_eq!(
        ok("#1 answer: like this\n```rust\nlet x = 1;\n```\n#2 done"),
        vec![
            Entry::Answer {
                n: 1,
                body: s("like this\n```rust\nlet x = 1;\n```")
            },
            Entry::Done { n: 2, note: None },
        ]
    );
    // Line 1 is no fence, so the last line is no wrapper
    assert_eq!(
        ok("#1 answer: see\n```\ncode\n```"),
        vec![Entry::Answer {
            n: 1,
            body: s("see\n```\ncode\n```")
        }]
    );
    assert_eq!(
        ok("```\n#1 answer: x\n```sh\nls\n```\n```"),
        vec![Entry::Answer {
            n: 1,
            body: s("x\n```sh\nls\n```")
        }]
    );
}

#[test]
fn crlf_reaches_every_line_form() {
    assert_eq!(
        ok("#1 done\r\n#1 covers \"x\"\r\nunasked \"y\"\r\nask \"z\": q\r\n@: n\r\nEOF\r\n"),
        vec![
            Entry::Done { n: 1, note: None },
            Entry::Covers {
                n: 1,
                quote: s("x")
            },
            Entry::Unasked {
                quote: s("y"),
                body: None
            },
            Entry::Ask {
                quote: s("z"),
                body: s("q")
            },
            Entry::TurnNote { body: s("n") },
        ]
    );
}

#[test]
fn a_placeholder_is_refused_in_any_slot_and_any_quotes() {
    for left in [
        "#1 answer: <your answer>\r\n",
        "#1 didnt: <your answer>",
        "#1 done: <your answer>",
        "ask \"x\": <two lines: what you did, didn't, ask>",
        "#1 covers \u{201c}<new text you wrote for the whole-doc note>\u{201d}",
    ] {
        let errs = errors(left);
        assert_eq!(errs.len(), 1, "{left:?}: {errs:?}");
        assert!(
            errs[0].msg.contains("placeholder"),
            "{left:?}: {}",
            errs[0].msg
        );
    }
}

#[test]
fn the_colon_inside_a_quote_is_not_the_asks_colon() {
    assert_eq!(
        ok("ask \"ratio 1:2\": is this right?"),
        vec![Entry::Ask {
            quote: s("ratio 1:2"),
            body: s("is this right?")
        }]
    );
    assert_eq!(
        ok(r#"unasked "a \"b\": c": d"#),
        vec![Entry::Unasked {
            quote: s("a \"b\": c"),
            body: Some(s("d"))
        }]
    );
}

#[test]
fn ask_didnt_and_unasked_take_continuations() {
    assert_eq!(
        ok("ask \"x\": first\nsecond\n#1 didnt: one\ntwo\nunasked \"y\": a\nb"),
        vec![
            Entry::Ask {
                quote: s("x"),
                body: s("first\nsecond")
            },
            Entry::Didnt {
                n: 1,
                body: s("one\ntwo")
            },
            Entry::Unasked {
                quote: s("y"),
                body: Some(s("a\nb"))
            },
        ]
    );
}

#[test]
fn a_note_number_is_plain_ascii_digits_and_lines_count_from_the_top() {
    // `u32::from_str` takes a leading `+`
    assert_eq!(err_lines("#+1 done"), [1]);
    assert_eq!(err_lines("#\u{ff11} done"), [1]);
    assert_eq!(err_lines("\n\n#1 done\n#1 done"), [4]);
}

#[test]
fn mixed_tab_and_space_indents_share_no_prefix_so_nothing_is_stripped() {
    // Tabs are not expanded: `\t` and two spaces have no common prefix
    assert_eq!(err_lines("\t#1 done\n  #2 done"), [1, 2]);
    // The shared part is stripped, char for char, and what differs stays
    assert_eq!(err_lines(" \t#1 done\n  #2 done"), [1, 2]);
    assert_eq!(
        ok(" \t#1 answer: a\n \tb"),
        vec![Entry::Answer {
            n: 1,
            body: s("a\nb")
        }]
    );
}
