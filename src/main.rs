//! ShadowShell entry point: runs the interactive read-eval loop, reading
//! one line at a time via `reedline` (with persistent history and
//! multiline continuation, see `line_editor`), parsing it into a
//! pipeline, and executing it via `job_control::Shell` (process groups,
//! terminal ownership, job tracking) until the user exits or sends EOF
//! (Ctrl+D on an empty line). Full prompt theming/animation arrives in a
//! later phase.

mod builtins;
mod executor;
mod job_control;
mod jobs;
mod line_editor;
mod parser;

use std::process::ExitCode;

use reedline::{DefaultPrompt, DefaultPromptSegment, Signal};

use executor::ExecutionOutcome;
use job_control::Shell;

/// What the main loop should do after processing one line of input.
enum LoopControl {
    /// Keep reading input; carries the exit code to report via the prompt.
    Continue(i32),
    /// Stop the shell and exit the process with this code.
    Exit(i32),
}

fn main() -> ExitCode {
    let mut line_editor = line_editor::build();
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::WorkingDirectory,
        DefaultPromptSegment::Empty,
    );
    let mut shell = Shell::new();
    let mut last_exit_code: i32 = 0;

    loop {
        // Report any background jobs that finished since the last prompt,
        // matching common shell notification timing.
        shell.notify_job_changes();

        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => match run_line(&line, last_exit_code, &mut shell) {
                LoopControl::Continue(code) => last_exit_code = code,
                LoopControl::Exit(code) => {
                    warn_about_active_jobs(&shell);
                    return to_exit_code(code);
                }
            },
            // Ctrl+C cancels the current line; the shell keeps running.
            Ok(Signal::CtrlC) => continue,
            // Ctrl+D on an empty line signals EOF: end the session.
            Ok(Signal::CtrlD) => break,
            // Any other signal (host commands, external breaks) is not used
            // by this minimal loop; ignore and keep reading.
            Ok(_) => continue,
            Err(err) => {
                eprintln!("shadowshell: input error: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    warn_about_active_jobs(&shell);
    to_exit_code(last_exit_code)
}

/// Parses and executes one line of input, returning how the main loop
/// should proceed. A blank line or a parse error does not change the
/// previous exit code's continuation behavior beyond reporting it.
fn run_line(line: &str, last_exit_code: i32, shell: &mut Shell) -> LoopControl {
    match parser::parse_line(line) {
        // Blank/whitespace-only input: nothing to run, exit code unchanged.
        Ok(None) => LoopControl::Continue(last_exit_code),
        Ok(Some(pipeline)) => match executor::execute(&pipeline, shell, line) {
            ExecutionOutcome::Completed(code) => LoopControl::Continue(code),
            ExecutionOutcome::Exit(code) => LoopControl::Exit(code),
        },
        Err(err) => {
            eprintln!("shadowshell: {err}");
            LoopControl::Continue(2)
        }
    }
}

/// Warns the user if background/stopped jobs are still tracked when the
/// shell is about to exit. Matching common shell defaults, these jobs are
/// left running (reparented to the system init process) rather than
/// killed; this is purely an informational notice.
fn warn_about_active_jobs(shell: &Shell) {
    let count = shell.jobs.list().len();
    if count > 0 {
        eprintln!(
            "shadowshell: warning: {count} job(s) still running or stopped; they will keep running after exit"
        );
    }
}

/// Converts a shell-style exit code to a process [`ExitCode`], wrapping
/// into the 0-255 range the same way POSIX shells do.
fn to_exit_code(code: i32) -> ExitCode {
    ExitCode::from((code & 0xFF) as u8)
}
