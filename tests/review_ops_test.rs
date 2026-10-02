use std::process::Command;

use birta::config::{Config, ReviewConfig};
use birta::review::{
    self,
    ops::{
        self, Act, AnchorReq, At, BlockScope, By, ByKind, CommentKind, Header, Hunk, Line, Op,
        ParsedLine, ReplyKind, Scope, To, Verdict,
    },
};
use serde_json::{Value, json};

const REV_A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
const REV_B: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
const HEADER: &str = r#"{"birta":"review/1","doc":"pr.md","about":"ops after the last hand-back are drafts; don't act on them"}"#;

fn ok(n: u32, raw: &str) -> Line {
    match ops::parse_line(n, raw) {
        ParsedLine::Ok(line) => line,
        other => panic!("{raw}: expected Ok, got {other:?}"),
    }
}

fn is_bad(n: u32, raw: &str) -> bool {
    matches!(ops::parse_line(n, raw), ParsedLine::Bad(b) if b.n == n)
}

fn is_unknown(n: u32, raw: &str, op: &str) -> bool {
    matches!(ops::parse_line(n, raw), ParsedLine::Unknown(u) if u.n == n && u.op == op)
}

fn log(lines: &[&str]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

// --- literal lines parse to the expected structs ---

#[test]
fn a_human_comment_line_parses_every_envelope_and_anchor_field() {
    let raw = format!(
        r#"{{"n":12,"ts":1790000000000,"by":{{"type":"human","name":"andres","via":"browser"}},"cid":"c-9f","op":"comment","at":{{"quote":"we just fail","pre":"On error ","suf":" and retry","lines":[5,5],"rev":"{REV_A}","scope":"quote","shown":"we just fail"}},"verdict":"no","body":"say why"}}"#
    );
    let line = ok(12, &raw);
    assert_eq!(line.n, 12);
    assert_eq!(line.ts, Some(1_790_000_000_000));
    assert_eq!(line.by.kind, ByKind::Human);
    assert_eq!(line.by.name.as_deref(), Some("andres"));
    assert_eq!(line.cid.as_deref(), Some("c-9f"));
    let Op::Comment {
        at: At::Anchor(anchor),
        verdict,
        body,
        ..
    } = &line.op
    else {
        panic!("not an anchored comment: {:?}", line.op);
    };
    assert_eq!(*verdict, Some(Verdict::No));
    assert_eq!(body.as_deref(), Some("say why"));
    assert_eq!(anchor.quote, "we just fail");
    assert_eq!(anchor.pre, "On error ");
    assert_eq!(anchor.suf, " and retry");
    assert_eq!(anchor.lines, (5, 5));
    assert_eq!(anchor.rev.to_string(), REV_A);
    assert_eq!(anchor.scope, Scope::Quote);
    assert_eq!(anchor.shown.as_deref(), Some("we just fail"));
}

#[test]
fn the_physical_line_number_wins_over_the_written_n() {
    let line = ok(5, r#"{"n":99,"op":"retract","to":3}"#);
    assert_eq!(line.n, 5);
    let line = ok(6, r#"{"op":"retract","to":3}"#);
    assert_eq!(line.n, 6);
}

#[test]
fn a_hand_appended_line_without_envelope_parses_with_unknown_author() {
    let line = ok(4, r#"{"op":"done","to":3,"turn":2}"#);
    assert_eq!(line.by.kind, ByKind::Unknown);
    assert_eq!(line.by.name, None);
    assert_eq!(line.ts, None);
    assert_eq!(line.cid, None);
    assert!(matches!(
        line.op,
        Op::Done {
            to: 3,
            turn: 2,
            late: false
        }
    ));
}

#[test]
fn submit_and_approve_carry_the_persisted_hold() {
    let line = ok(
        31,
        r#"{"op":"submit","upto":30,"not_before":1790000005000,"hold_ms":5000,"to":"*"}"#,
    );
    let Op::Submit {
        upto,
        not_before,
        hold_ms,
        to,
        ..
    } = &line.op
    else {
        panic!("{:?}", line.op);
    };
    assert_eq!(*upto, 30);
    assert_eq!(*not_before, 1_790_000_005_000);
    assert_eq!(*hold_ms, 5000);
    assert_eq!(*to, Some(To::One("*".to_string())));

    let line = ok(
        32,
        r#"{"op":"submit","upto":30,"not_before":1,"hold_ms":5000,"to":["claude","codex"]}"#,
    );
    assert!(matches!(
        &line.op,
        Op::Submit { to: Some(To::Many(names)), .. } if names == &["claude", "codex"]
    ));

    let raw =
        format!(r#"{{"op":"approve","rev":"{REV_A}","not_before":1790000905000,"hold_ms":5000}}"#);
    let Op::Approve {
        rev,
        not_before,
        hold_ms,
    } = ok(44, &raw).op
    else {
        panic!("not an approve");
    };
    assert_eq!(rev.to_string(), REV_A);
    assert_eq!(not_before, 1_790_000_905_000);
    assert_eq!(hold_ms, 5000);
}

#[test]
fn a_submit_or_approve_without_its_hold_fields_is_bad() {
    assert!(is_bad(3, r#"{"op":"submit","upto":2,"hold_ms":5000}"#));
    assert!(is_bad(3, r#"{"op":"submit","upto":2,"not_before":1}"#));
    assert!(is_bad(
        3,
        &format!(r#"{{"op":"approve","rev":"{REV_A}","hold_ms":5000}}"#)
    ));
}

#[test]
fn turn_ops_parse_to_their_fields() {
    assert!(matches!(
        ok(3, r#"{"op":"withdraw","to":31}"#).op,
        Op::Withdraw { to: 31 }
    ));
    assert!(matches!(
        ok(3, r#"{"op":"claim","round":31}"#).op,
        Op::Claim {
            round: 31,
            target: None
        }
    ));
    assert!(matches!(
        ok(3, r#"{"op":"yield","turn":33}"#).op,
        Op::Yield { turn: Some(33), .. }
    ));
    let Op::Reply {
        to,
        verdict,
        body,
        kind,
        ..
    } = ok(
        3,
        r#"{"op":"reply","to":12,"verdict":"your_call","body":"flaky","kind":"answer","late":true}"#,
    )
    .op
    else {
        panic!("not a reply");
    };
    assert_eq!(to, 12);
    assert_eq!(verdict, Some(Verdict::YourCall));
    assert_eq!(body.as_deref(), Some("flaky"));
    assert_eq!(kind, Some(ReplyKind::Answer));
    assert!(matches!(
        ok(3, r#"{"op":"covers","to":1,"quote":"Cut to 9","turn":33}"#).op,
        Op::Covers { to: 1, ref quote, turn: 33, late: false } if quote == "Cut to 9"
    ));
    assert!(matches!(
        ok(3, r#"{"op":"turn_note","body":"Kept the diagram.","turn":33}"#).op,
        Op::TurnNote { ref body, turn: 33, late: false } if body == "Kept the diagram."
    ));
    assert!(matches!(
        ok(3, r#"{"op":"amend","to":12,"body":"say why, briefly"}"#).op,
        Op::Amend { to: 12, .. }
    ));
}

#[test]
fn an_amend_drops_its_verdict_only_when_it_names_none() {
    // An amend from before the field keeps its verdict
    let old = ok(3, r#"{"op":"amend","to":12,"verdict":"yes"}"#);
    assert_eq!(
        serde_json::to_value(&old).unwrap().get("drop_verdict"),
        None,
        "false is omitted"
    );
    let dropped = ok(
        3,
        r#"{"op":"amend","to":12,"body":"why","drop_verdict":true}"#,
    );
    assert_eq!(
        serde_json::to_value(&dropped).unwrap()["drop_verdict"],
        json!(true)
    );
    // Setting and dropping a verdict at once means nothing, so the line is bad
    assert!(is_bad(
        3,
        r#"{"op":"amend","to":12,"verdict":"yes","drop_verdict":true}"#
    ));
    assert!(is_bad(3, r#"{"op":"amend","to":12,"drop_verdict":"yes"}"#));
}

#[test]
fn seen_takes_ids_in_all_three_grammars() {
    let line = ok(3, r##"{"op":"seen","ids":["12.0","#15","nh:12"]}"##);
    let Op::Seen { ids } = line.op else {
        panic!("not seen");
    };
    assert_eq!(ids, ["12.0", "#15", "nh:12"]);
    // The research name `changes` was renamed to `ids`.
    assert!(is_bad(3, r#"{"op":"seen","changes":["12.0"]}"#));
}

#[test]
fn owner_keeps_an_explicit_null_drafter() {
    assert!(matches!(
        ok(3, r#"{"op":"owner","drafted_by":"claude"}"#).op,
        Op::Owner { drafted_by: Some(ref n) } if n == "claude"
    ));
    assert!(matches!(
        ok(3, r#"{"op":"owner","drafted_by":null}"#).op,
        Op::Owner { drafted_by: None }
    ));
}

#[test]
fn edit_hunks_keep_terminators_bom_and_a_missing_final_eol() {
    let raw = format!(
        r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":0,"d":["﻿- [ ] a\r\n"],"i":["﻿- [x] a\r\n"]}},{{"a":4,"d":["last"],"i":[]}}],"act":"checkbox"}}"#
    );
    let Op::Edit {
        rev,
        to_rev,
        patch,
        of,
        act,
    } = ok(9, &raw).op
    else {
        panic!("not an edit");
    };
    assert_eq!(rev.to_string(), REV_A);
    assert_eq!(to_rev.to_string(), REV_B);
    assert_eq!(of, None);
    assert_eq!(act, Act::Checkbox);
    assert_eq!(
        patch,
        Some(vec![
            Hunk {
                a: 0,
                d: vec!["\u{feff}- [ ] a\r\n".to_string()],
                i: vec!["\u{feff}- [x] a\r\n".to_string()],
            },
            Hunk {
                a: 4,
                d: vec!["last".to_string()],
                i: vec![],
            },
        ])
    );
}

#[test]
fn edit_acts_are_the_frozen_three() {
    let edit = |act: &str| {
        format!(r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":null,"act":"{act}"}}"#)
    };
    assert!(matches!(
        ok(3, &edit("undo")).op,
        Op::Edit { act: Act::Undo, .. }
    ));
    assert!(matches!(
        ok(3, &edit("edit")).op,
        Op::Edit {
            act: Act::Edit,
            patch: None,
            ..
        }
    ));
    // `take` was a research name; the frozen set is undo|checkbox|edit.
    assert!(is_bad(3, &edit("take")));
}

#[test]
fn external_records_a_rev_only_step_with_a_null_patch() {
    let raw = format!(
        r#"{{"op":"external","rev":"{REV_A}","to_rev":"{REV_B}","patch":null,"cause":"git","head":"abc1234"}}"#
    );
    let Op::External {
        patch, cause, head, ..
    } = ok(3, &raw).op
    else {
        panic!("not external");
    };
    assert_eq!(patch, None);
    assert_eq!(cause.as_deref(), Some("git"));
    assert_eq!(head.as_deref(), Some("abc1234"));
}

#[test]
fn snap_holds_the_baseline_text_verbatim() {
    let raw = format!(r#"{{"op":"snap","rev":"{REV_A}","text":"﻿# T\r\nbody"}}"#);
    let Op::Snap { rev, text, .. } = ok(2, &raw).op else {
        panic!("not a snap");
    };
    assert_eq!(rev.to_string(), REV_A);
    assert_eq!(text, "\u{feff}# T\r\nbody");
}

#[test]
fn verdicts_use_their_snake_case_names() {
    let cases = [
        ("yes", Verdict::Yes),
        ("no", Verdict::No),
        ("ask", Verdict::Ask),
        ("later", Verdict::Later),
        ("your_call", Verdict::YourCall),
        ("rewrite", Verdict::Rewrite),
        ("stet", Verdict::Stet),
    ];
    for (name, want) in cases {
        let raw = format!(r#"{{"op":"comment","at":"doc","verdict":"{name}"}}"#);
        assert!(
            matches!(ok(3, &raw).op, Op::Comment { verdict: Some(v), .. } if v == want),
            "{name}"
        );
        assert_eq!(serde_json::to_value(want).unwrap(), json!(name));
    }
    for bad in ["YourCall", "yourcall", "Yes", "maybe"] {
        let raw = format!(r#"{{"op":"comment","at":"doc","verdict":"{bad}"}}"#);
        assert!(is_bad(3, &raw), "{bad}");
    }
}

// --- `At` ---

#[test]
fn at_parses_doc_dir_and_anchor_forms() {
    assert!(matches!(
        ok(3, r#"{"op":"comment","at":"doc","body":"x"}"#).op,
        Op::Comment { at: At::Doc, .. }
    ));
    assert!(matches!(
        ok(3, r#"{"op":"comment","at":"dir","body":"x"}"#).op,
        Op::Comment { at: At::Dir, .. }
    ));
    let block = format!(
        r#"{{"op":"comment","at":{{"quote":"- a","pre":"","suf":"","lines":[3,4],"rev":"{REV_A}","scope":"block"}},"verdict":"yes"}}"#
    );
    let Op::Comment {
        at: At::Anchor(anchor),
        ..
    } = ok(3, &block).op
    else {
        panic!("not an anchor");
    };
    assert_eq!(anchor.scope, Scope::Block);
    assert_eq!(anchor.shown, None);
    let section = block.replace(r#""scope":"block""#, r#""scope":"section""#);
    assert!(matches!(
        ok(3, &section).op,
        Op::Comment { at: At::Anchor(ref a), .. } if a.scope == Scope::Section
    ));
}

#[test]
fn at_rejects_other_strings_and_types() {
    for at in [r#""section""#, r#""Doc""#, r#""""#, "5", "null", "[]"] {
        let raw = format!(r#"{{"op":"comment","at":{at},"body":"x"}}"#);
        assert!(is_bad(3, &raw), "{at}");
    }
    assert!(is_bad(3, r#"{"op":"comment","body":"x"}"#), "missing at");
}

#[test]
fn at_serializes_to_its_three_forms() {
    assert_eq!(serde_json::to_value(At::Doc).unwrap(), json!("doc"));
    assert_eq!(serde_json::to_value(At::Dir).unwrap(), json!("dir"));
    let raw = format!(
        r#"{{"op":"comment","at":{{"quote":"q","pre":"p","suf":"s","lines":[1,2],"rev":"{REV_A}","scope":"section"}}}}"#
    );
    let Op::Comment { at, .. } = ok(3, &raw).op else {
        panic!("not a comment");
    };
    assert_eq!(
        serde_json::to_value(&at).unwrap(),
        json!({"quote":"q","pre":"p","suf":"s","lines":[1,2],"rev":REV_A,"scope":"section"}),
        "`shown` is omitted when absent"
    );
}

// --- every op survives parse and re-serialize unchanged ---

#[test]
fn every_op_reserializes_to_its_literal_line() {
    let by_agent = r#""by":{"type":"agent","name":"claude","via":"cli"}"#;
    let by_human = r#""by":{"type":"human","name":"andres","via":"browser"}"#;
    let by_birta = r#""by":{"type":"birta"}"#;
    let anchor = format!(
        r#"{{"quote":"we just fail","pre":"On error ","suf":" and retry","lines":[5,5],"rev":"{REV_A}","scope":"quote","shown":"we just fail"}}"#
    );
    let bodies = [
        format!(r#"{by_birta},"op":"snap","rev":"{REV_A}","text":"a\n","checkpoint":true"#),
        format!(
            r#"{by_human},"cid":"c-1","op":"comment","at":{anchor},"verdict":"no","body":"why""#
        ),
        format!(
            r#"{by_agent},"op":"comment","at":"doc","verdict":"ask","body":"which one?","turn":33"#
        ),
        format!(r#"{by_human},"op":"comment","at":"doc","body":"not done","to":36,"kind":"aside""#),
        format!(r#"{by_agent},"op":"reply","to":12,"body":"flaky S3","kind":"answer","late":true"#),
        format!(r#"{by_human},"op":"reply","to":15,"verdict":"yes""#),
        format!(r#"{by_human},"op":"amend","to":12,"body":"say why","verdict":"rewrite""#),
        format!(r#"{by_human},"op":"amend","to":12,"drop_verdict":true"#),
        format!(r#"{by_human},"op":"retract","to":12"#),
        format!(
            r#"{by_human},"op":"submit","upto":30,"not_before":1790000005000,"hold_ms":5000,"to":"*""#
        ),
        format!(r#"{by_human},"op":"withdraw","to":31"#),
        format!(r#"{by_agent},"op":"claim","round":31"#),
        format!(r#"{by_agent},"op":"done","to":12,"turn":33"#),
        format!(r#"{by_agent},"op":"covers","to":1,"quote":"Cut to 9","turn":33"#),
        format!(r#"{by_agent},"op":"unasked","quote":"retry","body":"tidied","turn":33"#),
        format!(r#"{by_agent},"op":"turn_note","body":"Kept the diagram.","turn":33"#),
        format!(r#"{by_agent},"op":"yield","turn":33"#),
        format!(r#"{by_human},"op":"yield","release":true"#),
        format!(r#"{by_agent},"op":"read","ref":{{"n":40,"h":"a1b2c3"}},"rev":"{REV_A}""#),
        format!(
            r#"{by_agent},"op":"suggest","at":{anchor},"with":"we retry","body":"clearer","of":"37.0""#
        ),
        format!(r#"{by_human},"op":"apply","to":41"#),
        format!(r#"{by_human},"op":"reject","to":41,"body":"no""#),
        format!(r##"{by_human},"op":"seen","ids":["37.0","#36","nh:12"]"##),
        format!(r#"{by_human},"op":"owner","drafted_by":null"#),
        format!(
            r#"{by_human},"op":"approve","rev":"{REV_A}","not_before":1790000905000,"hold_ms":5000"#
        ),
        format!(
            r#"{by_human},"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":2,"d":["x\r\n"],"i":[]}}],"of":"37.0","act":"undo""#
        ),
        format!(
            r#"{by_birta},"op":"external","rev":"{REV_A}","to_rev":"{REV_B}","patch":null,"cause":"git","head":"abc1234""#
        ),
    ];
    for (i, body) in bodies.iter().enumerate() {
        let n = i as u32 + 2;
        let raw = format!(r#"{{"n":{n},"ts":1790000000000,{body}}}"#);
        let want: Value = serde_json::from_str(&raw).unwrap();
        let got = serde_json::to_value(ok(n, &raw)).unwrap();
        assert_eq!(got, want, "{raw}");
    }
}

#[test]
fn a_built_line_serializes_to_the_envelope() {
    let line = Line {
        n: 3,
        ts: Some(1_790_000_000_000),
        by: By {
            kind: ByKind::Birta,
            name: None,
            via: None,
        },
        cid: None,
        op: Op::Owner { drafted_by: None },
    };
    assert_eq!(
        serde_json::to_value(&line).unwrap(),
        json!({"n":3,"ts":1_790_000_000_000u64,"by":{"type":"birta"},"op":"owner","drafted_by":null})
    );
    let withdraw = Line {
        n: 40,
        ts: Some(1),
        by: By {
            kind: ByKind::Human,
            name: Some("andres".to_string()),
            via: None,
        },
        cid: Some("c-2".to_string()),
        op: Op::Withdraw { to: 31 },
    };
    assert_eq!(
        serde_json::to_string(&withdraw).unwrap(),
        r#"{"n":40,"ts":1,"by":{"type":"human","name":"andres"},"cid":"c-2","op":"withdraw","to":31}"#
    );
}

#[test]
fn unknown_fields_on_a_known_op_are_ignored() {
    let line = ok(3, r#"{"op":"claim","round":31,"future":{"x":1}}"#);
    assert!(matches!(
        line.op,
        Op::Claim {
            round: 31,
            target: None
        }
    ));
}

// --- Ok / Unknown / Bad ---

#[test]
fn an_unknown_op_is_counted_not_fatal() {
    assert!(is_unknown(7, r#"{"op":"frobnicate","x":1}"#, "frobnicate"));
    // No `cancel` or `hold` ops: `withdraw` and the persisted `not_before` replace them.
    assert!(is_unknown(7, r#"{"op":"cancel","to":3}"#, "cancel"));
    assert!(is_unknown(7, r#"{"op":"hold","to":3}"#, "hold"));
}

#[test]
fn a_known_op_with_missing_or_mistyped_fields_is_bad_not_unknown() {
    for raw in [
        r#"{"op":"claim"}"#,
        r#"{"op":"claim","round":"31"}"#,
        r#"{"op":"withdraw"}"#,
        r#"{"op":"retract","to":-1}"#,
        r#"{"op":"reply","body":"no target"}"#,
        r#"{"op":"reply","to":3,"kind":"shrug"}"#,
    ] {
        assert!(is_bad(9, raw), "{raw}");
    }
}

#[test]
fn a_line_without_a_string_op_is_bad() {
    for raw in [
        "",
        "   ",
        "not json",
        r#"{"op":"claim","round":31"#,
        r#"{"n":3}"#,
        r#"{"op":5}"#,
        r#"{"op":null}"#,
        "[1,2]",
        r#""claim""#,
    ] {
        assert!(is_bad(9, raw), "{raw:?}");
    }
}

#[test]
fn a_bad_line_keeps_its_number_and_later_lines_keep_theirs() {
    let text = log(&[
        HEADER,
        r#"{"op":"owner","drafted_by":null}"#,
        "",
        "garbage",
        r#"{"op":"frobnicate"}"#,
        r#"{"op":"claim","round":2}"#,
        r#"{"op":"frobnicate"}"#,
        r#"{"op":"retract","to":2}"#,
    ]);
    let parsed = ops::parse(&text);
    assert!(!parsed.read_only);
    assert_eq!(
        parsed.header.as_ref().map(|h| h.doc.as_str()),
        Some("pr.md")
    );
    let ns: Vec<u32> = parsed.lines.iter().map(|l| l.n).collect();
    assert_eq!(ns, [2, 6, 8]);
    let bad: Vec<u32> = parsed.bad.iter().map(|b| b.n).collect();
    assert_eq!(bad, [3, 4]);
    let unknown: Vec<(u32, &str)> = parsed
        .unknown
        .iter()
        .map(|u| (u.n, u.op.as_str()))
        .collect();
    assert_eq!(unknown, [(5, "frobnicate"), (7, "frobnicate")]);
}

#[test]
fn a_final_newline_is_not_a_blank_bad_line() {
    let parsed = ops::parse(log(&[HEADER, r#"{"op":"claim","round":2}"#]));
    assert_eq!(parsed.lines.len(), 1);
    assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
}

#[test]
fn a_torn_last_line_is_bad_and_the_rest_still_parse() {
    let text = format!(
        "{}{}",
        log(&[HEADER, r#"{"op":"claim","round":2}"#]),
        r#"{"op":"retract","t"#
    );
    let parsed = ops::parse(&text);
    assert_eq!(parsed.lines.len(), 1);
    let bad: Vec<u32> = parsed.bad.iter().map(|b| b.n).collect();
    assert_eq!(bad, [3]);
}

#[test]
fn a_crlf_log_parses_like_an_lf_log() {
    let text = log(&[
        HEADER,
        r#"{"op":"claim","round":2}"#,
        r#"{"op":"withdraw","to":2}"#,
    ])
    .replace('\n', "\r\n");
    let parsed = ops::parse(&text);
    assert!(!parsed.read_only);
    assert!(parsed.header.is_some());
    assert_eq!(parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(), [2, 3]);
    assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
}

// --- the header ---

#[test]
fn the_header_is_the_exact_frozen_first_line() {
    assert_eq!(review::SCHEMA_LOG, "review/1");
    assert_eq!(review::SCHEMA_STATE, "review-state/1");
    assert_eq!(
        serde_json::to_string(&Header::new("pr.md")).unwrap(),
        HEADER
    );
}

#[test]
fn a_header_with_a_major_above_one_makes_the_log_read_only() {
    let with = |schema: &str| {
        let header = format!(r#"{{"birta":"{schema}","doc":"pr.md","about":"x"}}"#);
        ops::parse(log(&[
            &header,
            r#"{"op":"claim","round":2}"#,
            r#"{"op":"later_op"}"#,
        ]))
    };
    for schema in ["review/1", "review/1.4"] {
        assert!(!with(schema).read_only, "{schema}");
    }
    for schema in ["review/2", "review/10", "review/2.0"] {
        let parsed = with(schema);
        assert!(parsed.read_only, "{schema}");
        // Read-only still reads: lines parse and unknown ops are counted.
        assert_eq!(parsed.lines.len(), 1, "{schema}");
        assert_eq!(parsed.unknown.len(), 1, "{schema}");
    }
}

#[test]
fn a_log_without_a_valid_header_is_read_only() {
    for first in [
        r#"{"op":"claim","round":2}"#,
        r#"{"birta":"notes/1","doc":"pr.md","about":"x"}"#,
        r#"{"birta":"review/x","doc":"pr.md","about":"x"}"#,
        r#"{"birta":"review","doc":"pr.md","about":"x"}"#,
        "garbage",
        "",
    ] {
        let parsed = ops::parse(log(&[first, r#"{"op":"claim","round":2}"#]));
        assert!(parsed.read_only, "{first:?}");
        assert!(parsed.header.is_none(), "{first:?}");
        assert!(parsed.lines.iter().all(|l| l.n == 2), "{first:?}");
    }
}

// --- AnchorReq (client → server) ---

#[test]
fn anchor_req_parses_the_frozen_forms() {
    let parse = |v: Value| serde_json::from_value::<AnchorReq>(v);
    assert!(matches!(parse(json!({"k":"doc"})).unwrap(), AnchorReq::Doc));
    assert!(matches!(parse(json!({"k":"dir"})).unwrap(), AnchorReq::Dir));
    assert!(matches!(
        parse(json!({"k":"block","sp":"3:1-4:9","scope":"section"})).unwrap(),
        AnchorReq::Block { ref sp, scope: BlockScope::Section } if sp == "3:1-4:9"
    ));
    let AnchorReq::Quote {
        sp,
        q,
        pre,
        suf,
        off,
        tl,
    } = parse(json!({"k":"quote","sp":"5:1-5:40","q":"fail","pre":"we just ","suf":" here","off":8,"tl":17}))
        .unwrap()
    else {
        panic!("not a quote");
    };
    assert_eq!(sp.as_deref(), Some("5:1-5:40"));
    assert_eq!(
        (q.as_str(), pre.as_str(), suf.as_str()),
        ("fail", "we just ", " here")
    );
    assert_eq!((off, tl), (Some(8), Some(17)));
    // A selection inside raw HTML has no owner, so no `sp`.
    assert!(matches!(
        parse(json!({"k":"quote","q":"fail","pre":"","suf":""})).unwrap(),
        AnchorReq::Quote {
            sp: None,
            off: None,
            tl: None,
            ..
        }
    ));
    assert!(matches!(
        parse(json!({"k":"change","id":"37.0"})).unwrap(),
        AnchorReq::Change { ref id } if id == "37.0"
    ));
}

#[test]
fn anchor_req_rejects_forms_outside_the_protocol() {
    for v in [
        json!({"k":"blocks","sp":"1:1-1:3","sp_end":"4:1-4:3","q":"x"}),
        json!({"k":"block","sp":"1:1-1:3","scope":"quote"}),
        json!({"k":"block","scope":"block"}),
        json!({"k":"quote","sp":"1:1-1:3","pre":"","suf":""}),
        json!({"k":"change"}),
        json!({"k":"anchor"}),
        json!({"sp":"1:1-1:3"}),
        json!("doc"),
    ] {
        assert!(
            serde_json::from_value::<AnchorReq>(v.clone()).is_err(),
            "{v}"
        );
    }
}

// --- the gate ---

#[test]
fn review_config_defaults_to_off_with_git_exclude_on() {
    let empty: Config = toml::from_str("").unwrap();
    assert!(!empty.review.enabled);
    assert!(empty.review.git_exclude);
    let default = Config::default();
    assert!(!default.review.enabled);
    assert!(default.review.git_exclude);
    let table: Config = toml::from_str("[review]\n").unwrap();
    assert!(!table.review.enabled);
    assert!(table.review.git_exclude);
}

#[test]
fn review_config_fields_default_independently() {
    let on: Config = toml::from_str("[review]\nenabled = true\n").unwrap();
    assert!(on.review.enabled);
    assert!(on.review.git_exclude);
    let no_exclude: Config = toml::from_str("[review]\ngit_exclude = false\n").unwrap();
    assert!(!no_exclude.review.enabled);
    assert!(!no_exclude.review.git_exclude);
}

/// Runs in a child process with a controlled environment; a no-op in the normal run.
#[test]
fn gate_child() {
    let Ok(want) = std::env::var("BIRTA_TEST_GATE_WANT") else {
        return;
    };
    let config = Config {
        review: ReviewConfig {
            enabled: std::env::var("BIRTA_TEST_GATE_CONFIG").as_deref() == Ok("on"),
            git_exclude: true,
        },
        ..Config::default()
    };
    assert_eq!(review::enabled(&config), want == "on");
}

#[test]
fn the_gate_is_on_by_env_or_config_and_off_by_default() {
    let cases: [(Option<&str>, bool, bool); 13] = [
        (None, false, false),
        (Some("0"), false, false),
        (Some(""), false, false),
        (Some("yes-ish"), false, false),
        (Some("2"), false, false),
        (Some("01"), false, false),
        (Some(" 1"), false, false),
        (Some("1 "), false, false),
        (Some("true"), false, false),
        (Some("1"), false, true),
        (None, true, true),
        (Some("0"), true, true),
        (Some("1"), true, true),
    ];
    let exe = std::env::current_exe().unwrap();
    for (env, config_on, want) in cases {
        let mut cmd = Command::new(&exe);
        cmd.args(["--exact", "gate_child", "--test-threads=1"])
            .env_remove("BIRTA_REVIEW")
            .env("BIRTA_TEST_GATE_WANT", if want { "on" } else { "off" })
            .env(
                "BIRTA_TEST_GATE_CONFIG",
                if config_on { "on" } else { "off" },
            );
        if let Some(v) = env {
            cmd.env("BIRTA_REVIEW", v);
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "env={env:?} config={config_on}: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// --- adversary killers and rulings ---

#[test]
fn unknown_fields_are_ignored_at_every_level() {
    let raws = [
        r#"{"x":1,"op":"claim","round":2}"#.to_string(),
        r#"{"by":{"type":"agent","name":"claude","x":1},"op":"claim","round":2}"#.to_string(),
        format!(
            r#"{{"op":"comment","at":{{"quote":"q","pre":"","suf":"","lines":[1,1],"rev":"{REV_A}","scope":"quote","x":1}}}}"#
        ),
        format!(
            r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":0,"d":[],"i":["x\n"],"x":1}}],"act":"edit"}}"#
        ),
        r#"{"op":"submit","upto":2,"not_before":1,"hold_ms":5000,"reopen":true}"#.to_string(),
        r#"{"op":"read","ref":{"n":3,"h":"a1b2c3","x":1},"rev":"ce013625030ba8dba906f756967f9e9ca394464a"}"#
            .to_string(),
    ];
    for raw in raws {
        ok(3, &raw);
    }
}

#[test]
fn op_names_are_case_sensitive_and_never_guessed() {
    assert!(is_unknown(7, r#"{"op":"Claim","round":2}"#, "Claim"));
    assert!(is_unknown(7, r#"{"op":"claimm","round":2}"#, "claimm"));
    assert!(is_unknown(
        7,
        r#"{"op":"turnNote","body":"x","turn":2}"#,
        "turnNote"
    ));
    assert!(is_bad(7, r#"{"op":"","round":2}"#));
}

#[test]
fn author_types_are_exact() {
    // A later `owner` counts only from a human, so `Human` must not pass as one.
    for kind in ["Human", "HUMAN", "robot", ""] {
        let raw = format!(r#"{{"by":{{"type":"{kind}"}},"op":"owner","drafted_by":"x"}}"#);
        assert!(is_bad(3, &raw), "{kind}");
    }
    assert!(is_bad(
        3,
        r#"{"by":{"name":"andres"},"op":"retract","to":2}"#
    ));
    assert!(is_bad(3, r#"{"by":"human","op":"retract","to":2}"#));
    for (via, want) in [
        ("browser", "browser"),
        ("nvim", "nvim"),
        ("cli", "cli"),
        ("external", "external"),
    ] {
        let raw = format!(r#"{{"by":{{"type":"human","via":"{via}"}},"op":"retract","to":2}}"#);
        assert_eq!(serde_json::to_value(ok(3, &raw).by).unwrap()["via"], want);
    }
    assert!(is_bad(
        3,
        r#"{"by":{"type":"human","via":"web"},"op":"retract","to":2}"#
    ));
}

#[test]
fn a_duplicate_key_is_bad() {
    let raws = [
        r#"{"op":"claim","op":"withdraw","to":3}"#.to_string(),
        r#"{"op":"claim","round":2,"round":99}"#.to_string(),
        r#"{"n":3,"n":4,"op":"claim","round":2}"#.to_string(),
        r#"{"by":{"type":"agent","type":"human"},"op":"owner","drafted_by":"x"}"#.to_string(),
        format!(
            r#"{{"op":"comment","at":{{"quote":"q","quote":"r","pre":"","suf":"","lines":[1,1],"rev":"{REV_A}","scope":"quote"}}}}"#
        ),
        format!(
            r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":0,"a":1,"d":[],"i":[]}}],"act":"edit"}}"#
        ),
        r#"{"op":"frobnicate","x":1,"x":2}"#.to_string(),
    ];
    for raw in raws {
        assert!(is_bad(3, &raw), "{raw}");
    }
}

#[test]
fn a_mistyped_envelope_n_or_ts_is_bad() {
    for raw in [
        r#"{"n":"x","op":"retract","to":3}"#,
        r#"{"n":4294967296,"op":"retract","to":3}"#,
        r#"{"n":-1,"op":"retract","to":3}"#,
        r#"{"n":2.5,"op":"retract","to":3}"#,
        r#"{"ts":"2026-09-30T10:00:00Z","op":"retract","to":3}"#,
        r#"{"ts":"1790000000000","op":"retract","to":3}"#,
        r#"{"ts":1.79e12,"op":"retract","to":3}"#,
        r#"{"ts":-1,"op":"retract","to":3}"#,
        r#"{"cid":5,"op":"retract","to":3}"#,
    ] {
        assert!(is_bad(5, raw), "{raw}");
    }
}

#[test]
fn a_malformed_rev_is_bad() {
    let short = &REV_A[..7];
    let upper = REV_A.to_uppercase();
    for rev in [short, upper.as_str(), "", "not-a-rev"] {
        let approve = format!(r#"{{"op":"approve","rev":"{rev}","not_before":1,"hold_ms":5000}}"#);
        assert!(is_bad(3, &approve), "{rev}");
        let edit = format!(
            r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{rev}","patch":null,"act":"edit"}}"#
        );
        assert!(is_bad(3, &edit), "{rev}");
        let anchor = format!(
            r#"{{"op":"comment","at":{{"quote":"q","pre":"","suf":"","lines":[1,1],"rev":"{rev}","scope":"quote"}}}}"#
        );
        assert!(is_bad(3, &anchor), "{rev}");
    }
}

#[test]
fn a_future_enum_value_on_a_known_op_is_bad() {
    // New enum values need `review/2`; a 1.x reader must not half-read them.
    let raws = [
        r#"{"op":"comment","at":"doc","verdict":"maybe"}"#.to_string(),
        r#"{"op":"comment","at":"doc","kind":"margin"}"#.to_string(),
        r#"{"op":"comment","at":"folder"}"#.to_string(),
        format!(
            r#"{{"op":"comment","at":{{"quote":"q","pre":"","suf":"","lines":[1,1],"rev":"{REV_A}","scope":"paragraph"}}}}"#
        ),
        r#"{"op":"reply","to":3,"kind":"shrug"}"#.to_string(),
        format!(r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":null,"act":"take"}}"#),
    ];
    for raw in raws {
        assert!(is_bad(3, &raw), "{raw}");
    }
}

#[test]
fn comment_to_kind_and_turn_have_fixed_types() {
    let line = ok(
        3,
        r#"{"op":"comment","at":"doc","body":"not done","to":36,"kind":"aside","turn":33}"#,
    );
    assert!(matches!(
        line.op,
        Op::Comment {
            to: Some(36),
            kind: Some(CommentKind::Aside),
            turn: Some(33),
            ..
        }
    ));
    assert!(is_bad(3, r#"{"op":"comment","at":"doc","to":"36"}"#));
    assert!(is_bad(3, r#"{"op":"comment","at":"doc","turn":"33"}"#));
}

#[test]
fn submit_batch_is_a_number() {
    assert!(matches!(
        ok(
            3,
            r#"{"op":"submit","upto":2,"not_before":1,"hold_ms":5000,"batch":7}"#
        )
        .op,
        Op::Submit {
            batch: Some(7),
            to: None,
            ..
        }
    ));
    assert!(is_bad(
        3,
        r#"{"op":"submit","upto":2,"not_before":1,"hold_ms":5000,"batch":"b1"}"#
    ));
}

#[test]
fn a_bom_before_the_header_is_tolerated() {
    let text = format!("\u{feff}{}", log(&[HEADER, r#"{"op":"claim","round":2}"#]));
    let parsed = ops::parse(&text);
    assert!(!parsed.read_only);
    assert_eq!(parsed.header, Some(Header::new("pr.md")));
    assert_eq!(parsed.lines.len(), 1);
    assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
    // Only line 1 may carry one.
    let text = log(&[HEADER, "\u{feff}{\"op\":\"claim\",\"round\":2}"]);
    assert_eq!(ops::parse(&text).bad.len(), 1);
}

#[test]
fn the_header_version_must_be_plain_digits_with_major_one() {
    let parse = |schema: &str| {
        let header = format!(r#"{{"birta":"{schema}","doc":"pr.md","about":"x"}}"#);
        ops::parse(log(&[&header, r#"{"op":"claim","round":2}"#]))
    };
    for schema in ["review/1", "review/1.0", "review/1.12"] {
        assert!(!parse(schema).read_only, "{schema}");
    }
    for schema in [
        "review/0",
        "review/0.9",
        "review/+1",
        "review/-1",
        "review/01",
        "review/1.",
        "review/.1",
        "review/1.+2",
        "review/1.01",
        "review/ 1",
        "review/1 ",
        "review/1.2.3",
        "Review/1",
        "review/4294967297",
    ] {
        assert!(parse(schema).read_only, "{schema}");
    }
    let mistyped = r#"{"birta":1,"doc":"pr.md","about":"x"}"#;
    assert!(ops::parse(log(&[mistyped])).read_only);
    let duplicated = r#"{"birta":"review/2","birta":"review/1","doc":"pr.md","about":"x"}"#;
    assert!(ops::parse(log(&[duplicated])).read_only);
}

#[test]
fn a_complete_last_line_without_a_newline_is_ok() {
    let text = format!("{HEADER}\n{}", r#"{"op":"claim","round":2}"#);
    let parsed = ops::parse(&text);
    assert_eq!(parsed.lines.len(), 1);
    assert_eq!(parsed.lines[0].n, 2);
    assert!(parsed.bad.is_empty(), "{:?}", parsed.bad);
    let parsed = ops::parse(HEADER);
    assert!(!parsed.read_only);
    assert!(parsed.lines.is_empty() && parsed.bad.is_empty());
}

#[test]
fn invalid_utf8_spoils_only_its_own_line() {
    let mut bytes = log(&[HEADER, r#"{"op":"claim","round":2}"#]).into_bytes();
    bytes.extend_from_slice(b"{\"op\":\"reply\",\"to\":2,\"body\":\"caf\xe9\"}\n");
    bytes.extend_from_slice(b"{\"op\":\"withdraw\",\"to\":2}\n");
    let parsed = ops::parse(&bytes);
    assert!(!parsed.read_only);
    assert_eq!(parsed.lines.iter().map(|l| l.n).collect::<Vec<_>>(), [2, 4]);
    assert_eq!(parsed.bad.iter().map(|b| b.n).collect::<Vec<_>>(), [3]);
    let mut header = b"{\"birta\":\"review/1\",\"doc\":\"\xff\",\"about\":\"x\"}\n".to_vec();
    header.extend_from_slice(b"{\"op\":\"claim\",\"round\":2}\n");
    let parsed = ops::parse(&header);
    assert!(parsed.read_only);
    assert_eq!(parsed.lines.len(), 1);
}

#[test]
fn anchor_lines_that_are_zero_or_inverted_are_bad() {
    let comment = |lines: &str| {
        format!(
            r#"{{"op":"comment","at":{{"quote":"x","pre":"","suf":"","lines":{lines},"rev":"{REV_A}","scope":"quote"}}}}"#
        )
    };
    for lines in ["[5,2]", "[0,3]", "[0,0]", "[1,4294967296]"] {
        assert!(is_bad(4, &comment(lines)), "{lines}");
    }
    for lines in ["[1,1]", "[2,5]", "[4294967295,4294967295]"] {
        ok(4, &comment(lines));
    }
}

#[test]
fn a_hunk_whose_lines_run_past_u32_is_bad() {
    let edit = |a: u32, d: &str, i: &str| {
        format!(
            r#"{{"op":"edit","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":{a},"d":{d},"i":{i}}}],"act":"edit"}}"#
        )
    };
    let max = u32::MAX;
    for raw in [
        edit(max, "[]", "[]"),
        edit(max - 1, r#"["x\n"]"#, "[]"),
        edit(max - 1, "[]", r#"["x\n"]"#),
        edit(max - 2, r#"["x\n","y\n"]"#, "[]"),
    ] {
        assert!(is_bad(4, &raw), "{raw}");
    }
    ok(4, &edit(max - 2, r#"["x\n"]"#, r#"["y\n"]"#));
    // An earlier hunk's inserts push a later one's new lines past the end
    let pushed = format!(
        r#"{{"op":"external","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":0,"d":[],"i":["x\n","y\n"]}},{{"a":{},"d":[],"i":["z\n"]}}]}}"#,
        max - 3
    );
    assert!(is_bad(4, &pushed), "{pushed}");
    // Out of order, the second hunk would start before the first ended
    let unordered = format!(
        r#"{{"op":"external","rev":"{REV_A}","to_rev":"{REV_B}","patch":[{{"a":0,"d":["x\n"],"i":[]}},{{"a":0,"d":[],"i":["y\n"]}}]}}"#
    );
    assert!(is_bad(4, &unordered), "{unordered}");
}

#[test]
fn agent_outcomes_take_an_optional_late_flag() {
    let late = |raw: &str| match ok(3, raw).op {
        Op::Done { late, .. }
        | Op::Covers { late, .. }
        | Op::Unasked { late, .. }
        | Op::TurnNote { late, .. } => late,
        other => panic!("{other:?}"),
    };
    assert!(late(r#"{"op":"done","to":1,"turn":2,"late":true}"#));
    assert!(late(
        r#"{"op":"covers","to":1,"quote":"q","turn":2,"late":true}"#
    ));
    assert!(late(r#"{"op":"unasked","quote":"q","turn":2,"late":true}"#));
    assert!(late(
        r#"{"op":"turn_note","body":"b","turn":2,"late":true}"#
    ));
    assert!(!late(r#"{"op":"done","to":1,"turn":2}"#));
    // Absent when false, so older readers see the line as before
    let op = Op::Done {
        to: 1,
        turn: 2,
        late: false,
    };
    assert!(!serde_json::to_string(&op).unwrap().contains("late"));
}
