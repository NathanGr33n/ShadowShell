//! Built-in `help` topics and the first-run welcome banner.

/// Prints the first-run welcome message (once per user config dir).
pub fn print_welcome() {
    println!(
        "\
Welcome to ShadowShell — a modern dev shell with sensible defaults.

  Tab             complete files & commands
  Right arrow     accept autosuggestion
  alias           list shortcuts (try: ll, gs, cb)
  sleep 3 &       background job → live badge in the prompt
  help            more topics (keys, config, jobs, …)

Config:  ~/.config/shadowshell/config.toml
Replay:  shadowshell --welcome
"
    );
}

/// CLI `--help` / `-h` text.
pub fn print_cli_help(bin: &str) {
    println!(
        "\
{bin} — modern Unix shell (Linux/macOS)

USAGE:
  {bin}                       Interactive shell
  {bin} SCRIPT [ARGS...]      Run a script file
  {bin} -c COMMAND            Run one command and exit
  {bin} --welcome             Show the welcome banner and exit
  {bin} -h, --help            Show this help
  {bin} -V, --version         Show version

EXAMPLES:
  {bin}
  {bin} ./deploy.sh staging
  {bin} -c 'echo hi && ls'

Inside the shell, type  help  or  help <topic>.
"
    );
}

/// Handles the `help` builtin. Returns the exit status.
pub fn run_help(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None => {
            print_help_index();
            0
        }
        Some("keys" | "keybindings" | "editing") => {
            print_topic_keys();
            0
        }
        Some("aliases" | "alias") => {
            print_topic_aliases();
            0
        }
        Some("jobs" | "job") => {
            print_topic_jobs();
            0
        }
        Some("config" | "configuration") => {
            print_topic_config();
            0
        }
        Some("personality" | "project") => {
            print_topic_personality();
            0
        }
        Some("scripting" | "scripts" | "script") => {
            print_topic_scripting();
            0
        }
        Some("welcome") => {
            print_welcome();
            0
        }
        Some(other) => {
            eprintln!("help: unknown topic `{other}`");
            eprintln!("Try: help  (lists topics)");
            1
        }
    }
}

fn print_help_index() {
    println!(
        "\
ShadowShell help

Topics:
  help keys          Line editing & shortcuts
  help aliases       Built-in shortcuts
  help jobs          Background jobs & prompt badge
  help config        Configuration file
  help personality   Project-aware colors & aliases
  help scripting     Scripts and non-interactive use
  help welcome       First-run banner

Also:  alias   jobs   fg   bg   export   exit
"
    );
}

fn print_topic_keys() {
    println!(
        "\
Line editing (reedline)

  Up / Down          history
  Ctrl+R             reverse history search
  Tab                completions (files, commands, aliases)
  Shift+Tab          previous completion
  Right              accept autosuggestion (when at end of line)
  Ctrl+C             cancel line
  Ctrl+D             exit (empty line) / delete
  Ctrl+L             clear screen (terminal)
  \\ at EOL           continue on next line
  open quotes        multiline until closed
"
    );
}

fn print_topic_aliases() {
    println!(
        "\
Aliases

  alias                 list all
  alias name=value      define or override
  unalias name          remove one
  unalias -a            clear all

Common defaults:
  ll          ls -lah
  la          ls -A
  gs/ga/gc    git status / add / commit
  cb/ct/cr    cargo build / test / run
  ..          cd ..

Project personalities may add temporary aliases (e.g. Rust: b, t, r).
Those do not overwrite names you set yourself.
"
    );
}

fn print_topic_jobs() {
    println!(
        "\
Jobs

  cmd &              run in background
  jobs               list jobs
  fg [%n]            foreground
  bg [%n]            resume in background
  Ctrl+Z             stop foreground job

The right prompt shows a live badge while jobs run or are stopped:
  ⠋ sleep 30         one running job
  ⠋×2 ⏸×1            counts
  ⏸ [1] vim          stopped

Done notifications print when a background job finishes (also while idle).
"
    );
}

fn print_topic_config() {
    println!(
        "\
Configuration

  File:  ~/.config/shadowshell/config.toml
  Example is created on first run if missing.

  theme = \"onedark\"   # or \"nord\"
  history_capacity = 1000
  autosuggestions = true
  syntax_highlighting = true
  tab_completion = true

  [colors]
  cwd = [97, 175, 239]

  [personality]
  enabled = true

  [personality.rust]
  accent = [222, 163, 90]

Malformed files fall back to defaults with a warning.
See also:  help personality
"
    );
}

fn print_topic_personality() {
    println!(
        "\
Directory personality

When you enter a project tree (markers searched upward), the shell adapts:

  Marker              Kind     Badge
  Cargo.toml          Rust     rs
  package.json        Node     js
  pyproject.toml      Python   py
  go.mod              Go       go
  build.zig           Zig      zig
  CMakeLists.txt      CMake    c

Effects:
  • cwd prompt color accent
  • small badge on the right
  • temporary aliases (removed when you leave)
  • tab-completion priority for project tools

Disable or customize under [personality] in config.toml.
"
    );
}

fn print_topic_scripting() {
    println!(
        "\
Scripting

  shadowshell script.sh args...
  shadowshell -c 'echo hi'

Supports variables, export, if/while/for/case, functions, pipelines,
redirection, &&/||, and $(command) substitution.

  $0, $1, …   positional parameters in scripts
  $?          last exit status

Not a full bash clone — advanced features may be missing.
"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_unknown_topic_is_error() {
        assert_eq!(run_help(&["nope".into()]), 1);
    }

    #[test]
    fn help_index_ok() {
        assert_eq!(run_help(&[]), 0);
    }

    #[test]
    fn help_keys_ok() {
        assert_eq!(run_help(&["keys".into()]), 0);
    }
}
