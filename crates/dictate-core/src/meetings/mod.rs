//! A recording's transcript, summary and speakers, and the library that holds the recordings.

pub mod library;
pub mod speakers;
pub mod summary;
pub mod transcript;
pub mod write;

pub use library::{Kind, MeetingLibrary, MeetingRecord, Meta};
pub use speakers::Speaker;
pub use summary::{ActionItem, Mark, Section, SummaryDoc};
pub use transcript::TranscriptLine;
pub use write::{append_log, atomic_write, export_file_name, export_markdown, transcript_text};
