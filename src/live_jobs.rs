//! Live job dashboard state shared between the job table and the prompt.
//!
//! The interactive prompt reads a snapshot of active (running/stopped) jobs and
//! renders an ambient spinner/pulse. During `read_line`, an idle callback can
//! poll for completions without holding `&mut Shell` and push notifications
//! through reedline's external printer (which also triggers a prompt repaint,
//! advancing the spinner animation).

use std::sync::Mutex;
use std::time::Instant;

use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

use crate::jobs::{JobStatus, JobTable};

/// One active job as shown in the prompt dashboard.
#[derive(Debug, Clone)]
pub struct LiveJobEntry {
    pub id: u32,
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub command_line: String,
    pub status: LiveJobStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveJobStatus {
    Running,
    Stopped,
}

/// Immutable view used by the prompt renderer.
#[derive(Debug, Clone)]
pub struct JobDashboardView {
    /// When the dashboard was first non-empty this session of activity; used
    /// for spinner frame selection.
    pub pulse_origin: Instant,
    pub entries: Vec<LiveJobEntry>,
}

impl JobDashboardView {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn running_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == LiveJobStatus::Running)
            .count()
    }

    pub fn stopped_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == LiveJobStatus::Stopped)
            .count()
    }
}

/// Thread-safe dashboard updated from job-control paths and read by the prompt.
#[derive(Debug)]
pub struct LiveJobs {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    pulse_origin: Instant,
    entries: Vec<LiveJobEntry>,
    /// Finished jobs detected during idle poll, awaiting notification print.
    pending_notifications: Vec<FinishedJob>,
    /// Job IDs finished during idle poll; the job table should drop these
    /// without waiting again (they were already reaped).
    finished_ids: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct FinishedJob {
    pub id: u32,
    pub command_line: String,
    pub code: i32,
}

impl Default for LiveJobs {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveJobs {
    pub fn new() -> Self {
        LiveJobs {
            inner: Mutex::new(Inner {
                pulse_origin: Instant::now(),
                entries: Vec::new(),
                pending_notifications: Vec::new(),
                finished_ids: Vec::new(),
            }),
        }
    }

    /// Rebuilds the dashboard from the authoritative job table.
    pub fn sync_from_table(&self, table: &JobTable) {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let was_empty = guard.entries.is_empty();
        guard.entries = table
            .list()
            .iter()
            .filter_map(|j| {
                let status = match j.status {
                    JobStatus::Running => LiveJobStatus::Running,
                    JobStatus::Stopped => LiveJobStatus::Stopped,
                    JobStatus::Done(_) => return None,
                };
                Some(LiveJobEntry {
                    id: j.id,
                    pgid: j.pgid.as_raw(),
                    pids: j.pids.iter().map(|p| p.as_raw()).collect(),
                    command_line: j.command_line.clone(),
                    status,
                })
            })
            .collect();
        if was_empty && !guard.entries.is_empty() {
            guard.pulse_origin = Instant::now();
        }
    }

    /// Snapshot for prompt rendering.
    pub fn view(&self) -> JobDashboardView {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        JobDashboardView {
            pulse_origin: guard.pulse_origin,
            entries: guard.entries.clone(),
        }
    }

    /// Non-blocking poll of tracked process groups. Completed jobs are removed
    /// from the dashboard and queued as notifications (caller prints them).
    /// Returns `true` if the dashboard changed (prompt should repaint).
    pub fn poll_completions(&self) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let before = guard.entries.len();
        let mut still_active = Vec::with_capacity(before);
        let mut newly_finished = Vec::new();

        for entry in guard.entries.drain(..) {
            if entry.status == LiveJobStatus::Stopped {
                // Leave stopped jobs alone until fg/bg/notify path updates them.
                still_active.push(entry);
                continue;
            }

            match poll_group_done(entry.pgid, &entry.pids) {
                PollResult::StillRunning => still_active.push(entry),
                PollResult::Finished(code) => {
                    newly_finished.push(FinishedJob {
                        id: entry.id,
                        command_line: entry.command_line,
                        code,
                    });
                }
            }
        }

        let changed = still_active.len() != before;
        for finished in &newly_finished {
            guard.finished_ids.push(finished.id);
        }
        guard.pending_notifications.extend(newly_finished);
        guard.entries = still_active;
        changed
    }

    /// Drains finished-job notifications produced by [`Self::poll_completions`].
    pub fn take_notifications(&self) -> Vec<FinishedJob> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut guard.pending_notifications)
    }

    /// Job IDs reaped during idle polling that the [`crate::jobs::JobTable`]
    /// should drop without calling `wait` again.
    pub fn take_finished_ids(&self) -> Vec<u32> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut guard.finished_ids)
    }
}

enum PollResult {
    StillRunning,
    Finished(i32),
}

/// Checks whether every process in `pids` (group `pgid`) has exited, without
/// blocking. Partially-reaped groups stay "running" until all members are done.
fn poll_group_done(pgid: i32, pids: &[i32]) -> PollResult {
    if pids.is_empty() {
        return PollResult::Finished(0);
    }
    let last_pid = pids.last().copied();
    let mut remaining = pids.len();
    let mut finished_code = 0;
    let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED;

    loop {
        match waitpid(Pid::from_raw(-pgid), Some(flags)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                remaining = remaining.saturating_sub(1);
                if Some(pid.as_raw()) == last_pid {
                    finished_code = code;
                }
                if remaining == 0 {
                    return PollResult::Finished(finished_code);
                }
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                remaining = remaining.saturating_sub(1);
                if Some(pid.as_raw()) == last_pid {
                    finished_code = 128 + sig as i32;
                }
                if remaining == 0 {
                    return PollResult::Finished(finished_code);
                }
            }
            Ok(WaitStatus::StillAlive) => return PollResult::StillRunning,
            Ok(WaitStatus::Stopped(_, _)) => {
                // Treat as still tracked; status update happens via main notify path.
                return PollResult::StillRunning;
            }
            Ok(_) => return PollResult::StillRunning,
            Err(_) => {
                // No children / already fully reaped.
                return if remaining < pids.len() {
                    PollResult::Finished(finished_code)
                } else {
                    // Group may have been reaped elsewhere; drop from dashboard.
                    PollResult::Finished(0)
                };
            }
        }
    }
}

/// Truncates a command line for the compact prompt badge.
pub fn truncate_cmd(cmd: &str, max_chars: usize) -> String {
    let trimmed = cmd.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobTable;
    use nix::unistd::Pid;

    #[test]
    fn sync_filters_done_jobs() {
        let mut table = JobTable::new();
        let id = table.add(Pid::from_raw(1), vec![Pid::from_raw(1)], "sleep 1".into());
        table.set_status_by_pgid(Pid::from_raw(1), JobStatus::Running);
        let live = LiveJobs::new();
        live.sync_from_table(&table);
        assert_eq!(live.view().entries.len(), 1);
        assert_eq!(live.view().entries[0].id, id);

        table.set_status_by_pgid(Pid::from_raw(1), JobStatus::Done(0));
        live.sync_from_table(&table);
        assert!(live.view().is_empty());
    }

    #[test]
    fn truncate_cmd_short_unchanged() {
        assert_eq!(truncate_cmd("ls -la", 20), "ls -la");
    }

    #[test]
    fn truncate_cmd_long_gets_ellipsis() {
        let s = truncate_cmd("cargo test --all-features -- --nocapture", 12);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 12);
    }

    #[test]
    fn view_counts_running_and_stopped() {
        let live = LiveJobs::new();
        {
            let mut g = live.inner.lock().unwrap();
            g.entries.push(LiveJobEntry {
                id: 1,
                pgid: 10,
                pids: vec![10],
                command_line: "sleep 1".into(),
                status: LiveJobStatus::Running,
            });
            g.entries.push(LiveJobEntry {
                id: 2,
                pgid: 11,
                pids: vec![11],
                command_line: "vim".into(),
                status: LiveJobStatus::Stopped,
            });
        }
        let v = live.view();
        assert_eq!(v.running_count(), 1);
        assert_eq!(v.stopped_count(), 1);
    }
}
