//! Configures the `reedline` line editor: persistent history, multiline
//! validation, history-based autosuggestions, syntax highlighting, and
//! tab completion (files + PATH executables).

use std::path::{Path, PathBuf};

use std::sync::Arc;
use std::time::Duration;

use nu_ansi_term::{Color as AnsiColor, Style as AnsiStyle};
use reedline::{
    default_emacs_keybindings, ColumnarMenu, DefaultHinter, EditCommand, Emacs, ExternalPrinter,
    FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu,
    ValidationResult, Validator,
};

use crate::completer::ShellCompleter;
use crate::config::Config;
use crate::highlighter::ShellHighlighter;
use crate::live_jobs::LiveJobs;
use crate::parser;
use crate::personality::PersonalityState;

/// Name of the persistent history file, stored directly under `$HOME`.
const HISTORY_FILE_NAME: &str = ".shadowshell_history";

/// Validates a line by attempting to parse it: incomplete constructs keep
/// the multiline prompt open; hard syntax errors submit and report.
struct ShellValidator;

impl Validator for ShellValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        match parser::parse_program(line) {
            Err(err) if err.is_incomplete() => ValidationResult::Incomplete,
            Err(_) | Ok(_) => ValidationResult::Complete,
        }
    }
}

/// Builds a `Reedline` editor from `config` (history size, feature toggles,
/// theme colors). When `live_jobs` is provided, installs an idle poll that
/// reaps finished background jobs, prints Done notifications, and forces
/// prompt repaints so the ambient spinner advances while the user is idle.
pub fn build(
    config: &Config,
    live_jobs: Option<Arc<LiveJobs>>,
    personality: Option<Arc<PersonalityState>>,
) -> Reedline {
    let mut editor = Reedline::create().with_validator(Box::new(ShellValidator));

    // History
    editor = match history_file_path() {
        Some(path) => match FileBackedHistory::with_file(config.history_capacity, path) {
            Ok(history) => editor.with_history(Box::new(history)),
            Err(err) => {
                eprintln!(
                    "shadowshell: could not load history file, continuing without persistent history: {err}"
                );
                editor
            }
        },
        None => {
            eprintln!("shadowshell: HOME not set, continuing without persistent history");
            editor
        }
    };

    // Autosuggestions (history hinter)
    if config.autosuggestions {
        let hinter = DefaultHinter::default()
            .with_style(AnsiStyle::new().italic().fg(AnsiColor::LightGray))
            .with_min_chars(1);
        editor = editor.with_hinter(Box::new(hinter));
    }

    // Syntax highlighting
    if config.syntax_highlighting {
        editor = editor.with_highlighter(Box::new(ShellHighlighter::new(config.theme.clone())));
    }

    // Tab completion + columnar menu + Tab keybinding
    if config.tab_completion {
        let completer = ShellCompleter::with_personality(personality);
        let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Tab,
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::Menu("completion_menu".into()),
                ReedlineEvent::MenuNext,
            ]),
        );
        // Shift-Tab cycles backwards when the menu is open.
        keybindings.add_binding(
            KeyModifiers::SHIFT,
            KeyCode::BackTab,
            ReedlineEvent::MenuPrevious,
        );
        // Right-arrow accepts the current hinter suggestion when at EOL.
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Right,
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::HistoryHintComplete,
                ReedlineEvent::Edit(vec![EditCommand::MoveRight { select: false }]),
            ]),
        );

        editor = editor
            .with_completer(Box::new(completer))
            .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
            .with_edit_mode(Box::new(Emacs::new(keybindings)));
    }

    if let Some(live_jobs) = live_jobs {
        editor = attach_live_job_idle(editor, live_jobs);
    }

    editor
}

/// Poll interval for spinner animation and background job reaping while the
/// line editor is waiting for input.
const LIVE_JOB_POLL_MS: u64 = 100;

/// Invisible marker that forces reedline's external-printer path to repaint
/// the prompt (advancing spinners) without printing visible text. Uses a
/// carriage-return-only payload that `print_external_message` still treats
/// as a non-empty message batch.
fn attach_live_job_idle(editor: Reedline, live_jobs: Arc<LiveJobs>) -> Reedline {
    let printer = ExternalPrinter::new(64);
    let printer_for_idle = printer.clone();
    let jobs = Arc::clone(&live_jobs);

    editor
        .with_external_printer(printer)
        .with_poll_interval(Duration::from_millis(LIVE_JOB_POLL_MS))
        .with_idle_callback(Box::new(move || {
            // Reap finished background jobs and queue Done notifications.
            let _changed = jobs.poll_completions();
            for finished in jobs.take_notifications() {
                let label = if finished.code == 0 {
                    "Done".to_string()
                } else {
                    format!("Exit {}", finished.code)
                };
                let line = format!(
                    "[{}]+  {:<22} {}",
                    finished.id, label, finished.command_line
                );
                let _ = printer_for_idle.print(line);
            }
        }))
}

fn history_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| history_path_from_home(Path::new(&home)))
}

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

    #[test]
    fn open_if_is_incomplete() {
        assert!(matches!(
            ShellValidator.validate("if true; then"),
            ValidationResult::Incomplete
        ));
    }
}
