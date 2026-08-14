//! Executes a parsed [`Pipeline`]: dispatches to a built-in when it is a
//! single, non-backgrounded command with no redirects, otherwise runs it
//! as a process-group pipeline via [`job_control::Shell`]. Built-ins are
//! only meaningful in the shell's own process, so using one inside a
//! multi-command pipeline or backgrounding one is reported as a clear,
//! scoped error rather than silently attempting (and failing) to spawn
//! it as an external program.

use crate::builtins::{self, BuiltinOutcome};
use crate::job_control::Shell;
use crate::parser::{Pipeline, SimpleCommand};

/// Names of built-in commands, used to detect and reject unsupported
/// combinations (pipelines, backgrounding) before attempting to spawn
/// them as external programs.
const BUILTIN_NAMES: [&str; 5] = ["cd", "exit", "jobs", "fg", "bg"];

/// Result of executing one parsed pipeline.
pub enum ExecutionOutcome {
    /// The pipeline ran; carries the exit code to report as `$?`.
    Completed(i32),
    /// The shell was asked to exit with this code.
    Exit(i32),
}

/// Executes `pipeline` (parsed from `command_line`, which is kept for job
/// listings and `fg`/`bg` display), using `shell` for job control.
pub fn execute(pipeline: &Pipeline, shell: &mut Shell, command_line: &str) -> ExecutionOutcome {
    if let Some(single) = single_builtin_candidate(pipeline) {
        if is_builtin(&single.program) {
            if !single.redirects.is_empty() {
                eprintln!(
                    "shadowshell: {}: redirects on built-ins are not yet supported",
                    single.program
                );
                return ExecutionOutcome::Completed(1);
            }
            if let Some(outcome) = builtins::try_run(&single.program, &single.args, shell) {
                return match outcome {
                    BuiltinOutcome::Ran(code) => ExecutionOutcome::Completed(code),
                    BuiltinOutcome::Exit(code) => ExecutionOutcome::Exit(code),
                };
            }
        }
    } else if let Some(name) = builtin_used_unsupported(pipeline) {
        eprintln!("shadowshell: {name}: built-ins cannot be used in a pipeline or backgrounded yet");
        return ExecutionOutcome::Completed(1);
    }

    // Not a built-in (or not one usable here): run as a real pipeline.
    // Parser invariant: `pipeline.commands` always has at least one entry.
    ExecutionOutcome::Completed(shell.run_pipeline(pipeline, command_line))
}

/// Returns the single command in `pipeline` when it is eligible for
/// built-in dispatch: exactly one command and not backgrounded. Whether
/// that command's `program` is actually a built-in name is checked
/// separately by [`is_builtin`], so plain external commands (e.g.
/// `echo hi > out.txt`) fall through to normal pipeline execution.
fn single_builtin_candidate(pipeline: &Pipeline) -> Option<&SimpleCommand> {
    if pipeline.background || pipeline.commands.len() != 1 {
        return None;
    }
    pipeline.commands.first()
}

/// Returns whether `name` is a recognized built-in command.
fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// If `pipeline` (already known not to be a plain single foreground
/// command) uses a known built-in name anywhere, returns that name so the
/// caller can report a clear, scoped error instead of silently searching
/// for a same-named external program that would just fail.
fn builtin_used_unsupported(pipeline: &Pipeline) -> Option<&str> {
    pipeline
        .commands
        .iter()
        .map(|c| c.program.as_str())
        .find(|name| BUILTIN_NAMES.contains(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_line;

    fn run(line: &str) -> ExecutionOutcome {
        let pipeline = parse_line(line).unwrap().unwrap();
        let mut shell = Shell::for_test();
        execute(&pipeline, &mut shell, line)
    }

    #[test]
    fn external_command_success_reports_zero() {
        assert!(matches!(run("true"), ExecutionOutcome::Completed(0)));
    }

    #[test]
    fn external_command_failure_reports_nonzero() {
        assert!(matches!(run("false"), ExecutionOutcome::Completed(code) if code != 0));
    }

    #[test]
    fn missing_command_reports_127() {
        assert!(matches!(
            run("shadowshell-nonexistent-command"),
            ExecutionOutcome::Completed(127)
        ));
    }

    #[test]
    fn builtin_exit_is_dispatched() {
        assert!(matches!(run("exit 3"), ExecutionOutcome::Exit(3)));
    }

    #[test]
    fn builtin_in_pipeline_is_a_clear_error() {
        assert!(matches!(
            run("cd /tmp | cat"),
            ExecutionOutcome::Completed(1)
        ));
    }

    #[test]
    fn builtin_backgrounded_is_a_clear_error() {
        assert!(matches!(run("cd /tmp &"), ExecutionOutcome::Completed(1)));
    }

    #[test]
    fn builtin_with_redirect_is_a_clear_error() {
        assert!(matches!(
            run("exit 0 > /tmp/shadowshell-unused"),
            ExecutionOutcome::Completed(1)
        ));
    }

    #[test]
    fn pipeline_of_external_commands_runs() {
        assert!(matches!(run("true | true"), ExecutionOutcome::Completed(0)));
    }

    #[test]
    fn external_command_with_redirect_runs_normally() {
        // Regression test: a single non-builtin command with a redirect
        // must NOT be caught by the built-in redirect guard.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("shadowshell-executor-test-{}.txt", std::process::id()));
        let line = format!("echo hi > {}", path.display());

        assert!(matches!(run(&line), ExecutionOutcome::Completed(0)));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hi\n");

        let _ = std::fs::remove_file(&path);
    }
}
