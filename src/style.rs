//! Page themes, syntax highlighting styles, and scoped stylesheet generation.
//!
//! Rather than exposing hundreds of raw syntax themes, `mdp` provides a curated
//! set of five syntax highlighting styles (`github`, `solarized`, `ocean`,
//! `eighties`, and `mocha`) backed by [`syntect`], automatically pairing light
//! and dark variants so switching the page theme in the browser updates code
//! blocks seamlessly.

use crate::Options;
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::fmt;
use std::io::Cursor;
use std::sync::LazyLock;
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::{SyntaxSet, syntax_definition::SyntaxDefinition};
use syntect::util::LinesWithEndings;

/// Default code syntax highlighting style name.
pub const DEFAULT_STYLE: &str = "github";

/// Prefix applied to `syntect` CSS classes on highlighted code tokens.
const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "sy-" };

/// Light-mode GitHub page stylesheet embedded from `assets/github.css`.
const PAGE_CSS: &str = include_str!("../assets/github.css");

/// Dark-mode GitHub page stylesheet overrides embedded from `assets/github-dark.css`.
const DARK_CSS: &str = include_str!("../assets/github-dark.css");

/// Embedded TOML syntax definition (`assets/TOML.sublime-syntax`).
const TOML_SYNTAX: &str = include_str!("../assets/TOML.sublime-syntax");

/// Embedded GitHub Dark `.tmTheme` (`assets/GitHub-Dark.tmTheme`).
const GITHUB_DARK_THEME: &str = include_str!("../assets/GitHub-Dark.tmTheme");

/// Embedded Darcula `.tmTheme` (`assets/Darcula.tmTheme`).
const DARCULA_THEME: &str = include_str!("../assets/Darcula.tmTheme");

/// Embedded Railscasts `.tmTheme` (`assets/Railscasts.tmTheme`).
const RAILSCASTS_THEME: &str = include_str!("../assets/Railscasts.tmTheme");

/// Lazily initialized `syntect` syntax definitions, including default syntaxes and TOML.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    if let Ok(toml_def) = SyntaxDefinition::load_from_str(TOML_SYNTAX, true, Some("toml")) {
        builder.add(toml_def);
    }
    builder.build()
});

/// Lazily initialized `syntect` theme collection, including default themes,
/// official GitHub Dark, JetBrains Darcula, and Railscasts.
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| {
    let mut ts = ThemeSet::load_defaults();
    if let Ok(t) = ThemeSet::load_from_reader(&mut Cursor::new(GITHUB_DARK_THEME.as_bytes())) {
        ts.themes.insert("GitHub Dark".to_string(), t);
    }
    if let Ok(t) = ThemeSet::load_from_reader(&mut Cursor::new(DARCULA_THEME.as_bytes())) {
        ts.themes.insert("Darcula".to_string(), t);
    }
    if let Ok(t) = ThemeSet::load_from_reader(&mut Cursor::new(RAILSCASTS_THEME.as_bytes())) {
        ts.themes.insert("Railscasts".to_string(), t);
    }
    ts
});

/// Starting color scheme for the rendered page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system's `prefers-color-scheme` setting.
    #[default]
    Auto,
    /// Force light mode.
    Light,
    /// Force dark mode.
    Dark,
}

impl Theme {
    /// Returns the lowercase string identifier (`"auto"`, `"light"`, or `"dark"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A syntax highlighting choice in the page's top-bar menu, pairing a light
/// and dark [`syntect`] theme name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeStyle {
    /// Short identifier used in `data-code-style` and `<option value="...">`.
    pub id: String,
    /// Human-readable label shown in the top-bar `<select>` menu.
    pub label: String,
    /// Whether this style is the initially selected option.
    pub selected: bool,
    pub(crate) light: String,
    pub(crate) dark: String,
}

/// Curated code styles always included in the top-bar style switcher menu.
struct BuiltinStyle {
    id: &'static str,
    label: &'static str,
    aliases: &'static [&'static str],
    light: &'static str,
    dark: &'static str,
}

/// The built-in syntax highlighting families.
const BUILTIN_STYLES: &[BuiltinStyle] = &[
    BuiltinStyle {
        id: "github",
        label: "GitHub",
        aliases: &["github", "github-light", "github-dark"],
        light: "InspiredGitHub",
        dark: "GitHub Dark",
    },
    BuiltinStyle {
        id: "solarized",
        label: "Solarized",
        aliases: &["solarized", "solarized-light", "solarized-dark"],
        light: "Solarized (light)",
        dark: "Solarized (dark)",
    },
    BuiltinStyle {
        id: "ocean",
        label: "Ocean",
        aliases: &["ocean", "ocean-light", "ocean-dark"],
        light: "base16-ocean.light",
        dark: "base16-ocean.dark",
    },
    BuiltinStyle {
        id: "darcula",
        label: "Darcula",
        aliases: &["darcula", "eighties"],
        light: "Darcula",
        dark: "Darcula",
    },
    BuiltinStyle {
        id: "railscasts",
        label: "Railscasts",
        aliases: &["railscasts"],
        light: "Railscasts",
        dark: "Railscasts",
    },
    BuiltinStyle {
        id: "mocha",
        label: "Mocha",
        aliases: &["mocha"],
        light: "base16-mocha.dark",
        dark: "base16-mocha.dark",
    },
];

/// Returns the list of valid `--style` names accepted on the command line.
pub fn available_styles() -> &'static [&'static str] {
    &[
        "github",
        "github-light",
        "github-dark",
        "solarized",
        "solarized-light",
        "solarized-dark",
        "ocean",
        "ocean-light",
        "ocean-dark",
        "darcula",
        "railscasts",
        "mocha",
    ]
}

/// Reports whether `name` is a recognized style or `syntect` theme name.
pub fn known_style(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    BUILTIN_STYLES
        .iter()
        .any(|s| s.aliases.contains(&lower.as_str()))
        || THEME_SET.themes.contains_key(name)
}

/// Resolves a style name into its `(light_theme, dark_theme)` [`syntect`] theme pair.
///
/// Family names like `"github"` or `"solarized-dark"` resolve to the family's
/// light and dark themes; explicit single-mode names like `"github-light"` or a
/// raw `syntect` theme name use that theme for the requested mode.
pub fn styles_for(name: &str) -> (String, String) {
    let lower = name.to_ascii_lowercase();
    for s in BUILTIN_STYLES {
        if s.aliases.contains(&lower.as_str()) {
            return (s.light.to_string(), s.dark.to_string());
        }
    }
    (name.to_string(), name.to_string())
}

/// Resolves a single style name to a concrete [`syntect`] theme key for dark-mode overrides.
pub fn resolve_theme_name(name: &str, for_dark: bool) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "github-light" => return "InspiredGitHub".to_string(),
        "github-dark" => return "GitHub Dark".to_string(),
        "solarized-light" => return "Solarized (light)".to_string(),
        "solarized-dark" => return "Solarized (dark)".to_string(),
        "ocean-light" => return "base16-ocean.light".to_string(),
        "ocean-dark" => return "base16-ocean.dark".to_string(),
        "darcula" => return "Darcula".to_string(),
        "railscasts" => return "Railscasts".to_string(),
        _ => {}
    }
    for s in BUILTIN_STYLES {
        if s.aliases.contains(&lower.as_str()) {
            return if for_dark { s.dark } else { s.light }.to_string();
        }
    }
    name.to_string()
}

/// Returns the menu choices for the page's code style selector and the ID of
/// the active style requested by `opts`.
pub fn code_styles(opts: &Options) -> (Vec<CodeStyle>, String) {
    let (default_light, default_dark) = styles_for(DEFAULT_STYLE);
    let light = if opts.light_style.is_empty() {
        default_light
    } else {
        resolve_theme_name(&opts.light_style, false)
    };
    let dark = if opts.dark_style.is_empty() {
        default_dark
    } else {
        resolve_theme_name(&opts.dark_style, true)
    };

    let mut choices: Vec<CodeStyle> = BUILTIN_STYLES
        .iter()
        .map(|s| CodeStyle {
            id: s.id.to_string(),
            label: s.label.to_string(),
            selected: false,
            light: s.light.to_string(),
            dark: s.dark.to_string(),
        })
        .collect();

    if let Some(found) = choices
        .iter_mut()
        .find(|c| c.light == light && c.dark == dark)
    {
        found.selected = true;
        let selected = found.id.clone();
        return (choices, selected);
    }

    let label = if dark != light {
        format!("{light} / {dark}")
    } else {
        light.clone()
    };
    let custom = CodeStyle {
        id: "custom".to_string(),
        label,
        selected: true,
        light,
        dark,
    };
    choices.insert(0, custom);
    (choices, "custom".to_string())
}

/// Generates the complete page stylesheet combining `github.css`,
/// `github-dark.css`, and scoped syntax highlighting rules for every menu choice.
pub fn stylesheet(choices: &[CodeStyle]) -> Result<String> {
    let mut buf = String::with_capacity(PAGE_CSS.len() + DARK_CSS.len() + 8192);
    buf.push_str(PAGE_CSS);
    buf.push_str(DARK_CSS);

    for choice in choices {
        for th in [Theme::Light, Theme::Dark] {
            let theme_name = if th == Theme::Dark {
                &choice.dark
            } else {
                &choice.light
            };
            let css = highlight_css(theme_name)?;
            buf.push_str(&scope_css(&css, &choice.id, th));
        }
    }
    Ok(buf)
}

/// Generates the raw CSS rules for a single `syntect` theme.
fn highlight_css(theme_name: &str) -> Result<String> {
    let theme = THEME_SET
        .themes
        .get(theme_name)
        .unwrap_or_else(|| &THEME_SET.themes["InspiredGitHub"]);
    let mut css = css_for_theme_with_class_style(theme, CLASS_STYLE)?;
    if theme_name == "InspiredGitHub" {
        css.push_str(".chroma { background-color: var(--pre-bg); }\n");
    } else if let Some(bg) = theme.settings.background {
        let fg = theme
            .settings
            .foreground
            .map(|c| format!(" color: #{:02x}{:02x}{:02x};", c.r, c.g, c.b))
            .unwrap_or_default();
        css.push_str(&format!(
            ".chroma {{ background-color: #{:02x}{:02x}{:02x};{fg} }}\n",
            bg.r, bg.g, bg.b
        ));
    }
    Ok(css)
}

/// Regex matching top-level selectors in `syntect`-generated CSS.
static SELECTOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(\.(?:sy-[\w-]+|chroma))\b").expect("valid selector regex"));

/// Scopes `syntect` CSS selectors to `[data-theme="<th_name>"][data-code-style="<id>"]`.
fn scope_css(css: &str, id: &str, th: Theme) -> String {
    let prefix = format!(r#":root[data-theme="{}"][data-code-style="{}"] "#, th, id);
    SELECTOR_RE
        .replace_all(css, |caps: &regex::Captures<'_>| {
            format!("{prefix}{}", &caps[1])
        })
        .into_owned()
}

/// Highlights a fenced code block for `lang`, returning HTML wrapped in `<pre><code>`.
///
/// If `lang` is empty, returns `<pre><code>...</code></pre>` with HTML-escaped text.
/// If `lang` is not recognized by `syntect`, returns `<pre><code class="language-...">...</code></pre>`
/// so client-side tools or styles still see the language class.
pub fn highlight_code(code: &str, lang: &str) -> String {
    let lang = lang.split_whitespace().next().unwrap_or("");
    if lang.is_empty() {
        return format!(
            "<pre><code>{}</code></pre>\n",
            crate::github::escape_html(code)
        );
    }

    if lang.eq_ignore_ascii_case("text")
        || lang.eq_ignore_ascii_case("txt")
        || lang.eq_ignore_ascii_case("plaintext")
    {
        return format!(
            "<pre class=\"chroma\"><code>{}</code></pre>\n",
            crate::github::escape_html(code)
        );
    }

    let Some(syntax) = SYNTAX_SET
        .find_syntax_by_token(lang)
        .filter(|s| s.name != "Plain Text")
    else {
        return format!(
            "<pre><code class=\"language-{}\">{}</code></pre>\n",
            crate::github::escape_html(lang),
            crate::github::escape_html(code)
        );
    };

    if syntax.name == "Plain Text" {
        return format!(
            "<pre class=\"chroma\"><code>{}</code></pre>\n",
            crate::github::escape_html(code)
        );
    }

    let mut generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAX_SET, CLASS_STYLE);
    for line in LinesWithEndings::from(code) {
        let _ = generator.parse_html_for_line_which_includes_newline(line);
    }
    let html = generator.finalize();
    format!("<pre class=\"chroma\"><code>{html}</code></pre>\n")
}
