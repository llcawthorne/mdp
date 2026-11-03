//! HTML link and image post-processing.
//!
//! After Markdown rendering and HTML sanitization, [`fix_links`] rewrites
//! relative paths in `<img src>`, `<source srcset>`, and `<a href>` tags
//! relative to the Markdown file's parent directory:
//! - Local images are inlined as `data:<mime>;base64,...` URIs so standalone
//!   temp files and served pages are self-contained.
//! - Images ending in `#gh-dark-mode-only` or `#gh-light-mode-only` receive a
//!   matching CSS class so only the active color mode displays them.
//! - Relative `<a href>` links become absolute `file://` URLs unless `served`
//!   is `true` (watch mode, where the local HTTP server resolves them).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// Regex matching `<img ...>`, `<source ...>`, or `<a ...>` opening tags.
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<(img|source|a)\b([^>]*?)(\s*/?)>"#).expect("valid HTML tag regex")
});

/// Regex matching `key="value"` or `key='value'` or `key=value` attributes inside a tag.
static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#)
        .expect("valid HTML attribute regex")
});

// Explicit lifetime specifier note (`pub fn fix_links<'a>(page_html: &'a str, dir: &Path, ...)-> Cow<'a, str>`):
//   Why do we need an explicit `'a` here when `escape_html` in `github.rs`
//   didn't need one?
//   Because `fix_links` takes **two** reference parameters (`page_html: &str`
//   and `dir: &Path`). With two input lifetimes, Rust's compiler refuses to
//   guess which one the returned `Cow<'_, str>` borrows from!
//   Writing `<'a>` on `page_html: &'a str` and `Cow<'a, str>` tells the borrow
//   checker: "the returned `Cow` may borrow from `page_html`, and never borrows
//   from `dir`"—so `dir` can be a temporary path that drops immediately after
//   `fix_links` returns while the returned `Cow<'a, str>` stays valid as long
//   as `page_html` is alive.
/// Rewrites `<img src>`, `<source srcset>`, and `<a href>` attributes in `page_html`
/// relative to `dir`.
pub fn fix_links<'a>(page_html: &'a str, dir: &Path, served: bool) -> Cow<'a, str> {
    TAG_RE.replace_all(page_html, |caps: &Captures<'_>| {
        let tag = caps[1].to_ascii_lowercase();
        let attrs_raw = &caps[2];
        let closing = &caps[3];

        let mut changed = false;
        let mut extra_class: Option<&'static str> = None;

        let new_attrs = ATTR_RE.replace_all(attrs_raw, |acaps: &Captures<'_>| {
            let key = acaps[1].to_ascii_lowercase();
            let raw_val = acaps
                .get(2)
                .or_else(|| acaps.get(3))
                .or_else(|| acaps.get(4))
                .map(|m| m.as_str())
                .unwrap_or("");

            let new_val = match (tag.as_str(), key.as_str()) {
                ("img", "src") => {
                    if let Some(mode) = mode_only(raw_val) {
                        extra_class = Some(mode);
                        changed = true;
                    }
                    embed_image(raw_val, dir)
                }
                ("source", "srcset") => embed_srcset(raw_val, dir),
                ("a", "href") if !served => absolute_link(raw_val, dir),
                _ => return acaps[0].to_string(),
            };

            if new_val != raw_val {
                changed = true;
                format!(r#"{}="{}""#, &acaps[1], new_val)
            } else {
                acaps[0].to_string()
            }
        });

        if !changed {
            return caps[0].to_string();
        }

        let mut out = format!("<{}{}", &caps[1], new_attrs);
        if let Some(cls) = extra_class {
            out.push_str(&format!(r#" class="{cls}""#));
        }
        out.push_str(closing);
        out.push('>');
        out
    })
}

/// Returns the CSS class name if `src` ends with `#gh-dark-mode-only` or `#gh-light-mode-only`.
fn mode_only(src: &str) -> Option<&'static str> {
    for mode in ["gh-dark-mode-only", "gh-light-mode-only"] {
        if src.ends_with(&format!("#{mode}")) {
            return Some(mode);
        }
    }
    None
}

// Struct lifetime note (`struct LocalRef<'a>`):
//   `LocalRef<'a>` mixes an owned `PathBuf` (allocated because we joined `dir`
//   and decoded percent-escapes) with zero-copy borrowed slices `Option<&'a str>`
//   for `query` and `fragment` (sliced directly out of `reference: &'a str`).
//   In Rust, any struct holding a reference must declare a lifetime parameter
//   (`<'a>`) so the compiler guarantees a `LocalRef<'a>` never outlives the
//   `reference` string its `query` and `fragment` fields point into.
/// Parsed components of a relative or local file reference.
struct LocalRef<'a> {
    path: PathBuf,
    had_trailing_slash: bool,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

/// Resolves `reference` against `dir` if it refers to a local filesystem path
/// rather than a remote URL (`https://...`, `mailto:...`, `data:...`) or a
/// bare `#fragment`.
fn local_path<'a>(reference: &'a str, dir: &Path) -> Option<LocalRef<'a>> {
    if reference.is_empty() || reference.starts_with('#') || reference.starts_with("//") {
        return None;
    }

    let (before_frag, fragment) = match reference.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (reference, None),
    };
    let (raw_path, query) = match before_frag.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (before_frag, None),
    };

    if raw_path.is_empty() {
        return None;
    }

    // Reject URLs with a scheme (e.g. "https:", "mailto:", "data:", "javascript:")
    if let Some((scheme, _)) = raw_path.split_once(':')
        && !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }

    // `percent_decode_str(raw_path).decode_utf8()` returns `Result<Cow<'_, str>, Utf8Error>`:
    // if `raw_path` has no `%XX` sequences (the 99% case), `decoded` is a
    // `Cow::Borrowed(&str)` pointing straight into `raw_path` with zero allocation!
    // And `?` on `.ok()?` early-returns `None` if a `%XX` sequence decodes to
    // invalid UTF-8.
    let decoded = percent_decode_str(raw_path).decode_utf8().ok()?;
    let had_trailing_slash = decoded.ends_with('/');
    let rel_path = Path::new(decoded.as_ref());

    let joined = if rel_path.is_absolute() {
        rel_path.to_path_buf()
    } else {
        let base = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        base.join(rel_path)
    };

    let abs = if joined.is_absolute() {
        joined
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(joined)
    } else {
        joined
    };

    Some(LocalRef {
        path: clean_path(&abs),
        had_trailing_slash,
        query,
        fragment,
    })
}

/// Lexically resolves `.` and `..` path components without requiring the target
/// file to exist on disk.
///
/// Unlike `std::fs::canonicalize`, which queries the OS filesystem and fails
/// with `ENOENT` if a linked file does not exist yet, iterating over
/// [`Path::components`] normalizes `.` and `..` purely in memory.
fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Reads a local image file and encodes it as a `data:<mime>;base64,...` URI,
/// or returns `src` unchanged if it is remote or unreadable.
pub fn embed_image(src: &str, dir: &Path) -> String {
    let Some(local) = local_path(src, dir) else {
        return src.to_string();
    };
    let Ok(data) = fs::read(&local.path) else {
        return src.to_string();
    };

    let mime_type = mime_guess::from_path(&local.path)
        .first_raw()
        .unwrap_or_else(|| sniff_mime(&data));

    format!("data:{mime_type};base64,{}", BASE64.encode(&data))
}

/// Sniffs basic image/binary MIME types when a file has no recognized extension.
fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Embeds each local image candidate in an HTML `srcset` attribute (`"a.png 1x, b.png 2x"`).
pub fn embed_srcset(srcset: &str, dir: &Path) -> String {
    if srcset.contains("data:") {
        return srcset.to_string();
    }
    srcset
        .split(',')
        .map(|candidate| {
            let mut fields: Vec<String> =
                candidate.split_whitespace().map(str::to_string).collect();
            if let Some(first) = fields.first_mut() {
                *first = embed_image(first, dir);
            }
            fields.join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrites a relative local link into a `file://` URL, preserving query strings
/// and `#fragment` anchors.
pub fn absolute_link(href: &str, dir: &Path) -> String {
    let Some(local) = local_path(href, dir) else {
        return href.to_string();
    };

    let mut slashed = local.path.to_string_lossy().replace('\\', "/");
    if local.had_trailing_slash && !slashed.ends_with('/') {
        slashed.push('/');
    }

    let mut out = format!("file://{slashed}");
    if let Some(q) = local.query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(frag) = local.fragment {
        out.push('#');
        out.push_str(frag);
    }
    out
}
