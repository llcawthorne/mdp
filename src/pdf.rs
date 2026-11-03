//! Headless Chromium PDF generation and OS dark-mode detection.
//!
//! When invoked with `--pdf` (or `-o output.pdf`), `mdp` writes the rendered
//! self-contained HTML page to a temporary directory and drives a local
//! Chromium-based browser (`Google Chrome`, `Chromium`, `Microsoft Edge`, or
//! `Brave Browser`) in headless mode to print a PDF complete with a sidebar
//! document outline generated from the page's headings.

use crate::style::Theme;
use crate::{Config, preview};
use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use url::Url;

/// Renders `page` into a PDF file on disk, writes the destination path to `out`,
/// and opens the PDF in the system viewer unless `cfg.skip_preview` is set.
pub fn run_pdf(cfg: &Config, page: &[u8], out: &mut dyn Write) -> Result<()> {
    let pdf_name = cfg
        .output
        .clone()
        .unwrap_or_else(|| pdf_path(&cfg.filename));
    print_pdf(page, &pdf_name)?;

    writeln!(out, "{}", pdf_name.display())?;
    if cfg.skip_preview {
        return Ok(());
    }
    preview(&pdf_name.to_string_lossy())
}

/// Returns the default PDF output path for `filename`: alongside the Markdown
/// file with its extension replaced by `.pdf`.
pub fn pdf_path(filename: &Path) -> PathBuf {
    filename.with_extension("pdf")
}

/// Launches headless Chrome to print `page` (a complete HTML document) to `pdf_name`.
pub fn print_pdf(page: &[u8], pdf_name: &Path) -> Result<()> {
    let chrome = find_chrome()?;
    let temp_dir = tempfile::Builder::new()
        .prefix("mdp")
        .tempdir()
        .context("failed to create temporary directory for PDF rendering")?;

    let html_path = temp_dir.path().join("page.html");
    fs::write(&html_path, page).context("failed to write temporary HTML file for PDF")?;

    let page_url = Url::from_file_path(&html_path)
        .map_err(|()| anyhow::anyhow!("failed to construct file:// URL for {}", html_path.display()))?;
    let temp_pdf = temp_dir.path().join("page.pdf");
    let profile_dir = temp_dir.path().join("profile");

    let output = Command::new(&chrome)
        .arg("--headless")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-pdf-header-footer")
        .arg("--generate-pdf-document-outline")
        .arg("--virtual-time-budget=10000")
        .arg(format!("--print-to-pdf={}", temp_pdf.display()))
        .arg(page_url.as_str())
        .output()
        .with_context(|| format!("failed to execute {}", chrome.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "printing PDF with {} failed ({}):\n{}{}",
            chrome.display(),
            output.status,
            stdout,
            stderr
        );
    }

    let pdf_bytes = fs::read(&temp_pdf)
        .with_context(|| format!("printing PDF with {}: no PDF written", chrome.display()))?;
    fs::write(pdf_name, pdf_bytes)
        .with_context(|| format!("failed to write PDF to {}", pdf_name.display()))?;
    Ok(())
}

/// Resolves the effective color theme for PDF output.
///
/// Because headless Chrome does not reliably inherit the host operating system's
/// dark/light appearance setting when printing, [`Theme::Auto`] is resolved by
/// querying [`os_prefers_dark`].
pub fn pdf_theme(th: Theme) -> Theme {
    pdf_theme_with(th, os_prefers_dark)
}

/// Resolves the effective PDF color theme using a caller-supplied dark-mode
/// detector (enabling deterministic unit tests without shelling out to the OS).
pub fn pdf_theme_with(th: Theme, prefers_dark: impl FnOnce() -> bool) -> Theme {
    match th {
        Theme::Light => Theme::Light,
        Theme::Dark => Theme::Dark,
        Theme::Auto => {
            if prefers_dark() {
                Theme::Dark
            } else {
                Theme::Light
            }
        }
    }
}

/// Queries the operating system to determine whether dark mode is active.
pub fn os_prefers_dark() -> bool {
    if cfg!(target_os = "macos") {
        Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "Dark")
            .unwrap_or(false)
    } else if cfg!(target_os = "linux") {
        Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .map(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout)
                        .to_ascii_lowercase()
                        .contains("dark")
            })
            .unwrap_or(false)
    } else if cfg!(target_os = "windows") {
        Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("0x0"))
            .unwrap_or(false)
    } else {
        false
    }
}

/// Locates a Chromium-based browser executable for headless PDF printing.
///
/// Checks the `MDP_CHROME` environment variable first, then standard macOS
/// application bundles, and finally searches `PATH` for common binary names.
pub fn find_chrome() -> Result<PathBuf> {
    if let Ok(path) = env::var("MDP_CHROME")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    if cfg!(target_os = "macos") {
        let home = env::var_os("HOME").map(PathBuf::from);
        for app in [
            "Google Chrome",
            "Chromium",
            "Microsoft Edge",
            "Brave Browser",
        ] {
            let rel = PathBuf::from("Applications")
                .join(format!("{app}.app"))
                .join("Contents/MacOS")
                .join(app);
            let mut candidates = vec![PathBuf::from("/").join(&rel)];
            if let Some(ref h) = home {
                candidates.push(h.join(&rel));
            }
            for candidate in candidates {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
        "chrome",
    ] {
        if let Some(found) = find_in_path(name) {
            return Ok(found);
        }
    }

    bail!(
        "making a PDF needs Chrome, Chromium, Edge or Brave; \
         if one is installed somewhere unusual, set MDP_CHROME to its path"
    )
}

/// Searches the `PATH` environment variable for an executable named `binary`.
fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let with_exe = dir.join(format!("{binary}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}
