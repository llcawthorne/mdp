//! Compile-time embedded client-side libraries (KaTeX and Mermaid).
//!
//! Files in `assets/` are baked into the compiled binary via `include_str!` and
//! `include_bytes!`. Because base64-encoding the 20 KaTeX `.woff2` font files
//! into `katex.min.css` is non-trivial work, [`katex_css`] uses
//! [`std::sync::LazyLock`] so the stylesheet is assembled only once on first
//! use when a document actually contains math.
//!
//! # Stepping Stone to `mdBook`
//!
//! In `mdBook`, check out `crates/mdbook-html/src/html_handlebars/hbs_renderer.rs`
//! and `crates/mdbook-driver/src/init.rs`: `mdBook` uses the exact same
//! `include_str!` and `include_bytes!` macros to bake its default Handlebars
//! template (`index.hbs`), stylesheets (`general.css`, `chrome.css`), FontAwesome
//! icons, and JavaScript (`book.js`) into the binary, and uses [`std::sync::LazyLock`]
//! across all workspace crates for compiled regular expressions and static tables.

// Rust trait import note (`use base64::Engine`):
//   In Rust, calling a trait method (`BASE64.encode(...)`) requires the trait
//   itself (`base64::Engine`) to be in scope, even when you already have the
//   concrete struct (`STANDARD`) in scope. This prevents method-name collisions
//   when multiple crates implement extension traits on foreign types.
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use regex::{Captures, Regex};
use std::sync::LazyLock;

// Go vs. Rust (`include_str!` / `include_bytes!` vs. Go's `//go:embed`):
//   In `mdpgo/assets.go`, `//go:embed assets/katex assets/mermaid` embeds an
//   `embed.FS` virtual filesystem that is queried by string path at runtime
//   (`assets.ReadFile("assets/katex/fonts/" + m[1] + ".woff2")`).
//   In Rust, `include_str!("...")` and `include_bytes!("...")` are built-in
//   compiler macros that resolve each file at *compile time* and place its
//   bytes directly into the binary's read-only data segment (`.rodata`) as a
//   `&'static str` or `&'static [u8]`. If a file is missing or invalid UTF-8,
//   compilation fails immediately rather than returning an error at runtime.

/// Raw `katex.min.css` before font inlining.
const KATEX_RAW_CSS: &str = include_str!("../assets/katex/katex.min.css");

/// Embedded `katex.min.js` bundle (`katex@0.16.11`).
const KATEX_JS: &str = include_str!("../assets/katex/katex.min.js");

/// Embedded `mermaid.min.js` bundle (`mermaid@11.4.1`).
const MERMAID_JS: &str = include_str!("../assets/mermaid/mermaid.min.js");

/// Look up an embedded KaTeX `.woff2` font by its stem name.
///
/// The return type `&'static [u8]` is a fat pointer (pointer + length) into the
/// binary's read-only data segment—valid for the entire `'static` lifetime of
/// the running process with zero heap allocation.
fn katex_font_bytes(name: &str) -> &'static [u8] {
    match name {
        "KaTeX_AMS-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_AMS-Regular.woff2"),
        "KaTeX_Caligraphic-Bold" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Caligraphic-Bold.woff2")
        }
        "KaTeX_Caligraphic-Regular" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Caligraphic-Regular.woff2")
        }
        "KaTeX_Fraktur-Bold" => include_bytes!("../assets/katex/fonts/KaTeX_Fraktur-Bold.woff2"),
        "KaTeX_Fraktur-Regular" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Fraktur-Regular.woff2")
        }
        "KaTeX_Main-Bold" => include_bytes!("../assets/katex/fonts/KaTeX_Main-Bold.woff2"),
        "KaTeX_Main-BoldItalic" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Main-BoldItalic.woff2")
        }
        "KaTeX_Main-Italic" => include_bytes!("../assets/katex/fonts/KaTeX_Main-Italic.woff2"),
        "KaTeX_Main-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_Main-Regular.woff2"),
        "KaTeX_Math-BoldItalic" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Math-BoldItalic.woff2")
        }
        "KaTeX_Math-Italic" => include_bytes!("../assets/katex/fonts/KaTeX_Math-Italic.woff2"),
        "KaTeX_SansSerif-Bold" => {
            include_bytes!("../assets/katex/fonts/KaTeX_SansSerif-Bold.woff2")
        }
        "KaTeX_SansSerif-Italic" => {
            include_bytes!("../assets/katex/fonts/KaTeX_SansSerif-Italic.woff2")
        }
        "KaTeX_SansSerif-Regular" => {
            include_bytes!("../assets/katex/fonts/KaTeX_SansSerif-Regular.woff2")
        }
        "KaTeX_Script-Regular" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Script-Regular.woff2")
        }
        "KaTeX_Size1-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_Size1-Regular.woff2"),
        "KaTeX_Size2-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_Size2-Regular.woff2"),
        "KaTeX_Size3-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_Size3-Regular.woff2"),
        "KaTeX_Size4-Regular" => include_bytes!("../assets/katex/fonts/KaTeX_Size4-Regular.woff2"),
        "KaTeX_Typewriter-Regular" => {
            include_bytes!("../assets/katex/fonts/KaTeX_Typewriter-Regular.woff2")
        }
        other => panic!("unknown embedded KaTeX font: {other}"),
    }
}

// Go vs. Rust (`std::sync::LazyLock<T>` vs. Go's `sync.OnceValue`):
//   In `mdpgo/assets.go`, `katexCSS` is a `var katexCSS = sync.OnceValue(func() string { ... })`
//   function closure that returns a `string` by value.
//   In Rust, `static` items must be initialized with compile-time constants, so
//   heap-allocated or computed globals use the smart pointer `std::sync::LazyLock<T>`.
//   - Unlike Java's `static` initializer blocks (which run eagerly when the class
//     loads), `KATEX_CSS` is only computed the first time it is dereferenced.
//   - Because `LazyLock<String>` implements `Deref<Target = String>` (and `String`
//     derefs to `str`), `&KATEX_CSS` in `katex_css()` below coerces directly to
//     `&'static str` via **deref coercion**!
/// Lazily computed KaTeX stylesheet with all `.woff2` fonts inlined as `data:` URIs.
static KATEX_CSS: LazyLock<String> = LazyLock::new(|| {
    let re = Regex::new(r#"url\(fonts/(KaTeX_[\w-]+)\.woff2\) format\("woff2"\)[^;}]*"#)
        .expect("valid KaTeX font regex");
    re.replace_all(KATEX_RAW_CSS, |caps: &Captures<'_>| {
        let font_b64 = BASE64.encode(katex_font_bytes(&caps[1]));
        format!(r#"url(data:font/woff2;base64,{font_b64}) format("woff2")"#)
    })
    .into_owned()
});

/// Returns the KaTeX CSS stylesheet with all WOFF2 fonts embedded as base64
/// `data:` URIs so math renders offline and in `file://` pages and PDFs.
pub fn katex_css() -> &'static str {
    &KATEX_CSS
}

/// Returns the embedded `katex.min.js` script bundle.
pub fn katex_js() -> &'static str {
    KATEX_JS
}

/// Returns the embedded `mermaid.min.js` script bundle.
pub fn mermaid_js() -> &'static str {
    MERMAID_JS
}
