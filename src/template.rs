const VIEWER_HTML: &str = include_str!("../assets/viewer.html");
const GITHUB_CSS: &str = include_str!("../assets/github-markdown.css");
const THEME_OVERRIDES: &str = include_str!("../assets/theme-overrides.css");
const PAGE_CSS: &str = include_str!("../assets/page.css");

pub fn render_page(filename: &str, content_html: &str) -> String {
    VIEWER_HTML
        .replace("{{GITHUB_CSS}}", GITHUB_CSS)
        .replace("{{THEME_OVERRIDES}}", THEME_OVERRIDES)
        .replace("{{PAGE_CSS}}", PAGE_CSS)
        .replace("{{FILENAME}}", filename)
        .replace("{{CONTENT}}", content_html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_page_contains_filename() {
        let page = render_page("test.md", "<p>hello</p>");
        assert!(
            page.contains("test.md"),
            "rendered page should contain the filename"
        );
    }

    #[test]
    fn render_page_contains_content() {
        let page = render_page("test.md", "<p>hello</p>");
        assert!(
            page.contains("<p>hello</p>"),
            "rendered page should contain the content HTML"
        );
    }

    #[test]
    fn render_page_contains_markdown_body_class() {
        let page = render_page("test.md", "");
        assert!(
            page.contains("markdown-body"),
            "rendered page should contain the markdown-body class"
        );
    }

    #[test]
    fn render_page_contains_github_css() {
        let page = render_page("test.md", "");
        assert!(
            page.contains(".markdown-body"),
            "rendered page should contain github-markdown-css rules"
        );
    }
}
