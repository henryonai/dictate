//! The screen on Linux: a monitor or a window, recorded through the ScreenCast portal of xdg-desktop-portal (the `ashpd` crate) and
//! a PipeWire stream (the `pipewire` crate).
//!
//! What the portal does: the person chooses what to share in a dialog of the desktop, not in a list of ours (`Sources::picker_is_the_system`).
//! `Mode::Screen` asks the portal for a monitor and `Mode::Window` for a window, and the `display` or the `id` that the mode has is not
//! used. The portal hands over a PipeWire node and a connection to the PipeWire daemon that can see only that node; the stream that
//! reads it offers shared memory formats only (BGRx, BGRA, RGBx, with no DMA-BUF modifier, so that the desktop gives memory that can be
//! read) and copies each picture, as BGRA with an opaque alpha, in rows with nothing between them. The pictures come when something
//! changes: the offer says that no more than the frames a second of the video are wanted (`maxFramerate`), and every picture that comes
//! is passed on, the last of a burst too, because it is what the video shows until the screen changes again. The session takes the latest
//! picture at its own pace and repeats it meanwhile.
//!
//! The dialog may wait on the person for a long time: `start` blocks until it is answered, for [`DIALOG_WAIT`] at the most (the app
//! starts a recording off the thread of its window). A dialog that is cancelled is `StartError::PortalCancelled`, and a desktop that
//! has no ScreenCast portal is `StartError::PortalUnavailable`. The portal gives a restore token with the choice, and with the token
//! the next start of the same kind has no dialog; where the token is kept is the app's to say ([`TokenStore`]).
//!
//! The end of the capture is the end of the stream: the window was closed, the monitor was unplugged, or the person stopped the sharing
//! from the desktop. That ends the screen track, and with it the recording.
//!
//! The threads: the D-Bus work (ashpd) runs on a runtime of its own, for as long as the process lives (`portal_runtime`: ashpd keeps one
//! connection to the session bus for the process, and zbus runs the tasks of that connection on the runtime that made it, so that
//! runtime must outlast every recording), and holds the portal session of a recording until the recording stops. The PipeWire loop runs
//! on a thread of a recording's own. What is plain code (the pictures, the restore token, how a start that is refused is told, how the
//! stream ends) is built on every system, so that its tests run everywhere; the part that calls the system is `native`, for Linux.

// The plain code is for the other systems to test: what only the capture of Linux uses is not dead there.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use super::Sources;
use crate::record::video::Frame;
use crate::record::{EndReason, Mode, SourceEvent, StartError, Track};

/// How long the start waits for the person to choose in the dialog of the portal. Five minutes: somebody who started a recording
/// and went to make coffee has not been given up on, and a dialog that nobody comes back to does not hold the start for ever.
const DIALOG_WAIT: Duration = Duration::from_secs(300);

/// How long the calls of the portal that need no one to answer them (is it there, make a session, give the connection) are waited
/// for. They are answered in milliseconds, unless the desktop's portal is stuck.
const CALL_WAIT: Duration = Duration::from_secs(15);

/// How long the end of the capture is waited for. The loop of PipeWire stops in a few milliseconds, unless the daemon does not
/// answer, and `stop` must not wait for that.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// What the capture is of: a monitor or a window.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Display,
    Window,
}

/// The pixel formats that the stream offers, and reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PixelFormat {
    /// Blue, green, red, and a byte that is not used.
    Bgrx,
    Bgra,
    /// Red, green, blue, and a byte that is not used.
    Rgbx,
}

/// Where the restore token of the portal is kept between recordings. The app keeps it in its settings (the key
/// `screencast_restore_token`); `NoTokens` keeps nothing, and the dialog is shown every time.
///
/// The text is the app's to keep as it is, and not to read: it has the kind of the capture in it (a token of a window is not given
/// for a monitor, which it would restore instead of showing the dialog), and the token of the portal after that.
pub trait TokenStore: Send + Sync {
    /// What `save` was given the last time, if it was.
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str);
}

/// A [`TokenStore`] that keeps nothing.
pub struct NoTokens;

impl TokenStore for NoTokens {
    fn load(&self) -> Option<String> {
        None
    }

    fn save(&self, _token: &str) {}
}

/// Why the portal did not give a capture, as far as this module tells them apart.
#[derive(Debug, PartialEq)]
enum Refusal {
    /// The person closed the dialog without choosing.
    Cancelled,
    /// There is no portal to ask (no session bus, no ScreenCast interface).
    Unavailable,
    /// Nobody answered the dialog in time.
    NoAnswer,
    Other(String),
}

/// How the capture tells the session that it is over: once, whoever sees the end first (the stream or the portal session), and not at
/// all after a `stop`.
#[derive(Clone)]
struct Ender {
    tx: Sender<SourceEvent>,
    target: Target,
    over: Arc<AtomicBool>,
}

/// How many pictures that cannot be read, one after the other, the capture puts up with. A picture that cannot be read once is a buffer
/// that was not there yet; a run of them (a second or so of a screen that changes) is a desktop that gives what cannot be copied.
const UNREADABLE_PICTURES: u32 = 30;

/// How many pictures in a row could not be read.
#[derive(Default)]
struct Unreadable(u32);

/// What the stream says of a buffer, as far as a picture is read from it.
struct Buffer<'a> {
    /// Where the picture starts in `data`, and how many bytes it is: what its chunk says.
    offset: u32,
    size: u32,
    stride: i32,
    /// The desktop says that what is in the buffer is damaged.
    corrupted: bool,
    /// What is in the buffer, when it is memory that is mapped (a DMA-BUF is not).
    data: Option<&'a [u8]>,
}

/// What the callbacks of the stream keep: what the pictures are, as the stream last said so, and where they go.
struct Pictures {
    /// `None` before the stream has said, or for a format that is not offered.
    format: Option<PixelFormat>,
    /// In pixels.
    size: (u32, u32),
    unreadable: Unreadable,
    tx: Sender<SourceEvent>,
}

/// The picture that `bytes` are, in `height` rows of `stride` bytes of `format` pixels `width` wide, as BGRA with an opaque alpha in
/// rows with nothing between them (the last row of `bytes` needs no more than its pixels, and not its padding). `stride` is what the
/// buffer says: 0 is "no padding". `None` for what is not a picture (no pixels, rows that are shorter than their pixels, a stride that
/// goes up the screen, bytes that are too few).
fn picture(
    format: PixelFormat,
    width: u32,
    height: u32,
    stride: i32,
    bytes: &[u8],
) -> Option<Frame> {
    let row = (width as usize).checked_mul(4)?;
    let stride = match stride {
        0 => row,
        above_zero if above_zero > 0 => above_zero as usize,
        _ => return None,
    };
    if width == 0 || height == 0 || stride < row {
        return None;
    }
    let rows = height as usize;
    let needed = stride.checked_mul(rows - 1)?.checked_add(row)?;
    if bytes.len() < needed {
        return None;
    }
    let mut bgra = vec![0_u8; row.checked_mul(rows)?];
    for (line, copy) in bgra.chunks_exact_mut(row).enumerate() {
        let from = &bytes[line * stride..line * stride + row];
        match format {
            PixelFormat::Bgrx | PixelFormat::Bgra => {
                copy.copy_from_slice(from);
                for pixel in copy.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            }
            PixelFormat::Rgbx => {
                for (to, pixel) in copy.chunks_exact_mut(4).zip(from.chunks_exact(4)) {
                    to.copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                }
            }
        }
    }
    Some(Frame {
        width,
        height,
        stride: u32::try_from(row).ok()?,
        bgra,
    })
}

/// What a chunk of a buffer holds: `size` bytes from `offset`. `None` for a chunk with nothing in it, or one that reaches past the
/// buffer.
fn chunk_bytes(buffer: &[u8], offset: u32, size: u32) -> Option<&[u8]> {
    if size == 0 {
        return None;
    }
    let from = offset as usize;
    buffer.get(from..from.checked_add(size as usize)?)
}

/// What the capture of `mode` is. The screen of a recording of sound alone is not a thing to capture.
fn target_of(mode: &Mode) -> Result<Target, StartError> {
    match mode {
        Mode::Screen { .. } => Ok(Target::Display),
        Mode::Window { .. } => Ok(Target::Window),
        Mode::Audio => Err(StartError::Source(
            "the screen is not recorded in a recording of sound".into(),
        )),
    }
}

impl Target {
    /// What the kept text of a token starts with, so that a token is given back to the kind of capture that it was made for.
    fn tag(self) -> &'static str {
        match self {
            Target::Display => "display:",
            Target::Window => "window:",
        }
    }
}

/// The token of the portal that the store keeps for a capture of `target`: the store's text, less its tag.
fn kept_token(store: &dyn TokenStore, target: Target) -> Option<String> {
    let text = store.load()?;
    let token = text.strip_prefix(target.tag())?;
    (!token.is_empty()).then(|| token.to_owned())
}

/// Keeps the token that the portal gave, for the next start of a capture of `target`. A start that was given none leaves the token that
/// is kept as it is.
fn keep_token(store: &dyn TokenStore, target: Target, token: Option<&str>) {
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        store.save(&format!("{}{token}", target.tag()));
    }
}

/// What a start that the portal refused is, as the session and the person hear of it.
fn start_error(refusal: Refusal) -> StartError {
    match refusal {
        Refusal::Cancelled => StartError::PortalCancelled,
        Refusal::Unavailable => StartError::PortalUnavailable,
        Refusal::NoAnswer => StartError::Source(format!(
            "nothing was chosen in the dialog of the desktop within {} minutes",
            DIALOG_WAIT.as_secs() / 60
        )),
        Refusal::Other(text) => StartError::Source(text),
    }
}

/// What the picker offers when the portal has been asked whether it is there: it is the portal that asks the person, and there is
/// no list of ours.
fn portal_sources(answered: bool) -> Result<Sources, StartError> {
    if answered {
        Ok(Sources {
            displays: Vec::new(),
            windows: Vec::new(),
            picker_is_the_system: true,
        })
    } else {
        Err(StartError::PortalUnavailable)
    }
}

impl Ender {
    fn new(tx: Sender<SourceEvent>, target: Target) -> Ender {
        Ender {
            tx,
            target,
            over: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The capture is over, and was not asked to stop: the session is told, the first time only. A monitor that is gone and a window
    /// that is closed are told apart.
    fn ended(&self) {
        self.end(match self.target {
            Target::Display => EndReason::DisplayGone,
            Target::Window => EndReason::WindowClosed,
        });
    }

    /// The capture cannot go on, and says why; the first end is the only one that is told.
    fn failed(&self, why: &str) {
        self.end(EndReason::Failed(why.to_owned()));
    }

    fn end(&self, reason: EndReason) {
        if !self.over.swap(true, Ordering::SeqCst) {
            // A session that is over has no one to read.
            let _ = self.tx.send(SourceEvent::Ended {
                track: Track::Screen,
                reason,
            });
        }
    }

    /// The capture was stopped by the session: nothing more is said.
    fn stopped(&self) {
        self.over.store(true, Ordering::SeqCst);
    }
}

impl Unreadable {
    /// A picture was read.
    fn read(&mut self) {
        self.0 = 0;
    }

    /// A picture could not be read. Whether this is the one that gives the capture up: the run has got to [`UNREADABLE_PICTURES`].
    fn fail(&mut self) -> bool {
        self.0 += 1;
        self.0 >= UNREADABLE_PICTURES
    }
}

impl Pictures {
    fn new(tx: Sender<SourceEvent>) -> Pictures {
        Pictures {
            format: None,
            size: (0, 0),
            unreadable: Unreadable::default(),
            tx,
        }
    }

    /// The stream says what its pictures are (it says so again when a window is resized).
    fn say(&mut self, format: Option<PixelFormat>, size: (u32, u32)) {
        self.format = format;
        self.size = size;
    }

    /// A buffer has arrived: the picture that is in it goes to the channel at once, whole, whatever came before it. A screen sends a picture
    /// when something changes and nothing when it stops changing, so the last picture of a burst is what the video shows from then on:
    /// none is held back, and the session takes the latest at its own pace. A buffer that has nothing new in it, or that the desktop says
    /// is damaged, is no picture and no fault, and uses up nothing. Whether the capture is to be given up, for a run of buffers that cannot
    /// be read.
    fn arrived(&mut self, buffer: Buffer) -> bool {
        if buffer.size == 0 || buffer.corrupted {
            return false;
        }
        let (width, height) = self.size;
        let frame = self.format.and_then(|format| {
            let bytes = chunk_bytes(buffer.data?, buffer.offset, buffer.size)?;
            picture(format, width, height, buffer.stride, bytes)
        });
        match frame {
            Some(frame) => {
                self.unreadable.read();
                // A session that is over has no one to read.
                let _ = self.tx.send(SourceEvent::Frame(frame));
                false
            }
            None => self.unreadable.fail(),
        }
    }
}

#[cfg(target_os = "linux")]
pub use native::{list_sources, start};

/// The capture itself, which calls the system and is for Linux.
#[cfg(target_os = "linux")]
mod native {
    use std::future::Future;
    use std::os::fd::OwnedFd;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Mutex, PoisonError};
    use std::thread::{self, JoinHandle};

    use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
    use ashpd::desktop::{PersistMode, ResponseError, Session};
    use ashpd::zbus::DBusError;
    use futures_util::StreamExt;
    use pipewire as pw;
    use pw::properties::properties;
    use pw::spa;
    use tokio::runtime::{Builder, Handle};
    use tokio::sync::oneshot;

    use super::*;
    use crate::record::{RecordOptions, RunningSource};

    /// What the thread that waits for the answer of a task on the portal's runtime waits for beyond the limit of the task's own calls.
    const SLACK: Duration = Duration::from_secs(5);

    // ---- The portal ----

    /// The runtime that the D-Bus work of the portal runs on, which lives as long as the process does. `ashpd` keeps one connection to the
    /// session bus for the whole process, and zbus runs the tasks of that connection on the runtime that was current when it made it:
    /// a runtime that ended with a recording would leave every later use of the connection (the next recording, the shortcut of the
    /// app, which has its own portal) with tasks that are not run. So the runtime is made once, on a thread of its own that does
    /// nothing else, and the portal sessions of the recordings are tasks on it.
    fn portal_runtime() -> Result<Handle, StartError> {
        static RUNTIME: Mutex<Option<Handle>> = Mutex::new(None);
        let mut kept = RUNTIME.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(handle) = kept.as_ref() {
            return Ok(handle.clone());
        }
        let cannot = |why: String| StartError::Source(format!("the portal cannot be asked: {why}"));
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| cannot(error.to_string()))?;
        let handle = runtime.handle().clone();
        thread::Builder::new()
            .name("dictate portal".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .map_err(|error| cannot(error.to_string()))?;
        *kept = Some(handle.clone());
        Ok(handle)
    }

    /// Whether what zbus says is that there is nobody to ask: no bus to connect to, or no one on it with the name of the portal (it is not
    /// installed, or it has not started).
    fn nobody_there(error: &ashpd::zbus::Error) -> bool {
        use ashpd::zbus::Error as Zbus;
        const GONE: [&str; 4] = [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.UnknownObject",
            "org.freedesktop.DBus.Error.UnknownInterface",
        ];
        match error {
            Zbus::Address(_) | Zbus::InputOutput(_) | Zbus::Handshake(_) => true,
            Zbus::FDO(error) => GONE.contains(&error.name().as_str()),
            Zbus::MethodError(name, ..) => GONE.contains(&name.as_str()),
            _ => false,
        }
    }

    /// What the portal's refusal is, from what ashpd says.
    fn refusal(error: &ashpd::Error) -> Refusal {
        match error {
            ashpd::Error::Response(ResponseError::Cancelled)
            | ashpd::Error::Portal(ashpd::PortalError::Cancelled(_)) => Refusal::Cancelled,
            ashpd::Error::PortalNotFound(_) => Refusal::Unavailable,
            // A call that the bus refuses comes either way, as an error of zbus or as one of the portal that has it inside.
            ashpd::Error::Zbus(error) | ashpd::Error::Portal(ashpd::PortalError::ZBus(error))
                if nobody_there(error) =>
            {
                Refusal::Unavailable
            }
            other => Refusal::Other(other.to_string()),
        }
    }

    /// A call of the portal, given `limit` to be answered; a call that is not is `unanswered`.
    async fn within<T>(
        limit: Duration,
        unanswered: Refusal,
        call: impl Future<Output = Result<T, ashpd::Error>>,
    ) -> Result<T, Refusal> {
        match tokio::time::timeout(limit, call).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) => Err(refusal(&error)),
            Err(_) => Err(unanswered),
        }
    }

    /// What the answer of a request is, said as a refusal when it is not one to go on with.
    fn told<T>(answer: Result<T, ashpd::Error>) -> Result<T, Refusal> {
        answer.map_err(|error| refusal(&error))
    }

    type PortalSession = Session<'static, Screencast<'static>>;

    /// A portal session that gives a stream: dropping `keep` (or sending on it) ends the session, which stops the stream.
    struct Opened {
        fd: OwnedFd,
        node: u32,
        keep: oneshot::Sender<()>,
    }

    /// Asks the portal for a capture of `target`, which is the dialog of the desktop unless the token that is kept for it is still good,
    /// and keeps the token that comes with the answer.
    async fn share(
        portal: &Screencast<'static>,
        session: &PortalSession,
        target: Target,
        tokens: &dyn TokenStore,
    ) -> Result<(OwnedFd, u32), Refusal> {
        let token = kept_token(tokens, target);
        let types = match target {
            Target::Display => SourceType::Monitor,
            Target::Window => SourceType::Window,
        };
        let selected = within(
            CALL_WAIT,
            Refusal::Unavailable,
            portal.select_sources(
                session,
                CursorMode::Embedded,
                types.into(),
                false,
                token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            ),
        )
        .await?;
        told(selected.response())?;
        // Here the person is asked, and the call waits for as long as they take.
        let started = within(DIALOG_WAIT, Refusal::NoAnswer, portal.start(session, None)).await?;
        let streams = told(started.response())?;
        keep_token(tokens, target, streams.restore_token());
        let node = streams
            .streams()
            .first()
            .ok_or_else(|| Refusal::Other("the desktop shared nothing".into()))?
            .pipe_wire_node_id();
        let fd = within(
            CALL_WAIT,
            Refusal::Unavailable,
            portal.open_pipe_wire_remote(session),
        )
        .await?;
        Ok((fd, node))
    }

    /// The portal session of a recording, for as long as it lasts: makes it, tells `start` what came of it, and then holds it until the
    /// recording stops (`keep` is sent on or dropped) or the desktop ends it (the person stopped the sharing), which is told to the
    /// session as the end of the capture. The session is closed at the end, which also closes the dialog if it is still open.
    async fn keep_session(
        target: Target,
        tokens: Arc<dyn TokenStore>,
        ender: Ender,
        told: Sender<Result<Opened, Refusal>>,
    ) {
        let (session, fd, node) = match open(target, &*tokens).await {
            Ok(opened) => opened,
            Err(refusal) => {
                let _ = told.send(Err(refusal));
                return;
            }
        };
        let mut closed = session.receive_closed().await.ok().map(Box::pin);
        let (keep, kept) = oneshot::channel();
        if told.send(Ok(Opened { fd, node, keep })).is_err() {
            // `start` has given up waiting.
            close(&session).await;
            return;
        }
        let closed_by_the_desktop = async {
            match closed.as_mut() {
                Some(closed) => {
                    closed.next().await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = kept => {}
            _ = closed_by_the_desktop => ender.ended(),
        }
        close(&session).await;
    }

    async fn close(session: &PortalSession) {
        // A session that the desktop closed has nothing to close, and says so.
        let _ = tokio::time::timeout(CALL_WAIT, session.close()).await;
    }

    /// A portal session with what it shares, or why it does not. A session that cannot give a capture is closed here.
    async fn open(
        target: Target,
        tokens: &dyn TokenStore,
    ) -> Result<(PortalSession, OwnedFd, u32), Refusal> {
        let portal = within(CALL_WAIT, Refusal::Unavailable, Screencast::new()).await?;
        let session = within(CALL_WAIT, Refusal::Unavailable, portal.create_session()).await?;
        match share(&portal, &session, target, tokens).await {
            Ok((fd, node)) => Ok((session, fd, node)),
            Err(refusal) => {
                close(&session).await;
                Err(refusal)
            }
        }
    }

    /// Asks the portal for a capture of `target`, on the thread of the caller, which waits: for the dialog of the desktop, if there is
    /// one, for [`DIALOG_WAIT`] at the most.
    fn open_portal(
        target: Target,
        tokens: Arc<dyn TokenStore>,
        ender: Ender,
    ) -> Result<Opened, StartError> {
        let runtime = portal_runtime()?;
        let (told, answered) = mpsc::channel();
        runtime.spawn(keep_session(target, tokens, ender, told));
        // The calls have limits of their own, the dialog's the longest: this is only for a task that was lost.
        let limit = DIALOG_WAIT + 5 * CALL_WAIT;
        match answered.recv_timeout(limit) {
            Ok(Ok(opened)) => Ok(opened),
            Ok(Err(refusal)) => Err(start_error(refusal)),
            Err(_) => Err(start_error(Refusal::Other(
                "the portal did not answer".into(),
            ))),
        }
    }

    /// What the picker offers: the system's dialog, when the portal is there to show it.
    pub fn list_sources() -> Result<Sources, StartError> {
        let runtime = portal_runtime().map_err(|_| StartError::PortalUnavailable)?;
        let (told, answered) = mpsc::channel();
        runtime.spawn(async move {
            let asked = tokio::time::timeout(CALL_WAIT, async {
                Screencast::new().await?.available_source_types().await
            })
            .await;
            let _ = told.send(matches!(asked, Ok(Ok(_))));
        });
        portal_sources(answered.recv_timeout(CALL_WAIT + SLACK).unwrap_or(false))
    }

    // ---- The stream ----

    /// What the stream is told to do from outside: stop.
    struct Quit;

    impl PixelFormat {
        /// The format of PipeWire that this is, and `None` for one that is not offered.
        fn of(format: spa::param::video::VideoFormat) -> Option<PixelFormat> {
            use spa::param::video::VideoFormat;
            if format == VideoFormat::BGRx {
                Some(PixelFormat::Bgrx)
            } else if format == VideoFormat::BGRA {
                Some(PixelFormat::Bgra)
            } else if format == VideoFormat::RGBx {
                Some(PixelFormat::Rgbx)
            } else {
                None
            }
        }
    }

    /// The formats that the stream offers, as the pod that `Stream::connect` takes: BGRx, BGRA and RGBx in shared memory (there is no
    /// modifier in the offer, so a desktop that gives DMA-BUFs only for a format that has one gives memory), any size, any rate (a screen
    /// that gives pictures when something changes says 0/1, a source of video has one of its own) and at most `fps` pictures a second.
    fn offered_formats(fps: u32) -> Vec<u8> {
        use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
        use spa::param::video::VideoFormat;
        use spa::utils::{Fraction, Rectangle};
        let most = Fraction {
            num: fps.max(1),
            denom: 1,
        };
        let object = spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::EnumFormat,
            spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            spa::pod::property!(
                FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                VideoFormat::BGRx,
                VideoFormat::BGRx,
                VideoFormat::BGRA,
                VideoFormat::RGBx
            ),
            spa::pod::property!(
                FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                Rectangle {
                    width: 1920,
                    height: 1080
                },
                Rectangle {
                    width: 1,
                    height: 1
                },
                Rectangle {
                    width: 16_384,
                    height: 16_384
                }
            ),
            spa::pod::property!(
                FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                most,
                Fraction { num: 0, denom: 1 },
                Fraction {
                    num: 1000,
                    denom: 1
                }
            ),
            spa::pod::property!(
                FormatProperties::VideoMaxFramerate,
                Choice,
                Range,
                Fraction,
                most,
                Fraction { num: 0, denom: 1 },
                most
            ),
        );
        spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(object),
        )
        .map(|(written, _)| written.into_inner())
        .unwrap_or_default()
    }

    fn text(error: impl std::fmt::Display) -> String {
        error.to_string()
    }

    /// The loop of PipeWire, on the thread that this is run on: connects to the daemon through `fd`, which the portal opened for this
    /// capture, reads the node `node` and sends its pictures to `tx`, until `quit` says to stop, or the stream ends. `ready` is told
    /// when the stream is connected (the pictures follow when the first buffer is there), or why it could not be.
    fn run_stream(
        fd: OwnedFd,
        node: u32,
        fps: u32,
        tx: Sender<SourceEvent>,
        ender: Ender,
        quit: pw::channel::Receiver<Quit>,
        ready: &Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(text)?;
        let _quit = quit.attach(mainloop.loop_(), {
            let mainloop = mainloop.clone();
            move |Quit| mainloop.quit()
        });
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(text)?;
        let core = context.connect_fd_rc(fd, None).map_err(text)?;
        let stream = pw::stream::StreamBox::new(
            &core,
            "dictate-screen",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
                // What is shared is this one node: when it is gone (the window was closed, the monitor was unplugged, the person
                // stopped the sharing) the stream must end, and not wait for the session manager to link it to another one.
                *pw::keys::NODE_DONT_RECONNECT => "true",
            },
        )
        .map_err(text)?;
        let pictures = Pictures::new(tx);
        let _listener = stream
            .add_local_listener_with_user_data(pictures)
            .state_changed({
                let (mainloop, ender) = (mainloop.clone(), ender.clone());
                move |_, _, _, now| {
                    // The window was closed, the monitor was unplugged, or the person stopped the sharing.
                    if matches!(
                        now,
                        pw::stream::StreamState::Unconnected | pw::stream::StreamState::Error(_)
                    ) {
                        ender.ended();
                        mainloop.quit();
                    }
                }
            })
            .param_changed(|_, pictures, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media, subtype)) = spa::param::format_utils::parse_format(param) else {
                    return;
                };
                if media != spa::param::format::MediaType::Video
                    || subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let mut format = spa::param::video::VideoInfoRaw::default();
                if format.parse(param).is_ok() {
                    let size = format.size();
                    pictures.say(PixelFormat::of(format.format()), (size.width, size.height));
                }
            })
            .process({
                let (mainloop, ender) = (mainloop.clone(), ender.clone());
                move |stream, pictures| {
                    // The buffer goes back to the stream when it is dropped, whether or not the picture is used.
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    let Some(data) = buffer.datas_mut().first_mut() else {
                        return;
                    };
                    let chunk = data.chunk();
                    let (offset, size, stride) = (chunk.offset(), chunk.size(), chunk.stride());
                    let corrupted = chunk.flags().contains(spa::buffer::ChunkFlags::CORRUPTED);
                    let given_up = pictures.arrived(Buffer {
                        offset,
                        size,
                        stride,
                        corrupted,
                        data: data.data().map(|bytes| &*bytes),
                    });
                    if given_up {
                        ender.failed("the desktop gives pictures that cannot be read");
                        mainloop.quit();
                    }
                }
            })
            .register()
            .map_err(text)?;
        let offer = offered_formats(fps);
        let mut params = [spa::pod::Pod::from_bytes(&offer)
            .ok_or_else(|| "the formats of the stream are not a pod".to_owned())?];
        stream
            .connect(
                spa::utils::Direction::Input,
                Some(node),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(text)?;
        let _ = ready.send(Ok(()));
        mainloop.run();
        Ok(())
    }

    /// A capture that runs: stopped by `stop`, or by being dropped.
    struct Capture {
        quit: pw::channel::Sender<Quit>,
        /// Told when the thread of the stream is over.
        over: Receiver<()>,
        ender: Ender,
        /// Dropped after the stream has ended: that closes the portal session.
        _portal: oneshot::Sender<()>,
    }

    impl RunningSource for Capture {
        fn stop(self: Box<Self>) {
            // `drop` does it.
            drop(self);
        }
    }

    impl Drop for Capture {
        /// Ends the stream, and waits for the end of it for [`STOP_WAIT`] at the most: the daemon that does not answer does not hold the
        /// session's thread. From here on nothing is sent.
        fn drop(&mut self) {
            self.ender.stopped();
            // A loop that has ended by itself has no one to tell.
            let _ = self.quit.send(Quit);
            let _ = self.over.recv_timeout(STOP_WAIT);
        }
    }

    /// Starts the screen track: asks the portal for the capture that `options` name, and reads the stream that it gives on a thread of its
    /// own. The pictures and the end are sent to `tx`; the channel is in the loop's callbacks from before the stream is connected, so that
    /// the first picture (all there is of a screen that does not change) has somewhere to go.
    pub fn start(
        options: &RecordOptions,
        tokens: Arc<dyn TokenStore>,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        let target = target_of(&options.mode)?;
        let ender = Ender::new(tx.clone(), target);
        let Opened { fd, node, keep } = open_portal(target, tokens, ender.clone())?;
        // From here on the portal session closes with any return: it ends when `keep` is dropped.
        let fps = options.quality.spec().fps;
        let capture = begin(fd, node, fps, tx, ender, keep)?;
        Ok(Box::new(capture))
    }

    /// Reads the node `node` of the PipeWire daemon that `fd` is a connection to, on a thread of its own, and waits for the stream to be
    /// connected. `portal` is held until the capture is over.
    fn begin(
        fd: OwnedFd,
        node: u32,
        fps: u32,
        tx: Sender<SourceEvent>,
        ender: Ender,
        portal: oneshot::Sender<()>,
    ) -> Result<Capture, StartError> {
        let (quit, quit_receiver) = pw::channel::channel::<Quit>();
        let (ready, readied) = mpsc::channel();
        let (finished, over) = mpsc::channel();
        let thread: JoinHandle<()> = thread::Builder::new()
            .name("dictate screen".into())
            .spawn({
                let ender = ender.clone();
                move || {
                    pw::init();
                    if let Err(why) = run_stream(fd, node, fps, tx, ender, quit_receiver, &ready) {
                        let _ = ready.send(Err(why));
                    }
                    let _ = finished.send(());
                }
            })
            .map_err(|error| StartError::Source(format!("the stream cannot be read: {error}")))?;
        match readied.recv_timeout(CALL_WAIT) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => {
                // The start fails, and that is all that is said of it: not an end of a capture that was never there.
                ender.stopped();
                let _ = thread.join();
                return Err(StartError::Source(why));
            }
            Err(_) => {
                // The daemon does not answer: the thread is told to stop, and left to it.
                ender.stopped();
                let _ = quit.send(Quit);
                return Err(StartError::Source(
                    "PipeWire did not answer the stream".into(),
                ));
            }
        }
        Ok(Capture {
            quit,
            over,
            ender,
            _portal: portal,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ashpd::zbus;
        use spa::param::video::VideoFormat;

        #[test]
        fn a_cancelled_portal_dialog_is_a_refusal() {
            let cancelled = ashpd::Error::Response(ResponseError::Cancelled);

            assert_eq!(refusal(&cancelled), Refusal::Cancelled);
            // As the person hears of it.
            assert_eq!(
                start_error(refusal(&cancelled)),
                StartError::PortalCancelled
            );
        }

        #[test]
        fn a_dialog_that_was_cancelled_through_the_portals_own_error_is_a_cancel_too() {
            let cancelled = ashpd::Error::Portal(ashpd::PortalError::Cancelled("closed".into()));

            assert_eq!(refusal(&cancelled), Refusal::Cancelled);
        }

        #[test]
        fn a_desktop_without_a_screencast_portal_is_unavailable() {
            let interface =
                zbus::names::OwnedInterfaceName::try_from("org.freedesktop.portal.ScreenCast")
                    .unwrap();

            assert_eq!(
                refusal(&ashpd::Error::PortalNotFound(interface)),
                Refusal::Unavailable
            );
        }

        #[test]
        fn nobody_to_ask_on_the_session_bus_is_unavailable() {
            // No bus to connect to, and a bus on which nobody has the name of the portal (it is not installed, or it did not start).
            let no_bus = ashpd::Error::Zbus(zbus::Error::Address("unix:path=/nowhere".into()));
            let no_portal = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
                zbus::fdo::Error::ServiceUnknown("org.freedesktop.portal.Desktop".into()),
            )));
            let no_owner = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
                zbus::fdo::Error::NameHasNoOwner("org.freedesktop.portal.Desktop".into()),
            )));

            for error in [no_bus, no_portal, no_owner] {
                assert_eq!(refusal(&error), Refusal::Unavailable, "{error}");
            }
        }

        #[test]
        fn a_call_that_a_bus_without_the_portal_refuses_is_unavailable_too() {
            // The bus answers a call to the name of the portal with "not provided by any .service file": ashpd has it as an error of the
            // portal, with the error of zbus inside.
            let service_unknown =
                ashpd::Error::Portal(ashpd::PortalError::ZBus(zbus::Error::FDO(Box::new(
                    zbus::fdo::Error::ServiceUnknown("org.freedesktop.portal.Desktop".into()),
                ))));

            assert_eq!(refusal(&service_unknown), Refusal::Unavailable);
        }

        #[test]
        fn what_else_the_portal_says_is_told_as_it_was_said() {
            let other = refusal(&ashpd::Error::Response(ResponseError::Other));
            assert!(
                matches!(&other, Refusal::Other(text) if text.contains("Other")),
                "{other:?}"
            );

            // A bus that answers with a failure is not a bus that is not there.
            let failed = refusal(&ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
                zbus::fdo::Error::InvalidArgs("no such option".into()),
            ))));
            assert!(
                matches!(&failed, Refusal::Other(text) if text.contains("no such option")),
                "{failed:?}"
            );
        }

        #[tokio::test]
        async fn a_call_that_is_not_answered_in_time_is_the_refusal_that_was_given_for_it() {
            let never = std::future::pending::<Result<(), ashpd::Error>>();

            let dialog = within(Duration::from_millis(20), Refusal::NoAnswer, never).await;
            assert_eq!(dialog, Err(Refusal::NoAnswer));

            let never = std::future::pending::<Result<(), ashpd::Error>>();
            let call = within(Duration::from_millis(20), Refusal::Unavailable, never).await;
            assert_eq!(call, Err(Refusal::Unavailable));
        }

        #[tokio::test]
        async fn a_call_that_is_answered_gives_its_answer_or_why_it_is_refused() {
            let answered = within(Duration::from_secs(5), Refusal::NoAnswer, async { Ok(7) }).await;
            assert_eq!(answered, Ok(7));

            let cancelled = within(Duration::from_secs(5), Refusal::NoAnswer, async {
                Err::<(), _>(ashpd::Error::Response(ResponseError::Cancelled))
            })
            .await;
            assert_eq!(cancelled, Err(Refusal::Cancelled));
        }

        #[test]
        fn the_formats_of_the_stream_are_told_apart() {
            assert_eq!(PixelFormat::of(VideoFormat::BGRx), Some(PixelFormat::Bgrx));
            assert_eq!(PixelFormat::of(VideoFormat::BGRA), Some(PixelFormat::Bgra));
            assert_eq!(PixelFormat::of(VideoFormat::RGBx), Some(PixelFormat::Rgbx));
            // Not offered, so not read: a producer that gives one of them has not understood the offer.
            for other in [
                VideoFormat::RGBA,
                VideoFormat::RGB,
                VideoFormat::NV12,
                VideoFormat::I420,
                VideoFormat::Unknown,
            ] {
                assert_eq!(PixelFormat::of(other), None);
            }
        }

        /// The properties of what the stream offers, by key.
        fn offered(fps: u32) -> Vec<spa::pod::Property> {
            let bytes = offered_formats(fps);
            let (_, value) = spa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes)
                .expect("the offer is a pod");
            match value {
                spa::pod::Value::Object(object) => object.properties,
                other => panic!("the offer is not an object: {other:?}"),
            }
        }

        fn property(
            properties: &[spa::pod::Property],
            key: spa::param::format::FormatProperties,
        ) -> Option<&spa::pod::Value> {
            properties
                .iter()
                .find(|property| property.key == key.as_raw())
                .map(|property| &property.value)
        }

        #[test]
        fn the_offer_is_raw_video_and_nothing_else() {
            let bytes = offered_formats(15);
            let pod = spa::pod::Pod::from_bytes(&bytes).expect("a pod");

            let (media, subtype) = spa::param::format_utils::parse_format(pod).expect("a format");

            assert_eq!(media, spa::param::format::MediaType::Video);
            assert_eq!(subtype, spa::param::format::MediaSubtype::Raw);
        }

        #[test]
        fn the_offer_has_the_shared_memory_formats_and_no_dma_buf_modifier() {
            use spa::param::format::FormatProperties;
            let properties = offered(15);

            // BGRx is the one that is asked for first: it is what the desktops give.
            let Some(spa::pod::Value::Choice(spa::pod::ChoiceValue::Id(spa::utils::Choice(
                _,
                spa::utils::ChoiceEnum::Enum {
                    default,
                    alternatives,
                },
            )))) = property(&properties, FormatProperties::VideoFormat)
            else {
                panic!("the formats are an enumeration of ids");
            };
            assert_eq!(default.0, VideoFormat::BGRx.as_raw());
            let mut offered: Vec<u32> = alternatives.iter().map(|id| id.0).collect();
            offered.sort_unstable();
            let mut wanted = vec![
                VideoFormat::BGRx.as_raw(),
                VideoFormat::BGRA.as_raw(),
                VideoFormat::RGBx.as_raw(),
            ];
            wanted.sort_unstable();
            assert_eq!(offered, wanted);
            // A format with a modifier is one that a desktop gives as a DMA-BUF, which cannot be read as memory.
            assert!(property(&properties, FormatProperties::VideoModifier).is_none());
        }

        #[test]
        fn the_offer_takes_a_screen_that_has_no_fixed_rate_and_one_that_has() {
            // A desktop says that its rate varies (0/1, with a most) and a source of video has a rate of its own (30/1): both have to be
            // inside what is offered, or the negotiation of the format ends in "no more input formats".
            use spa::param::format::FormatProperties;
            let properties = offered(15);

            let Some(spa::pod::Value::Choice(spa::pod::ChoiceValue::Fraction(spa::utils::Choice(
                _,
                spa::utils::ChoiceEnum::Range { min, max, .. },
            )))) = property(&properties, FormatProperties::VideoFramerate)
            else {
                panic!("the rates are a range of fractions");
            };
            assert_eq!((min.num, min.denom), (0, 1));
            assert!(max.num / max.denom >= 240, "{max:?}");
        }

        #[test]
        fn the_offer_says_how_many_pictures_a_second_are_wanted_at_most() {
            use spa::param::format::FormatProperties;

            let properties = offered(15);

            let Some(spa::pod::Value::Choice(spa::pod::ChoiceValue::Fraction(spa::utils::Choice(
                _,
                spa::utils::ChoiceEnum::Range { default, max, .. },
            )))) = property(&properties, FormatProperties::VideoMaxFramerate)
            else {
                panic!("the most pictures a second are a range of fractions");
            };
            assert_eq!((default.num, default.denom), (15, 1));
            assert_eq!((max.num, max.denom), (15, 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Mutex;

    // ---- The pictures ----

    #[test]
    fn rgbx_is_swizzled_to_bgra() {
        // Two pixels: red and green, as RGBx has them; the byte after the blue is not used, and what is in it does not matter.
        let rgbx = [255, 0, 0, 7, 0, 255, 0, 9];

        let frame = picture(PixelFormat::Rgbx, 2, 1, 8, &rgbx).expect("a picture");

        // BGRA: red is [0, 0, 255, 255] and green is [0, 255, 0, 255].
        assert_eq!(frame.bgra, [0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn bgrx_and_bgra_keep_their_colours_and_have_an_opaque_alpha() {
        let blue_unused = [200, 100, 50, 0, 1, 2, 3, 128];

        for format in [PixelFormat::Bgrx, PixelFormat::Bgra] {
            let frame = picture(format, 2, 1, 8, &blue_unused).expect("a picture");

            assert_eq!(frame.bgra, [200, 100, 50, 255, 1, 2, 3, 255], "{format:?}");
        }
    }

    #[test]
    fn a_stride_wider_than_the_row_is_copied_row_by_row() {
        // 2 x 3 pixels, a row of 8 bytes in a stride of 12: the 4 bytes of padding (9s) are not in the picture.
        let mut buffer = Vec::new();
        for row in 0..3u8 {
            buffer.extend([row, 10, 20, 0, row, 11, 21, 0]);
            buffer.extend([9, 9, 9, 9]);
        }

        let frame = picture(PixelFormat::Bgrx, 2, 3, 12, &buffer).expect("a picture");

        assert_eq!((frame.width, frame.height), (2, 3));
        // The rows follow one another: the stride of the frame is its row.
        assert_eq!(frame.stride, 8);
        assert_eq!(
            frame.bgra,
            [
                0, 10, 20, 255, 0, 11, 21, 255, //
                1, 10, 20, 255, 1, 11, 21, 255, //
                2, 10, 20, 255, 2, 11, 21, 255,
            ]
        );
    }

    #[test]
    fn a_last_row_without_its_padding_is_still_a_picture() {
        // 1 x 2 pixels in a stride of 8: the buffer ends where the second pixel does.
        let buffer = [1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 0];

        let frame = picture(PixelFormat::Bgrx, 1, 2, 8, &buffer).expect("a picture");

        assert_eq!(frame.bgra, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn a_stride_of_nothing_means_rows_with_no_padding() {
        let buffer = [1, 2, 3, 0, 4, 5, 6, 0];

        let frame = picture(PixelFormat::Bgrx, 1, 2, 0, &buffer).expect("a picture");

        assert_eq!(frame.stride, 4);
        assert_eq!(frame.bgra, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn what_is_not_a_whole_picture_is_not_one() {
        let format = PixelFormat::Bgrx;
        // Too few bytes for the rows, a stride that is narrower than the pixels, one that goes up the screen, and nothing to see.
        assert!(picture(format, 2, 2, 8, &[0; 15]).is_none());
        assert!(picture(format, 4, 1, 8, &[0; 16]).is_none());
        assert!(picture(format, 2, 2, -8, &[0; 16]).is_none());
        assert!(picture(format, 0, 2, 8, &[0; 16]).is_none());
        assert!(picture(format, 2, 0, 8, &[0; 16]).is_none());
    }

    #[test]
    fn a_chunk_is_read_from_its_offset_for_its_size() {
        let buffer = [0, 1, 2, 3, 4, 5, 6, 7];

        assert_eq!(chunk_bytes(&buffer, 2, 4), Some(&buffer[2..6]));
        assert_eq!(chunk_bytes(&buffer, 0, 8), Some(&buffer[..]));
    }

    #[test]
    fn a_chunk_with_nothing_in_it_or_that_reaches_past_the_buffer_is_not_read() {
        let buffer = [0; 8];

        // No size is a buffer that has no new picture in it.
        assert_eq!(chunk_bytes(&buffer, 0, 0), None);
        assert_eq!(chunk_bytes(&buffer, 4, 5), None);
        assert_eq!(chunk_bytes(&buffer, 9, 1), None);
        // The sum of the two does not fit.
        assert_eq!(chunk_bytes(&buffer, u32::MAX, u32::MAX), None);
    }

    // ---- The restore token ----

    /// A store in memory, which counts what it was asked.
    #[derive(Default)]
    struct Memory {
        text: Mutex<Option<String>>,
        saves: Mutex<u32>,
    }

    impl TokenStore for Memory {
        fn load(&self) -> Option<String> {
            self.text.lock().unwrap().clone()
        }

        fn save(&self, token: &str) {
            *self.text.lock().unwrap() = Some(token.to_owned());
            *self.saves.lock().unwrap() += 1;
        }
    }

    #[test]
    fn the_restore_token_is_saved_and_reused() {
        let store = Memory::default();

        // The first start has none to give the portal, which shows the dialog; the portal gives one with the choice.
        assert_eq!(kept_token(&store, Target::Display), None);
        keep_token(&store, Target::Display, Some("f3a1"));

        // The next one gives it back.
        assert_eq!(kept_token(&store, Target::Display).as_deref(), Some("f3a1"));
        // And the one the portal gives then is the one that is kept.
        keep_token(&store, Target::Display, Some("9c2e"));
        assert_eq!(kept_token(&store, Target::Display).as_deref(), Some("9c2e"));
    }

    #[test]
    fn the_token_of_a_window_is_not_given_for_a_monitor_and_the_other_way_round() {
        // A token restores what was chosen, whatever kind is asked for: the one of a window would record that window when the person
        // asked for a monitor.
        let store = Memory::default();
        keep_token(&store, Target::Window, Some("w1"));

        assert_eq!(kept_token(&store, Target::Display), None);
        assert_eq!(kept_token(&store, Target::Window).as_deref(), Some("w1"));

        keep_token(&store, Target::Display, Some("d1"));
        assert_eq!(kept_token(&store, Target::Window), None);
        assert_eq!(kept_token(&store, Target::Display).as_deref(), Some("d1"));
    }

    #[test]
    fn a_start_that_was_given_no_token_keeps_the_one_it_had() {
        // A desktop that does not keep choices gives none: what is kept stays, and is of use to one that does.
        let store = Memory::default();
        keep_token(&store, Target::Display, Some("d1"));

        keep_token(&store, Target::Display, None);
        keep_token(&store, Target::Display, Some(""));

        assert_eq!(kept_token(&store, Target::Display).as_deref(), Some("d1"));
        assert_eq!(*store.saves.lock().unwrap(), 1);
    }

    #[test]
    fn a_store_with_text_that_is_not_a_token_of_ours_gives_no_token() {
        let store = Memory::default();
        // Another version of the app, or a settings file that was edited by hand.
        store.save("a-token-with-no-kind");
        assert_eq!(kept_token(&store, Target::Display), None);
        store.save("display:");
        assert_eq!(kept_token(&store, Target::Display), None);
    }

    #[test]
    fn no_tokens_keeps_nothing() {
        keep_token(&NoTokens, Target::Window, Some("w1"));

        assert_eq!(kept_token(&NoTokens, Target::Window), None);
    }

    // ---- What is captured, and how a start is told ----

    #[test]
    fn a_screen_is_a_monitor_and_a_window_is_a_window_and_the_ids_are_not_used() {
        let screen = |display: Option<&str>| Mode::Screen {
            display: display.map(Into::into),
        };

        assert_eq!(target_of(&screen(None)), Ok(Target::Display));
        // The system's dialog picks: a number that the picker of another system handed out is not a reason to refuse.
        assert_eq!(target_of(&screen(Some("2"))), Ok(Target::Display));
        assert_eq!(
            target_of(&Mode::Window { id: "12345".into() }),
            Ok(Target::Window)
        );
    }

    #[test]
    fn a_recording_of_sound_alone_has_no_screen_to_take() {
        assert_eq!(
            target_of(&Mode::Audio),
            Err(StartError::Source(
                "the screen is not recorded in a recording of sound".into()
            ))
        );
    }

    #[test]
    fn a_start_that_the_portal_refuses_says_why() {
        assert_eq!(start_error(Refusal::Cancelled), StartError::PortalCancelled);
        assert_eq!(
            start_error(Refusal::Unavailable),
            StartError::PortalUnavailable
        );
        assert_eq!(
            start_error(Refusal::Other("the backend crashed".into())),
            StartError::Source("the backend crashed".into())
        );
        // Not a cancel: the person did not choose, and is told that nothing was chosen in the time given.
        assert_eq!(
            start_error(Refusal::NoAnswer),
            StartError::Source(
                "nothing was chosen in the dialog of the desktop within 5 minutes".into()
            )
        );
    }

    #[test]
    fn a_portal_that_answers_makes_the_system_the_picker() {
        assert_eq!(
            portal_sources(true),
            Ok(Sources {
                displays: Vec::new(),
                windows: Vec::new(),
                picker_is_the_system: true,
            })
        );
    }

    #[test]
    fn no_portal_is_a_start_that_has_none() {
        assert_eq!(portal_sources(false), Err(StartError::PortalUnavailable));
    }

    // ---- How the capture ends ----

    #[test]
    fn a_closed_stream_ends_a_window_as_closed_and_a_monitor_as_gone() {
        for (target, reason) in [
            (Target::Window, EndReason::WindowClosed),
            (Target::Display, EndReason::DisplayGone),
        ] {
            let (tx, rx) = mpsc::channel();

            Ender::new(tx, target).ended();

            assert!(
                matches!(rx.try_recv(), Ok(SourceEvent::Ended { track: Track::Screen, reason: got }) if got == reason),
                "{target:?}"
            );
        }
    }

    #[test]
    fn the_end_is_told_once_whoever_sees_it_first() {
        // The stream and the portal session both see the end of a capture.
        let (tx, rx) = mpsc::channel();
        let ender = Ender::new(tx, Target::Display);

        ender.clone().ended();
        ender.ended();

        assert!(matches!(rx.try_recv(), Ok(SourceEvent::Ended { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_capture_that_was_stopped_says_nothing_of_its_end() {
        let (tx, rx) = mpsc::channel();
        let ender = Ender::new(tx, Target::Window);

        ender.stopped();
        ender.ended();

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_failed_stream_ends_the_track_with_what_went_wrong_once() {
        let (tx, rx) = mpsc::channel();
        let ender = Ender::new(tx, Target::Display);

        ender.failed("the pictures cannot be read");
        // Whatever comes after is the same end.
        ender.ended();
        ender.failed("again");

        assert!(matches!(
            rx.try_recv(),
            Ok(SourceEvent::Ended { track: Track::Screen, reason: EndReason::Failed(why) }) if why == "the pictures cannot be read"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_capture_that_was_stopped_does_not_fail() {
        let (tx, rx) = mpsc::channel();
        let ender = Ender::new(tx, Target::Display);

        ender.stopped();
        ender.failed("the pictures cannot be read");

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_capture_is_given_up_after_a_run_of_pictures_that_cannot_be_read_and_not_before() {
        let mut unreadable = Unreadable::default();

        for _ in 1..UNREADABLE_PICTURES {
            assert!(!unreadable.fail(), "one that was bad once");
        }
        assert!(
            unreadable.fail(),
            "a run of them is a stream that gives nothing"
        );
    }

    #[test]
    fn a_picture_that_is_read_ends_the_run() {
        let mut unreadable = Unreadable::default();

        for _ in 0..3 {
            for _ in 1..UNREADABLE_PICTURES {
                assert!(!unreadable.fail());
            }
            unreadable.read();
        }
    }

    // ---- The buffers of the stream ----

    /// A picture of 2 x 1 pixels in BGRx, of `shade`: what a buffer of the stream carries.
    fn bytes_of(shade: u8) -> Vec<u8> {
        vec![shade, shade, shade, 0, shade, shade, shade, 0]
    }

    /// A buffer that holds `data` (8 bytes of 2 x 1 pixels), as the stream hands it over.
    fn buffer(data: &[u8]) -> Buffer<'_> {
        Buffer {
            offset: 0,
            size: data.len() as u32,
            stride: 8,
            corrupted: false,
            data: Some(data),
        }
    }

    /// The pictures of a stream that said its pictures are BGRx and 2 x 1, and the channel that they go to.
    fn pictures() -> (Pictures, mpsc::Receiver<SourceEvent>) {
        let (tx, rx) = mpsc::channel();
        let mut pictures = Pictures::new(tx);
        pictures.say(Some(PixelFormat::Bgrx), (2, 1));
        (pictures, rx)
    }

    fn shade_of(event: SourceEvent) -> u8 {
        match event {
            SourceEvent::Frame(frame) => frame.bgra[0],
            _ => panic!("not a picture"),
        }
    }

    #[test]
    fn every_picture_of_a_quick_burst_reaches_the_channel_and_the_last_one_is_the_last() {
        // A screen sends a picture when something changes, and then nothing when it stops changing: what the last one shows is what the
        // video shows from then on, so none of a burst (a slide that fades, a line that is typed) may be held back, least of all the end.
        let (mut pictures, rx) = pictures();

        for shade in 1..=20u8 {
            assert!(!pictures.arrived(buffer(&bytes_of(shade))));
        }

        let shades: Vec<u8> = rx.try_iter().map(shade_of).collect();
        assert_eq!(shades, (1..=20).collect::<Vec<u8>>());
    }

    #[test]
    fn a_buffer_with_nothing_in_it_or_a_damaged_one_is_no_picture_and_takes_nothing_with_it() {
        // The first buffer of a stream may be an empty one, and the picture that matters (all there is of a screen that does not
        // change) follows at once: whatever comes before it must not use up its place.
        let (mut pictures, rx) = pictures();
        let data = bytes_of(7);

        for _ in 0..UNREADABLE_PICTURES * 2 {
            let empty = Buffer {
                size: 0,
                ..buffer(&data)
            };
            let damaged = Buffer {
                corrupted: true,
                ..buffer(&data)
            };
            assert!(!pictures.arrived(empty));
            assert!(!pictures.arrived(damaged));
        }
        assert!(rx.try_recv().is_err(), "nothing was sent for them");

        assert!(!pictures.arrived(buffer(&data)));
        assert_eq!(shade_of(rx.try_recv().expect("the picture")), 7);
    }

    #[test]
    fn a_run_of_buffers_that_cannot_be_read_gives_the_capture_up_and_sends_nothing() {
        let (mut pictures, rx) = pictures();
        let unmapped = Buffer {
            data: None,
            ..buffer(&[])
        };

        for _ in 1..UNREADABLE_PICTURES {
            assert!(!pictures.arrived(Buffer {
                size: 8,
                ..unmapped
            }));
        }
        assert!(pictures.arrived(Buffer {
            size: 8,
            ..unmapped
        }));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_buffer_that_is_read_ends_the_run_of_the_ones_that_were_not() {
        let (mut pictures, rx) = pictures();
        let unmapped = Buffer {
            size: 8,
            data: None,
            ..buffer(&[])
        };

        for _ in 0..3 {
            for _ in 1..UNREADABLE_PICTURES {
                assert!(!pictures.arrived(Buffer { ..unmapped }));
            }
            assert!(!pictures.arrived(buffer(&bytes_of(1))));
        }
        assert_eq!(rx.try_iter().count(), 3);
    }

    #[test]
    fn buffers_before_the_stream_has_said_what_its_pictures_are_cannot_be_read() {
        // Or in a format that is not offered: they are not pictures, and a run of them is a stream that gives nothing.
        let (tx, rx) = mpsc::channel();
        let mut pictures = Pictures::new(tx);
        let data = bytes_of(1);

        for _ in 1..UNREADABLE_PICTURES {
            assert!(!pictures.arrived(buffer(&data)));
        }
        assert!(pictures.arrived(buffer(&data)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_picture_follows_the_size_that_the_stream_last_said() {
        // A window that is resized says its format again, and the next buffer is of the new size.
        let (mut pictures, rx) = pictures();
        assert!(!pictures.arrived(buffer(&bytes_of(1))));

        pictures.say(Some(PixelFormat::Bgrx), (1, 2));
        assert!(!pictures.arrived(Buffer {
            stride: 4,
            ..buffer(&bytes_of(2))
        }));

        let sizes: Vec<(u32, u32)> = rx
            .try_iter()
            .map(|event| match event {
                SourceEvent::Frame(frame) => (frame.width, frame.height),
                _ => panic!("not a picture"),
            })
            .collect();
        assert_eq!(sizes, [(2, 1), (1, 2)]);
    }

    // ---- With the real system: they record this machine's screen, so they are asked for ----

    #[cfg(target_os = "linux")]
    mod real {
        use super::*;
        use crate::media::test_support::real_ffmpeg;
        use crate::record::capture::OsCapturer;
        use crate::record::quality::Quality;
        use crate::record::session::Session;
        use crate::record::test_support::video_seconds;
        use crate::record::{Capturer, RecordOptions};

        /// Stops a test that was not asked for: it records the screen.
        fn require_enabled() {
            assert_eq!(
                std::env::var("DICTATE_TEST_SCREEN").as_deref(),
                Ok("1"),
                "set DICTATE_TEST_SCREEN=1 to record this machine's screen for a few seconds"
            );
        }

        /// The picture alone: a machine that these tests run on may have no sound device to record.
        fn options(mode: Mode) -> RecordOptions {
            RecordOptions {
                mode,
                quality: Quality::Small,
                mic: false,
                system_audio: false,
            }
        }

        #[test]
        #[ignore = "records the screen: needs a desktop session with the ScreenCast portal, and you choose what to share in its dialog; set DICTATE_TEST_SCREEN=1"]
        fn linux_records_three_seconds_through_the_portal() {
            require_enabled();
            let Some(ffmpeg) = real_ffmpeg() else { return };
            // Kept in memory for the two starts of this test: where the desktop keeps what was chosen (GNOME and KDE do), the second one
            // has no dialog.
            let tokens = Arc::new(Memory::default());
            let capturer = OsCapturer::new(Some(ffmpeg.clone())).with_token_store(tokens.clone());
            let screen = || Mode::Screen { display: None };

            // First what the capture delivers on its own: a picture as large as it says, soon after the dialog is answered.
            let (tx, rx) = mpsc::channel();
            let source = capturer
                .start(Track::Screen, &options(screen()), tx)
                .expect("the portal gives the screen");
            let first = loop {
                match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(SourceEvent::Frame(frame)) => break frame,
                    Ok(SourceEvent::Ended { reason, .. }) => {
                        panic!("the capture ended: {reason:?}")
                    }
                    Ok(_) => {}
                    Err(_) => panic!("no picture in 10 s of a capture of the screen"),
                }
            };
            source.stop();
            eprintln!(
                "first picture: {}x{}, stride {}; the desktop kept a token: {}",
                first.width,
                first.height,
                first.stride,
                tokens.load().is_some()
            );
            assert_eq!(first.stride, first.width * 4);
            assert_eq!(first.bgra.len(), (first.stride * first.height) as usize);
            assert!(
                first
                    .bgra
                    .chunks_exact(4)
                    .any(|pixel| pixel[..3] != [0, 0, 0]),
                "a picture that is all black"
            );
            assert!(
                first.bgra.chunks_exact(4).all(|pixel| pixel[3] == 255),
                "a picture that is not opaque"
            );

            // And then what a recording makes of it.
            let dir = tempfile::tempdir().unwrap();
            let session = Session::start(
                options(screen()),
                dir.path(),
                Arc::new(capturer),
                Some(ffmpeg.clone()),
                Box::new(|_| {}),
            )
            .expect("the session starts");
            std::thread::sleep(Duration::from_secs(3));
            let finished = session.stop().expect("the recording is made");
            eprintln!(
                "{:?}: {} s, notes: {:?}",
                finished.media.file_name(),
                finished.duration,
                finished.notes
            );

            // No sound is recorded, so the video is what is left, as it is.
            let name = finished
                .media
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(name.starts_with("video."), "{name}");
            assert!(
                (finished.duration - 3.0).abs() <= 0.5,
                "{} s recorded",
                finished.duration
            );
            let seconds = video_seconds(&finished.media, &ffmpeg);
            assert!((seconds - 3.0).abs() <= 0.5, "{seconds} s of video");
        }
    }
}
