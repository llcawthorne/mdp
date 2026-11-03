//! EPUB 3 e-book generation (`--epub` / `-e`, or `-o *.epub`).
//!
//! Packages the rendered Markdown document, GitHub + [`syntect`] stylesheet,
//! and an EPUB 3 Navigation Document (`nav.xhtml`) into a standards-compliant
//! Open Container Format (`.epub`) ZIP archive.
//!
//! # Go (`mdpgo`) vs. Rust (`mdp`)
//!
//! - **Native EPUB 3 Support**: Unlike `mdpgo` (which only supported HTML and
//!   headless-Chrome PDF output), `mdp` in Rust adds native EPUB 3 generation
//!   in pure Rust without requiring an external tool like Pandoc or Calibre.
//! - **In-Memory `Write + Seek` via `Cursor<Vec<u8>>`**: The ZIP format writes
//!   its central directory header at the end of the archive and seeks back to
//!   update offsets/CRCs, so `zip::ZipWriter<W>` requires `W: Write + Seek`.
//!   In Go, `bytes.Buffer` only implements `io.Writer` (not `io.WriteSeeker`).
//!   In Rust's standard library, wrapping a `Vec<u8>` in [`std::io::Cursor`]
//!   implements both [`Write`] and [`std::io::Seek`], letting `build_epub`
//!   construct the entire `.epub` archive in memory before touching disk!
//!
//! # Stepping Stone to `mdBook`
//!
//! In `mdBook`, EPUB support lives in the ecosystem plugin `mdbook-epub`, which
//! implements `mdBook`'s external renderer protocol: when `book.toml` contains
//! `[output.epub]`, `crates/mdbook-driver/src/mdbook.rs` serializes the parsed
//! book and configuration into a `RenderContext` JSON payload and pipes it to
//! the `mdbook-epub` child process over `stdin`.
//! Because `mdp` previews single Markdown files directly, `src/epub.rs` reuses
//! `crate::render::render_document` (`pub(crate)`) in-process and packages
//! the Open Container Format (OCF) archive directly:
//! 1. `mimetype` (`application/epub+zip`, stored uncompressed as the first ZIP entry).
//! 2. `META-INF/container.xml` pointing to `EPUB/package.opf`.
//! 3. `EPUB/package.opf` declaring metadata, manifest items, and spine order.
//! 4. `EPUB/style.css` containing the GitHub and [`syntect`] CSS rules.
//! 5. `EPUB/nav.xhtml` containing the EPUB 3 `<nav epub:type="toc">` outline.
//! 6. `EPUB/content.xhtml` containing the rendered document converted from HTML5
//!    to strict XHTML5 (with self-closed void elements like `<hr />` and `<img ... />`).

use crate::github::escape_html;
use crate::render::{FRAGMENT_ENCODE_SET, HeadingEntry, render_document, slug};
use crate::style::Theme;
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
///
/// E-readers parse `.xhtml` files inside an EPUB using a strict XML parser rather
/// than a forgiving HTML5 parser, so unclosed void tags like `<hr>`, `<br>`,
/// `<img ...>`, or `<input ...>` must be written as `<hr />`, `<br />`, etc.
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

/// Renders `input` Markdown bytes into an in-memory EPUB 3 archive.
pub fn build_epub(input: &[u8], filename: &Path, opts: &Options) -> Result<Vec<u8>> {
    let doc = render_document(input, filename, opts)?;

    let book_id = {
        let s = slug(&doc.title);
        if s.is_empty() {
            String::from("urn:mdp:book")
        } else {
            format!("urn:mdp:{s}")
        }
    };

    let package_opf = render_package_opf(&doc.title, &book_id);
    let nav_xhtml = render_nav_xhtml(&doc.title, &doc.headings);
    let content_xhtml = render_content_xhtml(
        &doc.title,
        opts.theme,
        &doc.code_style,
        &html_to_xhtml(&doc.body),
    );

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));

    // Per the EPUB Open Container Format (OCF) specification, `mimetype` MUST be
    // the first file in the ZIP archive and MUST be stored uncompressed (`Stored`)
    // so e-readers can identify an `.epub` file by checking magic bytes at offset 30.
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
        ("EPUB/style.css", doc.css.as_bytes()),
        ("EPUB/nav.xhtml", nav_xhtml.as_bytes()),
        ("EPUB/content.xhtml", content_xhtml.as_bytes()),
    ] {
        zip.start_file(path, deflated)
            .with_context(|| format!("failed to start EPUB entry {path}"))?;
        zip.write_all(data)
            .with_context(|| format!("failed to write EPUB entry {path}"))?;
    }

    // `zip.finish()` writes the central directory and returns our `Cursor<Vec<u8>>`;
    // `.into_inner()` unwraps the underlying `Vec<u8>` with zero copying.
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
fn render_package_opf(title: &str, book_id: &str) -> String {
    let escaped_title = escape_html(title);
    let escaped_id = escape_html(book_id);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id" xml:lang="en">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">{escaped_id}</dc:identifier>
    <dc:title>{escaped_title}</dc:title>
    <dc:language>en</dc:language>
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
    let theme_str = theme.as_str();
    let body_class = if theme == Theme::Dark {
        r#" class="dark""#
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en" data-theme="{theme_str}">
<head>
  <meta charset="utf-8" />
  <title>{escaped_title}</title>
  <link rel="stylesheet" type="text/css" href="style.css" />
</head>
<body{body_class}>
  <article class="markdown-body" data-code-style="{escaped_style}">
{xhtml_body}  </article>
</body>
</html>
"#
    )
}
