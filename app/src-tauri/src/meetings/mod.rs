//! Meetings in the app: the real backend of the pipeline, the registry of the jobs that run it, the commands and views of
//! the meetings window, and the local server that plays a recording's media to it.

pub mod backend;
pub mod commands;
pub mod jobs;
pub mod library;
pub mod media_server;
pub mod notify;
#[cfg(test)]
mod test_fixture;
pub mod views;
pub mod window;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dictate_core::meetings::library::MeetingLibrary;
use tauri::{AppHandle, Manager};
use tokio::time::MissedTickBehavior;

use crate::platform;
use crate::state::AppState;
use backend::{AppBackend, DiarizerCache};
use commands::TextClipboard;
use jobs::{AppEvents, Events, Registry};
use library::{sweep_library, Meetings, SystemTrash};
use media_server::MediaServer;
use notify::AppNotifier;

/// How long a model may sit in memory with nobody using it (Whisper's and the speaker separation's), and how often the
/// app looks.
const IDLE_MODEL: Duration = Duration::from_secs(300);
const IDLE_CHECK: Duration = Duration::from_secs(60);

/// Puts the meetings in the app: the registry of jobs on the real backend, the commands' state, the local media server, and
/// the clearing away of what a job that was cut short left. The jobs wait for that to be done.
pub fn start(app: &AppHandle, state: &AppState, diarize_dir: PathBuf) {
    let events: Arc<dyn Events> = Arc::new(AppEvents(app.clone()));
    let notifier = Arc::new(AppNotifier {
        app: app.clone(),
        settings: state.settings.clone(),
    });
    let diarizer = Arc::new(DiarizerCache::default());
    let backend = Arc::new(AppBackend::new(state, diarizer.clone(), diarize_dir));
    let registry = Registry::new(
        state.jobs.clone(),
        backend,
        events.clone(),
        notifier,
        jobs::system_clock(),
    );
    // Playing is a convenience: with no server the window shows no player, and everything else works.
    let media = match MediaServer::start() {
        Ok(server) => Some(Arc::new(server)),
        Err(e) => {
            log::warn!("The media server did not start: {e}");
            None
        }
    };
    let library = MeetingLibrary::new(MeetingLibrary::default_directory());
    let meetings = Meetings::new(library, registry, media, events, Arc::new(SystemTrash));
    app.manage(meetings.clone());
    app.manage(TextClipboard(platform::current_clipboard()));

    // Off the thread of the interface: it reads every recording's folder, and the key store may wait for the person.
    let (directory, registry, state_for_keys) = (
        meetings.library.directory.clone(),
        meetings.registry.clone(),
        state.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        start_up(&directory, &state_for_keys, || registry.open());
    });

    let (whisper, diarizer) = (state.whisper.clone(), diarizer);
    every(IDLE_CHECK, move || {
        whisper.unload_idle(IDLE_MODEL);
        diarizer.unload_idle(IDLE_MODEL);
    });
}

/// What has to be done before the first job: what a job that was cut short left in the folders is cleared away (a job that
/// is running has such files too, which is why it is done before any), and the key flags are read (the backend goes by them
/// and never asks the key store), and then `open` lets the jobs go.
fn start_up(directory: &Path, state: &AppState, open: impl FnOnce()) {
    let swept = sweep_library(directory);
    if swept > 0 {
        log::info!("Cleared {swept} unfinished files out of the meetings library");
    }
    state.loaded_keys();
    open();
}

/// Runs `work` on a blocking thread every `period`, for as long as the app runs.
fn every(period: Duration, work: impl Fn() + Send + Sync + 'static) {
    let work = Arc::new(work);
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(period);
        // A tick that came while the work was still going does not make two runs follow each other at once.
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick is now: there is nothing to unload yet.
        tick.tick().await;
        loop {
            tick.tick().await;
            let work = work.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || work()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use dictate_services::secrets::Account;
    use dictate_services::settings::SettingsStore;
    use dictate_services::whisper::models::ModelDir;

    use super::*;
    use crate::state::ModelCache;
    use test_fixture::{Fixture, SCREEN};

    #[test]
    fn start_up_clears_the_folders_and_reads_the_keys_before_the_jobs_are_let_go() {
        let library = Fixture::new();
        let stale = library.folder(SCREEN).join("transcript.txt.1-0.tmp");
        fs::write(&stale, b"half").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let secrets = Arc::new(crate::test_support::CountingStore::default());
        let state = AppState::new(
            store,
            None,
            secrets.clone(),
            ModelDir::new(dir.path().join("models")),
        );
        let opened = AtomicUsize::new(0);

        start_up(library.dir(), &state, || {
            // At the moment the jobs are let go the folders are clean and the flags are read.
            assert!(!stale.exists(), "a job would have met the leftovers");
            assert_eq!(secrets.gets(), 2, "both keys were asked for");
            assert!(!state.keys.get(Account::Ai));
            opened.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert!(library.folder(SCREEN).join("transcript.txt").is_file());
    }

    #[test]
    fn the_idle_check_runs_again_and_again() {
        let (tx, rx) = mpsc::channel();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        every(Duration::from_millis(5), move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = tx.send(n);
        });

        // Three runs, on three ticks, in order.
        for expected in 1..=3 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(30)), Ok(expected));
        }
    }

    #[test]
    fn a_model_that_nobody_has_used_is_let_go_by_the_idle_check() {
        let cache = Arc::new(ModelCache::<String>::default());
        cache.get("tiny", || Ok("tiny".to_owned())).unwrap();
        let (tx, rx) = mpsc::channel();
        let held = cache.clone();
        every(Duration::from_millis(5), move || {
            held.unload_idle(Duration::ZERO);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(30)).unwrap();

        let loads = AtomicUsize::new(0);
        cache
            .get("tiny", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok("tiny".to_owned())
            })
            .unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1, "it was still in memory");
    }
}
