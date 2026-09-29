use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{Config, DebouncedEvent, DebouncedEventKind, Debouncer};

use crate::paths::{self, Roots};
use crate::render;
use crate::rev::Rev;
use crate::scan::{self, ScanBudget};
use crate::server::{AppState, ContentUpdate, WsEvent};

/// Most watch roots per server; more would exhaust inotify watches.
pub(crate) const MAX_ROOTS: usize = 64;

const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FsKind {
    Changed,
    Removed,
}

/// A markdown file event, under one lexical alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FsEvent {
    /// Lexical, `/`-separated path under the served root.
    pub(crate) rel: String,
    pub(crate) canonical: PathBuf,
    pub(crate) kind: FsKind,
}

/// Canonical watched paths to the lexical prefixes that reach them from the root.
pub(crate) struct PathMap {
    entries: Vec<(PathBuf, PathBuf)>,
}

impl PathMap {
    pub(crate) fn new(root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self {
            entries: vec![(root, PathBuf::new())],
        }
    }

    pub(crate) fn add(&mut self, canonical: &Path, lexical_rel: &Path) {
        let entry = (canonical.to_path_buf(), lexical_rel.to_path_buf());
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }

    /// Every lexical alias of `event_path`, which is never canonicalized itself: a deleted
    /// file can't be, and a link would be followed. Falls back to the canonical parent.
    pub(crate) fn lexical(&self, event_path: &Path) -> Vec<String> {
        let rels = self.aliases(event_path);
        if !rels.is_empty() {
            return rels;
        }
        let (Some(parent), Some(name)) = (event_path.parent(), event_path.file_name()) else {
            return rels;
        };
        match parent.canonicalize() {
            Ok(parent) => self.aliases(&parent.join(name)),
            Err(_) => rels,
        }
    }

    fn aliases(&self, path: &Path) -> Vec<String> {
        let mut rels: Vec<String> = Vec::new();
        for (canonical, lexical) in &self.entries {
            let Ok(rest) = path.strip_prefix(canonical) else {
                continue;
            };
            let rel = if rest.as_os_str().is_empty() {
                lexical.clone()
            } else {
                lexical.join(rest)
            };
            if let Some(rel) = rel.to_str().filter(|r| !r.is_empty())
                && !rels.iter().any(|r| r == rel)
            {
                rels.push(rel.to_string());
            }
        }
        rels
    }
}

/// Classifies one reported path; empty for anything that is not a served markdown file.
pub(crate) fn classify(event_path: &Path, map: &PathMap) -> Vec<FsEvent> {
    if !paths::is_md(event_path) {
        return Vec::new();
    }
    let kind = match std::fs::symlink_metadata(event_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FsKind::Removed,
        Ok(meta) if meta.is_file() => FsKind::Changed,
        // A link to a file; a dangling one (an editor lock) is not a doc.
        Ok(meta) if meta.is_symlink() && event_path.is_file() => FsKind::Changed,
        _ => return Vec::new(),
    };
    let canonical = canonical_of(event_path);
    map.lexical(event_path)
        .into_iter()
        .filter(|rel| !in_skipped_dir(rel))
        .map(|rel| FsEvent {
            rel,
            canonical: canonical.clone(),
            kind,
        })
        .collect()
}

fn in_skipped_dir(rel: &str) -> bool {
    Path::new(rel)
        .parent()
        .is_some_and(|dir| dir.components().any(|c| scan::is_skipped(c.as_os_str())))
}

fn canonical_of(path: &Path) -> PathBuf {
    path.canonicalize()
        .ok()
        .or_else(|| {
            let parent = path.parent()?.canonicalize().ok()?;
            Some(parent.join(path.file_name()?))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

/// A watched link target and the lexical path of the link reaching it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    canonical: PathBuf,
    lexical: PathBuf,
    dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Watch {
    path: PathBuf,
    recursive: bool,
}

impl Watch {
    fn for_target(target: &Target) -> Option<Self> {
        if target.dir {
            return Some(Self {
                path: target.canonical.clone(),
                recursive: true,
            });
        }
        target.canonical.parent().map(|parent| Self {
            path: parent.to_path_buf(),
            recursive: false,
        })
    }

    fn covers(&self, target: &Target) -> bool {
        if self.recursive {
            target.canonical.starts_with(&self.path)
        } else {
            !target.dir && target.canonical.parent() == Some(self.path.as_path())
        }
    }
}

/// What a stat tells about a file's bytes. inotify reports opens, and the debouncer drops
/// event kinds, so this is how a read (ours included) is told apart from an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileSig {
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileSig {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            ino: meta.ino(),
            len: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

/// State the event handler reads, apart from the debouncer it runs under.
struct Shared {
    roots: Arc<Roots>,
    map: RwLock<PathMap>,
    /// Every lexical doc path seen, so a vanished folder can report the docs it held.
    known: Mutex<HashSet<String>>,
    /// Last signature per canonical doc; an event that leaves it unchanged is dropped.
    sigs: Mutex<HashMap<PathBuf, FileSig>>,
}

struct Watches {
    debouncer: Debouncer<notify::RecommendedWatcher>,
    /// The root first; it is never removed.
    watches: Vec<Watch>,
    targets: Vec<Target>,
}

impl Watches {
    fn covered(&self, target: &Target) -> bool {
        self.watches.iter().any(|w| w.covers(target))
    }

    /// Watches `target`'s folder, replacing the watches the new one covers.
    fn watch(&mut self, watch: Watch) {
        if watch.recursive {
            let (covered, kept): (Vec<_>, Vec<_>) = self
                .watches
                .drain(..)
                .enumerate()
                .partition(|(i, w)| *i > 0 && w.path.starts_with(&watch.path));
            self.watches = kept.into_iter().map(|(_, w)| w).collect();
            for (_, old) in covered {
                let _ = self.debouncer.watcher().unwatch(&old.path);
            }
        }
        let mode = if watch.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watch_path(&mut self.debouncer, &watch.path, mode);
        self.watches.push(watch);
    }

    /// Drops watches nothing needs any more, then re-watches targets left uncovered.
    fn reconcile(&mut self) {
        let targets = &self.targets;
        let (unused, kept): (Vec<_>, Vec<_>) = self
            .watches
            .drain(..)
            .enumerate()
            .partition(|(i, w)| *i > 0 && !targets.iter().any(|t| w.covers(t)));
        self.watches = kept.into_iter().map(|(_, w)| w).collect();
        for (_, old) in unused {
            let _ = self.debouncer.watcher().unwatch(&old.path);
        }
        for target in self.targets.clone() {
            if !self.covered(&target)
                && self.watches.len() < MAX_ROOTS
                && let Some(watch) = Watch::for_target(&target)
            {
                self.watch(watch);
            }
        }
    }
}

enum LinkOp {
    /// A symlink appeared or changed at this lexical path.
    Changed(PathBuf),
    /// Something at this lexical path vanished; links at or below it are forgotten.
    Removed(PathBuf),
}

/// The root plus one watch per symlinked target, since FSEvents does not follow links.
pub(crate) struct WatchSet {
    shared: Arc<Shared>,
    watches: Arc<Mutex<Watches>>,
}

impl WatchSet {
    pub(crate) fn new(
        roots: Arc<Roots>,
        on_events: impl Fn(Vec<FsEvent>) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let root = roots.root().to_path_buf();
        let shared = Arc::new(Shared {
            map: RwLock::new(PathMap::new(&root)),
            known: Mutex::new(HashSet::new()),
            sigs: Mutex::new(HashMap::new()),
            roots,
        });
        let (link_tx, link_rx) = mpsc::channel();
        let request_tx = link_tx.clone();
        shared.roots.observe_links(move |lexical| {
            let _ = request_tx.send(LinkOp::Changed(lexical.to_path_buf()));
        });
        let handler_shared = Arc::clone(&shared);
        // Links are watched as roots of their own; following them would also walk
        // into targets the guards refuse, such as `/`.
        let config = Config::default()
            .with_timeout(DEBOUNCE)
            .with_notify_config(notify::Config::default().with_follow_symlinks(false));
        let mut debouncer =
            notify_debouncer_mini::new_debouncer_opt::<_, notify::RecommendedWatcher>(
                config,
                move |events: notify_debouncer_mini::DebounceEventResult| match events {
                    Ok(events) => handle_batch(&handler_shared, &events, &link_tx, &on_events),
                    Err(e) => eprintln!("birta: watcher error: {e}"),
                },
            )?;
        watch_path(&mut debouncer, &root, RecursiveMode::Recursive);
        let watches = Arc::new(Mutex::new(Watches {
            debouncer,
            watches: vec![Watch {
                path: root,
                recursive: true,
            }],
            targets: Vec::new(),
        }));

        // The handler only queues link changes: it runs on the debouncer's thread, and
        // a watch change must not wait on the lock of the set it is part of.
        let weak = Arc::downgrade(&watches);
        let linker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("birta links".to_string())
            .spawn(move || apply_links(&linker_shared, &weak, &link_rx))?;

        Ok(Self { shared, watches })
    }

    /// Maps `lexical_rel` to `canonical`, watching it unless an existing root covers it.
    /// Returns whether a new watch was added.
    pub(crate) fn add_root(&mut self, canonical: &Path, lexical_rel: &Path) -> bool {
        add_target(&self.shared, &self.watches, canonical, lexical_rel)
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only tests inspect the watches")
    )]
    pub(crate) fn watched(&self) -> Vec<PathBuf> {
        let watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        watches.watches.iter().map(|w| w.path.clone()).collect()
    }

    /// Records the docs a walk found, under every alias, so a vanished folder reports them.
    pub(crate) fn seed(&self, md_files: &[String]) {
        let map = self.shared.map.read().unwrap_or_else(|e| e.into_inner());
        let mut known = self.shared.known.lock().unwrap_or_else(|e| e.into_inner());
        let mut sigs = self.shared.sigs.lock().unwrap_or_else(|e| e.into_inner());
        for rel in md_files {
            known.insert(rel.clone());
            if let Ok(canonical) = self.shared.roots.root().join(rel).canonicalize() {
                known.extend(map.lexical(&canonical));
                if let Some(sig) = FileSig::of(&canonical) {
                    sigs.insert(canonical, sig);
                }
            }
        }
    }
}

fn add_target(shared: &Shared, watches: &Mutex<Watches>, canonical: &Path, lexical: &Path) -> bool {
    let target = Target {
        canonical: canonical.to_path_buf(),
        lexical: lexical.to_path_buf(),
        dir: canonical.is_dir(),
    };
    let mut w = watches.lock().unwrap_or_else(|e| e.into_inner());
    if w.targets.contains(&target) {
        return false;
    }
    let added = if w.covered(&target) {
        false
    } else if w.watches.len() >= MAX_ROOTS {
        eprintln!(
            "birta: warning: more than {MAX_ROOTS} watched folders; {} will not live-reload",
            lexical.display()
        );
        return false;
    } else {
        let Some(watch) = Watch::for_target(&target) else {
            return false;
        };
        w.watch(watch);
        true
    };
    w.targets.push(target);
    rebuild_map(shared, &w.targets);
    added
}

/// Forgets every target reached at or below `lexical`, and the watches only they needed.
fn forget_targets(shared: &Shared, watches: &Mutex<Watches>, lexical: &Path) {
    let mut w = watches.lock().unwrap_or_else(|e| e.into_inner());
    let before = w.targets.len();
    w.targets.retain(|t| !t.lexical.starts_with(lexical));
    if w.targets.len() != before {
        w.reconcile();
        rebuild_map(shared, &w.targets);
    }
}

fn rebuild_map(shared: &Shared, targets: &[Target]) {
    let mut map = PathMap::new(shared.roots.root());
    for target in targets {
        map.add(&target.canonical, &target.lexical);
    }
    *shared.map.write().unwrap_or_else(|e| e.into_inner()) = map;
}

fn apply_links(shared: &Shared, watches: &Weak<Mutex<Watches>>, ops: &mpsc::Receiver<LinkOp>) {
    loop {
        // `Roots` keeps a sender alive, so a dropped set is noticed by polling.
        let op = match ops.recv_timeout(Duration::from_secs(1)) {
            Ok(op) => op,
            Err(mpsc::RecvTimeoutError::Timeout) if watches.strong_count() > 0 => continue,
            Err(_) => return,
        };
        let Some(watches) = watches.upgrade() else {
            return;
        };
        match op {
            LinkOp::Changed(lexical) => {
                let link = shared.roots.register_link(&lexical);
                let unchanged = link.as_ref().is_some_and(|link| {
                    let w = watches.lock().unwrap_or_else(|e| e.into_inner());
                    w.targets
                        .iter()
                        .any(|t| t.lexical == lexical && t.canonical == link.target)
                });
                if unchanged {
                    continue;
                }
                forget_targets(shared, &watches, &lexical);
                if let Some(link) = link {
                    add_target(shared, &watches, &link.target, &lexical);
                }
            }
            LinkOp::Removed(lexical) => {
                shared.roots.unregister_links(&lexical);
                forget_targets(shared, &watches, &lexical);
            }
        }
    }
}

fn handle_batch(
    shared: &Shared,
    events: &[DebouncedEvent],
    links: &Sender<LinkOp>,
    on_events: &impl Fn(Vec<FsEvent>),
) {
    let paths: BTreeSet<&Path> = events
        .iter()
        .filter(|e| e.kind == DebouncedEventKind::Any)
        .map(|e| e.path.as_path())
        .collect();
    let mut found = Vec::new();
    // Folders (or links) that vanished, with the canonical path they had.
    let mut vanished = Vec::new();
    {
        let map = shared.map.read().unwrap_or_else(|e| e.into_inner());
        for path in paths {
            let rels = map.lexical(path);
            if rels.is_empty() {
                continue;
            }
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_symlink() => {
                    for rel in &rels {
                        let _ = links.send(LinkOp::Changed(rel.into()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    for rel in &rels {
                        let _ = links.send(LinkOp::Removed(rel.into()));
                        vanished.push((rel.clone(), canonical_of(path)));
                    }
                }
                _ => {}
            }
            found.extend(classify(path, &map));
        }
    }

    let mut known = shared.known.lock().unwrap_or_else(|e| e.into_inner());
    let mut sigs = shared.sigs.lock().unwrap_or_else(|e| e.into_inner());
    // Decided once per file, since every alias of it arrives in the same batch.
    let mut edited: HashMap<PathBuf, bool> = HashMap::new();
    let mut out = Vec::new();
    for event in found {
        let fresh = match event.kind {
            FsKind::Changed => {
                known.insert(event.rel.clone());
                *edited.entry(event.canonical.clone()).or_insert_with(|| {
                    match FileSig::of(&event.canonical) {
                        Some(sig) => sigs.insert(event.canonical.clone(), sig) != Some(sig),
                        None => true,
                    }
                })
            }
            // Only docs once seen: editor locks and temp files come and go unseen.
            FsKind::Removed => {
                sigs.remove(&event.canonical);
                known.remove(&event.rel)
            }
        };
        if fresh {
            out.push(event);
        }
    }
    for (rel, canonical) in vanished {
        let prefix = format!("{rel}/");
        let under: Vec<String> = known
            .iter()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for doc in under {
            known.remove(&doc);
            out.push(FsEvent {
                canonical: canonical.join(&doc[prefix.len()..]),
                rel: doc,
                kind: FsKind::Removed,
            });
        }
    }
    drop((known, sigs));
    if !out.is_empty() {
        on_events(out);
    }
}

fn watch_path(
    debouncer: &mut Debouncer<notify::RecommendedWatcher>,
    path: &Path,
    mode: RecursiveMode,
) {
    if let Err(e) = debouncer.watcher().watch(path, mode) {
        eprintln!("birta: warning: watch failed for {}: {e}", path.display());
        eprintln!("birta: hint: on Linux, try increasing fs.inotify.max_user_watches");
    }
}

/// Walks the served root, then watches it and every allowed link target, broadcasting
/// renders of changed docs and `gone` for removed ones.
pub(crate) fn watch(state: Arc<AppState>) -> anyhow::Result<WatchSet> {
    let roots = Arc::clone(&state.roots);
    let found = scan::walk(roots.root(), &roots, ScanBudget::default());
    if found.truncated {
        eprintln!("birta: scan limited; some files under the served folder were skipped");
    }
    let rt = tokio::runtime::Handle::current();
    let mut set = WatchSet::new(Arc::clone(&roots), move |events| {
        publish(&state, &rt, events)
    })?;
    for link in roots.links() {
        set.add_root(&link.target, &link.lexical);
    }
    set.seed(&found.md_files);
    Ok(set)
}

fn publish(state: &AppState, rt: &tokio::runtime::Handle, events: Vec<FsEvent>) {
    let syntax_theme = rt.block_on(async {
        let reg = state.registry.read().await;
        reg.active().active_data().syntax.clone()
    });
    // Each file is read once, however many aliases it has.
    let mut docs: HashMap<PathBuf, Option<(String, Rev)>> = HashMap::new();
    for event in events {
        if event.kind == FsKind::Removed {
            let _ = state.hub.send(WsEvent::Gone(event.rel));
            continue;
        }
        let doc = docs
            .entry(event.canonical.clone())
            .or_insert_with(|| read_doc(&event.canonical, &event.rel));
        let Some((markdown, rev)) = doc else {
            continue;
        };
        let html = render::render_dir(markdown, syntax_theme.as_ref(), Path::new(&event.rel));
        let source = render::render_source(markdown, syntax_theme.as_ref());
        let file_stats = render::format_file_stats(markdown);
        rt.block_on(state.publish_content(ContentUpdate {
            relpath: event.rel,
            rendered_html: html,
            source_html: source,
            file_stats,
            rev: rev.clone(),
        }));
    }
}

fn read_doc(canonical: &Path, rel: &str) -> Option<(String, Rev)> {
    let bytes = match std::fs::read(canonical) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("birta: could not read {rel}: {e}");
            return None;
        }
    };
    // The rev must name exactly the bytes rendered.
    let rev = Rev::of(&bytes);
    match String::from_utf8(bytes) {
        Ok(markdown) => Some((markdown, rev)),
        Err(e) => {
            eprintln!("birta: could not read {rel}: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::paths::Roots;

    /// `<tmp>/root` is served; `<tmp>/outside` is not.
    struct Tree {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
    }

    impl Tree {
        fn new(files: &[&str]) -> Self {
            Self::new_in(tempfile::tempdir().unwrap(), files)
        }

        fn new_in(tmp: tempfile::TempDir, files: &[&str]) -> Self {
            let base = tmp.path().canonicalize().unwrap();
            let root = base.join("root");
            std::fs::create_dir(&root).unwrap();
            let tree = Self {
                _tmp: tmp,
                base,
                root,
            };
            for file in files {
                tree.write(file, "x\n");
            }
            tree
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn at(&self, rel: &str) -> PathBuf {
            self.base.join(rel)
        }
    }

    fn sorted(mut names: Vec<String>) -> Vec<String> {
        names.sort();
        names
    }

    fn strs(names: &[&str]) -> Vec<String> {
        sorted(names.iter().map(|n| n.to_string()).collect())
    }

    fn event(rel: &str, canonical: PathBuf, kind: FsKind) -> FsEvent {
        FsEvent {
            rel: rel.to_string(),
            canonical,
            kind,
        }
    }

    #[test]
    fn path_map_maps_the_root_and_linked_targets() {
        let tree = Tree::new(&[]);
        let mut map = PathMap::new(&tree.root);
        map.add(&tree.at("outside"), Path::new("linked"));
        map.add(&tree.at("o2"), Path::new("linked/l2"));

        let rows: &[(&str, &[&str])] = &[
            ("root/a.md", &["a.md"]),
            ("root/sub/b.md", &["sub/b.md"]),
            ("outside/x.md", &["linked/x.md"]),
            ("outside/sub/y.md", &["linked/sub/y.md"]),
            ("o2/z.md", &["linked/l2/z.md"]),
            ("elsewhere/a.md", &[]),
        ];
        let mut bad = Vec::new();
        for (path, want) in rows {
            let got = sorted(map.lexical(&tree.at(path)));
            if got != strs(want) {
                bad.push(format!("{path}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn path_map_returns_every_alias() {
        let tree = Tree::new(&["root/dir/x.md"]);
        let mut map = PathMap::new(&tree.root);
        map.add(&tree.at("outside"), Path::new("one"));
        map.add(&tree.at("outside"), Path::new("two"));
        map.add(&tree.at("outside/sub"), Path::new("deep"));
        map.add(&tree.at("root/dir"), Path::new("inner"));

        assert_eq!(
            sorted(map.lexical(&tree.at("outside/x.md"))),
            strs(&["one/x.md", "two/x.md"])
        );
        assert_eq!(
            sorted(map.lexical(&tree.at("outside/sub/y.md"))),
            strs(&["deep/y.md", "one/sub/y.md", "two/sub/y.md"])
        );
        assert_eq!(
            sorted(map.lexical(&tree.at("root/dir/x.md"))),
            strs(&["dir/x.md", "inner/x.md"])
        );
    }

    #[test]
    fn path_map_is_component_wise() {
        let tree = Tree::new(&[]);
        let mut map = PathMap::new(&tree.root);
        map.add(&tree.at("outside"), Path::new("linked"));

        assert!(map.lexical(&tree.at("rootx/a.md")).is_empty());
        assert!(map.lexical(&tree.at("outsider/a.md")).is_empty());
    }

    #[test]
    fn path_map_file_entry_maps_only_that_file() {
        let tree = Tree::new(&[]);
        let mut map = PathMap::new(&tree.root);
        map.add(&tree.at("outside/doc.md"), Path::new("ext.md"));

        assert_eq!(map.lexical(&tree.at("outside/doc.md")), strs(&["ext.md"]));
        assert!(map.lexical(&tree.at("outside/other.md")).is_empty());
        assert!(map.lexical(&tree.at("outside/doc.md.bak")).is_empty());
    }

    /// On macOS `/tmp` is a link to `/private/tmp`; on Linux both rows are trivially canonical.
    #[test]
    fn path_map_handles_tmp_and_private_tmp() {
        let tmp = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        let lexical_root = tmp.path().join("root");
        std::fs::create_dir(&lexical_root).unwrap();
        std::fs::write(lexical_root.join("a.md"), "x\n").unwrap();
        let canonical_root = lexical_root.canonicalize().unwrap();
        let map = PathMap::new(&lexical_root);

        let rows: &[(PathBuf, &[&str])] = &[
            (canonical_root.join("a.md"), &["a.md"]),
            (lexical_root.join("a.md"), &["a.md"]),
            // Deleted: only the parent can still be canonicalized.
            (canonical_root.join("gone.md"), &["gone.md"]),
            (lexical_root.join("gone.md"), &["gone.md"]),
        ];
        let mut bad = Vec::new();
        for (path, want) in rows {
            let got = sorted(map.lexical(path));
            if got != strs(want) {
                bad.push(format!("{path:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn classify_by_what_is_on_disk() {
        let tree = Tree::new(&[
            "root/a.md",
            "root/B.MD",
            "root/notes.txt",
            "root/node_modules/x.md",
            "root/sub/.git/x.md",
            "root/.context/plan.md",
            "outside/x.md",
        ]);
        std::fs::create_dir(tree.at("root/folder.md")).unwrap();
        for file in [
            "root/a.md.birta.jsonl",
            "root/.birta.jsonl",
            "root/.doc.md.swp",
            "root/doc.md~",
            "root/doc.md___jb_tmp___",
        ] {
            tree.write(file, "x\n");
        }
        // An Emacs lock file is a dangling symlink.
        std::os::unix::fs::symlink("user@host.1234", tree.at("root/.#doc.md")).unwrap();
        let mut map = PathMap::new(&tree.root);
        map.add(&tree.at("outside"), Path::new("one"));
        map.add(&tree.at("outside"), Path::new("two"));

        use FsKind::*;
        let rows: Vec<(&str, Vec<FsEvent>)> = vec![
            (
                "root/a.md",
                vec![event("a.md", tree.at("root/a.md"), Changed)],
            ),
            (
                "root/B.MD",
                vec![event("B.MD", tree.at("root/B.MD"), Changed)],
            ),
            (
                "root/.context/plan.md",
                vec![event(
                    ".context/plan.md",
                    tree.at("root/.context/plan.md"),
                    Changed,
                )],
            ),
            (
                "root/gone.md",
                vec![event("gone.md", tree.at("root/gone.md"), Removed)],
            ),
            (
                "outside/x.md",
                vec![
                    event("one/x.md", tree.at("outside/x.md"), Changed),
                    event("two/x.md", tree.at("outside/x.md"), Changed),
                ],
            ),
            (
                "outside/gone.md",
                vec![
                    event("one/gone.md", tree.at("outside/gone.md"), Removed),
                    event("two/gone.md", tree.at("outside/gone.md"), Removed),
                ],
            ),
            ("root/notes.txt", vec![]),
            ("root/a.md.birta.jsonl", vec![]),
            ("root/.birta.jsonl", vec![]),
            ("root/.doc.md.swp", vec![]),
            ("root/doc.md~", vec![]),
            ("root/doc.md___jb_tmp___", vec![]),
            ("root/.#doc.md", vec![]),
            ("root/folder.md", vec![]),
            ("root/node_modules/x.md", vec![]),
            ("root/sub/.git/x.md", vec![]),
            ("elsewhere/x.md", vec![]),
        ];
        let mut bad = Vec::new();
        for (path, mut want) in rows {
            let mut got = classify(&tree.at(path), &map);
            got.sort_by(|a, b| a.rel.cmp(&b.rel));
            want.sort_by(|a, b| a.rel.cmp(&b.rel));
            if got != want {
                bad.push(format!("{path}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// A watch set over `<tmp>/root` whose events land on a channel.
    fn watch_set(tree: &Tree) -> (WatchSet, mpsc::Receiver<FsEvent>) {
        let (set, rx, _) = watch_set_with_roots(tree);
        (set, rx)
    }

    fn watch_set_with_roots(tree: &Tree) -> (WatchSet, mpsc::Receiver<FsEvent>, Arc<Roots>) {
        let roots = Arc::new(Roots::new(tree.root.clone(), Some(tree.at("home"))));
        let (tx, rx) = mpsc::channel();
        let set = WatchSet::new(Arc::clone(&roots), move |events| {
            for event in events {
                let _ = tx.send(event);
            }
        })
        .unwrap();
        (set, rx, roots)
    }

    /// Rewrites `path` until events for every rel in `want` arrive, or 10 s pass.
    /// Returns every rel seen, including repeats.
    fn events_after_write(
        tree: &Tree,
        path: &str,
        rx: &mpsc::Receiver<FsEvent>,
        want: &[&str],
    ) -> Vec<String> {
        let started = Instant::now();
        let mut seen: Vec<String> = Vec::new();
        let mut n = 0;
        while started.elapsed() < Duration::from_secs(10) {
            tree.write(path, &format!("edit {n}\n"));
            n += 1;
            let round = Instant::now();
            while round.elapsed() < Duration::from_secs(2) {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => seen.push(event.rel),
                    Err(_) if want.iter().all(|w| seen.iter().any(|s| s == w)) => {
                        return sorted(seen);
                    }
                    Err(_) => {}
                }
            }
        }
        sorted(seen)
    }

    /// [`events_after_write`] without repeats.
    fn rels_after_write(
        tree: &Tree,
        path: &str,
        rx: &mpsc::Receiver<FsEvent>,
        want: &[&str],
    ) -> Vec<String> {
        let mut rels = events_after_write(tree, path, rx, want);
        rels.dedup();
        rels
    }

    #[test]
    fn watch_set_starts_with_the_root() {
        let tree = Tree::new(&["root/a.md"]);
        let (set, rx) = watch_set(&tree);

        assert_eq!(set.watched(), std::slice::from_ref(&tree.root));
        assert_eq!(
            rels_after_write(&tree, "root/a.md", &rx, &["a.md"]),
            strs(&["a.md"])
        );
    }

    #[test]
    fn watch_set_skips_covered_targets_but_maps_their_aliases() {
        let tree = Tree::new(&["root/dir/x.md", "outside/sub/y.md"]);
        let (mut set, rx) = watch_set(&tree);

        assert!(set.add_root(&tree.at("outside"), Path::new("linked")));
        assert!(!set.add_root(&tree.at("outside"), Path::new("again")));
        assert!(!set.add_root(&tree.at("outside/sub"), Path::new("deep")));
        assert!(!set.add_root(&tree.at("root/dir"), Path::new("inner")));
        assert_eq!(set.watched(), [tree.root.clone(), tree.at("outside")]);

        let want = ["again/sub/y.md", "deep/y.md", "linked/sub/y.md"];
        assert_eq!(
            rels_after_write(&tree, "outside/sub/y.md", &rx, &want),
            strs(&want)
        );
    }

    #[test]
    fn watch_set_ancestor_added_after_descendant_is_watched_once() {
        let tree = Tree::new(&["root/a.md", "outside/sub/y.md"]);
        let (mut set, rx) = watch_set(&tree);

        assert!(set.add_root(&tree.at("outside/sub"), Path::new("deep")));
        assert!(set.add_root(&tree.at("outside"), Path::new("linked")));
        assert_eq!(set.watched(), [tree.root.clone(), tree.at("outside")]);

        tree.write("outside/sub/y.md", "edit\n");
        let started = Instant::now();
        let mut seen = Vec::new();
        // Collect until both arrive, then keep draining so a second watch's copy would show.
        while started.elapsed() < Duration::from_secs(10) {
            let both = ["deep/y.md", "linked/sub/y.md"]
                .iter()
                .all(|w| seen.iter().any(|s| s == w));
            let wait = if both { 1000 } else { 100 };
            match rx.recv_timeout(Duration::from_millis(wait)) {
                Ok(event) => seen.push(event.rel),
                Err(_) if both => break,
                Err(_) => {}
            }
        }
        assert_eq!(sorted(seen), strs(&["deep/y.md", "linked/sub/y.md"]));
    }

    #[test]
    fn watch_set_file_target_delivers_only_that_file() {
        let tree = Tree::new(&[
            "root/a.md",
            "outside/doc.md",
            "outside/other.md",
            "outside/sub/deep.md",
        ]);
        let (mut set, rx) = watch_set(&tree);

        assert!(set.add_root(&tree.at("outside/doc.md"), Path::new("ext.md")));
        assert_eq!(
            rels_after_write(&tree, "outside/doc.md", &rx, &["ext.md"]),
            strs(&["ext.md"])
        );

        tree.write("outside/other.md", "edit\n");
        tree.write("outside/sub/deep.md", "edit\n");
        // The sentinel shares the siblings' folder, so their events come no later than it.
        let seen = rels_after_write(&tree, "outside/doc.md", &rx, &["ext.md"]);
        assert_eq!(seen, strs(&["ext.md"]));
    }

    #[test]
    fn link_to_ancestor_or_root_created_after_start_is_not_watched() {
        let tree = Tree::new(&["root/a.md", "outside/x.md"]);
        let (set, rx) = watch_set(&tree);

        std::os::unix::fs::symlink("..", tree.at("root/up")).unwrap();
        std::os::unix::fs::symlink("/", tree.at("root/slash")).unwrap();
        std::os::unix::fs::symlink("../outside", tree.at("root/ok")).unwrap();
        // `ok/x.md` arriving proves the links above were handled.
        let seen = rels_after_write(&tree, "outside/x.md", &rx, &["ok/x.md"]);

        assert_eq!(seen, strs(&["ok/x.md"]));
        assert_eq!(set.watched(), [tree.root.clone(), tree.at("outside")]);
    }

    #[test]
    fn link_first_registered_by_a_request_is_watched() {
        let tree = Tree::new(&["root/a.md", "outside/x.md"]);
        // Made before the watch set, so no event announces it; only the request does.
        std::os::unix::fs::symlink("../outside", tree.at("root/late")).unwrap();
        let (_set, rx, roots) = watch_set_with_roots(&tree);

        assert!(
            roots
                .resolve_file("late/x.md", crate::paths::FileKind::Markdown)
                .is_ok()
        );
        let seen = rels_after_write(&tree, "outside/x.md", &rx, &["late/x.md"]);

        assert_eq!(seen, strs(&["late/x.md"]));
    }

    #[test]
    fn reads_are_not_changes() {
        let tree = Tree::new(&["root/a.md", "root/b.md"]);
        let (set, rx) = watch_set(&tree);
        // As `watch` does after its walk; an unseen file's first event can't be judged.
        set.seed(&["a.md".to_string(), "b.md".to_string()]);

        std::fs::read(tree.at("root/a.md")).unwrap();
        // The sentinel shares the folder, so a read's event comes no later than it.
        let seen = rels_after_write(&tree, "root/b.md", &rx, &["b.md"]);

        assert_eq!(seen, strs(&["b.md"]));
    }

    #[test]
    fn watch_set_caps_roots() {
        let tree = Tree::new(&[]);
        let (mut set, rx) = watch_set(&tree);

        let mut added = 0;
        for i in 0..MAX_ROOTS + 6 {
            let dir = tree.at(&format!("t{i}"));
            std::fs::create_dir(&dir).unwrap();
            if set.add_root(&dir, Path::new(&format!("l{i}"))) {
                added += 1;
            }
        }

        assert_eq!(set.watched().len(), MAX_ROOTS);
        assert_eq!(added, MAX_ROOTS - 1);
        // A target an existing watch covers still maps once the cap is reached.
        assert!(!set.add_root(&tree.at("t0/sub"), Path::new("late")));
        tree.write("t0/sub/x.md", "x\n");
        let seen = rels_after_write(&tree, "t0/sub/x.md", &rx, &["late/x.md"]);
        assert_eq!(seen, strs(&["l0/sub/x.md", "late/x.md"]));
    }
}
