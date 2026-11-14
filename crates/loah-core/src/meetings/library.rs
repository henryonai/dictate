//! The recordings in the Dictate library: a folder per recording, written by the app and read by the MCP server.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::text::search::fold;

/// Files inside a recording's folder. The app and the MCP server both depend on these names and formats.
pub mod files {
    /// Title, source, media, created, duration.
    pub const META: &str = "meta.json";
    /// "[mm:ss] Speaker: text" paragraphs.
    pub const TRANSCRIPT: &str = "transcript.txt";
    pub const LIVE_TRANSCRIPT: &str = "live_transcript.txt";
    /// See `SummaryDoc`.
    pub const SUMMARY: &str = "summary.md";
    /// Indices of the ticked `SummaryDoc` action items.
    pub const TASKS: &str = "tasks.json";
    pub const LOG: &str = "log.txt";
    /// The recording's sound, mixed into one track.
    pub const AUDIO: &str = "audio.m4a";
}

/// `source` of a screen recording, stored as is.
pub const SCREEN_SOURCE: &str = "Quay màn hình";

/// The environment variable that points at another library folder.
const DIRECTORY_VARIABLE: &str = "DICTATE_MEETINGS_DIR";

/// The library's folder inside the Documents folder.
const LIBRARY_FOLDER: &str = "Dictate Meetings";

/// `meta.json`: what the app knows about a recording apart from its text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    /// `SCREEN_SOURCE`, the link, or the path of the imported file.
    pub source: String,
    /// The recording's video or audio file: the absolute path of an imported file (never copied into the
    /// folder), or of the recorded or downloaded one; empty while there is none.
    pub media: String,
    /// Seconds after the Unix epoch.
    pub created: f64,
    /// Seconds, once the audio is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

impl Meta {
    /// The folder's `meta.json`, with `""` and `0` for what is missing or of the wrong type and the keys it
    /// doesn't know ignored. `None` when there is no `meta.json` or it isn't a JSON object.
    pub fn read(folder: &Path) -> Option<Meta> {
        let fields = Fields::read(folder)?;
        Some(Meta {
            title: fields.title.unwrap_or_default(),
            source: fields.source.unwrap_or_default(),
            media: fields.media.unwrap_or_default(),
            created: fields.created.unwrap_or_default(),
            duration: fields.duration,
        })
    }
}

/// The fields of a `meta.json`, each `None` when missing or of the wrong type, so a reader can tell "no date"
/// from a date of 0.
#[derive(Default)]
struct Fields {
    title: Option<String>,
    source: Option<String>,
    media: Option<String>,
    created: Option<f64>,
    duration: Option<f64>,
}

impl Fields {
    fn read(folder: &Path) -> Option<Fields> {
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(folder.join(files::META)).ok()?).ok()?;
        if !json.is_object() {
            return None;
        }
        let text = |key: &str| json.get(key)?.as_str().map(str::to_string);
        let number = |key: &str| json.get(key)?.as_f64();
        Some(Fields {
            title: text("title"),
            source: text("source"),
            media: text("media"),
            created: number("created"),
            duration: number("duration"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Screen,
    File,
    Link,
}

/// One recording in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingRecord {
    /// The folder name.
    pub id: String,
    pub folder: PathBuf,
    pub title: String,
    pub source: String,
    /// `Meta::media`.
    pub media: String,
    pub created: DateTime<Utc>,
    pub duration: Option<f64>,
}

impl MeetingRecord {
    /// Reads a recording's folder; `None` when `path` isn't a folder. Whatever `meta.json` lacks (or holds in
    /// the wrong shape) is filled in: the title from the folder name, the date from the folder name read as
    /// local time, else the oldest possible date so the recording sorts last.
    pub fn from_folder(path: &Path) -> Option<Self> {
        if !path.is_dir() {
            return None;
        }
        let id = path.file_name()?.to_string_lossy().into_owned();
        let fields = Fields::read(path).unwrap_or_default();
        let created = fields
            .created
            .and_then(from_epoch_seconds)
            .or_else(|| date_from_folder_name(&id))
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        Some(MeetingRecord {
            title: fields.title.unwrap_or_else(|| id.clone()),
            source: fields.source.unwrap_or_default(),
            media: fields.media.unwrap_or_default(),
            duration: fields.duration,
            created,
            folder: path.to_path_buf(),
            id,
        })
    }

    /// The video or audio to play and to transcribe: `Meta::media`; `None` when `meta.json` has none yet.
    pub fn media(&self) -> Option<PathBuf> {
        (!self.media.is_empty()).then(|| PathBuf::from(&self.media))
    }

    pub fn kind(&self) -> Kind {
        if self.source == SCREEN_SOURCE {
            Kind::Screen
        } else if self.source.starts_with("http") {
            Kind::Link
        } else {
            Kind::File
        }
    }

    /// The text of `file` in the recording's folder; `None` when it is missing or not UTF-8.
    pub fn read(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.folder.join(file)).ok()
    }

    pub fn summary(&self) -> Option<String> {
        self.read_non_empty(files::SUMMARY)
    }

    /// The transcript and `false`, or the rougher realtime captions and `true` when that's all there is.
    pub fn transcript(&self) -> Option<(String, bool)> {
        self.read_non_empty(files::TRANSCRIPT)
            .map(|text| (text, false))
            .or_else(|| {
                self.read_non_empty(files::LIVE_TRANSCRIPT)
                    .map(|text| (text, true))
            })
    }

    /// The indices of the ticked action items; none when `tasks.json` is missing or not a list of integers.
    pub fn done_tasks(&self) -> BTreeSet<usize> {
        fs::read(self.folder.join(files::TASKS))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<i64>>(&bytes).ok())
            .map(|ticked| {
                ticked
                    .into_iter()
                    .filter_map(|index| usize::try_from(index).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read_non_empty(&self, file: &str) -> Option<String> {
        self.read(file).filter(|text| !text.is_empty())
    }
}

/// The moment `seconds` after the Unix epoch; `None` when it is out of range.
fn from_epoch_seconds(seconds: f64) -> Option<DateTime<Utc>> {
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.floor();
    let nanos = ((seconds - whole) * 1e9) as u32;
    DateTime::from_timestamp(whole as i64, nanos.min(999_999_999))
}

/// Folders are named "yyyy-MM-dd HH-mm-ss" (plus " (2)"… when several start in the same second), in local time.
fn date_from_folder_name(name: &str) -> Option<DateTime<Utc>> {
    let stamp: String = name.chars().take(19).collect();
    NaiveDateTime::parse_from_str(&stamp, "%Y-%m-%d %H-%M-%S")
        .ok()?
        .and_local_timezone(Local)
        .earliest()
        .map(|local| local.with_timezone(&Utc))
}

/// The recordings in the library folder, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingLibrary {
    pub directory: PathBuf,
}

impl MeetingLibrary {
    /// "Dictate Meetings" in the OS Documents folder, or the folder in `DICTATE_MEETINGS_DIR` (another library,
    /// e.g. for a demo).
    pub fn default_directory() -> PathBuf {
        resolve_directory(
            std::env::var_os(DIRECTORY_VARIABLE).as_deref(),
            dirs::home_dir().as_deref(),
            dirs::document_dir().as_deref(),
        )
    }

    pub fn new(directory: PathBuf) -> Self {
        MeetingLibrary { directory }
    }

    /// Every recording, newest first (the later folder first when two share a time). Empty when the library
    /// folder doesn't exist.
    pub fn meetings(&self) -> Vec<MeetingRecord> {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut all: Vec<MeetingRecord> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| MeetingRecord::from_folder(&entry.path()))
            .collect();
        all.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        all
    }

    /// By folder id, else by title: exact, then the only title containing it (case and diacritics ignored).
    pub fn find(&self, id_or_title: &str) -> Option<MeetingRecord> {
        let key = id_or_title.trim_matches(|c| matches!(c, ' ' | '"' | '\'' | '/'));
        let all = self.meetings();
        if let Some(found) = all.iter().find(|m| m.id == key) {
            return Some(found.clone());
        }
        let folded = fold(key);
        if let Some(found) = all.iter().find(|m| fold(&m.title) == folded) {
            return Some(found.clone());
        }
        if folded.is_empty() {
            return None;
        }
        let mut containing = all.iter().filter(|m| fold(&m.title).contains(&folded));
        match (containing.next(), containing.next()) {
            (Some(only), None) => Some(only.clone()),
            _ => None,
        }
    }
}

/// The library folder: `env` (the `DICTATE_MEETINGS_DIR` value) when set and not empty, with a leading `~`
/// meaning `home`; else "Dictate Meetings" inside `documents`, or inside `home`/Documents.
fn resolve_directory(
    env: Option<&OsStr>,
    home: Option<&Path>,
    documents: Option<&Path>,
) -> PathBuf {
    if let Some(dir) = env.filter(|dir| !dir.is_empty()) {
        return expand_tilde(Path::new(dir), home);
    }
    documents
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join("Documents")))
        .unwrap_or_else(|| PathBuf::from("Documents"))
        .join(LIBRARY_FOLDER)
}

/// `~` or `~/…` (any separator the OS accepts) with `home` in place of the `~`. Left as is without a home,
/// and `~user` is not expanded.
fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) if rest.as_os_str().is_empty() => home.to_path_buf(),
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{Local, TimeZone};

    use super::*;
    use crate::test_fixture::{Fixture, SUMMARY, TRANSCRIPT};

    /// The local time `y-m-d h:mi:s` as UTC, the way the library reads a folder name.
    fn local(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, m, d, h, mi, s)
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    fn record(f: &Fixture, id: &str) -> MeetingRecord {
        MeetingRecord::from_folder(&f.dir().join(id)).unwrap()
    }

    #[test]
    fn lists_newest_first() {
        let f = Fixture::new();
        let ids: Vec<_> = f.library().meetings().into_iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                "2026-10-19 22-20-58",
                "2026-10-18 11-50-17",
                "2026-10-17 09-00-00"
            ]
        );
    }

    #[test]
    fn kinds_and_live() {
        let f = Fixture::new();
        let m = f.library().meetings();
        assert_eq!(m[0].kind(), Kind::Screen);
        assert_eq!(m[1].kind(), Kind::Link);
        assert_eq!(m[2].transcript().map(|t| t.1), Some(true));
        assert_eq!(m[0].done_tasks(), BTreeSet::from([0]));
    }

    #[test]
    fn find_by_title_folded() {
        let f = Fixture::new();
        assert_eq!(
            f.library().find("hop ke hoach").map(|m| m.id).as_deref(),
            Some("2026-10-19 22-20-58")
        );
        assert!(f.library().find("\"Video về Swift\"").is_some());
    }

    #[test]
    fn broken_meta_falls_back() {
        let f = Fixture::new();
        f.write("2026-01-02 03-04-05", None, &[]);
        let folder = f.dir().join("2026-01-02 03-04-05");
        fs::write(folder.join(files::META), "{not json").unwrap();
        let m = MeetingRecord::from_folder(&folder).unwrap();
        assert_eq!(m.title, "2026-01-02 03-04-05");
        assert_eq!(m.source, "");
        assert_eq!(m.duration, None);
        assert_eq!(m.created, local(2026, 1, 2, 3, 4, 5));
    }

    #[test]
    fn missing_library_dir_is_empty() {
        let library = MeetingLibrary::new("/nonexistent/dictate".into());
        assert!(library.meetings().is_empty());
    }

    #[test]
    fn reads_meta_fields() {
        let f = Fixture::new();
        let m = record(&f, "2026-10-19 22-20-58");
        assert_eq!(m.id, "2026-10-19 22-20-58");
        assert_eq!(m.folder, f.dir().join("2026-10-19 22-20-58"));
        assert_eq!(m.title, "Họp kế hoạch refactor");
        assert_eq!(m.source, SCREEN_SOURCE);
        assert_eq!(m.duration, Some(373.55));
        // 1_792_423_258.39 s after the epoch, fraction kept.
        assert_eq!(m.created.timestamp(), 1_792_423_258);
        assert_eq!(m.created.timestamp_subsec_millis(), 390);
    }

    #[test]
    fn no_meta_uses_the_folder_name() {
        let f = Fixture::new();
        let m = record(&f, "2026-10-17 09-00-00");
        assert_eq!(m.title, "2026-10-17 09-00-00");
        assert_eq!(m.created, local(2026, 10, 17, 9, 0, 0));
    }

    #[test]
    fn folder_suffix_for_same_second_is_ignored_when_dating() {
        let f = Fixture::new();
        f.write("2026-10-17 09-00-00 (2)", None, &[]);
        let m = record(&f, "2026-10-17 09-00-00 (2)");
        assert_eq!(m.created, local(2026, 10, 17, 9, 0, 0));
    }

    #[test]
    fn meta_without_a_date_and_a_folder_without_one_is_the_oldest() {
        let f = Fixture::new();
        f.write("scratch", Some(serde_json::json!({})), &[]);
        let m = record(&f, "scratch");
        assert_eq!(m.created, DateTime::<Utc>::MIN_UTC);
        assert_eq!(
            f.library().meetings().last().map(|m| m.id.as_str()),
            Some("scratch")
        );
    }

    #[test]
    fn meta_with_the_wrong_types_falls_back() {
        let f = Fixture::new();
        f.write(
            "2026-02-03 04-05-06",
            Some(serde_json::json!({"title": 7, "source": ["x"], "created": "soon", "duration": "long"})),
            &[],
        );
        let m = record(&f, "2026-02-03 04-05-06");
        assert_eq!(m.title, "2026-02-03 04-05-06");
        assert_eq!(m.source, "");
        assert_eq!(m.duration, None);
        assert_eq!(m.created, local(2026, 2, 3, 4, 5, 6));
    }

    #[test]
    fn meta_that_is_not_an_object_is_ignored() {
        let f = Fixture::new();
        f.write(
            "2026-02-03 04-05-06",
            Some(serde_json::json!(["title"])),
            &[],
        );
        assert_eq!(
            record(&f, "2026-02-03 04-05-06").title,
            "2026-02-03 04-05-06"
        );
    }

    #[test]
    fn a_created_time_out_of_range_falls_back_to_the_folder_name() {
        let f = Fixture::new();
        f.write(
            "2026-02-03 04-05-06",
            Some(serde_json::json!({"created": 1e300})),
            &[],
        );
        assert_eq!(
            record(&f, "2026-02-03 04-05-06").created,
            local(2026, 2, 3, 4, 5, 6)
        );
    }

    #[test]
    fn a_file_is_not_a_recording() {
        let f = Fixture::new();
        fs::write(f.dir().join("notes.txt"), "x").unwrap();
        assert!(MeetingRecord::from_folder(&f.dir().join("notes.txt")).is_none());
        assert!(MeetingRecord::from_folder(&f.dir().join("missing")).is_none());
        assert_eq!(f.library().meetings().len(), 3);
    }

    #[test]
    fn kind_is_screen_link_or_file() {
        let f = Fixture::empty();
        let sources = [
            (SCREEN_SOURCE, Kind::Screen),
            ("http://example.com/v", Kind::Link),
            ("https://example.com/v", Kind::Link),
            ("/Users/x/call.m4a", Kind::File),
            ("", Kind::File),
        ];
        for (n, (source, kind)) in sources.into_iter().enumerate() {
            let id = format!("2026-03-04 05-06-0{n}");
            f.write(&id, Some(serde_json::json!({ "source": source })), &[]);
            assert_eq!(record(&f, &id).kind(), kind, "source {source:?}");
        }
    }

    #[test]
    fn reads_summary_and_transcript() {
        let f = Fixture::new();
        let m = record(&f, "2026-10-19 22-20-58");
        assert_eq!(m.summary().as_deref(), Some(SUMMARY));
        assert_eq!(m.transcript(), Some((TRANSCRIPT.to_string(), false)));
        assert_eq!(m.read(files::SUMMARY).as_deref(), Some(SUMMARY));
        assert_eq!(m.read(files::LOG), None);
    }

    #[test]
    fn no_summary_yet_is_none() {
        let f = Fixture::new();
        assert_eq!(record(&f, "2026-10-18 11-50-17").summary(), None);
    }

    #[test]
    fn empty_files_count_as_missing() {
        let f = Fixture::new();
        f.write(
            "2026-04-05 06-07-08",
            None,
            &[
                (files::SUMMARY, ""),
                (files::TRANSCRIPT, ""),
                (files::LIVE_TRANSCRIPT, "[00:00] Bạn: xin chào"),
            ],
        );
        let m = record(&f, "2026-04-05 06-07-08");
        assert_eq!(m.summary(), None);
        assert_eq!(
            m.transcript(),
            Some(("[00:00] Bạn: xin chào".to_string(), true))
        );
        f.write("2026-04-05 06-07-09", None, &[(files::LIVE_TRANSCRIPT, "")]);
        assert_eq!(record(&f, "2026-04-05 06-07-09").transcript(), None);
    }

    #[test]
    fn the_transcript_wins_over_live_captions() {
        let f = Fixture::new();
        f.write(
            "2026-04-05 06-07-08",
            None,
            &[
                (files::TRANSCRIPT, "[00:00] Final"),
                (files::LIVE_TRANSCRIPT, "[00:00] Live"),
            ],
        );
        let m = record(&f, "2026-04-05 06-07-08");
        assert_eq!(m.transcript(), Some(("[00:00] Final".to_string(), false)));
    }

    #[test]
    fn media_is_the_original_path_when_there_is_one() {
        let f = Fixture::new();
        // The fixture's screen recording has no `media` in its meta.
        assert_eq!(record(&f, "2026-10-19 22-20-58").media(), None);
        let meta = Meta {
            title: "Cuộc gọi".into(),
            source: "/Users/Lan/Phim ảnh/cuộc gọi.mp4".into(),
            media: "/Users/Lan/Phim ảnh/cuộc gọi.mp4".into(),
            created: 1_792_423_258.0,
            duration: None,
        };
        f.write_meta("2026-11-09 10-15-31", &meta);
        assert_eq!(
            record(&f, "2026-11-09 10-15-31").media(),
            Some(PathBuf::from("/Users/Lan/Phim ảnh/cuộc gọi.mp4"))
        );
        // An empty `media` is none, too.
        f.write_meta(
            "2026-11-09 10-15-32",
            &Meta {
                media: String::new(),
                ..meta
            },
        );
        assert_eq!(record(&f, "2026-11-09 10-15-32").media(), None);
    }

    #[test]
    fn done_tasks_ignores_unreadable_files() {
        let f = Fixture::new();
        let tasks = |text: &str| {
            f.write("2026-05-06 07-08-09", None, &[(files::TASKS, text)]);
            record(&f, "2026-05-06 07-08-09").done_tasks()
        };
        assert_eq!(tasks("[1, 0, 1]"), BTreeSet::from([0, 1]));
        assert_eq!(tasks("[]"), BTreeSet::new());
        assert_eq!(tasks("not json"), BTreeSet::new());
        assert_eq!(tasks(r#"["0"]"#), BTreeSet::new());
        assert_eq!(tasks("[-1, 2]"), BTreeSet::from([2]));
        assert_eq!(
            record(&f, "2026-10-18 11-50-17").done_tasks(),
            BTreeSet::new()
        );
    }

    #[test]
    fn equal_times_list_the_later_folder_first() {
        let f = Fixture::empty();
        for id in [
            "2030-01-01 00-00-00 (3)",
            "2030-01-01 00-00-00",
            "2030-01-01 00-00-00 (4)",
            "2030-01-01 00-00-00 (2)",
        ] {
            f.write(id, None, &[]);
        }
        let ids: Vec<_> = f.library().meetings().into_iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                "2030-01-01 00-00-00 (4)",
                "2030-01-01 00-00-00 (3)",
                "2030-01-01 00-00-00 (2)",
                "2030-01-01 00-00-00",
            ]
        );
    }

    #[test]
    fn find_by_id_title_or_the_only_title_containing_it() {
        let f = Fixture::new();
        let library = f.library();
        let id = |q: &str| library.find(q).map(|m| m.id);
        assert_eq!(
            id("2026-10-18 11-50-17").as_deref(),
            Some("2026-10-18 11-50-17")
        );
        assert_eq!(
            id("hop ke hoach refactor").as_deref(),
            Some("2026-10-19 22-20-58")
        );
        assert_eq!(id("swift").as_deref(), Some("2026-10-18 11-50-17"));
        assert_eq!(id("không có"), None);
    }

    #[test]
    fn find_trims_quotes_slashes_and_spaces() {
        let f = Fixture::new();
        let library = f.library();
        let id = |q: &str| library.find(q).map(|m| m.id);
        assert_eq!(
            id(" '/2026-10-18 11-50-17/' ").as_deref(),
            Some("2026-10-18 11-50-17")
        );
        assert_eq!(id("\"swift\"").as_deref(), Some("2026-10-18 11-50-17"));
    }

    #[test]
    fn find_prefers_the_exact_title_and_refuses_an_ambiguous_one() {
        let f = Fixture::new();
        f.write(
            "2026-06-01 10-00-00",
            Some(serde_json::json!({"title": "Swift"})),
            &[],
        );
        f.write(
            "2026-06-02 10-00-00",
            Some(serde_json::json!({"title": "Swift concurrency"})),
            &[],
        );
        let library = f.library();
        let id = |q: &str| library.find(q).map(|m| m.id);
        // "Video về Swift", "Swift" and "Swift concurrency" all contain "swift", one is exact.
        assert_eq!(id("SWIFT").as_deref(), Some("2026-06-01 10-00-00"));
        // "Swift" and "Swift concurrency" both contain "swi".
        assert_eq!(id("swi"), None);
    }

    #[test]
    fn find_with_nothing_to_look_for_finds_nothing() {
        // With one recording an empty key would otherwise be "the only title containing it".
        let f = Fixture::empty();
        f.write(
            "2026-06-01 10-00-00",
            Some(serde_json::json!({"title": "Swift"})),
            &[],
        );
        assert_eq!(f.library().find("").map(|m| m.id), None);
        assert_eq!(f.library().find(" \"/ ").map(|m| m.id), None);
    }

    #[test]
    fn the_env_directory_wins_and_a_tilde_means_home() {
        let home = Path::new("/home/henry");
        let docs = Some(Path::new("/home/henry/Docs"));
        let dir = |env: &str| resolve_directory(Some(OsStr::new(env)), Some(home), docs);
        assert_eq!(dir("/srv/demo"), Path::new("/srv/demo"));
        assert_eq!(dir("~"), home);
        assert_eq!(dir("~/Demo Library"), home.join("Demo Library"));
        assert_eq!(dir("~henry/x"), Path::new("~henry/x"));
        assert_eq!(dir("rel/demo"), Path::new("rel/demo"));
    }

    #[test]
    fn a_tilde_stays_as_typed_without_a_home() {
        let dir = resolve_directory(Some(OsStr::new("~/Demo")), None, None);
        assert_eq!(dir, Path::new("~/Demo"));
    }

    #[test]
    fn without_env_the_library_is_in_documents() {
        let home = Some(Path::new("/home/henry"));
        let docs = Some(Path::new("/home/henry/Tài liệu"));
        let expected = Path::new("/home/henry/Tài liệu/Dictate Meetings");
        assert_eq!(resolve_directory(None, home, docs), expected);
        // An empty variable counts as unset.
        assert_eq!(
            resolve_directory(Some(OsStr::new("")), home, docs),
            expected
        );
    }

    #[test]
    fn documents_falls_back_to_home() {
        let home = Some(Path::new("/home/henry"));
        assert_eq!(
            resolve_directory(None, home, None),
            Path::new("/home/henry/Documents/Dictate Meetings")
        );
        assert_eq!(
            resolve_directory(None, None, None),
            Path::new("Documents/Dictate Meetings")
        );
    }

    #[test]
    fn the_default_directory_ends_in_dictate_meetings_unless_overridden() {
        // Reads the real environment, so it only checks what holds either way.
        let dir = MeetingLibrary::default_directory();
        assert!(!dir.as_os_str().is_empty());
        if std::env::var_os("DICTATE_MEETINGS_DIR").is_none_or(|v| v.is_empty()) {
            assert!(dir.ends_with("Dictate Meetings"));
        }
    }
}
