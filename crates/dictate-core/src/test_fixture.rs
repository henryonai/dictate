//! Test data shared by the meetings tests: a recording's summary and transcript.

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
