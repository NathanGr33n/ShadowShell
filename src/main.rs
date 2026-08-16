//! ShadowShell entry point: interactive read-eval loop or script execution.
//! Interactive mode uses `reedline` with persistent history and multiline
//! continuation; scripts are read from a file path given on the command line.

mod builtins;
mod env;
mod executor;
mod expand;
mod interpreter;
mod job_control;
mod jobs;
mod line_editor;
mod parser;
mod prompt;

use std::env as process_env;
use std::fs;
use std::process::ExitCode;

use reedline::Signal;

use interpreter::{interpret, InterpretOutcome};
use job_control::Shell;
use prompt::ShellPrompt;

/// What the main loop should do after processing one buffer of input.
enum LoopControl {
    Continue(i32),
    Exit(i32),
}

fn main() -> ExitCode {
    let mut args = process_env::args().skip(1).collect::<Vec<_>>();
    if let Some(script) = args.first().cloned() {
        args.remove(0);
        return run_script(&script, args);
    }

    run_interactive()
}

fn run_interactive() -> ExitCode {
    let mut line_editor = line_editor::build();
    let mut shell = Shell::new();
    let mut last_exit_code: i32 = 0;

    loop {
        shell.notify_job_changes();
        let prompt = ShellPrompt::new(last_exit_code);

        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => match run_source(&line, last_exit_code, &mut shell) {
                LoopControl::Continue(code) => last_exit_code = code,
                LoopControl::Exit(code) => {
                    warn_about_active_jobs(&shell);
                    return to_exit_code(code);
                }
            },
            Ok(Signal::CtrlC) => continue,
            Ok(Signal::CtrlD) => break,
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

fn run_script(path: &str, positionals: Vec<String>) -> ExitCode {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("shadowshell: {path}: {err}");
            return ExitCode::from(127);
        }
    };

    let mut shell = Shell::new();
    shell.env.shell_name = path.to_string();
    shell.env.set_positionals(positionals);

    let source = strip_shebang(&source);

    match parser::parse_program(source) {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(program)) => match interpret(&program, &mut shell) {
            InterpretOutcome::Completed(code) | InterpretOutcome::Return(code) => {
                to_exit_code(code)
            }
            InterpretOutcome::Exit(code) => to_exit_code(code),
        },
        Err(err) => {
            eprintln!("shadowshell: {path}: {err}");
            ExitCode::from(2)
        }
    }
}

fn run_source(source: &str, last_exit_code: i32, shell: &mut Shell) -> LoopControl {
    match parser::parse_program(source) {
        Ok(None) => LoopControl::Continue(last_exit_code),
        Ok(Some(program)) => match interpret(&program, shell) {
            InterpretOutcome::Completed(code) | InterpretOutcome::Return(code) => {
                LoopControl::Continue(code)
            }
            InterpretOutcome::Exit(code) => LoopControl::Exit(code),
        },
        Err(err) => {
            eprintln!("shadowshell: {err}");
            LoopControl::Continue(2)
        }
    }
}

fn strip_shebang(source: &str) -> &str {
    if let Some(rest) = source.strip_prefix("#!") {
        if let Some(pos) = rest.find('\n') {
            return &rest[pos + 1..];
        }
        return "";
    }
    source
}

fn warn_about_active_jobs(shell: &Shell) {
    let count = shell.jobs.list().len();
    if count > 0 {
        eprintln!(
            "shadowshell: warning: {count} job(s) still running or stopped; they will keep running after exit"
        );
    }
}

fn to_exit_code(code: i32) -> ExitCode {
    ExitCode::from((code & 0xFF) as u8)
}
