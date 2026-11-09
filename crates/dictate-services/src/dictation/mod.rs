//! Dictation logic with no OS code and no I/O: the hotkey press and release state machine ([`machine`]), the
//! rolling window of audio and the text built from it while recording ([`live`]), and the language lists and
//! hotkey labels ([`lang`]). The work after the recording stops is in [`pipeline`], behind a trait that the app
//! implements with the network clients, so that it needs no network to be tested.

pub mod lang;
pub mod live;
pub mod machine;
pub mod pipeline;
