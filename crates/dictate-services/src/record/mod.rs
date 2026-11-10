//! Recording the screen: the audio of a recording as separate stems on the recording's own clock, and the mix of
//! them into the one `audio.m4a` that the rest of the app works from.

pub mod clock;
pub mod mix;
pub mod stem;
