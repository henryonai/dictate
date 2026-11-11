//! The recordings as the meetings window works on them: listing, importing, starting jobs again, editing, exporting and
//! deleting. The commands are thin wrappers over this; what they can't do without a window (the dialogs, the opener) is
//! theirs, and what a test can reach is here.
//!
//! Everything here may block (it reads and writes the library's folders): call it on a blocking thread.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use dictate_core::meetings::library::{
    files, Kind, MeetingLibrary, MeetingRecord, Meta, SCREEN_SOURCE,
};
use dictate_core::meetings::{
    append_log, atomic_write, export_file_name, export_markdown, SummaryDoc,
};
use dictate_services::media::ytdlp::link_is_valid;
use dictate_services::meetings::{sweep_stale_files, JobSpec, MeetingError, StartAt};
use dictate_services::record::marker::MARKER;
use dictate_services::record::session::Finished;
use dictate_services::summary;

use super::backend::ai_ready;
use super::jobs::{Events, Registry};
use super::media_server::MediaServer;
use super::views::{self, ErrorView, MeetingDetail, MeetingRow};
use crate::commands::api_error_text;
use crate::config::summary_ai_config;
use crate::state::AppState;

/// How long deleting a recording waits for its job to stop.
const STOP_LIMIT: Duration = Duration::from_secs(30);

const NOT_FOUND: &str = "The recording was not found.";

/// The extensions of the audio and video that a recording is made of: what the file dialog offers first, and the only
/// files that "Open video/audio in another app" opens. `meta.json` is data, and a library that came from elsewhere may
/// name anything as its media: opening a script or a program with the system's opener would run it.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts", "m2ts",
    "mts", "ogv", "asf", "m4a", "mp3", "wav", "aac", "flac", "ogg", "oga", "opus", "wma", "aiff",
    "aif", "caf", "amr", "mka", "mp2", "ac3",
];

/// Whether the file is named like audio or video.
fn is_media_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        MEDIA_EXTENSIONS.contains(&extension.as_str())
    })
}

/// Where a deleted recording goes: the Trash (the Recycle Bin), so that it can be got back.
pub trait Trash: Send + Sync {
    fn delete(&self, folder: &Path) -> Result<(), String>;
}

/// The Trash of the system.
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn delete(&self, folder: &Path) -> Result<(), String> {
        trash::delete(folder).map_err(|e| e.to_string())
    }
}

#[derive(Clone)]
pub struct Meetings {
    pub library: MeetingLibrary,
    pub registry: Registry,
    media: Option<Arc<MediaServer>>,
    events: Arc<dyn Events>,
    trash: Arc<dyn Trash>,
}

impl Meetings {
    pub fn new(
        library: MeetingLibrary,
        registry: Registry,
        media: Option<Arc<MediaServer>>,
        events: Arc<dyn Events>,
        trash: Arc<dyn Trash>,
    ) -> Meetings {
        Meetings {
            library,
            registry,
            media,
            events,
            trash,
        }
    }

    /// The folder of the recording `id`, when `id` is the name of one folder of the library and nothing else: an id comes
    /// from the window, and a path in it (`..`, a separator) must never reach a folder outside the library.
    pub fn folder_of(&self, id: &str) -> Option<PathBuf> {
        let mut parts = Path::new(id).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(name)), None) if name == OsStr::new(id) => {
                Some(self.library.directory.join(id))
            }
            _ => None,
        }
    }

    fn record(&self, id: &str) -> Result<MeetingRecord, String> {
        self.folder_of(id)
            .and_then(|folder| MeetingRecord::from_folder(&folder))
            .ok_or_else(|| NOT_FOUND.to_owned())
    }

    /// What a job of the recording is given: its folder, and the link to download when it came from one.
    fn spec_for(record: &MeetingRecord) -> JobSpec {
        JobSpec {
            id: record.id.clone(),
            folder: record.folder.clone(),
            link: (record.kind() == Kind::Link).then(|| record.source.clone()),
            notes: Vec::new(),
        }
    }

    /// The recordings that match `query`, with the jobs that work on them.
    pub fn list(&self, query: &str) -> Vec<MeetingRow> {
        views::rows(self.library.meetings(), query, &|record| {
            views::state_of(self.registry.status(&record.id), record)
        })
    }

    pub fn detail(&self, id: &str) -> Result<MeetingDetail, String> {
        let record = self.record(id)?;
        let (live, error) = views::state_of(self.registry.status(id), &record);
        let url = |path: &Path| {
            self.media
                .as_ref()
                .map_or_else(String::new, |media| media.url_for(path))
        };
        let mut detail = views::detail(&record, live, error, &url);
        if self.media.is_none() {
            detail.media = None;
        }
        Ok(detail)
    }

    /// Adds each file as a recording, without copying or moving it, and starts its job. The ids of the recordings made.
    /// A folder is not media and is passed over.
    pub fn import_files(&self, paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .filter_map(|path| {
                let path = Path::new(path);
                if path.is_dir() {
                    return None;
                }
                let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
                let text = absolute.to_string_lossy().into_owned();
                let title = absolute
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| text.clone());
                match self.create(&title, &text, &text, None, StartAt::Audio) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        log::warn!("Could not import {}: {e}", absolute.display());
                        None
                    }
                }
            })
            .collect()
    }

    /// Adds a link as a recording and starts its job (the download first).
    pub fn import_link(&self, url: &str) -> Result<String, ErrorView> {
        let url = url.trim();
        if !link_is_valid(url) {
            return Err(MeetingError::invalid_link().into());
        }
        self.create(url, url, "", Some(url.to_owned()), StartAt::Download)
            .map_err(|e| ErrorView::other(format!("Couldn't create the recording's folder: {e}")))
    }

    /// A new recording: its folder and `meta.json`, and its job.
    fn create(
        &self,
        title: &str,
        source: &str,
        media: &str,
        link: Option<String>,
        from: StartAt,
    ) -> io::Result<String> {
        let now = Local::now();
        let (id, folder) = self.library.create_folder(now)?;
        let meta = Meta {
            title: title.to_owned(),
            source: source.to_owned(),
            media: media.to_owned(),
            created: now.timestamp_millis() as f64 / 1000.0,
            duration: None,
        };
        if let Err(e) = meta.write(&folder) {
            let _ = fs::remove_dir(&folder);
            return Err(e);
        }
        let spec = JobSpec {
            id: id.clone(),
            folder,
            link,
            notes: Vec::new(),
        };
        // A folder that was just made has no job.
        self.registry
            .start(spec, from)
            .map_err(|_| io::Error::other("the new recording already has a job"))?;
        Ok(id)
    }

    /// The folder of a recording that is about to be made: the folder and its `meta.json`, with the source that the screen
    /// recordings of the Swift app have (for every mode, the sound alone included), the title, and `media_name` in the folder
    /// as the media, by its absolute path, as the library has it. The length is not known yet. No job is started: that is
    /// `finish_recording`, when the recording is over. The id and the folder.
    pub fn begin_recording(&self, title: &str, media_name: &str) -> io::Result<(String, PathBuf)> {
        let now = Local::now();
        let (id, folder) = self.library.create_folder(now)?;
        let meta = Meta {
            title: title.to_owned(),
            source: SCREEN_SOURCE.to_owned(),
            media: absolute(&folder.join(media_name)),
            created: now.timestamp_millis() as f64 / 1000.0,
            duration: None,
        };
        if let Err(e) = meta.write(&folder) {
            let _ = fs::remove_dir(&folder);
            return Err(e);
        }
        Ok((id, folder))
    }

    /// The recording `id` in `folder` is over: its `meta.json` gets the file that was made as the media and how long it is, what
    /// went wrong on the way is written in its log, and its job starts, from the audio. A `meta.json` that is not there (a
    /// recording that was begun and then lost it) is made again, named by the folder.
    pub fn finish_recording(&self, id: &str, folder: &Path, finished: &Finished) -> io::Result<()> {
        let mut meta = Meta::read(folder).unwrap_or_else(|| Meta {
            title: id.to_owned(),
            source: SCREEN_SOURCE.to_owned(),
            created: Local::now().timestamp_millis() as f64 / 1000.0,
            ..Meta::default()
        });
        meta.media = absolute(&finished.media);
        if finished.duration.is_finite() && finished.duration > 0.0 {
            meta.duration = Some(finished.duration);
        }
        meta.write(folder)?;
        // The job writes them again when it starts the log afresh; they are written here too, for the recording whose job does not
        // get to start (the app ends first).
        for note in &finished.notes {
            // A line that is not written is a line of a log less: the recording is not lost for it.
            if let Err(e) = append_log(folder, note) {
                log::warn!("Could not write a note of the recording {id}: {e}");
            }
        }
        let spec = JobSpec {
            id: id.to_owned(),
            folder: folder.to_path_buf(),
            link: None,
            notes: finished.notes.clone(),
        };
        self.registry
            .start(spec, StartAt::Audio)
            .map_err(|_| io::Error::other("the recording already has a job"))
    }

    /// Tells the window that the library changed: a recording was begun, or is gone.
    pub fn library_changed(&self) {
        self.events.changed();
    }

    /// Whether `folder` is a recording's folder of this library (a folder directly inside it).
    fn is_a_folder_of_the_library(&self, folder: &Path) -> bool {
        folder.parent() == Some(self.library.directory.as_path()) && folder.is_dir()
    }

    /// Removes the folder of a recording that did not begin. Only a folder of the library is: this is called with what
    /// `begin_recording` made, and must not be able to remove anything else.
    pub fn discard_folder(&self, folder: &Path) {
        if !self.is_a_folder_of_the_library(folder) {
            log::warn!("Not removing {}: it is not a recording", folder.display());
            return;
        }
        if let Err(e) = fs::remove_dir_all(folder) {
            log::warn!("Could not remove {}: {e}", folder.display());
        }
    }

    /// Clears away the folder of a recording that was put together and had nothing in it (`finalize` says so), and never more
    /// than that. A folder that holds only `meta.json` and the log has nothing that anyone could want: those two files are
    /// removed, and then the folder, which `remove_dir` takes only when it is empty. One that holds anything else (a stem that
    /// could not be read, a video that could not be played: what a person may still get something out of) is moved to the
    /// Trash, where it can be got back, and stays where it is when the Trash will not take it: nothing is removed by force.
    pub fn discard_nothing_recorded(&self, folder: &Path) {
        if !self.is_a_folder_of_the_library(folder) {
            log::warn!("Not removing {}: it is not a recording", folder.display());
            return;
        }
        let holds_more = fs::read_dir(folder).map_or(true, |mut entries| {
            entries.any(|entry| {
                entry.map_or(true, |entry| {
                    let name = entry.file_name();
                    name != OsStr::new(files::META) && name != OsStr::new(files::LOG)
                })
            })
        });
        if holds_more {
            if let Err(why) = self.trash.delete(folder) {
                log::warn!(
                    "The Trash did not take {}: {why}; it is kept",
                    folder.display()
                );
            }
            return;
        }
        for name in [files::META, files::LOG] {
            if let Err(e) = fs::remove_file(folder.join(name)) {
                if e.kind() != io::ErrorKind::NotFound {
                    log::warn!("Could not remove {name} of {}: {e}", folder.display());
                }
            }
        }
        if let Err(e) = fs::remove_dir(folder) {
            log::warn!("Could not remove {}: {e}", folder.display());
        }
    }

    /// A recording that is thrown away: its folder goes to the Trash like a deleted recording, or is removed when the Trash
    /// will not take it. The window is told that the library changed. An error is the reason that the folder is still there.
    pub fn trash_recording(&self, folder: &Path) -> Result<(), String> {
        if !self.is_a_folder_of_the_library(folder) {
            return Err("That is not a recording of the library.".to_owned());
        }
        let moved = self.trash.delete(folder).or_else(|why| {
            log::warn!("The Trash did not take {}: {why}", folder.display());
            fs::remove_dir_all(folder).map_err(|e| e.to_string())
        });
        self.events.changed();
        moved
    }

    /// Starts the job of the recording from where `from` says. A recording that has a job says so, with a key: a second click
    /// on "Try again" can reach it.
    fn restart(
        &self,
        id: &str,
        from: impl FnOnce(&MeetingRecord) -> StartAt,
    ) -> Result<(), ErrorView> {
        let record = self.record(id).map_err(ErrorView::other)?;
        self.registry
            .start(Self::spec_for(&record), from(&record))
            .map_err(|_| ErrorView::busy())
    }

    /// "Transcribe + summarize again from scratch".
    pub fn rerun(&self, id: &str) -> Result<(), ErrorView> {
        self.restart(id, |_| StartAt::Audio)
    }

    /// "Summarize again". A recording with no transcript starts from the audio: the pipeline sees to that.
    pub fn resummarize(&self, id: &str) -> Result<(), ErrorView> {
        self.restart(id, |_| StartAt::Summary)
    }

    /// "Try again": from the step that failed.
    pub fn retry(&self, id: &str) -> Result<(), ErrorView> {
        self.restart(id, retry_start)
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.registry.cancel(id)
    }

    /// Ticks or unticks an action item of the summary.
    pub fn set_task(&self, id: &str, index: usize, done: bool) -> Result<(), String> {
        let record = self.record(id)?;
        let summary = record
            .summary()
            .ok_or("The recording has no summary.".to_owned())?;
        if index >= SummaryDoc::parse(&summary).action_items().len() {
            return Err("There is no such action item.".to_owned());
        }
        let mut ticked = record.done_tasks();
        if done {
            ticked.insert(index);
        } else {
            ticked.remove(&index);
        }
        record
            .set_done_tasks(&ticked)
            .map_err(|e| format!("Couldn't save the ticked action items: {e}"))?;
        self.events.changed();
        Ok(())
    }

    /// Gives speakers new names in the transcript and the summary, and the new names are written in the log. Refused while
    /// the recording has a job: it is writing the same files. The recording's place is held for the writing, so that no job can
    /// start in the middle of it, and given back before `resummarize` starts one: the summary is written again from the
    /// transcript with its new names.
    pub fn rename_speakers(
        &self,
        id: &str,
        names: &HashMap<String, String>,
        resummarize: bool,
    ) -> Result<Vec<(String, String)>, ErrorView> {
        let record = self.record(id).map_err(ErrorView::other)?;
        let applied = self
            .while_held(id, || record.rename_speakers(names))?
            .map_err(|e| ErrorView::other(e.to_string()))?;
        if applied.is_empty() {
            return Ok(applied);
        }
        self.events.changed();
        if resummarize {
            if let Err(e) = self.resummarize(id) {
                log::warn!(
                    "Could not summarize {id} again after renaming speakers: {}",
                    e.detail.as_deref().unwrap_or(&e.key)
                );
            }
        }
        Ok(applied)
    }

    /// Does `work`, which writes the files of the recording, with the recording's place held: no job can start in the middle of
    /// it (it would read what is about to change, and write over what is changed). A recording that has a job, or is held,
    /// says so, with a key, and `work` is not done. The place is given back as soon as `work` is over.
    fn while_held<T>(&self, id: &str, work: impl FnOnce() -> T) -> Result<T, ErrorView> {
        let _held = self.registry.hold(id).map_err(|_| ErrorView::busy())?;
        Ok(work())
    }

    /// The Markdown of an export: the summary, then the transcript.
    pub fn markdown(&self, id: &str) -> Result<String, String> {
        let record = self.record(id)?;
        let transcript = record.transcript().map(|(text, _)| text);
        Ok(export_markdown(
            &record.title,
            record.summary().as_deref(),
            transcript.as_deref(),
        ))
    }

    /// The name an export is offered under.
    pub fn export_name(&self, id: &str) -> Result<String, String> {
        Ok(export_file_name(&self.record(id)?.title))
    }

    /// Writes the export to `destination`, whole or not at all.
    pub fn export_to(&self, id: &str, destination: &Path) -> Result<(), ErrorView> {
        let markdown = self.markdown(id).map_err(ErrorView::other)?;
        atomic_write(destination, markdown.as_bytes()).map_err(|e| {
            let name = destination.file_name().unwrap_or(destination.as_os_str());
            ErrorView::other(format!("Couldn't save {}: {e}", name.to_string_lossy()))
        })
    }

    /// `log.txt` as it is; nothing yet is empty.
    pub fn log(&self, id: &str) -> Result<String, String> {
        let record = self.record(id)?;
        match fs::read(record.folder.join(files::LOG)) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("Couldn't read the log: {e}")),
        }
    }

    /// The media file of the recording, when it is there and is audio or video. The original of an imported file may have
    /// been moved since the window last looked, so this is an error a person can reach.
    pub fn media_path(&self, id: &str) -> Result<PathBuf, ErrorView> {
        self.record(id)
            .map_err(ErrorView::other)?
            .media()
            .filter(|path| path.is_file() && is_media_file(path))
            .ok_or_else(|| MeetingError::no_media().into())
    }

    /// The folder of the recording, for the file manager.
    pub fn folder_path(&self, id: &str) -> Result<PathBuf, String> {
        self.record(id).map(|record| record.folder)
    }

    /// Stops the recording's job, waits for it to end, and moves the recording's folder to the Trash.
    pub async fn delete(&self, id: &str) -> Result<(), String> {
        let folder = self
            .folder_of(id)
            .filter(|folder| folder.is_dir())
            .ok_or_else(|| NOT_FOUND.to_owned())?;
        // The job is waited for: it writes into the folder, and a folder that is in the Trash is no place for it.
        self.registry.cancel(id);
        if !self.registry.wait_idle(id, STOP_LIMIT).await {
            return Err(
                "The recording is still being processed. Try again in a moment.".to_owned(),
            );
        }
        let trash = self.trash.clone();
        tauri::async_runtime::spawn_blocking(move || trash.delete(&folder))
            .await
            .map_err(|e| format!("The work stopped unexpectedly: {e}"))?
            .map_err(|e| format!("Couldn't move the recording to the Trash: {e}"))?;
        self.registry.forget(id);
        self.events.changed();
        Ok(())
    }

    /// The answer of the AI model to a question about the recording's transcript.
    pub async fn ask(
        &self,
        state: &AppState,
        id: &str,
        question: &str,
    ) -> Result<String, ErrorView> {
        let question = question.trim();
        if question.is_empty() {
            return Err(ErrorView::empty_question());
        }
        // The recording's files are read, and the key store asked (only when the flags say that a key is there), on a blocking
        // thread.
        let (meetings, state, id) = (self.clone(), state.clone(), id.to_owned());
        let (config, title, transcript) = tauri::async_runtime::spawn_blocking(move || {
            let record = meetings.record(&id).map_err(ErrorView::other)?;
            let Some((transcript, _)) = record.transcript() else {
                return Err(ErrorView::no_transcript());
            };
            if !ai_ready(&state.settings.get(), state.loaded_keys()) {
                return Err(ErrorView::no_ai());
            }
            let config = summary_ai_config(&state.settings, &*state.secrets).map_err(|e| {
                // The settings say a key is saved and the store does not give it: as good as none.
                log::warn!("The AI is not set up for a question: {e}");
                ErrorView::no_ai()
            })?;
            Ok((config, record.title, transcript))
        })
        .await
        .map_err(|e| ErrorView::other(format!("The work stopped unexpectedly: {e}")))??;
        summary::ask(&config, &title, &transcript, question)
            .await
            .map_err(|e| ErrorView::other(api_error_text(&e)))
    }
}

/// Where "Try again" starts: a link that has no media yet (its download failed, or was cut short) downloads again; a recording
/// that has a transcript is only summarized again; any other starts from the audio.
pub fn retry_start(record: &MeetingRecord) -> StartAt {
    let media_is_there = record.media().is_some_and(|path| path.is_file());
    if record.kind() == Kind::Link && !media_is_there {
        StartAt::Download
    } else if record
        .read(files::TRANSCRIPT)
        .is_some_and(|transcript| !transcript.is_empty())
    {
        StartAt::Summary
    } else {
        StartAt::Audio
    }
}

/// `path` as an absolute one (the library may be given as a relative one).
fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Clears out of every recording's folder what a job that was cut short left (the temporary files of writes, the unfinished
/// downloads, the slices of audio the API was sent). For the start of the app, before any job runs. How many files went.
pub fn sweep_library(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        // A recording that was cut off and is not finished yet has its video in a `.part` file: that is for `recover`.
        .filter(|entry| !entry.path().join(MARKER).is_file())
        .map(|entry| sweep_stale_files(&entry.path()))
        .sum()
}

/// What the window may ask to be shown of the settings.
pub fn is_settings_tab(tab: &str) -> bool {
    matches!(tab, "general" | "speech" | "ai" | "accounts")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};

    use dictate_core::meetings::library::{files, Meta};
    use dictate_services::meetings::fakes::{gate, FakeBackend, GateControl};
    use dictate_services::settings::SettingsStore;
    use dictate_services::whisper::models::ModelDir;
    use serde_json::json;

    use super::*;
    use crate::meetings::jobs::Clock;
    use crate::meetings::notify::{Notice, Notifier};
    use crate::meetings::test_fixture::{Fixture, FILE, LINK, SCREEN, SUMMARY, TRANSCRIPT};
    use crate::meetings::views::{JobView, KindView};
    use crate::state::{lock, Job, Jobs};
    use crate::test_support::CountingStore;

    const PATIENCE: Duration = Duration::from_secs(30);

    /// What the window would hear, counted and handed to the test.
    struct Heard {
        changed: AtomicUsize,
        ended: Mutex<mpsc::Sender<JobView>>,
    }

    impl Events for Heard {
        fn job(&self, view: &JobView) {
            if view.outcome.is_some() {
                let _ = lock(&self.ended).send(view.clone());
            }
        }
        fn changed(&self) {
            self.changed.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Notifier for Heard {
        fn notify(&self, _: Notice) {}
    }

    /// A Trash that takes a folder away as the real one does, and notes whether the recording still had a job then.
    struct FakeTrash {
        jobs: Arc<Jobs>,
        moved: Mutex<Vec<(PathBuf, bool)>>,
        fail: bool,
    }

    impl Trash for FakeTrash {
        fn delete(&self, folder: &Path) -> Result<(), String> {
            let id = folder.file_name().unwrap().to_string_lossy().into_owned();
            let running = self.jobs.is_active(&Job::Meeting(id));
            lock(&self.moved).push((folder.to_path_buf(), running));
            if self.fail {
                return Err("The Trash is full.".into());
            }
            fs::remove_dir_all(folder).map_err(|e| e.to_string())
        }
    }

    struct Rig {
        meetings: Meetings,
        backend: Arc<FakeBackend>,
        trash: Arc<FakeTrash>,
        heard: Arc<Heard>,
        ended: mpsc::Receiver<JobView>,
        /// The ends of jobs that `ended` met on its way to another one.
        skipped: Mutex<Vec<JobView>>,
        library: Fixture,
    }

    impl Rig {
        fn with(backend: FakeBackend, library: Fixture, media: Option<Arc<MediaServer>>) -> Rig {
            let (tx, ended) = mpsc::channel();
            let heard = Arc::new(Heard {
                changed: AtomicUsize::new(0),
                ended: Mutex::new(tx),
            });
            let backend = Arc::new(backend);
            let jobs = Arc::new(Jobs::default());
            let clock: Clock = Box::new(|| 1_000);
            let registry = Registry::new(
                jobs.clone(),
                backend.clone(),
                heard.clone(),
                heard.clone(),
                clock,
            );
            registry.open();
            let trash = Arc::new(FakeTrash {
                jobs,
                moved: Mutex::new(Vec::new()),
                fail: false,
            });
            let meetings = Meetings::new(
                library.library(),
                registry,
                media,
                heard.clone(),
                trash.clone(),
            );
            Rig {
                meetings,
                backend,
                trash,
                heard,
                ended,
                skipped: Mutex::new(Vec::new()),
                library,
            }
        }

        fn new() -> Rig {
            Rig::with(FakeBackend::new(), Fixture::new(), None)
        }

        fn empty(backend: FakeBackend) -> Rig {
            Rig::with(backend, Fixture::empty(), None)
        }

        /// Waits for the job of `id` to end. The ends of other recordings' jobs that come first are kept for the test to ask
        /// for, not thrown away: two jobs that run at once end in any order.
        fn ended(&self, id: &str) -> JobView {
            loop {
                {
                    let mut kept = lock(&self.skipped);
                    if let Some(at) = kept.iter().position(|view| view.id == id) {
                        return kept.remove(at);
                    }
                }
                let view = self.ended.recv_timeout(PATIENCE).expect("the job to end");
                lock(&self.skipped).push(view);
            }
        }

        fn changed(&self) -> usize {
            self.heard.changed.load(Ordering::SeqCst)
        }

        fn meta(&self, id: &str) -> Meta {
            Meta::read(&self.library.folder(id)).expect("meta.json")
        }

        /// Gives the recording a media file that is there, as an import of a file would have: without one its job ends at the audio
        /// before the backend is asked for anything.
        fn give_media(&self, id: &str) -> PathBuf {
            let media = media_file(&self.library.outside(), &format!("{id}.m4a"));
            let mut meta = self.meta(id);
            meta.media = media.to_string_lossy().into_owned();
            meta.write(&self.library.folder(id)).unwrap();
            media
        }

        /// The names of the files in a recording's folder, sorted.
        fn files(&self, id: &str) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.library.folder(id))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    /// A media file in a folder with a Vietnamese name, away from the library.
    fn media_file(dir: &Path, name: &str) -> PathBuf {
        let folder = dir.join("Phim ảnh");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        fs::write(&path, b"media").unwrap();
        path
    }

    fn held_at_audio() -> (FakeBackend, GateControl) {
        let (held, control) = gate();
        let mut backend = FakeBackend::new();
        backend.gates.prepare = Some(held);
        (backend, control)
    }

    #[test]
    fn the_end_of_a_job_that_came_first_is_kept_for_whoever_asks_for_it_later() {
        // Two jobs that run at once end in any order; a test that waits for one must not lose the other's end on the way.
        let rig = Rig::empty(FakeBackend::new());
        let view = |id: &str| JobView {
            id: id.into(),
            step: None,
            progress: None,
            step_started_ms: 0,
            outcome: Some(crate::meetings::views::OutcomeView::Done { summary: true }),
        };
        {
            let ends = lock(&rig.heard.ended);
            ends.send(view("second")).unwrap();
            ends.send(view("first")).unwrap();
        }

        assert_eq!(rig.ended("first").id, "first");
        assert_eq!(rig.ended("second").id, "second");
    }

    // ---- the names of recordings

    #[test]
    fn an_id_is_one_folder_of_the_library_and_nothing_else() {
        let rig = Rig::new();
        let library = rig.library.dir();

        assert_eq!(rig.meetings.folder_of(SCREEN), Some(library.join(SCREEN)));
        assert_eq!(
            rig.meetings.folder_of("2026-10-19 22-20-58 (2)"),
            Some(library.join("2026-10-19 22-20-58 (2)"))
        );
        for bad in [
            "",
            ".",
            "..",
            "../outside",
            "a/b",
            "/etc",
            "SCREEN/",
            "./a",
            "a/..",
        ] {
            assert_eq!(rig.meetings.folder_of(bad), None, "{bad:?}");
        }
        #[cfg(windows)]
        for bad in [r"C:\Windows", r"..\x", r"a\b", "C:x"] {
            assert_eq!(rig.meetings.folder_of(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nothing_outside_the_library_is_read_or_deleted_by_a_crafted_id() {
        let rig = Rig::new();
        // The folder beside the library: `..` from a recording's id reaches it.
        let outside = rig.library.outside();
        fs::write(outside.join("transcript.txt"), "secret").unwrap();
        let id = format!("../{}", outside.file_name().unwrap().to_string_lossy());

        assert!(rig.meetings.detail(&id).is_err());
        assert!(rig.meetings.log(&id).is_err());
        assert!(rig.meetings.markdown(&id).is_err());
        assert!(rig.meetings.rerun(&id).is_err());
        let deleted = tauri::async_runtime::block_on(rig.meetings.delete(&id));
        assert!(deleted.is_err());
        assert!(
            lock(&rig.trash.moved).is_empty(),
            "the Trash was asked for a folder outside the library"
        );
        assert!(outside.join("transcript.txt").exists());
    }

    // ---- the list and the detail

    #[test]
    fn the_list_shows_every_recording_and_what_its_job_is_doing() {
        let (backend, control) = held_at_audio();
        let rig = Rig::with(backend, Fixture::new(), None);
        rig.give_media(LINK);

        let rows = rig.meetings.list("");
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            [SCREEN, LINK, FILE]
        );
        assert!(rows
            .iter()
            .all(|row| row.live.is_none() && row.error.is_none()));

        // The link has a job that is running: the row says so, and has no error.
        rig.meetings.rerun(LINK).unwrap();
        control.started.blocking_recv().unwrap();
        let rows = rig.meetings.list("");
        let link = rows.iter().find(|row| row.id == LINK).unwrap();
        assert_eq!(
            link.live.as_ref().map(|job| job.outcome.is_none()),
            Some(true)
        );
        assert_eq!(link.error, None);
        assert_eq!(link.kind, KindView::Link);

        // A search finds it by its summary and by its transcript, with no diacritics.
        let found = |query: &str| -> Vec<String> {
            rig.meetings
                .list(query)
                .into_iter()
                .map(|row| row.id)
                .collect()
        };
        assert_eq!(found("quyet dinh"), [SCREEN]);
        assert_eq!(found("hop"), [SCREEN, FILE]);

        control.release.send(()).unwrap();
        rig.ended(LINK);
    }

    #[test]
    fn a_folder_that_was_never_processed_is_an_error_after_a_restart_and_a_cancel_is_cancelled() {
        let rig = Rig::new();
        rig.library
            .write("2026-11-09 10-00-00", Some(json!({"title": "New"})), &[]);

        let row = |id: &str| {
            rig.meetings
                .list("")
                .into_iter()
                .find(|row| row.id == id)
                .unwrap()
        };
        let error = row("2026-11-09 10-00-00")
            .error
            .expect("not finished processing");
        assert_eq!(error.key, "meetings.error.unfinished");
        assert_eq!(row(SCREEN).error, None);
    }

    #[test]
    fn the_detail_of_a_recording_is_its_row_and_what_is_in_its_folder() {
        let media = Arc::new(MediaServer::start().unwrap());
        let rig = Rig::with(FakeBackend::new(), Fixture::new(), Some(media));
        let audio = rig.library.folder(SCREEN).join(files::AUDIO);
        fs::write(&audio, b"audio").unwrap();

        let detail = rig.meetings.detail(SCREEN).unwrap();
        assert_eq!(detail.row.id, SCREEN);
        assert_eq!(detail.transcript.len(), 3);
        assert_eq!(detail.summary.unwrap().markdown, SUMMARY);
        let url = detail.media.expect("audio.m4a plays").url;
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert!(rig.meetings.detail("no such recording").is_err());
    }

    #[test]
    fn without_a_media_server_there_is_nothing_to_play() {
        let rig = Rig::new();
        fs::write(rig.library.folder(SCREEN).join(files::AUDIO), b"audio").unwrap();
        assert_eq!(rig.meetings.detail(SCREEN).unwrap().media, None);
    }

    // ---- importing

    #[test]
    fn import_link_rejects_non_http() {
        let rig = Rig::empty(FakeBackend::new());
        for url in [
            "ftp://example.com/x",
            "-x",
            "--exec=rm",
            "",
            "   ",
            "javascript:alert(1)",
            "youtu.be/x",
            "https://",
            "https://a b",
        ] {
            let error = rig.meetings.import_link(url).unwrap_err();
            assert_eq!(error.key, "meetings.error.invalid_link", "{url:?}");
        }
        // Nothing was made, and nothing runs.
        assert_eq!(fs::read_dir(rig.library.dir()).unwrap().count(), 0);
        assert!(rig.backend.calls().is_empty());
        assert_eq!(rig.changed(), 0);
    }

    #[test]
    fn a_link_becomes_a_recording_named_by_it_and_its_job_downloads_first() {
        let rig = Rig::empty(FakeBackend::new());

        let id = rig
            .meetings
            .import_link("  https://www.youtube.com/watch?v=x \n")
            .unwrap();
        rig.ended(&id);

        let meta = rig.meta(&id);
        assert_eq!(meta.title, "https://www.youtube.com/watch?v=x");
        assert_eq!(meta.source, meta.title);
        assert!(meta.created > 1_700_000_000.0, "{}", meta.created);
        assert_eq!(
            rig.backend.calls().first().map(String::as_str),
            Some("download")
        );
        // The id is the folder's name, the time it was made.
        assert!(rig.library.folder(&id).is_dir());
        assert_eq!(id.len(), "2026-11-09 10-00-00".len());
    }

    #[test]
    fn import_files_creates_meta_with_original_path_and_no_copy() {
        // No AI: the job ends with the transcript, and the recording keeps the name its file had.
        let mut backend = FakeBackend::new();
        backend.ai_ready = false;
        let rig = Rig::empty(backend);
        let original = media_file(&rig.library.outside(), "cuộc họp tuần 1.MP4");
        let again = media_file(&rig.library.outside(), "âm thanh.m4a");

        let ids = rig.meetings.import_files(&[
            original.to_string_lossy().into_owned(),
            again.to_string_lossy().into_owned(),
        ]);
        assert_eq!(ids.len(), 2, "one recording for each file");
        for id in &ids {
            rig.ended(id);
        }

        // The first: named by the file without its extension, which is where it is played from.
        let meta = rig.meta(&ids[0]);
        assert_eq!(meta.title, "cuộc họp tuần 1");
        assert_eq!(PathBuf::from(&meta.media), original);
        assert_eq!(PathBuf::from(&meta.source), original);
        assert!(meta.created > 1_700_000_000.0);
        // Only what the pipeline wrote is in the folder: the media is not there.
        assert_eq!(
            rig.files(&ids[0]),
            ["log.txt", "meta.json", "transcript.txt"]
        );
        // The original is where it was, untouched.
        assert_eq!(fs::read(&original).unwrap(), b"media");
        assert_eq!(rig.backend.media_seen().len(), 2);
        assert!(rig.backend.media_seen().contains(&original));
        // And a job that starts at the audio, not at a download.
        assert_eq!(
            rig.backend.calls().first().map(String::as_str),
            Some("prepare_audio")
        );
    }

    #[test]
    fn an_imported_file_is_named_by_its_file_until_the_summary_names_it() {
        let rig = Rig::empty(FakeBackend::new());
        let original = media_file(&rig.library.outside(), "cuộc họp tuần 1.mp4");

        let ids = rig
            .meetings
            .import_files(&[original.to_string_lossy().into_owned()]);
        rig.ended(&ids[0]);

        // The summarizer was told the file's name as the title, and that it is no title of the recording's own.
        let (title, untitled, _) = rig.backend.summarize_seen().remove(0);
        assert_eq!((title.as_str(), untitled), ("cuộc họp tuần 1", true));
        let meta = rig.meta(&ids[0]);
        assert_eq!(meta.title, "Weekly sync");
        // Its source is still the file, and the media with it.
        assert_eq!(PathBuf::from(&meta.source), original);
        assert_eq!(PathBuf::from(&meta.media), original);
        assert_eq!(
            rig.files(&ids[0]),
            ["log.txt", "meta.json", "summary.md", "transcript.txt"]
        );
    }

    #[test]
    fn a_file_is_imported_by_its_absolute_path_and_a_folder_is_not_imported() {
        let rig = Rig::empty(FakeBackend::new());
        let dir = rig.library.outside();
        let original = media_file(&dir, "call.wav");
        let folder = original.parent().unwrap().to_path_buf();

        let ids = rig.meetings.import_files(&[
            folder.to_string_lossy().into_owned(),
            original.to_string_lossy().into_owned(),
        ]);
        assert_eq!(ids.len(), 1, "a folder is not media");
        rig.ended(&ids[0]);
        assert!(Path::new(&rig.meta(&ids[0]).media).is_absolute());

        // No files, no recordings.
        assert!(rig.meetings.import_files(&[]).is_empty());
    }

    #[test]
    fn two_files_imported_in_one_second_get_two_folders() {
        let rig = Rig::empty(FakeBackend::new());
        let dir = rig.library.outside();
        let (one, two) = (media_file(&dir, "one.wav"), media_file(&dir, "two.wav"));
        let ids = rig.meetings.import_files(&[
            one.to_string_lossy().into_owned(),
            two.to_string_lossy().into_owned(),
        ]);
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        for id in &ids {
            rig.ended(id);
        }
    }

    // ---- recording

    use dictate_core::meetings::library::SCREEN_SOURCE;
    use dictate_services::record::session::Finished;

    /// A recording that ended well: `name` is in its folder, with something in it.
    fn finished(rig: &Rig, id: &str, name: &str, notes: &[&str]) -> Finished {
        let media = std::path::absolute(rig.library.folder(id).join(name)).unwrap();
        fs::write(&media, b"sound").unwrap();
        Finished {
            media,
            duration: 12.5,
            notes: notes.iter().map(|note| (*note).to_owned()).collect(),
        }
    }

    #[test]
    fn a_recording_begins_as_a_folder_with_the_source_of_the_swift_app_and_no_job() {
        let rig = Rig::empty(FakeBackend::new());

        let (id, folder) = rig
            .meetings
            .begin_recording("Screen recording 10:00 10/11", "audio.m4a")
            .unwrap();

        assert_eq!(folder, rig.library.folder(&id));
        assert!(folder.is_dir());
        let meta = rig.meta(&id);
        assert_eq!(meta.title, "Screen recording 10:00 10/11");
        assert_eq!(meta.source, SCREEN_SOURCE);
        // Where the recording will be, as an absolute path: the pipeline and the player read it from there.
        assert!(Path::new(&meta.media).is_absolute(), "{}", meta.media);
        assert!(Path::new(&meta.media).ends_with(format!("{id}/audio.m4a")));
        assert_eq!(meta.duration, None);
        assert!(meta.created > 1_700_000_000.0, "{}", meta.created);
        // Nothing runs until the recording is over, and the window is not told of a recording that has not begun.
        assert_eq!(rig.meetings.registry.status(&id), None);
        assert!(rig.backend.calls().is_empty());
    }

    #[test]
    fn a_recording_that_ends_well_gets_its_media_and_duration_and_a_job_from_the_audio() {
        // The job is held at the audio, so that what the recording wrote is read before the pipeline writes its own.
        let (backend, control) = held_at_audio();
        let rig = Rig::empty(backend);
        let (id, folder) = rig
            .meetings
            .begin_recording("Họp", "recording.mp4")
            .unwrap();
        let done = finished(&rig, &id, "audio.m4a", &["The system's audio was lost."]);

        rig.meetings.finish_recording(&id, &folder, &done).unwrap();
        control.started.blocking_recv().unwrap();

        let meta = rig.meta(&id);
        assert_eq!(PathBuf::from(&meta.media), done.media);
        assert_eq!(meta.duration, Some(12.5));
        assert_eq!(meta.source, SCREEN_SOURCE);
        assert_eq!(meta.title, "Họp");
        // What went wrong on the way is in the log of the recording, before the pipeline's own lines.
        let log = rig.meetings.log(&id).unwrap();
        assert!(log.starts_with("The system's audio was lost.\n"), "{log:?}");
        // The job starts at the audio, from the file that was made.
        assert_eq!(rig.backend.calls(), ["prepare_audio"]);
        assert_eq!(rig.backend.media_seen(), [done.media]);
        control.release.send(()).unwrap();
        rig.ended(&id);
    }

    #[test]
    fn a_recording_whose_media_is_a_wav_is_a_recording_like_any_other() {
        // With no encoder for audio.m4a the sound is kept as recording.wav: the library plays and opens it.
        let rig = Rig::empty(FakeBackend::new());
        let (id, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        let done = finished(&rig, &id, "recording.wav", &[]);

        rig.meetings.finish_recording(&id, &folder, &done).unwrap();
        rig.ended(&id);

        assert_eq!(rig.meetings.media_path(&id).unwrap(), done.media);
        assert_eq!(rig.backend.media_seen(), [done.media]);
    }

    #[test]
    fn a_recording_without_a_known_length_leaves_the_duration_to_the_pipeline() {
        let (backend, control) = held_at_audio();
        let rig = Rig::empty(backend);
        let (id, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        let mut done = finished(&rig, &id, "audio.m4a", &[]);
        done.duration = 0.0;

        rig.meetings.finish_recording(&id, &folder, &done).unwrap();
        control.started.blocking_recv().unwrap();

        assert_eq!(rig.meta(&id).duration, None);
        control.release.send(()).unwrap();
        rig.ended(&id);
    }

    #[test]
    fn a_folder_with_a_marker_and_no_meta_is_given_one_when_its_recording_is_finished() {
        let (backend, control) = held_at_audio();
        let rig = Rig::empty(backend);
        let (id, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        fs::remove_file(folder.join(files::META)).unwrap();
        let done = finished(&rig, &id, "audio.m4a", &[]);

        rig.meetings.finish_recording(&id, &folder, &done).unwrap();
        control.started.blocking_recv().unwrap();

        let meta = rig.meta(&id);
        assert_eq!(meta.title, id);
        assert_eq!(meta.source, SCREEN_SOURCE);
        assert_eq!(PathBuf::from(&meta.media), done.media);
        control.release.send(()).unwrap();
        rig.ended(&id);
    }

    #[test]
    fn a_recording_that_has_not_ended_cannot_be_run_again_by_a_click() {
        // The place of the recording is held while it is made: "Try again" or "Transcribe again" on its row would start
        // a job on a folder that is being written.
        let rig = Rig::empty(FakeBackend::new());
        let (id, _) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();

        let held = rig.meetings.registry.hold(&id).unwrap();

        assert_eq!(
            rig.meetings.rerun(&id).unwrap_err().key,
            "meetings.error.busy"
        );
        assert_eq!(
            rig.meetings.retry(&id).unwrap_err().key,
            "meetings.error.busy"
        );
        drop(held);
        assert!(rig.backend.calls().is_empty());
    }

    #[test]
    fn a_folder_that_a_start_left_is_removed_and_no_other_folder_is() {
        let rig = Rig::new();
        let (id, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        fs::write(folder.join("recording.json"), b"{}").unwrap();

        rig.meetings.discard_folder(&folder);

        assert!(!folder.exists(), "{id} is gone");
        assert!(
            rig.library.folder(SCREEN).is_dir(),
            "the others are not touched"
        );
        // A path that is not a folder of the library is left alone, whoever asks.
        let outside = rig.library.outside();
        rig.meetings.discard_folder(&outside);
        assert!(outside.is_dir());
        rig.meetings.discard_folder(&rig.library.dir());
        assert!(rig.library.dir().is_dir());
    }

    #[test]
    fn a_folder_with_nothing_recorded_in_it_is_removed_by_what_the_library_wrote_and_not_by_force()
    {
        let rig = Rig::new();
        let (_, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        fs::write(folder.join(files::LOG), "The recording was interrupted.").unwrap();

        rig.meetings.discard_nothing_recorded(&folder);

        assert!(!folder.exists());
        assert!(
            lock(&rig.trash.moved).is_empty(),
            "there is nothing to get back"
        );
        assert!(
            rig.library.folder(SCREEN).is_dir(),
            "the others are not touched"
        );
        // A path that is not a folder of the library is left alone, whoever asks.
        let outside = rig.library.outside();
        rig.meetings.discard_nothing_recorded(&outside);
        assert!(outside.is_dir());
    }

    #[test]
    fn a_folder_that_has_anything_else_in_it_goes_to_the_trash_and_is_not_removed() {
        // A file of the recording that could not be used (a stem that cannot be read, a video that cannot be played) may be
        // what a person can still get something out of: it is moved, and can be got back, and nothing deletes it.
        let rig = Rig::new();
        let (_, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        fs::write(folder.join("mic.wav.unreadable"), b"RIFF").unwrap();

        rig.meetings.discard_nothing_recorded(&folder);

        assert_eq!(*lock(&rig.trash.moved), [(folder.clone(), false)]);
    }

    #[test]
    fn a_folder_that_has_anything_else_in_it_stays_when_the_trash_will_not_take_it() {
        let mut rig = Rig::new();
        let jobs = rig.trash.jobs.clone();
        rig.trash = Arc::new(FakeTrash {
            jobs,
            moved: Mutex::new(Vec::new()),
            fail: true,
        });
        rig.meetings.trash = rig.trash.clone();
        let (_, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        fs::write(folder.join("video.mp4"), b"a video that may be played").unwrap();

        rig.meetings.discard_nothing_recorded(&folder);

        assert_eq!(lock(&rig.trash.moved).len(), 1, "the Trash was tried");
        // Nothing is removed, `meta.json` included: the recording is still there to be seen and to be deleted by its person.
        assert_eq!(
            fs::read(folder.join("video.mp4")).unwrap(),
            b"a video that may be played"
        );
        assert!(folder.join(files::META).is_file());
    }

    #[test]
    fn a_recording_that_is_thrown_away_goes_to_the_trash_and_the_window_is_told() {
        let rig = Rig::empty(FakeBackend::new());
        let (_, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();
        let before = rig.changed();

        rig.meetings.trash_recording(&folder).unwrap();

        assert_eq!(*lock(&rig.trash.moved), [(folder.clone(), false)]);
        assert!(!folder.exists());
        assert_eq!(rig.changed(), before + 1);
        assert!(rig.backend.calls().is_empty(), "no job was started");
    }

    #[test]
    fn a_recording_is_removed_when_the_trash_will_not_take_it() {
        let mut rig = Rig::empty(FakeBackend::new());
        let jobs = rig.trash.jobs.clone();
        rig.trash = Arc::new(FakeTrash {
            jobs,
            moved: Mutex::new(Vec::new()),
            fail: true,
        });
        rig.meetings.trash = rig.trash.clone();
        let (_, folder) = rig.meetings.begin_recording("Họp", "audio.m4a").unwrap();

        rig.meetings.trash_recording(&folder).unwrap();

        assert_eq!(lock(&rig.trash.moved).len(), 1, "the Trash was tried first");
        assert!(!folder.exists(), "and then the folder was removed");
    }

    #[test]
    fn only_a_folder_of_the_library_is_trashed_as_a_recording() {
        let rig = Rig::empty(FakeBackend::new());
        let outside = rig.library.outside();

        assert!(rig.meetings.trash_recording(&outside).is_err());

        assert!(outside.is_dir());
        assert!(lock(&rig.trash.moved).is_empty());
    }

    // ---- starting jobs again

    #[test]
    fn rerun_starts_at_the_audio_and_resummarize_at_the_summary() {
        let rig = Rig::new();
        rig.give_media(SCREEN);
        rig.give_media(FILE);
        // The recording has its transcript: it can be summarized again.
        rig.meetings.resummarize(SCREEN).unwrap();
        rig.ended(SCREEN);
        assert_eq!(rig.backend.calls(), ["summarize"]);

        // A recording with no transcript is summarized again from the audio.
        rig.meetings.resummarize(FILE).unwrap();
        rig.ended(FILE);
        assert_eq!(
            rig.backend.calls().get(1).map(String::as_str),
            Some("prepare_audio")
        );

        rig.meetings.rerun(SCREEN).unwrap();
        rig.ended(SCREEN);
        assert_eq!(
            rig.backend.calls().get(4).map(String::as_str),
            Some("prepare_audio")
        );

        assert!(rig.meetings.rerun("no such recording").is_err());
    }

    #[test]
    fn a_recording_that_is_being_processed_is_not_started_a_second_time() {
        let (backend, control) = held_at_audio();
        let rig = Rig::with(backend, Fixture::new(), None);
        rig.give_media(SCREEN);
        rig.meetings.rerun(SCREEN).unwrap();
        control.started.blocking_recv().unwrap();

        // A second click is answered with the key of the text that says why, not with English.
        assert_eq!(
            rig.meetings.rerun(SCREEN).unwrap_err().key,
            "meetings.error.busy"
        );
        assert_eq!(
            rig.meetings.resummarize(SCREEN).unwrap_err().key,
            "meetings.error.busy"
        );
        assert_eq!(
            rig.meetings.retry(SCREEN).unwrap_err().key,
            "meetings.error.busy"
        );
        assert!(rig.meetings.cancel(SCREEN));
        rig.ended(SCREEN);
        assert!(!rig.meetings.cancel(SCREEN));
    }

    #[test]
    fn try_again_starts_where_it_failed() {
        let rig = Rig::new();
        let folder = |id: &str| rig.library.folder(id);
        let record = |id: &str| MeetingRecord::from_folder(&folder(id)).unwrap();

        // A link whose download did not come to its end has no media: it downloads again.
        assert_eq!(retry_start(&record(LINK)), StartAt::Download);
        // The same link with its media on disk goes on from the transcript it has.
        let media = media_file(&rig.library.outside(), "source.m4a");
        let mut meta = rig.meta(LINK);
        meta.media = media.to_string_lossy().into_owned();
        meta.write(&folder(LINK)).unwrap();
        assert_eq!(retry_start(&record(LINK)), StartAt::Summary);
        // A recording with a transcript is only summarized again, whatever its media is; one without starts over.
        assert_eq!(retry_start(&record(SCREEN)), StartAt::Summary);
        assert_eq!(retry_start(&record(FILE)), StartAt::Audio);
        // Live captions are not a transcript to summarize.
        rig.library.write(
            "2026-11-09 10-00-00",
            Some(json!({"title": "Live", "source": "https://a.test/x", "media": media.to_string_lossy()})),
            &[(files::LIVE_TRANSCRIPT, "[00:00] Bạn: xin chào")],
        );
        assert_eq!(retry_start(&record("2026-11-09 10-00-00")), StartAt::Audio);
    }

    #[test]
    fn try_again_on_a_failed_link_downloads_again() {
        let rig = Rig::empty(FakeBackend::new());
        rig.library.write(
            "2026-11-09 10-00-00",
            Some(json!({"title": "https://a.test/x", "source": "https://a.test/x", "media": "", "created": 1_792_423_258.0})),
            &[],
        );

        rig.meetings.retry("2026-11-09 10-00-00").unwrap();
        rig.ended("2026-11-09 10-00-00");

        assert_eq!(
            rig.backend.calls().first().map(String::as_str),
            Some("download")
        );
    }

    // ---- editing

    #[test]
    fn ticking_an_action_item_writes_tasks_json_as_a_sorted_list() {
        let rig = Rig::new();
        let tasks = || fs::read_to_string(rig.library.folder(SCREEN).join(files::TASKS)).unwrap();
        let before = rig.changed();

        // The fixture has the first ticked.
        rig.meetings.set_task(SCREEN, 1, true).unwrap();
        assert_eq!(tasks(), "[0,1]");
        rig.meetings.set_task(SCREEN, 0, false).unwrap();
        assert_eq!(tasks(), "[1]");
        rig.meetings.set_task(SCREEN, 1, false).unwrap();
        assert_eq!(tasks(), "[]");
        // The list shows the open items, and was told each time.
        assert_eq!(rig.meetings.list("")[0].open_tasks, 2);
        assert_eq!(rig.changed(), before + 3);
        // No temporary file is left.
        assert!(!rig.files(SCREEN).iter().any(|name| name.ends_with(".tmp")));

        // An item that is not there, and a recording with no summary.
        assert!(rig.meetings.set_task(SCREEN, 2, true).is_err());
        assert!(rig.meetings.set_task(LINK, 0, true).is_err());
        assert!(rig.meetings.set_task("no such recording", 0, true).is_err());
    }

    #[test]
    fn renaming_speakers_changes_the_transcript_and_the_summary_and_nothing_else() {
        let rig = Rig::new();
        let before_meta = fs::read(rig.library.folder(SCREEN).join(files::META)).unwrap();
        let before_tasks = fs::read(rig.library.folder(SCREEN).join(files::TASKS)).unwrap();
        let names = HashMap::from([("Speaker 1".to_owned(), "Lan".to_owned())]);

        let applied = rig.meetings.rename_speakers(SCREEN, &names, false).unwrap();

        assert_eq!(applied, [("Speaker 1".to_owned(), "Lan".to_owned())]);
        let transcript =
            fs::read_to_string(rig.library.folder(SCREEN).join(files::TRANSCRIPT)).unwrap();
        assert!(
            transcript.contains("[00:00] Lan: Chào mọi người"),
            "{transcript}"
        );
        assert!(transcript.contains("Speaker 2: "), "the other one is left");
        // meta.json keeps its date as it was (a missing or zero `created` is never written over a real one), the ticks stay,
        // and the log says what was done.
        assert_eq!(
            fs::read(rig.library.folder(SCREEN).join(files::META)).unwrap(),
            before_meta
        );
        assert_eq!(
            fs::read(rig.library.folder(SCREEN).join(files::TASKS)).unwrap(),
            before_tasks
        );
        assert!(
            fs::read_to_string(rig.library.folder(SCREEN).join(files::LOG))
                .unwrap()
                .contains("Renamed speakers: Speaker 1 → Lan")
        );
        // Nobody to rename is no change.
        let none = rig
            .meetings
            .rename_speakers(SCREEN, &HashMap::new(), false)
            .unwrap();
        assert!(none.is_empty());
        assert!(
            rig.backend.calls().is_empty(),
            "no AI call without being asked"
        );
    }

    #[test]
    fn renaming_speakers_can_summarize_again_with_the_new_names() {
        let rig = Rig::new();
        let names = HashMap::from([("Speaker 2".to_owned(), "Bình".to_owned())]);

        rig.meetings.rename_speakers(SCREEN, &names, true).unwrap();
        rig.ended(SCREEN);

        assert_eq!(rig.backend.calls(), ["summarize"]);
        let (_, _, transcript) = rig.backend.summarize_seen().remove(0);
        assert!(
            transcript.contains("Bình"),
            "the summary is of the transcript with the new names"
        );
        // The summary is another one, and its ticks are gone.
        assert!(!rig.library.folder(SCREEN).join(files::TASKS).exists());

        // No renames, nothing to summarize again.
        let rig = Rig::new();
        rig.meetings
            .rename_speakers(SCREEN, &HashMap::new(), true)
            .unwrap();
        assert!(!rig.meetings.registry.is_running(SCREEN));
    }

    /// The sheet that renames speakers puts its own sentence ("Couldn't save the speaker names: …", in the language of the window) before
    /// the reason, so the reason is all that the error carries: not its English words as well.
    #[cfg(unix)]
    #[test]
    fn a_rename_that_cannot_be_written_carries_only_the_reason() {
        use std::os::unix::fs::PermissionsExt;
        let rig = Rig::new();
        let folder = rig.library.folder(SCREEN);
        let names = HashMap::from([("Speaker 1".to_owned(), "Lan".to_owned())]);
        // A folder that nothing can be written in: the temporary file of the write cannot be made.
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
        let writable = fs::write(folder.join("probe"), b"").is_ok();

        let result = rig.meetings.rename_speakers(SCREEN, &names, false);

        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            // Whoever runs this can write anywhere (root): there is no failure to look at.
            return;
        }
        let refused = result.unwrap_err();
        assert_eq!(
            (refused.kind.as_str(), refused.key.as_str()),
            ("Other", "meetings.error.other")
        );
        let detail = refused.detail.expect("the reason");
        assert!(
            !detail.starts_with("Couldn't"),
            "the window has the words, the detail is the reason: {detail}"
        );
        assert!(detail.to_lowercase().contains("denied"), "{detail}");
        // Nothing was changed.
        assert!(fs::read_to_string(folder.join(files::TRANSCRIPT))
            .unwrap()
            .contains("Speaker 1: "));
    }

    #[test]
    fn rename_refused_while_running() {
        let (backend, control) = held_at_audio();
        let rig = Rig::with(backend, Fixture::new(), None);
        rig.give_media(SCREEN);
        rig.meetings.rerun(SCREEN).unwrap();
        control.started.blocking_recv().unwrap();
        let before = fs::read(rig.library.folder(SCREEN).join(files::TRANSCRIPT)).unwrap();
        let names = HashMap::from([("Speaker 1".to_owned(), "Lan".to_owned())]);

        let refused = rig
            .meetings
            .rename_speakers(SCREEN, &names, false)
            .unwrap_err();

        assert_eq!(
            (refused.kind.as_str(), refused.key.as_str()),
            ("Other", "meetings.error.busy")
        );
        assert_eq!(
            fs::read(rig.library.folder(SCREEN).join(files::TRANSCRIPT)).unwrap(),
            before
        );
        assert!(rig.meetings.registry.is_running(SCREEN));

        // When the job is over the names can be given.
        rig.meetings.cancel(SCREEN);
        rig.ended(SCREEN);
        assert!(rig.meetings.rename_speakers(SCREEN, &names, false).is_ok());
    }

    #[test]
    fn work_on_the_files_of_a_recording_holds_its_place_and_gives_it_back() {
        let rig = Rig::new();
        let busy = |error: ErrorView| (error.kind, error.key);

        let inside = rig
            .meetings
            .while_held(SCREEN, || {
                (
                    rig.meetings.registry.is_running(SCREEN),
                    rig.meetings.resummarize(SCREEN).map_err(busy),
                    rig.meetings.while_held(SCREEN, || ()).map_err(busy),
                )
            })
            .unwrap();

        let refused = Err(("Other".to_owned(), "meetings.error.busy".to_owned()));
        assert_eq!(inside, (true, refused.clone(), refused));
        // Given back, whatever the work did: a job can start.
        assert!(!rig.meetings.registry.is_running(SCREEN));
        rig.meetings.resummarize(SCREEN).unwrap();
        rig.ended(SCREEN);
        // Another recording is not held up while this one is.
        rig.meetings
            .while_held(SCREEN, || rig.meetings.while_held(LINK, || ()))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn a_recording_that_has_a_job_is_not_held() {
        let (backend, control) = held_at_audio();
        let rig = Rig::with(backend, Fixture::new(), None);
        rig.give_media(SCREEN);
        rig.meetings.rerun(SCREEN).unwrap();
        control.started.blocking_recv().unwrap();
        let mut worked = false;

        let refused = rig.meetings.while_held(SCREEN, || worked = true);

        assert_eq!(refused.unwrap_err().key, "meetings.error.busy");
        assert!(!worked, "the work was done anyway");
        rig.meetings.cancel(SCREEN);
        rig.ended(SCREEN);
    }

    #[test]
    fn renaming_speakers_gives_the_recording_back_whatever_came_of_it() {
        let rig = Rig::new();
        let names = HashMap::from([("Speaker 1".to_owned(), "Lan".to_owned())]);

        // Renamed, and summarized again at once: the job could not start with the place still held.
        rig.meetings.rename_speakers(SCREEN, &names, true).unwrap();
        rig.ended(SCREEN);
        assert_eq!(rig.backend.calls(), ["summarize"]);
        // Nothing to rename, and a recording that does not exist.
        rig.meetings
            .rename_speakers(SCREEN, &HashMap::new(), false)
            .unwrap();
        assert!(rig
            .meetings
            .rename_speakers("no such recording", &names, false)
            .is_err());
        assert!(!rig.meetings.registry.is_running(SCREEN));
        assert!(!rig.meetings.registry.is_running("no such recording"));
    }

    // ---- exporting and reading

    #[test]
    fn the_markdown_is_the_summary_then_the_transcript() {
        let rig = Rig::new();

        let markdown = rig.meetings.markdown(SCREEN).unwrap();
        assert_eq!(
            markdown,
            format!("{SUMMARY}\n\n---\n\n## Transcript\n\n{TRANSCRIPT}")
        );
        // No summary: the title stands for it. No transcript: there is none.
        assert_eq!(
            rig.meetings.markdown(LINK).unwrap(),
            "# Video về Swift\n\n---\n\n## Transcript\n\n[00:00] Swift concurrency rất hay."
        );
        assert!(rig.meetings.markdown(FILE).unwrap().ends_with("Ngắn gọn."));
    }

    #[test]
    fn an_export_is_offered_under_the_title_made_safe_and_written_whole() {
        let rig = Rig::new();
        rig.library.write(
            "2026-11-09 10-00-00",
            Some(json!({"title": "Họp: Q4 / kế hoạch?"})),
            &[(files::SUMMARY, "# Họp\n\n## Tóm tắt\nx")],
        );
        assert_eq!(
            rig.meetings.export_name("2026-11-09 10-00-00").unwrap(),
            "Họp- Q4 - kế hoạch-.md"
        );

        let destination = rig.library.outside().join("Xuất ra.md");
        rig.meetings.export_to(SCREEN, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            rig.meetings.markdown(SCREEN).unwrap()
        );
        // The folder is this test's own: the export is all that is in it, so no temporary file of the write is left.
        let names: Vec<String> = fs::read_dir(rig.library.outside())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Xuất ra.md"]);
    }

    #[test]
    fn a_failed_export_is_an_error_the_window_can_show() {
        let rig = Rig::new();
        let nowhere = rig.library.dir().join("no such folder").join("a.md");

        let error = rig.meetings.export_to(SCREEN, &nowhere).unwrap_err();

        assert_eq!(error.key, "meetings.error.other");
        assert!(error.detail.unwrap().starts_with("Couldn't save"));
    }

    #[test]
    fn the_log_is_read_as_it_is_and_empty_when_there_is_none() {
        let rig = Rig::new();
        assert_eq!(rig.meetings.log(SCREEN).unwrap(), "");
        fs::write(
            rig.library.folder(SCREEN).join(files::LOG),
            "Audio: 00:10\nDone: summary.md\n",
        )
        .unwrap();
        assert_eq!(
            rig.meetings.log(SCREEN).unwrap(),
            "Audio: 00:10\nDone: summary.md\n"
        );
        assert!(rig.meetings.log("no such recording").is_err());
    }

    #[test]
    fn the_media_to_open_is_the_media_file_when_it_is_there() {
        let rig = Rig::new();
        let dir = rig.library.outside();
        let media = media_file(&dir, "talk.mp4");
        rig.library.write(
            "2026-11-09 10-00-00",
            Some(json!({"title": "T", "source": media.to_string_lossy(), "media": media.to_string_lossy()})),
            &[],
        );
        assert_eq!(
            rig.meetings.media_path("2026-11-09 10-00-00").unwrap(),
            media
        );
        fs::remove_file(&media).unwrap();
        assert!(rig.meetings.media_path("2026-11-09 10-00-00").is_err());
        // A recording with no media file in `meta.json`.
        let none = rig.meetings.media_path(SCREEN).unwrap_err();
        assert_eq!(
            (none.kind.as_str(), none.key.as_str()),
            ("NoMedia", "meetings.error.no_media")
        );
    }

    #[test]
    fn only_a_media_file_is_opened_in_another_app() {
        // `meta.json` is data: a library that came from elsewhere may name anything as the media, and opening it would run it.
        let rig = Rig::new();
        let dir = rig.library.outside();
        let named = |name: &str| {
            let path = dir.join(name);
            fs::write(&path, b"x").unwrap();
            rig.library.write(
                "2026-11-09 10-00-00",
                Some(json!({"title": "T", "media": path.to_string_lossy()})),
                &[],
            );
            path
        };
        for name in ["talk.mp4", "talk.MOV", "call.m4a", "voice.wav", "clip.webm"] {
            let path = named(name);
            assert_eq!(
                rig.meetings.media_path("2026-11-09 10-00-00"),
                Ok(path),
                "{name}"
            );
        }
        for name in [
            "evil.command",
            "run.sh",
            "setup.exe",
            "script.bat",
            "noextension",
            "notes.txt",
            "x.app",
        ] {
            named(name);
            assert!(
                rig.meetings.media_path("2026-11-09 10-00-00").is_err(),
                "{name}"
            );
        }
    }

    // ---- deleting

    #[test]
    fn delete_moves_to_trash_and_cancels() {
        let (backend, control) = held_at_audio();
        let rig = Rig::with(backend, Fixture::new(), None);
        rig.give_media(SCREEN);
        rig.meetings.rerun(SCREEN).unwrap();
        control.started.blocking_recv().unwrap();
        let before = rig.changed();

        tauri::async_runtime::block_on(rig.meetings.delete(SCREEN)).unwrap();

        // The job was cancelled and had ended when the folder went to the Trash.
        let moved = lock(&rig.trash.moved).clone();
        assert_eq!(moved, [(rig.library.folder(SCREEN), false)]);
        assert!(!rig.library.folder(SCREEN).exists());
        assert!(
            !control.finished.load(Ordering::SeqCst),
            "the job was cancelled, not let through"
        );
        // The list is told, the recording is gone from it, and what was kept of its job with it.
        assert!(rig.changed() > before);
        assert_eq!(rig.meetings.registry.status(SCREEN), None);
        let ids: Vec<_> = rig
            .meetings
            .list("")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(ids, [LINK, FILE]);
        // Nothing else was touched.
        assert!(rig.library.folder(LINK).is_dir());
    }

    #[test]
    fn deleting_a_recording_that_has_no_job_goes_straight_to_the_trash() {
        let rig = Rig::new();
        tauri::async_runtime::block_on(rig.meetings.delete(FILE)).unwrap();
        assert_eq!(lock(&rig.trash.moved).len(), 1);
        assert!(
            tauri::async_runtime::block_on(rig.meetings.delete(FILE)).is_err(),
            "it is gone"
        );
    }

    #[test]
    fn a_trash_that_fails_leaves_the_recording_in_the_list() {
        let mut rig = Rig::new();
        let jobs = rig.trash.jobs.clone();
        rig.trash = Arc::new(FakeTrash {
            jobs,
            moved: Mutex::new(Vec::new()),
            fail: true,
        });
        rig.meetings.trash = rig.trash.clone();

        let error = tauri::async_runtime::block_on(rig.meetings.delete(FILE)).unwrap_err();

        assert!(error.contains("The Trash is full."), "{error}");
        assert!(rig.library.folder(FILE).is_dir());
        assert_eq!(rig.meetings.list("").len(), 3);
    }

    // ---- questions

    /// An app state in which the AI is not set up, and a key store that counts how often it is asked.
    fn state(dir: &Path) -> (AppState, Arc<CountingStore>) {
        let (store, _) = SettingsStore::open(dir.join("settings.json"));
        let secrets = Arc::new(CountingStore::default());
        let models = ModelDir::new(dir.join("models"));
        (AppState::new(store, None, secrets.clone(), models), secrets)
    }

    #[test]
    fn a_blank_question_is_refused_before_anything_is_asked() {
        let rig = Rig::new();
        let dir = tempfile::tempdir().unwrap();
        let (state, secrets) = state(dir.path());

        for blank in ["", "   ", "\n\t "] {
            let error = tauri::async_runtime::block_on(rig.meetings.ask(&state, SCREEN, blank))
                .unwrap_err();
            assert_eq!(error.key, "meetings.error.empty_question", "{blank:?}");
            assert_eq!(error.kind, "Other");
        }
        assert_eq!(secrets.gets(), 0, "the key store was not asked");
    }

    #[test]
    fn a_question_needs_an_ai_and_a_transcript_and_does_not_ask_the_key_store_without_a_key() {
        let rig = Rig::new();
        let dir = tempfile::tempdir().unwrap();
        let (state, secrets) = state(dir.path());
        // The start of the app reads the flags once; after that the key store is not asked for them.
        state.loaded_keys();
        let asked = secrets.gets();
        state
            .keys
            .set(dictate_services::secrets::Account::Ai, false);

        // No AI key saved (as far as the flags say): there is nothing to ask.
        let error =
            tauri::async_runtime::block_on(rig.meetings.ask(&state, SCREEN, "Who owns the docs?"))
                .unwrap_err();
        // The window says it in its own language, from the key; the buttons of an API error are for it.
        assert_eq!(
            (error.kind.as_str(), error.key.as_str(), error.detail),
            ("Api", "meetings.error.no_ai", None)
        );
        assert_eq!(secrets.gets(), asked);

        // A recording with no transcript has nothing to ask about.
        state.keys.set(dictate_services::secrets::Account::Ai, true);
        let error =
            tauri::async_runtime::block_on(rig.meetings.ask(&state, FILE, "Who owns the docs?"))
                .unwrap_err();
        assert_eq!(error.key, "meetings.error.no_transcript");
        let error =
            tauri::async_runtime::block_on(rig.meetings.ask(&state, "no such recording", "Who?"))
                .unwrap_err();
        assert_eq!(error.key, "meetings.error.other");
    }

    // ---- start-up

    #[test]
    fn startup_sweeps_every_recording_folder() {
        let f = Fixture::new();
        let stale = |id: &str, name: &str| fs::write(f.folder(id).join(name), b"x").unwrap();
        stale(SCREEN, "transcript.txt.123-0.tmp");
        stale(SCREEN, "audio.m4a.4d2-0.part");
        stale(LINK, "source.m4a.part");
        stale(LINK, "api-slice-0.wav");
        stale(FILE, "meta.json.9-1.tmp");
        fs::create_dir_all(f.folder(LINK).join("api-parts")).unwrap();
        fs::write(f.folder(LINK).join("api-parts").join("0.json"), b"[]").unwrap();
        // Files of the library folder itself are not a recording's.
        fs::write(f.dir().join("notes.tmp"), b"x").unwrap();

        assert_eq!(sweep_library(&f.dir()), 5);

        // What is whole stays, with the cache of the API parts a second run continues from.
        assert!(f.folder(SCREEN).join(files::TRANSCRIPT).is_file());
        assert!(f.folder(SCREEN).join(files::SUMMARY).is_file());
        assert!(f.folder(LINK).join("api-parts").join("0.json").is_file());
        assert!(f.dir().join("notes.tmp").is_file());
        assert_eq!(sweep_library(&f.dir()), 0);
        assert_eq!(sweep_library(&f.dir().join("no such library")), 0);
    }

    #[test]
    fn startup_leaves_the_parts_of_a_recording_that_is_still_to_be_finished() {
        // A recording that was cut off and could not be put together yet keeps its video, which is a `.part` file, for the next
        // try: the sweep is for what a job left, and this is not that.
        let f = Fixture::new();
        let cut_off = f.folder(FILE);
        fs::write(cut_off.join("recording.json"), b"{}").unwrap();
        fs::write(cut_off.join("video.mp4.1a2b.part"), b"video").unwrap();
        fs::write(cut_off.join("meta.json.9-1.tmp"), b"x").unwrap();
        fs::write(f.folder(SCREEN).join("audio.m4a.4d2-0.part"), b"x").unwrap();

        assert_eq!(sweep_library(&f.dir()), 1);

        assert!(cut_off.join("video.mp4.1a2b.part").is_file());
        assert!(cut_off.join("meta.json.9-1.tmp").is_file());
        assert!(!f.folder(SCREEN).join("audio.m4a.4d2-0.part").exists());
    }

    #[test]
    fn the_settings_the_window_may_ask_to_see_are_the_tabs_of_settings() {
        for tab in ["general", "speech", "ai", "accounts"] {
            assert!(is_settings_tab(tab), "{tab}");
        }
        for tab in ["", "General", "meetings", "../x", "accounts "] {
            assert!(!is_settings_tab(tab), "{tab:?}");
        }
    }
}
