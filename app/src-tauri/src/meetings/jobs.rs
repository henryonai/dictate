//! The jobs that process recordings: one pipeline run for each recording that is being worked on, started and stopped here,
//! with what each is doing told to the window and kept for the ones that come later.
//!
//! A recording has at most one job (`Job::Meeting(id)` in `state::Jobs`, where quitting cancels them all). What a job does
//! is `pipeline::run`; this adds the things around it: waiting for the start-up of the app, one job at a time for each id,
//! the events of the window, the last outcome of each recording kept in memory, and the banner that says it is done.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dictate_core::meetings::library::{files, Meta};
use dictate_core::meetings::SummaryDoc;
use dictate_services::meetings::{
    pipeline, ErrorKind, JobEvent, JobSpec, LocalLock, MeetingBackend, MeetingError, Outcome,
    StartAt,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

use super::notify::{Notice, NoticeKind, Notifier};
use super::views::{JobView, OutcomeView, StepView};
use crate::state::{lock, AlreadyRunning, Job, JobGuard, Jobs};

/// The event that tells the window how a job goes: a [`JobView`] each time the step or the progress changes, and one with
/// an outcome when it has ended.
pub const JOB_EVENT: &str = "meeting-job";
/// The event that tells the window that the list of recordings changed: a recording is new, gone, has a new title, or a
/// job of it started or ended.
pub const CHANGED_EVENT: &str = "meetings-changed";

/// What happens in the jobs, for whoever shows it.
///
/// `job()` and `changed()` may be called while the registry's table of views is locked (so that the events of a recording
/// come in the order the table changed): an implementation must not call back into the [`Registry`] (`start`, `status` and
/// the rest take that lock), or it deadlocks.
pub trait Events: Send + Sync {
    fn job(&self, view: &JobView);
    fn changed(&self);
}

/// The events of the app: to the meetings window.
pub struct AppEvents(pub AppHandle);

impl Events for AppEvents {
    fn job(&self, view: &JobView) {
        if let Err(e) = self.0.emit_to(super::window::LABEL, JOB_EVENT, view) {
            log::warn!("Could not tell the meetings window of a job: {e}");
        }
    }

    fn changed(&self) {
        if let Err(e) = self.0.emit_to(super::window::LABEL, CHANGED_EVENT, ()) {
            log::warn!("Could not tell the meetings window of a change: {e}");
        }
    }
}

/// The time, in milliseconds since the Unix epoch.
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

/// The time of the system.
pub fn system_clock() -> Clock {
    Box::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    })
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    backend: Arc<dyn MeetingBackend>,
    /// One local transcription at a time, whoever asks.
    local: LocalLock,
    jobs: Arc<Jobs>,
    events: Arc<dyn Events>,
    notifier: Arc<dyn Notifier>,
    now: Clock,
    /// For each recording that has had a job since the app started: the job that is running (no outcome) or how the last
    /// one ended.
    views: Mutex<HashMap<String, JobView>>,
    /// Whether the start-up of the app is over: jobs wait for it.
    open: watch::Sender<bool>,
    /// Told whenever a job has ended and given up its place.
    ended: Notify,
}

impl Registry {
    /// A registry that holds jobs back until [`Registry::open`]: the app first clears away what a job that was cut short
    /// left in the folders, and reads which keys are saved, and a job that ran before would meet either half done.
    pub fn new(
        jobs: Arc<Jobs>,
        backend: Arc<dyn MeetingBackend>,
        events: Arc<dyn Events>,
        notifier: Arc<dyn Notifier>,
        now: Clock,
    ) -> Registry {
        Registry {
            inner: Arc::new(Inner {
                backend,
                local: LocalLock::new(),
                jobs,
                events,
                notifier,
                now,
                views: Mutex::new(HashMap::new()),
                open: watch::channel(false).0,
                ended: Notify::new(),
            }),
        }
    }

    /// The start-up is over: the jobs that are waiting go on.
    pub fn open(&self) {
        self.inner.open.send_replace(true);
    }

    /// Starts processing the recording `spec` names, from `from`. `AlreadyRunning` when it already has a job.
    pub fn start(&self, spec: JobSpec, from: StartAt) -> Result<(), AlreadyRunning> {
        let inner = &self.inner;
        let guard = inner.jobs.start(Job::Meeting(spec.id.clone()))?;
        let view = JobView {
            id: spec.id.clone(),
            step: None,
            progress: None,
            step_started_ms: (inner.now)(),
            outcome: None,
        };
        {
            // Told while the table is locked, like the end of a job, so that the events of a recording come in the order in
            // which the table changed.
            let mut views = lock(&inner.views);
            views.insert(spec.id.clone(), view.clone());
            inner.events.job(&view);
            inner.events.changed();
        }

        // A second task watches the first: a job that panics would otherwise stay "running" in the window for ever.
        let (watcher, id_and_folder) = (self.inner.clone(), spec.clone());
        let inner = self.inner.clone();
        let job = tauri::async_runtime::spawn(async move { inner.run(guard, spec, from).await });
        tauri::async_runtime::spawn(async move {
            if job.await.is_err() {
                watcher.crashed(&id_and_folder);
            }
        });
        Ok(())
    }

    /// Asks the job of the recording to stop. Whether there was one.
    pub fn cancel(&self, id: &str) -> bool {
        self.inner.jobs.cancel(&Job::Meeting(id.to_owned()))
    }

    /// The job of the recording that is running, or how the last one ended; `None` when none ran since the app started.
    pub fn status(&self, id: &str) -> Option<JobView> {
        lock(&self.inner.views).get(id).cloned()
    }

    /// Whether the recording has a job that is running.
    pub fn is_running(&self, id: &str) -> bool {
        self.inner.jobs.is_active(&Job::Meeting(id.to_owned()))
    }

    /// Forgets what is kept of the recording (it was deleted).
    pub fn forget(&self, id: &str) {
        lock(&self.inner.views).remove(id);
    }

    /// Waits until the recording has no job, for at most `limit`. Whether it has none.
    pub async fn wait_idle(&self, id: &str, limit: Duration) -> bool {
        let job = Job::Meeting(id.to_owned());
        let idle = async {
            loop {
                // Asked to be told before looking, so that a job that ends in between is not missed.
                let ended = self.inner.ended.notified();
                tokio::pin!(ended);
                ended.as_mut().enable();
                if !self.inner.jobs.is_active(&job) {
                    return;
                }
                ended.await;
            }
        };
        tokio::time::timeout(limit, idle).await.is_ok()
    }
}

impl Inner {
    /// One job, from the wait for the start-up to its end.
    async fn run(self: Arc<Self>, guard: JobGuard, spec: JobSpec, from: StartAt) {
        let token = guard.token();
        let outcome = if self.start_up_is_over(&token).await {
            let emit = |event: JobEvent| self.hear(&spec.id, event);
            pipeline::run(&*self.backend, &self.local, &spec, from, &token, &emit).await
        } else {
            Outcome::Failed(MeetingError::cancelled())
        };
        self.finish(guard, &spec, outcome);
    }

    /// Waits for `Registry::open`. `false` when the job was cancelled first.
    async fn start_up_is_over(&self, cancel: &CancellationToken) -> bool {
        let mut open = self.open.subscribe();
        tokio::select! {
            biased;
            () = cancel.cancelled() => false,
            opened = open.wait_for(|open| *open) => opened.is_ok(),
        }
    }

    /// What the pipeline says while a job runs. Its last word, `Finished`, is not heard here: `finish` has the outcome
    /// that `run` returns, which is the same, and also for the run that is refused and says nothing.
    fn hear(&self, id: &str, event: JobEvent) {
        match event {
            JobEvent::Step { step, progress } => self.step(id, step.into(), progress),
            JobEvent::TitleChanged(_) => self.events.changed(),
            JobEvent::Log(_) | JobEvent::Finished(_) => {}
        }
    }

    /// The job is in `step`, `progress` of the way. It is told to the window when the step is another, or the progress
    /// has gone on by a whole percent: a step that reports with every block would make a thousand events a second.
    fn step(&self, id: &str, step: StepView, progress: Option<f32>) {
        let view = {
            let mut views = lock(&self.views);
            let Some(view) = views.get_mut(id).filter(|view| view.outcome.is_none()) else {
                return;
            };
            let another_step = view.step != Some(step);
            if !another_step && percent(progress) == percent(view.progress) {
                return;
            }
            if another_step {
                view.step_started_ms = (self.now)();
            }
            view.step = Some(step);
            view.progress = progress;
            view.clone()
        };
        self.events.job(&view);
    }

    /// The job is over: its outcome is kept, the recording is free to have another job, and then the window and the person
    /// are told. The recording is freed first because "Try again" comes straight after the event that shows the error.
    fn finish(self: &Arc<Self>, guard: JobGuard, spec: &JobSpec, outcome: Outcome) {
        let view = self.keep(spec, &outcome);
        drop(guard);
        self.announce(spec, &view, outcome);
    }

    /// The task of the job panicked (its place was given up as it unwound): the job failed, and the person is told as for any
    /// other failure.
    fn crashed(self: &Arc<Self>, spec: &JobSpec) {
        let outcome = Outcome::Failed(MeetingError::other("The job stopped unexpectedly."));
        let view = self.keep(spec, &outcome);
        self.announce(spec, &view, outcome);
    }

    /// Remembers how the job ended, as the recording's last outcome.
    fn keep(&self, spec: &JobSpec, outcome: &Outcome) -> JobView {
        let view = JobView {
            id: spec.id.clone(),
            step: None,
            progress: None,
            step_started_ms: (self.now)(),
            outcome: Some(OutcomeView::from(outcome)),
        };
        lock(&self.views).insert(spec.id.clone(), view.clone());
        view
    }

    /// Tells whoever waits for the job, the window and the person that it ended.
    fn announce(self: &Arc<Self>, spec: &JobSpec, view: &JobView, outcome: Outcome) {
        self.ended.notify_waiters();
        {
            // The recording was free from the moment `finish` gave up its place, and the window may have started another job
            // since: its start was told, and an end that comes after would look like the end of that one. The table is locked
            // while this is told, as `Registry::start` locks it while it tells a start: either the start is told first and this
            // is not, or this is told first.
            let views = lock(&self.views);
            if views.get(&spec.id) == Some(view) {
                self.events.job(view);
            }
        }
        self.events.changed();

        // A job that was cancelled is what the person asked for.
        if matches!(&outcome, Outcome::Failed(error) if error.kind == ErrorKind::Cancelled) {
            return;
        }
        // Reading the summary for what it found is disk work.
        let (notifier, spec) = (self.notifier.clone(), spec.clone());
        tauri::async_runtime::spawn_blocking(move || notifier.notify(notice_for(&spec, &outcome)));
    }
}

/// How far a job is, in whole percents.
fn percent(progress: Option<f32>) -> Option<i32> {
    progress.map(|fraction| (fraction.clamp(0.0, 1.0) * 100.0).floor() as i32)
}

/// The banner for a job that ended: the recording's title as it is now (the summary may have named it) and, for a summary,
/// what it found.
fn notice_for(spec: &JobSpec, outcome: &Outcome) -> Notice {
    let title = Meta::read(&spec.folder)
        .map(|meta| meta.title)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| spec.id.clone());
    let kind = match outcome {
        Outcome::Done { summary: true } => {
            let markdown =
                std::fs::read_to_string(spec.folder.join(files::SUMMARY)).unwrap_or_default();
            let doc = SummaryDoc::parse(&markdown);
            NoticeKind::SummaryReady {
                decisions: doc.decisions().len(),
                actions: doc.action_items().len(),
            }
        }
        Outcome::Done { summary: false } => NoticeKind::TranscriptOnly,
        Outcome::Failed(error) => NoticeKind::Failed(error.clone()),
    };
    Notice {
        id: spec.id.clone(),
        title,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    use dictate_core::meetings::library::SCREEN_SOURCE;
    use dictate_services::meetings::fakes::{gate, FakeBackend, GateControl};
    use serde_json::json;

    use super::*;
    use crate::meetings::test_fixture::Fixture;
    use crate::meetings::views::ErrorView;

    /// How long a test waits for something that happens by itself, before it says it never will. Nothing waits this long
    /// when it works.
    const PATIENCE: Duration = Duration::from_secs(30);

    #[derive(Debug, Clone, PartialEq)]
    enum Seen {
        Job(JobView),
        Changed,
    }

    /// Events and banners, written down and handed to the test as they come.
    struct Heard {
        seen: Mutex<Vec<Seen>>,
        events: Mutex<mpsc::Sender<Seen>>,
        notices: Mutex<mpsc::Sender<Notice>>,
        /// What the window does at once when it hears a job has ended.
        on_end: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
        /// A bug in whoever hears a step: the job's task panics.
        panic_on_step: std::sync::atomic::AtomicBool,
    }

    impl Events for Heard {
        fn job(&self, view: &JobView) {
            if view.step.is_some() && self.panic_on_step.load(Ordering::SeqCst) {
                panic!("a bug while hearing a step");
            }
            if view.outcome.is_some() {
                // Taken out first: what it does may start another job, whose end would come here too. It is done before the
                // test hears of the event, so that the test finds what it did.
                let react = lock(&self.on_end).take();
                if let Some(react) = react {
                    react();
                }
            }
            self.hear(Seen::Job(view.clone()));
        }
        fn changed(&self) {
            self.hear(Seen::Changed);
        }
    }

    impl Heard {
        fn hear(&self, seen: Seen) {
            lock(&self.seen).push(seen.clone());
            let _ = lock(&self.events).send(seen);
        }
    }

    impl Notifier for Heard {
        fn notify(&self, notice: Notice) {
            let _ = lock(&self.notices).send(notice);
        }
    }

    struct Rig {
        registry: Registry,
        jobs: Arc<Jobs>,
        backend: Arc<FakeBackend>,
        heard: Arc<Heard>,
        events: mpsc::Receiver<Seen>,
        notices: mpsc::Receiver<Notice>,
        /// The ends of jobs that `ended` met on its way to another one.
        skipped: Mutex<Vec<JobView>>,
        library: Fixture,
    }

    impl Rig {
        /// A registry (closed, until `open`) on a backend of `backend`.
        fn closed(backend: FakeBackend) -> Rig {
            let (events_tx, events) = mpsc::channel();
            let (notices_tx, notices) = mpsc::channel();
            let heard = Arc::new(Heard {
                seen: Mutex::new(Vec::new()),
                events: Mutex::new(events_tx),
                notices: Mutex::new(notices_tx),
                on_end: Mutex::new(None),
                panic_on_step: std::sync::atomic::AtomicBool::new(false),
            });
            let backend = Arc::new(backend);
            let jobs = Arc::new(Jobs::default());
            // The time of the registry moves by one every time it is asked.
            let ticks = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
            let registry = Registry::new(
                jobs.clone(),
                backend.clone(),
                heard.clone(),
                heard.clone(),
                Box::new(move || ticks.fetch_add(1, Ordering::SeqCst)),
            );
            Rig {
                registry,
                jobs,
                backend,
                heard,
                events,
                notices,
                skipped: Mutex::new(Vec::new()),
                library: Fixture::empty(),
            }
        }

        fn new(backend: FakeBackend) -> Rig {
            let rig = Rig::closed(backend);
            rig.registry.open();
            rig
        }

        /// An imported file called `id`'s title, with a media file there, as an import leaves it.
        fn recording(&self, id: &str, title: &str) -> JobSpec {
            let media = self.library.outside().join(format!("{id}.m4a"));
            std::fs::write(&media, b"audio").unwrap();
            self.library.write(
                id,
                Some(json!({
                    "title": title,
                    "source": media.to_string_lossy(),
                    "media": media.to_string_lossy(),
                    "created": 1_792_423_258.0,
                })),
                &[],
            );
            JobSpec {
                id: id.into(),
                folder: self.library.folder(id),
                link: None,
            }
        }

        /// Waits for the next event that `pick` takes.
        fn wait<T>(&self, what: &str, mut pick: impl FnMut(Seen) -> Option<T>) -> T {
            loop {
                let seen = self
                    .events
                    .recv_timeout(PATIENCE)
                    .unwrap_or_else(|_| panic!("no {what}"));
                if let Some(found) = pick(seen) {
                    return found;
                }
            }
        }

        /// Waits for the event that says the job of `id` has ended, and gives its view. The ends of other recordings' jobs that
        /// come first are kept for a later call, not thrown away: jobs that run at once end in any order.
        fn ended(&self, id: &str) -> JobView {
            loop {
                {
                    let mut kept = lock(&self.skipped);
                    if let Some(at) = kept.iter().position(|view| view.id == id) {
                        return kept.remove(at);
                    }
                }
                let view = self.wait("end of a job", |seen| match seen {
                    Seen::Job(view) if view.outcome.is_some() => Some(view),
                    _ => None,
                });
                if view.id == id {
                    return view;
                }
                lock(&self.skipped).push(view);
            }
        }

        /// Waits for the list to be told that the job is over: `announce` tells the end of a job, and then the list, so a test that
        /// has just heard the end has not heard the list yet. It takes the next change of the list from the channel and throws
        /// away the job events on the way, so it only suits a test with one job (after `ended`, which has read up to the end).
        fn list_told_of_the_end(&self) {
            self.wait("the list to be told of the end", |seen| {
                (seen == Seen::Changed).then_some(())
            });
        }

        /// Everything heard so far.
        fn all(&self) -> Vec<Seen> {
            lock(&self.heard.seen).clone()
        }

        fn job_views(&self) -> Vec<JobView> {
            self.all()
                .into_iter()
                .filter_map(|seen| match seen {
                    Seen::Job(view) => Some(view),
                    Seen::Changed => None,
                })
                .collect()
        }
    }

    /// A backend whose call to `prepare_audio` is held until the test lets it go.
    fn held_at_audio() -> (FakeBackend, GateControl) {
        let (held, control) = gate();
        let mut backend = FakeBackend::new();
        backend.gates.prepare = Some(held);
        (backend, control)
    }

    /// Why the job of `view` failed.
    fn error_of(view: &JobView) -> ErrorView {
        match &view.outcome {
            Some(OutcomeView::Failed { error }) => error.clone(),
            other => panic!("not a failure: {other:?}"),
        }
    }

    #[test]
    fn a_job_tells_each_step_and_then_how_it_ended() {
        let rig = Rig::new(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        let last = rig.ended(&spec.id);

        assert_eq!(last.outcome, Some(OutcomeView::Done { summary: true }));
        assert_eq!((last.step, last.progress), (None, None));
        let steps: Vec<_> = rig
            .job_views()
            .iter()
            .filter(|view| view.outcome.is_none())
            .filter_map(|view| view.step)
            .collect();
        // Audio, transcript and summary, each starting with no figure; the progress of one repeats its step.
        assert_eq!(
            steps.first(),
            Some(&StepView::Audio),
            "the first step is the audio: {steps:?}"
        );
        assert!(steps.contains(&StepView::Transcript) && steps.contains(&StepView::Summary));
        let mut in_order = steps.clone();
        in_order.dedup();
        assert_eq!(
            in_order,
            [StepView::Audio, StepView::Transcript, StepView::Summary]
        );
        assert_eq!(rig.registry.status(&spec.id), Some(last));
        assert_eq!(
            rig.backend.calls().first().map(String::as_str),
            Some("prepare_audio")
        );
    }

    #[test]
    fn the_list_is_told_when_a_job_starts_and_when_it_ends() {
        let rig = Rig::new(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        rig.ended(&spec.id);
        rig.list_told_of_the_end();

        let all = rig.all();
        assert_eq!(
            all.first(),
            Some(&Seen::Job(JobView {
                id: spec.id.clone(),
                step: None,
                progress: None,
                step_started_ms: 1_000,
                outcome: None,
            })),
            "a job that is starting is told first, with no step"
        );
        assert_eq!(
            all.get(1),
            Some(&Seen::Changed),
            "and the list is told as it starts"
        );
        // The last two: the end of the job, then the list.
        let n = all.len();
        assert!(matches!(&all[n - 2], Seen::Job(view) if view.outcome.is_some()));
        assert_eq!(all[n - 1], Seen::Changed);
    }

    #[test]
    fn the_time_a_step_began_changes_with_the_step_and_not_with_its_progress() {
        let mut backend = FakeBackend::new();
        backend.local_progress = vec![0.25, 0.5, 0.75];
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        rig.ended(&spec.id);

        let views = rig.job_views();
        let started = |step: StepView| -> Vec<u64> {
            let mut times: Vec<u64> = views
                .iter()
                .filter(|view| view.step == Some(step))
                .map(|view| view.step_started_ms)
                .collect();
            times.dedup();
            times
        };
        // One start for the transcript, though it reported four times (none, and three figures).
        assert_eq!(started(StepView::Transcript).len(), 1, "{views:?}");
        let progress: Vec<_> = views
            .iter()
            .filter(|view| view.step == Some(StepView::Transcript))
            .map(|view| view.progress)
            .collect();
        assert_eq!(progress.len(), 5, "{progress:?}");
        assert_eq!(progress[1..4], [Some(0.25), Some(0.5), Some(0.75)]);
        // And it is later than the step before it.
        assert!(started(StepView::Transcript)[0] > started(StepView::Audio)[0]);
    }

    #[test]
    fn progress_is_told_once_for_each_whole_percent() {
        let mut backend = FakeBackend::new();
        backend.local_progress = vec![0.101, 0.102, 0.1049, 0.11, 0.5, 0.504];
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        rig.ended(&spec.id);

        let told: Vec<f32> = rig
            .job_views()
            .iter()
            .filter(|view| view.step == Some(StepView::Transcript))
            .filter_map(|view| view.progress)
            .collect();
        // 0.101, 0.11, 0.5 and the 1.0 that ends the step: the others are the same percent as the one before them.
        assert_eq!(told, [0.101, 0.11, 0.5, 1.0]);
    }

    #[test]
    fn a_recording_has_one_job_and_the_second_start_is_refused() {
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        assert_eq!(
            rig.registry.start(spec.clone(), StartAt::Summary),
            Err(AlreadyRunning)
        );
        assert!(rig.registry.is_running(&spec.id));
        // The one that is running is not disturbed.
        let running = rig.registry.status(&spec.id).unwrap();
        assert_eq!(running.outcome, None);

        control.release.send(()).unwrap();
        rig.ended(&spec.id);
        assert!(!rig.registry.is_running(&spec.id));
        assert_eq!(rig.registry.start(spec, StartAt::Audio), Ok(()));
    }

    #[test]
    fn other_recordings_are_not_held_up_by_one_that_is_running() {
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let (held, free) = (
            rig.recording("2026-11-09 10-00-00", "Held"),
            rig.recording("2026-11-09 10-00-01", "Free"),
        );
        rig.registry.start(held.clone(), StartAt::Audio).unwrap();
        control.started.blocking_recv().unwrap();

        // The gate holds one call: the other recording's job runs through while the first one is still held.
        rig.registry.start(free.clone(), StartAt::Audio).unwrap();
        let last = rig.ended(&free.id);

        assert_eq!(last.outcome, Some(OutcomeView::Done { summary: true }));
        assert!(rig.registry.is_running(&held.id));
        control.release.send(()).unwrap();
        rig.ended(&held.id);
    }

    #[test]
    fn a_cancelled_job_ends_as_cancelled_and_leaves_the_folder_as_it_was() {
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        control.started.blocking_recv().unwrap();

        assert!(rig.registry.cancel(&spec.id));
        let last = rig.ended(&spec.id);

        assert_eq!(error_of(&last).kind, "Cancelled");
        assert!(
            !control.finished.load(Ordering::SeqCst),
            "the call was dropped, not let through"
        );
        assert!(!spec.folder.join("transcript.txt").exists());
        // There is nothing to cancel any more, and never was for a recording that had no job.
        assert!(!rig.registry.cancel(&spec.id));
        assert!(!rig.registry.cancel("no such recording"));
    }

    #[test]
    fn the_last_outcome_stays_until_the_next_job_starts() {
        let mut backend = FakeBackend::new();
        backend.summary = Err("API error 401".into());
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        assert_eq!(rig.registry.status(&spec.id), None);
        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        let last = rig.ended(&spec.id);
        assert!(matches!(last.outcome, Some(OutcomeView::Failed { .. })));
        assert_eq!(rig.registry.status(&spec.id), Some(last));
        assert_eq!(rig.registry.status("another"), None);

        // A new job replaces it at once.
        let (backend, _control) = held_at_audio();
        let again = Rig::new(backend);
        let spec = again.recording("2026-11-09 10-00-00", "Weekly");
        again.registry.start(spec.clone(), StartAt::Audio).unwrap();
        assert_eq!(
            again.registry.status(&spec.id).map(|view| view.outcome),
            Some(None)
        );

        rig.registry.forget(&spec.id);
        assert_eq!(rig.registry.status(&spec.id), None);
    }

    #[test]
    fn the_recording_is_free_by_the_time_its_end_is_told() {
        let mut backend = FakeBackend::new();
        backend.summary = Err("API error 401".into());
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        // "Try again" is the window's answer to the event that showed the error. Whoever hears that the job ended must find the
        // recording free, or the answer meets "already running".
        let (registry, id, answer) = (
            rig.registry.clone(),
            spec.id.clone(),
            Arc::new(Mutex::new(None)),
        );
        let told = answer.clone();
        *lock(&rig.heard.on_end) = Some(Box::new(move || {
            *lock(&told) = Some(registry.is_running(&id));
        }));

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        rig.ended(&spec.id);

        assert_eq!(*lock(&answer), Some(false), "the recording was not free");
        // And so the start that follows the end works.
        assert_eq!(rig.registry.start(spec.clone(), StartAt::Summary), Ok(()));
        rig.ended(&spec.id);
    }

    #[test]
    fn the_end_of_a_job_is_not_told_after_a_newer_job_of_the_recording_started() {
        // The recording is free as soon as its job has ended, and the window may start another one before the end of the
        // first is told: its start is told first, and the end that comes after would look like the end of the new job.
        let rig = Rig::new(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        let inner = rig.registry.inner.clone();
        let done = Outcome::Done { summary: true };
        let stale = inner.keep(&spec, &done);
        let newer = JobView {
            id: spec.id.clone(),
            step: Some(StepView::Audio),
            progress: None,
            step_started_ms: 5_000,
            outcome: None,
        };
        lock(&inner.views).insert(spec.id.clone(), newer.clone());
        rig.heard.job(&newer);

        inner.announce(&spec, &stale, done.clone());

        assert_eq!(
            rig.job_views(),
            std::slice::from_ref(&newer),
            "the stale end was told"
        );
        assert_eq!(rig.registry.status(&spec.id), Some(newer));
        // The list is still told: it reloads whatever it holds.
        assert_eq!(rig.all().last(), Some(&Seen::Changed));

        // The end of the job that is the latest is told.
        let latest = inner.keep(&spec, &done);
        inner.announce(&spec, &latest, done);
        assert_eq!(rig.job_views().last(), Some(&latest));
    }

    #[test]
    fn a_job_that_panics_still_ends_and_frees_the_recording() {
        let rig = Rig::new(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        rig.heard.panic_on_step.store(true, Ordering::SeqCst);

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        let last = rig.ended(&spec.id);

        // The row does not stay "running" for ever: it fails, with a reason, and the recording is free again.
        let error = error_of(&last);
        assert_eq!(error.kind, "Other");
        assert_eq!(
            error.detail.as_deref(),
            Some("The job stopped unexpectedly.")
        );
        assert_eq!(rig.registry.status(&spec.id), Some(last));
        assert!(!rig.registry.is_running(&spec.id));
        rig.heard.panic_on_step.store(false, Ordering::SeqCst);
        assert_eq!(rig.registry.start(spec.clone(), StartAt::Audio), Ok(()));
        assert_eq!(
            rig.ended(&spec.id).outcome,
            Some(OutcomeView::Done { summary: true })
        );
        // A banner says so, as for any failure.
        assert!(matches!(notice(&rig).kind, NoticeKind::Failed(_)));
    }

    #[test]
    fn a_title_that_the_summary_gives_tells_the_list() {
        let rig = Rig::new(FakeBackend::new());
        // Named after its file: the summary names it.
        let spec = rig.recording("2026-11-09 10-00-00", "2026-11-09 10-00-00");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        rig.ended(&spec.id);
        rig.list_told_of_the_end();

        assert_eq!(Meta::read(&spec.folder).unwrap().title, "Weekly sync");
        let changes = rig
            .all()
            .iter()
            .filter(|seen| **seen == Seen::Changed)
            .count();
        // The start, the new title and the end.
        assert_eq!(changes, 3);
    }

    #[test]
    fn a_run_that_the_pipeline_refuses_still_ends_the_job() {
        // The pipeline lets one run work in a folder, whatever the registries above it say.
        let (backend, control) = held_at_audio();
        let first = Rig::new(backend);
        let spec = first.recording("2026-11-09 10-00-00", "Weekly");
        first.registry.start(spec.clone(), StartAt::Audio).unwrap();
        control.started.blocking_recv().unwrap();

        let second = Rig::new(FakeBackend::new());
        second.registry.start(spec.clone(), StartAt::Audio).unwrap();
        let refused = second.ended(&spec.id);
        assert_eq!(
            error_of(&refused).detail.as_deref(),
            Some("This recording is already being processed.")
        );
        assert!(!second.registry.is_running(&spec.id));
        assert!(second.backend.calls().is_empty());

        // The run that was working goes on.
        assert!(first.registry.is_running(&spec.id));
        control.release.send(()).unwrap();
        first.ended(&spec.id);
    }

    #[test]
    fn jobs_wait_for_the_start_up_of_the_app() {
        let rig = Rig::closed(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        // It is there and running, waiting. (There is nothing to hear that tells it has not gone on: this gives it time to.)
        assert!(rig.registry.is_running(&spec.id));
        std::thread::sleep(Duration::from_millis(100));
        assert!(rig.backend.calls().is_empty(), "{:?}", rig.backend.calls());
        assert_eq!(rig.registry.status(&spec.id).unwrap().outcome, None);

        rig.registry.open();
        let last = rig.ended(&spec.id);
        assert_eq!(last.outcome, Some(OutcomeView::Done { summary: true }));
    }

    #[test]
    fn a_job_cancelled_while_it_waits_for_the_start_up_never_runs() {
        let rig = Rig::closed(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();

        assert!(rig.registry.cancel(&spec.id));
        let last = rig.ended(&spec.id);

        assert_eq!(error_of(&last).kind, "Cancelled");
        assert!(rig.backend.calls().is_empty());
        assert!(!rig.registry.is_running(&spec.id));
    }

    #[test]
    fn quitting_cancels_the_jobs_and_waits_for_them() {
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        control.started.blocking_recv().unwrap();

        // What `RunEvent::Exit` does.
        assert!(rig.jobs.cancel_all_and_wait(PATIENCE));

        assert!(!rig.registry.is_running(&spec.id));
        let last = rig.registry.status(&spec.id).unwrap();
        assert_eq!(error_of(&last).kind, "Cancelled");
    }

    #[tokio::test]
    async fn waiting_for_a_job_that_was_cancelled_ends_with_the_job() {
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        // No job: nothing to wait for.
        assert!(
            rig.registry
                .wait_idle(&spec.id, Duration::from_millis(10))
                .await
        );

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();
        // A job that does not end in time is not idle.
        assert!(
            !rig.registry
                .wait_idle(&spec.id, Duration::from_millis(20))
                .await
        );

        rig.registry.cancel(&spec.id);
        assert!(rig.registry.wait_idle(&spec.id, PATIENCE).await);
        assert!(!rig.registry.is_running(&spec.id));
        drop(control);
    }

    // ---- the banners

    fn notice(rig: &Rig) -> Notice {
        rig.notices.recv_timeout(PATIENCE).expect("a banner")
    }

    #[test]
    fn a_summary_that_is_done_is_a_banner_with_its_title_and_what_it_found() {
        let mut backend = FakeBackend::new();
        backend.summary = Ok(
            "# Weekly sync\n\n## Quyết định\n- Ship it\n- Wait\n\n## Việc cần làm\n- [ ] **An** — Write it\n".into(),
        );
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "2026-11-09 10-00-00");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();

        let n = notice(&rig);
        assert_eq!(n.id, spec.id);
        // The title the summary gave, not the one the file had.
        assert_eq!(n.title, "Weekly sync");
        assert_eq!(
            n.kind,
            NoticeKind::SummaryReady {
                decisions: 2,
                actions: 1
            }
        );
    }

    #[test]
    fn a_transcript_without_an_ai_is_a_banner_that_says_so() {
        let mut backend = FakeBackend::new();
        backend.ai_ready = false;
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();

        assert_eq!(notice(&rig).kind, NoticeKind::TranscriptOnly);
    }

    #[test]
    fn a_failure_is_a_banner_with_the_error() {
        let mut backend = FakeBackend::new();
        backend.summary = Err("API error 401".into());
        let rig = Rig::new(backend);
        let spec = rig.recording("2026-11-09 10-00-00", "Weekly");
        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();

        match notice(&rig).kind {
            NoticeKind::Failed(error) => {
                assert_eq!(error.kind, ErrorKind::Api);
                assert_eq!(error.key, "meetings.error.summary_failed");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_job_that_was_cancelled_is_no_banner() {
        // Cancelling is what the person asked for. A banner would be sent before the end of the next job: the first job's is
        // held at the audio, and a gate holds one call only, so the second one runs through.
        let (backend, control) = held_at_audio();
        let rig = Rig::new(backend);
        let (cancelled, next) = (
            rig.recording("2026-11-09 10-00-00", "Cancelled"),
            rig.recording("2026-11-09 10-00-01", "Next"),
        );
        rig.registry
            .start(cancelled.clone(), StartAt::Audio)
            .unwrap();
        control.started.blocking_recv().unwrap();
        rig.registry.cancel(&cancelled.id);
        rig.ended(&cancelled.id);

        rig.registry.start(next.clone(), StartAt::Audio).unwrap();
        rig.ended(&next.id);

        assert_eq!(
            notice(&rig).id,
            next.id,
            "the first banner is the second job's"
        );
        assert!(rig.notices.try_recv().is_err());
    }

    #[test]
    fn a_screen_recording_keeps_its_own_title_in_the_banner() {
        // Screen recordings are named by the app, and the summary names them: the banner has what the folder says last.
        let rig = Rig::new(FakeBackend::new());
        let spec = rig.recording("2026-11-09 10-00-00", "Ghi màn hình 10:00");
        let mut meta = Meta::read(&spec.folder).unwrap();
        meta.source = SCREEN_SOURCE.into();
        meta.write(&spec.folder).unwrap();

        rig.registry.start(spec.clone(), StartAt::Audio).unwrap();

        assert_eq!(notice(&rig).title, "Weekly sync");
    }
}
