//! EPUB 3 e-book generation (`--epub` / `-e`, or `-o *.epub`).
//!
//! Packages the rendered Markdown document, GitHub + [`syntect`] stylesheet,
//! and an EPUB 3 Navigation Document (`nav.xhtml`) into a standards-compliant
//! Open Container Format (`.epub`) ZIP archive.

use crate::github::escape_html;
use crate::render::{FRAGMENT_ENCODE_SET, HeadingEntry, render_document_epub, slug};
use crate::style::{Theme, highlight_css, resolve_theme_name};
use crate::{Config, Options, preview};
use anyhow::{Context, Result};
use percent_encoding::utf8_percent_encode;
use regex::{Captures, Regex};
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Required uncompressed payload of the first `mimetype` entry in an EPUB archive.
const EPUB_MIMETYPE: &[u8] = b"application/epub+zip";

/// Fixed `META-INF/container.xml` pointing EPUB readers to `EPUB/package.opf`.
const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

/// Matches HTML5 void elements so they can be converted into self-closing XHTML tags.
static VOID_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(?P<tag>area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)\b(?P<attrs>[^>]*?)>")
        .expect("valid void tag regex")
});

/// Renders `input` Markdown bytes into an `.epub` file on disk, writes the
/// destination path to `out`, and opens the EPUB in the system viewer unless
/// `cfg.skip_preview` is set.
pub fn run_epub(cfg: &Config, input: &[u8], out: &mut dyn Write) -> Result<()> {
    let epub_name = cfg
        .output
        .clone()
        .unwrap_or_else(|| epub_path(&cfg.filename));
    write_epub(input, &cfg.filename, &cfg.opts, &epub_name)?;

    writeln!(out, "{}", epub_name.display())?;
    if cfg.skip_preview {
        return Ok(());
    }
    preview(&epub_name.to_string_lossy())
}

/// Returns the default EPUB output path for `filename`: alongside the Markdown
/// file with its extension replaced by `.epub`.
pub fn epub_path(filename: &Path) -> PathBuf {
    filename.with_extension("epub")
}

/// Renders `input` Markdown bytes into an EPUB 3 archive and writes it to `epub_name`.
pub fn write_epub(input: &[u8], filename: &Path, opts: &Options, epub_name: &Path) -> Result<()> {
    let bytes = build_epub(input, filename, opts)?;
    fs::write(epub_name, bytes)
        .with_context(|| format!("failed to write EPUB to {}", epub_name.display()))
}

/// Resolves the effective color theme for EPUB output.
///
/// Like PDF output, EPUB reader environments cannot execute dynamic JavaScript
/// theme switchers. Therefore, [`Theme::Auto`] queries [`crate::pdf::os_prefers_dark`].
pub fn epub_theme(th: Theme) -> Theme {
    crate::pdf::pdf_theme(th)
}

/// Renders `input` Markdown bytes into an in-memory EPUB 3 archive.
pub fn build_epub(input: &[u8], filename: &Path, opts: &Options) -> Result<Vec<u8>> {
    let effective_theme = epub_theme(opts.theme);
    let active_theme_name = if effective_theme == Theme::Dark {
        resolve_theme_name(&opts.dark_style, true)
    } else {
        resolve_theme_name(&opts.light_style, false)
    };
    let doc = render_document_epub(input, filename, opts, &active_theme_name)?;

    let book_id = {
        let s = slug(&doc.title);
        if s.is_empty() {
            String::from("urn:mdp:book")
        } else {
            format!("urn:mdp:{s}")
        }
    };

    let package_opf = render_package_opf(&doc.title, &book_id, opts.author.as_deref());
    let nav_xhtml = render_nav_xhtml(&doc.title, &doc.headings);
    let content_xhtml = render_content_xhtml(
        &doc.title,
        effective_theme,
        &doc.code_style,
        &html_to_xhtml(&doc.body),
    );

    // E-readers (Apple Books, Calibre, e-ink devices) often do not execute JavaScript
    // or support multi-attribute `:root[data-theme][data-code-style]` selector scoping.
    // In addition to scoped CSS rules, prepend the direct syntax rules for the active style
    // so every EPUB reader renders syntax highlighting out of the box.
    let direct_highlight_css = highlight_css(&active_theme_name).unwrap_or_default();
    let epub_css = format!("{direct_highlight_css}\n{}", doc.css);

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));

    // Per the EPUB Open Container Format (OCF) specification, `mimetype` MUST be
    // the first file in the ZIP archive and MUST be stored uncompressed.
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    zip.start_file("mimetype", stored)
        .context("failed to write EPUB mimetype entry")?;
    zip.write_all(EPUB_MIMETYPE)
        .context("failed to write EPUB mimetype bytes")?;

    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for (path, data) in [
        ("META-INF/container.xml", CONTAINER_XML.as_bytes()),
        ("EPUB/package.opf", package_opf.as_bytes()),
        ("EPUB/style.css", epub_css.as_bytes()),
        ("EPUB/nav.xhtml", nav_xhtml.as_bytes()),
        ("EPUB/content.xhtml", content_xhtml.as_bytes()),
    ] {
        zip.start_file(path, deflated)
            .with_context(|| format!("failed to start EPUB entry {path}"))?;
        zip.write_all(data)
            .with_context(|| format!("failed to write EPUB entry {path}"))?;
    }

    let cursor = zip.finish().context("failed to finalize EPUB archive")?;
    Ok(cursor.into_inner())
}

/// Converts an HTML5 fragment into well-formed XHTML by self-closing void tags.
fn html_to_xhtml(html: &str) -> String {
    VOID_TAG_RE
        .replace_all(html, |caps: &Captures<'_>| {
            let tag = &caps["tag"];
            let attrs = caps["attrs"].trim_end();
            let attrs = attrs.strip_suffix('/').unwrap_or(attrs).trim_end();
            if attrs.is_empty() {
                format!("<{tag} />")
            } else {
                format!("<{tag}{attrs} />")
            }
        })
        .into_owned()
}

/// Generates the `EPUB/package.opf` Open Packaging Format manifest and spine.
fn render_package_opf(title: &str, book_id: &str, author: Option<&str>) -> String {
    let escaped_title = escape_html(title);
    let escaped_id = escape_html(book_id);
    let author_tag = author
        .map(|a| format!("    <dc:creator>{}</dc:creator>\n", escape_html(a)))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id" xml:lang="en">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">{escaped_id}</dc:identifier>
    <dc:title>{escaped_title}</dc:title>
{author_tag}    <dc:language>en</dc:language>
    <meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="style" href="style.css" media-type="text/css"/>
    <item id="content" href="content.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>
"#
    )
}

/// Generates the `EPUB/nav.xhtml` EPUB 3 Navigation Document from `headings`.
fn render_nav_xhtml(title: &str, headings: &[HeadingEntry]) -> String {
    let escaped_title = escape_html(title);
    let mut items = String::new();
    if headings.is_empty() {
        let _ = writeln!(
            items,
            r#"      <li><a href="content.xhtml">{escaped_title}</a></li>"#
        );
    } else {
        for h in headings {
            let encoded_id = utf8_percent_encode(&h.id, FRAGMENT_ENCODE_SET);
            let label = escape_html(&h.toc_title);
            let _ = writeln!(
                items,
                r#"      <li><a href="content.xhtml#{encoded_id}">{label}</a></li>"#
            );
        }
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en">
<head>
  <meta charset="utf-8" />
  <title>{escaped_title}</title>
  <link rel="stylesheet" type="text/css" href="style.css" />
</head>
<body>
  <nav epub:type="toc" id="toc">
    <h1>{escaped_title}</h1>
    <ol>
{items}    </ol>
  </nav>
</body>
</html>
"#
    )
}

/// Generates the `EPUB/content.xhtml` content document wrapping `xhtml_body`.
fn render_content_xhtml(
    title: &str,
    theme: Theme,
    code_style: &str,
    xhtml_body: &str,
) -> String {
    let escaped_title = escape_html(title);
    let escaped_style = escape_html(code_style);
    let theme_str = match theme {
        Theme::Dark => "dark",
        Theme::Light | Theme::Auto => "light",
    };
    let body_class = if theme == Theme::Dark {
        r#" class="dark""#
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en" data-theme="{theme_str}" data-code-style="{escaped_style}">
<head>
  <meta charset="utf-8" />
  <title>{escaped_title}</title>
  <link rel="stylesheet" type="text/css" href="style.css" />
</head>
<body{body_class}>
  <article class="markdown-body">
{xhtml_body}  </article>
</body>
</html>
"#
    )
}
