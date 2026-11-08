//! A recording's transcript, summary and speakers.

pub mod speakers;
pub mod summary;
pub mod transcript;

pub use speakers::Speaker;
pub use summary::{ActionItem, Mark, Section, SummaryDoc};
pub use transcript::TranscriptLine;
