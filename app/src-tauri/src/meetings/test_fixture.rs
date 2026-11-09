//! A throwaway library of recordings for the tests of the meetings module, removed when dropped.

use std::fs;
use std::path::{Path, PathBuf};

use dictate_core::meetings::library::{files, MeetingLibrary, SCREEN_SOURCE};
use serde_json::{json, Value};

/// The summary of the screen recording "Họp kế hoạch refactor": a decision, 2 action items and a 2-mark timeline.
pub const SUMMARY: &str = r#"# Họp kế hoạch refactor

## Tóm tắt
Nhóm thống nhất tách repo test và quản lý task trên GitHub.

## Ý chính
- Repo test riêng chạy nhanh hơn

## Quyết định
- Tách repo test riêng

## Việc cần làm
- [ ] **Henry** — Tạo repo test mới (thứ Sáu)
- [ ] **Chưa rõ** — Viết tài liệu hướng dẫn

## Câu hỏi còn mở
- Ai giữ quyền admin?

## Mốc thời gian
- [00:00] Mở đầu — chào hỏi
- [01:05] Kế hoạch refactor

## Ghi chú thêm
Gặp lại sau hai tuần."#;

/// Its transcript: three paragraphs, two speakers.
pub const TRANSCRIPT: &str = r#"[00:00] Speaker 1: Chào mọi người, hôm nay mình bàn về kế hoạch refactor.

[01:05] Speaker 2: Mình đề xuất tách repo test ra riêng.

[02:10] Speaker 1: Đồng ý, Henry tạo repo trước thứ Sáu nhé."#;

pub const SCREEN: &str = "2026-10-19 22-20-58";
pub const LINK: &str = "2026-10-18 11-50-17";
pub const FILE: &str = "2026-10-17 09-00-00";

pub struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    pub fn empty() -> Fixture {
        Fixture {
            dir: tempfile::tempdir().expect("a temp dir"),
        }
    }

    /// Three recordings, newest first:
    /// - `SCREEN`: a screen recording with a summary (the first of its 2 action items is ticked) and a transcript;
    /// - `LINK`: a link with a transcript and no summary;
    /// - `FILE`: an imported file called "Cuộc họp tuần" with a summary that has no action items, and no transcript.
    pub fn new() -> Fixture {
        let f = Fixture::empty();
        f.write(
            SCREEN,
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
        f.write(
            LINK,
            Some(json!({
                "title": "Video về Swift",
                "source": "https://www.youtube.com/watch?v=x",
                "created": 1_792_300_000.0,
            })),
            &[(files::TRANSCRIPT, "[00:00] Swift concurrency rất hay.")],
        );
        f.write(
            FILE,
            Some(json!({
                "title": "Cuộc họp tuần",
                "source": "/Users/Lan/Phim ảnh/họp tuần.m4a",
                "created": 1_792_000_000.0,
            })),
            &[(files::SUMMARY, "# Cuộc họp tuần\n\n## Tóm tắt\nNgắn gọn.")],
        );
        f
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn library(&self) -> MeetingLibrary {
        MeetingLibrary::new(self.dir().to_path_buf())
    }

    pub fn folder(&self, id: &str) -> PathBuf {
        self.dir().join(id)
    }

    /// Adds the recording `id` with a `meta.json` (unless `meta` is `None`) and the given files.
    pub fn write(&self, id: &str, meta: Option<Value>, contents: &[(&str, &str)]) {
        let folder = self.folder(id);
        fs::create_dir_all(&folder).expect("the recording's folder");
        if let Some(meta) = meta {
            fs::write(folder.join(files::META), meta.to_string()).expect("meta.json");
        }
        for (name, text) in contents {
            fs::write(folder.join(name), text).expect("a file of the recording");
        }
    }
}
