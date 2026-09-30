use birta::rev::Rev;
use birta::review::hunk;
use birta::review::ops::Hunk;

fn h(a: u32, d: &[&str], i: &[&str]) -> Hunk {
    Hunk {
        a,
        d: d.iter().map(|s| s.to_string()).collect(),
        i: i.iter().map(|s| s.to_string()).collect(),
    }
}

/// Old text, new text, and their `git hash-object --no-filters` revs.
const CORPUS: &[(&str, &str, &str, &str)] = &[
    (
        "lf",
        "a\nb\nc\n",
        "de980441c3ab03a8c07dda1ad27b8a11f39deb1e",
        "a7bc997ebe8cf84988b83d2e83f1d193124fe593",
    ),
    (
        "crlf",
        "a\r\nb\r\nc\r\n",
        "b5eff5721aa4f9468960ecd78cd2764deab97b55",
        "a89add9c20e3ceca359f138105a9fbac53765d86",
    ),
    (
        "mixed",
        "a\nb\r\nc\rd\n",
        "4c2c284d366cfa631ac53d8a0aab5c63310acf62",
        "58b25077bf0f4a63804b30be13d344af17c90959",
    ),
    (
        "bom",
        "\u{feff}# T\n\nbody\n",
        "545d6cb693cdcdb20450ce2ee9298ec8a711aa6c",
        "73c54ec01cc8ca48f48622eb8665b00ea6853c58",
    ),
    (
        "bom removed",
        "\u{feff}a\n",
        "7a2a9ee4f512b845f3e1296343cfa646f2d47624",
        "78981922613b2afb6025042ff6bd878ac1994e85",
    ),
    (
        "no final eol",
        "a\nb",
        "0a207c060e61f3b88eaee0a8cd0696f46fb155eb",
        "1c943a98887754f364fafaa1da3ac56e0e0875a9",
    ),
    (
        "final eol added",
        "a\nb",
        "0a207c060e61f3b88eaee0a8cd0696f46fb155eb",
        "422c2b7ab3b3c668038da977e4e93a5fc623169c",
    ),
    (
        "from empty",
        "",
        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
        "78981922613b2afb6025042ff6bd878ac1994e85",
    ),
    (
        "to empty",
        "a\nb\n",
        "422c2b7ab3b3c668038da977e4e93a5fc623169c",
        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    ),
    (
        "unicode",
        "héllo 🎉\nñ\n",
        "5775320ac5d395230847b1b388d4c18571164e8b",
        "4ad28bc3b4c6bdfdfe68bb0767c05005bf96c1a4",
    ),
];

/// The new text of each corpus entry, by name.
fn new_text(name: &str) -> &'static str {
    match name {
        "lf" => "a\nB\nc\nd\n",
        "crlf" => "a\r\nc\r\nx\r\n",
        "mixed" => "a\r\nb\r\nc\rD\n",
        "bom" => "\u{feff}# Title\n\nbody\n",
        "bom removed" => "a\n",
        "no final eol" => "a\nb\nc",
        "final eol added" => "a\nb\n",
        "from empty" => "a\n",
        "to empty" => "",
        "unicode" => "héllo 🎉\nñandú\n日本\n",
        _ => unreachable!("{name}"),
    }
}

// --- diff: the exact hunks, since change ids are their positions ---

#[test]
fn diff_gives_similar_line_hunks_with_terminators_kept() {
    // Taken from `similar =3.2.0`, `TextDiff::from_lines(..).grouped_ops(0)`
    let expected: &[(&str, Vec<Hunk>)] = &[
        ("lf", vec![h(1, &["b\n"], &["B\n"]), h(3, &[], &["d\n"])]),
        ("crlf", vec![h(1, &["b\r\n"], &[]), h(3, &[], &["x\r\n"])]),
        (
            "mixed",
            vec![h(0, &["a\n"], &["a\r\n"]), h(3, &["d\n"], &["D\n"])],
        ),
        (
            "bom",
            vec![h(0, &["\u{feff}# T\n"], &["\u{feff}# Title\n"])],
        ),
        ("bom removed", vec![h(0, &["\u{feff}a\n"], &["a\n"])]),
        ("no final eol", vec![h(1, &["b"], &["b\n", "c"])]),
        ("final eol added", vec![h(1, &["b"], &["b\n"])]),
        ("from empty", vec![h(0, &[], &["a\n"])]),
        ("to empty", vec![h(0, &["a\n", "b\n"], &[])]),
        ("unicode", vec![h(1, &["ñ\n"], &["ñandú\n", "日本\n"])]),
    ];
    for (name, want) in expected {
        let (_, old, _, _) = CORPUS.iter().find(|c| c.0 == *name).unwrap();
        assert_eq!(hunk::diff(old, new_text(name)), *want, "{name}");
    }
}

#[test]
fn diff_of_several_uneven_hunks_is_in_old_text_coordinates() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let new = "1\nA\nB\nC\n3\n4\n6\n7\nZ\n8\n";
    assert_eq!(
        hunk::diff(old, new),
        vec![
            h(1, &["2\n"], &["A\n", "B\n", "C\n"]),
            h(4, &["5\n"], &[]),
            h(7, &[], &["Z\n"]),
        ]
    );
    let old = "a\nb\nc\nd\ne\n";
    let new = "a\nX\nY\nY2\nc\ne\nf\n";
    assert_eq!(
        hunk::diff(old, new),
        vec![
            h(1, &["b\n"], &["X\n", "Y\n", "Y2\n"]),
            h(3, &["d\n"], &[]),
            h(5, &[], &["f\n"]),
        ]
    );
}

#[test]
fn equal_texts_diff_to_nothing_and_nothing_applies_as_is() {
    for (_, old, _, _) in CORPUS {
        assert_eq!(hunk::diff(old, old), vec![], "{old:?}");
        assert_eq!(hunk::apply(old, &[]).unwrap(), *old);
    }
}

#[test]
fn a_long_doc_with_two_edits_gives_two_one_line_hunks() {
    // A short diff deadline gives up on a doc this size and returns one large replace
    let old: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    let new = old
        .replace("line 123\n", "changed a\n")
        .replace("line 17777\n", "changed b\n");
    assert_eq!(
        hunk::diff(&old, &new),
        vec![
            h(123, &["line 123\n"], &["changed a\n"]),
            h(17777, &["line 17777\n"], &["changed b\n"]),
        ]
    );
}

// --- apply and reverse ---

#[test]
fn apply_and_reverse_round_trip_every_line_ending_byte_for_byte() {
    for (name, old, old_rev, new_rev) in CORPUS {
        let new = new_text(name);
        assert_eq!(Rev::of(old.as_bytes()).as_str(), *old_rev, "{name} old");
        let hunks = hunk::diff(old, new);
        let forward = hunk::apply(old, &hunks).unwrap();
        assert_eq!(Rev::of(forward.as_bytes()).as_str(), *new_rev, "{name} new");
        let back = hunk::apply(&forward, &hunk::reverse(&hunks)).unwrap();
        assert_eq!(Rev::of(back.as_bytes()).as_str(), *old_rev, "{name} back");
    }
}

#[test]
fn reverse_swaps_sides_and_rebases_each_hunk_into_the_new_text() {
    let hunks = vec![
        h(1, &["2\n"], &["A\n", "B\n", "C\n"]),
        h(4, &["5\n"], &[]),
        h(7, &[], &["Z\n"]),
    ];
    let reversed = vec![
        h(1, &["A\n", "B\n", "C\n"], &["2\n"]),
        h(6, &[], &["5\n"]),
        h(8, &["Z\n"], &[]),
    ];
    assert_eq!(hunk::reverse(&hunks), reversed);
    assert_eq!(hunk::reverse(&reversed), hunks);
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let new = "1\nA\nB\nC\n3\n4\n6\n7\nZ\n8\n";
    assert_eq!(hunk::apply(old, &hunks).unwrap(), new);
    assert_eq!(hunk::apply(new, &reversed).unwrap(), old);
}

#[test]
fn reverse_of_nothing_is_nothing() {
    assert_eq!(hunk::reverse(&[]), vec![]);
}

#[test]
fn apply_refuses_hunks_that_do_not_fit_the_text() {
    let text = "a\nb\nc\nb\n";
    let refused: &[(&str, Vec<Hunk>)] = &[
        ("old lines differ", vec![h(1, &["x\n"], &["y\n"])]),
        // `b` is at line 3 too, so a search would find it
        ("old lines are elsewhere", vec![h(2, &["b\n"], &["B\n"])]),
        ("terminator differs", vec![h(1, &["b\r\n"], &["B\n"])]),
        ("terminator missing", vec![h(1, &["b"], &["B\n"])]),
        ("past the end", vec![h(5, &[], &["z\n"])]),
        ("deletes past the end", vec![h(3, &["b\n", "c\n"], &[])]),
        (
            "unsorted",
            vec![h(3, &["b\n"], &["B\n"]), h(0, &["a\n"], &["A\n"])],
        ),
        (
            "overlapping",
            vec![h(0, &["a\n", "b\n"], &[]), h(1, &["b\n"], &["B\n"])],
        ),
    ];
    for (why, hunks) in refused {
        assert!(hunk::apply(text, hunks).is_err(), "{why}");
    }
}

#[test]
fn apply_refuses_hunks_that_would_merge_or_split_lines() {
    let refused: &[(&str, &str, Vec<Hunk>)] = &[
        (
            "insert after an unterminated last line",
            "a\nb",
            vec![h(2, &[], &["c\n"])],
        ),
        (
            "unterminated line before more text",
            "a\nb\n",
            vec![h(1, &[], &["x"])],
        ),
        (
            "unterminated replacement before more text",
            "a\nb\n",
            vec![h(0, &["a\n"], &["A"])],
        ),
        ("two lines in one", "a\nb\n", vec![h(1, &[], &["x\ny\n"])]),
        (
            "a lone CR inside a line",
            "a\nb\n",
            vec![h(1, &[], &["x\ry\n"])],
        ),
        ("an empty line string", "a\nb\n", vec![h(1, &[], &[""])]),
    ];
    for (why, text, hunks) in refused {
        assert!(hunk::apply(text, hunks).is_err(), "{why}");
    }
    // The last line may lack a terminator, as the text's own last line can
    assert_eq!(
        hunk::apply("a\nb\n", &[h(2, &[], &["c"])]).unwrap(),
        "a\nb\nc"
    );
}

#[test]
fn apply_refuses_old_lines_that_are_not_single_lines() {
    // The bytes would match, but `d.len()` would miscount the lines removed
    let text = "a\nb\nc\n";
    for d in [&["b\nc\n"][..], &["b", "\n"], &[""]] {
        assert!(hunk::apply(text, &[h(1, d, &[])]).is_err(), "{d:?}");
    }
}

#[test]
fn apply_refuses_hunks_that_merge_a_lone_cr_with_the_next_line() {
    let refused = [
        // Would give "x\r\nb\n": a lone CR line runs into the next line's LF
        ("a\n\nb\n", h(0, &["a\n"], &["x\r"])),
        ("a\rb\n\n", h(1, &["b\n"], &[])),
        ("a\rb\n", h(1, &[], &["\n"])),
    ];
    for (text, hunk) in refused {
        assert!(
            hunk::apply(text, std::slice::from_ref(&hunk)).is_err(),
            "{text:?} {hunk:?}"
        );
    }
}

#[test]
fn diff_pins_myers_on_a_swapped_list() {
    // Patience and Lcs give other hunks here, which would change snap-derived ids
    assert_eq!(
        hunk::diff("- a\n\n- b\n\n- c\n", "- b\n\n- a\n\n- c\n"),
        vec![h(0, &[], &["- b\n", "\n"]), h(1, &["\n", "- b\n"], &[])]
    );
}

// --- generated edits ---

/// xorshift64: a fixed seed keeps failures reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, pool: &[&'a str]) -> &'a str {
        pool[self.below(pool.len())]
    }
}

const POOL: &[&str] = &[
    "alpha\n",
    "beta\n",
    "gamma\r\n",
    "delta\r",
    "\n",
    "\r\n",
    "- [ ] task\n",
    "ñandú 🎉\n",
    "日本語\r\n",
    "  indented\n",
];

fn gen_doc(rng: &mut Rng) -> Vec<String> {
    let mut lines: Vec<String> = (0..rng.below(30))
        .map(|_| rng.pick(POOL).to_string())
        .collect();
    if !lines.is_empty() && rng.below(4) == 0 {
        lines[0] = format!("\u{feff}{}", lines[0]);
    }
    if rng.below(4) == 0 {
        lines.push("no final eol".to_string());
    }
    lines
}

fn gen_edit(rng: &mut Rng, lines: &[String]) -> Vec<String> {
    let mut out = lines.to_vec();
    for _ in 0..=rng.below(5) {
        let at = rng.below(out.len() + 1);
        match rng.below(3) {
            0 if at < out.len() => {
                let n = (1 + rng.below(3)).min(out.len() - at);
                out.drain(at..at + n);
            }
            1 if at < out.len() => out[at] = format!("edited {}\n", rng.next() % 1000),
            _ => {
                let new: Vec<String> = (0..1 + rng.below(3))
                    .map(|_| rng.pick(POOL).to_string())
                    .collect();
                out.splice(at..at, new);
            }
        }
    }
    out
}

#[test]
fn generated_edits_round_trip_through_diff_apply_and_reverse() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut uneven_multi = 0;
    for case in 0..2_000 {
        let old: String = gen_doc(&mut rng).concat();
        // Most cases edit the old text, so hunks sit among kept lines; the rest start over
        let new: String = if case % 4 == 0 {
            gen_doc(&mut rng).concat()
        } else {
            gen_edit(&mut rng, &split(&old)).concat()
        };
        let hunks = hunk::diff(&old, &new);
        assert_eq!(hunks.is_empty(), old == new, "{old:?} -> {new:?}");
        for w in hunks.windows(2) {
            assert!(
                w[0].a as usize + w[0].d.len() <= w[1].a as usize,
                "unsorted or overlapping: {hunks:?}"
            );
        }
        for hk in &hunks {
            assert!(
                !hk.d.is_empty() || !hk.i.is_empty(),
                "empty hunk: {hunks:?}"
            );
        }
        if hunks.len() > 1
            && hunks[..hunks.len() - 1]
                .iter()
                .any(|hk| hk.d.len() != hk.i.len())
        {
            uneven_multi += 1;
        }

        assert_eq!(
            hunk::apply(&old, &hunks).unwrap(),
            new,
            "{old:?} -> {new:?}"
        );
        let reversed = hunk::reverse(&hunks);
        assert_eq!(
            hunk::apply(&new, &reversed).unwrap(),
            old,
            "{old:?} <- {new:?}"
        );
        assert_eq!(hunk::reverse(&reversed), hunks);
    }
    // The re-basing in `reverse` only shows when an earlier hunk changes the line count
    assert!(
        uneven_multi > 1_000,
        "only {uneven_multi} uneven multi-hunk cases"
    );
}

/// Lines with terminators, split on `\n`, `\r\n` and a lone `\r`.
fn split(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => Some(i + 2),
            b'\r' | b'\n' => Some(i + 1),
            _ => None,
        };
        if let Some(end) = end {
            lines.push(text[start..end].to_string());
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        lines.push(text[start..].to_string());
    }
    lines
}
