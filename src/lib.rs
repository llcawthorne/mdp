//! `mdp` renders Markdown files as self-contained, GitHub-styled HTML pages,
//! PDFs, or EPUBs and opens them in a browser or viewer.
//!
//! # Architecture
//!
//! Taking cues from [`mdBook`](https://github.com/rust-lang/mdBook), `mdp` is
//! structured as a library crate (`mdp`) paired with a thin CLI entry point
//! (`src/main.rs`) and a single-binary integration test suite (`tests/testsuite/main.rs`):
//!
//! - [`assets`]: Compile-time embedded KaTeX (CSS, JS, and 20 base64-inlined WOFF2
//!   fonts) and Mermaid JS assets.
//! - [`style`]: Page color themes ([`Theme`]), curated [`syntect`] syntax
//!   highlighting styles ([`CodeStyle`]), and scoped CSS generation.
//! - [`github`]: GitHub Flavored Markdown extensions beyond `pulldown-cmark`
//!   (YAML front matter tables, `:emoji:` shortcodes, blockquote alerts, and
//!   GFM's disallowed raw HTML `tagfilter`).
//! - [`links`]: HTML post-processing to inline local images as `data:` URIs,
//!   tag `#gh-dark-mode-only` / `#gh-light-mode-only` images, and resolve
//!   relative links to `file://` URLs.
//! - [`render`]: Streaming [`pulldown_cmark`] rendering pipeline, [`ammonia`]
//!   HTML sanitization, GitHub-compatible heading [`slug`] generation, slide-out
//!   table of contents generation, and [`handlebars`] template rendering.
//! - [`pdf`]: Headless Chromium PDF export with document outlines and OS dark-mode
//!   detection.
//! - [`epub`]: EPUB 3 e-book packaging with XHTML content, embedded styles, and
//!   navigation table of contents.
//! - [`watch`]: Live-reloading local HTTP server (`--watch`) built on [`axum`]
//!   and [`tokio`] with Server-Sent Events (SSE).

#![warn(missing_docs)]

// Rust module system note (`pub mod` vs. Go's flat `package main`):
//   In Go, all `.go` files in a directory share a single flat namespace.
//   In Rust, each file (`src/assets.rs`, `src/style.rs`, etc.) is a distinct
//   child module explicitly declared here in the crate root (`src/lib.rs`).
//   Re-exporting core items at the crate root (`pub use render::{parse_content, slug};`)
//   gives external callers a flat, ergonomic public API (`mdp::parse_content`)
//   while keeping the internal implementation modular.
pub mod assets;
pub mod epub;
pub mod github;
pub mod links;
pub mod pdf;
pub mod render;
pub mod style;
pub mod watch;

pub use render::{parse_content, slug};
pub use style::{CodeStyle, DEFAULT_STYLE, Theme};

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// Rendering options controlling page appearance, syntax highlighting, and link resolution.
///
/// Deriving `Default` lets callers and tests use struct update syntax:
/// `Options { toc: true, ..Options::default() }`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Start with the table of contents drawer open (`--toc` / `-t`).
    pub toc: bool,
    /// Page color scheme (`Auto`, `Light`, or `Dark`).
    pub theme: Theme,
    /// Syntax highlighting style name for light mode.
    pub light_style: String,
    /// Syntax highlighting style name for dark mode.
    pub dark_style: String,
    /// Whether the page is being served over HTTP (`--watch`), keeping relative
    /// links intact instead of converting them to `file://` URLs.
    pub served: bool,
}

/// Complete configuration parsed from command-line arguments.
///
/// Notice two idiomatic Rust differences compared to the Go version's `config`:
/// 1. File paths use [`PathBuf`] (an owned, OS-native path buffer that handles
///    platform path separators and non-UTF-8 paths) rather than plain `String`.
/// 2. Optional values use `Option<PathBuf>` instead of a sentinel empty string
///    `""`, making "no output file specified" explicit in the type system.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    /// Path to the input Markdown file.
    pub filename: PathBuf,
    /// Skip opening the rendered output in the browser/viewer (`--skip` / `-s`).
    pub skip_preview: bool,
    /// Serve a live-reloading local HTTP preview (`--watch` / `-w`).
    pub watch: bool,
    /// Explicit output file path (`--output` / `-o`), or `None` for a temporary file
    /// (HTML) or sibling `.pdf` / `.epub` file.
    pub output: Option<PathBuf>,
    /// Export a PDF using headless Chrome (`--pdf` / `-p`, or `-o *.pdf`).
    pub pdf: bool,
    /// Export an EPUB e-book (`--epub` / `-e`, or `-o *.epub`).
    pub epub: bool,
    /// Rendering options passed to [`parse_content`].
    pub opts: Options,
}

/// Command-line interface definition powered by [`clap`].
///
/// `#[derive(Parser)]` is a procedural macro that inspects the struct fields,
/// types (`bool`, `Option<PathBuf>`, `Vec<PathBuf>`), and `///` doc comments at
/// compile time to generate both the argument parser and `--help` output.
#[derive(Debug, Parser)]
#[command(
    name = "mdp",
    about = "Render Markdown files as GitHub-styled HTML pages, PDFs, or EPUBs",
    version
)]
struct Cli {
    /// Skip auto-preview; just print the file path or URL.
    #[arg(short = 's', long = "skip")]
    skip: bool,

    /// Open the table of contents drawer by default.
    #[arg(short = 't', long = "toc")]
    toc: bool,

    /// Watch the file and reload the preview when it changes.
    #[arg(short = 'w', long = "watch")]
    watch: bool,

    /// Write to FILE instead of a temp file (a .pdf or .epub name implies --pdf or --epub).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Make a PDF using headless Chrome; goes beside FILE unless -o says otherwise.
    #[arg(short = 'p', long = "pdf")]
    pdf: bool,

    /// Make an EPUB e-book; goes beside FILE unless -o says otherwise.
    #[arg(short = 'e', long = "epub")]
    epub: bool,

    /// Use dark colors instead of following the OS.
    #[arg(short = 'd', long = "dark")]
    dark: bool,

    /// Use light colors instead of following the OS.
    #[arg(short = 'l', long = "light")]
    light: bool,

    /// Code highlighting style (github, solarized, ocean, darcula, railscasts, mocha).
    #[arg(long = "style", default_value = DEFAULT_STYLE, value_name = "STYLE")]
    style: String,

    /// Code highlighting style for dark mode only.
    #[arg(long = "dark-style", value_name = "STYLE")]
    dark_style: Option<String>,

    /// Markdown file to preview or convert.
    #[arg(value_name = "FILE")]
    files: Vec<PathBuf>,
}

/// Renders the complete `--help` message into a string.
pub fn render_help() -> String {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    cmd.render_help().to_string()
}

/// Parses command-line arguments (excluding `argv[0]`) into a validated [`Config`].
///
/// Writes help or usage text to `stderr` when `--help` / `--version` is requested
/// or when positional arguments are missing.
///
/// # Rust Generics & Trait Objects Note
///
/// - `I: IntoIterator<Item = T>, T: Into<OsString> + Clone`: Accepting any
///   iterable of items convertible into `OsString` lets `main` pass
///   `env::args_os().skip(1)` while unit tests pass a simple array literal like
///   `["-swl", "a.md"]` without converting strings manually!
/// - `stderr: &mut dyn Write`: A mutable reference to a **trait object**
///   (dynamic dispatch via a vtable, analogous to Go's `io.Writer` or Java's
///   `OutputStream`). In production `main` passes a `Vec<u8>` buffer or
///   `io::stderr()`; tests pass `&mut Vec::new()`.
pub fn parse_args<I, T>(args: I, stderr: &mut dyn Write) -> Result<Config>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args_vec: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if args_vec.is_empty() {
        let help = render_help();
        let _ = write!(stderr, "{help}");
        bail!("{help}");
    }

    let full_args = std::iter::once(OsString::from("mdp")).chain(args_vec);

    let cli = match Cli::try_parse_from(full_args) {
        Ok(c) => c,
        Err(err) => {
            let _ = write!(stderr, "{err}");
            // `anyhow::bail!(...)` is shorthand for `return Err(anyhow::anyhow!(...))`.
            bail!("{err}");
        }
    };

    if cli.files.len() != 1 {
        bail!("expected exactly one Markdown file");
    }

    let mut pdf = cli.pdf;
    let mut epub = cli.epub;
    // Rust 2024 `if let ... && let ...` chain: combines pattern matching
    // (`Some(ref out_path)` and `Some(ext)`) in a single flat `if` condition.
    // Note `ref out_path`: borrows the `PathBuf` inside `cli.output` rather than
    // moving (consuming) it, because `cli.output` is moved into `Config` at the end.
    if let Some(ref out_path) = cli.output
        && let Some(ext) = out_path.extension().and_then(|e| e.to_str())
    {
        if ext.eq_ignore_ascii_case("pdf") {
            pdf = true;
        } else if ext.eq_ignore_ascii_case("epub") {
            epub = true;
        }
    }

    if pdf && epub {
        bail!("--pdf and --epub can't be used together");
    }
    if cli.watch && (pdf || epub || cli.output.is_some()) {
        bail!("--watch can't be used with --output, --pdf, or --epub");
    }
    if cli.dark && cli.light {
        bail!("--dark and --light can't be used together");
    }
    if !style::known_style(&cli.style) {
        bail!(
            "unknown style {:?}; valid styles: {}",
            cli.style,
            style::available_styles().join(", ")
        );
    }
    if let Some(ref ds) = cli.dark_style
        && !style::known_style(ds)
    {
        bail!(
            "unknown style {:?}; valid styles: {}",
            ds,
            style::available_styles().join(", ")
        );
    }

    let theme = if cli.dark {
        Theme::Dark
    } else if cli.light {
        Theme::Light
    } else {
        Theme::Auto
    };

    let (light_style, mut dark_style) = style::styles_for(&cli.style);
    if let Some(ds) = cli.dark_style {
        dark_style = style::resolve_theme_name(&ds, true);
    }

    Ok(Config {
        // `into_iter()` consumes `cli.files` by value, moving the single `PathBuf`
        // out of the vector without cloning it.
        filename: cli.files.into_iter().next().unwrap(),
        skip_preview: cli.skip,
        watch: cli.watch,
        output: cli.output,
        pdf,
        epub,
        opts: Options {
            toc: cli.toc,
            theme,
            light_style,
            dark_style,
            served: false,
        },
    })
}

/// Executes the one-shot HTML, PDF, or EPUB rendering pipeline for `cfg` and
/// writes the resulting output file path to `out`.
pub fn run(cfg: &Config, out: &mut dyn Write) -> Result<()> {
    // `.with_context(|| ...)` from `anyhow::Context` attaches a lazily evaluated
    // error message only if `fs::read` returns `Err`, avoiding `format!` string
    // allocation on the happy path.
    let input = fs::read(&cfg.filename)
        .with_context(|| format!("failed to read {}", cfg.filename.display()))?;

    if cfg.epub {
        return epub::run_epub(cfg, &input, out);
    }

    let mut opts = cfg.opts.clone();
    if cfg.pdf {
        opts.theme = pdf::pdf_theme(opts.theme);
    }

    let html_data = parse_content(&input, &cfg.filename, &opts)?;

    if cfg.pdf {
        return pdf::run_pdf(cfg, &html_data, out);
    }

    // Ownership & RAII note (`tempfile::NamedTempFile::keep`):
    //   By default, `NamedTempFile` deletes its underlying file as soon as it is
    //   dropped at the end of the block. Because we want the file to stay on
    //   disk when `cfg.skip_preview` is true (so the caller can inspect the path
    //   printed to `stdout`), calling `.keep()` disarms the automatic RAII
    //   deletion and returns the owned `(File, PathBuf)`. When `!cfg.skip_preview`,
    //   we explicitly remove the temp file after `preview` returns.
    let (out_path, is_temp) = match cfg.output {
        Some(ref path) => (path.clone(), false),
        None => {
            let temp = tempfile::Builder::new()
                .prefix("mdp")
                .suffix(".html")
                .tempfile()
                .context("failed to create temporary HTML file")?;
            let (_, path) = temp
                .keep()
                .context("failed to persist temporary HTML file")?;
            (path, true)
        }
    };

    writeln!(out, "{}", out_path.display())?;
    fs::write(&out_path, &html_data)
        .with_context(|| format!("failed to write {}", out_path.display()))?;

    if cfg.skip_preview {
        return Ok(());
    }

    let res = preview(&out_path.to_string_lossy());
    if is_temp {
        let _ = fs::remove_file(&out_path);
    }
    res
}

/// Opens a file path or URL in the operating system's default browser or viewer,
/// sleeping briefly afterward so temporary files can be loaded before removal.
pub fn preview(target: &str) -> Result<()> {
    opener::open(target).with_context(|| format!("failed to open {target}"))?;
    thread::sleep(Duration::from_secs(2));
    Ok(())
}
