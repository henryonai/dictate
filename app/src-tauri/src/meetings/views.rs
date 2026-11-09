//! What the meetings window is told about the library: the rows of the list, the detail of one recording, and the
//! state of the jobs that work on them. Everything is `Serialize` with camelCase names; the UI mirrors these types in
//! `app/ui/src/lib/api.ts`.
//!
//! The pipeline's own types (`Step`, `Outcome`, `MeetingError`) have no `Serialize`: they are mapped here.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use dictate_core::meetings::library::{files, Kind, MeetingRecord};
use dictate_core::meetings::speakers::speakers_in;
use dictate_core::meetings::summary::{
    ACTION_NAMES, DECISION_NAMES, KEY_POINT_NAMES, QUESTION_NAMES, SUMMARY_NAMES, TIMELINE_NAMES,
};
use dictate_core::meetings::{SummaryDoc, TranscriptLine};
use dictate_core::text::search::TextSearch;
use dictate_services::meetings::{startup_status, ErrorKind, MeetingError, Outcome, Step};
use serde::Serialize;

/// The step a job is in. Sent as `"download"`, `"audio"`, `"transcript"` or `"summary"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepView {
    Download,
    Audio,
    Transcript,
    Summary,
}

impl From<Step> for StepView {
    fn from(step: Step) -> Self {
        match step {
            Step::Download => StepView::Download,
            Step::Audio => StepView::Audio,
            Step::Transcript => StepView::Transcript,
            Step::Summary => StepView::Summary,
        }
    }
}

/// How a job ended: `{"kind": "done", "summary": true}` (the transcript is written, and the summary too when
/// `summary`) or `{"kind": "failed", "error": {...}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutcomeView {
    Done { summary: bool },
    Failed { error: ErrorView },
}

impl From<&Outcome> for OutcomeView {
    fn from(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::Done { summary } => OutcomeView::Done { summary: *summary },
            Outcome::Failed(error) => OutcomeView::Failed {
                error: ErrorView::from(error),
            },
        }
    }
}

/// A job as the window sees it: the `meeting-job` event carries one of these, and a row has one while its job runs.
///
/// A running job has `outcome: None`; `step` is `None` while the job is only starting or waiting. A job that has ended
/// has `step: None` and its `outcome`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    /// The recording's id (its folder name).
    pub id: String,
    pub step: Option<StepView>,
    /// 0 to 1; `None` is "working, no figure yet".
    pub progress: Option<f32>,
    /// When the step began, in milliseconds since the Unix epoch: the window counts the time it has run from it.
    pub step_started_ms: u64,
    pub outcome: Option<OutcomeView>,
}

/// Why a job failed: `key` is the translated text (`meetings.error.*`), `detail` what is behind it (in English, as the
/// program that failed said it) and `command` what installs a missing tool, to copy.
///
/// `kind` is the name of the pipeline's `ErrorKind`, as is: `"Api"`, `"MissingTool"`, `"NoModel"`, `"NoAudio"`,
/// `"NoMedia"`, `"Unsupported"`, `"Empty"`, `"Unfinished"`, `"Cancelled"` or `"Other"`. The error card picks its
/// buttons by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorView {
    pub kind: String,
    pub key: String,
    pub detail: Option<String>,
    pub command: Option<String>,
}

impl From<&MeetingError> for ErrorView {
    fn from(error: &MeetingError) -> Self {
        let (kind, command) = match &error.kind {
            ErrorKind::Api => ("Api", None),
            ErrorKind::MissingTool { command } => ("MissingTool", Some(command.clone())),
            ErrorKind::NoModel => ("NoModel", None),
            ErrorKind::NoAudio => ("NoAudio", None),
            ErrorKind::NoMedia => ("NoMedia", None),
            ErrorKind::Unsupported { command } => ("Unsupported", Some(command.clone())),
            ErrorKind::Empty => ("Empty", None),
            ErrorKind::Unfinished => ("Unfinished", None),
            ErrorKind::Cancelled => ("Cancelled", None),
            ErrorKind::Other => ("Other", None),
        };
        ErrorView {
            kind: kind.to_owned(),
            key: error.key.to_owned(),
            detail: error.detail.clone(),
            command,
        }
    }
}

impl From<MeetingError> for ErrorView {
    fn from(error: MeetingError) -> Self {
        ErrorView::from(&error)
    }
}

impl ErrorView {
    /// An error with no key of its own: the English `detail` is the whole message.
    pub fn other(detail: impl Into<String>) -> Self {
        MeetingError::other(detail).into()
    }
}

/// Where a recording came from. Sent as `"screen"`, `"file"` or `"link"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KindView {
    Screen,
    File,
    Link,
}

impl From<Kind> for KindView {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Screen => KindView::Screen,
            Kind::File => KindView::File,
            Kind::Link => KindView::Link,
        }
    }
}

/// One recording in the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRow {
    pub id: String,
    pub title: String,
    pub kind: KindView,
    /// Seconds since the Unix epoch; 0 when the recording has no date (it is listed last, as "Older").
    pub created: f64,
    /// Seconds, once the audio is known.
    pub duration: Option<f64>,
    pub has_summary: bool,
    /// The action items that are not ticked.
    pub open_tasks: usize,
    /// The job that is working on the recording now.
    pub live: Option<JobView>,
    /// Why the recording is not done: its last job failed (or was cancelled), or the app was closed before it was
    /// processed. A recording whose job is running has none.
    pub error: Option<ErrorView>,
}

/// One line of a transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    /// Seconds from the start of the recording.
    pub start: f64,
    /// `mm:ss` or `h:mm:ss`, as the transcript has it; empty for a line that has none.
    pub stamp: String,
    pub speaker: Option<String>,
    pub text: String,
}

/// A speaker of the transcript, with a line of theirs to listen to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerView {
    pub name: String,
    pub lines: usize,
    pub sample: LineView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    /// The place in the summary's list of action items: what `tasks.json` stores and `meeting_set_task` takes.
    pub index: usize,
    pub owner: Option<String>,
    pub text: String,
    pub done: bool,
}

/// A line of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkView {
    pub seconds: u32,
    pub label: String,
    pub text: String,
}

/// A section of the summary that the window has no place of its own for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionView {
    /// Empty for the text before the first heading.
    pub heading: String,
    pub body: String,
}

/// The summary, split into the sections the window shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryView {
    /// The whole of `summary.md`, as the model wrote it.
    pub markdown: String,
    pub title: Option<String>,
    /// The body of each section, `None` when there is none or it is empty.
    pub summary: Option<String>,
    pub key_points: Option<String>,
    pub decisions: Vec<String>,
    pub actions: Vec<ActionView>,
    pub questions: Vec<String>,
    pub timeline: Vec<MarkView>,
    pub other: Vec<SectionView>,
}

/// The media to play: the address the local server gives it, and whether it has a picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaView {
    pub url: String,
    pub video: bool,
}

/// A recording, whole.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    pub row: MeetingRow,
    /// `meta.json`'s `source`: the link, the path of the imported file, or the screen recording's own word.
    pub source: String,
    /// What the header names as the source: the host of a link (without `www.`) or the file name of an imported file.
    /// Empty for a screen recording, which the window names itself, in its own language.
    pub source_label: String,
    pub transcript: Vec<LineView>,
    /// The transcript is the rougher live captions of a screen recording, which a better one has not replaced.
    pub transcript_is_live: bool,
    /// Empty when the transcript is live captions: their speakers can't be renamed.
    pub speakers: Vec<SpeakerView>,
    pub summary: Option<SummaryView>,
    /// What to play: the recording's media file when it is there, else `audio.m4a` when that is.
    pub media: Option<MediaView>,
    /// The recording's media file is named in `meta.json` and is not there any more (moved or deleted). `audio.m4a` may
    /// still play, but the recording can't be transcribed again.
    pub media_missing: bool,
}

/// What a row says of its recording's job: the job that is running (`live`), or why the recording is not done (`error`).
pub type JobState = (Option<JobView>, Option<ErrorView>);

/// The job state of a recording for its row.
///
/// `status` is what the registry remembers of the recording: the job that is running, or how the last one ended.
/// Without it (nothing ran since the app started) the folder tells: neither a transcript nor a summary is "Not
/// finished processing".
pub fn state_of(status: Option<JobView>, record: &MeetingRecord) -> JobState {
    match status {
        Some(view) if view.outcome.is_none() => (Some(view), None),
        Some(JobView {
            outcome: Some(OutcomeView::Failed { error }),
            ..
        }) => (None, Some(error)),
        Some(_) => (None, None),
        None => (None, startup_status(record).map(ErrorView::from)),
    }
}

/// The row of a recording.
pub fn row(record: &MeetingRecord, live: Option<JobView>, error: Option<ErrorView>) -> MeetingRow {
    let summary = record.summary();
    let open_tasks = summary.as_deref().map_or(0, |markdown| {
        let done = record.done_tasks();
        let items = SummaryDoc::parse(markdown).action_items().len();
        (0..items).filter(|index| !done.contains(index)).count()
    });
    MeetingRow {
        id: record.id.clone(),
        title: record.title.clone(),
        kind: record.kind().into(),
        created: epoch_seconds(record.created),
        duration: record.duration,
        has_summary: summary.is_some(),
        open_tasks,
        live,
        error,
    }
}

/// Seconds since the Unix epoch; 0 for the oldest possible date, which is what the library gives a recording with no
/// date at all.
fn epoch_seconds(created: DateTime<Utc>) -> f64 {
    if created == DateTime::<Utc>::MIN_UTC {
        0.0
    } else {
        created.timestamp() as f64 + f64::from(created.timestamp_subsec_nanos()) / 1e9
    }
}

/// The recording's detail. `media_url` gives the address a file is played from.
pub fn detail(
    record: &MeetingRecord,
    live: Option<JobView>,
    error: Option<ErrorView>,
    media_url: &dyn Fn(&Path) -> String,
) -> MeetingDetail {
    let (text, is_live) = record.transcript().unwrap_or_default();
    let transcript = TranscriptLine::parse(&text).iter().map(line_view).collect();
    let speakers = if is_live {
        Vec::new()
    } else {
        speakers_in(&text)
            .into_iter()
            .map(|speaker| SpeakerView {
                name: speaker.name,
                lines: speaker.lines,
                sample: line_view(&speaker.sample),
            })
            .collect()
    };
    let summary = record
        .summary()
        .map(|markdown| summary_view(markdown, &record.done_tasks()));

    let original = record.media();
    let playable = original.clone().filter(|path| path.is_file());
    let audio = Some(record.folder.join(files::AUDIO)).filter(|path| path.is_file());
    let media = playable.or(audio).map(|path| MediaView {
        url: media_url(&path),
        video: is_video(&path),
    });

    MeetingDetail {
        row: row(record, live, error),
        source: record.source.clone(),
        source_label: source_label(record),
        transcript,
        transcript_is_live: is_live,
        speakers,
        summary,
        media,
        media_missing: original.is_some_and(|path| !path.is_file()),
    }
}

fn line_view(line: &TranscriptLine) -> LineView {
    LineView {
        start: line.start,
        stamp: line.stamp.clone(),
        speaker: line.speaker.clone(),
        text: line.text.clone(),
    }
}

/// Whether the file has a picture, by its extension.
fn is_video(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        matches!(extension.as_str(), "mov" | "mp4" | "m4v" | "webm")
    })
}

/// What the header says the recording came from: the host of a link without its `www.`, the name of the file of an
/// import. A screen recording gets none: the window has its own word for it, in its own language.
fn source_label(record: &MeetingRecord) -> String {
    let source = record.source.as_str();
    match record.kind() {
        Kind::Screen => String::new(),
        Kind::Link => reqwest::Url::parse(source)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .map(|host| {
                host.strip_prefix("www.")
                    .map_or(host.clone(), str::to_owned)
            })
            .unwrap_or_else(|| source.to_owned()),
        Kind::File => Path::new(source).file_name().map_or_else(
            || source.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}

fn summary_view(markdown: String, done: &BTreeSet<usize>) -> SummaryView {
    let doc = SummaryDoc::parse(&markdown);
    let body = |names: &[&str]| {
        doc.section(names)
            .map(|section| section.body.clone())
            .filter(|body| !body.trim().is_empty())
    };

    // The sections the window has a place for: the first one of each kind. Any other is shown as it is.
    let known: Vec<usize> = [
        SUMMARY_NAMES,
        KEY_POINT_NAMES,
        DECISION_NAMES,
        ACTION_NAMES,
        QUESTION_NAMES,
        TIMELINE_NAMES,
    ]
    .iter()
    .filter_map(|names| doc.section(names))
    .filter_map(|found| doc.sections.iter().position(|s| std::ptr::eq(s, found)))
    .collect();
    let other = doc
        .sections
        .iter()
        .enumerate()
        .filter(|(index, _)| !known.contains(index))
        .map(|(_, section)| SectionView {
            heading: section.heading.clone(),
            body: section.body.clone(),
        })
        .collect();

    SummaryView {
        title: doc.title.clone(),
        summary: body(SUMMARY_NAMES),
        key_points: body(KEY_POINT_NAMES),
        decisions: doc.decisions(),
        actions: doc
            .action_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| ActionView {
                index,
                owner: item.owner,
                text: item.text,
                done: done.contains(&index),
            })
            .collect(),
        questions: doc.questions(),
        timeline: doc
            .timeline()
            .into_iter()
            .map(|mark| MarkView {
                seconds: mark.seconds,
                label: mark.label,
                text: mark.text,
            })
            .collect(),
        other,
        markdown,
    }
}

/// The rows of `records` that match `query` (case and diacritics ignored: every word of it in the title, the summary or
/// the transcript), in the order given. Everything when the query is empty.
pub fn rows(
    records: Vec<MeetingRecord>,
    query: &str,
    state_of: &dyn Fn(&MeetingRecord) -> JobState,
) -> Vec<MeetingRow> {
    let search = TextSearch::new(query);
    records
        .iter()
        .filter(|record| search.is_empty() || mentions(&search, record))
        .map(|record| {
            let (live, error) = state_of(record);
            row(record, live, error)
        })
        .collect()
}

/// Whether the title, the summary or the transcript has every word of the search.
fn mentions(search: &TextSearch, record: &MeetingRecord) -> bool {
    search.matches(&record.title)
        || record
            .summary()
            .is_some_and(|summary| search.matches(&summary))
        || record
            .transcript()
            .is_some_and(|(transcript, _)| search.matches(&transcript))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::test_fixture::{Fixture, FILE, LINK, SCREEN, SUMMARY, TRANSCRIPT};
    use dictate_core::meetings::library::Meta;
    use serde_json::json;

    fn record(f: &Fixture, id: &str) -> MeetingRecord {
        MeetingRecord::from_folder(&f.folder(id)).expect("the recording")
    }

    fn no_state(_: &MeetingRecord) -> JobState {
        (None, None)
    }

    fn running(id: &str) -> JobView {
        JobView {
            id: id.into(),
            step: Some(StepView::Transcript),
            progress: Some(0.5),
            step_started_ms: 1_000,
            outcome: None,
        }
    }

    fn ended(id: &str, outcome: Outcome) -> JobView {
        JobView {
            id: id.into(),
            step: None,
            progress: None,
            step_started_ms: 2_000,
            outcome: Some(OutcomeView::from(&outcome)),
        }
    }

    // ---- the shapes the window reads

    #[test]
    fn job_views_are_sent_in_camel_case() {
        let view = JobView {
            id: "2026-10-19 22-20-58".into(),
            step: Some(StepView::Audio),
            progress: Some(0.25),
            step_started_ms: 1_700_000_000_123,
            outcome: None,
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            json!({
                "id": "2026-10-19 22-20-58",
                "step": "audio",
                "progress": 0.25,
                "stepStartedMs": 1_700_000_000_123_u64,
                "outcome": null,
            })
        );
        let done = ended("x", Outcome::Done { summary: true });
        assert_eq!(
            serde_json::to_value(&done).unwrap()["outcome"],
            json!({ "kind": "done", "summary": true })
        );
        let failed = ended("x", Outcome::Failed(MeetingError::no_audio()));
        assert_eq!(
            serde_json::to_value(&failed).unwrap()["outcome"],
            json!({
                "kind": "failed",
                "error": {
                    "kind": "NoAudio",
                    "key": "meetings.error.no_audio",
                    "detail": null,
                    "command": null,
                },
            })
        );
    }

    #[test]
    fn an_error_view_names_the_kind_and_carries_the_install_command_of_a_missing_tool() {
        let view = |error: MeetingError| ErrorView::from(&error);
        assert_eq!(view(MeetingError::cancelled()).kind, "Cancelled");
        assert_eq!(view(MeetingError::unfinished()).kind, "Unfinished");
        assert_eq!(view(MeetingError::no_model()).kind, "NoModel");
        assert_eq!(view(MeetingError::empty_transcript()).kind, "Empty");
        assert_eq!(view(MeetingError::summary_failed("401")).kind, "Api");
        let other = view(MeetingError::other("It broke"));
        assert_eq!(
            (
                other.kind.as_str(),
                other.key.as_str(),
                other.detail.as_deref(),
                other.command
            ),
            ("Other", "meetings.error.other", Some("It broke"), None)
        );

        let ytdlp = view(MeetingError::from_link(
            dictate_services::media::ytdlp::LinkError::Spawn("not found".into()),
        ));
        assert_eq!(ytdlp.kind, "MissingTool");
        assert_eq!(ytdlp.key, "meetings.error.ytdlp_missing");
        assert!(ytdlp
            .command
            .as_deref()
            .is_some_and(|c| c.contains("yt-dlp")));

        let ffmpeg = view(MeetingError::from_decode(
            dictate_services::media::decode::DecodeError::Unsupported {
                what: "ac3".into(),
                ffmpeg_hint: "brew install ffmpeg",
            },
        ));
        assert_eq!(ffmpeg.kind, "Unsupported");
        assert_eq!(ffmpeg.command.as_deref(), Some("brew install ffmpeg"));
        assert_eq!(ffmpeg.detail.as_deref(), Some("ac3"));
    }

    #[test]
    fn rows_are_sent_in_camel_case() {
        let f = Fixture::new();
        let row = row(&record(&f, SCREEN), None, None);
        let json = serde_json::to_value(&row).unwrap();
        for name in [
            "id",
            "title",
            "kind",
            "created",
            "duration",
            "hasSummary",
            "openTasks",
            "live",
            "error",
        ] {
            assert!(json.get(name).is_some(), "{name} in {json}");
        }
        assert_eq!(json["kind"], "screen");
        assert_eq!(json["hasSummary"], true);
    }

    // ---- the rows

    #[test]
    fn rows_group_data_and_open_tasks() {
        let f = Fixture::new();
        let all = rows(f.library().meetings(), "", &no_state);

        let ids: Vec<_> = all.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            [SCREEN, LINK, FILE],
            "the order of the library is kept"
        );

        // The screen recording: 2 action items, the first ticked.
        let screen = &all[0];
        assert_eq!(screen.title, "Họp kế hoạch refactor");
        assert_eq!(screen.kind, KindView::Screen);
        assert!(screen.has_summary);
        assert_eq!(screen.open_tasks, 1);
        assert_eq!(screen.duration, Some(373.55));
        assert!(
            (screen.created - 1_792_423_258.39).abs() < 1e-6,
            "{}",
            screen.created
        );

        // The link: no summary, so nothing to tick.
        let link = &all[1];
        assert_eq!(link.kind, KindView::Link);
        assert!(!link.has_summary);
        assert_eq!(link.open_tasks, 0);
        assert_eq!(link.duration, None);

        // The imported file: a summary with no action items.
        let file = &all[2];
        assert_eq!(file.kind, KindView::File);
        assert!(file.has_summary);
        assert_eq!(file.open_tasks, 0);
    }

    #[test]
    fn open_tasks_count_the_unticked_items_and_ignore_ticks_that_point_nowhere() {
        let f = Fixture::new();
        let tasks = |text: &str| {
            std::fs::write(f.folder(SCREEN).join(files::TASKS), text).unwrap();
            row(&record(&f, SCREEN), None, None).open_tasks
        };
        assert_eq!(tasks("[]"), 2);
        assert_eq!(tasks("[0]"), 1);
        assert_eq!(tasks("[0,1]"), 0);
        // A tick of a summary that was replaced: it points at no item, and takes none off.
        assert_eq!(tasks("[7]"), 2);
        assert_eq!(tasks("[1,7]"), 1);
    }

    #[test]
    fn a_recording_with_no_date_has_created_zero() {
        let f = Fixture::empty();
        f.write("scratch", Some(json!({})), &[]);
        let row = row(&record(&f, "scratch"), None, None);
        assert_eq!(row.created, 0.0);
        assert_eq!(row.title, "scratch");
    }

    #[test]
    fn list_search_is_diacritic_insensitive() {
        let f = Fixture::new();
        let ids = |query: &str| -> Vec<String> {
            rows(f.library().meetings(), query, &no_state)
                .into_iter()
                .map(|row| row.id)
                .collect()
        };
        // "hop" finds "Cuộc họp tuần", and "Họp kế hoạch refactor" too.
        assert_eq!(ids("hop"), [SCREEN, FILE]);
        assert_eq!(ids("  HỌP  "), [SCREEN, FILE]);
        // Every word has to be in the title, the summary or the transcript: "cuoc" is only in one title.
        assert_eq!(ids("cuoc hop"), [FILE]);
        // The words may be in the same text anywhere, and a text that has only one of them is not a match.
        assert_eq!(ids("hop tuan"), [SCREEN, FILE]);
        assert_eq!(ids("tuan swift"), Vec::<String>::new());
        // The summary and the transcript are searched too, not only the title.
        assert_eq!(ids("quyet dinh"), [SCREEN]);
        assert_eq!(ids("concurrency"), [LINK]);
        assert_eq!(ids("henry tao repo"), [SCREEN]);
        // Nothing found, and nothing asked.
        assert!(ids("zebra").is_empty());
        assert_eq!(ids("").len(), 3);
        assert_eq!(ids("   ").len(), 3);
    }

    #[test]
    fn a_search_finds_a_recording_that_has_nothing_but_live_captions() {
        let f = Fixture::empty();
        f.write(
            "2026-10-17 09-00-00",
            None,
            &[(files::LIVE_TRANSCRIPT, "[00:00] Bạn: thử ghi âm")],
        );
        assert_eq!(rows(f.library().meetings(), "ghi am", &no_state).len(), 1);
    }

    // ---- the job state of a row

    #[test]
    fn a_running_job_is_live_and_has_no_error() {
        let f = Fixture::empty();
        // Nothing on disk yet: a folder that is being processed looks unfinished, which it is not.
        f.write("2026-11-09 10-00-00", Some(json!({"title": "New"})), &[]);
        let record = record(&f, "2026-11-09 10-00-00");

        let (live, error) = state_of(Some(running(&record.id)), &record);
        assert_eq!(live, Some(running(&record.id)));
        assert_eq!(error, None);
    }

    #[test]
    fn a_failed_job_is_an_error_and_a_job_that_is_done_is_neither() {
        let f = Fixture::new();
        let record = record(&f, SCREEN);

        let failed = ended(
            SCREEN,
            Outcome::Failed(MeetingError::summary_failed("API error 401")),
        );
        let (live, error) = state_of(Some(failed), &record);
        assert_eq!(live, None);
        let error = error.expect("the failure");
        assert_eq!(
            (error.kind.as_str(), error.key.as_str()),
            ("Api", "meetings.error.summary_failed")
        );

        let (live, error) = state_of(
            Some(ended(SCREEN, Outcome::Done { summary: true })),
            &record,
        );
        assert_eq!((live, error), (None, None));
    }

    #[test]
    fn a_cancelled_job_says_so_even_when_the_folder_has_nothing() {
        let f = Fixture::empty();
        f.write("2026-11-09 10-00-00", Some(json!({"title": "New"})), &[]);
        let record = record(&f, "2026-11-09 10-00-00");
        let cancelled = ended(&record.id, Outcome::Failed(MeetingError::cancelled()));

        let (_, error) = state_of(Some(cancelled), &record);
        assert_eq!(error.map(|e| e.kind), Some("Cancelled".to_string()));
    }

    #[test]
    fn a_folder_with_neither_transcript_nor_summary_is_not_finished_processing() {
        let f = Fixture::empty();
        f.write(
            "2026-11-09 10-00-00",
            Some(json!({"title": "New"})),
            &[(files::LOG, "x")],
        );
        let (live, error) = state_of(None, &record(&f, "2026-11-09 10-00-00"));
        assert_eq!(live, None);
        let error = error.expect("unfinished");
        assert_eq!(
            (error.kind.as_str(), error.key.as_str()),
            ("Unfinished", "meetings.error.unfinished")
        );

        // A transcript alone is a recording with no summary, which is not a failure; and a summary alone is done.
        let f = Fixture::new();
        for id in [SCREEN, LINK, FILE] {
            assert_eq!(state_of(None, &record(&f, id)), (None, None), "{id}");
        }
    }

    // ---- the detail

    #[test]
    fn detail_splits_the_summary_into_its_sections() {
        let f = Fixture::new();
        let url = |path: &Path| format!("http://media.test/{}", path.display());
        let detail = detail(&record(&f, SCREEN), None, None, &url);

        let summary = detail.summary.expect("a summary");
        assert_eq!(summary.markdown, SUMMARY);
        assert_eq!(summary.title.as_deref(), Some("Họp kế hoạch refactor"));
        assert_eq!(
            summary.summary.as_deref(),
            Some("Nhóm thống nhất tách repo test và quản lý task trên GitHub.")
        );
        assert_eq!(
            summary.key_points.as_deref(),
            Some("- Repo test riêng chạy nhanh hơn")
        );
        assert_eq!(summary.decisions, ["Tách repo test riêng"]);
        assert_eq!(summary.questions, ["Ai giữ quyền admin?"]);
        assert_eq!(
            summary.actions,
            [
                ActionView {
                    index: 0,
                    owner: Some("Henry".into()),
                    text: "Tạo repo test mới (thứ Sáu)".into(),
                    done: true,
                },
                ActionView {
                    index: 1,
                    owner: None,
                    text: "Viết tài liệu hướng dẫn".into(),
                    done: false,
                },
            ]
        );
        assert_eq!(
            summary.timeline,
            [
                MarkView {
                    seconds: 0,
                    label: "00:00".into(),
                    text: "Mở đầu".into()
                },
                MarkView {
                    seconds: 65,
                    label: "01:05".into(),
                    text: "Kế hoạch refactor".into()
                },
            ]
        );
        // What the window has no place for comes as it is, with its heading.
        assert_eq!(
            summary.other,
            [SectionView {
                heading: "Ghi chú thêm".into(),
                body: "Gặp lại sau hai tuần.".into()
            }]
        );
    }

    #[test]
    fn detail_has_the_transcript_its_speakers_and_the_source() {
        let f = Fixture::new();
        let url = |_: &Path| String::new();
        let detail = detail(&record(&f, SCREEN), None, None, &url);

        assert_eq!(detail.row.id, SCREEN);
        assert!(!detail.transcript_is_live);
        assert_eq!(detail.transcript.len(), 3);
        assert_eq!(
            detail.transcript[1],
            LineView {
                start: 65.0,
                stamp: "01:05".into(),
                speaker: Some("Speaker 2".into()),
                text: "Mình đề xuất tách repo test ra riêng.".into(),
            }
        );
        let speakers: Vec<_> = detail
            .speakers
            .iter()
            .map(|s| (s.name.as_str(), s.lines))
            .collect();
        assert_eq!(speakers, [("Speaker 1", 2), ("Speaker 2", 1)]);
        assert_eq!(
            detail.speakers[0].sample.speaker.as_deref(),
            Some("Speaker 1")
        );
        assert!(TRANSCRIPT.contains(&detail.speakers[0].sample.text));
        // A screen recording is named by the window itself.
        assert_eq!(
            detail.source,
            dictate_core::meetings::library::SCREEN_SOURCE
        );
        assert_eq!(detail.source_label, "");
    }

    #[test]
    fn detail_names_the_source_by_its_host_or_its_file_name() {
        let f = Fixture::new();
        let url = |_: &Path| String::new();
        assert_eq!(
            detail(&record(&f, LINK), None, None, &url).source_label,
            "youtube.com"
        );
        assert_eq!(
            detail(&record(&f, FILE), None, None, &url).source_label,
            "họp tuần.m4a"
        );
        // No summary and no transcript are none, not empty.
        let f = Fixture::empty();
        f.write("2026-11-09 10-00-00", Some(json!({"title": "New"})), &[]);
        let none = detail(&record(&f, "2026-11-09 10-00-00"), None, None, &url);
        assert_eq!(
            (none.summary, none.transcript.len(), none.speakers.len()),
            (None, 0, 0)
        );
    }

    #[test]
    fn detail_marks_a_transcript_of_live_captions_and_names_no_speakers_in_it() {
        let f = Fixture::empty();
        f.write(
            "2026-10-17 09-00-00",
            None,
            &[(
                files::LIVE_TRANSCRIPT,
                "[00:00] Bạn: thử ghi âm một chút nhé các bạn",
            )],
        );
        let url = |_: &Path| String::new();
        let detail = detail(&record(&f, "2026-10-17 09-00-00"), None, None, &url);
        assert!(detail.transcript_is_live);
        assert_eq!(detail.transcript.len(), 1);
        assert!(detail.speakers.is_empty());
    }

    /// A recording whose `meta.json` names `media`.
    fn with_media(f: &Fixture, id: &str, media: &str) -> MeetingRecord {
        let meta = Meta {
            title: "Cuộc gọi".into(),
            source: media.into(),
            media: media.into(),
            created: 1_792_423_258.0,
            duration: None,
        };
        let folder = f.folder(id);
        std::fs::create_dir_all(&folder).unwrap();
        meta.write(&folder).unwrap();
        MeetingRecord::from_folder(&folder).unwrap()
    }

    #[test]
    fn detail_prefers_existing_media_then_audio_m4a() {
        let f = Fixture::empty();
        let original = f.dir().join("Phim ảnh").join("cuộc gọi.MP4");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, b"video").unwrap();
        let url = |path: &Path| format!("http://media.test/{}", path.display());

        // The media file is there: it plays, with a picture.
        let id = "2026-11-09 10-00-00";
        let record = with_media(&f, id, original.to_str().unwrap());
        std::fs::write(record.folder.join(files::AUDIO), b"audio").unwrap();
        let detail_of = |record: &MeetingRecord| detail(record, None, None, &url);
        let shown = detail_of(&record);
        assert_eq!(
            shown.media,
            Some(MediaView {
                url: url(&original),
                video: true
            })
        );
        assert!(!shown.media_missing);

        // It is gone: `audio.m4a` plays, with no picture, and the recording says its original is missing.
        std::fs::remove_file(&original).unwrap();
        let shown = detail_of(&record);
        assert_eq!(
            shown.media,
            Some(MediaView {
                url: url(&record.folder.join(files::AUDIO)),
                video: false
            })
        );
        assert!(shown.media_missing);

        // A recording with no media file in `meta.json` (a screen recording of a former version, or a download that
        // did not come to its end) plays its `audio.m4a` and has nothing missing.
        let id = "2026-11-09 10-00-01";
        let record = with_media(&f, id, "");
        std::fs::write(record.folder.join(files::AUDIO), b"audio").unwrap();
        let shown = detail_of(&record);
        assert_eq!(
            shown.media.map(|m| m.url),
            Some(url(&record.folder.join(files::AUDIO)))
        );
        assert!(!shown.media_missing);
    }

    #[test]
    fn detail_marks_missing_media() {
        let f = Fixture::empty();
        let url = |path: &Path| format!("http://media.test/{}", path.display());
        let gone = f.dir().join("moved").join("call.m4a");

        let record = with_media(&f, "2026-11-09 10-00-00", gone.to_str().unwrap());
        let shown = detail(&record, None, None, &url);
        assert_eq!(shown.media, None);
        assert!(shown.media_missing);

        // A media path that names a folder is not a file either.
        let folder_instead = with_media(&f, "2026-11-09 10-00-01", f.dir().to_str().unwrap());
        let shown = detail(&folder_instead, None, None, &url);
        assert_eq!(shown.media, None);
        assert!(shown.media_missing);
    }

    #[test]
    fn media_is_video_for_the_extensions_of_video() {
        let f = Fixture::empty();
        let url = |path: &Path| path.display().to_string();
        for (name, video) in [
            ("a.mov", true),
            ("a.MP4", true),
            ("a.m4v", true),
            ("a.webm", true),
            ("a.m4a", false),
            ("a.mp3", false),
            ("a.wav", false),
            ("a.mkv", false),
            ("a", false),
        ] {
            let file = f.dir().join(name);
            std::fs::write(&file, b"x").unwrap();
            let record = with_media(&f, &format!("r-{name}"), file.to_str().unwrap());
            let shown = detail(&record, None, None, &url);
            assert_eq!(shown.media.map(|m| m.video), Some(video), "{name}");
        }
    }

    #[test]
    fn detail_carries_the_job_state_of_its_row() {
        let f = Fixture::new();
        let url = |_: &Path| String::new();
        let shown = detail(&record(&f, SCREEN), Some(running(SCREEN)), None, &url);
        assert_eq!(shown.row.live, Some(running(SCREEN)));
        let error = ErrorView::other("It broke");
        let shown = detail(&record(&f, SCREEN), None, Some(error.clone()), &url);
        assert_eq!(shown.row.error, Some(error));
    }
}
