//! Executes a parsed command: dispatches to a built-in if recognized,
//! otherwise spawns an external process directly (never via a shell),
//! so argument values can never be reinterpreted as shell syntax.

use std::io::ErrorKind;
use std::process::Command;

use crate::builtins::{self, BuiltinOutcome};
use crate::parser::ParsedCommand;

/// Result of executing one parsed command line.
pub enum ExecutionOutcome {
    /// The command ran (built-in or external); carries its exit code.
    Completed(i32),
    /// The shell was asked to exit with this code.
    Exit(i32),
}

/// Executes `command`: dispatches to a built-in if recognized, otherwise
/// spawns an external process, waits for it, and forwards its exit code.
pub fn execute(command: &ParsedCommand) -> ExecutionOutcome {
    if let Some(outcome) = builtins::try_run(&command.program, &command.args) {
        return match outcome {
            BuiltinOutcome::Ran(code) => ExecutionOutcome::Completed(code),
            BuiltinOutcome::Exit(code) => ExecutionOutcome::Exit(code),
        };
    }

    ExecutionOutcome::Completed(run_external(command))
}

/// Spawns `command` as a child process, inheriting the shell's stdio, and
/// waits for it to finish. Returns the child's exit code, or 127 if the
/// program could not be found or executed (matching common shell
/// convention for "command not found").
fn run_external(command: &ParsedCommand) -> i32 {
    match Command::new(&command.program).args(&command.args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("{}: {}", command.program, describe_spawn_error(&err));
            127
        }
    }
}

/// Translates a process-spawn I/O error into a user-facing message,
/// special-casing the errors a shell user is most likely to hit.
fn describe_spawn_error(err: &std::io::Error) -> String {
    match err.kind() {
        ErrorKind::NotFound => "command not found".to_string(),
        ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(program: &str, args: &[&str]) -> ParsedCommand {
        ParsedCommand {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn external_command_success_reports_zero() {
        let outcome = execute(&cmd("true", &[]));
        assert!(matches!(outcome, ExecutionOutcome::Completed(0)));
    }

    #[test]
    fn external_command_failure_reports_nonzero() {
        let outcome = execute(&cmd("false", &[]));
        assert!(matches!(outcome, ExecutionOutcome::Completed(code) if code != 0));
    }

    #[test]
    fn missing_command_reports_127() {
        let outcome = execute(&cmd("shadowshell-nonexistent-command", &[]));
        assert!(matches!(outcome, ExecutionOutcome::Completed(127)));
    }

    #[test]
    fn builtin_exit_is_dispatched() {
        let outcome = execute(&cmd("exit", &["3"]));
        assert!(matches!(outcome, ExecutionOutcome::Exit(3)));
    }
}
