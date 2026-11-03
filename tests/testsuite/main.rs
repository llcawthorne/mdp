//! Integration test suite for `mdp`.
//!
//! Taking a cue from `mdBook`, all integration test modules are consolidated
//! under `tests/testsuite/main.rs` so Cargo links a single test binary instead
//! of linking one executable per `tests/*.rs` file.
//!
//! Rust vs. Go testing note:
//! - In Go (`mdpgo`), `_test.go` files in the root directory share `package main`
//!   and have access to private functions.
//! - In Rust, integration tests under `tests/` compile as a **separate crate**
//!   that links against the public API of the `mdp` library crate (`src/lib.rs`),
//!   verifying that the public API is ergonomic and self-sufficient.

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
        // Notice `&text[start..end]`: because `find()` returns byte offsets at
        // valid UTF-8 character boundaries (ASCII tag delimiters), slicing
        // `&text[start..]` and `&text[start..end]` is O(1) and guaranteed not
        // to panic.
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
