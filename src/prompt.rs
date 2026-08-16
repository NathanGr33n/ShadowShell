//! The interactive prompt: a small set of composable segments (current
//! directory, exit-status indicator, git branch/dirty state) rendered with
//! truecolor ANSI codes via `crossterm`. Git status is looked up in a
//! background thread so a slow or large repository never delays showing
//! the prompt or accepting keystrokes; while the lookup is still running,
//! a spinner placeholder is shown instead. A full user-facing config/theme
//! Colors come from the active [`crate::config::Theme`]. An optional live job
//! dashboard (spinner + compact job badge) appears when background/stopped
//! jobs are active.

use std::borrow::Cow;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crossterm::style::Stylize;
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};

use crate::config::Theme;
use crate::live_jobs::{self, JobDashboardView, LiveJobStatus, LiveJobs};
use crate::personality::PersonalityState;

/// Braille spinner frames, cycled based on elapsed time while a git lookup
/// is still pending.
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// How long each spinner frame is shown for.
const SPINNER_FRAME_MS: u128 = 80;

/// The shell's prompt: a snapshot of what to display, taken fresh before
/// each `read_line` call so it reflects the current directory and the
/// previous command's exit code.
pub struct ShellPrompt {
    cwd_display: String,
    last_exit_code: i32,
    git: Arc<Mutex<GitLookup>>,
    theme: Theme,
    live_jobs: Arc<LiveJobs>,
    personality: Option<Arc<PersonalityState>>,
}

/// State of the background git status lookup.
enum GitLookup {
    /// Still running; carries when it started, for spinner animation.
    Pending { since: Instant },
    /// Not a git repository, or the `git` command failed/is missing; the
    /// segment is simply omitted.
    Unavailable,
    /// Lookup finished successfully.
    Ready { branch: String, dirty: bool },
}

impl ShellPrompt {
    /// Builds a new prompt snapshot for the current directory, kicking off
    /// an asynchronous git status lookup that does not block this call.
    pub fn new(
        last_exit_code: i32,
        theme: Theme,
        live_jobs: Arc<LiveJobs>,
        personality: Option<Arc<PersonalityState>>,
    ) -> Self {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("?"));
        let home = env::var_os("HOME").map(PathBuf::from);
        let cwd_display = collapse_home(&cwd, home.as_deref());
        let git = spawn_git_lookup(cwd);

        ShellPrompt {
            cwd_display,
            last_exit_code,
            git,
            theme,
            live_jobs,
            personality,
        }
    }
}

impl Prompt for ShellPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(format!(
            "{} ",
            self.cwd_display
                .as_str()
                .with(self.theme.cwd.to_crossterm())
        ))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        let mut parts = Vec::new();
        if let Some(pers) = &self.personality {
            let badge = render_personality_badge(&pers.profile().badge, &self.theme);
            if !badge.is_empty() {
                parts.push(badge);
            }
        }
        let jobs = render_jobs_segment(&self.live_jobs.view(), &self.theme);
        if !jobs.is_empty() {
            parts.push(jobs);
        }
        let git = render_git_segment(&self.git, &self.theme);
        if !git.is_empty() {
            parts.push(git.trim_end().to_string());
        }
        if parts.is_empty() {
            Cow::Borrowed("")
        } else {
            Cow::Owned(format!("{} ", parts.join(" ")))
        }
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        let color = if self.last_exit_code == 0 {
            self.theme.success.to_crossterm()
        } else {
            self.theme.failure.to_crossterm()
        };
        Cow::Owned(format!("{} ", "❯".with(color).bold()))
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("::: ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({prefix}reverse-search: {}) ",
            history_search.term
        ))
    }
}

fn render_personality_badge(badge: &str, theme: &Theme) -> String {
    if badge.is_empty() {
        return String::new();
    }
    format!("{}", badge.with(theme.cwd.to_crossterm()).bold())
}

/// Compact ambient job dashboard for the right prompt.
///
/// Examples:
/// - one running job:  `⠋ sleep 30 &`
/// - several:          `⠋×2 ⏸×1`
/// - only stopped:     `⏸ [1] vim`
fn render_jobs_segment(view: &JobDashboardView, theme: &Theme) -> String {
    if view.is_empty() {
        return String::new();
    }
    let pulse = theme.spinner.to_crossterm();
    let stop_color = theme.failure.to_crossterm();
    let run = view.running_count();
    let stop = view.stopped_count();
    let frame = spinner_frame(view.pulse_origin.elapsed().as_millis());

    // Prefer a short command snippet when there's a single job.
    if view.entries.len() == 1 {
        let e = &view.entries[0];
        let cmd = live_jobs::truncate_cmd(&e.command_line, 18);
        return match e.status {
            LiveJobStatus::Running => {
                format!("{} {}", frame.to_string().with(pulse), cmd.with(pulse))
            }
            LiveJobStatus::Stopped => {
                format!(
                    "{} [{}] {}",
                    "⏸".with(stop_color),
                    e.id,
                    cmd.with(stop_color)
                )
            }
        };
    }

    let mut bits = Vec::new();
    if run > 0 {
        bits.push(format!("{}×{}", frame.to_string().with(pulse), run));
    }
    if stop > 0 {
        bits.push(format!("{}×{}", "⏸".with(stop_color), stop));
    }
    bits.join(" ")
}

/// Renders the right-hand git segment from the current lookup state
/// without ever blocking on the background thread.
fn render_git_segment(state: &Arc<Mutex<GitLookup>>, theme: &Theme) -> String {
    let lookup = match state.lock() {
        Ok(guard) => guard,
        // A poisoned lock (the lookup thread panicked) still holds a
        // usable last value; recovering it is safer than propagating a
        // panic into the prompt render path.
        Err(poisoned) => poisoned.into_inner(),
    };

    match &*lookup {
        GitLookup::Unavailable => String::new(),
        GitLookup::Pending { since } => {
            let frame = spinner_frame(since.elapsed().as_millis());
            format!("{} ", frame.to_string().with(theme.spinner.to_crossterm()))
        }
        GitLookup::Ready { branch, dirty } => {
            let color = if *dirty {
                theme.git_dirty.to_crossterm()
            } else {
                theme.git_clean.to_crossterm()
            };
            let marker = if *dirty { "*" } else { "" };
            format!("{} ", format!("{branch}{marker}").with(color))
        }
    }
}

/// Picks a spinner frame based on elapsed milliseconds, so repeated
/// renders while a lookup is still pending (e.g. after a terminal resize
/// or screen clear) animate rather than showing a frozen glyph.
fn spinner_frame(elapsed_ms: u128) -> char {
    let index = (elapsed_ms / SPINNER_FRAME_MS) as usize % SPINNER_FRAMES.len();
    SPINNER_FRAMES[index]
}

/// Spawns a background thread that looks up git status for `cwd` and
/// returns a handle the prompt can poll without blocking.
fn spawn_git_lookup(cwd: PathBuf) -> Arc<Mutex<GitLookup>> {
    let state = Arc::new(Mutex::new(GitLookup::Pending {
        since: Instant::now(),
    }));
    let state_for_thread = Arc::clone(&state);

    thread::spawn(move || {
        let result = query_git_status(&cwd);
        if let Ok(mut guard) = state_for_thread.lock() {
            *guard = result;
        }
    });

    state
}

/// Runs `git status` for `cwd` and interprets the result. Any failure
/// (not a repository, `git` not installed, unexpected output) maps to
/// [`GitLookup::Unavailable`] so the segment is silently omitted, matching
/// the Phase 4 error-handling scope.
fn query_git_status(cwd: &Path) -> GitLookup {
    // `--no-optional-locks` avoids touching the repository's index/lock
    // files just to render a prompt, so this never contends with a git
    // command the user is actually running.
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--branch",
        ])
        .current_dir(cwd)
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return GitLookup::Unavailable,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let Some(branch_line) = lines.next() else {
        return GitLookup::Unavailable;
    };

    GitLookup::Ready {
        branch: parse_branch_line(branch_line),
        // Any further lines are changed/untracked files.
        dirty: lines.next().is_some(),
    }
}

/// Extracts a short branch name from `git status --branch --porcelain`'s
/// header line, e.g. `## main...origin/main` -> `main`,
/// `## HEAD (no branch)` -> `HEAD`, `## No commits yet on main` -> `main`.
fn parse_branch_line(line: &str) -> String {
    let rest = line.trim_start_matches("## ");
    let rest = rest.strip_prefix("No commits yet on ").unwrap_or(rest);
    if rest.starts_with("HEAD") {
        return "HEAD".to_string();
    }
    rest.split("...").next().unwrap_or(rest).trim().to_string()
}

/// Collapses `cwd` to a `~`-relative display path when it is under `home`.
fn collapse_home(cwd: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(suffix) = cwd.strip_prefix(home)
    {
        return if suffix.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", suffix.display())
        };
    }
    cwd.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_home_root_becomes_tilde() {
        assert_eq!(
            collapse_home(
                Path::new("/home/testuser"),
                Some(Path::new("/home/testuser"))
            ),
            "~"
        );
    }

    #[test]
    fn collapse_home_subdir_gets_tilde_prefix() {
        assert_eq!(
            collapse_home(
                Path::new("/home/testuser/projects/shadowshell"),
                Some(Path::new("/home/testuser"))
            ),
            "~/projects/shadowshell"
        );
    }

    #[test]
    fn collapse_home_outside_home_is_unchanged() {
        assert_eq!(
            collapse_home(Path::new("/var/log"), Some(Path::new("/home/testuser"))),
            "/var/log"
        );
    }

    #[test]
    fn collapse_home_without_home_is_unchanged() {
        assert_eq!(collapse_home(Path::new("/var/log"), None), "/var/log");
    }

    #[test]
    fn parse_branch_line_with_upstream_tracking() {
        assert_eq!(parse_branch_line("## main...origin/main"), "main");
    }

    #[test]
    fn parse_branch_line_with_ahead_behind_info() {
        assert_eq!(
            parse_branch_line("## feature...origin/feature [ahead 2]"),
            "feature"
        );
    }

    #[test]
    fn parse_branch_line_without_upstream() {
        assert_eq!(parse_branch_line("## main"), "main");
    }

    #[test]
    fn parse_branch_line_detached_head() {
        assert_eq!(parse_branch_line("## HEAD (no branch)"), "HEAD");
    }

    #[test]
    fn parse_branch_line_no_commits_yet() {
        assert_eq!(parse_branch_line("## No commits yet on main"), "main");
    }

    #[test]
    fn spinner_frame_cycles_through_all_frames() {
        let frames: Vec<char> = (0..SPINNER_FRAMES.len())
            .map(|i| spinner_frame(i as u128 * SPINNER_FRAME_MS))
            .collect();
        assert_eq!(frames, SPINNER_FRAMES.to_vec());
        // Wraps back around after a full cycle.
        assert_eq!(
            spinner_frame(SPINNER_FRAMES.len() as u128 * SPINNER_FRAME_MS),
            SPINNER_FRAMES[0]
        );
    }

    #[test]
    fn query_git_status_on_this_repo_finds_a_branch() {
        // This crate's own checkout is a git repository, so this exercises
        // the real `git` invocation without depending on its exact output
        // (branch name/dirty state vary with the environment).
        match query_git_status(Path::new(env!("CARGO_MANIFEST_DIR"))) {
            GitLookup::Ready { branch, .. } => assert!(!branch.is_empty()),
            GitLookup::Unavailable => panic!("expected the crate root to be a git repository"),
            GitLookup::Pending { .. } => unreachable!("query_git_status runs synchronously"),
        }
    }

    #[test]
    fn query_git_status_outside_a_repo_is_unavailable() {
        assert!(matches!(
            query_git_status(Path::new(std::env::temp_dir().as_path())),
            GitLookup::Unavailable
        ));
    }

    /// Builds a `ShellPrompt` with the given exit code and git state,
    /// without going through `ShellPrompt::new` (which would spawn a real
    /// git lookup thread for the current directory).
    fn prompt_with(last_exit_code: i32, git: GitLookup) -> ShellPrompt {
        ShellPrompt {
            cwd_display: String::new(),
            last_exit_code,
            git: Arc::new(Mutex::new(git)),
            theme: crate::config::theme_onedark(),
            live_jobs: Arc::new(LiveJobs::new()),
            personality: None,
        }
    }

    // These tests assert on the literal ANSI SGR codes crossterm emits for
    // `Color::Rgb`, rather than on visual appearance, since color
    // perception isn't reliably verifiable and terminals may globally
    // suppress color (e.g. `NO_COLOR`). `force_color_output(true)` ensures
    // that ambient setting doesn't make these tests flaky.

    #[test]
    fn indicator_is_success_colored_on_exit_code_zero() {
        crossterm::style::force_color_output(true);
        let prompt = prompt_with(0, GitLookup::Unavailable);
        let rendered = prompt.render_prompt_indicator(PromptEditMode::Default);
        assert!(rendered.contains("38;2;152;195;121"), "got: {rendered:?}");
        assert!(rendered.contains('❯'));
    }

    #[test]
    fn indicator_is_failure_colored_on_nonzero_exit_code() {
        crossterm::style::force_color_output(true);
        let prompt = prompt_with(1, GitLookup::Unavailable);
        let rendered = prompt.render_prompt_indicator(PromptEditMode::Default);
        assert!(rendered.contains("38;2;224;108;117"), "got: {rendered:?}");
    }

    #[test]
    fn left_prompt_uses_cwd_color() {
        crossterm::style::force_color_output(true);
        let prompt = prompt_with(0, GitLookup::Unavailable);
        let rendered = prompt.render_prompt_left();
        assert!(rendered.contains("38;2;97;175;239"), "got: {rendered:?}");
    }

    #[test]
    fn git_segment_is_empty_when_unavailable() {
        let state = Arc::new(Mutex::new(GitLookup::Unavailable));
        assert_eq!(
            render_git_segment(&state, &crate::config::theme_onedark()),
            ""
        );
    }

    #[test]
    fn git_segment_shows_clean_branch_without_dirty_marker() {
        crossterm::style::force_color_output(true);
        let state = Arc::new(Mutex::new(GitLookup::Ready {
            branch: "main".to_string(),
            dirty: false,
        }));
        let rendered = render_git_segment(&state, &crate::config::theme_onedark());
        assert!(rendered.contains("main"));
        assert!(!rendered.contains('*'));
        assert!(rendered.contains("38;2;198;120;221"), "got: {rendered:?}");
    }

    #[test]
    fn git_segment_shows_dirty_marker_and_color() {
        crossterm::style::force_color_output(true);
        let state = Arc::new(Mutex::new(GitLookup::Ready {
            branch: "main".to_string(),
            dirty: true,
        }));
        let rendered = render_git_segment(&state, &crate::config::theme_onedark());
        assert!(rendered.contains("main*"), "got: {rendered:?}");
        assert!(rendered.contains("38;2;229;192;123"), "got: {rendered:?}");
    }

    #[test]
    fn git_segment_shows_spinner_while_pending() {
        let state = Arc::new(Mutex::new(GitLookup::Pending {
            since: Instant::now(),
        }));
        let rendered = render_git_segment(&state, &crate::config::theme_onedark());
        assert!(
            SPINNER_FRAMES.iter().any(|frame| rendered.contains(*frame)),
            "got: {rendered:?}"
        );
    }

    #[test]
    fn jobs_segment_empty_when_no_jobs() {
        let view = JobDashboardView {
            pulse_origin: Instant::now(),
            entries: vec![],
        };
        assert_eq!(
            render_jobs_segment(&view, &crate::config::theme_onedark()),
            ""
        );
    }

    #[test]
    fn jobs_segment_single_running_shows_spinner_and_cmd() {
        crossterm::style::force_color_output(true);
        let view = JobDashboardView {
            pulse_origin: Instant::now(),
            entries: vec![crate::live_jobs::LiveJobEntry {
                id: 1,
                pgid: 1,
                pids: vec![1],
                command_line: "sleep 30".into(),
                status: LiveJobStatus::Running,
            }],
        };
        let rendered = render_jobs_segment(&view, &crate::config::theme_onedark());
        assert!(rendered.contains("sleep 30"), "got: {rendered:?}");
        assert!(
            SPINNER_FRAMES.iter().any(|f| rendered.contains(*f)),
            "got: {rendered:?}"
        );
    }

    #[test]
    fn jobs_segment_multi_uses_counts() {
        let view = JobDashboardView {
            pulse_origin: Instant::now(),
            entries: vec![
                crate::live_jobs::LiveJobEntry {
                    id: 1,
                    pgid: 1,
                    pids: vec![1],
                    command_line: "a".into(),
                    status: LiveJobStatus::Running,
                },
                crate::live_jobs::LiveJobEntry {
                    id: 2,
                    pgid: 2,
                    pids: vec![2],
                    command_line: "b".into(),
                    status: LiveJobStatus::Running,
                },
                crate::live_jobs::LiveJobEntry {
                    id: 3,
                    pgid: 3,
                    pids: vec![3],
                    command_line: "c".into(),
                    status: LiveJobStatus::Stopped,
                },
            ],
        };
        let rendered = render_jobs_segment(&view, &crate::config::theme_onedark());
        assert!(rendered.contains("×2"), "got: {rendered:?}");
        assert!(rendered.contains("×1"), "got: {rendered:?}");
        assert!(rendered.contains('⏸'), "got: {rendered:?}");
    }
}
