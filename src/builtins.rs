//! Built-in shell commands that must run in the shell's own process
//! (rather than as a spawned child), because they mutate shell state
//! such as the current working directory, job table, or terminate the
//! shell itself.

use std::env;
use std::path::{Path, PathBuf};

use crate::job_control::Shell;
use crate::jobs::JobStatus;

/// Outcome of running a built-in command.
pub enum BuiltinOutcome {
    /// The built-in ran; the shell should report this exit code and continue.
    Ran(i32),
    /// The built-in requests the shell process exit with this code.
    Exit(i32),
}

/// Attempts to run `program` as a built-in with `args`. Returns `None` if
/// `program` is not a recognized built-in, so the caller can fall back to
/// spawning an external process.
pub fn try_run(program: &str, args: &[String], shell: &mut Shell) -> Option<BuiltinOutcome> {
    match program {
        "cd" => Some(BuiltinOutcome::Ran(run_cd(args))),
        "exit" => Some(BuiltinOutcome::Exit(run_exit(args))),
        "jobs" => Some(BuiltinOutcome::Ran(run_jobs(shell))),
        "fg" => Some(BuiltinOutcome::Ran(run_fg(args, shell))),
        "bg" => Some(BuiltinOutcome::Ran(run_bg(args, shell))),
        _ => None,
    }
}

/// Changes the shell's current working directory. With no arguments,
/// changes to the user's home directory (`$HOME`). Supports a leading
/// `~` as a home-directory prefix. Returns the exit code to report.
fn run_cd(args: &[String]) -> i32 {
    let target = match args.first() {
        Some(path) => expand_tilde(path),
        None => match env::var_os("HOME") {
            Some(home) => PathBuf::from(home),
            None => {
                eprintln!("cd: HOME not set");
                return 1;
            }
        },
    };

    match env::set_current_dir(&target) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("cd: {}: {}", target.display(), err);
            1
        }
    }
}

/// Expands a leading `~` in `path` to the current user's `$HOME`.
fn expand_tilde(path: &str) -> PathBuf {
    let home = env::var_os("HOME").map(PathBuf::from);
    expand_tilde_with_home(path, home.as_deref())
}

/// Core tilde-expansion logic, parameterized on the home directory so it
/// can be unit tested without touching real process environment state.
/// Only a bare `~` or a `~/...` prefix is expanded (POSIX behavior);
/// `~` appearing mid-word is left untouched.
fn expand_tilde_with_home(path: &str, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with('/'))
        && let Some(home) = home
    {
        let mut expanded = home.to_path_buf();
        if let Some(stripped) = rest.strip_prefix('/') {
            expanded.push(stripped);
        }
        return expanded;
    }
    PathBuf::from(path)
}

/// Parses an optional exit code argument (defaulting to 0) for `exit`.
/// Non-numeric arguments are reported and treated as an error (code 1),
/// matching common shell behavior.
fn run_exit(args: &[String]) -> i32 {
    match args.first() {
        None => 0,
        Some(code) => match code.parse::<i32>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("exit: {code}: numeric argument required");
                1
            }
        },
    }
}

/// Lists tracked jobs (running or stopped), bash-style.
fn run_jobs(shell: &Shell) -> i32 {
    for job in shell.jobs.list() {
        let label = match job.status {
            JobStatus::Running => "Running",
            JobStatus::Stopped => "Stopped",
            JobStatus::Done(_) => "Done",
        };
        println!("[{}]+  {label:<22} {}", job.id, job.command_line);
    }
    0
}

/// Parses an optional job-ID argument shared by `fg`/`bg`, accepting a
/// bare number or a `%`-prefixed number (e.g. `1` or `%1`). Returns
/// `Ok(None)` when no argument was given, meaning "the current job".
fn parse_job_id(args: &[String]) -> Result<Option<u32>, String> {
    match args.first() {
        None => Ok(None),
        Some(arg) => {
            let digits = arg.strip_prefix('%').unwrap_or(arg);
            digits
                .parse::<u32>()
                .map(Some)
                .map_err(|_| format!("{arg}: no such job"))
        }
    }
}

/// Brings a background/stopped job to the foreground, waiting for it to
/// finish or stop again.
fn run_fg(args: &[String], shell: &mut Shell) -> i32 {
    match parse_job_id(args) {
        Ok(id) => match shell.resume_job(id, true) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("fg: {err}");
                1
            }
        },
        Err(err) => {
            eprintln!("fg: {err}");
            1
        }
    }
}

/// Resumes a stopped job in the background without waiting for it.
fn run_bg(args: &[String], shell: &mut Shell) -> i32 {
    match parse_job_id(args) {
        Ok(id) => match shell.resume_job(id, false) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("bg: {err}");
                1
            }
        },
        Err(err) => {
            eprintln!("bg: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_alone_returns_home() {
        let home = Path::new("/home/testuser");
        assert_eq!(
            expand_tilde_with_home("~", Some(home)),
            PathBuf::from("/home/testuser")
        );
    }

    #[test]
    fn expand_tilde_prefix_joins_home() {
        let home = Path::new("/home/testuser");
        assert_eq!(
            expand_tilde_with_home("~/projects", Some(home)),
            PathBuf::from("/home/testuser/projects")
        );
    }

    #[test]
    fn expand_tilde_mid_word_is_unchanged() {
        assert_eq!(
            expand_tilde_with_home("foo~bar", Some(Path::new("/home/testuser"))),
            PathBuf::from("foo~bar")
        );
    }

    #[test]
    fn expand_tilde_without_home_is_unchanged() {
        assert_eq!(expand_tilde_with_home("~/projects", None), PathBuf::from("~/projects"));
    }

    #[test]
    fn exit_defaults_to_zero() {
        assert_eq!(run_exit(&[]), 0);
    }

    #[test]
    fn exit_parses_numeric_code() {
        assert_eq!(run_exit(&["42".to_string()]), 42);
    }

    #[test]
    fn exit_rejects_non_numeric_code() {
        assert_eq!(run_exit(&["abc".to_string()]), 1);
    }

    #[test]
    fn parse_job_id_none_when_no_args() {
        assert_eq!(parse_job_id(&[]), Ok(None));
    }

    #[test]
    fn parse_job_id_accepts_bare_number() {
        assert_eq!(parse_job_id(&["2".to_string()]), Ok(Some(2)));
    }

    #[test]
    fn parse_job_id_accepts_percent_prefix() {
        assert_eq!(parse_job_id(&["%2".to_string()]), Ok(Some(2)));
    }

    #[test]
    fn parse_job_id_rejects_non_numeric() {
        assert!(parse_job_id(&["abc".to_string()]).is_err());
    }
}
