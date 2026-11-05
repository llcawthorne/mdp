//! Unit and integration tests for CLI argument parsing (`parse_args`), `run`,
//! themes, syntax styles, TOC generation, PDF paths, and heading slugs.

use mdp::epub::epub_path;
use mdp::pdf::{find_chrome, pdf_path, pdf_theme_with};
use mdp::style::{available_styles, known_style};
use mdp::{Config, Options, Theme, parse_args, parse_content, run, slug};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::{CompressionMethod, ZipArchive};

const INPUT_FILE: &str = "testdata/test1.md";

#[test]
fn parse_content_highlights_code() {
    let input = b"```go\nfunc main() {}\n```\n";
    let result = parse_content(input, Path::new(INPUT_FILE), &Options::default())
        .expect("parse_content");
    let html = String::from_utf8_lossy(&result);

    for want in [
        r#"<pre class="chroma">"#,
        "func",
        r#":root[data-theme="light"][data-code-style="github"] .chroma"#,
    ] {
        assert!(html.contains(want), "result missing {want:?}");
    }

    // Verify TOML syntax highlighting
    let toml_input = b"```toml\n[dependencies]\nrand = \"0.8.5\"\n```\n";
    let toml_res = parse_content(toml_input, Path::new(INPUT_FILE), &Options::default())
        .expect("parse toml content");
    let toml_html = String::from_utf8_lossy(&toml_res);
    assert!(
        toml_html.contains(r#"<span class="sy-entity sy-name sy-section sy-table sy-toml">dependencies</span>"#)
            || toml_html.contains("sy-toml"),
        "TOML code block should be highlighted with sy-toml classes, got:\n{toml_html}"
    );

    // Verify Kotlin syntax highlighting
    let kt_input = b"```kotlin\nval x: Int = 1\nfun main() {}\n```\n";
    let kt_res = parse_content(kt_input, Path::new(INPUT_FILE), &Options::default())
        .expect("parse kotlin content");
    let kt_html = String::from_utf8_lossy(&kt_res);
    assert!(
        kt_html.contains("sy-kotlin"),
        "Kotlin code block should be highlighted with sy-kotlin classes, got:\n{kt_html}"
    );

    // Verify Elixir syntax highlighting
    let ex_input = b"```elixir\ndefmodule Hello do\n  def world do\n    :ok\n  end\nend\n```\n";
    let ex_res = parse_content(ex_input, Path::new(INPUT_FILE), &Options::default())
        .expect("parse elixir content");
    let ex_html = String::from_utf8_lossy(&ex_res);
    assert!(
        ex_html.contains("sy-elixir"),
        "Elixir code block should be highlighted with sy-elixir classes, got:\n{ex_html}"
    );

    // Verify newly supported languages
    for (lang, code, expected_class) in [
        ("swift", "let greeting = \"Hello\"\n", "sy-swift"),
        ("elm", "type Msg = Increment | Decrement\n", "sy-elm"),
        ("purescript", "module Main where\n", "sy-purescript"),
        ("dockerfile", "FROM rust:latest\n", "sy-dockerfile"),
        ("typescript", "const x: number = 42;\n", "chroma"),
        ("shell", "curl -s https://example.com\n", "chroma"),
    ] {
        let input = format!("```{lang}\n{code}```\n");
        let res = parse_content(input.as_bytes(), Path::new(INPUT_FILE), &Options::default())
            .unwrap_or_else(|e| panic!("failed to parse {lang}: {e}"));
        let html = String::from_utf8_lossy(&res);
        assert!(
            html.contains(expected_class),
            "{lang} code block should contain {expected_class}, got:\n{html}"
        );
    }
}

#[test]
fn parse_content_title() {
    let cases = [
        (
            "first h1",
            "Intro\n\n## Sub\n\n# *Real* `Title`\n\n# Second\n",
            "Real Title",
        ),
        ("setext h1", "My Doc\n======\n", "My Doc"),
        (
            "no h1 falls back to file name",
            "## Only a subheading\n",
            "notes.md",
        ),
        ("no headings falls back to file name", "just text\n", "notes.md"),
        ("title is escaped", "# a < b & c\n", "a &lt; b &amp; c"),
    ];

    for (name, input, want) in cases {
        let result = parse_content(
            input.as_bytes(),
            Path::new("some/dir/notes.md"),
            &Options::default(),
        )
        .expect("parse_content");
        let html = String::from_utf8_lossy(&result);
        let expected_tag = format!("<title>{want}</title>");
        assert!(
            html.contains(&expected_tag),
            "case {name}: missing {expected_tag:?}"
        );
    }
}

#[test]
fn parse_content_inlines_local_images() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Minimal valid 1x1 PNG header bytes
    let png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
    fs::write(dir.path().join("dot.png"), png_bytes).expect("write dot.png");

    let input = b"![dot](dot.png) ![gone](missing.png) ![remote](https://example.com/x.png)\n";
    let doc_path = dir.path().join("doc.md");
    let result = parse_content(input, &doc_path, &Options::default()).expect("parse_content");
    let html = String::from_utf8_lossy(&result);

    for want in [
        r#"src="data:image/png;base64,"#,
        r#"src="missing.png""#,
        r#"src="https://example.com/x.png""#,
    ] {
        assert!(html.contains(want), "missing {want:?} in:\n{html}");
    }
}

#[test]
fn parse_content_theme() {
    const DARK_PAGE: &str = r#":root[data-theme="dark"] {"#;
    const LIGHT_CODE: &str =
        r#":root[data-theme="light"][data-code-style="github"] .chroma { background-color: var(--pre-bg); }"#;
    const DARK_CODE: &str =
        r#":root[data-theme="dark"][data-code-style="github"] .chroma { background-color: #24292e;"#;

    let cases = [
        (Theme::Auto, r#"data-theme="light""#, r#"let choice = "auto";"#),
        (Theme::Light, r#"data-theme="light""#, r#"let choice = "light";"#),
        (Theme::Dark, r#"data-theme="dark""#, r#"let choice = "dark";"#),
    ];

    for (theme, attr, starting) in cases {
        let opts = Options {
            theme,
            ..Options::default()
        };
        let result = parse_content(b"# Hi\n", Path::new(INPUT_FILE), &opts).expect("parse_content");
        let html = String::from_utf8_lossy(&result);
        let root_open = format!("<html {attr}");
        for s in [root_open.as_str(), starting, DARK_PAGE, LIGHT_CODE, DARK_CODE] {
            assert!(html.contains(s), "theme {theme}: missing {s:?}");
        }
    }
}

#[test]
fn parse_args_flags_and_validation() {
    let default_opts = Options {
        toc: false,
        theme: Theme::Auto,
        light_style: "InspiredGitHub".to_string(),
        dark_style: "GitHub Dark".to_string(),
        author: None,
        served: false,
    };

    // Defaults
    let mut stderr = Vec::new();
    let got = parse_args(["a.md"], &mut stderr).expect("defaults");
    assert_eq!(
        got,
        Config {
            filename: PathBuf::from("a.md"),
            opts: default_opts.clone(),
            ..Config::default()
        }
    );

    // Short flags & style flag
    let got = parse_args(["--skip", "-w", "-d", "-t", "-s", "railscasts", "a.md"], &mut stderr)
        .expect("flags");
    assert_eq!(
        got,
        Config {
            filename: PathBuf::from("a.md"),
            skip_preview: true,
            watch: true,
            opts: Options {
                toc: true,
                theme: Theme::Dark,
                light_style: "Railscasts".to_string(),
                dark_style: "Railscasts".to_string(),
                author: None,
                served: false,
            },
            ..Config::default()
        }
    );

    // Combined short flags
    let got = parse_args(["--skip", "-wl", "a.md"], &mut stderr).expect("combined short flags");
    assert_eq!(
        got,
        Config {
            filename: PathBuf::from("a.md"),
            skip_preview: true,
            watch: true,
            opts: Options {
                theme: Theme::Light,
                ..default_opts.clone()
            },
            ..Config::default()
        }
    );

    // Style family resolution
    let got = parse_args(["--style", "solarized-light", "a.md"], &mut stderr).expect("solarized");
    assert_eq!(got.opts.light_style, "Solarized (light)");
    assert_eq!(got.opts.dark_style, "Solarized (dark)");

    // .PDF output implies --pdf
    let got = parse_args(["--output", "notes.PDF", "a.md"], &mut stderr).expect("pdf output");
    assert!(got.pdf);
    assert!(!got.epub);
    assert_eq!(got.output, Some(PathBuf::from("notes.PDF")));

    // -e / --epub and .EPUB output implies --epub
    let got = parse_args(["-e", "a.md"], &mut stderr).expect("-e flag");
    assert!(got.epub);
    assert!(!got.pdf);

    let got =
        parse_args(["--output", "book-name.EPUB", "a.md"], &mut stderr).expect("epub output");
    assert!(got.epub);
    assert!(!got.pdf);
    assert_eq!(got.output, Some(PathBuf::from("book-name.EPUB")));

    // --dark-style override
    let got = parse_args(
        ["--style", "github", "--dark-style", "darcula", "a.md"],
        &mut stderr,
    )
    .expect("dark-style override");
    assert_eq!(got.opts.light_style, "InspiredGitHub");
    assert_eq!(got.opts.dark_style, "Darcula");

    // Error cases
    let err_cases: &[(&[&str], &str)] = &[
        (&[], "Usage: mdp"),
        (&["--skip"], "expected exactly one"),
        (&["a.md", "b.md"], "expected exactly one"),
        (&["-w", "-o", "x.html", "a.md"], "--watch can't"),
        (&["-wp", "a.md"], "--watch can't"),
        (&["-we", "a.md"], "--watch can't"),
        (&["-pe", "a.md"], "--pdf and --epub"),
        (&["-p", "-o", "book.epub", "a.md"], "--pdf and --epub"),
        (&["-d", "-l", "a.md"], "together"),
        (&["--style", "nope", "a.md"], r#"unknown style "nope""#),
        (&["-s", "nope", "a.md"], r#"unknown style "nope""#),
        (&["--dark-style", "nope", "a.md"], r#"unknown style "nope""#),
        (&["--nope", "a.md"], "--nope"),
    ];

    for &(args, want_err) in err_cases {
        let mut err_out = Vec::new();
        let err = parse_args(args, &mut err_out).expect_err("expected error");
        let msg = err.to_string();
        assert!(
            msg.contains(want_err),
            "args {args:?}: err = {msg:?}, want substring {want_err:?}"
        );
    }
}

#[test]
fn code_style_menu() {
    // Selecting a built-in menu style highlights that option without adding a custom one
    let opts = Options {
        light_style: "Solarized (light)".to_string(),
        dark_style: "Solarized (dark)".to_string(),
        ..Options::default()
    };
    let result = parse_content(b"# Hi\n", Path::new(INPUT_FILE), &opts).expect("parse_content");
    let html = String::from_utf8_lossy(&result);
    assert!(html.contains(r#"<option value="solarized" selected>Solarized</option>"#));
    assert!(html.contains(r#" data-code-style="solarized""#));

    // Mixing two different styles creates a `custom` entry at the top of the menu
    let custom_opts = Options {
        light_style: "InspiredGitHub".to_string(),
        dark_style: "Solarized (dark)".to_string(),
        ..Options::default()
    };
    let result =
        parse_content(b"# Hi\n", Path::new(INPUT_FILE), &custom_opts).expect("parse_content");
    let html = String::from_utf8_lossy(&result);
    assert!(
        html.contains(
            r#"<option value="custom" selected>InspiredGitHub / Solarized (dark)</option>"#
        )
    );
    assert!(html.contains(r#" data-code-style="custom""#));

    // Curated styles have their CSS rules embedded
    for id in ["github", "solarized", "ocean", "darcula", "railscasts", "mocha"] {
        for th in ["light", "dark"] {
            let needle = format!(r#":root[data-theme="{th}"][data-code-style="{id}"] .chroma"#);
            assert!(html.contains(&needle), "missing {needle}");
        }
    }
}

#[test]
fn available_styles_are_known() {
    for name in available_styles() {
        assert!(known_style(name), "{name} should be a known style");
    }
}

#[test]
fn parse_content_toc() {
    struct TocCase<'a> {
        name: &'a str,
        input: &'a str,
        toc: bool,
        want: &'a [&'a str],
        not_want: &'a [&'a str],
    }

    let cases = &[
        TocCase {
            name: "lone h1 is the title, so the list starts at h2",
            input: "# Title\n\n## One\n\n### One A\n\n## Two\n",
            toc: false,
            want: &[r#"<nav class="toc""#, r##"href="#one""##, r##"href="#one-a""##, r##"href="#two""##],
            not_want: &[r##"href="#title""##],
        },
        TocCase {
            name: "several h1s are all listed",
            input: "# Part 1\n\n## A\n\n# Part 2\n",
            toc: false,
            want: &[r##"href="#part-1""##, r##"href="#a""##, r##"href="#part-2""##],
            not_want: &[],
        },
        TocCase {
            name: "closed unless asked for",
            input: "# Title\n\n## One\n",
            toc: false,
            want: &[r#"<nav class="toc""#, r#"id="toc-toggle""#, r#"aria-expanded="false""#],
            not_want: &[r#"class="toc-open""#],
        },
        TocCase {
            name: "--toc opens it",
            input: "# Title\n\n## One\n",
            toc: true,
            want: &[r#" data-code-style="github" class="toc-open">"#, r#"aria-expanded="true""#],
            not_want: &[],
        },
        TocCase {
            name: "no headings, no contents or button",
            input: "just text\n",
            toc: true,
            want: &[],
            not_want: &["<nav", r#"id="toc-toggle""#, r#"class="toc-open""#],
        },
    ];

    for c in cases {
        let opts = Options {
            toc: c.toc,
            ..Options::default()
        };
        let result =
            parse_content(c.input.as_bytes(), Path::new(INPUT_FILE), &opts).expect("parse_content");
        let html = String::from_utf8_lossy(&result);
        for &w in c.want {
            assert!(html.contains(w), "case {}: missing {w:?}", c.name);
        }
        for &nw in c.not_want {
            assert!(!html.contains(nw), "case {}: unexpectedly has {nw:?}", c.name);
        }
    }
}

#[test]
fn run_writes_temp_and_explicit_output_file() {
    // Temp file output
    let mut out = Vec::new();
    let cfg = Config {
        filename: PathBuf::from(INPUT_FILE),
        skip_preview: true,
        ..Config::default()
    };
    run(&cfg, &mut out).expect("run temp");
    let temp_path = PathBuf::from(String::from_utf8(out).unwrap().trim());
    let content = fs::read_to_string(&temp_path).expect("read temp output");
    let _ = fs::remove_file(&temp_path);
    assert!(content.contains("Test Markdown File"));

    // Explicit -o output
    let dir = tempfile::tempdir().expect("tempdir");
    let out_name = dir.path().join("notes.html");
    let mut out = Vec::new();
    let cfg = Config {
        filename: PathBuf::from(INPUT_FILE),
        output: Some(out_name.clone()),
        skip_preview: true,
        ..Config::default()
    };
    run(&cfg, &mut out).expect("run explicit output");
    assert_eq!(String::from_utf8(out).unwrap().trim(), out_name.to_string_lossy());
    let page = fs::read_to_string(&out_name).expect("read notes.html");
    assert!(page.contains("Test Markdown File"));
}

#[test]
fn pdf_path_replaces_extension() {
    assert_eq!(pdf_path(Path::new("notes.md")), PathBuf::from("notes.pdf"));
    assert_eq!(
        pdf_path(Path::new("dir/book.markdown")),
        PathBuf::from("dir/book.pdf")
    );
    assert_eq!(pdf_path(Path::new("README")), PathBuf::from("README.pdf"));
}

#[test]
fn epub_path_replaces_extension() {
    assert_eq!(
        epub_path(Path::new("notes.md")),
        PathBuf::from("notes.epub")
    );
    assert_eq!(
        epub_path(Path::new("dir/book.markdown")),
        PathBuf::from("dir/book.epub")
    );
    assert_eq!(epub_path(Path::new("README")), PathBuf::from("README.epub"));
}

#[test]
fn run_makes_epub() {
    let dir = tempfile::tempdir().expect("tempdir");
    let md_path = dir.path().join("notes.md");
    fs::write(
        &md_path,
        "# Book Title\n\nIntro paragraph.\n\n---\n\n## Tasks\n\n- [x] Done\n",
    )
    .expect("write notes.md");

    // 1. Default output path (`-e` without `-o` writes `notes.epub` beside `notes.md`)
    let mut out = Vec::new();
    let cfg = Config {
        filename: md_path.clone(),
        epub: true,
        skip_preview: true,
        ..Config::default()
    };
    run(&cfg, &mut out).expect("run --epub default path");
    let default_epub = dir.path().join("notes.epub");
    assert_eq!(
        String::from_utf8(out).unwrap().trim(),
        default_epub.to_string_lossy()
    );
    assert!(default_epub.is_file());

    // 2. Explicit `-o book-name.epub`
    let epub_name = dir.path().join("book-name.epub");
    let mut out = Vec::new();
    let cfg = Config {
        filename: md_path,
        output: Some(epub_name.clone()),
        epub: true,
        skip_preview: true,
        ..Config::default()
    };

    run(&cfg, &mut out).expect("run --epub explicit path");
    assert_eq!(
        String::from_utf8(out).unwrap().trim(),
        epub_name.to_string_lossy()
    );

    let file = fs::File::open(&epub_name).expect("open generated EPUB");
    let mut archive = ZipArchive::new(file).expect("valid ZIP archive");

    // Per the EPUB OCF spec, entry 0 must be `mimetype`, stored uncompressed
    {
        let mut first = archive.by_index(0).expect("first ZIP entry");
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), CompressionMethod::Stored);
        let mut mime = String::new();
        first.read_to_string(&mut mime).expect("read mimetype");
        assert_eq!(mime, "application/epub+zip");
    }

    let read_entry = |archive: &mut ZipArchive<fs::File>, name: &str| -> String {
        let mut entry = archive
            .by_name(name)
            .unwrap_or_else(|e| panic!("missing EPUB entry {name}: {e}"));
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("UTF-8 EPUB entry");
        text
    };

    let container = read_entry(&mut archive, "META-INF/container.xml");
    assert!(container.contains(r#"full-path="EPUB/package.opf""#));

    let opf = read_entry(&mut archive, "EPUB/package.opf");
    assert!(opf.contains("<dc:title>Book Title</dc:title>"));
    assert!(opf.contains(r#"href="nav.xhtml""#));
    assert!(opf.contains(r#"href="content.xhtml""#));

    let style = read_entry(&mut archive, "EPUB/style.css");
    assert!(style.contains(".markdown-body"));

    let nav = read_entry(&mut archive, "EPUB/nav.xhtml");
    assert!(nav.contains(r#"epub:type="toc""#));
    assert!(nav.contains(r#"href="content.xhtml#book-title""#));
    assert!(nav.contains(r#"href="content.xhtml#tasks""#));

    let content = read_entry(&mut archive, "EPUB/content.xhtml");
    let expected_theme = mdp::epub::epub_theme(Theme::Auto);
    assert!(content.contains(&format!(r#"data-theme="{expected_theme}" data-code-style="github">"#)));
    assert!(content.contains("Book Title"));
    assert!(
        content.contains("<hr />"),
        "void <hr> tag should be self-closed in XHTML"
    );
    assert!(
        !content.contains("<hr>"),
        "unclosed <hr> tag found in XHTML"
    );
    assert!(
        content.contains(r#"<input disabled="" type="checkbox" checked="" />"#),
        "void <input> tag should be self-closed in XHTML: {content}"
    );
}

#[test]
fn run_makes_pdf() {
    if find_chrome().is_err() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let pdf_name = dir.path().join("notes.pdf");
    let mut out = Vec::new();
    let cfg = Config {
        filename: PathBuf::from(INPUT_FILE),
        output: Some(pdf_name.clone()),
        pdf: true,
        skip_preview: true,
        ..Config::default()
    };

    run(&cfg, &mut out).expect("run --pdf");
    let pdf_bytes = fs::read(&pdf_name).expect("read generated PDF");
    assert!(pdf_bytes.starts_with(b"%PDF-"), "output is not a valid PDF");
    assert!(
        pdf_bytes
            .windows(b"/Outlines".len())
            .any(|w| w == b"/Outlines"),
        "generated PDF is missing /Outlines document outline"
    );
}

#[test]
fn slug_matches_github_anchors() {
    let cases = [
        ("Hello, World!", "hello-world"),
        ("Chapter 2 - A Guessing Game", "chapter-2---a-guessing-game"),
        ("Café & Crème", "café--crème"),
        ("日本語の見出し", "日本語の見出し"),
        ("🚀 Emoji launch", "-emoji-launch"),
        ("snake_case and kebab-case", "snake_case-and-kebab-case"),
        ("Rust's Option<T>: 100% safe?", "rusts-optiont-100-safe"),
        ("  ", "--"),
    ];

    for (input, want) in cases {
        assert_eq!(slug(input), want, "slug({input:?})");
    }
}

#[test]
fn pdf_theme_respects_preference_or_os() {
    assert_eq!(pdf_theme_with(Theme::Auto, || false), Theme::Light);
    assert_eq!(pdf_theme_with(Theme::Auto, || true), Theme::Dark);
    assert_eq!(pdf_theme_with(Theme::Light, || true), Theme::Light);
    assert_eq!(pdf_theme_with(Theme::Dark, || false), Theme::Dark);
}
