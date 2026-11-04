//! Command-line entry point for `mdp`.

use mdp::{parse_args, run, watch};
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut stderr_buf = Vec::new();
    let cfg = match parse_args(env::args_os().skip(1), &mut stderr_buf) {
        Ok(cfg) => cfg,
        Err(err) => {
            if !stderr_buf.is_empty() {
                let text = String::from_utf8_lossy(&stderr_buf);
                // clap help/version output starts with Usage or binary name + version
                if text.starts_with("Render Markdown") || text.starts_with("mdp ") || text.contains("Usage: mdp") {
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
        eprintln!("{err:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
