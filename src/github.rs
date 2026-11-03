//! GitHub-specific Markdown extensions and HTML post-processing.
//!
//! This module implements the GitHub Flavored Markdown (GFM) features layered
//! on top of [`pulldown_cmark`]:
//! - YAML front matter extraction and rendering as a `<table class="front-matter">`
//! - `:emoji:` shortcode replacement in text nodes via [`emojis`]
//! - Inline (`$...$`, `` $`...`$ ``) and display (`$$...$$`, ```` ```math ````) LaTeX math
//! - Fenced ```` ```mermaid ```` diagram blocks
//! - Blockquote alerts (`> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]`, `> [!CAUTION]`)
//! - GFM's disallowed raw HTML tag filter (`<script>`, `<style>`, `<iframe>`, `<textarea>`, etc.)

use pulldown_cmark::BlockQuoteKind;
use regex::{Captures, Regex};
use serde_yaml::Value;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::LazyLock;

// Rust smart pointer / lifetime note (`Cow<'_, str>`):
//   `std::borrow::Cow<'a, B>` ("Clone-on-Write") is an enum with two variants:
//       pub enum Cow<'a, B: ToOwned + ?Sized> {
//           Borrowed(&'a B),
//           Owned(<B as ToOwned>::Owned),
//       }
//   For `B = str`, `Cow<'_, str>` is either a zero-cost borrowed slice
//   `Cow::Borrowed(&str)` or a heap-allocated `Cow::Owned(String)`.
//   Because `Cow<'_, str>` implements `Deref<Target = str>` and `Display`,
//   callers can pass it directly to `format!`, `push_str(&cow)`, or compare it
//   without caring whether an allocation happened!
//   Notice the anonymous lifetime `'_` in the return type `Cow<'_, str>`:
//   by Rust's lifetime elision rules, `'_` ties the returned `Cow`'s borrow
//   lifetime to the single input reference `input: &str`.
/// Escapes `<`, `>`, `&`, `"`, and `'` for safe inclusion inside HTML text or attributes.
///
/// Returns [`Cow::Borrowed`] with zero allocation when `input` contains none of
/// the special characters.
pub fn escape_html(input: &str) -> Cow<'_, str> {
    let needs_escape = ['&', '<', '>', '"', '\''];
    let Some(first) = input.find(needs_escape) else {
        return Cow::Borrowed(input);
    };

    let mut out = String::with_capacity(input.len() + 16);
    out.push_str(&input[..first]);
    for ch in input[first..].chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

// Borrowing & lifetime elision note (`fn extract_front_matter(markdown: &str) -> (String, &str)`):
//   Even though the return type is a tuple `(String, &str)`, Rust's lifetime
//   elision automatically links the `&str` in the return tuple to the input
//   `markdown: &str` (equivalent to `fn extract_front_matter<'a>(markdown: &'a str) -> (String, &'a str)`).
//   The returned `body` slice `&rest[offset + line.len()..]` is a zero-copy
//   window directly into the caller's `markdown` buffer!
/// Extracts optional YAML front matter delimited by `---` at the start of `markdown`.
///
/// Returns `(front_matter_html, remaining_markdown)`. If the front matter block
/// contains invalid YAML, `front_matter_html` renders an inline error banner
/// (`<p><strong>Error in front matter:</strong> ...</p>`) and returns the rest
/// of the document so the page still renders.
pub fn extract_front_matter(markdown: &str) -> (String, &str) {
    let Some(rest) = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))
    else {
        return (String::new(), markdown);
    };

    let mut offset = 0;
    // `split_inclusive('\n')` yields each line *with* its `\n` (or `\r\n`) intact,
    // so `line.len()` is the exact byte advance in `rest` regardless of LF vs CRLF.
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" || trimmed == "..." {
            let yaml_src = &rest[..offset];
            let body = &rest[offset + line.len()..];
            let html = match serde_yaml::from_str::<Value>(yaml_src) {
                Ok(Value::Mapping(map)) => front_matter_table(&map),
                Ok(_) => String::new(),
                Err(err) => format!(
                    "<p><strong>Error in front matter:</strong> {}</p>\n",
                    escape_html(&err.to_string())
                ),
            };
            return (html, body);
        }
        offset += line.len();
    }

    (String::new(), markdown)
}

/// Renders a YAML mapping as a GitHub-style `<table class="front-matter">`.
fn front_matter_table(map: &serde_yaml::Mapping) -> String {
    if map.is_empty() {
        return String::new();
    }
    let mut out = String::from(r#"<table class="front-matter"><thead><tr>"#);
    for (k, _) in map {
        let key_str = yaml_scalar_to_string(k);
        let _ = write!(out, "<th>{}</th>", escape_html(&key_str));
    }
    out.push_str("</tr></thead><tbody><tr>");
    for (_, v) in map {
        let _ = write!(out, "<td>{}</td>", front_matter_value(v));
    }
    out.push_str("</tr></tbody></table>\n");
    out
}

/// Recursively formats a YAML [`Value`] as an HTML fragment for a front-matter cell.
fn front_matter_value(val: &Value) -> String {
    match val {
        Value::Mapping(map) => front_matter_table(map).trim().to_string(),
        Value::Sequence(seq) => {
            let mut out = String::from("<table><tbody><tr>");
            for item in seq {
                let _ = write!(out, "<td>{}</td>", front_matter_value(item));
            }
            out.push_str("</tr></tbody></table>");
            out
        }
        Value::Null => String::new(),
        other => escape_html(&yaml_scalar_to_string(other)).into_owned(),
    }
}

/// Converts a scalar YAML [`Value`] to its plain string representation.
fn yaml_scalar_to_string(val: &Value) -> String {
    match val {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Tagged(tagged) => yaml_scalar_to_string(&tagged.value),
        _ => String::new(),
    }
}

/// Regex matching `:shortcode:` emoji tokens in Markdown text.
static EMOJI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([a-zA-Z0-9_+-]+):").expect("valid emoji regex"));

/// Replaces recognized `:shortcode:` tokens (such as `:rocket:` or `:+1:`) with
/// their Unicode emoji characters, leaving unknown shortcodes untouched.
pub fn replace_emojis(text: &str) -> Cow<'_, str> {
    if !text.contains(':') {
        return Cow::Borrowed(text);
    }
    // `Regex::replace_all` itself returns `Cow<'_, str>`: if `EMOJI_RE` finds
    // no matches in `text`, `replace_all` returns `Cow::Borrowed(text)` without
    // allocating a new `String`!
    EMOJI_RE.replace_all(text, |caps: &Captures<'_>| {
        if let Some(emoji) = emojis::get_by_shortcode(&caps[1]) {
            emoji.as_str().to_string()
        } else {
            caps[0].to_string()
        }
    })
}

/// Returns the `(css_suffix, title)` pair for a GFM [`BlockQuoteKind`] alert.
pub fn alert_meta(kind: BlockQuoteKind) -> (&'static str, &'static str) {
    match kind {
        BlockQuoteKind::Note => ("note", "Note"),
        BlockQuoteKind::Tip => ("tip", "Tip"),
        BlockQuoteKind::Important => ("important", "Important"),
        BlockQuoteKind::Warning => ("warning", "Warning"),
        BlockQuoteKind::Caution => ("caution", "Caution"),
    }
}

/// Regex matching the raw HTML tags disallowed by the GFM `tagfilter` extension.
static TAG_FILTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(/?(?:title|textarea|style|xmp|iframe|noembed|noframes|script|plaintext))\b")
        .expect("valid tagfilter regex")
});

/// Escapes raw HTML tags disallowed by GitHub (`<script>`, `<style>`, `<textarea>`,
/// `<iframe>`, etc.) so they render as literal text.
pub fn filter_tags(html: &str) -> Cow<'_, str> {
    TAG_FILTER_RE.replace_all(html, "&lt;$1")
}

/// Regex matching bare GFM autolinks (`https://...`, `http://...`, `www....`, or `user@domain`).
static AUTOLINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:https?://[^\s<]+|www\.[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}[^\s<]*|[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,})",
    )
    .expect("valid GFM autolink regex")
});

/// Converts bare URLs (`https://...`, `www....`) and email addresses in a plain
/// text node into HTML `<a>` elements according to GFM's extended autolink rules,
/// returning `None` if no bare autolinks are present.
pub fn autolink_gfm(text: &str) -> Option<String> {
    if !text.contains("http://")
        && !text.contains("https://")
        && !text.contains("www.")
        && !text.contains('@')
    {
        return None;
    }

    let mut out = String::with_capacity(text.len() + 32);
    let mut last_end = 0;
    let mut matched_any = false;

    for m in AUTOLINK_RE.find_iter(text) {
        let start = m.start();
        // Require start of string or non-alphanumeric boundary before match
        if start > 0 {
            let prev = text[..start].chars().next_back().unwrap_or(' ');
            if prev.is_alphanumeric() || matches!(prev, '/' | ':' | '.') {
                continue;
            }
        }

        let raw = m.as_str();
        // Trim trailing punctuation per GFM extended autolink rules
        let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if trimmed.is_empty() {
            continue;
        }

        matched_any = true;
        out.push_str(&escape_html(&text[last_end..start]));

        let href = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with("www.") {
            format!("http://{trimmed}")
        } else {
            format!("mailto:{trimmed}")
        };

        let _ = write!(
            out,
            r#"<a href="{}">{}</a>"#,
            escape_html(&href),
            escape_html(trimmed)
        );
        last_end = start + trimmed.len();
    }

    if !matched_any {
        return None;
    }
    out.push_str(&escape_html(&text[last_end..]));
    Some(out)
}
