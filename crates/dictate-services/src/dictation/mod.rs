//! Dictation logic with no OS code and no I/O: the hotkey press and release state machine ([`machine`], section R1
//! of the plan 3 dictation reference), the rolling window of audio and the text built from it while recording
//! ([`live`], section R2), and the language lists and hotkey labels ([`lang`], sections R1 and R2).

pub mod lang;
pub mod live;
pub mod machine;
