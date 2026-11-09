//! Microphone audio for dictation: capture at the device's own rate, a level for the meter, resampling to the
//! 16 kHz mono that Whisper wants, and WAV files of that.

pub mod capture;
pub mod level;
pub mod resample;
pub mod wav;
