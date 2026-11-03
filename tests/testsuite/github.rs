//! Unit and integration tests for GFM extensions (`github.rs`) and HTML link/image
//! rewriting (`links.rs`).

use crate::markdown_parts;
use mdp::links::fix_links;
use mdp::{Options, parse_content};
use std::fs;
use std::path::Path;

#[test]
fn math_rendering() {
    let cases = [
        ("$x^2$", r#"<span class="math math-inline">x^2</span>"#),
        ("$`a $ b`$", r#"<span class="math math-inline">a $ b</span>"#),
        ("$$x$$", r#"<span class="math math-display">x</span>"#),
        ("$$\nx\n$$", r#"<span class="math math-display">x</span>"#),
        ("$a<b$", r#"<span class="math math-inline">a&lt;b</span>"#),
        // Non-math: prices, spaces inside dollars, escapes, line breaks
        ("$5 and $10", "$5 and $10"),
        ("$ x $", "$ x $"),
        (r"\$x\$", "$x$"),
        ("$x\ny$", "$x\ny$"),
        ("$x$5", "$x$5"),
        ("$$", "$$"),
    ];

    for (input, want) in cases {
        let md = format!("{input}\n");
        let page = parse_content(md.as_bytes(), Path::new("a.md"), &Options::default())
            .expect("parse_content succeeded");
        let text = String::from_utf8_lossy(&page);
        let expected = format!("<p>{want}</p>");
        assert!(
            text.contains(&expected),
            "case {input:?}: expected {expected:?} in:\n{}",
            markdown_parts(&page)
        );
    }
}

#[test]
fn scripts_only_when_needed() {
    const KATEX: &str = "katex.render(";
    const MERMAID: &str = "mermaid.run(";

    let cases: &[(&str, &str, &[&str], &[&str])] = &[
        ("plain", "# Hi\n", &[], &[KATEX, MERMAID]),
        (
            "math",
            "$x$\n",
            &[KATEX, "font/woff2;base64,"],
            &[MERMAID],
        ),
        ("math block", "```math\nx\n```\n", &[KATEX], &[MERMAID]),
        (
            "diagram",
            "```mermaid\ngraph LR\n```\n",
            &[MERMAID],
            &[KATEX],
        ),
        (
            "code that mentions them",
            "```go\n// $x$ and mermaid\n```\n",
            &[],
            &[KATEX, MERMAID],
        ),
    ];

    for &(name, input, want, not_want) in cases {
        let page = parse_content(input.as_bytes(), Path::new("a.md"), &Options::default())
            .expect("parse_content succeeded");
        let text = String::from_utf8_lossy(&page);
        for &w in want {
            assert!(text.contains(w), "case {name}: missing {w:?}");
        }
        for &nw in not_want {
            assert!(!text.contains(nw), "case {name}: unexpectedly contained {nw:?}");
        }
    }
}

#[test]
fn front_matter_error() {
    let page = parse_content(
        b"---\nkey: [unclosed\n---\n\n# Hi\n",
        Path::new("a.md"),
        &Options::default(),
    )
    .expect("parse_content succeeded");
    let parts = markdown_parts(&page);
    assert!(
        parts.contains("<strong>Error in front matter:</strong>"),
        "missing front matter error banner in:\n{parts}"
    );
}

#[test]
fn fix_links_rewrites_and_embeds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    fs::write(dir.join("dot.png"), b"\x89PNG\r\n\x1a\n").expect("write dot.png");

    let abs = format!("file://{}", dir.to_string_lossy().replace('\\', "/"));
    let parent_abs = format!(
        "file://{}",
        dir.parent()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    );

    let cases = [
        (
            "file link",
            r#"<a href="other.md#sec">x</a>"#,
            format!(r#"<a href="{abs}/other.md#sec">x</a>"#),
            false,
        ),
        (
            "folder link keeps its slash",
            r#"<a href="docs/">x</a>"#,
            format!(r#"<a href="{abs}/docs/">x</a>"#),
            false,
        ),
        (
            "parent folder",
            r#"<a href="../up.md">x</a>"#,
            format!(r#"<a href="{parent_abs}/up.md">x</a>"#),
            false,
        ),
        (
            "served links stay relative",
            r#"<a href="other.md">x</a>"#,
            r#"<a href="other.md">x</a>"#.to_string(),
            true,
        ),
        (
            "page anchor",
            r##"<a href="#top">x</a>"##,
            r##"<a href="#top">x</a>"##.to_string(),
            false,
        ),
        (
            "web link",
            r#"<a href="https://example.com/a.md">x</a>"#,
            r#"<a href="https://example.com/a.md">x</a>"#.to_string(),
            false,
        ),
        (
            "raw HTML image",
            r#"<img src="dot.png">"#,
            r#"<img src="data:image/png;base64,iVBORw0KGgo=">"#.to_string(),
            false,
        ),
        (
            "missing image",
            r#"<img src="gone.png">"#,
            r#"<img src="gone.png">"#.to_string(),
            false,
        ),
        (
            "srcset",
            r#"<source srcset="dot.png 2x, gone.png">"#,
            r#"<source srcset="data:image/png;base64,iVBORw0KGgo= 2x, gone.png">"#.to_string(),
            false,
        ),
        (
            "dark-only image",
            r##"<img src="https://x.test/a.png#gh-dark-mode-only">"##,
            r##"<img src="https://x.test/a.png#gh-dark-mode-only" class="gh-dark-mode-only">"##
                .to_string(),
            false,
        ),
    ];

    for (name, input, want, served) in cases {
        let got = fix_links(input, dir, served);
        assert_eq!(got, want, "case {name}");
    }
}

#[test]
fn tag_filter_escapes_disallowed_raw_html() {
    let page = parse_content(
        b"<textarea>x</textarea>\n\n<kbd>K</kbd> `<script>`\n",
        Path::new("a.md"),
        &Options::default(),
    )
    .expect("parse_content succeeded");
    let body = markdown_parts(&page);
    for want in [
        "&lt;textarea&gt;x&lt;/textarea&gt;",
        "<kbd>K</kbd>",
        "<code>&lt;script&gt;</code>",
    ] {
        assert!(body.contains(want), "missing {want:?} in:\n{body}");
    }
}

