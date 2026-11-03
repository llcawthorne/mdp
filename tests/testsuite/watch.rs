//! Integration tests for the live-reloading HTTP server (`watch.rs`).

use mdp::Options;
use mdp::watch::{RELOAD_SCRIPT, Watcher};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Copies `testdata/test1.md` into a temporary directory and spawns an
/// ephemeral HTTP server backed by [`Watcher`].
async fn spawn_test_watcher() -> (tempfile::TempDir, PathBuf, std::net::SocketAddr) {
    let temp = tempfile::tempdir().expect("tempdir");
    let input = fs::read("testdata/test1.md").expect("read testdata/test1.md");
    let filename = temp.path().join("test1.md");
    fs::write(&filename, input).expect("write test1.md");

    let watcher = Arc::new(
        Watcher::new(&filename, Options::default()).with_interval(Duration::from_millis(10)),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, watcher.router()).await;
    });

    (temp, filename, addr)
}

/// Sends a raw HTTP/1.1 GET request to `addr` and returns `(status_code, body)`.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write request");

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP header separator");
    let status_line = head.lines().next().expect("status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status");
    (status, body.to_string())
}

#[tokio::test]
async fn watch_serves_page_with_reload_script() {
    let (_temp, _file, addr) = spawn_test_watcher().await;
    let (status, body) = http_get(addr, "/").await;
    assert_eq!(status, 200);
    assert!(body.contains("Test Markdown File"));
    let expected_script = format!("{RELOAD_SCRIPT}\n</body>");
    assert!(
        body.contains(&expected_script),
        "missing reload script before </body>"
    );
}

#[tokio::test]
async fn watch_shows_error_page_that_still_reloads() {
    let (_temp, file, addr) = spawn_test_watcher().await;
    fs::remove_file(&file).expect("remove watched file");

    let (status, body) = http_get(addr, "/").await;
    assert_eq!(status, 500);
    assert!(
        body.contains(RELOAD_SCRIPT),
        "error page missing reload script:\n{body}"
    );
}

#[tokio::test]
async fn watch_sends_reload_when_file_changes() {
    let (_temp, file, addr) = spawn_test_watcher().await;
    expect_reload(addr, "/", &file).await;
}

#[tokio::test]
async fn watch_serves_linked_files_and_blocks_traversal() {
    let (temp, _file, addr) = spawn_test_watcher().await;
    let other = temp.path().join("other.md");
    fs::write(&other, "# Other Page\n\n[back](test1.md)\n").expect("write other.md");
    fs::write(temp.path().join("notes.txt"), "plain text").expect("write notes.txt");

    let (status, body) = http_get(addr, "/other.md").await;
    assert_eq!(status, 200);
    for want in [
        r#"<h1 id="other-page">Other Page</h1>"#,
        r#"href="test1.md""#,
        RELOAD_SCRIPT,
    ] {
        assert!(body.contains(want), "/other.md missing {want:?}");
    }

    let (status, body) = http_get(addr, "/notes.txt").await;
    assert_eq!(status, 200);
    assert_eq!(body, "plain text");

    let (status, _) = http_get(addr, "/../../etc/passwd").await;
    assert_eq!(status, 404);

    expect_reload(addr, "/other.md", &other).await;
}

/// Connects to `/events?path=...` and verifies that `: connected` arrives first,
/// no spurious reload arrives before the file changes, and `data: reload` arrives
/// promptly once `file` is modified.
async fn expect_reload(addr: std::net::SocketAddr, url_path: &str, file: &PathBuf) {
    let mut stream = TcpStream::connect(addr).await.expect("connect SSE");
    let encoded_path: String =
        url::form_urlencoded::byte_serialize(url_path.as_bytes()).collect();
    let req = format!(
        "GET /events?path={encoded_path} HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write SSE request");

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    // Skip HTTP response headers until blank line
    loop {
        line.clear();
        reader.read_line(&mut line).await.expect("read header line");
        if line == "\r\n" || line == "\n" {
            break;
        }
    }

    // Read until we find the `: connected` comment line (skipping chunk-size lines if chunked)
    loop {
        line.clear();
        reader.read_line(&mut line).await.expect("read SSE connected");
        let trimmed = line.trim();
        if trimmed == ": connected" {
            break;
        }
    }

    // Verify no reload is sent before the file changes
    let premature = tokio::time::timeout(Duration::from_millis(60), async {
        loop {
            let mut l = String::new();
            reader.read_line(&mut l).await.ok()?;
            if l.trim() == "data: reload" {
                return Some(());
            }
        }
    })
    .await;
    assert!(
        premature.is_err(),
        "reload event sent before the file changed"
    );

    // Modify the file and expect `data: reload`
    fs::write(file, "# Changed content!\n").expect("write updated file");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.expect("read SSE line");
            assert!(n > 0, "SSE stream closed before reload event");
            if line.trim() == "data: reload" {
                break;
            }
        }
    })
    .await
    .expect("timed out waiting for data: reload event");
}
