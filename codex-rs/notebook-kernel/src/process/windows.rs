//! A contained child whose leader is reaped even while the notebook is idle.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;

use codex_utils_pty::JobObject;
use tokio::process::Child;
use tokio::sync::watch;
use tokio::task::JoinHandle;

type Completion = Result<ExitStatus, Arc<io::Error>>;

struct Leader {
    child: Child,
    job: Arc<JobObject>,
}

impl Drop for Leader {
    fn drop(&mut self) {
        // Runtime shutdown can cancel the supervisor even if OwnedChild remains alive.
        let _ = self.job.terminate();
    }
}

pub(crate) struct OwnedChild {
    job: Arc<JobObject>,
    completion: watch::Receiver<Option<Completion>>,
    supervisor: Option<JoinHandle<()>>,
}

impl OwnedChild {
    // Only accepts a child already assigned and resumed by JobObject::spawn_contained.
    pub(super) fn new(child: Child, job: JobObject) -> Self {
        let job = Arc::new(job);
        let mut leader = Leader {
            child,
            job: Arc::clone(&job),
        };
        let (completed, completion) = watch::channel(None);
        let supervisor = tokio::spawn(async move {
            let status = leader.child.wait().await;
            // The leader exiting does not close the job. Kill its descendants immediately,
            // including when no API operation is currently polling the kernel.
            let terminated = leader.job.terminate();
            let result = status.and_then(|status| terminated.map(|()| status));
            let _ = completed.send_replace(Some(result.map_err(Arc::new)));
        });
        Self {
            job,
            completion,
            supervisor: Some(supervisor),
        }
    }

    pub(super) fn start_kill(&mut self) -> io::Result<()> {
        self.job.terminate()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.completion.borrow().as_ref() {
            Some(Ok(status)) => Ok(Some(*status)),
            Some(Err(error)) => Err(io::Error::new(error.kind(), error.to_string())),
            None => Ok(None),
        }
    }

    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                // Join the owned supervisor as well as reaping the leader.
                if let Some(supervisor) = self.supervisor.as_mut() {
                    supervisor.await.map_err(io::Error::other)?;
                    self.supervisor = None;
                }
                return Ok(status);
            }
            self.completion.changed().await.map_err(io::Error::other)?;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.job.terminate();
        // Aborting drops Child with kill_on_drop, retaining Tokio's orphan reaper.
        // The last job handle then closes with KILL_ON_JOB_CLOSE still enabled.
        if let Some(supervisor) = self.supervisor.as_ref() {
            supervisor.abort();
        }
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
