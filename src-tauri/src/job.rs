//! Child processes (recognition worker, installers) are placed in a Windows job object
//! that kills them when the job handle closes. The app holds that handle for its whole
//! life, so the children end even if the app crashes or is killed: a stray worker would
//! otherwise keep about 1 GB of RAM and the GPU model loaded.
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct Job(HANDLE);

// SAFETY: a job handle may be used from any thread; it is only read after creation.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// A new job that terminates its processes when the last handle to it closes.
    pub fn kill_on_close() -> Option<Self> {
        // SAFETY: plain Win32 calls; the handle is owned by the returned value.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let applied = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if applied == 0 {
                CloseHandle(handle);
                return None;
            }
            Some(Self(handle))
        }
    }

    /// Put a running process into the job. Best effort: false if Windows refuses.
    fn contain(&self, process: HANDLE) -> bool {
        // SAFETY: both handles are valid for the duration of the call.
        unsafe { AssignProcessToJobObject(self.0, process) != 0 }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The app-wide job, created on first use and kept until the process ends.
fn app_job() -> Option<&'static Job> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(Job::kill_on_close).as_ref()
}

/// Tie a child process's lifetime to the app. Best effort: a refusal leaves the
/// existing explicit shutdown and kill-on-drop handling in place.
pub fn contain_child(child: &tokio::process::Child) -> bool {
    match (app_job(), child.raw_handle()) {
        (Some(job), Some(handle)) => job.contain(handle as HANDLE),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn closing_the_job_ends_its_processes() {
        let job = Job::kill_on_close().expect("job object");
        // A harmless long-running child: ping waits about a second per echo.
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start ping");
        assert!(job.contain(child.as_raw_handle() as HANDLE));
        assert!(
            child.try_wait().unwrap().is_none(),
            "child runs inside the job"
        );
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(5);
        let ended = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if ended.is_none() {
            let _ = child.kill();
        }
        assert!(ended.is_some(), "closing the job terminated the child");
    }
}
