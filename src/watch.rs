//! Live-reloading local HTTP preview server (`--watch`).
//!
//! Inspired by `mdBook`'s `serve` command, [`Watcher`] uses [`axum`] and [`tokio`]
//! to serve the watched Markdown file at `/` alongside any linked Markdown or
//! static files in the same directory. Every rendered page has [`RELOAD_SCRIPT`]
//! injected before `</body>`, which subscribes to `/events?path=...` via
//! Server-Sent Events (SSE) and triggers `location.reload()` whenever the file's
//! modification timestamp or byte length changes.

use crate::github::escape_html;
use crate::render::parse_content;
use crate::{Options, preview};
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use std::convert::Infallible;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::net::TcpListener;

/// Default interval between file metadata checks in watch mode.
///
/// Polling metadata (rather than relying solely on OS inode events) handles
/// editors like Neovim or Helix that save atomically by writing a temporary file
/// and renaming it over the target.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Client-side JavaScript snippet injected before `</body>` in watch mode.
///
/// Opens an `EventSource` connection to `/events?path=<current pathname>` and
/// reloads the page whenever a `"reload"` event arrives.
pub const RELOAD_SCRIPT: &str = concat!(
    r#"<script>new EventSource("/events?path=" + "#,
    r#"encodeURIComponent(decodeURIComponent(location.pathname))).onmessage = () => location.reload();</script>"#
);

/// State for the live-reload HTTP preview server.
#[derive(Debug, Clone)]
pub struct Watcher {
    filename: PathBuf,
    root: PathBuf,
    opts: Options,
    interval: Duration,
}

/// Query parameters for the `/events` Server-Sent Events endpoint.
#[derive(Deserialize)]
struct EventQuery {
    #[serde(default)]
    path: String,
}

impl Watcher {
    /// Creates a new [`Watcher`] rooted in the parent directory of `filename`.
    ///
    /// Automatically sets `opts.served = true` so relative links between
    /// Markdown files are left intact for the local HTTP server to resolve
    /// instead of being rewritten to `file://` URLs.
    pub fn new(filename: impl Into<PathBuf>, mut opts: Options) -> Self {
        let filename = filename.into();
        let root = filename
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        opts.served = true;
        Self {
            filename,
            root,
            opts,
            interval: POLL_INTERVAL,
        }
    }

    /// Overrides the file-polling interval (useful for fast integration tests).
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Builds the [`axum::Router`] serving `/events` for SSE and all other paths
    /// for Markdown rendering or static file delivery.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/events", get(serve_events))
            .route("/", get(serve_file))
            .route("/{*path}", get(serve_file))
            .with_state(self)
    }

    /// Resolves a URL path from an incoming HTTP request to a filesystem path
    /// confined within `self.root`.
    ///
    /// - `"/"` or `""` resolves to `self.filename`.
    /// - Any other path is lexically normalized (collapsing `.` and preventing
    ///   `..` from climbing above `/`) before joining onto `self.root`.
    pub fn resolve(&self, url_path: &str) -> PathBuf {
        if url_path.is_empty() || url_path == "/" {
            return self.filename.clone();
        }

        let mut normalized = PathBuf::new();
        for comp in Path::new(url_path).components() {
            match comp {
                Component::Normal(part) => normalized.push(part),
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            }
        }
        self.root.join(normalized)
    }

    /// Reads, renders, and injects [`RELOAD_SCRIPT`] into the Markdown file at `path`.
    pub fn render(&self, path: &Path) -> Result<Vec<u8>> {
        let input = fs::read(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let page = parse_content(&input, path, &self.opts)?;
        Ok(inject_script(&page))
    }
}

/// Starts the live-reloading HTTP server on an ephemeral `127.0.0.1` port,
/// writes the `http://127.0.0.1:<port>/` URL to `out`, optionally opens the
/// user's browser, and serves requests until interrupted.
pub fn serve(
    filename: &Path,
    opts: Options,
    out: &mut dyn Write,
    skip_preview: bool,
) -> Result<()> {
    // Verify the target file exists before binding a socket.
    fs::metadata(filename)
        .with_context(|| format!("cannot watch {}: file does not exist", filename.display()))?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to initialize Tokio runtime")?;

    rt.block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind local HTTP listener")?;
        let local_addr = listener.local_addr()?;
        let url = format!("http://{local_addr}/");
        writeln!(out, "{url}")?;
        out.flush()?;

        if !skip_preview {
            let preview_url = url.clone();
            std::thread::spawn(move || {
                if let Err(err) = preview(&preview_url) {
                    eprintln!("{err}");
                }
            });
        }

        let watcher = Arc::new(Watcher::new(filename, opts));
        axum::serve(listener, watcher.router())
            .await
            .context("HTTP watch server error")
    })
}

/// Injects [`RELOAD_SCRIPT`] immediately before the closing `</body>` tag, or
/// appends it at the end if `</body>` is absent.
pub fn inject_script(page: &[u8]) -> Vec<u8> {
    let needle = b"</body>";
    if let Some(pos) = page
        .windows(needle.len())
        .rposition(|window| window == needle)
    {
        let mut out = Vec::with_capacity(page.len() + RELOAD_SCRIPT.len() + 1);
        out.extend_from_slice(&page[..pos]);
        out.extend_from_slice(RELOAD_SCRIPT.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(&page[pos..]);
        out
    } else {
        let mut out = Vec::with_capacity(page.len() + RELOAD_SCRIPT.len());
        out.extend_from_slice(page);
        out.extend_from_slice(RELOAD_SCRIPT.as_bytes());
        out
    }
}

/// Reports whether `path` has a recognized Markdown file extension.
fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "mdown" | "mkd")
    )
}

/// Handler for `GET /` and `GET /{*path}`: renders Markdown files with
/// [`RELOAD_SCRIPT`] and serves other files directly from disk.
async fn serve_file(State(watcher): State<Arc<Watcher>>, uri: Uri) -> Response {
    let url_path = uri.path();
    let file_path = watcher.resolve(url_path);

    if url_path != "/" && !is_markdown(&file_path) {
        return match tokio::fs::read(&file_path).await {
            Ok(bytes) => {
                let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
                (
                    StatusCode::OK,
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_str(mime.as_ref())
                            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                    )],
                    bytes,
                )
                    .into_response()
            }
            Err(_) => (StatusCode::NOT_FOUND, "404 page not found\n").into_response(),
        };
    }

    match watcher.render(&file_path) {
        Ok(page) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            page,
        )
            .into_response(),
        Err(err) => {
            let body = format!(
                "<pre>{}</pre>\n{}",
                escape_html(&format!("{err:#}")),
                RELOAD_SCRIPT
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                )],
                body,
            )
                .into_response()
        }
    }
}

/// Captures the `(modified_time, byte_len)` fingerprint of `path` if it exists.
fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some((modified, meta.len()))
}

/// Handler for `GET /events?path=...`: streams a Server-Sent Event (`data: reload`)
/// whenever the target file's modification time or size changes.
async fn serve_events(
    State(watcher): State<Arc<Watcher>>,
    Query(query): Query<EventQuery>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let target = watcher.resolve(&query.path);
    let interval = watcher.interval;
    let initial_stamp = file_stamp(&target);

    let stream = futures_util::stream::unfold(
        (target, interval, initial_stamp, true),
        |(target, interval, mut last_stamp, first_tick)| async move {
            if first_tick {
                return Some((
                    Ok(Event::default().comment("connected")),
                    (target, interval, last_stamp, false),
                ));
            }

            loop {
                tokio::time::sleep(interval).await;
                let Some(current) = file_stamp(&target) else {
                    // File may be mid-atomic-save; wait for next tick.
                    continue;
                };
                if last_stamp != Some(current) {
                    last_stamp = Some(current);
                    return Some((
                        Ok(Event::default().data("reload")),
                        (target, interval, last_stamp, false),
                    ));
                }
            }
        },
    );

    Sse::new(stream)
}
