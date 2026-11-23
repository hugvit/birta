use birta::theme::search;

const CUSTOM_THEME: &str = r##"[theme]
name = "custom-path-theme"

[theme.dark]
fg_primary = "#ffffff"
bg_primary = "#000000"
"##;

#[test]
fn cli_resolves_theme_by_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mine.toml");
    std::fs::write(&path, CUSTOM_THEME).unwrap();

    let resolved = search().resolve(path.to_str().unwrap()).unwrap();
    assert_eq!(resolved.name, "custom-path-theme");

    let without_ext = dir.path().join("mine");
    let resolved = search().resolve(without_ext.to_str().unwrap()).unwrap();
    assert_eq!(resolved.name, "custom-path-theme");
}

#[test]
fn cli_resolves_theme_relative_to_cwd() {
    // Integration tests run with the package root as cwd.
    let resolved = search()
        .resolve("./crates/birta-core/assets/themes/dracula.toml")
        .unwrap();
    assert_eq!(resolved.name, "dracula");
}
