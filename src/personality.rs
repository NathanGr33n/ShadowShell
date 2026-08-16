//! Directory "personality": ambient contextual tuning based on project type.
//!
//! Walking into a recognized project (Rust, Node, Python, …) applies a light
//! theme accent, temporary aliases, and tab-completion priority — then reverts
//! when you leave. User-defined aliases are never overwritten.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::config::{Rgb, Theme};

/// Detected project kind for the current working directory tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectKind {
    None,
    Rust,
    Node,
    Python,
    Go,
    Zig,
    CMake,
}

/// Built-in personality for a project kind.
#[derive(Debug, Clone)]
pub struct PersonalityProfile {
    pub kind: ProjectKind,
    /// Prompt cwd accent (and related tints).
    pub accent: Rgb,
    /// Temporary aliases applied while this personality is active.
    pub aliases: Vec<(String, String)>,
    /// Commands sorted first in tab completion.
    pub complete_priority: Vec<String>,
    /// Short label shown in the right prompt (empty = hidden).
    pub badge: String,
}

impl PersonalityProfile {
    pub fn none() -> Self {
        PersonalityProfile {
            kind: ProjectKind::None,
            accent: Rgb::new(97, 175, 239), // onedark cwd default
            aliases: Vec::new(),
            complete_priority: Vec::new(),
            badge: String::new(),
        }
    }
}

/// User overrides loaded from config (`[personality.rust]`, …).
#[derive(Debug, Clone, Default)]
pub struct PersonalityOverrides {
    /// Per-kind accent RGB override.
    pub accents: HashMap<ProjectKind, Rgb>,
    /// Extra aliases merged on top of built-ins for a kind.
    pub aliases: HashMap<ProjectKind, Vec<(String, String)>>,
    /// Disable personality entirely.
    pub enabled: bool,
}

impl PersonalityOverrides {
    pub fn enabled_default() -> Self {
        PersonalityOverrides {
            enabled: true,
            ..Default::default()
        }
    }
}

/// Active personality state shared with the prompt, completer, and main loop.
#[derive(Debug)]
pub struct PersonalityState {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    profile: PersonalityProfile,
    /// Alias keys we injected (so we can remove them without touching user ones).
    injected_alias_keys: HashSet<String>,
    /// Values we last injected, used to detect user overrides.
    injected_values: HashMap<String, String>,
    /// Last directory we detected for (skip redundant work).
    last_cwd: Option<PathBuf>,
    overrides: PersonalityOverrides,
}

impl PersonalityState {
    pub fn new(overrides: PersonalityOverrides) -> Self {
        PersonalityState {
            inner: Mutex::new(Inner {
                profile: PersonalityProfile::none(),
                injected_alias_keys: HashSet::new(),
                injected_values: HashMap::new(),
                last_cwd: None,
                overrides,
            }),
        }
    }

    pub fn profile(&self) -> PersonalityProfile {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .profile
            .clone()
    }

    pub fn complete_priority(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .profile
            .complete_priority
            .clone()
    }

    /// Detect personality for `cwd` if it changed. Returns `true` when the
    /// active kind changed (caller should refresh aliases / theme consumers).
    pub fn update_for_cwd(&self, cwd: &Path) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if guard.last_cwd.as_deref() == Some(cwd) {
            return false;
        }
        guard.last_cwd = Some(cwd.to_path_buf());

        if !guard.overrides.enabled {
            let changed = guard.profile.kind != ProjectKind::None;
            guard.profile = PersonalityProfile::none();
            return changed;
        }

        let kind = detect_project_kind(cwd);
        if kind == guard.profile.kind {
            // Still refresh profile in case overrides were only structural — kind same.
            return false;
        }

        guard.profile = build_profile(kind, &guard.overrides);
        true
    }

    /// Applies this profile's accent onto a base theme (cwd + light spinner tint).
    pub fn effective_theme(&self, base: &Theme) -> Theme {
        let profile = self.profile();
        apply_accent(base, profile.kind, profile.accent)
    }
}

/// Marker files (file name, kind). First match walking upward wins by priority order.
const MARKERS: &[(&str, ProjectKind)] = &[
    ("Cargo.toml", ProjectKind::Rust),
    ("package.json", ProjectKind::Node),
    ("pyproject.toml", ProjectKind::Python),
    ("requirements.txt", ProjectKind::Python),
    ("setup.py", ProjectKind::Python),
    ("go.mod", ProjectKind::Go),
    ("build.zig", ProjectKind::Zig),
    ("CMakeLists.txt", ProjectKind::CMake),
];

/// Walks from `cwd` toward filesystem root looking for project markers.
pub fn detect_project_kind(cwd: &Path) -> ProjectKind {
    let mut dir = cwd.to_path_buf();
    loop {
        for (name, kind) in MARKERS {
            if dir.join(name).is_file() {
                return *kind;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    ProjectKind::None
}

fn build_profile(kind: ProjectKind, overrides: &PersonalityOverrides) -> PersonalityProfile {
    let mut profile = builtin_profile(kind);
    if let Some(accent) = overrides.accents.get(&kind) {
        profile.accent = *accent;
    }
    if let Some(extra) = overrides.aliases.get(&kind) {
        for (k, v) in extra {
            // Extra aliases override built-in personality aliases of the same name.
            if let Some(slot) = profile.aliases.iter_mut().find(|(n, _)| n == k) {
                slot.1 = v.clone();
            } else {
                profile.aliases.push((k.clone(), v.clone()));
            }
        }
    }
    profile
}

fn builtin_profile(kind: ProjectKind) -> PersonalityProfile {
    match kind {
        ProjectKind::None => PersonalityProfile::none(),
        ProjectKind::Rust => PersonalityProfile {
            kind,
            // Warm orange/amber — cargo/rustc vibe
            accent: Rgb::new(222, 163, 90),
            aliases: vec![
                ("b".into(), "cargo build".into()),
                ("t".into(), "cargo test".into()),
                ("r".into(), "cargo run".into()),
                ("ch".into(), "cargo check".into()),
                ("cl".into(), "cargo clippy".into()),
                ("cf".into(), "cargo fmt".into()),
                ("cw".into(), "cargo watch -x check".into()),
            ],
            complete_priority: vec![
                "cargo".into(),
                "rustc".into(),
                "rustup".into(),
                "cb".into(),
                "ct".into(),
                "cr".into(),
                "cc".into(),
                "cclip".into(),
                "b".into(),
                "t".into(),
                "r".into(),
                "ch".into(),
                "cl".into(),
                "cf".into(),
            ],
            badge: "rs".into(),
        },
        ProjectKind::Node => PersonalityProfile {
            kind,
            // npm green
            accent: Rgb::new(120, 200, 120),
            aliases: vec![
                ("n".into(), "npm".into()),
                ("nr".into(), "npm run".into()),
                ("ni".into(), "npm install".into()),
                ("nt".into(), "npm test".into()),
                ("nb".into(), "npm run build".into()),
                ("y".into(), "yarn".into()),
                ("p".into(), "pnpm".into()),
            ],
            complete_priority: vec![
                "npm".into(),
                "npx".into(),
                "yarn".into(),
                "pnpm".into(),
                "node".into(),
                "nr".into(),
                "ni".into(),
                "nt".into(),
                "nb".into(),
            ],
            badge: "js".into(),
        },
        ProjectKind::Python => PersonalityProfile {
            kind,
            // Python blue/yellow-ish
            accent: Rgb::new(75, 139, 190),
            aliases: vec![
                ("py".into(), "python3".into()),
                ("pip".into(), "python3 -m pip".into()),
                ("venv".into(), "python3 -m venv .venv".into()),
                ("act".into(), "source .venv/bin/activate".into()),
                ("pytest".into(), "python3 -m pytest".into()),
            ],
            complete_priority: vec![
                "python3".into(),
                "python".into(),
                "pip".into(),
                "pip3".into(),
                "pytest".into(),
                "poetry".into(),
                "py".into(),
            ],
            badge: "py".into(),
        },
        ProjectKind::Go => PersonalityProfile {
            kind,
            // Go cyan
            accent: Rgb::new(0, 173, 216),
            aliases: vec![
                ("gb".into(), "go build".into()),
                ("gt".into(), "go test".into()),
                ("gr".into(), "go run .".into()),
                ("gm".into(), "go mod".into()),
                ("gf".into(), "gofmt -w .".into()),
            ],
            complete_priority: vec![
                "go".into(),
                "gofmt".into(),
                "gb".into(),
                "gt".into(),
                "gr".into(),
                "gm".into(),
            ],
            badge: "go".into(),
        },
        ProjectKind::Zig => PersonalityProfile {
            kind,
            accent: Rgb::new(247, 164, 29),
            aliases: vec![
                ("zb".into(), "zig build".into()),
                ("zt".into(), "zig build test".into()),
                ("zr".into(), "zig build run".into()),
            ],
            complete_priority: vec!["zig".into(), "zb".into(), "zt".into(), "zr".into()],
            badge: "zig".into(),
        },
        ProjectKind::CMake => PersonalityProfile {
            kind,
            accent: Rgb::new(100, 149, 237),
            aliases: vec![
                ("cmb".into(), "cmake --build build".into()),
                ("cmc".into(), "cmake -B build".into()),
                ("cmt".into(), "ctest --test-dir build".into()),
            ],
            complete_priority: vec![
                "cmake".into(),
                "ctest".into(),
                "make".into(),
                "ninja".into(),
                "cmb".into(),
                "cmc".into(),
            ],
            badge: "c".into(),
        },
    }
}

/// Returns a copy of `base` with cwd (and a soft spinner blend) set to `accent`
/// when a non-none personality is active.
pub fn apply_accent(base: &Theme, kind: ProjectKind, accent: Rgb) -> Theme {
    if kind == ProjectKind::None {
        return base.clone();
    }
    let mut theme = base.clone();
    theme.cwd = accent;
    // Soften spinner toward accent so the job badge matches the project.
    theme.spinner = blend(base.spinner, accent, 0.45);
    theme.git_clean = blend(base.git_clean, accent, 0.25);
    theme
}

fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 {
        let v = (x as f32) * (1.0 - t) + (y as f32) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

/// Reconcile shell aliases with the active personality.
///
/// - Removes previously injected keys only if they still hold the injected value
///   (user overrides stick).
/// - Inserts new personality aliases only when the name is free or still holds
///   the old injected value.
pub fn reconcile_aliases(shell_aliases: &mut HashMap<String, String>, state: &PersonalityState) {
    let profile = state.profile();
    let mut next_injected = HashSet::new();
    let mut next_values: HashMap<String, String> = HashMap::new();

    // Drop old injections that we still own.
    let old = state.injected_snapshot();
    for (key, old_val) in &old {
        let still_ours = shell_aliases.get(key).is_some_and(|v| v == old_val);
        let in_new = profile.aliases.iter().any(|(k, _)| k == key);
        if still_ours && !in_new {
            shell_aliases.remove(key);
        }
    }

    for (name, value) in &profile.aliases {
        match shell_aliases.get(name) {
            None => {
                shell_aliases.insert(name.clone(), value.clone());
                next_injected.insert(name.clone());
                next_values.insert(name.clone(), value.clone());
            }
            Some(existing) if old.get(name).is_some_and(|v| v == existing) => {
                // We own it — refresh value.
                shell_aliases.insert(name.clone(), value.clone());
                next_injected.insert(name.clone());
                next_values.insert(name.clone(), value.clone());
            }
            Some(_) => {
                // User (or base default) owns this name — leave it.
            }
        }
    }

    state.set_injected_snapshot(next_injected, next_values);
}

impl PersonalityState {
    fn injected_snapshot(&self) -> HashMap<String, String> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .injected_values
            .clone()
    }

    fn set_injected_snapshot(&self, keys: HashSet<String>, values: HashMap<String, String>) {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        guard.injected_alias_keys = keys;
        guard.injected_values = values;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detect_rust_from_cargo_toml() {
        let dir = tempfile_dir("ss-pers-rust");
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(detect_project_kind(&dir), ProjectKind::Rust);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_walks_parents() {
        let root = tempfile_dir("ss-pers-walk");
        fs::write(root.join("package.json"), "{}").unwrap();
        let nested = root.join("src").join("app");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(detect_project_kind(&nested), ProjectKind::Node);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detect_none_in_empty_tmp() {
        let dir = tempfile_dir("ss-pers-none");
        assert_eq!(detect_project_kind(&dir), ProjectKind::None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accent_changes_cwd_color() {
        let base = crate::config::theme_onedark();
        let themed = apply_accent(&base, ProjectKind::Rust, Rgb::new(222, 163, 90));
        assert_eq!(themed.cwd, Rgb::new(222, 163, 90));
        assert_ne!(themed.cwd, base.cwd);
    }

    #[test]
    fn reconcile_does_not_clobber_user_alias() {
        let state = PersonalityState::new(PersonalityOverrides::enabled_default());
        // Force rust profile
        {
            let mut g = state.inner.lock().unwrap();
            g.profile = builtin_profile(ProjectKind::Rust);
            g.overrides.enabled = true;
        }
        let mut aliases = HashMap::new();
        aliases.insert("b".into(), "echo user".into()); // user owns `b`
        reconcile_aliases(&mut aliases, &state);
        assert_eq!(aliases.get("b").map(String::as_str), Some("echo user"));
        // Other personality alias should appear
        assert_eq!(aliases.get("t").map(String::as_str), Some("cargo test"));
    }

    #[test]
    fn reconcile_removes_injected_when_leaving() {
        let state = PersonalityState::new(PersonalityOverrides::enabled_default());
        {
            let mut g = state.inner.lock().unwrap();
            g.profile = builtin_profile(ProjectKind::Rust);
        }
        let mut aliases = HashMap::new();
        reconcile_aliases(&mut aliases, &state);
        assert!(aliases.contains_key("t"));

        {
            let mut g = state.inner.lock().unwrap();
            g.profile = PersonalityProfile::none();
        }
        reconcile_aliases(&mut aliases, &state);
        assert!(!aliases.contains_key("t"));
    }

    fn tempfile_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
