//! The folder's docs with activity, for the file menu and link dots: `dir_state`.

use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::review::state::Summary;
use crate::review::{self, FOLDER_REVIEW, review_path_for};

/// At most this many files go into one `dir_state`.
pub(crate) const MAX_LISTED: usize = 500;
/// Review files summarized by the startup scan; the rest wait for their next change.
pub(crate) const STARTUP_SUMMARIES: usize = 200;
const STARTUP_WORKERS: usize = 8;

/// One doc with activity: it has a review file, or a write since start nobody viewed.
#[derive(Clone, Debug)]
pub(crate) struct Listed {
    /// Every lexical path the doc is reachable by.
    pub(crate) paths: Vec<String>,
    /// `None` when its review file can't be read or folded.
    pub(crate) summary: Option<Summary>,
    /// A write since start that no page has viewed yet.
    pub(crate) news: bool,
    /// Epoch ms of the latest write to the doc or its review file.
    pub(crate) activity_ms: u64,
}

/// A file as last seen: a rewrite in place changes the mtime even when the length stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FileSig {
    ino: u64,
    len: u64,
    mtime_ns: i128,
}

impl FileSig {
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            ino: meta.ino(),
            len: meta.len(),
            mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        })
    }

    fn epoch_ms(self) -> u64 {
        u64::try_from(self.mtime_ns / 1_000_000).unwrap_or(0)
    }
}

/// The `dir_state` frame for `files`, capped at [`MAX_LISTED`]; totals count them all,
/// and the folder's own review file too.
pub(crate) fn dir_state_json(
    root: &str,
    truncated: bool,
    folder: Option<&Summary>,
    mut files: Vec<Listed>,
) -> serde_json::Value {
    let fresh = |f: &Listed| f.news || f.summary.as_ref().is_some_and(|s| s.counts.unseen > 0);
    let (mut drafts, mut unseen_files, mut decided, mut items) = (0u64, 0u64, 0u64, 0u64);
    for summary in files
        .iter()
        .filter_map(|f| f.summary.as_ref())
        .chain(folder)
    {
        drafts += u64::from(summary.counts.drafts);
        decided += u64::from(summary.counts.decided);
        items += u64::from(summary.counts.items);
    }
    unseen_files += files.iter().filter(|f| fresh(f)).count() as u64;
    unseen_files += u64::from(folder.is_some_and(|s| s.counts.unseen > 0));
    files.sort_by(|a, b| {
        fresh(b)
            .cmp(&fresh(a))
            .then(b.activity_ms.cmp(&a.activity_ms))
            .then_with(|| a.paths.cmp(&b.paths))
    });
    files.truncate(MAX_LISTED);
    let listed: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "paths": f.paths,
                "summary": f.summary,
                "news": fresh(f),
            })
        })
        .collect();
    serde_json::json!({
        "type": "dir_state",
        "root": root,
        "truncated": truncated,
        "folder": folder.map(|s| serde_json::json!({"summary": s})),
        "files": listed,
        "totals": {
            "drafts": drafts,
            "unseen_files": unseen_files,
            "decided": decided,
            "items": items,
        },
    })
}

#[derive(Debug, Default)]
struct Entry {
    paths: BTreeSet<String>,
    /// The signatures of the doc and its review file the summary was made from.
    summarized: Option<(Option<FileSig>, Option<FileSig>)>,
    summary: Option<Summary>,
    has_log: bool,
    news: bool,
    activity_ms: u64,
}

#[derive(Debug, Default)]
struct Inner {
    truncated: bool,
    docs: HashMap<PathBuf, Entry>,
    folder: Option<(Option<FileSig>, Option<Summary>)>,
}

/// Every doc under the served root with a review file or news, by canonical path.
#[derive(Debug)]
pub(crate) struct DirIndex {
    root: PathBuf,
    inner: Mutex<Inner>,
}

impl DirIndex {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            inner: Mutex::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes in a walk of the root: every doc's aliases, and which have review files.
    /// Returns the docs whose review files want a summary.
    pub(crate) fn seed(&self, md_files: &[String], truncated: bool) -> Vec<PathBuf> {
        let mut inner = self.lock();
        inner.truncated = truncated;
        let mut logged = Vec::new();
        for rel in md_files {
            let Ok(canonical) = self.root.join(rel).canonicalize() else {
                continue;
            };
            let entry = inner.docs.entry(canonical.clone()).or_default();
            entry.paths.insert(rel.clone());
            if !entry.has_log && review_path_for(&canonical).is_file() {
                entry.has_log = true;
                logged.push(canonical);
            }
        }
        logged
    }

    /// Summarizes the review files of `docs` on a few threads, up to the startup cap.
    pub(crate) fn summarize_all(&self, docs: &[PathBuf]) {
        let docs = &docs[..docs.len().min(STARTUP_SUMMARIES)];
        let chunk = docs.len().div_ceil(STARTUP_WORKERS).max(1);
        std::thread::scope(|scope| {
            for part in docs.chunks(chunk) {
                scope.spawn(move || {
                    for doc in part {
                        self.refresh(doc);
                    }
                });
            }
        });
        self.refresh_folder();
    }

    /// Reads the doc and its review file again, summarizing only when either changed.
    /// Returns whether the listing may have changed.
    pub(crate) fn refresh(&self, doc: &Path) -> bool {
        let log = review_path_for(doc);
        let sigs = (FileSig::of(doc), FileSig::of(&log));
        {
            let mut inner = self.lock();
            let Some(entry) = inner.docs.get_mut(doc) else {
                return false;
            };
            if entry.summarized == Some(sigs) {
                return false;
            }
            entry.has_log = sigs.1.is_some();
            if !entry.has_log {
                entry.summarized = Some(sigs);
                entry.summary = None;
                return true;
            }
        }
        let summary = review::summarize(&log, doc).ok();
        let mut inner = self.lock();
        // A write since the signatures were read has its own refresh coming; storing
        // this older summary under them could overwrite that one's
        if (FileSig::of(doc), FileSig::of(&log)) != sigs {
            return true;
        }
        if let Some(entry) = inner.docs.get_mut(doc) {
            entry.summarized = Some(sigs);
            entry.summary = summary;
            entry.activity_ms = [sigs.0, sigs.1]
                .into_iter()
                .flatten()
                .map(FileSig::epoch_ms)
                .max()
                .unwrap_or(0)
                .max(entry.activity_ms);
        }
        true
    }

    /// A doc at `rel` was written. With no review file and no page on it under any of its
    /// paths, as `viewed` tells, that is news.
    pub(crate) fn wrote(
        &self,
        doc: &Path,
        rel: &str,
        viewed: impl Fn(&str) -> bool,
        now_ms: u64,
    ) -> bool {
        {
            let mut inner = self.lock();
            let entry = inner.docs.entry(doc.to_path_buf()).or_default();
            entry.paths.insert(rel.to_string());
            entry.activity_ms = now_ms;
            if !review_path_for(doc).is_file() {
                let news = entry.news || !entry.paths.iter().any(|p| viewed(p));
                let changed = news != entry.news;
                entry.news = news;
                entry.has_log = false;
                if !changed {
                    return false;
                }
                return true;
            }
        }
        self.refresh(doc)
    }

    /// The doc at `rel` is gone; it leaves the listing once no alias reaches it.
    pub(crate) fn removed(&self, doc: &Path, rel: &str) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.docs.get_mut(doc) else {
            return false;
        };
        entry.paths.remove(rel);
        if entry.paths.is_empty() || !doc.is_file() {
            inner.docs.remove(doc);
        }
        true
    }

    /// A page opened the doc, so its news is read, for every page.
    pub(crate) fn viewed(&self, doc: &Path) -> bool {
        let mut inner = self.lock();
        inner
            .docs
            .get_mut(doc)
            .is_some_and(|e| std::mem::replace(&mut e.news, false))
    }

    /// Adds `rel` as a path of `doc`, for a doc first met by a note.
    pub(crate) fn met(&self, doc: &Path, rel: &str) {
        let mut inner = self.lock();
        let entry = inner.docs.entry(doc.to_path_buf()).or_default();
        entry.paths.insert(rel.to_string());
    }

    /// Reads the folder's own review file again, when it changed.
    pub(crate) fn refresh_folder(&self) -> bool {
        let path = self.root.join(FOLDER_REVIEW);
        let sig = FileSig::of(&path);
        if self.lock().folder.as_ref().map(|f| f.0) == Some(sig) {
            return false;
        }
        let summary = sig.and_then(|_| {
            let parsed = review::log::Log::open(&path).read().ok()??;
            Some(Summary::of(review::fold::fold_dir(&parsed)))
        });
        self.lock().folder = Some((sig, summary));
        true
    }

    /// Every doc with a review file.
    pub(crate) fn logged(&self) -> Vec<PathBuf> {
        let inner = self.lock();
        inner
            .docs
            .iter()
            .filter(|(_, e)| e.has_log)
            .map(|(doc, _)| doc.clone())
            .collect()
    }

    /// The first lexical path of `doc`.
    pub(crate) fn path_of(&self, doc: &Path) -> Option<String> {
        self.lock().docs.get(doc)?.paths.iter().next().cloned()
    }

    /// The current `dir_state` frame.
    pub(crate) fn frame(&self) -> serde_json::Value {
        let inner = self.lock();
        let files = inner
            .docs
            .iter()
            .filter(|(doc, e)| !e.paths.is_empty() && (e.has_log || e.news) && doc.is_file())
            .map(|(_, e)| Listed {
                paths: e.paths.iter().cloned().collect(),
                summary: e.summary.clone(),
                news: e.news,
                activity_ms: e.activity_ms,
            })
            .collect();
        let folder = inner.folder.as_ref().and_then(|f| f.1.as_ref());
        dir_state_json("", inner.truncated, folder, files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::state::{Counts, Phrase, Stage};
    use serde_json::json;

    fn summary(stage: Stage, drafts: u32, unseen: u32, decided: u32, items: u32) -> Summary {
        Summary {
            stage,
            phrase: Phrase {
                stage,
                sent: None,
                picked_up: None,
                replied: None,
            },
            counts: Counts {
                drafts,
                sent_unclaimed: 0,
                unseen,
                unasked: 0,
                not_handled: 0,
                open_asks: 0,
                later: 0,
                lost: 0,
                items,
                decided,
                attention: unseen,
            },
            notes: drafts,
        }
    }

    fn listed(path: &str, summary: Option<Summary>, news: bool, activity_ms: u64) -> Listed {
        Listed {
            paths: vec![path.to_string()],
            summary,
            news,
            activity_ms,
        }
    }

    fn listed_paths(state: &serde_json::Value) -> Vec<String> {
        state["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["paths"][0].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn the_frame_has_the_frozen_shape() {
        let state = dir_state_json(
            "",
            false,
            None,
            vec![listed(
                "a.md",
                Some(summary(Stage::Drafts, 2, 0, 1, 3)),
                false,
                1,
            )],
        );
        assert_eq!(state["type"], "dir_state");
        assert_eq!(state["root"], "");
        assert_eq!(state["truncated"], false);
        assert_eq!(state["folder"], serde_json::Value::Null);
        let file = &state["files"][0];
        assert_eq!(file["paths"], json!(["a.md"]));
        assert_eq!(file["summary"]["stage"], "drafts");
        assert_eq!(file["summary"]["counts"]["drafts"], 2);
        assert_eq!(file["news"], false);
        let keys: Vec<&String> = state.as_object().unwrap().keys().collect();
        let mut want = ["type", "root", "truncated", "folder", "files", "totals"];
        want.sort_unstable();
        let mut keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, want);
    }

    #[test]
    fn news_is_the_memory_flag_or_an_unseen_count() {
        let state = dir_state_json(
            "",
            false,
            None,
            vec![
                listed("flag.md", None, true, 1),
                listed(
                    "unseen.md",
                    Some(summary(Stage::Replied, 0, 2, 0, 0)),
                    false,
                    1,
                ),
                listed(
                    "quiet.md",
                    Some(summary(Stage::Drafts, 1, 0, 0, 0)),
                    false,
                    1,
                ),
            ],
        );
        let news: Vec<(String, bool)> = state["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["paths"][0].as_str().unwrap().to_string(),
                    f["news"].as_bool().unwrap(),
                )
            })
            .collect();
        assert!(news.contains(&("flag.md".to_string(), true)), "{news:?}");
        assert!(news.contains(&("unseen.md".to_string(), true)), "{news:?}");
        assert!(news.contains(&("quiet.md".to_string(), false)), "{news:?}");
    }

    #[test]
    fn files_sort_by_news_then_latest_activity_then_path() {
        let quiet = || Some(summary(Stage::Drafts, 1, 0, 0, 0));
        let state = dir_state_json(
            "",
            false,
            None,
            vec![
                listed("old-quiet.md", quiet(), false, 10),
                listed("b-new-quiet.md", quiet(), false, 50),
                listed("a-new-quiet.md", quiet(), false, 50),
                listed("old-news.md", None, true, 5),
                listed(
                    "unseen.md",
                    Some(summary(Stage::Replied, 0, 1, 0, 0)),
                    false,
                    20,
                ),
            ],
        );
        assert_eq!(
            listed_paths(&state),
            [
                "unseen.md",
                "old-news.md",
                "a-new-quiet.md",
                "b-new-quiet.md",
                "old-quiet.md"
            ]
        );
    }

    #[test]
    fn the_listing_is_capped_but_the_totals_count_every_file() {
        let files: Vec<Listed> = (0..600u64)
            .map(|i| {
                // Every tenth file has news, and so outranks the rest
                listed(
                    &format!("doc{i:03}.md"),
                    Some(summary(Stage::Drafts, 1, 0, 2, 5)),
                    i.is_multiple_of(10),
                    i,
                )
            })
            .collect();
        let state = dir_state_json("", false, None, files);
        let paths = listed_paths(&state);
        assert_eq!(paths.len(), MAX_LISTED);
        assert!(
            paths[..60].iter().all(|p| {
                let i: u64 = p[3..6].parse().unwrap();
                i.is_multiple_of(10)
            }),
            "news first: {:?}",
            &paths[..60]
        );
        assert_eq!(paths[0], "doc590.md", "latest activity first");
        assert!(
            !paths.contains(&"doc001.md".to_string()),
            "the oldest quiet files are cut"
        );
        assert_eq!(
            state["totals"],
            json!({"drafts": 600, "unseen_files": 60, "decided": 1200, "items": 3000})
        );
    }

    #[test]
    fn totals_skip_files_without_a_summary() {
        let state = dir_state_json(
            "",
            false,
            None,
            vec![
                listed("news.md", None, true, 1),
                listed("a.md", Some(summary(Stage::Drafts, 3, 0, 1, 4)), false, 1),
            ],
        );
        assert_eq!(
            state["totals"],
            json!({"drafts": 3, "unseen_files": 1, "decided": 1, "items": 4})
        );
    }

    #[test]
    fn truncated_and_the_folder_summary_pass_through() {
        let folder = summary(Stage::Sent, 0, 0, 0, 0);
        let state = dir_state_json("docs", true, Some(&folder), Vec::new());
        assert_eq!(state["root"], "docs");
        assert_eq!(state["truncated"], true);
        assert_eq!(state["folder"]["summary"]["stage"], "sent");
        assert_eq!(state["files"], json!([]));
        assert_eq!(
            state["totals"],
            json!({"drafts": 0, "unseen_files": 0, "decided": 0, "items": 0})
        );
    }

    #[test]
    fn the_folder_review_file_counts_in_the_totals() {
        let folder = summary(Stage::Drafts, 2, 1, 1, 0);
        let state = dir_state_json(
            "",
            false,
            Some(&folder),
            vec![listed(
                "a.md",
                Some(summary(Stage::Drafts, 3, 0, 1, 4)),
                false,
                1,
            )],
        );
        assert_eq!(
            state["totals"],
            json!({"drafts": 5, "unseen_files": 1, "decided": 2, "items": 4})
        );
    }

    #[test]
    fn dir_state_snapshot() {
        let alias = Listed {
            paths: vec!["notes/plan.md".to_string(), "plan.md".to_string()],
            ..listed("", Some(summary(Stage::Drafts, 2, 0, 1, 4)), false, 30)
        };
        let state = dir_state_json(
            "",
            false,
            Some(&summary(Stage::Idle, 0, 0, 0, 0)),
            vec![
                alias,
                listed("b.md", None, true, 20),
                listed("c.md", Some(summary(Stage::Replied, 0, 1, 2, 2)), false, 10),
            ],
        );
        insta::assert_snapshot!(serde_json::to_string_pretty(&state).unwrap());
    }
}
