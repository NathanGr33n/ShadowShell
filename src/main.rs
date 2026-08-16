//! ShadowShell entry point: interactive read-eval loop or script execution.
//! Interactive mode uses `reedline` with persistent history, autosuggestions,
//! syntax highlighting, and tab completion; scripts are read from a file path
//! given on the command line.

mod aliases;
mod builtins;
mod completer;
mod config;
mod env;
mod expand;
mod help;
mod highlighter;
mod interpreter;
mod job_control;
mod jobs;
mod line_editor;
mod live_jobs;
mod parser;
mod personality;
mod prompt;

use std::env as process_env;
use std::fs;
use std::process::ExitCode;

use reedline::Signal;

use config::FirstRun;
use interpreter::{InterpretOutcome, interpret};
use job_control::Shell;
use prompt::ShellPrompt;

/// What the main loop should do after processing one buffer of input.
enum LoopControl {
    Continue(i32),
    Exit(i32),
}

/// Parsed command-line invocation.
enum Invocation {
    Interactive { theme: Option<String> },
    Script { path: String, args: Vec<String> },
    Command(String),
    Help,
    Version,
    Welcome,
}

fn main() -> ExitCode {
    let mut argv = process_env::args();
    let bin = argv.next().unwrap_or_else(|| "shadowshell".into());
    match parse_args(argv.collect()) {
        Ok(Invocation::Help) => {
            help::print_cli_help(cli_name(&bin));
            ExitCode::SUCCESS
        }
        Ok(Invocation::Version) => {
            println!("{} {}", cli_name(&bin), env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Invocation::Welcome) => {
            help::print_welcome();
            ExitCode::SUCCESS
        }
        Ok(Invocation::Command(cmd)) => run_command_string(&cmd),
        Ok(Invocation::Script { path, args }) => run_script(&path, args),
        Ok(Invocation::Interactive { theme }) => run_interactive(theme.as_deref()),
        Err(msg) => {
            eprintln!("shadowshell: {msg}");
            eprintln!("Try `{} --help` for usage.", cli_name(&bin));
            ExitCode::from(2)
        }
    }
}

fn cli_name(bin: &str) -> &str {
    std::path::Path::new(bin)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("shadowshell")
}

fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    if args.is_empty() {
        return Ok(Invocation::Interactive { theme: None });
    }

    let mut theme: Option<String> = None;
    let mut command: Option<String> = None;
    let mut script: Option<(String, Vec<String>)> = None;
    let mut help = false;
    let mut version = false;
    let mut welcome = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "-h" || arg == "--help" {
            help = true;
            continue;
        }
        if arg == "-V" || arg == "--version" {
            version = true;
            continue;
        }
        if arg == "--welcome" {
            welcome = true;
            continue;
        }
        if arg == "-c" {
            let cmd = iter
                .next()
                .ok_or_else(|| "option requires an argument: -c".to_string())?;
            if command.is_some() {
                return Err("multiple -c options".into());
            }
            command = Some(cmd);
            continue;
        }
        if arg == "--theme" {
            let name = iter
                .next()
                .ok_or_else(|| "option requires an argument: --theme".to_string())?;
            if theme.is_some() {
                return Err("multiple --theme options".into());
            }
            theme = Some(name);
            continue;
        }
        if let Some(name) = arg.strip_prefix("--theme=") {
            if name.is_empty() {
                return Err("option requires an argument: --theme".into());
            }
            if theme.is_some() {
                return Err("multiple --theme options".into());
            }
            theme = Some(name.to_string());
            continue;
        }
        if arg.starts_with('-') {
            return Err(format!("unknown option: {arg}"));
        }

        if script.is_some() || command.is_some() {
            return Err("unexpected arguments".into());
        }
        let rest: Vec<String> = iter.collect();
        script = Some((arg, rest));
        break;
    }

    let mode_count = usize::from(help)
        + usize::from(version)
        + usize::from(welcome)
        + usize::from(command.is_some())
        + usize::from(script.is_some());
    if mode_count > 1 {
        return Err("conflicting options".into());
    }

    if help {
        return Ok(Invocation::Help);
    }
    if version {
        return Ok(Invocation::Version);
    }
    if welcome {
        return Ok(Invocation::Welcome);
    }
    if let Some(cmd) = command {
        if theme.is_some() {
            return Err("--theme only applies to interactive sessions".into());
        }
        return Ok(Invocation::Command(cmd));
    }
    if let Some((path, args)) = script {
        if theme.is_some() {
            return Err("--theme only applies to interactive sessions".into());
        }
        return Ok(Invocation::Script { path, args });
    }

    Ok(Invocation::Interactive { theme })
}

fn run_interactive(theme_override: Option<&str>) -> ExitCode {
    let first_run = config::ensure_user_config();
    if matches!(first_run, FirstRun::Fresh { .. }) {
        help::print_welcome();
        first_run.mark_welcome_shown();
    }

    let mut config = config::load();
    if let Some(name) = theme_override {
        match config::theme_by_name(name) {
            Some(theme) => config.theme = theme,
            None => {
                eprintln!(
                    "shadowshell: unknown theme `{name}` (try: {})",
                    config::BUILTIN_THEME_NAMES.join(", ")
                );
                return ExitCode::from(2);
            }
        }
    }
    let mut shell = Shell::new();
    let personality = std::sync::Arc::new(personality::PersonalityState::new(
        config.personality.clone(),
    ));
    apply_directory_personality(&mut shell, &personality);

    let mut line_editor = line_editor::build(
        &config,
        Some(std::sync::Arc::clone(&shell.live_jobs)),
        Some(std::sync::Arc::clone(&personality)),
    );
    let mut last_exit_code: i32 = 0;

    loop {
        shell.notify_job_changes();
        shell.sync_live_jobs();
        apply_directory_personality(&mut shell, &personality);

        let theme = personality.effective_theme(&config.theme);
        let prompt = ShellPrompt::new(
            last_exit_code,
            theme,
            std::sync::Arc::clone(&shell.live_jobs),
            Some(std::sync::Arc::clone(&personality)),
        );

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

fn run_command_string(source: &str) -> ExitCode {
    let mut shell = Shell::new();
    match parser::parse_program(source) {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(program)) => match interpret(&program, &mut shell) {
            InterpretOutcome::Completed(code) | InterpretOutcome::Return(code) => {
                to_exit_code(code)
            }
            InterpretOutcome::Exit(code) => to_exit_code(code),
        },
        Err(err) => {
            eprintln!("shadowshell: {err}");
            ExitCode::from(2)
        }
    }
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

/// Detect project type for the current directory and reconcile temporary
/// personality aliases. Theme accent is applied when building the prompt.
fn apply_directory_personality(shell: &mut Shell, personality: &personality::PersonalityState) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _changed = personality.update_for_cwd(&cwd);
    personality::reconcile_aliases(&mut shell.aliases, personality);
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn parse_help_flags() {
        assert!(matches!(
            parse_args(vec!["--help".into()]).unwrap(),
            Invocation::Help
        ));
        assert!(matches!(
            parse_args(vec!["-h".into()]).unwrap(),
            Invocation::Help
        ));
    }

    #[test]
    fn parse_version() {
        assert!(matches!(
            parse_args(vec!["-V".into()]).unwrap(),
            Invocation::Version
        ));
    }

    #[test]
    fn parse_command() {
        match parse_args(vec!["-c".into(), "echo hi".into()]).unwrap() {
            Invocation::Command(c) => assert_eq!(c, "echo hi"),
            _ => panic!("expected Command"),
        }
    }

    #[test]
    fn parse_script_with_args() {
        match parse_args(vec!["run.sh".into(), "a".into(), "b".into()]).unwrap() {
            Invocation::Script { path, args } => {
                assert_eq!(path, "run.sh");
                assert_eq!(args, vec!["a", "b"]);
            }
            _ => panic!("expected Script"),
        }
    }

    #[test]
    fn parse_empty_is_interactive() {
        assert!(matches!(
            parse_args(vec![]).unwrap(),
            Invocation::Interactive { theme: None }
        ));
    }

    #[test]
    fn parse_theme_flag() {
        match parse_args(vec!["--theme".into(), "nord".into()]).unwrap() {
            Invocation::Interactive { theme: Some(t) } => assert_eq!(t, "nord"),
            _ => panic!("expected Interactive with theme"),
        }
        match parse_args(vec!["--theme=onedark".into()]).unwrap() {
            Invocation::Interactive { theme: Some(t) } => assert_eq!(t, "onedark"),
            _ => panic!("expected Interactive with theme"),
        }
    }

    #[test]
    fn parse_theme_with_script_errs() {
        assert!(parse_args(vec!["--theme".into(), "nord".into(), "x.sh".into()]).is_err());
    }

    #[test]
    fn parse_unknown_flag_errs() {
        assert!(parse_args(vec!["--nope".into()]).is_err());
    }
}
