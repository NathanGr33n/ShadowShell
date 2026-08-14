//! Configures the `reedline` line editor: persistent command history and a
//! validator that keeps multiline input open while a quote or trailing
//! backslash continuation is unterminated, instead of submitting a syntax
//! error immediately.

use std::path::{Path, PathBuf};

use reedline::{FileBackedHistory, Reedline, ValidationResult, Validator};

use crate::parser;

/// Number of history entries kept, matching common shell defaults.
const HISTORY_CAPACITY: usize = 1000;

/// Name of the persistent history file, stored directly under `$HOME`.
const HISTORY_FILE_NAME: &str = ".shadowshell_history";

/// Validates a line by attempting to parse it: an unterminated quote or
/// trailing backslash is treated as incomplete input, so `reedline` inserts
/// a newline and waits for more input instead of submitting a syntax error.
struct ShellValidator;

impl Validator for ShellValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        match parser::parse_line(line) {
            Err(_) => ValidationResult::Incomplete,
            Ok(_) => ValidationResult::Complete,
        }
    }
}

/// Builds a `Reedline` line editor configured with persistent history and
/// multiline continuation support. Falls back to an in-memory
/// (non-persistent) history if the history file cannot be opened, e.g.
/// `$HOME` is unset or the file is unreadable/corrupted.
pub fn build() -> Reedline {
    let editor = Reedline::create().with_validator(Box::new(ShellValidator));
    match history_file_path() {
        Some(path) => match FileBackedHistory::with_file(HISTORY_CAPACITY, path) {
            Ok(history) => editor.with_history(Box::new(history)),
            Err(err) => {
                eprintln!("shadowshell: could not load history file, continuing without persistent history: {err}");
                editor
            }
        },
        None => {
            eprintln!("shadowshell: HOME not set, continuing without persistent history");
            editor
        }
    }
}

/// Returns the path to the shell's persistent history file, or `None` if
/// `$HOME` is not set.
fn history_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| history_path_from_home(Path::new(&home)))
}

/// Joins the history file name onto `home`, separated for testability.
fn history_path_from_home(home: &Path) -> PathBuf {
    home.join(HISTORY_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_command_is_complete() {
        assert!(matches!(
            ShellValidator.validate("echo hi"),
            ValidationResult::Complete
        ));
    }

    #[test]
    fn unterminated_single_quote_is_incomplete() {
        assert!(matches!(
            ShellValidator.validate("echo 'hi"),
            ValidationResult::Incomplete
        ));
    }

    #[test]
    fn trailing_backslash_is_incomplete() {
        assert!(matches!(
            ShellValidator.validate(r"echo hi\"),
            ValidationResult::Incomplete
        ));
    }

    #[test]
    fn history_path_appends_file_name() {
        assert_eq!(
            history_path_from_home(Path::new("/home/testuser")),
            PathBuf::from("/home/testuser/.shadowshell_history")
        );
    }
}
