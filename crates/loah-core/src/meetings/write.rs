//! Writing into the library: a recording's folder, its files and its exports. Every file the app rewrites
//! goes through `atomic_write`, so a quit or a crash never leaves half a transcript, summary or meta.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Local};

use super::library::{files, MeetingLibrary, MeetingRecord, Meta};
use super::speakers;
use crate::text::segments::Segment;
use crate::text::timestamp;

/// Past this many bytes `log.txt` is cut down to `LOG_KEPT`.
const LOG_LIMIT: u64 = 60_000;

/// How much of the end of `log.txt` is kept when it passes `LOG_LIMIT`, in bytes.
const LOG_KEPT: usize = 40_000;

/// A paragraph takes in the next segment of the same speaker while that starts less than this long after it, in
/// seconds.
const PARAGRAPH_SECONDS: f64 = 60.0;

/// The longest name `export_file_name` gives, without ".md", in characters.
const EXPORT_NAME_LENGTH: usize = 120;

/// What `export_file_name` calls a title with nothing usable in it.
const EXPORT_NAME_FALLBACK: &str = "Dictate";

/// Writes `bytes` to a temporary file next to `path`, then renames it over `path`: a reader sees the old
/// file or the new one, whole, whatever happens in between. The folder of `path` must exist.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name to write to"))?;
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = path.with_file_name(temp_name);
    let written = write_synced(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// A new file `path` holding `bytes`, on disk when this returns.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl Meta {
    /// Replaces the folder's `meta.json` (pretty-printed JSON, written atomically).
    pub fn write(&self, folder: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self)?;
        atomic_write(&folder.join(files::META), &json)
    }
}

impl MeetingLibrary {
    /// A new folder for a recording started at `now`, named "yyyy-MM-dd HH-mm-ss" with " (2)", " (3)"… added
    /// when that name is taken, and its name (the recording's id). The library folder is created when missing.
    pub fn create_folder(&self, now: DateTime<Local>) -> io::Result<(String, PathBuf)> {
        fs::create_dir_all(&self.directory)?;
        let base = now.format("%Y-%m-%d %H-%M-%S").to_string();
        let mut id = base.clone();
        let mut attempt = 1;
        loop {
            let folder = self.directory.join(&id);
            // Not `create_dir_all`, which would succeed for a folder that is already there: creating is
            // what claims the name, so two recordings can't end up in one folder.
            match fs::create_dir(&folder) {
                Ok(()) => return Ok((id, folder)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    attempt += 1;
                    id = format!("{base} ({attempt})");
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// The text of `transcript.txt`: "[mm:ss] Speaker: text" paragraphs (no "Speaker: " for a segment without one)
/// separated by a blank line. Segments without text are dropped, and one that starts less than
/// `PARAGRAPH_SECONDS` after the paragraph before it, with the same speaker, joins that paragraph.
pub fn transcript_text(segments: &[Segment]) -> String {
    struct Paragraph<'a> {
        start: f64,
        speaker: Option<&'a str>,
        text: String,
    }
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        let speaker = segment.speaker.as_deref();
        match paragraphs.last_mut() {
            Some(last)
                if last.speaker == speaker && segment.start - last.start < PARAGRAPH_SECONDS =>
            {
                last.text.push(' ');
                last.text.push_str(text);
            }
            _ => paragraphs.push(Paragraph {
                start: segment.start,
                speaker,
                text: text.to_string(),
            }),
        }
    }
    paragraphs
        .iter()
        .map(|paragraph| {
            let stamp = timestamp::format(paragraph.start);
            match paragraph.speaker {
                Some(speaker) => format!("[{stamp}] {speaker}: {}", paragraph.text),
                None => format!("[{stamp}] {}", paragraph.text),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Adds `line` to the folder's `log.txt`. When that passes `LOG_LIMIT` bytes, only its last `LOG_KEPT` bytes
/// are kept, from the start of a line (rewritten atomically).
pub fn append_log(folder: &Path, line: &str) -> io::Result<()> {
    let path = folder.join(files::LOG);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    // One write, so lines from different threads don't mix.
    file.write_all(format!("{line}\n").as_bytes())?;
    let length = file.metadata()?.len();
    drop(file);
    if length > LOG_LIMIT {
        let log = fs::read(&path)?;
        atomic_write(&path, newest_lines(&log))?;
    }
    Ok(())
}

/// The last `LOG_KEPT` bytes of `log`, minus the line they start in the middle of.
fn newest_lines(log: &[u8]) -> &[u8] {
    let cut = match log.len().checked_sub(LOG_KEPT) {
        Some(cut) if cut > 0 => cut,
        _ => return log,
    };
    if log[cut - 1] == b'\n' {
        return &log[cut..];
    }
    match log[cut..].iter().position(|&byte| byte == b'\n') {
        Some(newline) => &log[cut + newline + 1..],
        None => &[],
    }
}

impl MeetingRecord {
    /// Saves which action items are ticked: `tasks.json`, a sorted JSON list such as `[0,2]`.
    pub fn set_done_tasks(&self, done: &BTreeSet<usize>) -> io::Result<()> {
        let json = serde_json::to_vec(done)?;
        atomic_write(&self.folder.join(files::TASKS), &json)
    }

    /// Forgets the ticked action items (they belong to a summary that is about to be replaced). Fine when none
    /// are saved.
    pub fn clear_tasks(&self) -> io::Result<()> {
        match fs::remove_file(self.folder.join(files::TASKS)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Renames speakers (old -> new) in `transcript.txt` and `summary.md`, all at once, and logs it. The new
    /// names are cleaned with `speakers::clean`; one that comes out empty or equal to the old one is dropped.
    /// Returns the renames done, sorted; nothing changes (and none is logged) when there are none, or when
    /// the recording has no transcript.
    pub fn rename_speakers(
        &self,
        names: &HashMap<String, String>,
    ) -> io::Result<Vec<(String, String)>> {
        let names: HashMap<String, String> = names
            .iter()
            .filter(|(old, _)| !old.is_empty())
            .filter_map(|(old, new)| {
                let new = speakers::clean(new)?;
                (new != *old).then(|| (old.clone(), new))
            })
            .collect();
        let Some(transcript) = self.read(files::TRANSCRIPT).filter(|_| !names.is_empty()) else {
            return Ok(Vec::new());
        };
        atomic_write(
            &self.folder.join(files::TRANSCRIPT),
            speakers::rename_transcript(&transcript, &names).as_bytes(),
        )?;
        if let Some(summary) = self.read(files::SUMMARY) {
            atomic_write(
                &self.folder.join(files::SUMMARY),
                speakers::rename_summary(&summary, &names).as_bytes(),
            )?;
        }
        let mut applied: Vec<(String, String)> = names.into_iter().collect();
        applied.sort();
        let renames: Vec<String> = applied
            .iter()
            .map(|(old, new)| format!("{old} → {new}"))
            .collect();
        // The files are renamed by now, so a log that can't be written is no reason to report a failure.
        let _ = append_log(
            &self.folder,
            &format!("Renamed speakers: {}", renames.join(", ")),
        );
        Ok(applied)
    }
}

/// The Markdown file of an export: the summary (a title line when there is none), then the transcript under
/// its own heading.
pub fn export_markdown(title: &str, summary: Option<&str>, transcript: Option<&str>) -> String {
    let mut markdown = summary.map_or_else(|| format!("# {title}"), str::to_string);
    if let Some(transcript) = transcript {
        markdown.push_str("\n\n---\n\n## Transcript\n\n");
        markdown.push_str(transcript);
    }
    markdown
}

/// A file name for exporting a recording: the title with each character a file system rejects (`/ \ : * ? " < > |`
/// and control characters) turned into `-`, without dots or spaces at either end, at most `EXPORT_NAME_LENGTH`
/// characters, plus ".md". "Dictate" stands in for a title with nothing left.
pub fn export_file_name(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let edge = ['.', ' '];
    let capped: String = safe
        .trim_matches(edge)
        .chars()
        .take(EXPORT_NAME_LENGTH)
        .collect();
    // The cut can end on a space or a dot, which Windows doesn't allow at the end of a name.
    let name = capped.trim_end_matches(edge);
    format!(
        "{}.md",
        if name.is_empty() {
            EXPORT_NAME_FALLBACK
        } else {
            name
        }
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::TimeZone;

    use super::*;
    use crate::meetings::library::files;
    use crate::meetings::transcript::TranscriptLine;
    use crate::test_fixture::{Fixture, SUMMARY, TRANSCRIPT};

    fn segment(start: f64, speaker: Option<&str>, text: &str) -> Segment {
        Segment {
            start,
            speaker: speaker.map(str::to_string),
            text: text.to_string(),
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn sample_meta() -> Meta {
        Meta {
            title: "Họp kế hoạch".into(),
            source: "/Users/Lan/Phim ảnh/họp.mp4".into(),
            media: "/Users/Lan/Phim ảnh/họp.mp4".into(),
            created: 1_792_423_258.5,
            duration: None,
        }
    }

    /// A recording with the fixture's transcript and the given summary (none when `summary` is `None`).
    fn recording(f: &Fixture, summary: Option<&str>) -> MeetingRecord {
        let id = "2026-11-09 10-15-31";
        f.write(id, Some(serde_json::json!({"title": "Họp"})), &[]);
        let folder = f.dir().join(id);
        fs::write(folder.join(files::TRANSCRIPT), TRANSCRIPT).unwrap();
        if let Some(summary) = summary {
            fs::write(folder.join(files::SUMMARY), summary).unwrap();
        }
        MeetingRecord::from_folder(&folder).unwrap()
    }

    #[test]
    fn atomic_write_never_leaves_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(files::TRANSCRIPT);
        atomic_write(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let second = "[00:00] Một bản dài hơn bản đầu.".as_bytes();
        atomic_write(&path, second).unwrap();
        assert_eq!(fs::read(&path).unwrap(), second);
        assert_eq!(names_in(dir.path()), [files::TRANSCRIPT]);
    }

    #[test]
    fn atomic_write_cleans_up_when_the_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A folder can't be replaced by a file.
        let target = dir.path().join("taken");
        fs::create_dir(&target).unwrap();
        assert!(atomic_write(&target, b"x").is_err());
        assert_eq!(names_in(dir.path()), ["taken"]);
    }

    #[test]
    fn atomic_write_into_a_missing_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(atomic_write(&dir.path().join("missing").join("a.txt"), b"x").is_err());
        assert!(names_in(dir.path()).is_empty());
    }

    #[test]
    fn meta_round_trips_and_omits_unknown_duration() {
        let dir = tempfile::tempdir().unwrap();
        let meta = sample_meta();
        meta.write(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join(files::META)).unwrap();
        assert!(!text.contains("\"duration\""), "{text}");
        assert!(text.contains("\n  \"title\""), "pretty printed: {text}");
        assert_eq!(Meta::read(dir.path()), Some(meta.clone()));
        assert_eq!(names_in(dir.path()), [files::META]);

        let timed = Meta {
            duration: Some(373.55),
            ..meta
        };
        timed.write(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join(files::META)).unwrap();
        assert!(text.contains("\"duration\": 373.55"), "{text}");
        assert_eq!(Meta::read(dir.path()), Some(timed));
    }

    #[test]
    fn meta_reads_a_swift_file_with_extra_keys() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(files::META),
            r#"{
              "title" : "Họp kế hoạch refactor",
              "source" : "Quay màn hình",
              "media" : "",
              "created" : 1792423258.39,
              "duration" : 373.55,
              "future" : {"x": [1, 2]}
            }"#,
        )
        .unwrap();
        assert_eq!(
            Meta::read(dir.path()),
            Some(Meta {
                title: "Họp kế hoạch refactor".into(),
                source: "Quay màn hình".into(),
                media: String::new(),
                created: 1_792_423_258.39,
                duration: Some(373.55),
            })
        );
    }

    #[test]
    fn meta_read_fills_in_what_is_missing_or_the_wrong_type() {
        let dir = tempfile::tempdir().unwrap();
        let read = |json: &str| {
            fs::write(dir.path().join(files::META), json).unwrap();
            Meta::read(dir.path())
        };
        let nothing = Meta::default();
        assert_eq!(read("{}"), Some(nothing.clone()));
        assert_eq!(
            read(r#"{"title": "Chỉ có tên"}"#),
            Some(Meta {
                title: "Chỉ có tên".into(),
                ..nothing.clone()
            })
        );
        assert_eq!(
            read(r#"{"title": 7, "source": "x", "created": "soon", "duration": "long"}"#),
            Some(Meta {
                source: "x".into(),
                ..nothing
            })
        );
        assert_eq!(read("{not json"), None);
        assert_eq!(read(r#"["title"]"#), None);
        fs::remove_file(dir.path().join(files::META)).unwrap();
        assert_eq!(Meta::read(dir.path()), None);
    }

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, mi, s)
            .earliest()
            .unwrap()
    }

    #[test]
    fn create_folder_adds_suffix_on_collision() {
        let f = Fixture::empty();
        let library = f.library();
        let now = at(2026, 11, 9, 10, 15, 31);
        for expected in [
            "2026-11-09 10-15-31",
            "2026-11-09 10-15-31 (2)",
            "2026-11-09 10-15-31 (3)",
        ] {
            let (id, folder) = library.create_folder(now).unwrap();
            assert_eq!(id, expected);
            assert_eq!(folder, f.dir().join(expected));
            assert!(folder.is_dir());
        }
    }

    #[test]
    fn create_folder_creates_the_library_directory() {
        let f = Fixture::empty();
        let library = MeetingLibrary::new(f.dir().join("Tài liệu").join("Dictate Meetings"));
        let (id, folder) = library.create_folder(at(2026, 11, 9, 10, 15, 31)).unwrap();
        assert!(folder.is_dir());
        assert_eq!(library.meetings().len(), 1);
        assert_eq!(library.meetings()[0].id, id);
    }

    #[test]
    fn create_folder_skips_a_name_taken_by_a_file() {
        let f = Fixture::empty();
        fs::write(f.dir().join("2026-11-09 10-15-31"), "x").unwrap();
        let (id, _) = f
            .library()
            .create_folder(at(2026, 11, 9, 10, 15, 31))
            .unwrap();
        assert_eq!(id, "2026-11-09 10-15-31 (2)");
    }

    #[test]
    fn transcript_text_merges_same_speaker_within_60s() {
        let segments = [
            segment(0.0, Some("S1"), "a"),
            segment(30.0, Some("S1"), "b"),
            segment(61.0, Some("S1"), "c"),
            segment(62.0, Some("S2"), "d"),
            segment(63.0, None, "  "),
            segment(3700.0, None, "e"),
        ];
        assert_eq!(
            transcript_text(&segments),
            "[00:00] S1: a b\n\n[01:01] S1: c\n\n[01:02] S2: d\n\n[1:01:40] e"
        );
    }

    #[test]
    fn transcript_text_starts_a_paragraph_at_exactly_60s() {
        let segments = [
            segment(0.0, Some("S1"), "a"),
            segment(59.9, Some("S1"), "b"),
            segment(60.0, Some("S1"), "c"),
        ];
        assert_eq!(
            transcript_text(&segments),
            "[00:00] S1: a b\n\n[01:00] S1: c"
        );
    }

    #[test]
    fn transcript_text_trims_and_separates_nameless_from_named_speakers() {
        let segments = [
            segment(1.9, None, "  xin chào \t"),
            segment(2.0, Some("Lan"), "\nchào bạn "),
            segment(3.0, None, "ừ"),
        ];
        assert_eq!(
            transcript_text(&segments),
            "[00:01] xin chào\n\n[00:02] Lan: chào bạn\n\n[00:03] ừ"
        );
        assert_eq!(transcript_text(&[]), "");
        assert_eq!(transcript_text(&[segment(0.0, None, " ")]), "");
    }

    #[test]
    fn transcript_text_round_trips_through_parse() {
        let segments = [
            segment(0.0, Some("Speaker 1"), "Xin chào"),
            segment(20.0, Some("Speaker 1"), "mọi người."),
            segment(75.0, Some("Speaker 2"), "Mình đề xuất tách repo."),
            segment(3700.0, None, "Hết giờ."),
        ];
        let lines = TranscriptLine::parse(&transcript_text(&segments));
        let got: Vec<_> = lines
            .iter()
            .map(|l| (l.start, l.speaker.as_deref(), l.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (0.0, Some("Speaker 1"), "Xin chào mọi người."),
                (75.0, Some("Speaker 2"), "Mình đề xuất tách repo."),
                (3700.0, None, "Hết giờ."),
            ]
        );
    }

    #[test]
    fn append_log_adds_lines() {
        let dir = tempfile::tempdir().unwrap();
        append_log(dir.path(), "Whisper: 10%").unwrap();
        append_log(dir.path(), "Đã lưu transcript (120 ký tự).").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(files::LOG)).unwrap(),
            "Whisper: 10%\nĐã lưu transcript (120 ký tự).\n"
        );
    }

    /// "line NNNN" and padding, `width` bytes with the newline.
    fn log_line(i: usize, width: usize) -> String {
        format!("line {i:04} {}", "x".repeat(width - 11))
    }

    #[test]
    fn append_log_caps_size() {
        // 707 lines of 99 bytes, 69,993 bytes in all. The cut at 40,000 bytes from the end lands inside a line.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(files::LOG);
        let seeded: Vec<String> = (0..706).map(|i| log_line(i, 99)).collect();
        fs::write(&log, seeded.join("\n") + "\n").unwrap();
        assert_eq!(fs::metadata(&log).unwrap().len(), 69_894);

        // The newest line takes the file past 60,000 bytes.
        append_log(dir.path(), &log_line(706, 99)).unwrap();
        let text = fs::read_to_string(&log).unwrap();
        assert_eq!(text.len(), 39_996, "the 404 newest lines");
        let kept: Vec<&str> = text.lines().collect();
        assert_eq!(kept.len(), 404);
        for (n, kept) in kept.iter().enumerate() {
            assert_eq!(
                *kept,
                log_line(303 + n, 99),
                "starts at a line boundary, in order"
            );
        }
        assert!(text.ends_with('\n'));

        // Below the cap a line is only added.
        append_log(dir.path(), "next").unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), format!("{text}next\n"));
    }

    #[test]
    fn append_log_keeps_the_line_the_cut_lands_on() {
        // 700 lines of 100 bytes: 40,000 bytes from the end is exactly where a line starts, so it stays whole.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(files::LOG);
        let seeded: Vec<String> = (0..699).map(|i| log_line(i, 100)).collect();
        fs::write(&log, seeded.join("\n") + "\n").unwrap();
        append_log(dir.path(), &log_line(699, 100)).unwrap();
        let text = fs::read_to_string(&log).unwrap();
        assert_eq!(text.len(), 40_000);
        assert!(text.starts_with(&log_line(300, 100)));
        assert!(text.ends_with(&format!("{}\n", log_line(699, 100))));
    }

    #[test]
    fn append_log_never_grows_past_the_cap_across_many_appends() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..2_000 {
            append_log(dir.path(), &log_line(i, 100)).unwrap();
            let length = fs::metadata(dir.path().join(files::LOG)).unwrap().len();
            assert!(length <= 60_000, "{length} bytes after line {i}");
        }
        let text = fs::read_to_string(dir.path().join(files::LOG)).unwrap();
        assert!(text.lines().all(|l| l.len() == 99));
        assert!(text.ends_with(&format!("{}\n", log_line(1_999, 100))));
    }

    #[test]
    fn append_log_keeps_whole_characters() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(files::LOG);
        // Lines of 3-byte characters, so a cut at a fixed byte count would land inside one.
        let line = "ệ".repeat(500);
        for _ in 0..50 {
            append_log(dir.path(), &line).unwrap();
        }
        let text = fs::read_to_string(&log).expect("still valid UTF-8");
        assert!(text.len() <= 60_000);
        assert!(text.lines().all(|l| l == line));
    }

    #[test]
    fn rename_speakers_rewrites_both_files_and_logs() {
        let f = Fixture::empty();
        let summary = "# Họp\n\n## Tóm tắt\nSpeaker 1 và Speaker 2 bàn kế hoạch.\n\n## Việc cần làm\n- [ ] **Speaker 1** — tạo repo\n";
        let m = recording(&f, Some(summary));
        let names = HashMap::from([
            ("Speaker 1".to_string(), "An".to_string()),
            ("Speaker 2".to_string(), "An ".to_string()),
            ("Speaker 3".to_string(), "Speaker 3".to_string()),
        ]);
        let applied = m.rename_speakers(&names).unwrap();
        assert_eq!(
            applied,
            [
                ("Speaker 1".to_string(), "An".to_string()),
                ("Speaker 2".to_string(), "An".to_string())
            ]
        );
        assert_eq!(
            fs::read_to_string(m.folder.join(files::TRANSCRIPT)).unwrap(),
            TRANSCRIPT
                .replace("Speaker 1", "An")
                .replace("Speaker 2", "An")
        );
        assert_eq!(
            fs::read_to_string(m.folder.join(files::SUMMARY)).unwrap(),
            "# Họp\n\n## Tóm tắt\nAn và An bàn kế hoạch.\n\n## Việc cần làm\n- [ ] **An** — tạo repo\n"
        );
        assert_eq!(
            fs::read_to_string(m.folder.join(files::LOG)).unwrap(),
            "Renamed speakers: Speaker 1 → An, Speaker 2 → An\n"
        );
        // Nothing but the recording's own files is left behind.
        assert_eq!(
            names_in(&m.folder),
            [files::LOG, files::META, files::SUMMARY, files::TRANSCRIPT]
        );
    }

    #[test]
    fn rename_speakers_without_a_summary_writes_no_summary() {
        let f = Fixture::empty();
        let m = recording(&f, None);
        let names = HashMap::from([("Speaker 2".to_string(), "Lan".to_string())]);
        assert_eq!(m.rename_speakers(&names).unwrap().len(), 1);
        assert!(!m.folder.join(files::SUMMARY).exists());
        assert_eq!(
            fs::read_to_string(m.folder.join(files::TRANSCRIPT)).unwrap(),
            TRANSCRIPT.replace("Speaker 2", "Lan")
        );
    }

    #[test]
    fn rename_speakers_ignores_empty_and_unchanged_names() {
        let f = Fixture::empty();
        let m = recording(&f, Some(SUMMARY));
        let names = HashMap::from([
            ("Speaker 1".to_string(), "Speaker 1".to_string()),
            ("Speaker 2".to_string(), " : . ".to_string()),
            (String::new(), "Lan".to_string()),
        ]);
        assert_eq!(m.rename_speakers(&names).unwrap(), []);
        assert_eq!(
            fs::read_to_string(m.folder.join(files::TRANSCRIPT)).unwrap(),
            TRANSCRIPT
        );
        assert!(!m.folder.join(files::LOG).exists());
    }

    #[test]
    fn rename_speakers_without_a_transcript_does_nothing() {
        let f = Fixture::empty();
        f.write("2026-11-09 10-15-31", None, &[(files::SUMMARY, SUMMARY)]);
        let m = MeetingRecord::from_folder(&f.dir().join("2026-11-09 10-15-31")).unwrap();
        let names = HashMap::from([("Speaker 1".to_string(), "An".to_string())]);
        assert_eq!(m.rename_speakers(&names).unwrap(), []);
        assert!(!m.folder.join(files::TRANSCRIPT).exists());
        assert!(!m.folder.join(files::LOG).exists());
    }

    #[test]
    fn set_done_tasks_writes_compact_sorted_json() {
        let f = Fixture::new();
        let m = f.library().find("2026-10-19 22-20-58").unwrap();
        m.set_done_tasks(&BTreeSet::from([2, 0])).unwrap();
        assert_eq!(
            fs::read_to_string(m.folder.join(files::TASKS)).unwrap(),
            "[0,2]"
        );
        assert_eq!(m.done_tasks(), BTreeSet::from([0, 2]));
        m.set_done_tasks(&BTreeSet::new()).unwrap();
        assert_eq!(
            fs::read_to_string(m.folder.join(files::TASKS)).unwrap(),
            "[]"
        );
    }

    #[test]
    fn clear_tasks_removes_the_file_and_ignores_a_missing_one() {
        let f = Fixture::new();
        let m = f.library().find("2026-10-19 22-20-58").unwrap();
        assert!(m.folder.join(files::TASKS).exists());
        m.clear_tasks().unwrap();
        assert!(!m.folder.join(files::TASKS).exists());
        assert_eq!(m.done_tasks(), BTreeSet::new());
        m.clear_tasks().unwrap();
    }

    #[test]
    fn export_markdown_matches_r5() {
        assert_eq!(
            export_markdown("Họp", Some("# Họp\n\n## Tóm tắt\nx"), Some("[00:00] a")),
            "# Họp\n\n## Tóm tắt\nx\n\n---\n\n## Transcript\n\n[00:00] a"
        );
        assert_eq!(
            export_markdown("Họp", Some("# Họp\n\nx"), None),
            "# Họp\n\nx"
        );
        assert_eq!(
            export_markdown("Họp", None, Some("[00:00] a")),
            "# Họp\n\n---\n\n## Transcript\n\n[00:00] a"
        );
        assert_eq!(export_markdown("Họp", None, None), "# Họp");
    }

    #[test]
    fn export_file_name_is_safe() {
        assert_eq!(export_file_name("a/b: c?"), "a-b- c-.md");
        assert_eq!(export_file_name("\"...\""), "-...-.md");
        assert_eq!(export_file_name("..."), "Dictate.md");
        assert_eq!(export_file_name("  . .  "), "Dictate.md");
        assert_eq!(export_file_name(""), "Dictate.md");
        assert_eq!(export_file_name(" ..Họp kế hoạch.. "), "Họp kế hoạch.md");
        assert_eq!(
            export_file_name(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a-b-c-d-e-f-g-h-i-j.md"
        );
        assert_eq!(export_file_name("a\tb\nc\u{7f}d"), "a-b-c-d.md");
        let long = export_file_name(&"a".repeat(300));
        assert_eq!(long, format!("{}.md", "a".repeat(120)));
        // The cap counts characters, not bytes, and never leaves a space or dot at the end.
        let long = export_file_name(&format!("{} {}", "ệ".repeat(119), "x".repeat(50)));
        assert_eq!(long, format!("{}.md", "ệ".repeat(119)));
    }
}
