//! Content revisions: the git blob hash of a document's bytes.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Lowercase hex of `sha1("blob <len>\0" + bytes)`, the same as `git hash-object --no-filters`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rev(String);

impl Rev {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = sha1_smol::Sha1::new();
        hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
        hasher.update(bytes);
        Self(hasher.digest().to_string())
    }

    /// Accepts exactly 40 lowercase hex digits, as [`Rev::of`] produces.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        (s.len() == 40 && s.bytes().all(hex)).then(|| Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "for human-facing rev labels"))]
    pub(crate) fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl fmt::Display for Rev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Rev {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Rev {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| de::Error::custom(format!("not a rev: {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev_matches_git_hash_object() {
        let cases: [(&[u8], &str); 3] = [
            (b"", "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"),
            (b"hello\n", "ce013625030ba8dba906f756967f9e9ca394464a"),
            // BOM and CRLF are hashed as-is (`--no-filters`).
            (
                b"\xef\xbb\xbf- [ ] a\r\n",
                "9c81c7b2a473ace8ccdecd0381661d6bd8a393ce",
            ),
        ];
        for (bytes, want) in cases {
            assert_eq!(Rev::of(bytes).as_str(), want, "{bytes:?}");
        }
    }

    #[test]
    fn rev_differs_by_one_byte() {
        assert_ne!(Rev::of(b"- [ ] a\n"), Rev::of(b"- [x] a\n"));
        assert_ne!(Rev::of(b"a\n"), Rev::of(b"a\r\n"));
    }

    #[test]
    fn short_is_seven_char_prefix() {
        let rev = Rev::of(b"hello\n");
        assert_eq!(rev.short(), "ce01362");
    }

    #[test]
    fn parse_round_trips_and_rejects_non_revs() {
        let rev = Rev::of(b"hello\n");
        assert_eq!(Rev::parse(rev.as_str()), Some(rev.clone()));
        assert_eq!(rev.to_string(), rev.as_str());
        for bad in [
            "",
            "ce01362",
            "CE013625030BA8DBA906F756967F9E9CA394464A",
            "ce013625030ba8dba906f756967f9e9ca394464a0",
            "ce013625030ba8dba906f756967f9e9ca394464g",
            " ce013625030ba8dba906f756967f9e9ca394464",
        ] {
            assert_eq!(Rev::parse(bad), None, "{bad:?}");
        }
    }
}
