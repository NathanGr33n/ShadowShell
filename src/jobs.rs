//! The job table: tracks pipelines that were backgrounded or stopped, so
//! `jobs`, `fg`, and `bg` can inspect and resume them. This module is
//! pure bookkeeping; it has no knowledge of how to actually signal or
//! wait on processes (see `job_control` for that).

use nix::unistd::Pid;

/// The current state of a tracked job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// The job's process group is running (in the background).
    Running,
    /// The job's process group is stopped (e.g. via Ctrl+Z).
    Stopped,
    /// The job finished, with the given exit code.
    Done(i32),
}

/// A single tracked job: a pipeline that is running in the background or
/// was stopped while in the foreground.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    pub pgid: Pid,
    /// PIDs of every process in the pipeline, in command order. The last
    /// entry is the one whose exit status represents the pipeline's own
    /// exit status, matching shell convention.
    pub pids: Vec<Pid>,
    pub command_line: String,
    pub status: JobStatus,
}

/// A table of tracked jobs, keyed by a shell-assigned, ever-increasing job
/// ID (matching common shell convention of not reusing IDs within a
/// session).
#[derive(Debug, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_id: u32,
}

impl JobTable {
    /// Creates an empty job table.
    pub fn new() -> Self {
        JobTable {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// Registers a new job and returns its assigned ID.
    pub fn add(&mut self, pgid: Pid, pids: Vec<Pid>, command_line: String) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            pgid,
            pids,
            command_line,
            status: JobStatus::Running,
        });
        id
    }

    /// Removes the job with the given process group ID, if tracked,
    /// without reporting it as done. Used when a previously-tracked job
    /// (backgrounded or stopped) is brought to the foreground and finishes
    /// there, so its result is reported directly by the caller instead of
    /// through the asynchronous "Done" notification path.
    pub fn remove_by_pgid(&mut self, pgid: Pid) {
        self.jobs.retain(|j| j.pgid != pgid);
    }

    /// Looks up a job by its shell-assigned ID.
    pub fn get(&self, id: u32) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Updates the status of the job with the given process group ID, if
    /// tracked. Returns the job's ID when a matching job was updated.
    pub fn set_status_by_pgid(&mut self, pgid: Pid, status: JobStatus) -> Option<u32> {
        let job = self.jobs.iter_mut().find(|j| j.pgid == pgid)?;
        job.status = status;
        Some(job.id)
    }

    /// Returns all tracked jobs, in ascending ID order.
    pub fn list(&self) -> &[Job] {
        &self.jobs
    }

    /// Returns the ID of the most recently added job that is still
    /// `Running` or `Stopped`, for use as the implicit target of `fg`/`bg`
    /// when no job ID is given (matching common shell "current job"
    /// convention, approximated here by most-recently-added).
    pub fn most_recent_active_id(&self) -> Option<u32> {
        self.jobs
            .iter()
            .rev()
            .find(|j| !matches!(j.status, JobStatus::Done(_)))
            .map(|j| j.id)
    }

    /// Removes and returns all jobs currently marked `Done`, so the shell
    /// can report their completion once and stop tracking them.
    pub fn take_done(&mut self) -> Vec<Job> {
        let (done, remaining): (Vec<Job>, Vec<Job>) = self
            .jobs
            .drain(..)
            .partition(|j| matches!(j.status, JobStatus::Done(_)));
        self.jobs = remaining;
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(raw: i32) -> Pid {
        Pid::from_raw(raw)
    }

    fn add(table: &mut JobTable, pgid: Pid, command_line: &str) -> u32 {
        table.add(pgid, vec![pgid], command_line.to_string())
    }

    #[test]
    fn ids_are_assigned_sequentially_starting_at_one() {
        let mut table = JobTable::new();
        assert_eq!(add(&mut table, pid(100), "sleep 1"), 1);
        assert_eq!(add(&mut table, pid(200), "sleep 2"), 2);
    }

    #[test]
    fn get_finds_job_by_id() {
        let mut table = JobTable::new();
        let id = add(&mut table, pid(100), "sleep 1");
        assert_eq!(table.get(id).unwrap().pgid, pid(100));
        assert!(table.get(id + 1).is_none());
    }

    #[test]
    fn set_status_by_pgid_updates_matching_job() {
        let mut table = JobTable::new();
        let id = add(&mut table, pid(100), "sleep 1");
        let updated = table.set_status_by_pgid(pid(100), JobStatus::Stopped);
        assert_eq!(updated, Some(id));
        assert_eq!(table.get(id).unwrap().status, JobStatus::Stopped);
    }

    #[test]
    fn set_status_by_pgid_ignores_unknown_pgid() {
        let mut table = JobTable::new();
        add(&mut table, pid(100), "sleep 1");
        assert_eq!(table.set_status_by_pgid(pid(999), JobStatus::Stopped), None);
    }

    #[test]
    fn most_recent_active_id_skips_done_jobs() {
        let mut table = JobTable::new();
        let first = add(&mut table, pid(100), "sleep 1");
        add(&mut table, pid(200), "sleep 2");
        table.set_status_by_pgid(pid(200), JobStatus::Done(0));
        assert_eq!(table.most_recent_active_id(), Some(first));
    }

    #[test]
    fn most_recent_active_id_is_none_when_all_done() {
        let mut table = JobTable::new();
        add(&mut table, pid(100), "sleep 1");
        table.set_status_by_pgid(pid(100), JobStatus::Done(0));
        assert_eq!(table.most_recent_active_id(), None);
    }

    #[test]
    fn take_done_removes_only_done_jobs() {
        let mut table = JobTable::new();
        let running = add(&mut table, pid(100), "sleep 1");
        add(&mut table, pid(200), "sleep 2");
        table.set_status_by_pgid(pid(200), JobStatus::Done(0));

        let done = table.take_done();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].pgid, pid(200));
        assert_eq!(table.list().len(), 1);
        assert_eq!(table.get(running).unwrap().pgid, pid(100));
    }

    #[test]
    fn remove_by_pgid_drops_job_without_reporting_it_done() {
        let mut table = JobTable::new();
        let kept = add(&mut table, pid(100), "sleep 1");
        add(&mut table, pid(200), "sleep 2");

        table.remove_by_pgid(pid(200));

        assert_eq!(table.list().len(), 1);
        assert_eq!(table.get(kept).unwrap().pgid, pid(100));
        assert!(table.take_done().is_empty());
    }
}
