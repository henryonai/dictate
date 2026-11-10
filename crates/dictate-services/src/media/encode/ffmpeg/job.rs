//! What ffmpeg started, counted: when the program has ended, are the programs that it started gone too?
//!
//! On Windows the answer matters because of the `.part` file: a program that is being ended still has it open for a moment, and
//! it can neither be removed nor renamed until it is closed. `process-wrap`'s job object does not say when the job is empty: its
//! `wait` returns after the first message of the job's completion port (a process that began, a process that ended), and it keeps
//! the handle of its job to itself, so the job can't be asked how many processes it has left. So the program is also put in a job
//! object of our own, which is made before the program can start anything (it is created suspended, and `process-wrap` resumes it
//! after this has run), and `wait` does not return until that job has no active process, or a few seconds have gone by.
//! The two jobs are nested, which Windows 8 and later allow: the one of `process-wrap` still kills what it must.

use std::time::{Duration, Instant};

/// Calls `done` until it says yes, `pause` apart, for at most `limit`. Whether it did. It is asked at least once.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn poll_until(limit: Duration, pause: Duration, mut done: impl FnMut() -> bool) -> bool {
    let give_up = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= give_up {
            return false;
        }
        std::thread::sleep(pause);
    }
}

#[cfg(windows)]
pub(super) use windows_job::{Counted, Job};

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, ExitStatus};
    use std::sync::Arc;
    use std::time::Duration;

    use process_wrap::std::{StdChildWrapper, StdCommandWrap, StdCommandWrapper};
    #[cfg(test)]
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    #[cfg(test)]
    use windows::Win32::System::JobObjects::JobObjectBasicProcessIdList;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    };
    #[cfg(test)]
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::poll_until;

    /// How long `wait` looks for the job to be empty after the program has ended, and how often.
    const EMPTY_LIMIT: Duration = Duration::from_millis(2_500);
    const EMPTY_PAUSE: Duration = Duration::from_millis(20);

    /// A job object that is only there to be asked how many processes are in it.
    #[derive(Debug)]
    pub(crate) struct Job(HANDLE);

    // SAFETY: a job handle is only ever passed by value to calls that Windows allows from any thread, and it is closed once, when
    // the last `Arc` is gone.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub(crate) fn new() -> io::Result<Arc<Job>> {
            // SAFETY: no attributes and no name are asked for.
            let handle = unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?;
            Ok(Arc::new(Job(handle)))
        }

        fn assign(&self, process: HANDLE) -> io::Result<()> {
            // SAFETY: both are handles that are open.
            unsafe { AssignProcessToJobObject(self.0, process) }.map_err(io::Error::other)
        }

        /// How many processes of the job have not ended.
        pub(crate) fn active_processes(&self) -> io::Result<u32> {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: `info` is the size that is told, and lives through the call.
            unsafe {
                QueryInformationJobObject(
                    Some(self.0),
                    JobObjectBasicAccountingInformation,
                    (&raw mut info).cast::<c_void>(),
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    None,
                )
            }
            .map_err(io::Error::other)?;
            Ok(info.ActiveProcesses)
        }
    }

    #[cfg(test)]
    impl Job {
        /// What is in the job, for a test that fails: how many processes are active, and the id and the program of each.
        pub(crate) fn describe(&self) -> String {
            let active = self
                .active_processes()
                .map_or_else(|e| format!("unknown ({e})"), |active| active.to_string());
            let members = self.process_ids().map_or_else(
                |e| format!("unknown ({e})"),
                |ids| {
                    ids.iter()
                        .map(|id| format!("{id} {}", image_name(*id)))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );
            format!("{active} active processes in the job: [{members}]")
        }

        fn process_ids(&self) -> io::Result<Vec<u32>> {
            #[repr(C)]
            struct IdList {
                assigned: u32,
                listed: u32,
                ids: [usize; 64],
            }
            let mut list = IdList {
                assigned: 0,
                listed: 0,
                ids: [0; 64],
            };
            // SAFETY: `list` is the size that is told, and lives through the call.
            unsafe {
                QueryInformationJobObject(
                    Some(self.0),
                    JobObjectBasicProcessIdList,
                    (&raw mut list).cast::<c_void>(),
                    std::mem::size_of::<IdList>() as u32,
                    None,
                )
            }
            .map_err(io::Error::other)?;
            let listed = (list.listed as usize).min(list.ids.len());
            Ok(list.ids[..listed].iter().map(|id| *id as u32).collect())
        }
    }

    /// The name of the program of a process, as far as it can be had: a process that has ended has none.
    #[cfg(test)]
    fn image_name(id: u32) -> String {
        // SAFETY: the handle is closed before this returns, and the buffer is the size that is told.
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id) else {
                return "(gone)".to_owned();
            };
            let mut buffer = [0u16; 520];
            let mut length = buffer.len() as u32;
            let named = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(process);
            match named {
                Ok(()) => {
                    let path = String::from_utf16_lossy(&buffer[..length as usize]);
                    path.rsplit('\\').next().unwrap_or("?").to_owned()
                }
                Err(_) => "(unnamed)".to_owned(),
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed only here. Closing it ends nothing: the job has no limit set.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// Puts the program in the job, before it has started anything, and makes `wait` wait for the job to be empty. It goes between
    /// `CreationFlags` and `JobObject` of `process-wrap`: that one starts the program suspended and resumes it when it has had its
    /// turn, which comes after this one's.
    #[derive(Debug)]
    pub(crate) struct Counted(pub(crate) Arc<Job>);

    impl StdCommandWrapper for Counted {
        fn wrap_child(
            &mut self,
            inner: Box<dyn StdChildWrapper>,
            _core: &StdCommandWrap,
        ) -> io::Result<Box<dyn StdChildWrapper>> {
            let process = HANDLE(inner.inner().as_raw_handle().cast());
            // A program that can't be put in the job is still run, and `wait` is as it was without it: the count is a help, and
            // not worth the recording.
            if self.0.assign(process).is_err() {
                return Ok(inner);
            }
            Ok(Box::new(CountedChild {
                inner,
                job: self.0.clone(),
            }))
        }
    }

    #[derive(Debug)]
    struct CountedChild {
        inner: Box<dyn StdChildWrapper>,
        job: Arc<Job>,
    }

    impl StdChildWrapper for CountedChild {
        fn inner(&self) -> &Child {
            self.inner.inner()
        }

        fn inner_mut(&mut self) -> &mut Child {
            self.inner.inner_mut()
        }

        fn into_inner(self: Box<Self>) -> Child {
            self.inner.into_inner()
        }

        /// Waits for the program, and then for everything it started to be gone: for a few seconds at most, so that a program that
        /// can't be ended does not hold the recording for ever.
        fn wait(&mut self) -> io::Result<ExitStatus> {
            let status = self.inner.wait()?;
            // A job that can't be asked is not waited for.
            poll_until(
                EMPTY_LIMIT,
                EMPTY_PAUSE,
                || !matches!(self.job.active_processes(), Ok(active) if active > 0),
            );
            Ok(status)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Command, Stdio};

        #[test]
        fn a_job_counts_the_processes_in_it_until_they_end() {
            let job = Job::new().unwrap();
            assert_eq!(job.active_processes().unwrap(), 0);
            // One program, which has nothing of its own to leave behind when it is killed.
            let mut child = Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            job.assign(HANDLE(child.as_raw_handle().cast())).unwrap();

            assert_eq!(job.active_processes().unwrap(), 1);
            assert!(
                job.describe().to_lowercase().contains("ping"),
                "{}",
                job.describe()
            );

            child.kill().unwrap();
            child.wait().unwrap();
            let emptied = poll_until(Duration::from_secs(3), Duration::from_millis(20), || {
                job.active_processes().unwrap() == 0
            });
            assert!(emptied, "{}", job.describe());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_ends_as_soon_as_the_condition_holds() {
        let mut asked = 0;

        let done = poll_until(Duration::from_secs(5), Duration::ZERO, || {
            asked += 1;
            asked == 4
        });

        assert!(done);
        assert_eq!(asked, 4);
    }

    #[test]
    fn a_condition_that_holds_at_once_is_not_waited_for() {
        let started = Instant::now();

        let done = poll_until(Duration::from_secs(5), Duration::from_secs(1), || true);

        assert!(done);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_condition_that_never_holds_is_given_up_at_the_limit() {
        let started = Instant::now();
        let mut asked = 0;

        let done = poll_until(Duration::from_millis(100), Duration::from_millis(5), || {
            asked += 1;
            false
        });

        assert!(!done);
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(asked >= 2, "it was asked once, and then again and again");
    }
}
