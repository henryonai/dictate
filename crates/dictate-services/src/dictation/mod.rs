//! Dictation logic with no OS code and no I/O: the hotkey press and release state machine ([`machine`]), the
//! rolling window of audio and the text built from it while recording ([`live`]), and the language lists and
//! hotkey labels ([`lang`]).

pub mod lang;
pub mod live;
pub mod machine;
