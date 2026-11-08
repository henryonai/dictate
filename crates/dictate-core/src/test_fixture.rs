//! Test data shared by the meetings tests: a recording's summary and transcript, and a library of recordings.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use tempfile::TempDir;

use crate::meetings::library::{files, MeetingLibrary, SCREEN_SOURCE};

/// The summary of the screen recording "Họp kế hoạch refactor": 2 action items and a 2-mark timeline.
pub const SUMMARY: &str = r#"# Họp kế hoạch refactor

## Tóm tắt
Nhóm thống nhất tách repo test và quản lý task trên GitHub.

## Quyết định
- Tách repo test riêng

## Việc cần làm
- [ ] **Henry** — Tạo repo test mới (thứ Sáu)
- [ ] **Chưa rõ** — Viết tài liệu hướng dẫn

## Mốc thời gian
- [00:00] Mở đầu — chào hỏi
- [01:05] Kế hoạch refactor"#;

/// The transcript of the same recording: three paragraphs, two speakers.
pub const TRANSCRIPT: &str = r#"[00:00] Speaker 1: Chào mọi người, hôm nay mình bàn về kế hoạch refactor.

[01:05] Speaker 2: Mình đề xuất tách repo test ra riêng.

[02:10] Speaker 1: Đồng ý, Henry tạo repo trước thứ Sáu nhé."#;

/// A throwaway Dictate library with three recordings, removed when dropped:
/// - "2026-10-19 22-20-58": screen recording with summary, 2 action items (the first ticked off) and a transcript;
/// - "2026-10-18 11-50-17": a link with a transcript but no summary yet;
/// - "2026-10-17 09-00-00": no meta.json, only realtime captions.
pub struct Fixture {
    dir: TempDir,
}

impl Fixture {
    /// A library folder with no recording in it.
    pub fn empty() -> Fixture {
        Fixture {
            dir: tempfile::tempdir().expect("create a temp dir"),
        }
    }

    pub fn new() -> Fixture {
        let fixture = Fixture::empty();
        fixture.write(
            "2026-10-19 22-20-58",
            Some(json!({
                "title": "Họp kế hoạch refactor",
                "source": SCREEN_SOURCE,
                "created": 1_792_423_258.39,
                "duration": 373.55,
            })),
            &[
                (files::SUMMARY, SUMMARY),
                (files::TRANSCRIPT, TRANSCRIPT),
                (files::TASKS, "[0]"),
            ],
        );
        fixture.write(
            "2026-10-18 11-50-17",
            Some(json!({
                "title": "Video về Swift",
                "source": "https://youtube.com/watch?v=x",
                "created": 1_792_300_000.0,
            })),
            &[(files::TRANSCRIPT, "[00:00] Swift concurrency rất hay.")],
        );
        fixture.write(
            "2026-10-17 09-00-00",
            None,
            &[(files::LIVE_TRANSCRIPT, "[00:00] Bạn: thử ghi âm")],
        );
        fixture
    }

    /// The library folder; recordings are its sub-folders.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn library(&self) -> MeetingLibrary {
        MeetingLibrary::new(self.dir().to_path_buf())
    }

    /// Adds the recording folder `id` with a `meta.json` (unless `meta` is `None`) and the given files.
    pub fn write(&self, id: &str, meta: Option<Value>, contents: &[(&str, &str)]) {
        let folder = self.dir().join(id);
        fs::create_dir_all(&folder).expect("create the recording folder");
        if let Some(meta) = meta {
            fs::write(folder.join(files::META), meta.to_string()).expect("write meta.json");
        }
        for (name, text) in contents {
            fs::write(folder.join(name), text).expect("write a recording file");
        }
    }
}
