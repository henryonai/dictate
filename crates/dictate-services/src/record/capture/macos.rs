//! The screen and the system's audio on macOS, from one ScreenCaptureKit stream.
//!
//! One `SCStream` serves both [`Track::Screen`] and [`Track::System`]: the first of the two that is started opens it, and the
//! last that is stopped closes it. Every start of a track is a [`Subscription`] to the stream that is running: the stream holds
//! `Routes`, which say where its pictures and its sound go, and a track that is started puts its channel there and takes it back
//! when it stops. So the system's audio can be stopped and started again while the screen goes on (the session does that when the
//! sound track is lost), and the stream is not touched. A stream that the system stops by itself (the display is unplugged, the
//! permission is taken away) is not joined again: the next start opens another.
//!
//! The stream exists once for the process, in `CURRENT`: the app records one thing at a time, and the capturer that the session
//! is given has no state of its own to keep it in.
//!
//! What the stream is: a display (without the windows of this app) or one window, at the size that `output_size` makes of its
//! size in pixels (ScreenCaptureKit scales), BGRA, at the frame rate of the quality. The sound is mono at 48 kHz, without what this
//! app plays. A recording of sound only has a stream too (ScreenCaptureKit has no other way to the system's audio): of the main
//! display, of 2 x 2 pixels and a frame a second, and what it delivers as pictures is thrown away.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use objc2_core_graphics::CGMainDisplayID;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::error::{SCError, SCStreamErrorCode};
use screencapturekit::prelude::{
    CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, PixelFormat, SCContentFilter,
    SCDisplay, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegateTrait, SCStreamOutputType,
};
use screencapturekit::utils::completion::is_timeout_error;

use super::{SourceInfo, Sources};
use crate::audio::capture::Stamper;
use crate::record::quality::output_size;
use crate::record::video::Frame;
use crate::record::{
    EndReason, Mode, RecordOptions, RunningSource, SourceEvent, StartError, Track,
};

/// The rate that the system's audio is asked for, which ScreenCaptureKit supports, and the one that its buffers are taken to
/// have when they do not say.
const AUDIO_RATE: u32 = 48_000;
/// The size of the pictures of a stream that is only listened to.
const SOUND_ONLY_SIZE: (u32, u32) = (2, 2);
/// How long the end of a capture is waited for. ScreenCaptureKit answers in a few tens of milliseconds; the crate would wait
/// 30 s for an answer that does not come, and `stop` must not.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// The windows of the picker, as the system describes them.
struct WindowFacts {
    id: u32,
    layer: i32,
    /// In points.
    width: f64,
    pid: Option<i32>,
    app: Option<String>,
    title: Option<String>,
}

/// The displays of the picker, as the system describes them.
struct DisplayFacts {
    id: u32,
    width: u32,
    height: u32,
}

/// What the stream captures: a display or a window.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Display,
    Window,
}

/// What the picker calls a window: `"<App> — <Title>"`, or the app alone when the window has no title, and `?` for an app that the
/// system does not name (as the Swift app did).
fn window_name(app: Option<&str>, title: Option<&str>) -> String {
    let app = app.filter(|app| !app.is_empty()).unwrap_or("?");
    match title.filter(|title| !title.is_empty()) {
        Some(title) => format!("{app} — {title}"),
        None => app.to_owned(),
    }
}

/// The windows that the picker offers, sorted by name (then by number, to keep the order the same from one look to the next): an
/// ordinary window (layer 0) that is more than 120 points wide, and not one of Dictate's own (`own_pid`).
fn listed_windows(windows: &[WindowFacts], own_pid: i32) -> Vec<SourceInfo> {
    let mut listed: Vec<(u32, SourceInfo)> = windows
        .iter()
        .filter(|window| window.layer == 0 && window.width > 120.0 && window.pid != Some(own_pid))
        .map(|window| {
            let info = SourceInfo {
                id: window.id.to_string(),
                name: window_name(window.app.as_deref(), window.title.as_deref()),
                app: window.app.clone().filter(|app| !app.is_empty()),
            };
            (window.id, info)
        })
        .collect();
    listed.sort_by(|(a_id, a), (b_id, b)| a.name.cmp(&b.name).then(a_id.cmp(b_id)));
    listed.into_iter().map(|(_, info)| info).collect()
}

/// The displays that the picker offers, the main one (`main`) first and the others as the system lists them, numbered from 1.
fn listed_displays(displays: &[DisplayFacts], main: u32) -> Vec<SourceInfo> {
    let mut ordered: Vec<&DisplayFacts> = displays.iter().collect();
    // A stable sort: `false` (the main display) comes before `true`.
    ordered.sort_by_key(|display| display.id != main);
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, display)| SourceInfo {
            id: display.id.to_string(),
            name: format!(
                "Display {} ({}x{})",
                index + 1,
                display.width,
                display.height
            ),
            app: None,
        })
        .collect()
}

/// Which of the displays (by number, in the order the system lists them) a recording takes: the one that `wanted` names, and none when
/// it names one that is not there (a display that was unplugged is not replaced by another); the main one (`main`), or else the
/// first, when no display is asked for. `None` when there is nothing to take.
fn choose_display(ids: &[u32], wanted: Option<&str>, main: u32) -> Option<usize> {
    match wanted {
        Some(wanted) => {
            let wanted = wanted.parse::<u32>().ok()?;
            ids.iter().position(|id| *id == wanted)
        }
        None => ids
            .iter()
            .position(|id| *id == main)
            .or_else(|| (!ids.is_empty()).then_some(0)),
    }
}

/// Why the system would not say what there is to record. Without Screen Recording it fails with a text and no kind of
/// error, so a text that is not about a call that was not answered is taken for that.
fn content_error(error: &SCError) -> StartError {
    match error {
        SCError::NoShareableContent(text) if !is_timeout_error(text) => {
            StartError::NeedsScreenPermission
        }
        other => start_failure(other),
    }
}

/// Why a stream could not start, as the error of a start.
fn start_failure(error: &SCError) -> StartError {
    match error {
        SCError::PermissionDenied(_)
        | SCError::SCStreamError {
            code: SCStreamErrorCode::UserDeclined,
            ..
        } => StartError::NeedsScreenPermission,
        other => StartError::Source(other.to_string()),
    }
}

/// How a recording of `target` ends when the system stops its stream with `error`. The crate does not know the codes for a display
/// that was unplugged or a window that was closed (Apple's `SCStreamError` has none): both are taken to come as "no capture
/// source", which is what a stream is left with.
fn stream_end(error: &SCError, target: Target) -> EndReason {
    match error.stream_error_code() {
        Some(SCStreamErrorCode::UserDeclined) => EndReason::PermissionRevoked,
        Some(SCStreamErrorCode::NoCaptureSource) => match target {
            Target::Display => EndReason::DisplayGone,
            Target::Window => EndReason::WindowClosed,
        },
        _ => EndReason::Failed(error.to_string()),
    }
}

/// How the system's audio ends, alone, when the system stops its stream with `error`: a track that is lost, which the session takes
/// up again with a new stream, unless the permission was taken away.
fn audio_end(error: &SCError) -> EndReason {
    match error.stream_error_code() {
        Some(SCStreamErrorCode::UserDeclined) => EndReason::PermissionRevoked,
        _ => EndReason::DeviceLost(error.to_string()),
    }
}

/// Whether a buffer of sound is what `mono_from_buffers` reads: 32-bit floats, in the byte order of this machine.
fn is_native_float32(is_float: bool, bits_per_channel: Option<u32>, big_endian: bool) -> bool {
    is_float && bits_per_channel == Some(32) && !big_endian
}

/// One mono sample for each frame of the buffers of a sound (`(channels, bytes of 32-bit floats)` each): the average of the channels.
/// ScreenCaptureKit gives its buffers planar (a buffer for each channel) unless asked for one channel, but a buffer of interleaved
/// channels is no harder. A frame that is not whole in every buffer is dropped.
fn mono_from_buffers(buffers: &[(u32, &[u8])]) -> Vec<f32> {
    const SAMPLE: usize = size_of::<f32>();
    let channels: usize = buffers
        .iter()
        .map(|(count, _)| (*count).max(1) as usize)
        .sum();
    let frames = buffers
        .iter()
        .map(|(count, bytes)| bytes.len() / (SAMPLE * (*count).max(1) as usize))
        .min()
        .unwrap_or(0);
    let mut mono = vec![0.0_f32; frames];
    for (count, bytes) in buffers {
        let frame = SAMPLE * (*count).max(1) as usize;
        for (sum, channels_of_frame) in mono.iter_mut().zip(bytes.chunks_exact(frame)) {
            for sample in channels_of_frame.chunks_exact(SAMPLE) {
                *sum += f32::from_ne_bytes([sample[0], sample[1], sample[2], sample[3]]);
            }
        }
    }
    for sample in &mut mono {
        *sample /= channels as f32;
    }
    mono
}

/// The size of `points` of a screen in pixels, at `scale` pixels to the point. The system says 0 (or nothing) before macOS 14: that
/// is a pixel to the point.
fn source_pixels(points: (f64, f64), scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let pixels = |side: f64| (side * scale).round().max(1.0) as u32;
    (pixels(points.0), pixels(points.1))
}

/// Where the system's audio goes: one track, started and stopped on its own.
struct AudioRoute {
    tx: Sender<SourceEvent>,
    stamper: Stamper,
    /// The rate of the first buffer: the rate of the track, which does not change.
    rate: Option<u32>,
}

/// Where what the one stream delivers goes. The tracks that listen come and go while the stream runs: the screen and the system's
/// audio each hand their channel in (`attach`) when they start and take it back (`detach`) when they stop, and the stream is not
/// touched. A lock is held only to hand a buffer over (an unbounded channel does not wait), so the callbacks of the system do not
/// wait for anything, and once `detach` has the lock a track gets nothing more.
struct Routes {
    video: Mutex<Option<Sender<SourceEvent>>>,
    audio: Mutex<Option<AudioRoute>>,
    /// False once the system has stopped the stream by itself.
    alive: AtomicBool,
    target: Target,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Routes {
    fn new(target: Target) -> Routes {
        Routes {
            video: Mutex::new(None),
            audio: Mutex::new(None),
            alive: AtomicBool::new(true),
            target,
        }
    }

    /// `track` is sent to `tx` from now on, in place of what it was sent to. The microphone is not the stream's.
    fn attach(&self, track: Track, tx: Sender<SourceEvent>) {
        match track {
            Track::Screen => *lock(&self.video) = Some(tx),
            Track::System => {
                *lock(&self.audio) = Some(AudioRoute {
                    tx,
                    stamper: Stamper::default(),
                    rate: None,
                });
            }
            Track::Mic => {}
        }
    }

    /// `track` is sent nowhere. It returns when no callback is still handing it anything.
    fn detach(&self, track: Track) {
        match track {
            Track::Screen => *lock(&self.video) = None,
            Track::System => *lock(&self.audio) = None,
            Track::Mic => {}
        }
    }

    /// Whether the screen listens: a frame is copied only when it does.
    fn wants_frames(&self) -> bool {
        lock(&self.video).is_some()
    }

    fn frame(&self, frame: Frame) {
        if let Some(tx) = lock(&self.video).as_ref() {
            // A session that is over has no one to read.
            let _ = tx.send(SourceEvent::Frame(frame));
        }
    }

    /// A buffer of the system's audio, mono, at `rate`, that arrived at `arrived`. A rate that is not the one of the first buffer
    /// ends the track, as a lost device does: it is taken up again at its new rate.
    fn audio(&self, arrived: Instant, rate: u32, mono: Vec<f32>) {
        let mut route = lock(&self.audio);
        let Some(listening) = route.as_mut() else {
            return;
        };
        if let Some(first) = listening.rate.filter(|first| *first != rate) {
            Self::lose_audio(
                &mut route,
                format!("the system's audio changed its rate from {first} Hz to {rate} Hz"),
            );
            return;
        }
        listening.rate = Some(rate);
        let at = listening.stamper.stamp(arrived, mono.len(), rate);
        let _ = listening.tx.send(SourceEvent::Audio {
            track: Track::System,
            at,
            rate,
            mono,
        });
    }

    /// The system's audio cannot go on as it is (`why`): the track ends as a lost device does, and the session takes it up again.
    fn audio_lost(&self, why: String) {
        Self::lose_audio(&mut lock(&self.audio), why);
    }

    fn lose_audio(route: &mut Option<AudioRoute>, why: String) {
        if let Some(listening) = route.take() {
            let _ = listening.tx.send(SourceEvent::Ended {
                track: Track::System,
                reason: EndReason::DeviceLost(why),
            });
        }
    }

    /// The system stopped the stream with `error`. The screen, when it listens, ends with it, and that ends the recording; when
    /// only the sound listens, it is a track that was lost.
    fn stopped(&self, error: &SCError) {
        self.alive.store(false, Ordering::Release);
        if let Some(tx) = lock(&self.video).as_ref() {
            let _ = tx.send(SourceEvent::Ended {
                track: Track::Screen,
                reason: stream_end(error, self.target),
            });
            return;
        }
        if let Some(listening) = lock(&self.audio).as_ref() {
            let _ = listening.tx.send(SourceEvent::Ended {
                track: Track::System,
                reason: audio_end(error),
            });
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

/// Hears of the stream being stopped by the system, which only ever happens with an error.
struct StopWatch(Arc<Routes>);

impl SCStreamDelegateTrait for StopWatch {
    fn did_stop_with_error(&self, error: SCError) {
        self.0.stopped(&error);
    }
}

/// The ScreenCaptureKit stream that is running, and where it sends what it delivers. It is stopped when it is dropped, which is
/// when the last [`Subscription`] to it is.
struct Stream {
    stream: Mutex<Option<SCStream>>,
    routes: Arc<Routes>,
    /// Whether the stream carries a video for the screen track, and the system's audio.
    video: bool,
    audio: bool,
}

/// The stream that is running, if any. Weak: the tracks that listen to it keep it, and it ends with the last of them.
static CURRENT: Mutex<Weak<Stream>> = Mutex::new(Weak::new());

impl Stream {
    /// Opens a stream for what `options` ask and starts it. Asks for Screen Recording, which is the first thing to fail without it.
    fn open(options: &RecordOptions) -> Result<Stream, StartError> {
        let content = SCShareableContent::get().map_err(|error| content_error(&error))?;
        let own_pid = own_pid();
        let own = content
            .applications()
            .into_iter()
            .find(|app| app.process_id() == own_pid);
        // This app's windows (the recording's own panel among them) are not in the picture, nor are those it will open later.
        let excluded: Vec<&SCRunningApplication> = own.iter().collect();

        let (filter, target, fallback_points) = match &options.mode {
            Mode::Window { id } => {
                let window = id
                    .parse::<u32>()
                    .ok()
                    .and_then(|id| {
                        content
                            .windows()
                            .into_iter()
                            .find(|window| window.window_id() == id)
                    })
                    .ok_or(StartError::WindowGone)?;
                let frame = window.frame();
                let filter = SCContentFilter::create()
                    .with_window(&window)
                    .build()
                    .map_err(|error| start_failure(&error))?;
                (
                    filter,
                    Target::Window,
                    (frame.size.width, frame.size.height),
                )
            }
            Mode::Screen { display } => display_filter(&content, display.as_deref(), &excluded)?,
            Mode::Audio => display_filter(&content, None, &excluded)?,
        };

        let video = options.mode.has_video();
        let spec = options.quality.spec();
        let (width, height) = if video {
            let rect = filter.content_rect();
            let points = if rect.size.width > 0.0 && rect.size.height > 0.0 {
                (rect.size.width, rect.size.height)
            } else {
                fallback_points
            };
            output_size(source_pixels(points, filter.point_pixel_scale()), &spec)
        } else {
            SOUND_ONLY_SIZE
        };
        let mut config = SCStreamConfiguration::new()
            .with_pixel_format(PixelFormat::BGRA)
            .with_width(width)
            .with_height(height)
            .with_scales_to_fit(true)
            .with_shows_cursor(video)
            .with_fps(if video { spec.fps } else { 1 });
        if options.system_audio {
            config = config
                .with_captures_audio(true)
                .with_excludes_current_process_audio(true)
                .with_sample_rate(AUDIO_RATE as i32)
                .with_channel_count(1);
        }

        let routes = Arc::new(Routes::new(target));
        let mut stream = SCStream::new_with_delegate(&filter, &config, StopWatch(routes.clone()))
            .map_err(|error| start_failure(&error))?;
        let pictures = routes.clone();
        stream
            .add_output_handler(
                move |sample: CMSampleBuffer, _: SCStreamOutputType| on_picture(&pictures, &sample),
                SCStreamOutputType::Screen,
            )
            .map_err(|error| start_failure(&error))?;
        if options.system_audio {
            let sound = routes.clone();
            stream
                .add_output_handler(
                    move |sample: CMSampleBuffer, _: SCStreamOutputType| on_sound(&sound, &sample),
                    SCStreamOutputType::Audio,
                )
                .map_err(|error| start_failure(&error))?;
        }
        stream
            .start_capture()
            .map_err(|error| start_failure(&error))?;
        Ok(Stream {
            stream: Mutex::new(Some(stream)),
            routes,
            video,
            audio: options.system_audio,
        })
    }
}

impl Drop for Stream {
    /// Stops the capture, and waits for it for [`STOP_WAIT`] at the most: the stop is on a thread of its own, so that a system that
    /// does not answer does not hold the session's thread.
    fn drop(&mut self) {
        let Some(stream) = lock(&self.stream).take() else {
            return;
        };
        if !self.routes.is_alive() {
            // The system has stopped it already.
            return;
        }
        let (done, finished) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("stop the screen capture".into())
            .spawn(move || {
                // Whatever it says, there is nothing to do about it.
                let _ = stream.stop_capture();
                drop(stream);
                let _ = done.send(());
            });
        if spawned.is_ok() {
            let _ = finished.recv_timeout(STOP_WAIT);
        }
    }
}

/// The display that a recording of `wanted` is about (`None` is the main one), and the filter, target and size in points of it.
fn display_filter(
    content: &SCShareableContent,
    wanted: Option<&str>,
    excluded: &[&SCRunningApplication],
) -> Result<(SCContentFilter, Target, (f64, f64)), StartError> {
    let displays = content.displays();
    let ids: Vec<u32> = displays.iter().map(SCDisplay::display_id).collect();
    let display = choose_display(&ids, wanted, CGMainDisplayID())
        .and_then(|at| displays.get(at))
        .ok_or_else(|| {
            StartError::Source(
                if wanted.is_some() {
                    "that display is not connected any more"
                } else {
                    "no display found to record"
                }
                .into(),
            )
        })?;
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_applications(excluded, &[])
        .build()
        .map_err(|error| start_failure(&error))?;
    let points = (f64::from(display.width()), f64::from(display.height()));
    Ok((filter, Target::Display, points))
}

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(-1)
}

/// BGRA, as the four letters that the system names the format with.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// A picture from the screen: copied, with its stride, and sent to the screen track, when it has one and the picture is new.
fn on_picture(routes: &Routes, sample: &CMSampleBuffer) {
    if !routes.wants_frames() {
        return;
    }
    // The system sends a frame when nothing changed too, with the status "idle" and no picture: what was sent last stays.
    if sample
        .frame_status()
        .is_some_and(|status| !status.has_content())
    {
        return;
    }
    let Some(buffer) = sample.pixel_buffer() else {
        return;
    };
    if buffer.pixel_format() != BGRA {
        return;
    }
    let Ok(pixels) = buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
        return;
    };
    // SAFETY: the buffer is locked for reading until `pixels` is dropped, which is after the bytes are copied.
    let Some(bytes) = (unsafe { pixels.as_slice() }) else {
        return;
    };
    routes.frame(Frame {
        width: u32::try_from(pixels.width()).unwrap_or(0),
        height: u32::try_from(pixels.height()).unwrap_or(0),
        stride: u32::try_from(pixels.bytes_per_row()).unwrap_or(0),
        bgra: bytes.to_vec(),
    });
}

/// A buffer of the system's audio: made mono, and sent to the system's track, when it has one.
fn on_sound(routes: &Routes, sample: &CMSampleBuffer) {
    let arrived = Instant::now();
    let format = sample.format_description();
    if let Some(format) = &format {
        if !is_native_float32(
            format.audio_is_float(),
            format.audio_bits_per_channel(),
            format.audio_is_big_endian(),
        ) {
            routes.audio_lost("the system's audio is not in 32-bit floats".into());
            return;
        }
    }
    let rate = format
        .and_then(|format| format.audio_sample_rate())
        .map_or(AUDIO_RATE, |rate| rate.round() as u32);
    let Ok(list) = sample.audio_buffer_list() else {
        return;
    };
    let buffers: Vec<(u32, &[u8])> = list
        .iter()
        .map(|buffer| (buffer.number_channels, buffer.data()))
        .collect();
    let mono = mono_from_buffers(&buffers);
    if !mono.is_empty() {
        routes.audio(arrived, rate, mono);
    }
}

/// A track that listens to the stream: stopping it, or dropping it, takes the track off, and the stream ends with its last one.
struct Subscription {
    stream: Arc<Stream>,
    track: Track,
}

impl RunningSource for Subscription {
    fn stop(self: Box<Self>) {
        // `drop` takes the track off, and the stream goes if this was its last.
        drop(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // The session stops a source before it starts the same track again, so this never takes off a newer one.
        self.stream.routes.detach(self.track);
    }
}

/// The stream that is running, or a new one for `options`.
fn join_or_open(options: &RecordOptions) -> Result<Arc<Stream>, StartError> {
    // A stream that is dropped is stopped, which takes a moment: not while `CURRENT` is held (declared first, so dropped last).
    let stale;
    let mut current = lock(&CURRENT);
    match current.upgrade() {
        Some(stream) if stream.routes.is_alive() => return Ok(stream),
        other => stale = other,
    }
    let stream = Arc::new(Stream::open(options)?);
    *current = Arc::downgrade(&stream);
    drop(current);
    drop(stale);
    Ok(stream)
}

/// Starts `track` (the screen or the system's audio) on the stream that is running, or on a new one.
pub fn start(
    track: Track,
    options: &RecordOptions,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    match track {
        Track::Screen if !options.mode.has_video() => {
            return Err(StartError::Source(
                "the screen is not recorded in a recording of sound".into(),
            ));
        }
        Track::System if !options.system_audio => {
            return Err(StartError::Source(
                "the system's audio is not asked for".into(),
            ));
        }
        Track::Mic => {
            return Err(StartError::Source(
                "the microphone is not a part of the screen capture".into(),
            ));
        }
        Track::Screen | Track::System => {}
    }
    let stream = join_or_open(options)?;
    let carried = match track {
        Track::Screen => stream.video,
        _ => stream.audio,
    };
    if !carried {
        return Err(StartError::Source(
            "the screen capture that is running was not set up for that".into(),
        ));
    }
    stream.routes.attach(track, tx);
    Ok(Box::new(Subscription { stream, track }))
}

/// The displays and windows that can be recorded, for the picker: the windows as the Swift app listed them (those on screen,
/// of layer 0, wider than 120 points, and not this app's own).
pub fn list_sources() -> Result<Sources, StartError> {
    let content = SCShareableContent::create()
        .with_exclude_desktop_windows(true)
        .with_on_screen_windows_only(true)
        .get()
        .map_err(|error| content_error(&error))?;
    let displays: Vec<DisplayFacts> = content
        .displays()
        .iter()
        .map(|display| DisplayFacts {
            id: display.display_id(),
            width: display.width(),
            height: display.height(),
        })
        .collect();
    let windows: Vec<WindowFacts> = content
        .windows()
        .iter()
        .map(|window| {
            let app = window.owning_application();
            WindowFacts {
                id: window.window_id(),
                layer: window.window_layer(),
                width: window.frame().size.width,
                pid: app.as_ref().map(SCRunningApplication::process_id),
                app: app.as_ref().map(SCRunningApplication::application_name),
                title: window.title(),
            }
        })
        .collect();
    Ok(Sources {
        displays: listed_displays(&displays, CGMainDisplayID()),
        windows: listed_windows(&windows, own_pid()),
        picker_is_the_system: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn window(id: u32, app: &str, title: &str) -> WindowFacts {
        WindowFacts {
            id,
            layer: 0,
            width: 800.0,
            pid: Some(100 + id as i32),
            app: Some(app.into()),
            title: Some(title.into()),
        }
    }

    fn info(id: &str, name: &str, app: Option<&str>) -> SourceInfo {
        SourceInfo {
            id: id.into(),
            name: name.into(),
            app: app.map(Into::into),
        }
    }

    #[test]
    fn window_names_follow_the_swift_format() {
        assert_eq!(window_name(Some("Safari"), Some("Apple")), "Safari — Apple");
        // A window without a title is the app alone.
        assert_eq!(window_name(Some("Finder"), Some("")), "Finder");
        assert_eq!(window_name(Some("Finder"), None), "Finder");
        // An app that the system does not name is a question mark.
        assert_eq!(window_name(None, Some("Untitled")), "? — Untitled");
        assert_eq!(window_name(None, None), "?");
        assert_eq!(window_name(Some(""), Some("Untitled")), "? — Untitled");
    }

    #[test]
    fn windows_of_dictate_itself_are_not_listed() {
        let mut own = window(1, "Dictate", "Settings");
        own.pid = Some(4242);

        let listed = listed_windows(&[own, window(2, "Safari", "Apple")], 4242);

        assert_eq!(listed, vec![info("2", "Safari — Apple", Some("Safari"))]);
    }

    #[test]
    fn only_ordinary_windows_that_are_wide_enough_are_listed() {
        let mut menu_bar = window(1, "Dock", "");
        menu_bar.layer = 25;
        let mut narrow = window(2, "Safari", "Narrow");
        narrow.width = 120.0;
        let mut wide_enough = window(3, "Safari", "Wide");
        wide_enough.width = 120.5;

        let listed = listed_windows(&[menu_bar, narrow, wide_enough], 1);

        assert_eq!(listed, vec![info("3", "Safari — Wide", Some("Safari"))]);
    }

    #[test]
    fn a_window_without_an_app_is_listed_under_a_question_mark() {
        let mut orphan = window(7, "", "Loose");
        orphan.app = None;
        orphan.pid = None;

        assert_eq!(
            listed_windows(&[orphan], 1),
            vec![info("7", "? — Loose", None)]
        );
    }

    #[test]
    fn windows_are_sorted_by_name_and_then_by_number() {
        let listed = listed_windows(
            &[
                window(9, "Safari", "Apple"),
                window(4, "Notes", ""),
                window(3, "Safari", "Apple"),
                window(5, "Arc", "Home"),
            ],
            1,
        );

        let names: Vec<(&str, &str)> = listed
            .iter()
            .map(|w| (w.id.as_str(), w.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("5", "Arc — Home"),
                ("4", "Notes"),
                ("3", "Safari — Apple"),
                ("9", "Safari — Apple")
            ]
        );
    }

    #[test]
    fn displays_are_numbered_with_the_main_one_first() {
        let displays = [
            DisplayFacts {
                id: 11,
                width: 1920,
                height: 1080,
            },
            DisplayFacts {
                id: 22,
                width: 2560,
                height: 1440,
            },
            DisplayFacts {
                id: 33,
                width: 1280,
                height: 720,
            },
        ];

        let listed = listed_displays(&displays, 22);

        assert_eq!(
            listed,
            vec![
                info("22", "Display 1 (2560x1440)", None),
                info("11", "Display 2 (1920x1080)", None),
                info("33", "Display 3 (1280x720)", None),
            ]
        );
    }

    #[test]
    fn displays_keep_the_systems_order_when_the_main_one_is_not_among_them() {
        let displays = [
            DisplayFacts {
                id: 11,
                width: 100,
                height: 50,
            },
            DisplayFacts {
                id: 22,
                width: 200,
                height: 100,
            },
        ];

        let ids: Vec<String> = listed_displays(&displays, 99)
            .into_iter()
            .map(|d| d.id)
            .collect();

        assert_eq!(ids, ["11", "22"]);
    }

    #[test]
    fn a_permission_error_maps_to_needs_screen_permission() {
        // What `SCShareableContent::get` says when Screen Recording is not allowed: all it gives is the text.
        let declined = SCError::NoShareableContent(
            "The user declined TCCs for application, window, display capture".into(),
        );
        assert_eq!(content_error(&declined), StartError::NeedsScreenPermission);
        assert_eq!(
            content_error(&SCError::permission_denied("Screen Recording")),
            StartError::NeedsScreenPermission
        );
        assert_eq!(
            content_error(&SCError::from_stream_error_code(
                SCStreamErrorCode::UserDeclined
            )),
            StartError::NeedsScreenPermission
        );
    }

    #[test]
    fn an_error_that_is_not_about_permission_says_what_the_system_said() {
        let too_old = SCError::feature_not_available("audio capture", "13.0");
        assert_eq!(
            content_error(&too_old),
            StartError::Source(too_old.to_string())
        );
        let internal = SCError::from_stream_error_code(SCStreamErrorCode::InternalError);
        assert_eq!(
            content_error(&internal),
            StartError::Source(internal.to_string())
        );
        // A system that did not answer is not one that said no.
        let silent = SCError::NoShareableContent(
            "screencapturekit: completion callback did not fire within 30s".into(),
        );
        assert_eq!(
            content_error(&silent),
            StartError::Source(silent.to_string())
        );
    }

    #[test]
    fn a_stream_without_a_source_ends_as_the_display_or_the_window_that_went() {
        let gone = SCError::from_stream_error_code(SCStreamErrorCode::NoCaptureSource);

        assert_eq!(stream_end(&gone, Target::Display), EndReason::DisplayGone);
        assert_eq!(stream_end(&gone, Target::Window), EndReason::WindowClosed);
    }

    #[test]
    fn a_stream_that_lost_the_permission_ends_as_revoked() {
        let declined = SCError::from_stream_error_code(SCStreamErrorCode::UserDeclined);

        assert_eq!(
            stream_end(&declined, Target::Display),
            EndReason::PermissionRevoked
        );
        assert_eq!(audio_end(&declined), EndReason::PermissionRevoked);
    }

    #[test]
    fn any_other_end_of_a_stream_says_what_the_system_said() {
        let stopped = SCError::from_stream_error_code_with_message(
            SCStreamErrorCode::UserStopped,
            "stopped from the menu bar",
        );

        assert_eq!(
            stream_end(&stopped, Target::Window),
            EndReason::Failed(stopped.to_string())
        );
        // The sound of a stream that ends is a track that is lost: the session may take it up again.
        assert_eq!(
            audio_end(&stopped),
            EndReason::DeviceLost(stopped.to_string())
        );
        let plain = SCError::StreamError("boom".into());
        assert_eq!(
            stream_end(&plain, Target::Display),
            EndReason::Failed(plain.to_string())
        );
    }

    #[test]
    fn a_named_display_is_taken_and_a_named_display_that_is_gone_is_not_replaced() {
        let ids = [11, 22, 33];

        assert_eq!(choose_display(&ids, Some("33"), 22), Some(2));
        // Unplugged since the picker was opened: the recording does not begin on another screen.
        assert_eq!(choose_display(&ids, Some("44"), 22), None);
        assert_eq!(choose_display(&ids, Some("main"), 22), None);
    }

    #[test]
    fn no_display_asked_for_is_the_main_one_or_else_the_first() {
        assert_eq!(choose_display(&[11, 22, 33], None, 22), Some(1));
        assert_eq!(choose_display(&[11, 22, 33], None, 99), Some(0));
        assert_eq!(choose_display(&[], None, 22), None);
    }

    #[test]
    fn a_stream_that_could_not_start_says_what_the_system_said_or_that_it_may_not() {
        let failed = SCError::CaptureStartFailed("could not be started".into());
        assert_eq!(
            start_failure(&failed),
            StartError::Source(failed.to_string())
        );
        assert_eq!(
            start_failure(&SCError::permission_denied("Screen Recording")),
            StartError::NeedsScreenPermission
        );
        assert_eq!(
            start_failure(&SCError::from_stream_error_code(
                SCStreamErrorCode::UserDeclined
            )),
            StartError::NeedsScreenPermission
        );
    }

    #[test]
    fn only_floats_of_32_bits_in_the_order_of_this_machine_are_read() {
        assert!(is_native_float32(true, Some(32), false));
        assert!(!is_native_float32(false, Some(32), false), "integers");
        assert!(!is_native_float32(true, Some(64), false), "doubles");
        assert!(
            !is_native_float32(true, Some(32), true),
            "the other byte order"
        );
        assert!(!is_native_float32(true, None, false));
    }

    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_ne_bytes()).collect()
    }

    #[test]
    fn planar_stereo_is_averaged_channel_against_channel() {
        let left = bytes(&[1.0, 0.5, -1.0]);
        let right = bytes(&[0.0, 0.5, 0.5]);

        let mono = mono_from_buffers(&[(1, &left), (1, &right)]);

        assert_eq!(mono, [0.5, 0.5, -0.25]);
    }

    #[test]
    fn interleaved_stereo_is_averaged_frame_by_frame() {
        let both = bytes(&[1.0, 0.0, 0.5, 0.5, -1.0, 0.5]);

        let mono = mono_from_buffers(&[(2, &both)]);

        assert_eq!(mono, [0.5, 0.5, -0.25]);
    }

    #[test]
    fn mono_is_left_as_it_is() {
        let one = bytes(&[0.25, -0.5]);

        assert_eq!(mono_from_buffers(&[(1, &one)]), [0.25, -0.5]);
    }

    #[test]
    fn channels_of_unequal_length_are_cut_to_the_shortest_and_a_torn_sample_is_dropped() {
        let long = bytes(&[1.0, 1.0, 1.0]);
        let mut short = bytes(&[0.0, 0.0]);
        // A buffer whose last sample is half there.
        short.extend([0u8, 0]);

        let mono = mono_from_buffers(&[(1, &long), (1, &short)]);

        assert_eq!(mono, [0.5, 0.5]);
    }

    #[test]
    fn no_buffers_make_no_sound() {
        assert!(mono_from_buffers(&[]).is_empty());
        assert!(mono_from_buffers(&[(2, &[])]).is_empty());
    }

    #[test]
    fn the_size_asked_of_the_system_is_in_pixels() {
        // A Retina display of 1512 x 982 points is 3024 x 1964 pixels.
        assert_eq!(source_pixels((1512.0, 982.0), 2.0), (3024, 1964));
        // A system that does not say (before macOS 14) is taken at one pixel to the point.
        assert_eq!(source_pixels((1512.0, 982.0), 0.0), (1512, 982));
        assert_eq!(source_pixels((1512.0, 982.0), f32::NAN), (1512, 982));
        // Never a side of nothing.
        assert_eq!(source_pixels((0.0, 0.4), 2.0), (1, 1));
    }

    fn frame() -> Frame {
        Frame {
            width: 2,
            height: 2,
            stride: 8,
            bgra: vec![7; 16],
        }
    }

    fn audio_events(rx: &mpsc::Receiver<SourceEvent>) -> Vec<(Instant, u32, usize)> {
        rx.try_iter()
            .map(|event| match event {
                SourceEvent::Audio {
                    track: Track::System,
                    at,
                    rate,
                    mono,
                } => (at, rate, mono.len()),
                _ => panic!("not the system's audio"),
            })
            .collect()
    }

    #[test]
    fn sound_goes_to_the_system_track_once_it_listens_and_not_before() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();

        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);
        assert!(rx.try_recv().is_err(), "no one listened yet");

        routes.attach(Track::System, tx);
        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);
        let heard = audio_events(&rx);
        assert_eq!(heard.len(), 1);
        assert_eq!((heard[0].1, heard[0].2), (48_000, 480));
    }

    #[test]
    fn frames_go_to_the_screen_track_and_only_while_it_listens() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();
        assert!(!routes.wants_frames());

        routes.frame(frame());
        assert!(rx.try_recv().is_err());

        routes.attach(Track::Screen, tx);
        assert!(routes.wants_frames());
        routes.frame(frame());
        assert!(matches!(rx.try_recv(), Ok(SourceEvent::Frame(_))));

        routes.detach(Track::Screen);
        assert!(!routes.wants_frames());
        routes.frame(frame());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_system_track_stops_and_starts_again_while_the_screen_goes_on() {
        let routes = Routes::new(Target::Display);
        let (screen_tx, screen) = mpsc::channel();
        let (first_tx, first) = mpsc::channel();
        routes.attach(Track::Screen, screen_tx);
        routes.attach(Track::System, first_tx);
        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);
        assert_eq!(audio_events(&first).len(), 1);

        // The system's audio is let go of: the screen is not.
        routes.detach(Track::System);
        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);
        routes.frame(frame());
        assert!(
            first.try_recv().is_err(),
            "the track that stopped hears nothing"
        );
        assert!(matches!(screen.try_recv(), Ok(SourceEvent::Frame(_))));

        // And taken up again, at another rate, as a device that was lost and replaced has.
        let (second_tx, second) = mpsc::channel();
        routes.attach(Track::System, second_tx);
        routes.audio(Instant::now(), 44_100, vec![0.5; 441]);
        let heard = audio_events(&second);
        assert_eq!((heard[0].1, heard[0].2), (44_100, 441));
        routes.frame(frame());
        assert!(matches!(screen.try_recv(), Ok(SourceEvent::Frame(_))));
    }

    #[test]
    fn buffers_that_arrive_together_are_stamped_one_after_the_other() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();
        routes.attach(Track::System, tx);
        // 10 ms of sound three times, all handed over at the same moment (the callback was kept waiting).
        let arrived = Instant::now();
        for _ in 0..3 {
            routes.audio(arrived, 48_000, vec![0.5; 480]);
        }

        let stamps: Vec<Instant> = audio_events(&rx).into_iter().map(|e| e.0).collect();

        let ten = Duration::from_millis(10);
        assert_eq!(stamps[1] - stamps[0], ten);
        assert_eq!(stamps[2] - stamps[1], ten);
    }

    #[test]
    fn a_change_of_rate_ends_the_track_as_a_lost_device() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();
        routes.attach(Track::System, tx);

        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);
        routes.audio(Instant::now(), 44_100, vec![0.5; 441]);
        // Nothing more is sent after the end.
        routes.audio(Instant::now(), 44_100, vec![0.5; 441]);

        let events: Vec<SourceEvent> = rx.try_iter().collect();
        assert_eq!(events.len(), 2, "the first buffer and the end");
        assert!(matches!(events[0], SourceEvent::Audio { .. }));
        match &events[1] {
            SourceEvent::Ended {
                track: Track::System,
                reason: EndReason::DeviceLost(why),
            } => assert!(why.contains("44100") && why.contains("48000"), "{why}"),
            _ => panic!("the track did not end as a lost device"),
        }
    }

    #[test]
    fn a_buffer_that_cannot_be_read_ends_the_track_as_a_lost_device() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();
        routes.attach(Track::System, tx);

        routes.audio_lost("16-bit integers".into());
        routes.audio(Instant::now(), 48_000, vec![0.5; 480]);

        let events: Vec<SourceEvent> = rx.try_iter().collect();
        assert_eq!(events.len(), 1, "the end, and nothing after it");
        assert!(matches!(
            &events[0],
            SourceEvent::Ended {
                track: Track::System,
                reason: EndReason::DeviceLost(why)
            } if why == "16-bit integers"
        ));
    }

    #[test]
    fn a_stream_that_stops_ends_the_screen_when_it_is_recorded() {
        let routes = Routes::new(Target::Window);
        let (screen_tx, screen) = mpsc::channel();
        let (system_tx, system) = mpsc::channel();
        routes.attach(Track::Screen, screen_tx);
        routes.attach(Track::System, system_tx);
        assert!(routes.is_alive());

        routes.stopped(&SCError::from_stream_error_code(
            SCStreamErrorCode::NoCaptureSource,
        ));

        assert!(!routes.is_alive());
        assert!(matches!(
            screen.try_recv(),
            Ok(SourceEvent::Ended {
                track: Track::Screen,
                reason: EndReason::WindowClosed
            })
        ));
        // The screen's end ends the recording, which stops the sound with it: it has nothing to say of its own.
        assert!(system.try_recv().is_err());
    }

    #[test]
    fn a_stream_that_stops_ends_the_system_track_when_only_sound_is_recorded() {
        let routes = Routes::new(Target::Display);
        let (tx, rx) = mpsc::channel();
        routes.attach(Track::System, tx);

        let error = SCError::from_stream_error_code(SCStreamErrorCode::NoCaptureSource);
        routes.stopped(&error);

        assert!(!routes.is_alive());
        match rx.try_recv() {
            Ok(SourceEvent::Ended {
                track: Track::System,
                reason: EndReason::DeviceLost(why),
            }) => assert_eq!(why, error.to_string()),
            _ => panic!("the track did not end as a lost device"),
        }
    }

    #[test]
    fn a_stream_that_stops_with_no_one_listening_tells_no_one() {
        let routes = Routes::new(Target::Display);

        routes.stopped(&SCError::StreamError("boom".into()));

        assert!(!routes.is_alive());
    }

    // ---- With the real system: they record this machine's screen, so they are asked for ----

    use crate::media::test_support::real_ffmpeg;
    use crate::record::capture::OsCapturer;
    use crate::record::quality::Quality;
    use crate::record::session::{Finished, Session};
    use crate::record::test_support::video_seconds;
    use crate::record::Capturer;
    use std::sync::Arc;

    /// Stops a test that was not asked for: it records the screen. The program that runs the test (the terminal) needs Screen
    /// Recording in System Settings, Privacy & Security.
    fn require_enabled() {
        assert_eq!(
            std::env::var("DICTATE_TEST_SCREEN").as_deref(),
            Ok("1"),
            "set DICTATE_TEST_SCREEN=1 to record this machine's screen for a few seconds"
        );
    }

    fn options(mode: Mode) -> RecordOptions {
        RecordOptions {
            mode,
            quality: Quality::Small,
            mic: false,
            system_audio: true,
        }
    }

    #[test]
    #[ignore = "records the screen: needs Screen Recording; set DICTATE_TEST_SCREEN=1"]
    fn macos_lists_displays_and_windows() {
        require_enabled();

        let sources = list_sources().expect("the displays and windows are listed");

        // Counts only: the titles of the windows on this machine are not for a log.
        eprintln!(
            "{} display(s), {} window(s)",
            sources.displays.len(),
            sources.windows.len()
        );
        assert!(!sources.displays.is_empty(), "a Mac has a display");
        assert!(sources.displays[0].name.starts_with("Display 1 ("));
        assert!(!sources.picker_is_the_system);
        assert!(
            sources
                .windows
                .windows(2)
                .all(|pair| pair[0].name <= pair[1].name),
            "the windows are sorted by name"
        );
        assert!(sources
            .windows
            .iter()
            .all(|window| window.id.parse::<u32>().is_ok()));
    }

    /// Records for `seconds` through a session on the capturer of this system, and gives what it made, with the folder that
    /// has it (which is removed when it is dropped: a recording of this machine's screen is not left behind).
    fn record(options: RecordOptions, seconds: u64) -> (Finished, tempfile::TempDir) {
        let ffmpeg = real_ffmpeg();
        let dir = tempfile::tempdir().unwrap();
        let capturer = Arc::new(OsCapturer {
            ffmpeg: ffmpeg.clone(),
        });
        let session = Session::start(options, dir.path(), capturer, ffmpeg, Box::new(|_| {}))
            .expect("the session starts");
        std::thread::sleep(Duration::from_secs(seconds));
        let finished = session.stop().expect("the recording is made");
        eprintln!(
            "{:?}: {} s, notes: {:?}",
            finished.media.file_name(),
            finished.duration,
            finished.notes
        );
        (finished, dir)
    }

    #[test]
    #[ignore = "records the screen: needs Screen Recording; set DICTATE_TEST_SCREEN=1"]
    fn macos_records_three_seconds_of_the_main_display() {
        require_enabled();
        let Some(ffmpeg) = real_ffmpeg() else { return };

        let (finished, _folder) = record(options(Mode::Screen { display: None }), 3);

        assert_eq!(finished.media.file_name().unwrap(), "recording.mp4");
        assert!(
            (finished.duration - 3.0).abs() <= 0.3,
            "{} s recorded",
            finished.duration
        );
        let seconds = video_seconds(&finished.media, &ffmpeg);
        assert!((seconds - 3.0).abs() <= 0.3, "{seconds} s of video");
    }

    #[test]
    #[ignore = "records the screen: needs Screen Recording; set DICTATE_TEST_SCREEN=1"]
    fn macos_records_sound_alone_from_a_stream_that_is_two_pixels_wide() {
        require_enabled();

        let (finished, _folder) = record(options(Mode::Audio), 2);

        assert_eq!(finished.media.file_name().unwrap(), "audio.m4a");
        assert!(
            (finished.duration - 2.0).abs() <= 0.3,
            "{} s recorded",
            finished.duration
        );
    }

    /// What reached `rx` in `wait`: the frames, how many buffers of sound, and what else (an end, a note).
    fn gather(
        rx: &mpsc::Receiver<SourceEvent>,
        wait: Duration,
    ) -> (Vec<Frame>, usize, Vec<String>) {
        let until = Instant::now() + wait;
        let (mut frames, mut sound, mut other) = (Vec::new(), 0, Vec::new());
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(SourceEvent::Frame(frame)) => frames.push(frame),
                Ok(SourceEvent::Audio { rate, mono, .. }) => {
                    assert_eq!(rate, 48_000);
                    assert!(!mono.is_empty());
                    sound += 1;
                }
                Ok(SourceEvent::Ended { track, reason }) => {
                    other.push(format!("{track:?} ended: {reason:?}"))
                }
                Ok(SourceEvent::Note { track, note }) => other.push(format!("{track:?}: {note:?}")),
                Err(_) => break,
            }
        }
        (frames, sound, other)
    }

    #[test]
    #[ignore = "records the screen: needs Screen Recording; set DICTATE_TEST_SCREEN=1"]
    fn macos_system_audio_stops_and_starts_again_while_the_screen_goes_on() {
        require_enabled();
        let capturer = OsCapturer { ffmpeg: None };
        let wanted = options(Mode::Screen { display: None });
        let (tx, rx) = mpsc::channel();

        let screen = capturer
            .start(Track::Screen, &wanted, tx.clone())
            .expect("the screen starts");
        let system = capturer
            .start(Track::System, &wanted, tx.clone())
            .expect("the system's audio joins the stream");
        let (frames, sound, other) = gather(&rx, Duration::from_secs(2));
        eprintln!(
            "together: {} frames, {sound} buffers of sound",
            frames.len()
        );
        assert!(other.is_empty(), "{other:?}");
        assert!(!frames.is_empty(), "no frame in 2 s");
        let first = &frames[0];
        // The size that was asked for (Small: 1280 at the most on the long side), a picture as large as it says.
        assert!(
            first.width.max(first.height) <= 1280,
            "{}x{}",
            first.width,
            first.height
        );
        assert!(first.stride >= first.width * 4);
        assert_eq!(first.bgra.len(), (first.stride * first.height) as usize);

        // The sound is let go of and the stream is not: the screen holds it. (A screen that does not change sends no frames,
        // so what the screen delivers meanwhile is not a sign.)
        assert!(sound > 0, "no sound in 2 s");
        system.stop();
        let _ = rx.try_iter().count();
        let (_, sound, other) = gather(&rx, Duration::from_secs(1));
        assert_eq!(sound, 0, "sound after the track was stopped");
        assert!(other.is_empty(), "{other:?}");
        assert!(
            lock(&CURRENT).upgrade().is_some(),
            "the stream went with the sound"
        );

        // And the sound is taken up again, on the same stream.
        let system = capturer
            .start(Track::System, &wanted, tx.clone())
            .expect("the system's audio starts again");
        let (frames, sound, other) = gather(&rx, Duration::from_secs(1));
        eprintln!("again: {} frames, {sound} buffers of sound", frames.len());
        assert!(other.is_empty(), "{other:?}");
        assert!(sound > 0, "no sound after it was started again");

        // The stream ends with the last of its tracks.
        system.stop();
        screen.stop();
        assert!(
            lock(&CURRENT).upgrade().is_none(),
            "the stream is still there with no track on it"
        );
        let (frames, sound, other) = gather(&rx, Duration::from_millis(500));
        assert!(other.is_empty(), "{other:?}");
        assert_eq!(
            (frames.len(), sound),
            (0, 0),
            "something is still delivered after every track was stopped"
        );
    }

    #[test]
    #[ignore = "records the screen: needs Screen Recording; set DICTATE_TEST_SCREEN=1"]
    fn macos_does_not_record_a_window_that_is_not_there() {
        require_enabled();
        let capturer = OsCapturer { ffmpeg: None };
        let (tx, _rx) = mpsc::channel();

        let error = capturer
            .start(
                Track::Screen,
                &options(Mode::Window {
                    id: "4294967295".into(),
                }),
                tx,
            )
            .err();

        assert_eq!(error, Some(StartError::WindowGone));
    }
}
