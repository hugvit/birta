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
fn bundle_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    write(&root.path().join("secret.png"), "TOPSECRET");
    let base = root.path().join("site");
    fs::create_dir_all(&base).unwrap();
    // Symlink inside the tree pointing outside it.
    symlink(root.path().join("secret.png"), base.join("link.png")).unwrap();
    write(&base.join("index.md"), "![x](link.png)\n");

    let out = tempdir().unwrap();
    let theme = test_theme();
    let result = export_bundle(&base.join("index.md"), &base, out.path(), &opts(&theme)).unwrap();

    assert_eq!(result.assets, 0, "symlinked-out asset must not be copied");
    assert!(!out.path().join("link.png").exists());
    // The escaped secret never lands in the bundle under any name.
    assert!(!out.path().join("secret.png").exists());
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
