//! Interprets a parsed [`Program`]: expansions, assignments, control flow,
//! functions, and pipeline dispatch via the existing job-control executor.

use std::process::{Command as ProcessCommand, Stdio};

use crate::aliases;
use crate::builtins::{self, BuiltinOutcome};
use crate::expand::{expand_word_no_glob, expand_word_unsplit, expand_words};
use crate::help;
use crate::job_control::Shell;
use crate::parser::{
    AndOrOp, AstPipeline, AstSimpleCommand, Command, CompoundCommand, Program, Redirect,
    SimpleCommand,
};

/// Names of built-in commands recognized by the interpreter.
const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "jobs", "fg", "bg", "export", "unset", "return", "shift", "alias", "unalias",
    "help", "which", "type", ":", "true", "false",
];

/// Outcome of interpreting a program or command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpretOutcome {
    Completed(i32),
    Exit(i32),
    /// `return` from a function (status).
    Return(i32),
}

impl InterpretOutcome {
    pub fn code(self) -> i32 {
        match self {
            InterpretOutcome::Completed(c)
            | InterpretOutcome::Exit(c)
            | InterpretOutcome::Return(c) => c,
        }
    }
}

/// Interprets `program` against `shell`, updating `$?` and shell state.
pub fn interpret(program: &Program, shell: &mut Shell) -> InterpretOutcome {
    let mut outcome = InterpretOutcome::Completed(0);
    for list in &program.lists {
        outcome = interpret_and_or(list, shell);
        match outcome {
            InterpretOutcome::Exit(_) | InterpretOutcome::Return(_) => return outcome,
            InterpretOutcome::Completed(code) => shell.env.set_last_status(code),
        }
    }
    outcome
}

fn interpret_and_or(list: &crate::parser::AndOrList, shell: &mut Shell) -> InterpretOutcome {
    let mut outcome = interpret_pipeline(&list.first, shell);
    if matches!(
        outcome,
        InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)
    ) {
        return outcome;
    }
    let mut status = outcome.code();
    shell.env.set_last_status(status);

    for (op, pipe) in &list.rest {
        let run = match op {
            AndOrOp::And => status == 0,
            AndOrOp::Or => status != 0,
        };
        if !run {
            continue;
        }
        outcome = interpret_pipeline(pipe, shell);
        if matches!(
            outcome,
            InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)
        ) {
            return outcome;
        }
        status = outcome.code();
        shell.env.set_last_status(status);
    }
    InterpretOutcome::Completed(status)
}

fn interpret_pipeline(pipeline: &AstPipeline, shell: &mut Shell) -> InterpretOutcome {
    // Single simple command → may be built-in, function, assignment-only.
    if pipeline.commands.len() == 1 && !pipeline.background {
        match &pipeline.commands[0] {
            Command::Simple(simple) => return run_simple(simple, shell),
            Command::Compound(c) => return run_compound(c, shell),
            Command::FunctionDef { name, body } => {
                shell.functions.insert(name.clone(), (**body).clone());
                shell.env.set_last_status(0);
                return InterpretOutcome::Completed(0);
            }
        }
    }

    // Multi-command or background: expand each simple command into legacy form.
    // Compounds/functions inside pipelines are not supported.
    let mut legacy_commands = Vec::new();
    let mut display_parts = Vec::new();

    for cmd in &pipeline.commands {
        match cmd {
            Command::Simple(simple) => {
                let mut expanded = match expand_simple(simple, shell) {
                    Ok(e) => e,
                    Err(code) => return InterpretOutcome::Completed(code),
                };
                if !expanded.assignments_only {
                    let (program, args) = aliases::expand_command_words(
                        expanded.program,
                        expanded.args,
                        &shell.aliases,
                    );
                    expanded.program = program;
                    expanded.args = args;
                }
                if expanded.program.is_empty() && expanded.assignments_only {
                    // assignment-only in a pipeline is odd; just apply and use true
                    apply_assignments(&expanded.assignments, shell, false);
                    legacy_commands.push(SimpleCommand {
                        program: "true".into(),
                        args: vec![],
                        redirects: vec![],
                    });
                    display_parts.push("true".into());
                } else if is_builtin(&expanded.program)
                    || shell.functions.contains_key(&expanded.program)
                {
                    eprintln!(
                        "shadowshell: {}: built-ins/functions cannot be used in a pipeline or backgrounded yet",
                        expanded.program
                    );
                    return InterpretOutcome::Completed(1);
                } else {
                    display_parts.push(expanded.display_line());
                    legacy_commands.push(SimpleCommand {
                        program: expanded.program,
                        args: expanded.args,
                        redirects: expanded.redirects,
                    });
                }
            }
            _ => {
                eprintln!("shadowshell: compound commands cannot be used in a pipeline yet");
                return InterpretOutcome::Completed(1);
            }
        }
    }

    let legacy = crate::parser::Pipeline {
        commands: legacy_commands,
        background: pipeline.background,
    };
    let line = display_parts.join(" | ");
    let code = shell.run_pipeline(&legacy, &line);
    InterpretOutcome::Completed(code)
}

struct ExpandedSimple {
    assignments: Vec<(String, String)>,
    program: String,
    args: Vec<String>,
    redirects: Vec<Redirect>,
    assignments_only: bool,
}

impl ExpandedSimple {
    fn display_line(&self) -> String {
        let mut parts = Vec::new();
        if !self.program.is_empty() {
            parts.push(self.program.clone());
        }
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }
}

fn expand_simple(simple: &AstSimpleCommand, shell: &mut Shell) -> Result<ExpandedSimple, i32> {
    // Command substitution mutably borrows `shell`; snapshot env for pure
    // parameter expansion between substitution callbacks.
    let mut assignments = Vec::new();
    for a in &simple.assignments {
        let env_snap = shell.env.clone();
        let value = expand_word_unsplit(&a.value, &env_snap, |body| {
            run_command_substitution(body, shell)
        });
        assignments.push((a.name.clone(), value));
    }

    let env_snap = shell.env.clone();
    let fields = expand_words(&simple.words, &env_snap, |body| {
        run_command_substitution(body, shell)
    });
    let mut redirects = Vec::new();
    for r in &simple.redirects {
        let env_snap = shell.env.clone();
        let target = expand_word_unsplit(&r.target, &env_snap, |body| {
            run_command_substitution(body, shell)
        });
        redirects.push(Redirect {
            kind: r.kind,
            target,
        });
    }

    if fields.is_empty() {
        return Ok(ExpandedSimple {
            assignments,
            program: String::new(),
            args: vec![],
            redirects,
            assignments_only: true,
        });
    }

    let mut fields = fields;
    let program = fields.remove(0);
    Ok(ExpandedSimple {
        assignments,
        program,
        args: fields,
        redirects,
        assignments_only: false,
    })
}

fn apply_assignments(assignments: &[(String, String)], shell: &mut Shell, export: bool) {
    for (name, value) in assignments {
        if export {
            shell.env.export(name.clone(), Some(value.clone()));
        } else {
            shell.env.set(name.clone(), value.clone());
        }
    }
}

fn run_simple(simple: &AstSimpleCommand, shell: &mut Shell) -> InterpretOutcome {
    let mut expanded = match expand_simple(simple, shell) {
        Ok(e) => e,
        Err(code) => return InterpretOutcome::Completed(code),
    };

    if !expanded.assignments_only {
        let (program, args) =
            aliases::expand_command_words(expanded.program, expanded.args, &shell.aliases);
        expanded.program = program;
        expanded.args = args;
    }

    if expanded.assignments_only {
        apply_assignments(&expanded.assignments, shell, false);
        shell.env.set_last_status(0);
        return InterpretOutcome::Completed(0);
    }

    // Prefix assignments: apply only for the duration of an external command,
    // or permanently for built-ins/functions (bash: built-ins keep them).
    let is_func = shell.functions.contains_key(&expanded.program);
    let is_bi = is_builtin(&expanded.program);

    if is_bi || is_func {
        apply_assignments(&expanded.assignments, shell, false);
        if !expanded.redirects.is_empty() {
            eprintln!(
                "shadowshell: {}: redirects on built-ins/functions are not yet supported",
                expanded.program
            );
            return InterpretOutcome::Completed(1);
        }
        if is_func {
            return call_function(&expanded.program, &expanded.args, shell);
        }
        return run_builtin(&expanded.program, &expanded.args, shell);
    }

    // External: temporary env overlay via export for the child only.
    let saved: Vec<(String, Option<String>, bool)> = expanded
        .assignments
        .iter()
        .map(|(n, _)| {
            let prev = shell.env.get(n).map(str::to_string);
            let was_exported = shell.env.is_exported(n);
            (n.clone(), prev, was_exported)
        })
        .collect();

    for (name, value) in &expanded.assignments {
        shell.env.export(name.clone(), Some(value.clone()));
    }

    let legacy = crate::parser::Pipeline {
        commands: vec![SimpleCommand {
            program: expanded.program.clone(),
            args: expanded.args.clone(),
            redirects: expanded.redirects.clone(),
        }],
        background: false,
    };
    let line = expanded.display_line();
    let code = shell.run_pipeline(&legacy, &line);
    if code == 127 {
        suggest_similar(&expanded.program, shell);
    }

    // Restore previous values for prefix assignments.
    for (name, prev, was_exported) in saved {
        match prev {
            Some(v) => {
                if was_exported {
                    shell.env.export(name, Some(v));
                } else {
                    shell.env.unset(&name);
                    shell.env.set(name, v);
                }
            }
            None => {
                shell.env.unset(&name);
            }
        }
    }

    InterpretOutcome::Completed(code)
}

fn run_builtin(name: &str, args: &[String], shell: &mut Shell) -> InterpretOutcome {
    match name {
        "export" => InterpretOutcome::Completed(builtin_export(args, shell)),
        "unset" => InterpretOutcome::Completed(builtin_unset(args, shell)),
        "alias" => InterpretOutcome::Completed(builtin_alias(args, shell)),
        "unalias" => InterpretOutcome::Completed(builtin_unalias(args, shell)),
        "help" => {
            let code = help::run_help(args);
            shell.env.set_last_status(code);
            InterpretOutcome::Completed(code)
        }
        "which" => {
            let code = builtin_which(args, shell, false);
            shell.env.set_last_status(code);
            InterpretOutcome::Completed(code)
        }
        "type" => {
            let code = builtin_which(args, shell, true);
            shell.env.set_last_status(code);
            InterpretOutcome::Completed(code)
        }
        "return" => builtin_return(args, shell),
        "shift" => InterpretOutcome::Completed(builtin_shift(args, shell)),
        ":" | "true" => {
            shell.env.set_last_status(0);
            InterpretOutcome::Completed(0)
        }
        "false" => {
            shell.env.set_last_status(1);
            InterpretOutcome::Completed(1)
        }
        other => match builtins::try_run(other, args, shell) {
            Some(BuiltinOutcome::Ran(code)) => {
                shell.env.set_last_status(code);
                // Keep PWD in sync after cd.
                if other == "cd" && code == 0
                    && let Ok(cwd) = std::env::current_dir()
                {
                    let cwd_s = cwd.display().to_string();
                    if let Some(old) = shell.env.get("PWD").map(str::to_string) {
                        shell.env.set("OLDPWD", old);
                    }
                    shell.env.set("PWD", cwd_s.clone());
                    shell.env.export("PWD", Some(cwd_s));
                }
                InterpretOutcome::Completed(code)
            }
            Some(BuiltinOutcome::Exit(code)) => InterpretOutcome::Exit(code),
            None => InterpretOutcome::Completed(127),
        },
    }
}

fn builtin_alias(args: &[String], shell: &mut Shell) -> i32 {
    if args.is_empty() {
        let mut names: Vec<_> = shell.aliases.keys().cloned().collect();
        names.sort();
        for name in names {
            if let Some(value) = shell.aliases.get(&name) {
                println!("alias {name}='{value}'");
            }
        }
        return 0;
    }
    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            if !aliases::is_valid_alias_name(name) {
                eprintln!("alias: `{name}': invalid alias name");
                return 1;
            }
            shell.aliases.insert(name.to_string(), value.to_string());
        } else if let Some(value) = shell.aliases.get(arg) {
            println!("alias {arg}='{value}'");
        } else {
            eprintln!("alias: {arg}: not found");
            return 1;
        }
    }
    0
}

fn builtin_unalias(args: &[String], shell: &mut Shell) -> i32 {
    if args.is_empty() {
        eprintln!("unalias: usage: unalias name [name ...]");
        return 1;
    }
    if args.len() == 1 && args[0] == "-a" {
        shell.aliases.clear();
        return 0;
    }
    let mut status = 0;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            eprintln!("unalias: {name}: not found");
            status = 1;
        }
    }
    status
}

fn builtin_export(args: &[String], shell: &mut Shell) -> i32 {
    if args.is_empty() {
        for (k, v) in shell.env.exported_pairs() {
            println!("export {k}={v}");
        }
        return 0;
    }
    for arg in args {
        if let Some((name, value)) = arg.split_once('=') {
            if !is_valid_name(name) {
                eprintln!("export: `{name}': not a valid identifier");
                return 1;
            }
            shell.env.export(name, Some(value.to_string()));
        } else {
            if !is_valid_name(arg) {
                eprintln!("export: `{arg}': not a valid identifier");
                return 1;
            }
            shell.env.export(arg.clone(), None);
        }
    }
    0
}

fn builtin_unset(args: &[String], shell: &mut Shell) -> i32 {
    for arg in args {
        shell.env.unset(arg);
        shell.functions.remove(arg);
    }
    0
}

fn builtin_return(args: &[String], shell: &mut Shell) -> InterpretOutcome {
    if shell.function_depth == 0 {
        eprintln!("return: can only `return' from a function");
        shell.env.set_last_status(1);
        return InterpretOutcome::Completed(1);
    }
    let code = match args.first() {
        None => shell.env.last_status(),
        Some(s) => match s.parse::<i32>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("return: {s}: numeric argument required");
                1
            }
        },
    };
    InterpretOutcome::Return(code)
}

fn builtin_shift(args: &[String], shell: &mut Shell) -> i32 {
    let n = match args.first() {
        None => 1usize,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("shift: {s}: numeric argument required");
                return 1;
            }
        },
    };
    if shell.env.shift(n) { 0 } else { 1 }
}

fn call_function(name: &str, args: &[String], shell: &mut Shell) -> InterpretOutcome {
    let body = match shell.functions.get(name).cloned() {
        Some(b) => b,
        None => return InterpretOutcome::Completed(127),
    };

    let saved_positionals: Vec<String> = (1..=shell.env.positional_count())
        .filter_map(|i| shell.env.positional(i).map(str::to_string))
        .collect();

    shell
        .env
        .set_positionals(args.to_vec());
    shell.function_depth += 1;

    let outcome = run_compound(&body, shell);

    shell.function_depth -= 1;
    shell.env.set_positionals(saved_positionals);

    match outcome {
        InterpretOutcome::Return(code) => {
            shell.env.set_last_status(code);
            InterpretOutcome::Completed(code)
        }
        other => other,
    }
}

fn run_compound(compound: &CompoundCommand, shell: &mut Shell) -> InterpretOutcome {
    match compound {
        CompoundCommand::BraceGroup(prog) => interpret(prog, shell),
        CompoundCommand::Subshell(prog) => {
            // Approximate subshell: clone env, run, discard env changes
            // except last status. Jobs still share the table (limitation).
            let saved_env = shell.env.clone();
            let saved_funcs = shell.functions.clone();
            let outcome = interpret(prog, shell);
            let status = outcome.code();
            shell.env = saved_env;
            shell.functions = saved_funcs;
            shell.env.set_last_status(status);
            match outcome {
                InterpretOutcome::Exit(c) => InterpretOutcome::Exit(c),
                InterpretOutcome::Return(c) => InterpretOutcome::Return(c),
                InterpretOutcome::Completed(_) => InterpretOutcome::Completed(status),
            }
        }
        CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        } => {
            let cond = interpret(condition, shell);
            if matches!(
                cond,
                InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)
            ) {
                return cond;
            }
            if cond.code() == 0 {
                return interpret(then_branch, shell);
            }
            for (elif_cond, elif_body) in elif_branches {
                let c = interpret(elif_cond, shell);
                if matches!(c, InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)) {
                    return c;
                }
                if c.code() == 0 {
                    return interpret(elif_body, shell);
                }
            }
            if let Some(else_body) = else_branch {
                interpret(else_body, shell)
            } else {
                InterpretOutcome::Completed(0)
            }
        }
        CompoundCommand::While { condition, body } => {
            let mut last = 0;
            loop {
                let cond = interpret(condition, shell);
                if matches!(
                    cond,
                    InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)
                ) {
                    return cond;
                }
                if cond.code() != 0 {
                    break;
                }
                let b = interpret(body, shell);
                match b {
                    InterpretOutcome::Exit(_) | InterpretOutcome::Return(_) => return b,
                    InterpretOutcome::Completed(c) => last = c,
                }
            }
            InterpretOutcome::Completed(last)
        }
        CompoundCommand::Until { condition, body } => {
            let mut last = 0;
            loop {
                let cond = interpret(condition, shell);
                if matches!(
                    cond,
                    InterpretOutcome::Exit(_) | InterpretOutcome::Return(_)
                ) {
                    return cond;
                }
                if cond.code() == 0 {
                    break;
                }
                let b = interpret(body, shell);
                match b {
                    InterpretOutcome::Exit(_) | InterpretOutcome::Return(_) => return b,
                    InterpretOutcome::Completed(c) => last = c,
                }
            }
            InterpretOutcome::Completed(last)
        }
        CompoundCommand::For { name, words, body } => {
            let env_snap = shell.env.clone();
            let items = expand_words(words, &env_snap, |b| run_command_substitution(b, shell));
            let mut last = 0;
            for item in items {
                shell.env.set(name.clone(), item);
                let b = interpret(body, shell);
                match b {
                    InterpretOutcome::Exit(_) | InterpretOutcome::Return(_) => return b,
                    InterpretOutcome::Completed(c) => last = c,
                }
            }
            InterpretOutcome::Completed(last)
        }
        CompoundCommand::Case { word, arms } => {
            let env_snap = shell.env.clone();
            let subject =
                expand_word_unsplit(word, &env_snap, |b| run_command_substitution(b, shell));
            for arm in arms {
                for pat in &arm.patterns {
                    let env_snap = shell.env.clone();
                    let pat_s =
                        expand_word_no_glob(pat, &env_snap, |b| run_command_substitution(b, shell));
                    if case_match(&subject, &pat_s) {
                        return interpret(&arm.body, shell);
                    }
                }
            }
            InterpretOutcome::Completed(0)
        }
    }
}

/// Glob-style case pattern match (`*` `?` `[...]`).
fn case_match(subject: &str, pattern: &str) -> bool {
    // Use the `glob` crate's Pattern against the subject as a path-like string.
    // Patterns are matched against the whole string.
    match glob::Pattern::new(pattern) {
        Ok(p) => p.matches(subject),
        Err(_) => subject == pattern,
    }
}

/// Runs `body` as a nested program, capturing stdout. Does not steal the TTY.
fn run_command_substitution(body: &str, shell: &mut Shell) -> String {
    let program = match crate::parser::parse_program(body) {
        Ok(Some(p)) => p,
        Ok(None) => return String::new(),
        Err(_) => return String::new(),
    };

    // For simple single external commands, capture via std::process.
    // For richer scripts, fall back to interpreting without capture of
    // shell built-in output (limitation): run via `sh -c` only when the
    // body is a single external — otherwise interpret and capture nothing
    // from built-ins. Better approach: if the whole program is one simple
    // external pipeline, spawn with piped stdout.
    if let Some(capture) = try_capture_external(&program, shell) {
        return trim_trailing_newlines(&capture);
    }

    // Fallback: interpret for side effects (assignments) without stdout.
    let _ = interpret(&program, shell);
    String::new()
}

fn try_capture_external(program: &Program, shell: &Shell) -> Option<String> {
    if program.lists.len() != 1 || !program.lists[0].rest.is_empty() {
        return None;
    }
    let pipe = &program.lists[0].first;
    if pipe.background || pipe.commands.len() != 1 {
        return None;
    }
    let Command::Simple(simple) = &pipe.commands[0] else {
        return None;
    };
    if !simple.assignments.is_empty() || !simple.redirects.is_empty() {
        return None;
    }

    // Expand without nested command_sub (empty).
    let fields = expand_words(&simple.words, &shell.env, |_| String::new());
    if fields.is_empty() {
        return None;
    }
    if is_builtin(&fields[0]) || shell.functions.contains_key(&fields[0]) {
        return None;
    }

    let mut cmd = ProcessCommand::new(&fields[0]);
    if fields.len() > 1 {
        cmd.args(&fields[1..]);
    }
    cmd.env_clear();
    cmd.envs(shell.env.child_env());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::inherit());

    let output = cmd.output().ok()?;
    String::from_utf8(output.stdout).ok()
}

fn trim_trailing_newlines(s: &str) -> String {
    let mut end = s.len();
    let bytes = s.as_bytes();
    while end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    s[..end].to_string()
}

fn builtin_which(args: &[String], shell: &Shell, verbose: bool) -> i32 {
    if args.is_empty() {
        eprintln!(
            "{}: usage: {} name [name ...]",
            if verbose { "type" } else { "which" },
            if verbose { "type" } else { "which" }
        );
        return 1;
    }
    let mut status = 0;
    for name in args {
        if is_builtin(name) {
            if verbose {
                println!("{name} is a shell builtin");
            } else {
                println!("{name}: shell builtin");
            }
            continue;
        }
        if let Some(value) = shell.aliases.get(name) {
            if verbose {
                println!("{name} is aliased to `{value}'");
            } else {
                println!("{name}: aliased to `{value}'");
            }
            continue;
        }
        if shell.functions.contains_key(name) {
            if verbose {
                println!("{name} is a shell function");
            } else {
                println!("{name}: shell function");
            }
            continue;
        }
        match find_on_path(name) {
            Some(path) => println!("{}", path.display()),
            None => {
                eprintln!(
                    "{}: {name}: not found",
                    if verbose { "type" } else { "which" }
                );
                status = 1;
            }
        }
    }
    status
}

fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn suggest_similar(name: &str, shell: &Shell) {
    let mut candidates: Vec<String> = BUILTIN_NAMES.iter().map(|s| (*s).to_string()).collect();
    candidates.extend(shell.aliases.keys().cloned());
    candidates.extend(shell.functions.keys().cloned());
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for ent in rd.flatten() {
                if let Some(n) = ent.file_name().to_str() {
                    // Keep PATH scan cheap: same first character or shared prefix.
                    if name.is_empty()
                        || n.starts_with(name)
                        || name.starts_with(n)
                        || n.chars().next() == name.chars().next()
                    {
                        candidates.push(n.to_string());
                    }
                }
            }
        }
    }
    candidates.sort();
    candidates.dedup();

    let mut scored: Vec<(usize, String)> = candidates
        .into_iter()
        .filter_map(|c| {
            if c == name {
                return None;
            }
            // Prefer prefix hits, then small edit distance.
            if c.starts_with(name) || name.starts_with(&c) {
                let d = c.len().abs_diff(name.len());
                return Some((d, c));
            }
            let d = edit_distance(name, &c);
            if d > 0 && d <= 2 {
                Some((d + 10, c)) // rank after pure prefix matches
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored.dedup_by(|a, b| a.1 == b.1);

    let suggestions: Vec<&str> = scored.iter().take(3).map(|(_, s)| s.as_str()).collect();
    match suggestions.as_slice() {
        [] => {}
        [one] => eprintln!("shadowshell: did you mean `{one}`?"),
        many => eprintln!(
            "shadowshell: did you mean {}?",
            many.iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    if n.abs_diff(m) > 2 {
        return 3;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    fn run(src: &str) -> i32 {
        let mut shell = Shell::for_test();
        let prog = parse_program(src).unwrap().unwrap();
        interpret(&prog, &mut shell).code()
    }

    fn run_with(src: &str, f: impl FnOnce(&mut Shell)) -> (i32, Shell) {
        let mut shell = Shell::for_test();
        f(&mut shell);
        let prog = parse_program(src).unwrap().unwrap();
        let code = interpret(&prog, &mut shell).code();
        (code, shell)
    }

    #[test]
    fn true_false_status() {
        assert_eq!(run("true"), 0);
        assert_eq!(run("false"), 1);
    }

    #[test]
    fn and_or_short_circuit() {
        assert_eq!(run("true && true"), 0);
        assert_eq!(run("true && false"), 1);
        assert_eq!(run("false || true"), 0);
        assert_eq!(run("false && true"), 1);
    }

    #[test]
    fn assignment_and_expand() {
        let (code, shell) = run_with("FOO=hello", |_| {});
        assert_eq!(code, 0);
        assert_eq!(shell.env.get("FOO"), Some("hello"));
    }

    #[test]
    fn export_reaches_child() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "shadowshell-interp-export-{}.txt",
            std::process::id()
        ));
        let src = format!(
            "export SHADOWSHELL_IX=xyz; sh -c 'printf %s \"${{SHADOWSHELL_IX}}\"' > {}",
            path.display()
        );
        assert_eq!(run(&src), 0);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "xyz");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn if_then_else() {
        assert_eq!(run("if true; then true; else false; fi"), 0);
        assert_eq!(run("if false; then false; else true; fi"), 0);
        assert_eq!(run("if false; then true; fi"), 0);
    }

    #[test]
    fn for_loop_sets_var() {
        let (code, shell) = run_with("for x in a b c; do LAST=$x; done", |_| {});
        assert_eq!(code, 0);
        assert_eq!(shell.env.get("LAST"), Some("c"));
    }

    #[test]
    fn while_loop() {
        // n starts empty → set and count with external true once
        assert_eq!(run("while false; do false; done"), 0);
    }

    #[test]
    fn function_call_and_return() {
        let src = "f() { return 7; }; f";
        assert_eq!(run(src), 7);
    }

    #[test]
    fn status_special_param() {
        let (code, shell) = run_with("false; X=$?", |_| {});
        assert_eq!(code, 0); // assignment status
        assert_eq!(shell.env.get("X"), Some("1"));
    }

    #[test]
    fn command_substitution_external() {
        let (code, shell) = run_with("X=$(printf hi)", |_| {});
        assert_eq!(code, 0);
        assert_eq!(shell.env.get("X"), Some("hi"));
    }

    #[test]
    fn case_statement() {
        assert_eq!(run("case foo in bar) false;; foo) true;; esac"), 0);
        assert_eq!(run("case foo in bar) true;; *) false;; esac"), 1);
    }

    #[test]
    fn default_alias_ll_is_present() {
        let shell = Shell::for_test();
        assert_eq!(shell.aliases.get("ll").map(String::as_str), Some("ls -lah"));
    }

    #[test]
    fn alias_builtin_sets_and_lists() {
        let (code, shell) = run_with("alias foo=echo", |_| {});
        assert_eq!(code, 0);
        assert_eq!(shell.aliases.get("foo").map(String::as_str), Some("echo"));
    }

    #[test]
    fn unalias_removes() {
        let (code, shell) = run_with("unalias ll", |_| {});
        assert_eq!(code, 0);
        assert!(!shell.aliases.contains_key("ll"));
    }

    #[test]
    fn alias_expands_before_execution() {
        // true is a built-in; alias t=true then t should succeed
        assert_eq!(run("alias t=true; t"), 0);
        assert_eq!(run("alias f=false; f"), 1);
    }

    #[test]
    fn which_resolves_builtin() {
        assert_eq!(run("which true"), 0);
    }

    #[test]
    fn which_resolves_alias() {
        assert_eq!(run("which ll"), 0);
    }

    #[test]
    fn which_missing_is_error() {
        assert_ne!(run("which shadowshell-no-such-cmd-xyz"), 0);
    }

    #[test]
    fn type_resolves_builtin() {
        assert_eq!(run("type cd"), 0);
    }

    #[test]
    fn edit_distance_basic() {
        assert_eq!(edit_distance("cat", "bat"), 1);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("", "ab"), 2);
    }

    #[test]
    fn suggest_similar_prefix_preferred() {
        let shell = Shell::for_test();
        // Should not panic; visual check via stderr not asserted here.
        suggest_similar("hel", &shell);
        suggest_similar("cdd", &shell);
    }
}
