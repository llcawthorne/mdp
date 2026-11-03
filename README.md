# mdp

A Markdown preview tool written in idiomatic Rust (taking architectural cues from [`mdBook`](https://github.com/rust-lang/mdBook)): renders a Markdown file as a self-contained, GitHub-styled HTML page, PDF, or EPUB and opens it in your browser or reader.

## Features

* **GitHub Flavored Markdown**: tables, task lists, strikethrough, autolinks
* **GitHub's additions**: footnotes, `> [!NOTE]` alerts, `:emoji:` shortcodes, `$math$` and `$$math$$` (with KaTeX), Mermaid diagrams, and YAML front matter shown as a table
* **Safe raw HTML**: supports `<kbd>`, `<details>`, `<picture>`, and `align="center"`, with unsafe elements stripped via `ammonia` and disallowed raw tags like `<script>` filtered as text
* **Theme-aware images**: images ending in `#gh-dark-mode-only` or `#gh-light-mode-only` display only in that color scheme
* **GitHub-compatible heading IDs**: anchor slugs match GitHub's rules so `#links` carry over seamlessly
* **Curated syntax highlighting**: fenced code blocks highlighted via `syntect` across 5 curated style families (`github`, `solarized`, `ocean`, `eighties`/`darcula`, `mocha`)
* **Light & dark themes**: follows your OS `prefers-color-scheme` setting automatically, or starts in forced light (`-l`) or dark (`-d`) mode
* **Interactive top bar**: includes the document title, slide-out table of contents toggle, in-page search (`/` or `s`), syntax style menu, light/dark toggle, and print button
* **PDF & EPUB output**: headless Chromium PDF export (`-p` / `--pdf`) with a PDF sidebar outline, and native EPUB 3 export (`-e` / `--epub`) with embedded styles and table of contents navigation
* **Self-contained output**: local images are embedded as base64 `data:` URIs so pages work anywhere
* **Watch mode**: live-reloading local HTTP server (`-w` / `--watch`) powered by `axum`, `tokio`, and Server-Sent Events (SSE)

## Installation

```sh
cargo install --path .
```

## Usage

```sh
mdp [OPTIONS] <FILE>
```

| Option              | Meaning                                                                              |
|---------------------|--------------------------------------------------------------------------------------|
| `-s`, `--skip`      | Skip auto-preview; just print the output file path or URL                            |
| `-t`, `--toc`       | Start with the table of contents drawer open                                         |
| `-w`, `--watch`     | Serve a live-reloading local HTTP preview on file changes                            |
| `-o`, `--output F`  | Write to `F` instead of a temp file (`.pdf` or `.epub` implies `--pdf` or `--epub`)  |
| `-p`, `--pdf`       | Export a PDF using headless Chrome (placed beside `FILE` by default)                 |
| `-e`, `--epub`      | Export an EPUB 3 e-book (placed beside `FILE` by default)                            |
| `-d`, `--dark`      | Start in dark mode instead of following the OS                                       |
| `-l`, `--light`     | Start in light mode instead of following the OS                                      |
| `--style NAME`      | Code highlighting style (default `github`)                                           |
| `--dark-style NAME` | Code highlighting style for dark mode only                                           |

Flags can go before or after the file name, and short flags can be combined (e.g. `mdp -wd notes.md`).

Without `-w`, `mdp` writes the page to a temporary `.html` file, opens it in your default browser, and removes the temporary file once loaded. With `-o`, the file is written to the specified path and kept. With `-w`, `mdp` serves the page on a local `http://127.0.0.1:<port>/` URL until interrupted, along with the rest of its directory: links to other Markdown files render on the fly like browsing a repository on GitHub, and each page reloads automatically when its underlying file changes.

### Code Highlighting Styles

`mdp` ships with five curated style families in the top-bar menu:
* `github` (`github-light` / `github-dark`)
* `solarized` (`solarized-light` / `solarized-dark`)
* `ocean` (`ocean-light` / `ocean-dark`)
* `eighties` (or `darcula`)
* `mocha`

### PDFs and EPUBs

`-p` prints the rendered page to a PDF using headless Chrome, Chromium, Edge, or Brave (set `MDP_CHROME` to the browser binary path if installed in a non-standard location). `-e` packages the rendered document, CSS stylesheet, and heading outline into a standalone EPUB 3 archive. By default, `notes.md` produces `notes.pdf` or `notes.epub` alongside the source file unless `-o` specifies another destination (e.g. `mdp -o book-name.epub notes.md`).

## Development

Run the full test suite (unit tests, golden file tests, watch server SSE tests, and headless Chrome DOM & PDF tests):

```sh
cargo test
```

Regenerate golden HTML fixtures in `testdata/golden/*.html` after an intentional rendering change:

```sh
MDP_UPDATE_GOLDEN=1 cargo test
```

Build and verify the Rustdoc documentation:

```sh
cargo doc --no-deps
```

### Architecture & Module Responsibilities

Inspired by [`mdBook`](https://github.com/rust-lang/mdBook), `mdp` separates its CLI binary entry point (`src/main.rs`) from its library crate (`src/lib.rs`) and consolidates integration tests into a single test binary (`tests/testsuite/main.rs`):

* **`src/main.rs`**: Thin binary entry point handling exit codes and stdio routing.
* **`src/lib.rs`**: Library root defining `Config`, `Options`, `clap` CLI parsing (`parse_args`), one-shot execution (`run`), and OS browser launching (`preview` via `opener`).
* **`src/render.rs`**: Streaming Markdown-to-HTML pipeline built on `pulldown-cmark`, `ammonia` HTML sanitization, GitHub heading `slug` generation, nested TOC generation, and `handlebars` template rendering (`assets/page.hbs`).
* **`src/github.rs`**: GitHub Flavored Markdown extensions beyond `pulldown-cmark`: YAML front matter tables (`serde_yaml`), `:emoji:` shortcode replacement (`emojis`), blockquote alert metadata, and GFM's disallowed raw HTML `tagfilter`.
* **`src/links.rs`**: DOM link and image rewriting via `html5ever`, embedding local images as base64 `data:` URIs, applying `#gh-dark-mode-only` / `#gh-light-mode-only` classes, and resolving relative paths to `file://` URLs.
* **`src/style.rs`**: Curated `syntect` syntax highlighting themes, light/dark style pairings, and CSS selector scoping (`:root[data-theme="..."][data-code-style="..."]`).
* **`src/watch.rs`**: Async live-reload HTTP server built on `axum` and `tokio`, streaming Server-Sent Events (`/events`) when watched files change.
* **`src/pdf.rs`**: Headless Chromium runner for PDF export with document outlines and OS dark-mode detection.
* **`src/epub.rs`**: EPUB 3 e-book generator packaging XHTML content, embedded styles, and navigation table of contents into an Open Container Format ZIP archive.
* **`src/assets.rs`**: Compile-time embedded KaTeX (CSS, JS, and 20 base64-inlined WOFF2 fonts) and Mermaid JS assets initialized lazily via `std::sync::LazyLock`.
* **`tests/testsuite/`**: Single-binary integration test suite (`golden.rs`, `github.rs`, `watch.rs`, `chrome.rs`, `cli.rs`).
