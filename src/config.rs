//! User configuration loaded from `~/.config/shadowshell/config.toml`.
//! Malformed or missing files fall back to built-in defaults with a warning.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::personality::{PersonalityOverrides, ProjectKind};

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub theme: Theme,
    pub history_capacity: usize,
    pub autosuggestions: bool,
    pub syntax_highlighting: bool,
    pub tab_completion: bool,
    pub personality: PersonalityOverrides,
}

/// Named color theme used by the prompt and line highlighter.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Theme identifier (`onedark`, `nord`, …); kept for diagnostics/tests.
    #[allow(dead_code)]
    pub name: String,
    pub cwd: Rgb,
    pub success: Rgb,
    pub failure: Rgb,
    pub git_clean: Rgb,
    pub git_dirty: Rgb,
    pub spinner: Rgb,
    pub command_valid: Rgb,
    pub command_unknown: Rgb,
    pub string: Rgb,
    pub operator: Rgb,
    pub comment: Rgb,
}

/// 24-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }

    pub fn to_crossterm(self) -> crossterm::style::Color {
        crossterm::style::Color::Rgb {
            r: self.r,
            g: self.g,
            b: self.b,
        }
    }

    pub fn to_nu_ansi(self) -> nu_ansi_term::Color {
        nu_ansi_term::Color::Rgb(self.r, self.g, self.b)
    }
}

/// Onedark-inspired default theme (matches Phase 4 hardcoded palette).
pub fn theme_onedark() -> Theme {
    Theme {
        name: "onedark".into(),
        cwd: Rgb::new(97, 175, 239),
        success: Rgb::new(152, 195, 121),
        failure: Rgb::new(224, 108, 117),
        git_clean: Rgb::new(198, 120, 221),
        git_dirty: Rgb::new(229, 192, 123),
        spinner: Rgb::new(92, 99, 112),
        command_valid: Rgb::new(152, 195, 121),
        command_unknown: Rgb::new(224, 108, 117),
        string: Rgb::new(229, 192, 123),
        operator: Rgb::new(86, 182, 194),
        comment: Rgb::new(92, 99, 112),
    }
}

/// Higher-contrast light-friendly alternate built-in theme.
pub fn theme_nord() -> Theme {
    Theme {
        name: "nord".into(),
        cwd: Rgb::new(136, 192, 208),
        success: Rgb::new(163, 190, 140),
        failure: Rgb::new(191, 97, 106),
        git_clean: Rgb::new(180, 142, 173),
        git_dirty: Rgb::new(235, 203, 139),
        spinner: Rgb::new(76, 86, 106),
        command_valid: Rgb::new(163, 190, 140),
        command_unknown: Rgb::new(191, 97, 106),
        string: Rgb::new(235, 203, 139),
        operator: Rgb::new(143, 188, 187),
        comment: Rgb::new(76, 86, 106),
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: theme_onedark(),
            history_capacity: 1000,
            autosuggestions: true,
            syntax_highlighting: true,
            tab_completion: true,
            personality: PersonalityOverrides::enabled_default(),
        }
    }
}

// --- TOML wire format ---

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    history_capacity: Option<usize>,
    #[serde(default)]
    autosuggestions: Option<bool>,
    #[serde(default)]
    syntax_highlighting: Option<bool>,
    #[serde(default)]
    tab_completion: Option<bool>,
    #[serde(default)]
    colors: Option<FileColors>,
    #[serde(default)]
    personality: Option<FilePersonality>,
}

#[derive(Debug, Default, Deserialize)]
struct FileColors {
    cwd: Option<[u8; 3]>,
    success: Option<[u8; 3]>,
    failure: Option<[u8; 3]>,
    git_clean: Option<[u8; 3]>,
    git_dirty: Option<[u8; 3]>,
    spinner: Option<[u8; 3]>,
    command_valid: Option<[u8; 3]>,
    command_unknown: Option<[u8; 3]>,
    string: Option<[u8; 3]>,
    operator: Option<[u8; 3]>,
    comment: Option<[u8; 3]>,
}

#[derive(Debug, Default, Deserialize)]
struct FilePersonality {
    /// Master switch; default true when section is absent (handled in merge).
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    rust: Option<FileKindPersonality>,
    #[serde(default)]
    node: Option<FileKindPersonality>,
    #[serde(default)]
    python: Option<FileKindPersonality>,
    #[serde(default)]
    go: Option<FileKindPersonality>,
    #[serde(default)]
    zig: Option<FileKindPersonality>,
    #[serde(default)]
    cmake: Option<FileKindPersonality>,
}

#[derive(Debug, Default, Deserialize)]
struct FileKindPersonality {
    /// RGB accent override, e.g. `[222, 163, 90]`
    #[serde(default)]
    accent: Option<[u8; 3]>,
    /// Extra / overriding aliases for this project type.
    #[serde(default)]
    aliases: Option<std::collections::HashMap<String, String>>,
}

/// Loads config from the default path, or returns defaults.
pub fn load() -> Config {
    match config_file_path() {
        Some(path) if path.exists() => load_from_path(&path),
        Some(_) => Config::default(), // no file yet — silent defaults
        None => Config::default(),
    }
}

/// Loads and merges a TOML file into defaults. On error, warns and defaults.
pub fn load_from_path(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<FileConfig>(&text) {
            Ok(file) => merge_file(Config::default(), file),
            Err(err) => {
                eprintln!(
                    "shadowshell: warning: could not parse config {}: {err}; using defaults",
                    path.display()
                );
                Config::default()
            }
        },
        Err(err) => {
            eprintln!(
                "shadowshell: warning: could not read config {}: {err}; using defaults",
                path.display()
            );
            Config::default()
        }
    }
}

fn merge_file(mut base: Config, file: FileConfig) -> Config {
    if let Some(name) = file.theme.as_deref() {
        base.theme = match name {
            "nord" => theme_nord(),
            "onedark" => theme_onedark(),
            other => {
                eprintln!("shadowshell: warning: unknown theme `{other}`, using onedark");
                theme_onedark()
            }
        };
    }
    if let Some(n) = file.history_capacity {
        base.history_capacity = n.max(1);
    }
    if let Some(v) = file.autosuggestions {
        base.autosuggestions = v;
    }
    if let Some(v) = file.syntax_highlighting {
        base.syntax_highlighting = v;
    }
    if let Some(v) = file.tab_completion {
        base.tab_completion = v;
    }
    if let Some(c) = file.colors {
        apply_color_overrides(&mut base.theme, c);
    }
    if let Some(p) = file.personality {
        apply_personality_overrides(&mut base.personality, p);
    }
    base
}

fn apply_personality_overrides(out: &mut PersonalityOverrides, file: FilePersonality) {
    if let Some(en) = file.enabled {
        out.enabled = en;
    }
    let kinds = [
        (ProjectKind::Rust, file.rust),
        (ProjectKind::Node, file.node),
        (ProjectKind::Python, file.python),
        (ProjectKind::Go, file.go),
        (ProjectKind::Zig, file.zig),
        (ProjectKind::CMake, file.cmake),
    ];
    for (kind, section) in kinds {
        let Some(section) = section else { continue };
        if let Some([r, g, b]) = section.accent {
            out.accents.insert(kind, Rgb::new(r, g, b));
        }
        if let Some(aliases) = section.aliases {
            let list: Vec<(String, String)> = aliases.into_iter().collect();
            out.aliases.insert(kind, list);
        }
    }
}

fn apply_color_overrides(theme: &mut Theme, c: FileColors) {
    if let Some([r, g, b]) = c.cwd {
        theme.cwd = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.success {
        theme.success = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.failure {
        theme.failure = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.git_clean {
        theme.git_clean = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.git_dirty {
        theme.git_dirty = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.spinner {
        theme.spinner = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.command_valid {
        theme.command_valid = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.command_unknown {
        theme.command_unknown = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.string {
        theme.string = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.operator {
        theme.operator = Rgb::new(r, g, b);
    }
    if let Some([r, g, b]) = c.comment {
        theme.comment = Rgb::new(r, g, b);
    }
}

/// `~/.config/shadowshell/config.toml`, or `None` if `$HOME` is unset.
pub fn config_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        Path::new(&home)
            .join(".config")
            .join("shadowshell")
            .join("config.toml")
    })
}

/// Directory containing user config: `~/.config/shadowshell`.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".config").join("shadowshell"))
}

/// Ensures the config directory exists and seeds `config.toml` from the
/// embedded example when the file is missing. Returns whether this looks
/// like a first interactive run (welcome flag not yet written).
pub fn ensure_user_config() -> FirstRun {
    let Some(dir) = config_dir() else {
        return FirstRun::Skipped;
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!(
            "shadowshell: warning: could not create config dir {}: {err}",
            dir.display()
        );
        return FirstRun::Skipped;
    }

    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        const EXAMPLE: &str = include_str!("../config.toml.example");
        if let Err(err) = fs::write(&config_path, EXAMPLE) {
            eprintln!(
                "shadowshell: warning: could not write {}: {err}",
                config_path.display()
            );
        } else {
            eprintln!(
                "shadowshell: wrote default config to {}",
                config_path.display()
            );
        }
    }

    let welcome_flag = dir.join(".welcome_shown");
    if welcome_flag.exists() {
        FirstRun::Returning
    } else {
        FirstRun::Fresh { welcome_flag }
    }
}

/// Outcome of [`ensure_user_config`].
#[derive(Debug)]
pub enum FirstRun {
    /// `$HOME` unset or config dir unusable.
    Skipped,
    /// Welcome already shown previously.
    Returning,
    /// First interactive session; caller should show welcome then mark done.
    Fresh { welcome_flag: PathBuf },
}

impl FirstRun {
    /// Marks the welcome banner as shown so it is not repeated.
    pub fn mark_welcome_shown(self) {
        if let FirstRun::Fresh { welcome_flag } = self {
            let _ = fs::write(
                welcome_flag,
                b"1
",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_theme_is_onedark() {
        let cfg = Config::default();
        assert_eq!(cfg.theme.name, "onedark");
        assert!(cfg.autosuggestions);
        assert!(cfg.syntax_highlighting);
        assert!(cfg.tab_completion);
    }

    #[test]
    fn merge_selects_nord_theme() {
        let file = FileConfig {
            theme: Some("nord".into()),
            ..Default::default()
        };
        let cfg = merge_file(Config::default(), file);
        assert_eq!(cfg.theme.name, "nord");
        assert_eq!(cfg.theme.cwd, Rgb::new(136, 192, 208));
    }

    #[test]
    fn merge_color_override() {
        let file = FileConfig {
            colors: Some(FileColors {
                cwd: Some([1, 2, 3]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let cfg = merge_file(Config::default(), file);
        assert_eq!(cfg.theme.cwd, Rgb::new(1, 2, 3));
    }

    #[test]
    fn load_from_valid_toml_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("shadowshell-cfg-{}.toml", std::process::id()));
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            "theme = \"nord\"\nhistory_capacity = 42\nautosuggestions = false"
        )
        .unwrap();
        let cfg = load_from_path(&path);
        assert_eq!(cfg.theme.name, "nord");
        assert_eq!(cfg.history_capacity, 42);
        assert!(!cfg.autosuggestions);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_from_malformed_toml_falls_back() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("shadowshell-cfg-bad-{}.toml", std::process::id()));
        fs::write(&path, "theme = [not valid").unwrap();
        let cfg = load_from_path(&path);
        assert_eq!(cfg.theme.name, "onedark");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unknown_theme_falls_back_to_onedark() {
        let file = FileConfig {
            theme: Some("does-not-exist".into()),
            ..Default::default()
        };
        let cfg = merge_file(Config::default(), file);
        assert_eq!(cfg.theme.name, "onedark");
    }
}
