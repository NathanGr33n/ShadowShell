//! Built-in shell commands that must run in the shell's own process
//! (rather than as a spawned child), because they mutate shell state
//! such as the current working directory or terminate the shell itself.

use std::env;
use std::path::{Path, PathBuf};

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
pub fn try_run(program: &str, args: &[String]) -> Option<BuiltinOutcome> {
    match program {
        "cd" => Some(BuiltinOutcome::Ran(run_cd(args))),
        "exit" => Some(BuiltinOutcome::Exit(run_exit(args))),
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
}
