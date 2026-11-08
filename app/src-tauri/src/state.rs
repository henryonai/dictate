//! What the commands share: the settings file, the key store, the model folder and the jobs that can be
//! cancelled.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use dictate_services::secrets::SecretStore;
use dictate_services::settings::{LoadWarning, SettingsStore};
use dictate_services::whisper::models::ModelDir;
use tokio_util::sync::CancellationToken;

/// A job that runs for a while and can be cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Job {
    /// The download of the Whisper model with this id.
    Download(String),
    /// A local transcription.
    Transcribe,
}

/// The job is running already.
#[derive(Debug, PartialEq, Eq)]
pub struct AlreadyRunning;

/// The jobs that are running, at most one of each kind.
#[derive(Default)]
pub struct Jobs {
    active: Mutex<HashMap<Job, CancellationToken>>,
    finished: Condvar,
}

/// Proof that a job is running. Dropping it, whatever the outcome, frees the job to be started again.
pub struct JobGuard {
    jobs: Arc<Jobs>,
    job: Job,
    token: CancellationToken,
}

impl Jobs {
    /// Registers a job; `AlreadyRunning` when the same one is running.
    pub fn start(self: &Arc<Self>, job: Job) -> Result<JobGuard, AlreadyRunning> {
        let mut active = lock(&self.active);
        if active.contains_key(&job) {
            return Err(AlreadyRunning);
        }
        let token = CancellationToken::new();
        active.insert(job.clone(), token.clone());
        Ok(JobGuard {
            jobs: Arc::clone(self),
            job,
            token,
        })
    }

    /// Whether the job is running.
    pub fn is_active(&self, job: &Job) -> bool {
        lock(&self.active).contains_key(job)
    }

    /// Asks a running job to stop. Whether there was one.
    pub fn cancel(&self, job: &Job) -> bool {
        match lock(&self.active).get(job) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Asks every running job to stop and waits up to `timeout` for them to finish (a download flushes what it
    /// has so that it can resume). Whether they all did.
    pub fn cancel_all_and_wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut active = lock(&self.active);
        for token in active.values() {
            token.cancel();
        }
        while !active.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            active = self
                .finished
                .wait_timeout(active, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// A mutex's content even when a thread panicked while holding it: nothing here is left half-done.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl JobGuard {
    /// Cancelled when someone asks the job to stop.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        lock(&self.jobs.active).remove(&self.job);
        self.jobs.finished.notify_all();
    }
}

/// Everything the commands need, cheap to clone into a background task.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<SettingsStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub models: ModelDir,
    pub jobs: Arc<Jobs>,
    load_warning: Arc<Mutex<Option<String>>>,
}

impl AppState {
    pub fn new(
        settings: SettingsStore,
        load_warning: Option<LoadWarning>,
        secrets: Arc<dyn SecretStore>,
        models: ModelDir,
    ) -> AppState {
        AppState {
            settings: Arc::new(settings),
            secrets,
            models,
            jobs: Arc::new(Jobs::default()),
            load_warning: Arc::new(Mutex::new(load_warning.map(|warning| warning.message))),
        }
    }

    /// Why the settings file wasn't used as it was, once: the first call gets it, later ones nothing.
    pub fn take_load_warning(&self) -> Option<String> {
        lock(&self.load_warning).take()
    }
}

/// The folder of the Whisper models below the OS data directory, created when it is missing.
///
/// It is `whisper-models` and not `models`: on macOS `Dictate/models` is the same folder as the Swift app's
/// `Dictate/Models`, and that one holds the Swift app's files.
pub fn open_model_dir(data_dir: &Path) -> io::Result<ModelDir> {
    let root = data_dir.join("Dictate").join("whisper-models");
    fs::create_dir_all(&root)?;
    Ok(ModelDir::new(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictate_services::secrets::MemoryStore;
    use std::thread;

    fn download(id: &str) -> Job {
        Job::Download(id.to_string())
    }

    #[test]
    fn a_second_job_with_the_same_key_is_refused() {
        let jobs = Arc::new(Jobs::default());
        let _running = jobs.start(download("tiny")).unwrap();
        assert_eq!(
            jobs.start(download("tiny")).err(),
            Some(AlreadyRunning),
            "one download per model"
        );
    }

    #[test]
    fn different_jobs_run_side_by_side() {
        let jobs = Arc::new(Jobs::default());
        let _tiny = jobs.start(download("tiny")).unwrap();
        let _base = jobs.start(download("base")).unwrap();
        let _transcription = jobs.start(Job::Transcribe).unwrap();
    }

    #[test]
    fn dropping_the_guard_frees_the_job() {
        let jobs = Arc::new(Jobs::default());
        drop(jobs.start(download("tiny")).unwrap());
        assert!(jobs.start(download("tiny")).is_ok());
    }

    #[test]
    fn is_active_tells_whether_a_job_runs() {
        let jobs = Arc::new(Jobs::default());
        assert!(!jobs.is_active(&download("tiny")));
        let guard = jobs.start(download("tiny")).unwrap();
        assert!(jobs.is_active(&download("tiny")));
        assert!(!jobs.is_active(&download("base")));
        drop(guard);
        assert!(!jobs.is_active(&download("tiny")));
    }

    #[test]
    fn cancel_reaches_the_running_job_only() {
        let jobs = Arc::new(Jobs::default());
        assert!(!jobs.cancel(&download("tiny")), "nothing to cancel");

        let tiny = jobs.start(download("tiny")).unwrap();
        let base = jobs.start(download("base")).unwrap();
        assert!(jobs.cancel(&download("tiny")));
        assert!(tiny.token().is_cancelled());
        assert!(!base.token().is_cancelled());
    }

    #[test]
    fn cancel_all_waits_until_the_jobs_have_finished() {
        let jobs = Arc::new(Jobs::default());
        let guard = jobs.start(download("tiny")).unwrap();
        let token = guard.token();
        // A download that notices the cancel, takes a moment to flush its file and ends.
        let worker = thread::spawn(move || {
            while !token.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            thread::sleep(Duration::from_millis(50));
            drop(guard);
        });
        assert!(jobs.cancel_all_and_wait(Duration::from_secs(5)));
        // It returned because the job was over, not before.
        assert!(jobs.start(download("tiny")).is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn cancel_all_gives_up_after_the_timeout() {
        let jobs = Arc::new(Jobs::default());
        let stuck = jobs.start(download("tiny")).unwrap();
        assert!(!jobs.cancel_all_and_wait(Duration::from_millis(30)));
        assert!(stuck.token().is_cancelled());
    }

    #[test]
    fn cancel_all_with_nothing_running_returns_at_once() {
        let jobs = Arc::new(Jobs::default());
        assert!(jobs.cancel_all_and_wait(Duration::from_secs(5)));
    }

    #[test]
    fn the_load_warning_is_handed_out_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (store, warning) = SettingsStore::open(path);
        assert!(warning.is_some(), "the file is broken on purpose");

        let state = AppState::new(
            store,
            warning.clone(),
            Arc::new(MemoryStore::default()),
            ModelDir::new(dir.path().join("models")),
        );
        assert_eq!(state.take_load_warning(), warning.map(|w| w.message));
        assert_eq!(state.take_load_warning(), None);
    }

    #[test]
    fn no_warning_stays_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        let state = AppState::new(
            store,
            warning,
            Arc::new(MemoryStore::default()),
            ModelDir::new(dir.path().join("models")),
        );
        assert_eq!(state.take_load_warning(), None);
    }

    #[test]
    fn the_model_folder_is_dictate_whisper_models_below_the_data_dir_and_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let models = open_model_dir(dir.path()).unwrap();
        // Not `Dictate/models`: macOS compares names without regard to case, so that would be the folder
        // `Dictate/Models` of the Swift app, which keeps its own files there.
        assert_eq!(
            models.root(),
            dir.path().join("Dictate").join("whisper-models")
        );
        assert!(models.root().is_dir());
        // A second start finds it there.
        assert!(open_model_dir(dir.path()).is_ok());
    }
}
