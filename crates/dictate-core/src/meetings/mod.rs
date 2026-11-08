//! A recording's transcript, summary and speakers, and the library that holds the recordings.

pub mod library;
pub mod speakers;
pub mod summary;
pub mod transcript;

pub use library::{Kind, MeetingLibrary, MeetingRecord};
pub use speakers::Speaker;
pub use summary::{ActionItem, Mark, Section, SummaryDoc};
pub use transcript::TranscriptLine;
