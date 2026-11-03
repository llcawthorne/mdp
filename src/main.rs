//! Command-line entry point for `mdp`.
//!
//! Following `mdBook`'s structure, `src/main.rs` is kept intentionally thin:
//! all CLI parsing, rendering, PDF export, and HTTP watch-server logic lives in
//! the `mdp` library crate (`src/lib.rs`) so integration tests in
//! `tests/testsuite/` can call into the library directly without spawning
//! child processes for every test.

use mdp::{parse_args, run, watch};
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

// Rust note (`fn main() -> ExitCode` vs. `std::process::exit`):
//   Like Go's `os.Exit(1)` (which skips `defer`) or Java's `System.exit(1)`,
//   calling `std::process::exit(1)` in Rust terminates the process immediately
//   WITHOUT running destructors (`Drop::drop`) on local variables in the stack!
//   Returning `std::process::ExitCode` (`ExitCode::SUCCESS` or `ExitCode::FAILURE`)
//   from `main` unwinds the stack normally so every RAII guard (like `TempDir`)
//   runs its `Drop` implementation before the process exits.
fn main() -> ExitCode {
    let mut stderr_buf = Vec::new();
    // `env::args_os()` yields `OsString` (platform-native strings that don't
    // panic if a file path contains non-UTF-8 bytes), and `.skip(1)` skips
    // `argv[0]` (the binary name), matching `os.Args[1:]` in Go.
    let cfg = match parse_args(env::args_os().skip(1), &mut stderr_buf) {
        Ok(cfg) => cfg,
        Err(err) => {
            if !stderr_buf.is_empty() {
                // `String::from_utf8_lossy` returns `Cow<'_, str>`: a zero-copy
                // `&str` borrow when `stderr_buf` is valid UTF-8, or an owned
                // `String` with `U+FFFD` replacement chars only if invalid bytes exist.
                let text = String::from_utf8_lossy(&stderr_buf);
                // clap help/version output starts with the `about` line or `mdp <version>`
                if text.starts_with("Render Markdown") || text.starts_with("mdp ") {
                    let _ = io::stdout().write_all(&stderr_buf);
                    return ExitCode::SUCCESS;
                }
                let _ = io::stderr().write_all(&stderr_buf);
            } else {
                eprintln!("{err}");
            }
            return ExitCode::FAILURE;
        }
    };

    let result = if cfg.watch {
        watch::serve(&cfg.filename, cfg.opts, &mut io::stdout(), cfg.skip_preview)
    } else {
        run(&cfg, &mut io::stdout())
    };

    if let Err(err) = result {
        // `{err:#}` invokes `anyhow::Error`'s alternate `Display` format, which
        // prints the full causal chain (`outer context: inner cause`) on one line.
        eprintln!("{err:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
