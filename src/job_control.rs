//! OS-level job control: process groups, terminal ownership, and signal
//! handling for interactive foreground/background execution.
//!
//! Each pipeline is spawned into its own process group. A foreground
//! pipeline is temporarily given control of the terminal (via
//! `tcsetpgrp`) so the kernel delivers terminal-generated signals
//! (Ctrl+C/Ctrl+Z) to it directly instead of to the shell; the shell
//! reclaims the terminal once the pipeline finishes or stops. Background
//! pipelines are never given the terminal and are tracked in the job
//! table until they finish.
//!
//! Scope note: this implementation assumes the shell starts already in
//! the foreground of its controlling terminal. It does not implement the
//! classic "wait until we are the foreground process group" `SIGTTIN`
//! loop used by shells that might be launched directly as a background
//! job of another shell; that scenario is rare and out of scope here.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use nix::sys::signal::{self, SigHandler, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{self, Pid};

use crate::aliases;
use crate::env::ShellEnv;
use crate::jobs::{JobStatus, JobTable};
use crate::parser::{CompoundCommand, Pipeline, Redirect, RedirectKind};

/// Exit-code convention used when a foreground job is stopped (Ctrl+Z):
/// 128 + signal number, matching bash's `$?` convention.
const STOPPED_EXIT_CODE: i32 = 128 + Signal::SIGTSTP as i32;

/// Signals whose disposition an interactive shell changes for itself and
/// must reset to default in every spawned child before `exec`.
const JOB_CONTROL_SIGNALS: [Signal; 5] = [
    Signal::SIGINT,
    Signal::SIGQUIT,
    Signal::SIGTSTP,
    Signal::SIGTTIN,
    Signal::SIGTTOU,
];

/// Owns the job table, shell environment, and the OS-level state needed for
/// interactive job control: the shell's own process group and whether it is
/// attached to a controlling terminal.
pub struct Shell {
    pub jobs: JobTable,
    pub env: ShellEnv,
    /// Shell functions defined at runtime (`name() { ... }`).
    pub functions: HashMap<String, CompoundCommand>,
    /// Command aliases (name → replacement words string).
    pub aliases: HashMap<String, String>,
    /// Nesting depth of active function calls (for `return`).
    pub function_depth: usize,
    pgid: Pid,
    interactive: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    /// Sets up the shell for job control. Only performs terminal/signal
    /// setup when standard input is a TTY; otherwise the shell still
    /// tracks jobs but never attempts terminal ownership changes (e.g.
    /// when input is piped or redirected).
    pub fn new() -> Self {
        let interactive = unistd::isatty(io::stdin()).unwrap_or(false);
        let mut pgid = unistd::getpgrp();

        if interactive {
            // Ensure the shell is its own process group leader (a no-op
            // in the common case where it already is).
            let _ = unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0));
            pgid = unistd::getpgrp();

            for sig in JOB_CONTROL_SIGNALS {
                // SAFETY: installing `SigIgn` only records a disposition
                // with the kernel; it runs no handler code, so there is
                // nothing signal-safety-sensitive about calling this here.
                let _ = unsafe { signal::signal(sig, SigHandler::SigIgn) };
            }

            // Claim the terminal for the shell's own process group.
            let _ = unistd::tcsetpgrp(io::stdin(), pgid);
        }

        Shell {
            jobs: JobTable::new(),
            env: ShellEnv::from_process_env("shadowshell"),
            functions: HashMap::new(),
            aliases: aliases::default_alias_map(),
            function_depth: 0,
            pgid,
            interactive,
        }
    }

    /// Runs `pipeline` (the original `command_line` is kept for job-table
    /// display). Returns the exit code to report as `$?` and updates the
    /// shell environment's last status.
    pub fn run_pipeline(&mut self, pipeline: &Pipeline, command_line: &str) -> i32 {
        let code = self.run_pipeline_inner(pipeline, command_line);
        self.env.set_last_status(code);
        code
    }

    fn run_pipeline_inner(&mut self, pipeline: &Pipeline, command_line: &str) -> i32 {
        let mut children: Vec<Child> = Vec::with_capacity(pipeline.commands.len());
        let mut pgid: Option<Pid> = None;
        let last_index = pipeline.commands.len().saturating_sub(1);
        let mut prev_stdout: Option<std::process::ChildStdout> = None;

        for (i, cmd) in pipeline.commands.iter().enumerate() {
            let mut command = Command::new(&cmd.program);
            command.args(&cmd.args);
            // Replace the inherited process environment with the shell's
            // exported variable set so `export`/`unset` affect children.
            command.env_clear();
            command.envs(self.env.child_env());

            if let Some(stdout) = prev_stdout.take() {
                command.stdin(Stdio::from(stdout));
            }
            if i != last_index {
                command.stdout(Stdio::piped());
            }

            if let Err(err) = apply_redirects(&mut command, &cmd.redirects) {
                eprintln!("shadowshell: {err}");
                kill_all(&mut children);
                return 1;
            }

            // SAFETY: this closure runs in the forked child, after fork
            // but before exec, and only resets signal dispositions via
            // `signal(2)`, which is async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    reset_job_control_signals();
                    Ok(())
                });
            }
            // Every command in a pipeline joins the same process group,
            // led by the first command; `process_group(0)` creates a new
            // group led by the spawned child itself.
            match pgid {
                Some(pg) => {
                    command.process_group(pg.as_raw());
                }
                None => {
                    command.process_group(0);
                }
            }

            let mut child = match command.spawn() {
                Ok(c) => c,
                Err(err) => {
                    eprintln!(
                        "shadowshell: {}: {}",
                        cmd.program,
                        describe_spawn_error(&err)
                    );
                    kill_all(&mut children);
                    return 127;
                }
            };

            let child_pid = Pid::from_raw(child.id() as i32);
            let group = pgid.unwrap_or(child_pid);
            // Redundant, race-safe parent-side setpgid: whichever of the
            // parent (here) or the child (via `process_group` above) runs
            // first wins; the other's call is a harmless no-op.
            let _ = unistd::setpgid(child_pid, group);
            pgid = Some(group);

            prev_stdout = child.stdout.take();
            children.push(child);
        }

        let Some(pgid) = pgid else {
            // Parser invariant: a pipeline always has at least one command.
            return 0;
        };
        let pids: Vec<Pid> = children
            .iter()
            .map(|c| Pid::from_raw(c.id() as i32))
            .collect();

        if pipeline.background {
            let id = self.jobs.add(pgid, pids, command_line.to_string());
            println!("[{id}] {pgid}");
            return 0;
        }

        if self.interactive {
            let _ = unistd::tcsetpgrp(io::stdin(), pgid);
        }

        let exit_code = self.wait_for_group(pgid, &pids, command_line);

        if self.interactive {
            let _ = unistd::tcsetpgrp(io::stdin(), self.pgid);
        }

        exit_code
    }

    /// Resumes job `id` (or the most recently active job if `id` is
    /// `None`) by sending it `SIGCONT`. If `foreground` is true, the
    /// terminal is handed to it and the shell blocks until it finishes or
    /// stops again (`fg`); otherwise it resumes in the background and
    /// this returns immediately (`bg`).
    pub fn resume_job(&mut self, id: Option<u32>, foreground: bool) -> Result<i32, String> {
        let id = id
            .or_else(|| self.jobs.most_recent_active_id())
            .ok_or_else(|| "no such job".to_string())?;
        let (pgid, pids, command_line) = {
            let job = self.jobs.get(id).ok_or_else(|| format!("{id}: no such job"))?;
            (job.pgid, job.pids.clone(), job.command_line.clone())
        };

        if foreground {
            println!("{command_line}");
        } else {
            println!("[{id}] {command_line} &");
        }

        signal::killpg(pgid, Signal::SIGCONT).map_err(|err| format!("{id}: {err}"))?;

        if !foreground {
            self.jobs.set_status_by_pgid(pgid, JobStatus::Running);
            return Ok(0);
        }

        if self.interactive {
            let _ = unistd::tcsetpgrp(io::stdin(), pgid);
        }

        let exit_code = self.wait_for_group(pgid, &pids, &command_line);

        if self.interactive {
            let _ = unistd::tcsetpgrp(io::stdin(), self.pgid);
        }

        self.env.set_last_status(exit_code);
        Ok(exit_code)
    }

    /// Polls all `Running` background jobs for status changes without
    /// blocking, then prints bash-style notifications (e.g. `[1]+  Done`)
    /// for any that finished. Intended to be called just before each new
    /// prompt.
    pub fn notify_job_changes(&mut self) {
        let running: Vec<(Pid, Vec<Pid>)> = self
            .jobs
            .list()
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Running))
            .map(|j| (j.pgid, j.pids.clone()))
            .collect();

        for (pgid, pids) in running {
            let last_pid = pids.last().copied();
            let mut remaining = pids.len();
            let mut finished_code = None;

            while remaining > 0 {
                let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED;
                match waitpid(Pid::from_raw(-pgid.as_raw()), Some(flags)) {
                    Ok(WaitStatus::Exited(pid, code)) => {
                        remaining -= 1;
                        if Some(pid) == last_pid {
                            finished_code = Some(code);
                        }
                    }
                    Ok(WaitStatus::Signaled(pid, sig, _)) => {
                        remaining -= 1;
                        if Some(pid) == last_pid {
                            finished_code = Some(128 + sig as i32);
                        }
                    }
                    Ok(WaitStatus::Stopped(_, _)) => {
                        self.jobs.set_status_by_pgid(pgid, JobStatus::Stopped);
                        break;
                    }
                    // StillAlive (WNOHANG, nothing to report yet), an
                    // unexpected status, or an error (e.g. already fully
                    // reaped): stop polling this job for now.
                    _ => break,
                }
            }

            if remaining == 0 {
                self.jobs
                    .set_status_by_pgid(pgid, JobStatus::Done(finished_code.unwrap_or(0)));
            }
        }

        for job in self.jobs.take_done() {
            if let JobStatus::Done(code) = job.status {
                let label = if code == 0 {
                    "Done".to_string()
                } else {
                    format!("Exit {code}")
                };
                println!("[{}]+  {label:<22} {}", job.id, job.command_line);
            }
        }
    }

    /// Waits for every process in `pgid` to finish, reporting the exit
    /// status of `pids`'s last entry as the pipeline's own exit status
    /// (shell convention). If any member stops instead (e.g. Ctrl+Z), the
    /// whole group is recorded as a `Stopped` job and a stopped-job exit
    /// code is returned immediately, leaving the rest of the group
    /// stopped for a later `fg`/`bg`.
    fn wait_for_group(&mut self, pgid: Pid, pids: &[Pid], command_line: &str) -> i32 {
        let last_pid = pids.last().copied();
        let mut remaining = pids.len();
        let mut exit_code = 0;

        while remaining > 0 {
            match waitpid(Pid::from_raw(-pgid.as_raw()), Some(WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Exited(pid, code)) => {
                    remaining -= 1;
                    if Some(pid) == last_pid {
                        exit_code = code;
                    }
                }
                Ok(WaitStatus::Signaled(pid, sig, _)) => {
                    remaining -= 1;
                    if Some(pid) == last_pid {
                        exit_code = 128 + sig as i32;
                    }
                }
                Ok(WaitStatus::Stopped(_, _)) => {
                    let id = self.record_stopped(pgid, pids.to_vec(), command_line);
                    println!("\n[{id}]+  Stopped                 {command_line}");
                    return STOPPED_EXIT_CODE;
                }
                // Continued or another non-terminal status: keep waiting.
                Ok(_) => {}
                Err(_) => break,
            }
        }

        // The group fully finished; if it had previously been tracked
        // (resumed via `fg` from a Stopped state), its job entry is no
        // longer needed since we report the result directly here.
        self.jobs.remove_by_pgid(pgid);
        exit_code
    }

    /// Records `pgid` as `Stopped` in the job table, adding it first if it
    /// wasn't already tracked (the common case for a freshly-launched
    /// foreground pipeline that gets suspended).
    fn record_stopped(&mut self, pgid: Pid, pids: Vec<Pid>, command_line: &str) -> u32 {
        match self.jobs.set_status_by_pgid(pgid, JobStatus::Stopped) {
            Some(id) => id,
            None => {
                let id = self.jobs.add(pgid, pids, command_line.to_string());
                self.jobs.set_status_by_pgid(pgid, JobStatus::Stopped);
                id
            }
        }
    }
}

#[cfg(test)]
impl Shell {
    /// Test-only constructor that skips the TTY/terminal ownership setup
    /// entirely, regardless of whether the test process happens to have a
    /// controlling terminal, so tests never touch real terminal state.
    /// Crate-visible so other modules' tests (e.g. `executor`) can also
    /// construct a `Shell` without risking real terminal state.
    pub(crate) fn for_test() -> Self {
        Shell {
            jobs: JobTable::new(),
            env: ShellEnv::from_process_env("shadowshell"),
            functions: HashMap::new(),
            aliases: aliases::default_alias_map(),
            function_depth: 0,
            pgid: unistd::getpgrp(),
            interactive: false,
        }
    }
}

/// Resets job-control-related signal dispositions to their defaults. Must
/// only be called in a forked child before `exec`: installing `SigDfl` is
/// async-signal-safe (see `signal(7)`), unlike most other operations.
fn reset_job_control_signals() {
    for sig in JOB_CONTROL_SIGNALS {
        // SAFETY: called only from a `pre_exec` child hook, immediately
        // after `fork` and before `exec`; see function-level comment.
        let _ = unsafe { signal::signal(sig, SigHandler::SigDfl) };
    }
}

/// Opens and attaches each of `redirects` to `command`, applied in order
/// so a later redirect for the same stream overrides an earlier one.
fn apply_redirects(command: &mut Command, redirects: &[Redirect]) -> Result<(), String> {
    for redirect in redirects {
        let result = match redirect.kind {
            RedirectKind::Stdin => {
                File::open(&redirect.target).map(|f| command.stdin(Stdio::from(f)))
            }
            RedirectKind::StdoutTruncate => {
                File::create(&redirect.target).map(|f| command.stdout(Stdio::from(f)))
            }
            RedirectKind::StdoutAppend => OpenOptions::new()
                .create(true)
                .append(true)
                .open(&redirect.target)
                .map(|f| command.stdout(Stdio::from(f))),
            RedirectKind::StderrTruncate => {
                File::create(&redirect.target).map(|f| command.stderr(Stdio::from(f)))
            }
            RedirectKind::StderrAppend => OpenOptions::new()
                .create(true)
                .append(true)
                .open(&redirect.target)
                .map(|f| command.stderr(Stdio::from(f))),
        };
        result.map_err(|err| format!("{}: {}", redirect.target, err))?;
    }
    Ok(())
}

/// Translates a process-spawn I/O error into a user-facing message,
/// special-casing the errors a shell user is most likely to hit.
fn describe_spawn_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "command not found".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => err.to_string(),
    }
}

/// Best-effort termination of already-spawned pipeline members after a
/// later stage failed to spawn or set up redirects, so a partially-started
/// pipeline doesn't linger. Also reaps each killed child so it doesn't sit
/// around as a zombie for the rest of the shell's lifetime (these children
/// were never registered in the job table, so nothing else will ever wait
/// on them).
fn kill_all(children: &mut [Child]) {
    for child in children {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_line;
    use std::io::Read;

    fn pipeline(line: &str) -> Pipeline {
        parse_line(line).unwrap().unwrap()
    }

    #[test]
    fn single_external_command_success() {
        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(&pipeline("true"), "true");
        assert_eq!(code, 0);
    }

    #[test]
    fn single_external_command_failure_reports_nonzero() {
        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(&pipeline("false"), "false");
        assert_ne!(code, 0);
    }

    #[test]
    fn missing_command_reports_127() {
        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(
            &pipeline("shadowshell-nonexistent-command"),
            "shadowshell-nonexistent-command",
        );
        assert_eq!(code, 127);
    }

    #[test]
    fn two_stage_pipeline_reports_last_command_exit_code() {
        let mut shell = Shell::for_test();
        // `false | true`: last command succeeds, so the pipeline succeeds
        // (no pipefail support in this phase, matching default shell
        // behavior).
        let code = shell.run_pipeline(&pipeline("false | true"), "false | true");
        assert_eq!(code, 0);
    }

    #[test]
    fn stdout_redirect_writes_to_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("shadowshell-test-{}.txt", std::process::id()));
        let line = format!("echo hello > {}", path.display());

        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(&pipeline(&line), &line);
        assert_eq!(code, 0);

        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "hello\n");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn background_job_is_tracked_then_reaped_as_done() {
        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(&pipeline("true &"), "true &");
        assert_eq!(code, 0);
        assert_eq!(shell.jobs.list().len(), 1);

        // Give the child a brief moment to exit before polling, since
        // notify_job_changes only reaps non-blockingly.
        std::thread::sleep(std::time::Duration::from_millis(100));
        shell.notify_job_changes();

        assert!(shell.jobs.list().is_empty());
    }

    #[test]
    fn resume_job_foreground_waits_and_removes_job() {
        let mut shell = Shell::for_test();
        shell.run_pipeline(&pipeline("sleep 0.2 &"), "sleep 0.2 &");
        assert_eq!(shell.jobs.list().len(), 1);

        let code = shell.resume_job(None, true).unwrap();
        assert_eq!(code, 0);
        assert!(shell.jobs.list().is_empty());
    }

    #[test]
    fn resume_job_with_unknown_id_is_error() {
        let mut shell = Shell::for_test();
        assert!(shell.resume_job(Some(999), true).is_err());
    }

#[test]
    fn missing_redirect_target_reports_error_without_spawning() {
        let dir = std::env::temp_dir();
        let missing = dir.join("shadowshell-test-does-not-exist-dir/out.txt");
        let line = format!("echo hi < {}", missing.display());

        let mut shell = Shell::for_test();
        let code = shell.run_pipeline(&pipeline(&line), &line);
        assert_eq!(code, 1);
    }

    #[test]
    fn exported_shell_var_reaches_child_process() {
        let mut shell = Shell::for_test();
        shell
            .env
            .export("SHADOWSHELL_TEST_EXPORT", Some("from-shell".into()));

        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "shadowshell-env-test-{}.txt",
            std::process::id()
        ));
let line = format!(
            "sh -c 'printf %s \"${{SHADOWSHELL_TEST_EXPORT}}\"' > {}",
            path.display()
        );

        let code = shell.run_pipeline(&pipeline(&line), &line);
        assert_eq!(code, 0);
        assert_eq!(shell.env.last_status(), 0);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "from-shell");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unexported_shell_var_does_not_reach_child_process() {
        let mut shell = Shell::for_test();
        // Ensure the name is not lingering as exported from the process env.
        shell.env.unset("SHADOWSHELL_TEST_LOCAL");
        shell.env.set("SHADOWSHELL_TEST_LOCAL", "secret");

        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "shadowshell-env-local-{}.txt",
            std::process::id()
        ));
let line = format!(
            "sh -c 'printf %s \"${{SHADOWSHELL_TEST_LOCAL}}\"' > {}",
            path.display()
        );

        let code = shell.run_pipeline(&pipeline(&line), &line);
        assert_eq!(code, 0);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "");
        let _ = std::fs::remove_file(&path);
    }
}
