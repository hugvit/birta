//! One bounded walk of the served root, following symlinked folders.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use walkdir::{DirEntry, WalkDir};

use crate::paths::{self, FileKind, Roots};

/// Folders never walked or watched below the root, besides [`paths::DENY_DIRS`].
pub(crate) const SKIP_DIRS: [&str; 7] = [
    ".git",
    "node_modules",
    "target",
    ".venv",
    "__pycache__",
    ".Trash",
    "Library",
];

const FOLDER_REVIEW: &str = ".birta.jsonl";

/// Limits on one walk; hitting any of them sets [`ScanResult::truncated`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScanBudget {
    pub(crate) depth: usize,
    pub(crate) entries: usize,
    pub(crate) time: Duration,
}

impl Default for ScanBudget {
    fn default() -> Self {
        Self {
            depth: 12,
            entries: 50_000,
            time: Duration::from_millis(400),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ScanResult {
    /// Lexical, `/`-separated paths under the root. A folder reached by several links is
    /// listed once, under its lexicographically first path.
    pub(crate) md_files: Vec<String>,
    /// Per-doc review files, lexical like `md_files`.
    pub(crate) review_files: Vec<String>,
    /// Whether `<root>/.birta.jsonl` exists.
    pub(crate) folder_review: bool,
    /// Symlinks (not distinct targets) registered with the roots.
    pub(crate) links_registered: usize,
    pub(crate) truncated: bool,
}

/// Whether a folder named `name` below the root is never walked or watched.
pub(crate) fn is_skipped(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n)) || paths::is_denied_name(name)
}

/// Walks `root` (canonical) within `budget`, registering allowed symlinks with `roots`.
pub(crate) fn walk(root: &Path, roots: &Roots, budget: ScanBudget) -> ScanResult {
    let started = Instant::now();
    let mut result = ScanResult::default();
    let mut links = 0;
    let mut dirs_seen = HashSet::new();
    let mut entries = 0;

    // Siblings sort as `name/` for folders, so the walk runs in full-path string order and
    // the first path to reach a folder is its lexicographically first alias.
    let walker = WalkDir::new(root)
        .follow_links(true)
        .max_depth(budget.depth + 1)
        .sort_by(|a, b| sort_key(a).cmp(&sort_key(b)))
        .into_iter()
        .filter_entry(|entry| keep(entry, root, roots, &mut dirs_seen, &mut links));

    for entry in walker {
        entries += 1;
        if entries > budget.entries || started.elapsed() >= budget.time {
            result.truncated = true;
            break;
        }
        // Loops, dangling links and unreadable folders.
        let Ok(entry) = entry else { continue };
        if entry.depth() > budget.depth {
            result.truncated = true;
            continue;
        }
        if entry.depth() == 0 || !entry.file_type().is_file() {
            continue;
        }
        let Some(rel) = rel_of(&entry, root) else {
            continue;
        };
        let name = entry.file_name().to_str().unwrap_or("");
        if paths::is_review_file(name) {
            if entry.depth() == 1 && name.eq_ignore_ascii_case(FOLDER_REVIEW) {
                result.folder_review = true;
            } else {
                result.review_files.push(rel);
            }
        } else if paths::is_md(entry.path())
            && (!entry.path_is_symlink() || roots.resolve_file(&rel, FileKind::Markdown).is_ok())
        {
            result.md_files.push(rel);
        }
    }
    result.links_registered = links;
    result
}

/// Decides before descending: skips the skip set, refused links and folders already seen.
fn keep(
    entry: &DirEntry,
    root: &Path,
    roots: &Roots,
    dirs_seen: &mut HashSet<(u64, u64)>,
    links: &mut usize,
) -> bool {
    let dir = entry.file_type().is_dir();
    if entry.depth() > 0 {
        if dir && is_skipped(entry.file_name()) {
            return false;
        }
        if entry.path_is_symlink() {
            let registered = entry
                .path()
                .strip_prefix(root)
                .ok()
                .and_then(|rel| roots.register_link(rel));
            match registered {
                Some(_) => *links += 1,
                None if dir => return false,
                None => {}
            }
        }
    }
    if dir {
        let Ok(meta) = entry.metadata() else {
            return false;
        };
        return dirs_seen.insert((meta.dev(), meta.ino()));
    }
    true
}

fn sort_key(entry: &DirEntry) -> OsString {
    let mut key = entry.file_name().to_os_string();
    // Sorting sees links unfollowed, so ask the target.
    let ft = entry.file_type();
    if ft.is_dir() || (ft.is_symlink() && entry.path().is_dir()) {
        key.push("/");
    }
    key
}

fn rel_of(entry: &DirEntry, root: &Path) -> Option<String> {
    entry
        .path()
        .strip_prefix(root)
        .ok()?
        .to_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;
    use crate::paths::{Access, FileKind, PathError};

    /// `<tmp>/root` is served; everything else under `<tmp>` is not.
    struct Tree {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
    }

    impl Tree {
        fn new(files: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let root = base.join("root");
            std::fs::create_dir(&root).unwrap();
            let tree = Self {
                _tmp: tmp,
                base,
                root,
            };
            for file in files {
                tree.write(file);
            }
            tree
        }

        fn write(&self, rel: &str) {
            let path = self.base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "- [ ] x\n").unwrap();
        }

        /// Creates `<base>/<link>` pointing at `target` (absolute, or relative to the link).
        fn link(&self, link: &str, target: impl AsRef<Path>) {
            symlink(target, self.base.join(link)).unwrap();
        }

        fn roots(&self) -> Roots {
            Roots::new(self.root.clone(), Some(self.base.join("home")))
        }

        fn walk(&self, roots: &Roots) -> ScanResult {
            walk(&self.root, roots, ScanBudget::default())
        }
    }

    fn sorted(mut names: Vec<String>) -> Vec<String> {
        names.sort();
        names
    }

    fn strs(names: &[&str]) -> Vec<String> {
        sorted(names.iter().map(|n| n.to_string()).collect())
    }

    #[test]
    fn default_budget() {
        let budget = ScanBudget::default();
        assert_eq!(budget.depth, 12);
        assert_eq!(budget.entries, 50_000);
        assert_eq!(budget.time, Duration::from_millis(400));
    }

    #[test]
    fn walk_lists_docs_and_review_files_and_skips_the_skip_set() {
        let tree = Tree::new(&[
            "root/a.md",
            "root/B.MD",
            "root/notes.txt",
            "root/sub/c.markdown",
            "root/a.md.birta.jsonl",
            "root/sub/c.markdown.BIRTA.JSONL",
            "root/.birta.jsonl",
            "root/.context/plan.md",
            "root/.git/x.md",
            "root/node_modules/x.md",
            "root/sub/node_modules/y.md",
            "root/target/x.md",
            "root/.venv/x.md",
            "root/__pycache__/x.md",
            "root/.Trash/x.md",
            "root/Library/x.md",
            "root/.ssh/x.md",
        ]);
        std::fs::create_dir(tree.root.join("folder.md")).unwrap();
        let roots = tree.roots();

        let scan = tree.walk(&roots);

        assert_eq!(
            sorted(scan.md_files),
            strs(&["a.md", "B.MD", "sub/c.markdown", ".context/plan.md"])
        );
        assert_eq!(
            sorted(scan.review_files),
            strs(&["a.md.birta.jsonl", "sub/c.markdown.BIRTA.JSONL"])
        );
        assert!(scan.folder_review);
        assert_eq!(scan.links_registered, 0);
        assert!(!scan.truncated);
    }

    #[test]
    fn walk_without_folder_review() {
        let tree = Tree::new(&["root/a.md"]);
        let scan = tree.walk(&tree.roots());
        assert!(!scan.folder_review);
        assert!(scan.review_files.is_empty());
    }

    #[test]
    fn walk_follows_folder_links_and_registers_their_targets() {
        let tree = Tree::new(&[
            "root/a.md",
            "outside/doc.md",
            "outside/sub/deep.md",
            "vault/plan.md",
        ]);
        tree.link("root/linked", tree.base.join("outside"));
        tree.link("root/.context", "../vault");
        let roots = tree.roots();
        assert!(!roots.contains(&tree.base.join("vault/plan.md")));

        let scan = tree.walk(&roots);

        assert_eq!(
            sorted(scan.md_files),
            strs(&[
                "a.md",
                "linked/doc.md",
                "linked/sub/deep.md",
                ".context/plan.md"
            ])
        );
        assert_eq!(scan.links_registered, 2);
        assert!(roots.contains(&tree.base.join("outside/sub/deep.md")));
        assert!(roots.contains(&tree.base.join("vault/plan.md")));
    }

    #[test]
    fn walk_never_descends_into_guarded_link_targets() {
        let tree = Tree::new(&["root/a.md", "home/h.md", ".ssh/k.md", "outside/o.md"]);
        tree.link("root/slash", "/");
        tree.link("root/home", tree.base.join("home"));
        tree.link("root/keys", tree.base.join(".ssh"));
        tree.link("root/up", &tree.base);
        tree.link("root/gp", tree.base.parent().unwrap());
        let roots = tree.roots();

        let started = Instant::now();
        let scan = tree.walk(&roots);

        assert_eq!(scan.md_files, strs(&["a.md"]));
        assert_eq!(scan.links_registered, 0);
        assert!(!scan.truncated, "a refused link must not eat the budget");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!roots.contains(&tree.base.join("home/h.md")));
        assert!(!roots.contains(&tree.base.join("outside/o.md")));
    }

    #[test]
    fn walk_terminates_on_link_cycles() {
        let tree = Tree::new(&["root/a.md", "root/dir/x.md", "outside/doc.md"]);
        tree.link("root/self", "self");
        tree.link("root/p", "q");
        tree.link("root/q", "p");
        tree.link("root/dir/back", "..");
        tree.link("root/linked", tree.base.join("outside"));
        tree.link("outside/again", tree.base.join("outside"));
        tree.link("outside/home", tree.base.join("root"));
        let roots = tree.roots();

        let started = Instant::now();
        let scan = tree.walk(&roots);

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            sorted(scan.md_files),
            strs(&["a.md", "dir/x.md", "linked/doc.md"])
        );
        assert!(!scan.truncated);
    }

    #[test]
    fn two_links_to_one_target_list_only_the_first_alias() {
        let tree = Tree::new(&["root/a.md", "outside/doc.md", "o2/doc2.md"]);
        // Created out of order, so neither creation nor readdir order picks the winner.
        tree.link("root/two", tree.base.join("outside"));
        tree.link("root/one", tree.base.join("outside"));
        // `a.b/` sorts before `a/` as a string, though `a` comes first by file name.
        tree.link("root/a", tree.base.join("o2"));
        tree.link("root/a.b", tree.base.join("o2"));
        let roots = tree.roots();

        let started = Instant::now();
        let scan = tree.walk(&roots);

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(scan.links_registered, 4);
        assert_eq!(
            sorted(scan.md_files),
            strs(&["a.md", "one/doc.md", "a.b/doc2.md"])
        );
        assert!(!scan.truncated);
    }

    #[test]
    fn entry_budget_spans_link_targets() {
        let mut files: Vec<String> = (0..3).map(|i| format!("root/r{i}.md")).collect();
        files.extend((0..30).map(|i| format!("outside/o{i:02}.md")));
        let tree = Tree::new(&files.iter().map(String::as_str).collect::<Vec<_>>());
        tree.link("root/linked", tree.base.join("outside"));
        let budget = ScanBudget {
            entries: 10,
            ..ScanBudget::default()
        };

        let scan = walk(&tree.root, &tree.roots(), budget);

        assert!(scan.truncated);
        assert!(scan.md_files.len() <= 10, "{}", scan.md_files.len());
    }

    #[test]
    fn skip_set_applies_below_the_root_only() {
        let tree = Tree::new(&["Library/r/a.md", "target/notes/x.md"]);
        let root = tree.base.join("Library/r");
        tree.link("Library/r/n", tree.base.join("target/notes"));
        let roots = Roots::new(root.clone(), Some(tree.base.join("home")));

        let scan = walk(&root, &roots, ScanBudget::default());

        assert_eq!(sorted(scan.md_files), strs(&["a.md", "n/x.md"]));
        let mut map = crate::watcher::PathMap::new(&root);
        map.add(&tree.base.join("target/notes"), Path::new("n"));
        for (path, rel) in [
            (root.join("a.md"), "a.md"),
            (tree.base.join("target/notes/x.md"), "n/x.md"),
        ] {
            let got = crate::watcher::classify(&path, &map);
            let kinds: Vec<_> = got.iter().map(|e| (e.rel.as_str(), e.kind)).collect();
            assert_eq!(kinds, [(rel, crate::watcher::FsKind::Changed)], "{path:?}");
        }
    }

    #[test]
    fn walk_over_entry_budget_is_truncated() {
        let files: Vec<String> = (0..30).map(|i| format!("root/f{i:02}.md")).collect();
        let tree = Tree::new(&files.iter().map(String::as_str).collect::<Vec<_>>());
        let budget = ScanBudget {
            entries: 10,
            ..ScanBudget::default()
        };

        let scan = walk(&tree.root, &tree.roots(), budget);

        assert!(scan.truncated);
        assert!(scan.md_files.len() <= 10, "{}", scan.md_files.len());
    }

    #[test]
    fn walk_over_time_budget_is_truncated() {
        let files: Vec<String> = (0..200)
            .map(|i| format!("root/d{}/f{i}.md", i % 7))
            .collect();
        let tree = Tree::new(&files.iter().map(String::as_str).collect::<Vec<_>>());
        let budget = ScanBudget {
            time: Duration::ZERO,
            ..ScanBudget::default()
        };

        let scan = walk(&tree.root, &tree.roots(), budget);

        assert!(scan.truncated);
        assert!(scan.md_files.len() < 200);
    }

    #[test]
    fn walk_below_depth_budget_is_truncated() {
        let tree = Tree::new(&["root/top.md", "root/a/b/c/d/e/deep.md"]);
        let budget = ScanBudget {
            depth: 3,
            ..ScanBudget::default()
        };

        let scan = walk(&tree.root, &tree.roots(), budget);

        assert_eq!(scan.md_files, strs(&["top.md"]));
        assert!(scan.truncated);
    }

    #[test]
    fn walk_allows_file_links_out_of_root_read_only() {
        let tree = Tree::new(&[
            "root/a.md",
            "outside/doc.md",
            "outside/other.md",
            "outside/a.png",
            "outside/notes.txt",
            "outside/.env",
            "outside/doc.md.birta.jsonl",
            ".ssh/k.md",
        ]);
        tree.link("root/ext.md", tree.base.join("outside/doc.md"));
        tree.link("root/pic.png", tree.base.join("outside/a.png"));
        tree.link("root/fake.md", tree.base.join("outside/notes.txt"));
        tree.link("root/dot.png", tree.base.join("outside/.env"));
        tree.link(
            "root/review.md",
            tree.base.join("outside/doc.md.birta.jsonl"),
        );
        tree.link("root/key.md", tree.base.join(".ssh/k.md"));
        let roots = tree.roots();
        assert_eq!(
            roots.resolve_file("ext.md", FileKind::Markdown),
            Err(PathError::Denied),
            "before the walk"
        );

        let scan = tree.walk(&roots);

        assert!(scan.md_files.contains(&"ext.md".to_string()));
        assert_eq!(
            roots.resolve_file("ext.md", FileKind::Markdown),
            Ok(tree.base.join("outside/doc.md"))
        );
        assert_eq!(
            roots.resolve_file("pic.png", FileKind::Image),
            Ok(tree.base.join("outside/a.png"))
        );
        let doc = roots.resolve_doc("ext.md", Access::Write).unwrap();
        assert_eq!(doc.canonical, tree.base.join("outside/doc.md"));
        assert!(!doc.writable, "a file link out of the root is read-only");
        assert!(!doc.log_writable, "no review file beside it");
        assert!(roots.resolve_doc("ext.md", Access::Read).is_ok());

        let mut bad = Vec::new();
        for (rel, kind) in [
            ("fake.md", FileKind::Markdown),
            ("review.md", FileKind::Markdown),
            ("key.md", FileKind::Markdown),
            ("dot.png", FileKind::Image),
        ] {
            if let Ok(path) = roots.resolve_file(rel, kind) {
                bad.push(format!("{rel}: served {path:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(
            !roots.contains(&tree.base.join("outside/other.md")),
            "a file link must not open its folder"
        );
    }
}
