//! Recording from the default microphone.

use std::fmt;
use std::sync::mpsc::Sender;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample};

/// What the microphone reports while it records.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// One buffer of audio, mono, at the rate of [`Capture::sample_rate`].
    Samples(Vec<f32>),
    /// The recording has a problem; whoever listens decides what to do about it. `Silent` comes once while the
    /// stream goes on delivering buffers; `Stream` means the stream has broken off.
    Error(CaptureError),
}

/// Why the microphone cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// There is no microphone (or no audio system) on this machine.
    NoDevice,
    /// The stream could not start or broke off (the microphone was unplugged); the text is the system's detail.
    Stream(String),
    /// The microphone opened but delivers pure silence: on Windows the privacy switch for desktop apps is off.
    Silent,
}

/// Displays as the key of the text the UI shows for the error (the detail of `Stream` goes with it separately).
impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CaptureError::NoDevice => "audio.no_device",
            CaptureError::Stream(_) => "audio.stream",
            CaptureError::Silent => "audio.silent",
        })
    }
}

impl std::error::Error for CaptureError {}

/// Tells whether a microphone delivers nothing but silence at its start.
///
/// With the Windows privacy switch for desktop apps off, the microphone opens and runs but every sample is
/// exactly 0.0. A real microphone always has a little noise, so the first 1.5 seconds all being zero is the sign.
/// Only those first 1.5 seconds are looked at.
pub struct SilenceWatch {
    /// Samples that make 1.5 seconds.
    needed: usize,
    /// Zeros seen so far.
    zeros: usize,
    verdict: Verdict,
}

#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    Watching,
    Silent,
    Heard,
}

impl SilenceWatch {
    /// A watch for a stream of `rate` samples per second.
    pub fn new(rate: u32) -> Self {
        SilenceWatch {
            needed: (rate as usize * 3 / 2).max(1),
            zeros: 0,
            verdict: Verdict::Watching,
        }
    }

    /// Looks at the next buffer. True once the first 1.5 seconds were all exactly 0.0 (and then it stays true);
    /// never true after a sample that is not 0.0 within them.
    pub fn feed(&mut self, samples: &[f32]) -> bool {
        if self.verdict == Verdict::Watching {
            let wanted = (self.needed - self.zeros).min(samples.len());
            if samples[..wanted].iter().any(|&s| s != 0.0) {
                self.verdict = Verdict::Heard;
            } else {
                self.zeros += wanted;
                if self.zeros == self.needed {
                    self.verdict = Verdict::Silent;
                }
            }
        }
        self.verdict == Verdict::Silent
    }
}

/// One mono sample per frame of `channels` interleaved samples: their average, as a float from -1 to 1.
fn downmix<T: Copy>(data: &[T], channels: usize) -> Vec<f32>
where
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    data.chunks_exact(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
            sum / channels as f32
        })
        .collect()
}

/// Whether a stream error from the system ends the recording. A buffer overrun, a re-route to another device
/// and a refused real-time priority all leave the stream running.
fn stream_error_ends_recording(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

/// What the callback of the system's audio thread does with one buffer: make it mono, send it on, and watch for
/// a microphone that only delivers silence. It has no locks and does not log, as it runs in real time.
struct Tap {
    channels: usize,
    tx: Sender<CaptureEvent>,
    watch: SilenceWatch,
    silence_reported: bool,
}

impl Tap {
    fn new(channels: usize, rate: u32, tx: Sender<CaptureEvent>) -> Self {
        Tap {
            channels,
            tx,
            watch: SilenceWatch::new(rate),
            silence_reported: false,
        }
    }

    fn push<T: Copy>(&mut self, data: &[T])
    where
        f32: FromSample<T>,
    {
        let mono = downmix(data, self.channels);
        let silent = !self.silence_reported && self.watch.feed(&mono);
        // A closed channel means nobody listens any more: the recording is being stopped.
        let _ = self.tx.send(CaptureEvent::Samples(mono));
        if silent {
            self.silence_reported = true;
            let _ = self.tx.send(CaptureEvent::Error(CaptureError::Silent));
        }
    }
}

/// The callback for errors of the stream: forwards the first one that ends the recording. A backend that lost its
/// device may report it again with every callback, and the listener needs to hear of it only once.
fn error_handler(tx: Sender<CaptureEvent>) -> impl FnMut(cpal::Error) + Send + 'static {
    let mut reported = false;
    move |e: cpal::Error| {
        if !reported && stream_error_ends_recording(e.kind()) {
            reported = true;
            let _ = tx.send(CaptureEvent::Error(CaptureError::Stream(e.to_string())));
        }
    }
}

/// The error for a failure while opening the microphone.
fn start_error(e: &cpal::Error) -> CaptureError {
    match e.kind() {
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => CaptureError::NoDevice,
        _ => CaptureError::Stream(e.to_string()),
    }
}

/// A running recording from the default microphone. Dropping it, or [`stop`](Capture::stop), ends it.
pub struct Capture {
    stream: cpal::Stream,
    rate: u32,
}

impl Capture {
    /// Opens the default microphone and starts sending [`CaptureEvent`]s to `tx`, in the microphone's own format:
    /// its sample rate, its channels averaged to one. Reads 32-bit float, 16-bit and unsigned 16-bit samples.
    ///
    /// A stream that breaks later (the microphone is unplugged) arrives as `CaptureEvent::Error`.
    pub fn start(tx: Sender<CaptureEvent>) -> Result<Capture, CaptureError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(CaptureError::NoDevice)?;
        let supported = device.default_input_config().map_err(|e| start_error(&e))?;
        let rate = supported.sample_rate();
        let tap = Tap::new(usize::from(supported.channels()), rate, tx.clone());
        let on_error = error_handler(tx);
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => open::<f32>(&device, config, tap, on_error),
            SampleFormat::I16 => open::<i16>(&device, config, tap, on_error),
            SampleFormat::U16 => open::<u16>(&device, config, tap, on_error),
            other => {
                return Err(CaptureError::Stream(format!(
                    "the microphone delivers {other} samples, which are not supported"
                )))
            }
        }
        .map_err(|e| start_error(&e))?;
        stream.play().map_err(|e| start_error(&e))?;
        Ok(Capture { stream, rate })
    }

    /// The rate in Hz of the samples in [`CaptureEvent::Samples`].
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Stops the recording. Buffers already sent stay in the channel.
    pub fn stop(self) {
        drop(self.stream);
    }
}

/// Builds the input stream for samples of type `T`.
fn open<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut tap: Tap,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| tap.push(data),
        on_error,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// 1.5 seconds of silence at `rate`.
    fn silence(rate: u32) -> Vec<f32> {
        vec![0.0; rate as usize * 3 / 2]
    }

    #[test]
    fn capture_silence_is_detected() {
        let mut watch = SilenceWatch::new(16_000);
        assert!(watch.feed(&silence(16_000)));

        let mut with_a_whisper = silence(16_000);
        with_a_whisper[100] = 1e-6;
        let mut watch = SilenceWatch::new(16_000);
        assert!(!watch.feed(&with_a_whisper));
        // The verdict does not change with more silence after it.
        assert!(!watch.feed(&silence(16_000)));
    }

    #[test]
    fn silence_is_only_reported_after_the_full_1_5_seconds() {
        let mut watch = SilenceWatch::new(16_000);
        assert!(!watch.feed(&[0.0; 23_999]));
        assert!(watch.feed(&[0.0]));
    }

    #[test]
    fn silence_is_found_across_many_small_buffers() {
        let mut watch = SilenceWatch::new(48_000);
        let mut verdicts = (0..70).map(|_| watch.feed(&[0.0; 1_024]));
        // 70 x 1024 = 71 680 samples is just under 1.5 s at 48 kHz (72 000): not yet.
        assert!(!verdicts.any(|v| v));
        assert!(watch.feed(&[0.0; 1_024]));
    }

    #[test]
    fn a_sound_in_the_first_buffer_clears_the_microphone() {
        let mut watch = SilenceWatch::new(16_000);
        assert!(!watch.feed(&[0.0, 0.0, 0.25]));
        assert!(!watch.feed(&silence(16_000)));
    }

    #[test]
    fn a_sound_after_the_first_1_5_seconds_is_not_looked_at() {
        let mut buffer = silence(16_000);
        buffer.push(0.5);
        assert!(SilenceWatch::new(16_000).feed(&buffer));
    }

    #[test]
    fn errors_display_their_locale_keys() {
        assert_eq!(CaptureError::NoDevice.to_string(), "audio.no_device");
        assert_eq!(
            CaptureError::Stream("device gone".into()).to_string(),
            "audio.stream"
        );
        assert_eq!(CaptureError::Silent.to_string(), "audio.silent");
    }

    #[test]
    fn downmix_averages_the_channels_of_each_frame() {
        assert_eq!(
            downmix(&[1.0_f32, 0.0, 0.5, 0.5, -1.0, -0.5], 2),
            [0.5, 0.5, -0.75]
        );
    }

    #[test]
    fn downmix_of_mono_is_unchanged() {
        assert_eq!(downmix(&[0.25_f32, -0.5, 1.0], 1), [0.25, -0.5, 1.0]);
    }

    #[test]
    fn downmix_scales_i16_to_a_float() {
        assert_eq!(downmix(&[i16::MIN, 0, 16_384], 1), [-1.0, 0.0, 0.5]);
    }

    #[test]
    fn downmix_centres_u16_on_zero() {
        // An unsigned 16-bit microphone is silent at 32 768.
        assert_eq!(downmix(&[0_u16, 32_768, 49_152], 1), [-1.0, 0.0, 0.5]);
    }

    #[test]
    fn downmix_drops_a_trailing_partial_frame() {
        assert_eq!(downmix(&[0.2_f32, 0.4, 0.9], 2), [0.3]);
    }

    #[test]
    fn running_out_of_buffer_or_a_re_route_does_not_end_the_recording() {
        assert!(!stream_error_ends_recording(ErrorKind::Xrun));
        assert!(!stream_error_ends_recording(ErrorKind::DeviceChanged));
        assert!(!stream_error_ends_recording(ErrorKind::RealtimeDenied));
        assert!(stream_error_ends_recording(ErrorKind::DeviceNotAvailable));
        assert!(stream_error_ends_recording(ErrorKind::StreamInvalidated));
        assert!(stream_error_ends_recording(ErrorKind::BackendError));
    }

    #[test]
    fn a_lost_device_is_sent_as_a_stream_error() {
        let (tx, rx) = mpsc::channel();
        let mut on_error = error_handler(tx);
        on_error(cpal::Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "unplugged",
        ));
        assert_eq!(
            rx.try_recv().unwrap(),
            CaptureEvent::Error(CaptureError::Stream("unplugged".into()))
        );
    }

    #[test]
    fn only_the_first_stream_error_is_sent() {
        let (tx, rx) = mpsc::channel();
        let mut on_error = error_handler(tx);
        // A backend that has lost its device can report it on every callback.
        on_error(cpal::Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "unplugged",
        ));
        on_error(cpal::Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "unplugged again",
        ));
        on_error(cpal::Error::new(ErrorKind::StreamInvalidated));
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [CaptureEvent::Error(CaptureError::Stream(
                "unplugged".into()
            ))]
        );
    }

    #[test]
    fn an_ignored_error_does_not_use_up_the_one_that_is_sent() {
        let (tx, rx) = mpsc::channel();
        let mut on_error = error_handler(tx);
        on_error(cpal::Error::new(ErrorKind::Xrun));
        on_error(cpal::Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "unplugged",
        ));
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [CaptureEvent::Error(CaptureError::Stream(
                "unplugged".into()
            ))]
        );
    }

    #[test]
    fn a_buffer_overrun_is_not_sent() {
        let (tx, rx) = mpsc::channel();
        let mut on_error = error_handler(tx);
        on_error(cpal::Error::new(ErrorKind::Xrun));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn opening_failures_map_to_the_error_the_user_sees() {
        assert_eq!(
            start_error(&cpal::Error::new(ErrorKind::DeviceNotAvailable)),
            CaptureError::NoDevice
        );
        assert_eq!(
            start_error(&cpal::Error::new(ErrorKind::HostUnavailable)),
            CaptureError::NoDevice
        );
        assert_eq!(
            start_error(&cpal::Error::with_message(
                ErrorKind::PermissionDenied,
                "denied"
            )),
            CaptureError::Stream("denied".into())
        );
    }

    #[test]
    fn the_callback_sends_mono_buffers() {
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(2, 16_000, tx);
        tap.push(&[0.5_f32, 0.25, 0.1, 0.3]);
        assert_eq!(
            rx.try_recv().unwrap(),
            CaptureEvent::Samples(vec![0.375, 0.2])
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_callback_reports_a_silent_microphone_once() {
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(2, 16_000, tx);
        // 2 s of stereo i16 silence in 20 ms buffers (320 frames).
        for _ in 0..100 {
            tap.push(&[0_i16; 640]);
        }
        let events: Vec<CaptureEvent> = rx.try_iter().collect();
        let silent = events
            .iter()
            .filter(|e| **e == CaptureEvent::Error(CaptureError::Silent))
            .count();
        let buffers = events
            .iter()
            .filter(|e| matches!(e, CaptureEvent::Samples(s) if s.len() == 320))
            .count();
        assert_eq!(silent, 1);
        assert_eq!(buffers, 100, "every buffer is kept, silent or not");
    }

    #[test]
    fn the_callback_keeps_quiet_but_not_silent_audio() {
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(1, 16_000, tx);
        let mut hiss = [0_i16; 320];
        hiss[7] = 1;
        for _ in 0..100 {
            tap.push(&hiss);
        }
        assert!(rx.try_iter().all(|e| matches!(e, CaptureEvent::Samples(_))));
    }

    #[test]
    fn the_callback_survives_a_closed_channel() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let mut tap = Tap::new(1, 16_000, tx);
        tap.push(&[0.1_f32; 64]);
    }

    #[test]
    fn a_capture_can_move_to_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<Capture>();
    }

    /// Records from the real default microphone for two seconds. Needs a microphone, and on macOS the permission
    /// of the app that runs the test (the terminal).
    #[test]
    #[ignore = "needs a microphone"]
    fn capture_opens_default_device() {
        let (tx, rx) = mpsc::channel();
        let capture = Capture::start(tx).expect("the default microphone opens");
        let rate = capture.sample_rate();
        assert!(rate >= 8_000, "rate {rate}");

        let (mut samples, mut loudest, mut silent) = (0, 0.0_f32, false);
        let until = Instant::now() + Duration::from_secs(2);
        while let Some(wait) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(wait) {
                Ok(CaptureEvent::Samples(s)) => {
                    samples += s.len();
                    loudest = s.iter().fold(loudest, |m, x| m.max(x.abs()));
                }
                Ok(CaptureEvent::Error(CaptureError::Silent)) => silent = true,
                Ok(CaptureEvent::Error(e)) => panic!("the stream broke: {e:?}"),
                Err(_) => break,
            }
        }
        capture.stop();

        eprintln!("device rate {rate} Hz, {samples} samples in 2 s, loudest {loudest}, reported silent: {silent}");
        assert!(samples > rate as usize, "{samples} samples in 2 s");
    }
}
