//! The single choke point from a request path to a file under the served roots.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use axum::http::StatusCode;

/// Directories never served or written, at any depth (ASCII case-insensitive).
pub(crate) const DENY_DIRS: [&str; 7] = [".git", ".hg", ".svn", ".ssh", ".gnupg", ".aws", ".kube"];

/// The only extensions `/local` serves (ASCII case-insensitive).
pub(crate) const IMAGE_EXTS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "avif", "bmp",
];

const REVIEW_SUFFIX: &str = ".birta.jsonl";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    Markdown,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathError {
    /// 400: the name is not the requested kind.
    Invalid,
    /// 403: the path is outside the roots or names something never served.
    Denied,
    /// 404
    NotFound,
}

impl PathError {
    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Denied => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
        }
    }
}

/// A markdown document resolved for reading or writing.
#[derive(Debug)]
pub(crate) struct DocRef {
    /// Lexical path as requested, `/`-separated.
    pub(crate) rel: String,
    pub(crate) canonical: PathBuf,
    /// `<canonical>.birta.jsonl`
    pub(crate) review_path: PathBuf,
    pub(crate) writable: bool,
    /// Whether a review file may be created beside the canonical doc.
    pub(crate) log_writable: bool,
}

/// A symlink under the root that was allowed to reach outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Link {
    /// Where the link sits, relative to the root.
    pub(crate) lexical: PathBuf,
    pub(crate) target: PathBuf,
    /// Folder targets are roots; file targets are single read-only files.
    pub(crate) dir: bool,
}

/// The served root plus the targets of symlinks found inside it.
pub(crate) struct Roots {
    root: PathBuf,
    home: Option<PathBuf>,
    links: RwLock<Vec<Link>>,
    /// Told the lexical path of each link a request registers, so it gets watched.
    observer: OnceLock<LinkObserver>,
}

type LinkObserver = Box<dyn Fn(&Path) + Send + Sync>;

/// Where a canonical path was found: a folder root, or an allowed file's folder.
struct Base {
    dir: PathBuf,
    folder_root: bool,
}

impl Roots {
    /// Symlinks to `home` or any folder above it are never followed.
    pub(crate) fn new(root: PathBuf, home: Option<PathBuf>) -> Self {
        // Canonical, so on-disk case and resolved links match what `canonicalize` returns.
        let canonical = |p: PathBuf| p.canonicalize().unwrap_or(p);
        Self {
            root: canonical(root),
            home: home.map(canonical),
            links: RwLock::new(Vec::new()),
            observer: OnceLock::new(),
        }
    }

    /// Sets the one observer of links first found by a request.
    pub(crate) fn observe_links(&self, observer: impl Fn(&Path) + Send + Sync + 'static) {
        let _ = self.observer.set(Box::new(observer));
    }

    /// The canonical served root.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Checks, in order: plain names, deny set and dot rules, extension, existence,
    /// containment (registering symlinked folders on the way), then the same name
    /// rules on the canonical path.
    pub(crate) fn resolve_file(&self, rel: &str, kind: FileKind) -> Result<PathBuf, PathError> {
        let lexical = lexical_rel(rel)?;
        check_names(&lexical, kind)?;
        check_extension(&lexical, kind)?;
        let canonical = self
            .root
            .join(&lexical)
            .canonicalize()
            .map_err(|_| PathError::NotFound)?;
        let base = match self.base_of(&canonical) {
            Some(base) => base,
            None => {
                self.register_links(&lexical);
                self.base_of(&canonical).ok_or(PathError::Denied)?
            }
        };
        // A link inside the root must not reach a name the lexical rules refuse.
        let inner = canonical
            .strip_prefix(&base.dir)
            .map_err(|_| PathError::Denied)?;
        check_names(inner, kind)?;
        check_extension(inner, kind)?;
        if !canonical.is_file() {
            return Err(PathError::NotFound);
        }
        Ok(canonical)
    }

    pub(crate) fn resolve_doc(&self, rel: &str, access: Access) -> Result<DocRef, PathError> {
        let canonical = self.resolve_file(rel, FileKind::Markdown)?;
        let mut review_path = OsString::from(canonical.as_os_str());
        review_path.push(REVIEW_SUFFIX);
        // A file reached only through a file link is read-only, and so is its review.
        let folder_root = self.base_of(&canonical).is_some_and(|b| b.folder_root);
        Ok(DocRef {
            rel: rel.to_string(),
            writable: access == Access::Write && folder_root && is_writable(&canonical),
            log_writable: folder_root,
            review_path: review_path.into(),
            canonical,
        })
    }

    /// Whether `canonical` sits in a folder root, where its review file may live.
    pub(crate) fn folder_root(&self, canonical: &Path) -> bool {
        self.base_of(canonical).is_some_and(|b| b.folder_root)
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the directory walker checks containment")
    )]
    pub(crate) fn contains(&self, canonical: &Path) -> bool {
        self.base_of(canonical).is_some()
    }

    /// Every registered link.
    pub(crate) fn links(&self) -> Vec<Link> {
        self.links.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Registers the symlink at `lexical` when its target is allowed: a folder, or a
    /// markdown or image file. Replaces whatever that link pointed at before.
    pub(crate) fn register_link(&self, lexical: &Path) -> Option<Link> {
        let path = self.root.join(lexical);
        let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
        let target = path.canonicalize().ok().filter(|_| is_link);
        let link = target.and_then(|target| {
            let dir = target.is_dir();
            let allowed = if dir {
                self.link_allowed(&target)
            } else {
                file_link_allowed(&target)
            };
            allowed.then(|| Link {
                lexical: lexical.to_path_buf(),
                target,
                dir,
            })
        });
        let mut links = self.links.write().unwrap_or_else(|e| e.into_inner());
        links.retain(|l| l.lexical != lexical);
        links.extend(link.clone());
        link
    }

    /// Registers every allowed symlink along `lexical`, file links included. For a path the
    /// user named, such as the entry doc, which the budgeted walk may never reach.
    pub(crate) fn register_path(&self, lexical: &Path) {
        let mut rel = PathBuf::new();
        for component in lexical.components() {
            rel.push(component);
            let path = self.root.join(&rel);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                self.register_link(&rel);
            }
        }
    }

    /// Forgets every link at or below `lexical`, so a removed or moved link stops serving
    /// its old target. Returns what was forgotten.
    pub(crate) fn unregister_links(&self, lexical: &Path) -> Vec<Link> {
        let mut links = self.links.write().unwrap_or_else(|e| e.into_inner());
        let (gone, kept) = links
            .drain(..)
            .partition(|l| l.lexical.starts_with(lexical));
        *links = kept;
        gone
    }

    /// The folder root holding `canonical`, preferring the served root, else the folder of
    /// an allowed file link to exactly `canonical`.
    fn base_of(&self, canonical: &Path) -> Option<Base> {
        if canonical.starts_with(&self.root) {
            return Some(Base {
                dir: self.root.clone(),
                folder_root: true,
            });
        }
        let links = self.links.read().unwrap_or_else(|e| e.into_inner());
        if let Some(link) = links
            .iter()
            .find(|l| l.dir && canonical.starts_with(&l.target))
        {
            return Some(Base {
                dir: link.target.clone(),
                folder_root: true,
            });
        }
        links
            .iter()
            .find(|l| !l.dir && l.target == canonical)
            .and_then(|l| l.target.parent())
            .map(|dir| Base {
                dir: dir.to_path_buf(),
                folder_root: false,
            })
    }

    /// Registers every allowed folder symlink along `lexical`. Only links that already
    /// exist inside the root are reachable, so this trusts nothing a scan wouldn't.
    /// File links are left to the walker and the watcher.
    fn register_links(&self, lexical: &Path) {
        let mut rel = PathBuf::new();
        for component in lexical.components() {
            rel.push(component);
            let path = self.root.join(&rel);
            let is_link =
                std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            if is_link
                && path.is_dir()
                && self.register_link(&rel).is_some()
                && let Some(observer) = self.observer.get()
            {
                observer(&rel);
            }
        }
    }

    fn link_allowed(&self, target: &Path) -> bool {
        target.is_dir()
            && target.parent().is_some()
            && !self
                .home
                .as_deref()
                .is_some_and(|home| home.starts_with(target))
            && !self.root.starts_with(target)
            && !target.components().any(|c| is_denied_name(c.as_os_str()))
    }
}

/// A file link may reach a markdown or image file whose own names pass the name rules.
fn file_link_allowed(target: &Path) -> bool {
    let kind = if is_md(target) {
        FileKind::Markdown
    } else if is_image(target) {
        FileKind::Image
    } else {
        return false;
    };
    let Some(parent) = target.parent() else {
        return false;
    };
    target.is_file()
        && !parent.components().any(|c| is_denied_name(c.as_os_str()))
        && target
            .file_name()
            .is_some_and(|leaf| check_names(Path::new(leaf), kind).is_ok())
}

/// `rel` as a relative path of plain names only; anything else is [`PathError::Denied`].
pub(crate) fn lexical_rel(rel: &str) -> Result<PathBuf, PathError> {
    let plain = |seg: &str| !seg.is_empty() && seg != "." && seg != "..";
    if rel.contains('\0') || !rel.split('/').all(plain) {
        return Err(PathError::Denied);
    }
    let path = PathBuf::from(rel);
    if path.components().all(|c| matches!(c, Component::Normal(_))) {
        Ok(path)
    } else {
        Err(PathError::Denied)
    }
}

pub(crate) fn is_md(path: &Path) -> bool {
    has_extension(path, &["md", "markdown"])
}

pub(crate) fn is_image(path: &Path) -> bool {
    has_extension(path, &IMAGE_EXTS)
}

pub(crate) fn is_review_file(name: &str) -> bool {
    name.len() >= REVIEW_SUFFIX.len()
        && name.as_bytes()[name.len() - REVIEW_SUFFIX.len()..]
            .eq_ignore_ascii_case(REVIEW_SUFFIX.as_bytes())
}

/// Whether the file can be opened for writing; the mode bits alone miss ownership.
pub(crate) fn is_writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn has_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub(crate) fn is_denied_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| DENY_DIRS.iter().any(|d| d.eq_ignore_ascii_case(n)))
}

/// The deny set applies to every component; review files and (for images) dotfiles
/// are refused by leaf name.
fn check_names(path: &Path, kind: FileKind) -> Result<(), PathError> {
    if path.components().any(|c| is_denied_name(c.as_os_str())) {
        return Err(PathError::Denied);
    }
    let leaf = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if is_review_file(leaf) || (kind == FileKind::Image && leaf.starts_with('.')) {
        return Err(PathError::Denied);
    }
    Ok(())
}

fn check_extension(path: &Path, kind: FileKind) -> Result<(), PathError> {
    match kind {
        FileKind::Markdown if !is_md(path) => Err(PathError::Invalid),
        FileKind::Image if !is_image(path) => Err(PathError::NotFound),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, Instant};

    use super::*;

    /// `<tmp>/root` is served; `<tmp>/outside`, `<tmp>/home` and `<tmp>/.ssh` are not.
    struct Tree {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        root: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let root = base.join("root");
            std::fs::create_dir(&root).unwrap();
            let tree = Self {
                _tmp: tmp,
                base,
                root,
            };
            for file in [
                "root/a.png",
                "root/b.PNG",
                "root/c.Svg",
                "root/notes.txt",
                "root/.env",
                "root/x.birta.jsonl",
                "root/y.BIRTA.JSONL",
                "root/.birta.jsonl",
                "root/doc.md",
                "root/doc.md.birta.jsonl",
                "root/upper.MD",
                "root/long.markdown",
                "root/dir/.git/config",
                "root/dir/.git/a.png",
                "root/dir2/.GIT/a.png",
                "root/.git/x.md",
                "root/.hg/a.png",
                "root/.svn/a.png",
                "root/.ssh/a.png",
                "root/.gnupg/a.png",
                "root/.aws/a.png",
                "root/.kube/a.png",
                "root/.context/x.md",
                "root/.context/img/a.png",
                "root/.context/img/.cache/a.png",
                "root/.context/.env",
                "root/x/.secret.png",
                "root/folder.md/inner.md",
                "outside/a.png",
                "outside/doc.md",
                "outside/sub/deep.png",
                "secret.md",
                "home/a.png",
                ".ssh/a.png",
                ".git/sub/a.png",
                "o2/a.png",
                "home/proj/a.png",
                "rootx/a.png",
                ".Kube/a.png",
            ] {
                tree.write(file, "- [ ] x\n");
            }
            std::fs::create_dir(tree.base.join("o1")).unwrap();
            tree
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// Creates `<base>/<link>` pointing at `target` (absolute, or relative to the link).
        fn link(&self, link: &str, target: impl AsRef<Path>) {
            symlink(target, self.base.join(link)).unwrap();
        }

        fn roots(&self) -> Roots {
            Roots::new(self.root.clone(), Some(self.base.join("home")))
        }
    }

    type Row<'a> = (&'a str, Result<&'a str, PathError>);

    /// Resolves each row; returns every mismatch. `Ok` holds the path relative to `<tmp>`.
    fn file_mismatches(tree: &Tree, roots: &Roots, kind: FileKind, rows: &[Row]) -> Vec<String> {
        let mut bad = Vec::new();
        for (rel, want) in rows {
            let got = roots.resolve_file(rel, kind);
            let want = want.map(|w| tree.base.join(w));
            if got != want {
                bad.push(format!("{kind:?} {rel:?}: got {got:?}, want {want:?}"));
            }
        }
        bad
    }

    #[test]
    fn lexical_rel_allows_only_plain_names() {
        for ok in ["a.md", "a/b.md", ".context/x.md", "a b/c.png", "..a/b..png"] {
            assert_eq!(lexical_rel(ok), Ok(PathBuf::from(ok)), "{ok:?}");
        }
        for bad in [
            "",
            ".",
            "..",
            "/etc/passwd",
            "../x.png",
            "a/../x.png",
            "a/..",
            "./a.png",
            "a/./b.png",
            "a\0b.png",
        ] {
            assert_eq!(lexical_rel(bad), Err(PathError::Denied), "{bad:?}");
        }
    }

    #[test]
    fn path_error_statuses() {
        assert_eq!(PathError::Invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(PathError::Denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(PathError::NotFound.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn name_predicates_ignore_ascii_case() {
        for md in ["a.md", "a.MD", "a.Markdown", "dir/a.markdown"] {
            assert!(is_md(Path::new(md)), "{md}");
        }
        for not in ["a.mdx", "a.txt", "md", "a.md.png", "a"] {
            assert!(!is_md(Path::new(not)), "{not}");
        }
        for review in [
            "x.birta.jsonl",
            "X.BIRTA.JSONL",
            ".birta.jsonl",
            "doc.md.Birta.Jsonl",
        ] {
            assert!(is_review_file(review), "{review}");
        }
        for not in [
            "x.jsonl",
            "birta.jsonl",
            "x.birta.jsonl.png",
            "x.birta.json",
        ] {
            assert!(!is_review_file(not), "{not}");
        }
    }

    #[test]
    fn image_rules() {
        let tree = Tree::new();
        let roots = tree.roots();
        use PathError::*;
        let rows: &[Row] = &[
            ("a.png", Ok("root/a.png")),
            ("b.PNG", Ok("root/b.PNG")),
            ("c.Svg", Ok("root/c.Svg")),
            ("notes.txt", Err(NotFound)),
            ("doc.md", Err(NotFound)),
            ("missing.png", Err(NotFound)),
            (".env", Err(Denied)),
            ("x.birta.jsonl", Err(Denied)),
            ("y.BIRTA.JSONL", Err(Denied)),
            (".birta.jsonl", Err(Denied)),
            ("doc.md.birta.jsonl", Err(Denied)),
            ("dir/.git/config", Err(Denied)),
            ("dir/.git/a.png", Err(Denied)),
            ("dir2/.GIT/a.png", Err(Denied)),
            (".hg/a.png", Err(Denied)),
            (".svn/a.png", Err(Denied)),
            (".ssh/a.png", Err(Denied)),
            (".gnupg/a.png", Err(Denied)),
            (".aws/a.png", Err(Denied)),
            (".kube/a.png", Err(Denied)),
            (".context/img/a.png", Ok("root/.context/img/a.png")),
            (
                ".context/img/.cache/a.png",
                Ok("root/.context/img/.cache/a.png"),
            ),
            (".context/.env", Err(Denied)),
            ("x/.secret.png", Err(Denied)),
            ("/etc/passwd", Err(Denied)),
            ("../outside/a.png", Err(Denied)),
            ("x/../a.png", Err(Denied)),
        ];
        let bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn markdown_rules() {
        let tree = Tree::new();
        let roots = tree.roots();
        use PathError::*;
        let rows: &[Row] = &[
            ("doc.md", Ok("root/doc.md")),
            ("upper.MD", Ok("root/upper.MD")),
            ("long.markdown", Ok("root/long.markdown")),
            (".context/x.md", Ok("root/.context/x.md")),
            (".git/x.md", Err(Denied)),
            ("a.png", Err(Invalid)),
            ("missing.txt", Err(Invalid)),
            ("x.birta.jsonl", Err(Denied)),
            ("missing.md", Err(NotFound)),
            ("folder.md", Err(NotFound)),
            ("../secret.md", Err(Denied)),
        ];
        let bad = file_mismatches(&tree, &roots, FileKind::Markdown, rows);
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn symlinked_dir_inside_root_is_served_even_when_created_after_start() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/linked", tree.base.join("outside"));
        tree.link("root/inner", "dir");

        let rows: &[Row] = &[
            ("linked/a.png", Ok("outside/a.png")),
            ("linked/sub/deep.png", Ok("outside/sub/deep.png")),
            ("inner/.git/a.png", Err(PathError::Denied)),
        ];
        let mut bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
        bad.extend(file_mismatches(
            &tree,
            &roots,
            FileKind::Markdown,
            &[("linked/doc.md", Ok("outside/doc.md"))],
        ));
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(roots.contains(&tree.base.join("outside/a.png")));
    }

    #[test]
    fn symlink_through_a_linked_dir_is_served() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/l1", tree.base.join("o1"));
        tree.link("o1/l2", tree.base.join("o2"));

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[("l1/l2/a.png", Ok("o2/a.png"))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn symlinks_to_guarded_targets_are_refused() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/slash", "/");
        tree.link("root/home", tree.base.join("home"));
        tree.link("root/up", &tree.base);
        tree.link("root/keys", tree.base.join(".ssh"));
        tree.link("root/gitsub", tree.base.join(".git/sub"));
        tree.link("root/pic.png", tree.base.join("outside/a.png"));
        // Through `/` and back down to a real image outside the root.
        let via_slash = format!("slash{}", tree.base.join("outside/a.png").display());

        let rows: &[Row] = &[
            (&via_slash, Err(PathError::Denied)),
            ("home/a.png", Err(PathError::Denied)),
            ("up/outside/a.png", Err(PathError::Denied)),
            ("keys/a.png", Err(PathError::Denied)),
            ("gitsub/a.png", Err(PathError::Denied)),
            ("pic.png", Err(PathError::Denied)),
        ];
        // Twice: a refused link must not be remembered as a root.
        for _ in 0..2 {
            let bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
            assert!(bad.is_empty(), "{bad:#?}");
        }
        assert!(!roots.contains(&tree.base.join("outside/a.png")));
        assert!(!roots.contains(&tree.base.join("home/a.png")));
    }

    #[test]
    fn symlink_loops_terminate() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/self", "self");
        tree.link("root/p", "q");
        tree.link("root/q", "p");
        tree.link("root/dir/back", "..");

        let started = Instant::now();
        let rows: &[Row] = &[
            ("self/a.png", Err(PathError::NotFound)),
            ("p/a.png", Err(PathError::NotFound)),
            ("dir/back/dir/back/dir/back/a.png", Ok("root/a.png")),
        ];
        let bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn resolve_doc_for_write() {
        let tree = Tree::new();
        let roots = tree.roots();

        let doc = roots.resolve_doc("doc.md", Access::Write).unwrap();
        assert_eq!(doc.rel, "doc.md");
        assert_eq!(doc.canonical, tree.root.join("doc.md"));
        assert_eq!(doc.review_path, tree.root.join("doc.md.birta.jsonl"));
        assert!(doc.writable);
        assert!(doc.log_writable);

        let nested = roots.resolve_doc(".context/x.md", Access::Write).unwrap();
        assert_eq!(nested.rel, ".context/x.md");
    }

    #[test]
    fn resolve_doc_write_follows_links_only_into_directory_roots() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/evil.md", "../secret.md");
        tree.link("root/alias.md", "doc.md");
        tree.link("root/fake.md", "notes.txt");
        tree.link("root/linked", tree.base.join("outside"));

        let alias = roots.resolve_doc("alias.md", Access::Write).unwrap();
        assert_eq!(alias.rel, "alias.md");
        assert_eq!(alias.canonical, tree.root.join("doc.md"));
        assert_eq!(alias.review_path, tree.root.join("doc.md.birta.jsonl"));
        assert!(alias.writable && alias.log_writable);

        let linked = roots.resolve_doc("linked/doc.md", Access::Write).unwrap();
        assert_eq!(linked.canonical, tree.base.join("outside/doc.md"));
        assert!(linked.writable && linked.log_writable);

        assert_eq!(
            roots.resolve_doc("evil.md", Access::Write).err(),
            Some(PathError::Denied)
        );
        assert!(roots.resolve_doc("fake.md", Access::Write).is_err());
        assert_eq!(
            roots.resolve_doc("notes.txt", Access::Write).err(),
            Some(PathError::Invalid)
        );
        assert_eq!(
            roots.resolve_doc("x.birta.jsonl", Access::Write).err(),
            Some(PathError::Denied)
        );
        assert_eq!(
            roots.resolve_doc("", Access::Write).err(),
            Some(PathError::Denied)
        );
    }

    #[test]
    fn resolve_doc_for_read_never_claims_writable() {
        let tree = Tree::new();
        let doc = tree.roots().resolve_doc("doc.md", Access::Read).unwrap();
        assert_eq!(doc.canonical, tree.root.join("doc.md"));
        assert!(!doc.writable);
    }

    #[test]
    fn resolve_doc_reports_read_only_file() {
        let tree = Tree::new();
        let roots = tree.roots();
        let file = tree.root.join("doc.md");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();

        let doc = roots.resolve_doc("doc.md", Access::Write).unwrap();

        assert!(!doc.writable);
    }

    #[test]
    fn link_targets_obey_dot_and_deny_rules() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/pic.png", ".env");
        tree.link("root/git.png", "dir/.git/a.png");
        tree.link("root/hid.png", "x/.secret.png");
        tree.link("root/jsonl.png", "x.birta.jsonl");
        tree.link("root/ok.png", ".context/img/a.png");
        tree.link("root/gitdir", "dir/.git");
        tree.link("root/g.md", ".git/x.md");
        tree.link("root/r.md", "doc.md.birta.jsonl");
        tree.link("root/c.md", ".context/x.md");
        use PathError::*;

        let images: &[Row] = &[
            ("pic.png", Err(Denied)),
            ("git.png", Err(Denied)),
            ("hid.png", Err(Denied)),
            ("jsonl.png", Err(Denied)),
            ("gitdir/a.png", Err(Denied)),
            ("ok.png", Ok("root/.context/img/a.png")),
        ];
        let docs: &[Row] = &[
            ("g.md", Err(Denied)),
            ("r.md", Err(Denied)),
            ("c.md", Ok("root/.context/x.md")),
        ];
        let mut bad = file_mismatches(&tree, &roots, FileKind::Image, images);
        bad.extend(file_mismatches(&tree, &roots, FileKind::Markdown, docs));
        for rel in ["g.md", "r.md"] {
            let got = roots.resolve_doc(rel, Access::Write).err();
            if got != Some(Denied) {
                bad.push(format!("resolve_doc {rel:?}: got {got:?}, want Denied"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn rules_are_relative_to_the_root() {
        let tree = Tree::new();
        tree.write(".aws/served/a.png", "png");
        tree.write(".aws/served/doc.md", "- [ ] x\n");
        let roots = Roots::new(tree.base.join(".aws/served"), None);

        let rows: &[Row] = &[("a.png", Ok(".aws/served/a.png"))];
        let mut bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
        let md: &[Row] = &[("doc.md", Ok(".aws/served/doc.md"))];
        bad.extend(file_mismatches(&tree, &roots, FileKind::Markdown, md));
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(roots.resolve_doc("doc.md", Access::Write).is_ok());
    }

    #[test]
    fn guard_uses_canonical_target() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("alias", ".ssh");
        tree.link("root/k", tree.base.join("alias"));

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[("k/a.png", Err(PathError::Denied))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn guard_refuses_any_ancestor() {
        let tree = Tree::new();
        let roots = tree.roots();
        let grandparent = tree.base.parent().unwrap();
        tree.link("root/gp", grandparent);
        let name = tree.base.file_name().unwrap().to_str().unwrap();
        let rel = format!("gp/{name}/outside/a.png");

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[(&rel, Err(PathError::Denied))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn contains_is_component_wise() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/p.png", tree.base.join("rootx/a.png"));

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[("p.png", Err(PathError::Denied))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(!roots.contains(&tree.base.join("rootx/a.png")));
        assert!(roots.contains(&tree.root.join("a.png")));
    }

    #[test]
    fn guard_deny_ignores_case() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/k", tree.base.join(".Kube"));

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[("k/a.png", Err(PathError::Denied))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn links_to_ancestors_of_home_are_refused() {
        let tree = Tree::new();
        tree.write("users/me/a.png", "png");
        tree.write("users/other/a.png", "png");
        let roots = Roots::new(tree.root.clone(), Some(tree.base.join("users/me")));
        tree.link("root/users", tree.base.join("users"));
        tree.link("root/me", tree.base.join("users/me"));

        let rows: &[Row] = &[
            ("users/other/a.png", Err(PathError::Denied)),
            ("users/me/a.png", Err(PathError::Denied)),
            ("me/a.png", Err(PathError::Denied)),
        ];
        let bad = file_mismatches(&tree, &roots, FileKind::Image, rows);
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn unregistered_links_stop_serving_their_targets() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/linked", tree.base.join("outside"));
        tree.link("root/ext.md", tree.base.join("outside/doc.md"));
        let ext = roots.register_link(Path::new("ext.md")).unwrap();
        assert!(!ext.dir);
        assert_eq!(
            roots.resolve_file("linked/a.png", FileKind::Image),
            Ok(tree.base.join("outside/a.png"))
        );
        assert!(roots.resolve_file("ext.md", FileKind::Markdown).is_ok());

        std::fs::remove_file(tree.root.join("linked")).unwrap();
        std::fs::remove_file(tree.root.join("ext.md")).unwrap();
        tree.link("root/ext.md", tree.base.join("outside/sub/deep.png"));
        let gone = roots.unregister_links(Path::new("linked"));
        assert_eq!(gone.len(), 1);
        roots.unregister_links(Path::new("ext.md"));

        assert!(!roots.contains(&tree.base.join("outside/a.png")));
        assert!(!roots.contains(&tree.base.join("outside/doc.md")));
        assert!(roots.links().is_empty());
    }

    #[test]
    fn register_link_replaces_a_retargeted_link() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/l", tree.base.join("outside"));
        roots.register_link(Path::new("l")).unwrap();

        std::fs::remove_file(tree.root.join("l")).unwrap();
        tree.link("root/l", tree.base.join("o2"));
        let link = roots.register_link(Path::new("l")).unwrap();

        assert_eq!(link.target, tree.base.join("o2"));
        assert!(!roots.contains(&tree.base.join("outside/a.png")));
        assert!(roots.contains(&tree.base.join("o2/a.png")));

        std::fs::remove_file(tree.root.join("l")).unwrap();
        tree.link("root/l", tree.base.join(".ssh"));
        assert_eq!(roots.register_link(Path::new("l")), None);
        assert!(!roots.contains(&tree.base.join("o2/a.png")));
    }

    #[test]
    fn link_into_a_folder_under_home_is_served() {
        let tree = Tree::new();
        let roots = tree.roots();
        tree.link("root/proj", tree.base.join("home/proj"));

        let bad = file_mismatches(
            &tree,
            &roots,
            FileKind::Image,
            &[("proj/a.png", Ok("home/proj/a.png"))],
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }
}
