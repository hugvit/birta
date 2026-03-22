use std::fs;

#[test]
fn snapshot_all_fixtures() {
    insta::glob!("fixtures/*.md", |path| {
        let markdown = fs::read_to_string(path).unwrap();
        let html = sheen::render::render(&markdown);
        insta::assert_snapshot!(html);
    });
}
