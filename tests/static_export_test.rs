//! Integration tests for multi-file static bundle export.

use std::fs;
use std::path::Path;

use birta::static_export::{BundleOptions, export_bundle};
use birta::theme::{ResolvedTheme, ThemeVariants, Variant, VariantData};
use tempfile::tempdir;

fn test_theme() -> ResolvedTheme {
    ResolvedTheme {
        name: "github".to_string(),
        variants: ThemeVariants::Single(Box::new(VariantData {
            css_vars: String::new(),
            syntax: None,
        })),
        active_variant: Variant::Dark,
    }
}

fn opts<'a>(theme: &'a ResolvedTheme) -> BundleOptions<'a> {
    BundleOptions {
        theme,
        custom_css: None,
        font_css: None,
        show_header: true,
        reading_mode: false,
        raw_mode: false,
        variant_explicit: false,
        keybindings_json: "{}",
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn bundle_nested_backlink_and_image() {
    // The high-value regression cluster: nested back-link + image, mirrored tree.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "# Index\n\n[a](docs/a.md)\n");
    write(
        &base.join("docs/a.md"),
        "# A\n\n[back](../index.md)\n\n![pic](../img/x.png)\n",
    );
    write(&base.join("img/x.png"), "PNGDATA");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 2, "index + docs/a");
    assert_eq!(result.assets, 1, "img/x.png");

    // Files written at mirrored paths.
    assert!(out.path().join("index.html").is_file());
    assert!(out.path().join("docs/a.html").is_file());
    assert!(out.path().join("img/x.png").is_file());
    assert_eq!(
        fs::read_to_string(out.path().join("img/x.png")).unwrap(),
        "PNGDATA",
        "asset copied verbatim"
    );

    let index = fs::read_to_string(out.path().join("index.html")).unwrap();
    assert!(
        index.contains("href=\"docs/a.html\""),
        "index links to docs/a.html"
    );

    let a = fs::read_to_string(out.path().join("docs/a.html")).unwrap();
    assert!(
        a.contains("href=\"../index.html\""),
        "nested back-link resolves up a dir, got page: {a}"
    );
    assert!(
        a.contains("src=\"../img/x.png\""),
        "nested image resolves up a dir, got page: {a}"
    );

    // No server-mode routes leak into the static bundle.
    for page in [&index, &a] {
        assert!(!page.contains("/view/"), "no /view/ routes in bundle");
        assert!(!page.contains("/local/"), "no /local/ routes in bundle");
    }
}

#[test]
fn bundle_skips_broken_link() {
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "[gone](missing.md)\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 1, "only the entry is written");
    assert!(out.path().join("index.html").is_file());
    assert!(
        !out.path().join("missing.html").exists(),
        "broken link target is not written"
    );
}

#[test]
fn bundle_rejects_escaping_link() {
    // A real file exists one level above base_dir; an escaping link must not
    // pull it into the bundle or write outside out_dir.
    let root = tempdir().unwrap();
    write(&root.path().join("outside.md"), "# Secret\n");
    let base = root.path().join("site");
    write(&base.join("index.md"), "[esc](../outside.md)\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), &base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 1, "escaping link is skipped");
    assert!(
        !out.path().join("outside.html").exists(),
        "escaping target not written inside bundle"
    );
    // Nothing was written next to out_dir either.
    assert!(!out.path().parent().unwrap().join("outside.html").exists());
}

#[cfg(unix)]
#[test]
fn bundle_copies_file_link_images_as_the_server_serves_them() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    write(&root.path().join("outside/pic.png"), "PNGDATA");
    write(&root.path().join("secret.env"), "TOPSECRET");
    write(&root.path().join(".ssh/key.png"), "KEY");
    let base = root.path().join("site");
    fs::create_dir_all(&base).unwrap();
    // Symlinks inside the tree pointing outside it: an image, a non-image, a denied folder.
    symlink(root.path().join("outside/pic.png"), base.join("link.png")).unwrap();
    symlink(root.path().join("secret.env"), base.join("leak.png")).unwrap();
    symlink(root.path().join(".ssh/key.png"), base.join("key.png")).unwrap();
    write(
        &base.join("index.md"),
        "![a](link.png) ![b](leak.png) ![c](key.png) ![d](../outside/pic.png)\n",
    );

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), &base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(
        result.assets, 1,
        "only the image link the server would serve"
    );
    assert_eq!(fs::read(out.path().join("link.png")).unwrap(), b"PNGDATA");
    for name in ["leak.png", "key.png", "pic.png", "secret.env"] {
        assert!(!out.path().join(name).exists(), "{name}");
    }
    assert!(
        !out.path()
            .parent()
            .unwrap()
            .join("outside/pic.png")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn bundle_follows_links_inside_linked_folders() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let base = root.path().join("site");
    write(&base.join("index.md"), "[a](docs/a.md)\n\n[ext](ext.md)\n");
    write(
        &root.path().join("outside/a.md"),
        "[b](b.md)\n\n[home](../index.md)\n",
    );
    write(
        &root.path().join("outside/b.md"),
        "# B\n\n![pic](img/p.png)\n\n<img src=\"../docs/img/p.png\">\n",
    );
    write(&root.path().join("outside/img/p.png"), "PNGDATA");
    write(&root.path().join("other/doc.md"), "# Ext\n");
    symlink("../outside", base.join("docs")).unwrap();
    symlink("../other/doc.md", base.join("ext.md")).unwrap();

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), &base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 4, "index, docs/a, docs/b, ext");
    let a = fs::read_to_string(out.path().join("docs/a.html")).unwrap();
    assert!(a.contains("href=\"../index.html\""), "{a}");
    assert!(out.path().join("docs/b.html").is_file());
    assert_eq!(result.assets, 1, "one image, reached two ways");
    assert_eq!(
        fs::read(out.path().join("docs/img/p.png")).unwrap(),
        b"PNGDATA"
    );
    let ext = fs::read_to_string(out.path().join("ext.html")).unwrap();
    assert!(ext.contains("Ext"), "{ext}");
}

#[cfg(unix)]
#[test]
fn bundle_never_follows_links_the_server_refuses() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let base = root.path().join("site");
    write(
        &base.join("index.md"),
        "[k](keys/k.md) [up](up/x.md) [esc](docs/../../x.md) [txt](notes.md)\n",
    );
    write(&root.path().join(".ssh/k.md"), "# Key\n");
    write(&root.path().join("x.md"), "# Above\n");
    write(&root.path().join("outside/a.md"), "# A\n");
    write(&root.path().join("notes.txt"), "plain\n");
    symlink("../.ssh", base.join("keys")).unwrap();
    symlink("..", base.join("up")).unwrap();
    symlink("../outside", base.join("docs")).unwrap();
    symlink("../notes.txt", base.join("notes.md")).unwrap();

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), &base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 1);
    for page in ["keys/k.html", "up/x.html", "x.html", "notes.html"] {
        assert!(!out.path().join(page).exists(), "{page}");
    }
}

#[test]
fn bundle_errors_on_output_collision() {
    // a.md and a.markdown both map to a.html.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "[x](a.md)\n[y](a.markdown)\n");
    write(&base.join("a.md"), "# md\n");
    write(&base.join("a.markdown"), "# markdown\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let err = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme))
        .expect_err("colliding output paths must error");
    let msg = err.to_string();
    assert!(
        msg.contains("a.md") && msg.contains("a.markdown"),
        "error should name both colliding sources, got: {msg}"
    );
}

#[test]
fn bundle_skips_directory_valued_asset() {
    // Regression: a link/image pointing at an in-tree directory must warn+skip,
    // not abort the whole export when fs::copy fails on the directory.
    let src = tempdir().unwrap();
    let base = src.path();
    fs::create_dir_all(base.join("docs")).unwrap();
    write(&base.join("index.md"), "[d](docs/)\n\n![x](docs)\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 1);
    assert_eq!(
        result.assets, 0,
        "a directory target is not a copyable asset"
    );
    assert!(out.path().join("index.html").is_file());
}

#[test]
fn bundle_dedupes_shared_asset() {
    // One image referenced from two pages is copied exactly once.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "[p](page.md)\n\n![l](logo.png)\n");
    write(&base.join("page.md"), "![l](logo.png)\n");
    write(&base.join("logo.png"), "LOGO");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 2);
    assert_eq!(result.assets, 1, "shared asset copied once");
    assert!(out.path().join("logo.png").is_file());
}

#[test]
fn bundle_resolves_downward_and_nested_raw_html_image() {
    // index -> docs/a.md (down a dir), and docs/a.md references a raw-HTML image
    // in a sibling subdir; it must resolve relative to docs/ and be copied.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "[a](docs/a.md)\n");
    write(
        &base.join("docs/a.md"),
        "# A\n\n<img src=\"img/pic.png\">\n",
    );
    write(&base.join("docs/img/pic.png"), "PIC");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.assets, 1, "nested raw-html image copied");
    assert!(out.path().join("docs/img/pic.png").is_file());
    let a = fs::read_to_string(out.path().join("docs/a.html")).unwrap();
    assert!(
        a.contains("src=\"img/pic.png\""),
        "raw-html image src stays relative to its page, got: {a}"
    );
}

#[test]
fn bundle_markdown_entry_extension() {
    // A `.markdown` entry file produces `<stem>.html` and entry_html points to it.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("README.markdown"), "# Readme\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(
        &base.join("README.markdown"),
        base,
        out.path(),
        &opts(&theme),
    )
    .unwrap();

    assert_eq!(
        result.entry_html,
        out.path().canonicalize().unwrap().join("README.html")
    );
    assert!(result.entry_html.is_file());
}

#[test]
fn bundle_single_file_no_links() {
    // Backward-compat shape: a link-less file yields a one-page bundle folder.
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("solo.md"), "# Solo\n\njust text\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("solo.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 1);
    assert_eq!(result.assets, 0);
    assert_eq!(
        result.entry_html,
        out.path().canonicalize().unwrap().join("solo.html")
    );
}

#[test]
fn bundle_handles_cyclic_links() {
    let src = tempdir().unwrap();
    let base = src.path();
    write(&base.join("index.md"), "[p](page.md)\n");
    write(&base.join("page.md"), "[home](index.md)\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.pages, 2, "cycle must terminate with each page once");
    assert!(out.path().join("index.html").is_file());
    assert!(out.path().join("page.html").is_file());
}

const DOC_SCRIPT_BODY: &str = "window.pwned=1";

/// Base64 SHA-256 from `openssl`, an oracle independent of birta's hashing.
fn sha256_b64(bytes: &[u8]) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .args(["-c", "openssl dgst -sha256 -binary | openssl base64 -A"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn unescape(s: &str) -> String {
    s.replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The CSP `<meta>` value; panics unless it is the first element in `<head>` and the only one.
fn csp_meta(page: &str) -> String {
    let head = page.find("<head>").expect("no <head>") + "<head>".len();
    let first = page[head..].trim_start();
    let open = "<meta http-equiv=\"Content-Security-Policy\" content=\"";
    assert!(
        first.starts_with(open),
        "CSP meta is not first in <head>: {}",
        &first[..first.len().min(200)]
    );
    assert_eq!(
        page.to_ascii_lowercase()
            .matches("http-equiv=\"content-security-policy\"")
            .count(),
        1,
        "want one CSP meta"
    );
    let value = &first[open.len()..];
    unescape(&value[..value.find('"').unwrap()])
}

/// Sources of `name`, without the directive name.
fn sources<'a>(csp: &'a str, name: &str) -> Vec<&'a str> {
    let directive = csp
        .split(';')
        .map(str::trim)
        .find(|d| d.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no {name}: {csp}"));
    directive.split_whitespace().skip(1).collect()
}

/// `(open tag, body)` of every `<script>` in the page.
fn scripts(page: &str) -> Vec<(&str, &str)> {
    page.match_indices("<script")
        .map(|(i, _)| {
            let body = i + page[i..].find('>').unwrap() + 1;
            let end = body + page[body..].find("</script>").unwrap();
            (&page[i..body], &page[body..end])
        })
        .collect()
}

fn csp_bundle() -> (tempfile::TempDir, String, String) {
    let src = tempdir().unwrap();
    let base = src.path();
    write(
        &base.join("index.md"),
        &format!("# Index\n\n[a](docs/a.md)\n\n<script>{DOC_SCRIPT_BODY}</script>\n"),
    );
    write(
        &base.join("docs/a.md"),
        "# A\n\n```mermaid\ngraph TD\n  A --> B\n```\n",
    );
    let out = tempdir().unwrap();
    let theme = test_theme();
    export_bundle(&base.join("index.md"), base, out.path(), &opts(&theme)).unwrap();
    let index = fs::read_to_string(out.path().join("index.html")).unwrap();
    let a = fs::read_to_string(out.path().join("docs/a.html")).unwrap();
    (out, index, a)
}

#[test]
fn bundle_csp_meta_allows_only_the_viewer_script_and_pinned_cdn() {
    let (_out, index, a) = csp_bundle();
    let csp = csp_meta(&index);
    assert_eq!(csp, csp_meta(&a), "every page shares one policy");

    let script_src = sources(&csp, "script-src");
    for banned in [
        "'unsafe-inline'",
        "'unsafe-eval'",
        "'strict-dynamic'",
        "'self'",
        "*",
        "http:",
        "https:",
        "data:",
        "file:",
    ] {
        assert!(!script_src.contains(&banned), "{banned} in {csp}");
    }
    assert!(
        !script_src.iter().any(|s| s.starts_with("'nonce-")),
        "a static nonce is public: {csp}"
    );

    // The hash covers the template's inline script bytes, and never the doc's.
    let inline: Vec<&str> = scripts(&index)
        .into_iter()
        .filter(|(tag, _)| !tag.contains(" src=") && !tag.contains("application/json"))
        .map(|(_, body)| body)
        .filter(|body| *body != DOC_SCRIPT_BODY)
        .collect();
    assert!(!inline.is_empty(), "no viewer script found");
    for body in &inline {
        let hash = format!("'sha256-{}'", sha256_b64(body.as_bytes()));
        assert!(script_src.contains(&hash.as_str()), "{hash} not in {csp}");
    }
    let doc_hash = format!("'sha256-{}'", sha256_b64(DOC_SCRIPT_BODY.as_bytes()));
    assert!(
        !script_src.contains(&doc_hash.as_str()),
        "doc script allowed"
    );
    assert_eq!(
        script_src
            .iter()
            .filter(|s| s.starts_with("'sha256-"))
            .count(),
        inline.len(),
        "hashes beyond the template's inline scripts: {csp}"
    );

    // Host sources are exact-release path prefixes, and cover every CDN script.
    let hosts: Vec<&str> = script_src
        .iter()
        .copied()
        .filter(|s| !s.starts_with('\''))
        .collect();
    for host in &hosts {
        let package = host
            .strip_prefix("https://cdn.jsdelivr.net/npm/")
            .and_then(|p| p.strip_suffix('/'))
            .unwrap_or_else(|| panic!("not a jsdelivr package prefix: {host}"));
        let version = package.split_once('@').map_or("", |(_, v)| v);
        assert!(
            version.split('.').count() == 3
                && version
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
            "{host} is not an exact release"
        );
    }
    let external: Vec<&str> = scripts(&index)
        .into_iter()
        .filter_map(|(tag, _)| {
            let start = tag.find(" src=\"")? + 6;
            Some(&tag[start..start + tag[start..].find('"')?])
        })
        .collect();
    assert!(external.len() >= 2, "{external:?}");
    for src in external {
        assert!(
            hosts.iter().any(|h| src.starts_with(h)),
            "{src} is not allowed by {csp}"
        );
    }
}

#[test]
fn bundle_csp_meta_snapshot() {
    let (_out, index, _) = csp_bundle();
    let csp = csp_meta(&index);
    let redacted = csp
        .split(' ')
        .map(|s| {
            if s.starts_with("'sha256-") {
                "'sha256-[hash]'"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    insta::assert_snapshot!(redacted.split("; ").collect::<Vec<_>>().join("\n"));
}
