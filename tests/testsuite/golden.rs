//! Golden file tests comparing rendered Markdown against `testdata/golden/*.html`.
//!
//! Run with `MDP_UPDATE_GOLDEN=1 cargo test` to regenerate golden `.html` files
//! when intentional rendering or syntax-highlighting changes are made.

use crate::markdown_parts;
use mdp::{Options, parse_content};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn golden_files() {
    let golden_dir = Path::new("testdata/golden");
    // Iterator chain note (`read_dir` -> `filter_map` -> `filter` -> `collect`):
    //   `fs::read_dir` yields `io::Result<DirEntry>`. `.filter_map(|e| e.ok().map(|e| e.path()))`
    //   unwraps valid entries into `PathBuf`s, and `.filter(...)` keeps only `.md`
    //   files. Because filesystem directory iteration order is OS-dependent,
    //   we sort `inputs` explicitly so golden tests run in deterministic order.
    let mut inputs: Vec<PathBuf> = fs::read_dir(golden_dir)
        .expect("read testdata/golden directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    inputs.sort();
    assert!(!inputs.is_empty(), "no golden inputs found in testdata/golden");

    // In Rust's built-in `libtest` harness, custom CLI flags aren't registered
    // via a global `flag.Bool("-update", ...)` like in Go; instead, reading an
    // environment variable (`MDP_UPDATE_GOLDEN=1 cargo test`) is the standard
    // convention used by `mdBook`, `cargo`, and `insta` snapshot tests.
    let update = env::var_os("MDP_UPDATE_GOLDEN").is_some();

    let mut mismatches = Vec::new();
    for input in inputs {
        let md = fs::read(&input).unwrap_or_else(|e| panic!("read {}: {e}", input.display()));
        let opts = Options {
            served: true,
            ..Options::default()
        };
        let page = parse_content(&md, &input, &opts)
            .unwrap_or_else(|e| panic!("render {}: {e}", input.display()));
        let got = markdown_parts(&page);

        let golden_file = input.with_extension("html");
        if update {
            fs::write(&golden_file, &got)
                .unwrap_or_else(|e| panic!("write {}: {e}", golden_file.display()));
            continue;
        }

        let want = fs::read_to_string(&golden_file).unwrap_or_else(|e| {
            panic!(
                "read {} ({e}); run with MDP_UPDATE_GOLDEN=1 to create it",
                golden_file.display()
            )
        });
        if got != want {
            mismatches.push((input.display().to_string(), got, want));
        }
    }

    if !mismatches.is_empty() {
        for (name, got, want) in &mismatches {
            eprintln!("=== MISMATCH in {name} ===\n--- GOT ---\n{got}\n--- WANT ---\n{want}");
        }
        panic!(
            "{} golden files differed: {:?}",
            mismatches.len(),
            mismatches.iter().map(|m| &m.0).collect::<Vec<_>>()
        );
    }
}
