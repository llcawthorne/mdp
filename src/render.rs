//! Core Markdown-to-HTML rendering, sanitization, heading slugs, and TOC generation.
//!
//! [`parse_content`] drives the rendering pipeline inspired by `mdBook`:
//! 1. Extracts optional YAML front matter via [`crate::github::extract_front_matter`].
//! 2. Streams Markdown through [`pulldown_cmark::Parser`] with GFM extensions enabled
//!    (tables, footnotes, strikethrough, task lists, blockquote alerts, and math).
//! 3. Transforms events in a single pass: assigning GitHub-compatible heading IDs
//!    ([`slug`]), recording headings for the slide-out table of contents,
//!    highlighting fenced code blocks with [`crate::style::highlight_code`],
//!    rendering math and Mermaid blocks, and numbering footnotes.
//! 4. Filters disallowed raw HTML tags (`<script>`, `<style>`, `<iframe>`, etc.) and
//!    sanitizes the HTML fragment with [`ammonia`].
//! 5. Rewrites relative links and embeds local images as `data:` URIs via [`crate::links::fix_links`].
//! 6. Renders the final standalone HTML document with [`handlebars`].

use crate::Options;
use crate::assets::{katex_css, katex_js, mermaid_js};
use crate::github::{
    alert_meta, autolink_gfm, escape_html, extract_front_matter, filter_tags, replace_emojis,
};
use crate::links::fix_links;
use crate::style::{
    CodeStyle, Theme, code_styles, highlight_code, highlight_code_inline, stylesheet,
};
use ammonia::Builder as AmmoniaBuilder;
use anyhow::{Context, Result};
use handlebars::Handlebars;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use pulldown_cmark::{Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, Options as CmarkOptions, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::LazyLock;

/// Embedded Handlebars page template (`assets/page.hbs`).
const PAGE_TEMPLATE: &str = include_str!("../assets/page.hbs");

/// Characters percent-encoded when building `#fragment` hrefs in the TOC.
pub(crate) const FRAGMENT_ENCODE_SET: &AsciiSet =
    &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');

/// Template context serialized into `assets/page.hbs`.
#[derive(Serialize)]
struct PageContext<'a> {
    title: String,
    theme: &'static str,
    is_dark: bool,
    css: String,
    styles: Vec<CodeStyle>,
    code_style: String,
    toc: Option<String>,
    toc_open: bool,
    body: String,
    katex_css: Option<&'a str>,
    katex_js: Option<&'a str>,
    mermaid_js: Option<&'a str>,
}

/// A heading collected during the Markdown pass for the table of contents and page title.
#[derive(Debug, Clone)]
pub(crate) struct HeadingEntry {
    pub(crate) level: u8,
    pub(crate) id: String,
    pub(crate) toc_title: String,
    pub(crate) full_text: String,
}

/// Intermediate rendered document components shared between HTML and EPUB output.
pub(crate) struct RenderedDocument {
    pub(crate) title: String,
    pub(crate) css: String,
    pub(crate) styles: Vec<CodeStyle>,
    pub(crate) code_style: String,
    pub(crate) toc: Option<String>,
    pub(crate) headings: Vec<HeadingEntry>,
    pub(crate) body: String,
    pub(crate) uses_math: bool,
    pub(crate) uses_mermaid: bool,
}

/// Lazily initialized [`ammonia::Builder`] configured to match GitHub's allowed HTML subset.
static SANITIZER: LazyLock<AmmoniaBuilder<'static>> = LazyLock::new(|| {
    let mut builder = AmmoniaBuilder::default();
    builder.add_tags([
        "kbd", "details", "summary", "picture", "source", "input",
    ]);
    builder.add_tag_attributes("pre", ["class", "style"]);
    builder.add_tag_attributes("code", ["class", "style"]);
    builder.add_tag_attributes("span", ["class", "style"]);
    builder.add_tag_attributes("input", ["type", "checked", "disabled"]);
    for h in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        builder.add_tag_attributes(h, ["id", "align"]);
    }
    for el in ["p", "div", "img", "table", "td", "th"] {
        builder.add_tag_attributes(el, ["align"]);
    }
    builder.add_tag_attributes("div", ["id", "role"]);
    builder.add_allowed_classes("div", ["math", "math-display", "footnotes"]);
    builder.add_tag_attributes("p", ["class"]);
    builder.add_tag_attributes("blockquote", ["class"]);
    builder.add_tag_attributes("table", ["class"]);
    builder.add_tag_attributes("img", ["class", "width", "height", "alt", "src", "title"]);
    builder.add_tag_attributes("source", ["srcset", "media", "type", "width", "height"]);
    builder.add_tag_attributes("a", ["class", "role", "id", "href", "title"]);
    builder.add_tag_attributes("sup", ["id"]);
    builder.add_tag_attributes("li", ["id"]);
    builder.link_rel(Some("nofollow"));
    builder
});

/// Regex matching `<a>` tags whose disallowed `href` (`javascript:`, `data:`) was stripped by `ammonia`.
static STRIPPED_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<a rel="nofollow">([^<]*)</a>"#).expect("valid stripped link regex"));

/// Lazily compiled [`Handlebars`] registry holding the default page template.
static HANDLEBARS: LazyLock<Handlebars<'static>> = LazyLock::new(|| {
    let mut reg = Handlebars::new();
    reg.register_template_string("page", PAGE_TEMPLATE)
        .expect("valid embedded page.hbs template");
    reg
});

/// Renders `input` Markdown bytes into a complete, self-contained HTML page.
///
/// `filename` is used as the fallback page title when the document has no
/// top-level `#` heading, and its parent directory is used to resolve relative
/// links and local images.
pub fn parse_content(input: &[u8], filename: &Path, opts: &Options) -> Result<Vec<u8>> {
    let doc = render_document(input, filename, opts)?;
    let toc_open = opts.toc && doc.toc.is_some();

    let ctx = PageContext {
        title: doc.title,
        theme: opts.theme.as_str(),
        is_dark: opts.theme == Theme::Dark,
        css: doc.css,
        styles: doc.styles,
        code_style: doc.code_style,
        toc: doc.toc,
        toc_open,
        body: doc.body,
        katex_css: doc.uses_math.then(katex_css),
        katex_js: doc.uses_math.then(katex_js),
        mermaid_js: doc.uses_mermaid.then(mermaid_js),
    };

    let page = HANDLEBARS
        .render("page", &ctx)
        .context("failed to render page template")?;
    Ok(page.into_bytes())
}

/// Renders `input` Markdown bytes into its structured document components
/// (title, CSS stylesheet, TOC, headings, and sanitized HTML body).
pub(crate) fn render_document(
    input: &[u8],
    filename: &Path,
    opts: &Options,
) -> Result<RenderedDocument> {
    render_document_internal(input, filename, opts, None)
}

/// Renders `input` Markdown bytes into a [`RenderedDocument`] with inline CSS styles on code blocks.
///
/// Used by EPUB output to guarantee syntax highlighting in e-readers that ignore class-based CSS.
pub(crate) fn render_document_epub(
    input: &[u8],
    filename: &Path,
    opts: &Options,
    theme_name: &str,
) -> Result<RenderedDocument> {
    render_document_internal(input, filename, opts, Some(theme_name))
}

fn render_document_internal(
    input: &[u8],
    filename: &Path,
    opts: &Options,
    inline_theme: Option<&str>,
) -> Result<RenderedDocument> {
    let markdown = std::str::from_utf8(input).context("Markdown file is not valid UTF-8")?;
    let (front_matter_html, body_md) = extract_front_matter(markdown);

    let mut rendered = render_markdown_body(body_md, &front_matter_html, inline_theme);

    let filtered = filter_tags(&rendered.html);
    let sanitized = SANITIZER.clean(&filtered).to_string();
    let unwrapped = STRIPPED_LINK_RE.replace_all(&sanitized, "$1");
    let parent_dir = filename.parent().unwrap_or_else(|| Path::new("."));
    rendered.html = fix_links(&unwrapped, parent_dir, opts.served).into_owned();

    let (choices, selected) = code_styles(opts);
    let css = stylesheet(&choices)?;
    let toc_html = render_toc(&rendered.headings);

    let page_title = rendered
        .headings
        .iter()
        .find(|h| h.level == 1 && !h.full_text.is_empty())
        .map(|h| h.full_text.clone())
        .unwrap_or_else(|| {
            filename
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| String::from("mdp"))
        });

    Ok(RenderedDocument {
        title: page_title,
        css,
        styles: choices,
        code_style: selected,
        toc: toc_html,
        headings: rendered.headings,
        body: rendered.html,
        uses_math: rendered.uses_math,
        uses_mermaid: rendered.uses_mermaid,
    })
}

/// Output of the Markdown-to-HTML event pass before page-template wrapping.
struct RenderedBody {
    html: String,
    headings: Vec<HeadingEntry>,
    uses_math: bool,
    uses_mermaid: bool,
}

/// Converts a heading level enum to its `1..=6` numeric depth.
fn level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Runs [`pulldown_cmark`] over `body_md`, transforming GFM extensions, code
/// blocks, headings, math, Mermaid diagrams, and footnotes into an HTML string.
fn render_markdown_body(
    body_md: &str,
    front_matter_html: &str,
    inline_theme: Option<&str>,
) -> RenderedBody {
    let mut cmark_opts = CmarkOptions::empty();
    cmark_opts.insert(CmarkOptions::ENABLE_TABLES);
    cmark_opts.insert(CmarkOptions::ENABLE_FOOTNOTES);
    cmark_opts.insert(CmarkOptions::ENABLE_STRIKETHROUGH);
    cmark_opts.insert(CmarkOptions::ENABLE_TASKLISTS);
    cmark_opts.insert(CmarkOptions::ENABLE_GFM);
    cmark_opts.insert(CmarkOptions::ENABLE_MATH);

    let raw_events: Vec<(Event<'_>, std::ops::Range<usize>)> =
        Parser::new_ext(body_md, cmark_opts)
            .into_offset_iter()
            .collect();
    let events = coalesce_backtick_math(body_md, raw_events);

    let mut out_events: Vec<Event<'_>> = Vec::new();
    let mut headings: Vec<HeadingEntry> = Vec::new();
    let mut seen_ids: HashMap<String, usize> = HashMap::new();
    let mut uses_math = false;
    let mut uses_mermaid = false;

    // Heading capture state
    let mut in_heading: Option<(HeadingLevel, Vec<Event<'_>>, String, String)> = None;

    // Code block capture state: Some((info_string, accumulated_code))
    let mut in_code_block: Option<(String, String)> = None;

    // Table alignment state: tracks column alignments and whether we're in <thead>
    let mut table_alignments: Option<Vec<Alignment>> = None;
    let mut in_table_head = false;
    let mut table_col_idx: usize = 0;

    // Footnote state: maps footnote label -> 1-based number, and stores definition events
    let mut footnote_numbers: HashMap<String, usize> = HashMap::new();
    let mut footnote_order: Vec<String> = Vec::new();
    let mut in_footnote_def: Option<(String, Vec<Event<'_>>)> = None;
    let mut footnote_defs: HashMap<String, Vec<Event<'_>>> = HashMap::new();
    let mut in_link_depth: usize = 0;

    for (event, range) in events {
        // If inside a fenced/indented code block, accumulate text until End(CodeBlock)
        if let Some((ref lang, ref mut code_buf)) = in_code_block {
            match event {
                Event::End(TagEnd::CodeBlock) => {
                    let lang_trimmed = lang.trim();
                    let rendered_block = if lang_trimmed.eq_ignore_ascii_case("math") {
                        uses_math = true;
                        format!(
                            "<div class=\"math math-display\">{}</div>\n",
                            escape_html(code_buf)
                        )
                    } else if lang_trimmed.eq_ignore_ascii_case("mermaid") {
                        uses_mermaid = true;
                        format!("<pre class=\"mermaid\">{}</pre>\n", escape_html(code_buf))
                    } else if let Some(th) = inline_theme {
                        highlight_code_inline(code_buf, lang_trimmed, th)
                    } else {
                        highlight_code(code_buf, lang_trimmed)
                    };
                    in_code_block = None;
                    push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(rendered_block)));
                }
                Event::Text(t) => code_buf.push_str(&t),
                _ => {}
            }
            continue;
        }

        // If inside a heading, buffer inner events so we can compute the GitHub slug ID first
        if let Some((level, ref mut h_events, ref mut full_text, ref mut toc_text)) = in_heading {
            match event {
                Event::End(TagEnd::Heading(_)) => {
                    let lvl = level_to_u8(level);
                    let base = slug(full_text.trim());
                    let mut id = base.clone();
                    while seen_ids.get(&id).copied().unwrap_or(0) > 0 {
                        let n = *seen_ids.entry(base.clone()).or_insert(0);
                        id = format!("{base}-{n}");
                        *seen_ids.get_mut(&base).unwrap() += 1;
                    }
                    *seen_ids.entry(id.clone()).or_insert(0) += 1;

                    let mut inner_html = String::new();
                    pulldown_cmark::html::push_html(&mut inner_html, h_events.drain(..));
                    let heading_html = format!("<h{lvl} id=\"{id}\">{inner_html}</h{lvl}>\n");

                    headings.push(HeadingEntry {
                        level: lvl,
                        id,
                        toc_title: toc_text.trim().to_string(),
                        full_text: full_text.trim().to_string(),
                    });
                    in_heading = None;
                    push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(heading_html)));
                }
                Event::Text(t) => {
                    let replaced = replace_emojis(&t).into_owned();
                    full_text.push_str(&replaced);
                    toc_text.push_str(&replaced);
                    h_events.push(Event::Text(CowStr::from(replaced)));
                }
                Event::Code(c) => {
                    full_text.push_str(&c);
                    toc_text.push_str(&c);
                    h_events.push(Event::Code(c));
                }
                Event::InlineMath(m) => {
                    uses_math = true;
                    let trimmed = m.trim();
                    full_text.push_str(trimmed);
                    let span = format!(
                        r#"<span class="math math-inline">{}</span>"#,
                        escape_html(trimmed)
                    );
                    h_events.push(Event::Html(CowStr::from(span)));
                }
                Event::DisplayMath(m) => {
                    uses_math = true;
                    let trimmed = m.trim();
                    full_text.push_str(trimmed);
                    let span = format!(
                        r#"<span class="math math-display">{}</span>"#,
                        escape_html(trimmed)
                    );
                    h_events.push(Event::Html(CowStr::from(span)));
                }
                other => h_events.push(other),
            }
            continue;
        }

        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                in_heading = Some((level, Vec::new(), String::new(), String::new()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.into_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                in_code_block = Some((lang, String::new()));
            }
            Event::InlineMath(m) => {
                let followed_by_digit =
                    range.end < body_md.len() && body_md.as_bytes()[range.end].is_ascii_digit();
                if followed_by_digit
                    || m.is_empty()
                    || m.starts_with(' ')
                    || m.ends_with(' ')
                    || m.contains('\n')
                {
                    // Preserve strict GitHub single-$ rules when spaces, newlines, or trailing digits appear
                    push_event(
                        &mut out_events,
                        &mut in_footnote_def,
                        Event::Text(CowStr::from(format!("${m}$"))),
                    );
                } else {
                    uses_math = true;
                    let trimmed = m.trim();
                    let inner = if trimmed.starts_with('`')
                        && trimmed.ends_with('`')
                        && trimmed.len() >= 2
                    {
                        trimmed[1..trimmed.len() - 1].trim()
                    } else {
                        trimmed
                    };
                    let span = format!(
                        r#"<span class="math math-inline">{}</span>"#,
                        escape_html(inner)
                    );
                    push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(span)));
                }
            }
            Event::DisplayMath(m) => {
                let trimmed = m.trim();
                if trimmed.is_empty() {
                    push_event(
                        &mut out_events,
                        &mut in_footnote_def,
                        Event::Text(CowStr::from("$$")),
                    );
                } else {
                    uses_math = true;
                    let span = format!(
                        r#"<span class="math math-display">{}</span>"#,
                        escape_html(trimmed)
                    );
                    push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(span)));
                }
            }
            Event::Start(Tag::BlockQuote(Some(kind))) => {
                let (cls, title) = alert_meta(kind);
                let html = format!(
                    "<blockquote class=\"markdown-alert markdown-alert-{cls}\"><p class=\"markdown-alert-title\">{title}</p>\n"
                );
                push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(html)));
            }
            Event::End(TagEnd::BlockQuote(Some(_))) => {
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("</blockquote>\n")),
                );
            }
            Event::Start(Tag::Table(aligns)) => {
                table_alignments = Some(aligns);
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("<table>\n")),
                );
            }
            Event::End(TagEnd::Table) => {
                table_alignments = None;
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("</tbody>\n</table>\n")),
                );
            }
            Event::Start(Tag::TableHead) => {
                in_table_head = true;
                table_col_idx = 0;
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("<thead>\n<tr>\n")),
                );
            }
            Event::End(TagEnd::TableHead) => {
                in_table_head = false;
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("</tr>\n</thead>\n<tbody>\n")),
                );
            }
            Event::Start(Tag::TableRow) => {
                table_col_idx = 0;
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("<tr>\n")),
                );
            }
            Event::End(TagEnd::TableRow) => {
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from("</tr>\n")),
                );
            }
            Event::Start(Tag::TableCell) => {
                let tag = if in_table_head { "th" } else { "td" };
                let align_attr = table_alignments
                    .as_ref()
                    .and_then(|a| a.get(table_col_idx))
                    .map(|a| match a {
                        Alignment::None => "",
                        Alignment::Left => r#" align="left""#,
                        Alignment::Center => r#" align="center""#,
                        Alignment::Right => r#" align="right""#,
                    })
                    .unwrap_or("");
                table_col_idx += 1;
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from(format!("<{tag}{align_attr}>"))),
                );
            }
            Event::End(TagEnd::TableCell) => {
                let tag = if in_table_head { "th" } else { "td" };
                push_event(
                    &mut out_events,
                    &mut in_footnote_def,
                    Event::Html(CowStr::from(format!("</{tag}>\n"))),
                );
            }
            Event::FootnoteReference(name) => {
                let key = name.to_string();
                let num = if let Some(&n) = footnote_numbers.get(&key) {
                    n
                } else {
                    let n = footnote_numbers.len() + 1;
                    footnote_numbers.insert(key.clone(), n);
                    footnote_order.push(key);
                    n
                };
                let html = format!(
                    r##"<sup id="fnref:{num}"><a href="#fn:{num}" class="footnote-ref" role="doc-noteref">{num}</a></sup>"##
                );
                push_event(&mut out_events, &mut in_footnote_def, Event::Html(CowStr::from(html)));
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                in_footnote_def = Some((name.to_string(), Vec::new()));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                if let Some((name, def_events)) = in_footnote_def.take() {
                    footnote_defs.insert(name, def_events);
                }
            }
            Event::Start(Tag::Link { .. }) => {
                in_link_depth += 1;
                push_event(&mut out_events, &mut in_footnote_def, event);
            }
            Event::End(TagEnd::Link) => {
                in_link_depth = in_link_depth.saturating_sub(1);
                push_event(&mut out_events, &mut in_footnote_def, event);
            }
            Event::Text(t) => {
                let replaced = replace_emojis(&t).into_owned();
                if in_link_depth == 0
                    && let Some(linked_html) = autolink_gfm(&replaced)
                {
                    push_event(
                        &mut out_events,
                        &mut in_footnote_def,
                        Event::Html(CowStr::from(linked_html)),
                    );
                } else {
                    push_event(
                        &mut out_events,
                        &mut in_footnote_def,
                        Event::Text(CowStr::from(replaced)),
                    );
                }
            }
            other => push_event(&mut out_events, &mut in_footnote_def, other),
        }
    }

    let mut html_out = String::with_capacity(body_md.len() + front_matter_html.len() + 256);
    html_out.push_str(front_matter_html);
    pulldown_cmark::html::push_html(&mut html_out, out_events.into_iter());

    // Remove empty `<tbody>\n</tbody>` generated for header-only tables
    html_out = html_out.replace("<tbody>\n</tbody>\n", "");

    // Render collected footnotes at the bottom of the article in reference order
    if !footnote_order.is_empty() {
        html_out.push_str("<div class=\"footnotes\" role=\"doc-endnotes\">\n<hr>\n<ol>\n");
        for name in &footnote_order {
            let num = footnote_numbers[name];
            let mut def_html = String::new();
            if let Some(events) = footnote_defs.remove(name) {
                pulldown_cmark::html::push_html(&mut def_html, events.into_iter());
            }
            let backref = format!(
                "\u{00a0}<a href=\"#fnref:{num}\" class=\"footnote-backref\" role=\"doc-backlink\">\u{21a9}\u{fe0e}</a>"
            );
            let trimmed = def_html.trim_end();
            if let Some(before_p) = trimmed.strip_suffix("</p>") {
                let _ = write!(html_out, "<li id=\"fn:{num}\">\n{before_p}{backref}</p>\n</li>\n");
            } else {
                let _ = write!(html_out, "<li id=\"fn:{num}\">\n{def_html}{backref}\n</li>\n");
            }
        }
        html_out.push_str("</ol>\n</div>\n");
    }

    RenderedBody {
        html: html_out,
        headings,
        uses_math,
        uses_mermaid,
    }
}

/// Converts GitHub's backtick inline math syntax (`` $`...`$ ``) from a
/// `[Text("...$"), Code("..."), Text("$...")]` sequence into inline math HTML.
fn coalesce_backtick_math<'a>(
    body_md: &str,
    mut raw: Vec<(Event<'a>, std::ops::Range<usize>)>,
) -> Vec<(Event<'a>, std::ops::Range<usize>)> {
    let bytes = body_md.as_bytes();
    let len = raw.len();
    let mut i = 0;
    while i + 2 < len {
        let code_range = raw[i + 1].1.clone();
        let prev_end = raw[i].1.end;
        let next_start = raw[i + 2].1.start;

        let is_unquoted_dollar_before = prev_end == code_range.start
            && code_range.start >= 1
            && bytes[code_range.start - 1] == b'$'
            && (code_range.start < 2 || bytes[code_range.start - 2] != b'\\');
        let is_dollar_after = next_start == code_range.end
            && code_range.end < bytes.len()
            && bytes[code_range.end] == b'$';

        if is_unquoted_dollar_before
            && is_dollar_after
            && let Event::Text(ref prev_text) = raw[i].0
            && prev_text.ends_with('$')
            && let Event::Code(ref code_text) = raw[i + 1].0
            && let Event::Text(ref next_text) = raw[i + 2].0
            && next_text.starts_with('$')
        {
            let new_prev = prev_text[..prev_text.len() - 1].to_string();
            let math_str = code_text.trim().to_string();
            let new_next = next_text[1..].to_string();

            raw[i].0 = Event::Text(CowStr::from(new_prev));
            raw[i + 1].0 = Event::InlineMath(CowStr::from(math_str));
            raw[i + 2].0 = Event::Text(CowStr::from(new_next));
            i += 3;
            continue;
        }
        i += 1;
    }
    raw
}

/// Pushes an event either to the active footnote definition buffer or to the main event list.
fn push_event<'a>(
    out_events: &mut Vec<Event<'a>>,
    in_footnote_def: &mut Option<(String, Vec<Event<'a>>)>,
    event: Event<'a>,
) {
    if let Some((_, ref mut def_events)) = *in_footnote_def {
        def_events.push(event);
    } else {
        out_events.push(event);
    }
}

/// Converts heading text into a GitHub-compatible anchor slug:
/// lowercase, spaces become `-`, punctuation and emoji are removed, and Unicode
/// letters, marks, numbers, `_`, and `-` are preserved.
///
/// # Examples
///
/// ```
/// use mdp::slug;
///
/// assert_eq!(slug("Hello, World!"), "hello-world");
/// assert_eq!(slug("Rust's Option<T>: 100% safe?"), "rusts-optiont-100-safe");
/// assert_eq!(slug("日本語の見出し"), "日本語の見出し");
/// ```
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch == ' ' {
            out.push('-');
        } else if ch == '-' || ch == '_' || ch.is_alphabetic() || is_non_emoji_number_or_mark(ch) {
            out.push(ch);
        }
    }
    out
}

/// Returns `true` for Unicode numbers and combining marks while excluding symbol-like emoji.
fn is_non_emoji_number_or_mark(ch: char) -> bool {
    ch.is_ascii_digit()
        || ('\u{00B2}'..='\u{00B3}').contains(&ch)
        || ch == '\u{00B9}'
        || ('\u{0300}'..='\u{036F}').contains(&ch)
        || ('\u{0660}'..='\u{0669}').contains(&ch)
        || ('\u{06F0}'..='\u{06F9}').contains(&ch)
        || ('\u{0966}'..='\u{096F}').contains(&ch)
}

/// Renders a nested HTML `<ul>` table of contents for `headings`, or `None` if
/// no headings remain after skipping a lone top-level `#` title heading.
fn render_toc(headings: &[HeadingEntry]) -> Option<String> {
    let h1_count = headings.iter().filter(|h| h.level == 1).count();
    let min_level = if h1_count == 1 { 2 } else { 1 };

    let filtered: Vec<&HeadingEntry> = headings
        .iter()
        .filter(|h| h.level >= min_level)
        .collect();
    if filtered.is_empty() {
        return None;
    }

    let mut html = String::new();
    let mut stack: Vec<u8> = Vec::new();

    for h in filtered {
        let lvl = h.level;
        if stack.is_empty() {
            html.push_str("<ul>\n<li>\n");
            stack.push(lvl);
        } else if lvl > *stack.last().unwrap() {
            html.push_str("<ul>\n<li>\n");
            stack.push(lvl);
        } else {
            while stack.len() > 1 && *stack.last().unwrap() > lvl {
                stack.pop();
                html.push_str("</li>\n</ul>\n");
            }
            if let Some(last) = stack.last_mut() {
                *last = lvl;
            }
            html.push_str("</li>\n<li>\n");
        }

        let encoded_id = utf8_percent_encode(&h.id, FRAGMENT_ENCODE_SET);
        let _ = write!(
            html,
            r##"<a href="#{encoded_id}" rel="nofollow">{}</a>"##,
            escape_html(&h.toc_title)
        );
    }

    while !stack.is_empty() {
        stack.pop();
        html.push_str("</li>\n</ul>\n");
    }

    // Keep track of used imports for compiler lint cleanliness
    let _ = HashSet::<u8>::new;
    Some(html)
}
