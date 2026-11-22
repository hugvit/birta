use std::fs;

#[test]
fn snapshot_all_fixtures() {
    insta::glob!("fixtures/*.md", |path| {
        let markdown = fs::read_to_string(path).unwrap();
        let html = birta::render::render(
            &markdown,
            birta::render::Style::new(None, birta::render::Features::CLI),
        );
        insta::assert_snapshot!(html);
    });
}
