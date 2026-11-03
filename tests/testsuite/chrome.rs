//! End-to-end headless Chrome tests verifying that KaTeX and Mermaid scripts
//! execute cleanly in a real browser DOM.

use mdp::pdf::find_chrome;
use mdp::{Options, Theme, parse_content};
use std::fs;
use std::path::Path;
use std::process::Command;
use url::Url;

#[test]
fn renders_in_chrome() {
    let Ok(chrome) = find_chrome() else {
        return;
    };

    let cases = [
        ("math.md", Theme::Light),
        ("mermaid.md", Theme::Light),
        ("mermaid.md", Theme::Dark),
        ("github.md", Theme::Dark),
    ];

    for (file, theme) in cases {
        let input = Path::new("testdata/golden").join(file);
        let md = fs::read(&input).expect("read golden md");
        let opts = Options {
            theme,
            ..Options::default()
        };
        let page = parse_content(&md, &input, &opts).expect("parse_content");
        let page_str = String::from_utf8_lossy(&page);

        let want_math = page_str.matches(r#"class="math math-"#).count();
        let want_diagrams = page_str.matches(r#"<pre class="mermaid">"#).count();

        let dom = dump_dom(&chrome, &page);

        let root_tag = dom
            .split_once("<html")
            .and_then(|(_, rest)| rest.split_once('>'))
            .map(|(attrs, _)| attrs)
            .expect("html root element");

        let start = dom
            .find(r#"<article class="markdown-body">"#)
            .expect("article start");
        let end = dom.rfind("</article>").expect("article end");
        let article = &dom[start..end];

        let got_math = article.matches(r#"<span class="katex">"#).count();
        assert_eq!(
            got_math, want_math,
            "{file}/{theme}: {got_math} formulas typeset, want {want_math}"
        );
        assert!(
            !article.contains(r#"class="katex-error""#)
                && !article.contains(r##"mathcolor="#cc0000""##),
            "{file}/{theme}: KaTeX reported an error"
        );

        let drawn = article.matches("aria-roledescription=").count()
            - article.matches(r#"aria-roledescription="error""#).count();
        assert_eq!(
            drawn, want_diagrams,
            "{file}/{theme}: {drawn} diagrams drawn, want {want_diagrams}"
        );
        assert!(
            !article.contains("Syntax error"),
            "{file}/{theme}: Mermaid reported a syntax error"
        );

        let want_theme_attr = format!(r#"data-theme="{theme}""#);
        assert!(
            root_tag.contains(&want_theme_attr),
            "{file}/{theme}: root <html> tag missing {want_theme_attr}: {root_tag}"
        );
    }
}

fn dump_dom(chrome: &Path, page: &[u8]) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let html_path = dir.path().join("page.html");
    fs::write(&html_path, page).expect("write page.html");

    let page_url = Url::from_file_path(&html_path).expect("file URL");
    let profile_dir = dir.path().join("profile");

    let out = Command::new(chrome)
        .arg("--headless")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--virtual-time-budget=10000")
        .arg("--dump-dom")
        .arg(page_url.as_str())
        .output()
        .expect("execute chrome --dump-dom");

    assert!(
        out.status.success(),
        "chrome --dump-dom failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
