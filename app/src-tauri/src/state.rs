//! What the commands share: the settings file, the key store, which keys exist, the model folder and the jobs
//! that can be cancelled.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{LoadWarning, SettingsStore};
use dictate_services::whisper::local::LocalWhisper;
use dictate_services::whisper::models::ModelDir;
use tokio_util::sync::CancellationToken;

use crate::config::saved_key;

/// A job that runs for a while and can be cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Job {
    /// The download of the Whisper model with this id.
    Download(String),
    /// A local transcription.
    Transcribe,
    /// A dictation with this number, from the shortcut to the text in the field. Each has its own, so that a new one
    /// can begin while the one before it is told to stop.
    Dictation(u64),
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

/// A model that is loaded once and used by every dictation and transcription after it: loading takes seconds and
/// the model takes a good part of a gigabyte. It is loaded again when another one is asked for, and let go of when
/// nobody has used it for a while (`unload_idle`).
pub struct ModelCache<T> {
    slot: Mutex<Option<Loaded<T>>>,
    /// What time it is: `Instant::now`, except in the tests, which move it by hand.
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
}

/// The model in memory.
struct Loaded<T> {
    id: String,
    model: Arc<T>,
    /// When it was last asked for, or last seen in use.
    last_used: Instant,
}

impl<T> Default for ModelCache<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
            clock: Box::new(Instant::now),
        }
    }
}

impl<T> ModelCache<T> {
    #[cfg(test)]
    fn with_clock(clock: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self {
            slot: Mutex::new(None),
            clock: Box::new(clock),
        }
    }

    /// The model `id`, loaded with `load` when it isn't the one in memory. The lock is held while it loads, so that two
    /// that ask for the same model at once load it once. A failed load is not remembered: the next one tries again.
    /// Asking for a model counts as using it.
    pub fn get(
        &self,
        id: &str,
        load: impl FnOnce() -> Result<T, String>,
    ) -> Result<Arc<T>, String> {
        let mut slot = lock(&self.slot);
        if let Some(loaded) = slot.as_mut().filter(|loaded| loaded.id == id) {
            loaded.last_used = (self.clock)();
            return Ok(Arc::clone(&loaded.model));
        }
        // The old one goes first: two big models are not in memory at once.
        *slot = None;
        let model = Arc::new(load()?);
        *slot = Some(Loaded {
            id: id.to_owned(),
            model: Arc::clone(&model),
            last_used: (self.clock)(),
        });
        Ok(model)
    }

    /// Lets the model go from memory when nothing has used it for longer than `idle`. The app asks every minute.
    ///
    /// A model that something still holds is in use however long ago it was asked for (a transcription of hours
    /// asks once and holds it): it stays, and counts as used now, so that it is not let go the moment the job ends
    /// and the next one has to load it again.
    // Nothing calls it until the app's timer does.
    #[allow(dead_code)]
    pub fn unload_idle(&self, idle: Duration) {
        let mut slot = lock(&self.slot);
        let now = (self.clock)();
        let Some(loaded) = slot.as_mut() else {
            return;
        };
        if Arc::strong_count(&loaded.model) > 1 {
            loaded.last_used = now;
        } else if now.saturating_duration_since(loaded.last_used) > idle {
            *slot = None;
        }
    }

    /// Lets the model `id` go from memory, because its file is deleted. Another one stays.
    pub fn forget(&self, id: &str) {
        let mut slot = lock(&self.slot);
        if slot.as_ref().is_some_and(|loaded| loaded.id == id) {
            *slot = None;
        }
    }
}

/// The Whisper model of the dictation, in memory.
pub type WhisperCache = ModelCache<LocalWhisper>;

/// Which keys are saved, as the settings view and the dictation need to know it. Asking the OS key store for
/// that on every call can make the system show a prompt each time, so it is read once (see
/// `AppState::loaded_keys`) and kept up to date when a key is saved or removed.
#[derive(Debug, Default)]
pub struct KeyFlags {
    /// The key of the AI provider.
    pub ai: AtomicBool,
    /// The key of the transcription service itself; the AI key it may borrow doesn't count.
    pub transcribe: AtomicBool,
}

impl KeyFlags {
    fn flag(&self, account: Account) -> &AtomicBool {
        match account {
            Account::Ai => &self.ai,
            Account::Transcribe => &self.transcribe,
        }
    }

    /// Whether a key is saved for the account.
    pub fn get(&self, account: Account) -> bool {
        self.flag(account).load(Ordering::SeqCst)
    }

    pub fn set(&self, account: Account, saved: bool) {
        self.flag(account).store(saved, Ordering::SeqCst);
    }
}

/// Everything the commands need, cheap to clone into a background task.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<SettingsStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub keys: Arc<KeyFlags>,
    keys_read: Arc<OnceLock<()>>,
    pub models: ModelDir,
    pub whisper: Arc<WhisperCache>,
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
            keys: Arc::default(),
            keys_read: Arc::default(),
            models,
            whisper: Arc::default(),
            jobs: Arc::new(Jobs::default()),
            load_warning: Arc::new(Mutex::new(load_warning.map(|warning| warning.message))),
        }
    }

    /// Why the settings file wasn't used as it was, once: the first call gets it, later ones nothing.
    pub fn take_load_warning(&self) -> Option<String> {
        lock(&self.load_warning).take()
    }

    /// The key flags, after the key store was asked once which keys it holds. The first caller asks, which may
    /// take a while (a locked Keychain waits for the user), so call this from a blocking thread; anyone who comes
    /// meanwhile waits for the answer instead of asking again, and later calls return at once. A key store that
    /// can't be read counts as holding no key.
    pub fn loaded_keys(&self) -> &KeyFlags {
        self.keys_read.get_or_init(|| {
            for account in [Account::Ai, Account::Transcribe] {
                self.keys
                    .set(account, saved_key(&*self.secrets, account).is_some());
            }
        });
        &self.keys
    }
}

/// The folder the models are kept below: the local data directory of the OS, and not the roaming one that
/// `dirs::data_dir()` is on Windows (`AppData\Roaming`): a profile that roams must not carry gigabytes of
/// models along. On macOS and Linux both are the same folder. `None` when the system has none.
pub fn models_base_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
}

/// The folder of the Whisper models below `base_dir` (see `models_base_dir`), created when it is missing.
///
/// It is `whisper-models` and not `models`: on macOS `Dictate/models` is the same folder as the Swift app's
/// `Dictate/Models`, and that one holds the Swift app's files.
pub fn open_model_dir(base_dir: &Path) -> io::Result<ModelDir> {
    let root = base_dir.join("Dictate").join("whisper-models");
    fs::create_dir_all(&root)?;
    Ok(ModelDir::new(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CountingStore, NoKeyring};
    use dictate_services::secrets::MemoryStore;
    use std::sync::atomic::AtomicUsize;
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

    /// The state with the given key store, in a folder of its own.
    fn state_with(dir: &tempfile::TempDir, secrets: Arc<dyn SecretStore>) -> AppState {
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        AppState::new(
            store,
            warning,
            secrets,
            ModelDir::new(dir.path().join("models")),
        )
    }

    #[test]
    fn the_key_flags_tell_which_keys_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = MemoryStore::default();
        secrets.set(Account::Transcribe, "sk-stt").unwrap();
        let state = state_with(&dir, Arc::new(secrets));

        let keys = state.loaded_keys();

        assert!(!keys.get(Account::Ai));
        assert!(keys.get(Account::Transcribe));
    }

    #[test]
    fn an_empty_key_or_a_key_store_that_is_down_counts_as_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = MemoryStore::default();
        secrets.set(Account::Ai, "").unwrap();
        let state = state_with(&dir, Arc::new(secrets));
        assert!(!state.loaded_keys().get(Account::Ai));

        let state = state_with(&dir, Arc::new(NoKeyring));
        let keys = state.loaded_keys();
        assert!(!keys.get(Account::Ai) && !keys.get(Account::Transcribe));
    }

    #[test]
    fn the_key_store_is_read_once_and_every_caller_waits_for_it() {
        let dir = tempfile::tempdir().unwrap();
        // A store that takes its time, like a Keychain waiting for the user to answer a prompt.
        let secrets = Arc::new(CountingStore::slow(Duration::from_millis(40)));
        secrets.set(Account::Ai, "sk-ai").unwrap();
        let state = state_with(&dir, secrets.clone());

        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    // Whoever asks first reads; the others get the flags only when it is done.
                    let keys = state.loaded_keys();
                    assert!(keys.get(Account::Ai), "asked before the keys were read");
                    assert!(!keys.get(Account::Transcribe));
                });
            }
        });
        state.loaded_keys();

        assert_eq!(secrets.gets(), 2, "one read per key, in all");
    }

    #[test]
    fn a_flag_can_be_changed_after_the_load() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with(&dir, Arc::new(MemoryStore::default()));
        let keys = state.loaded_keys();

        keys.set(Account::Ai, true);
        assert!(keys.get(Account::Ai) && !keys.get(Account::Transcribe));
        keys.set(Account::Ai, false);
        assert!(!keys.get(Account::Ai));
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
    fn the_models_live_in_the_local_data_dir_not_the_roaming_one() {
        // On Windows `AppData\Local` and `AppData\Roaming` differ, and a profile that roams must not carry
        // gigabytes of models along. Elsewhere there is only one data dir.
        assert_eq!(models_base_dir(), dirs::data_local_dir());
        if cfg!(windows) {
            assert_ne!(models_base_dir(), dirs::data_dir());
        } else {
            assert_eq!(models_base_dir(), dirs::data_dir());
        }
    }

    #[test]
    fn the_model_folder_is_dictate_whisper_models_below_the_base_dir_and_is_created() {
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

    // ---- the model in memory

    #[test]
    fn a_model_is_loaded_once_for_every_dictation() {
        let cache = ModelCache::<String>::default();
        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok("tiny model".to_string())
        };

        let first = cache.get("tiny", load).unwrap();
        let second = cache.get("tiny", load).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn another_model_replaces_the_one_in_memory() {
        let cache = ModelCache::<String>::default();
        let tiny = cache.get("tiny", || Ok("tiny".to_string())).unwrap();
        let base = cache.get("base", || Ok("base".to_string())).unwrap();
        assert_eq!((tiny.as_str(), base.as_str()), ("tiny", "base"));

        // The settings went back to the first one: it is loaded again, because only one is kept.
        let loads = AtomicUsize::new(0);
        cache
            .get("tiny", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok("tiny".to_string())
            })
            .unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_model_that_fails_to_load_is_asked_for_again_next_time() {
        let cache = ModelCache::<String>::default();
        assert_eq!(
            cache.get("tiny", || Err("damaged".into())).unwrap_err(),
            "damaged"
        );
        // The file was downloaded again meanwhile.
        assert_eq!(
            cache.get("tiny", || Ok("fine".into())).unwrap().as_str(),
            "fine"
        );
    }

    #[test]
    fn a_deleted_model_is_let_go_and_another_one_stays() {
        let cache = ModelCache::<String>::default();
        let kept = cache.get("tiny", || Ok("tiny".to_string())).unwrap();

        cache.forget("base");
        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok("tiny".to_string())
        };
        assert!(Arc::ptr_eq(&cache.get("tiny", load).unwrap(), &kept));
        assert_eq!(
            loads.load(Ordering::SeqCst),
            0,
            "another model's deletion unloaded this one"
        );

        cache.forget("tiny");
        drop(kept);
        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    /// A clock that stands still until the test moves it.
    fn hand_clock(start: Instant) -> (Arc<Mutex<Instant>>, impl Fn() -> Instant + Send + Sync) {
        let now = Arc::new(Mutex::new(start));
        let reader = Arc::clone(&now);
        (now, move || *lock(&reader))
    }

    #[test]
    fn model_cache_unloads_after_idle() {
        let start = Instant::now();
        let (now, clock) = hand_clock(start);
        let cache = ModelCache::<String>::with_clock(clock);
        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok("tiny".to_string())
        };
        let idle = Duration::from_secs(300);
        let at = |seconds: u64| *lock(&now) = start + Duration::from_secs(seconds);

        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        // Used 299 s ago: it stays, and asking for it is a use.
        at(299);
        cache.unload_idle(idle);
        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "unloaded after 299 s");

        // 299 s since that use, though 598 s since it was loaded: the last use is what counts.
        at(598);
        cache.unload_idle(idle);
        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "the use did not count");

        // Exactly as long as `idle` is not longer than it.
        at(898);
        cache.unload_idle(idle);
        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "unloaded after 300 s");

        // 301 s without a use: it goes, and the next one loads it again.
        at(898 + 301);
        cache.unload_idle(idle);
        cache.get("tiny", load).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2, "still there after 301 s");
    }

    #[test]
    fn model_cache_unload_idle_with_nothing_loaded_does_nothing() {
        let cache = ModelCache::<String>::default();

        cache.unload_idle(Duration::ZERO);

        assert_eq!(
            cache.get("tiny", || Ok("tiny".into())).unwrap().as_str(),
            "tiny"
        );
    }

    #[test]
    fn model_cache_keeps_a_model_that_is_in_use() {
        let start = Instant::now();
        let (now, clock) = hand_clock(start);
        let cache = ModelCache::<String>::with_clock(clock);
        let idle = Duration::from_secs(300);
        let at = |seconds: u64| *lock(&now) = start + Duration::from_secs(seconds);
        // A transcription of hours takes the model once and holds it.
        let running = cache.get("tiny", || Ok("tiny".to_string())).unwrap();

        at(3_600);
        cache.unload_idle(idle);
        let again = cache
            .get("tiny", || Err("loaded a second copy".into()))
            .unwrap();
        assert!(Arc::ptr_eq(&running, &again), "two models in memory");
        drop(again);

        // It ends: the model has been used until just now, so it stays for the idle time from here.
        drop(running);
        at(3_600 + 200);
        cache.unload_idle(idle);
        let loads = AtomicUsize::new(0);
        cache
            .get("tiny", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok("tiny".to_string())
            })
            .unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            0,
            "unloaded straight after a long job"
        );

        at(3_600 + 200 + 301);
        cache.unload_idle(idle);
        cache
            .get("tiny", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok("tiny".to_string())
            })
            .unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dictations_have_jobs_of_their_own() {
        let jobs = Arc::new(Jobs::default());
        let first = jobs.start(Job::Dictation(1)).unwrap();
        // The next dictation begins while the first is being told to stop.
        let second = jobs.start(Job::Dictation(2)).unwrap();
        assert!(jobs.cancel(&Job::Dictation(1)));
        assert!(first.token().is_cancelled());
        assert!(!second.token().is_cancelled());
        // Quitting stops them all.
        assert!(!jobs.cancel_all_and_wait(Duration::from_millis(10)));
        assert!(second.token().is_cancelled());
    }
}
