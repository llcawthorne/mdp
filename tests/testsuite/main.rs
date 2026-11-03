//! Integration test suite for `mdp`.
//!
//! Taking a cue from `mdBook`, all integration test modules are consolidated
//! under `tests/testsuite/main.rs` so Cargo links a single test binary instead
//! of linking one executable per `tests/*.rs` file.

mod chrome;
mod cli;
mod github;
mod golden;
mod watch;

/// Extracts the `<title>`, `<nav>`, and `<article>` sections of a rendered
/// HTML page, separated by newlines, so tests can assert against the
/// Markdown-derived structure independent of embedded CSS or JS assets.
pub fn markdown_parts(page: &[u8]) -> String {
    let text = String::from_utf8_lossy(page);
    let mut out = String::new();
    for (open, close) in [
        ("<title>", "</title>"),
        ("<nav", "</nav>"),
        ("<article", "</article>"),
    ] {
        if let Some(start) = text.find(open)
            && let Some(rel_end) = text[start..].find(close)
        {
            let end = start + rel_end + close.len();
            out.push_str(&text[start..end]);
            out.push('\n');
        }
    }
    out
}
