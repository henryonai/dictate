//! The screen on Windows: a display or a window, recorded through the Graphics Capture API (the `windows-capture` crate).
//!
//! The capture runs on a thread of its own, which the crate starts (`start_free_threaded`) and which has the dispatcher queue
//! that the API's events need. What it delivers are pictures in BGRA, at the size of what is recorded (a window has the size
//! of the window, and the session fits that to the video): the API sends a picture when something changes, so a screen that
//! stays as it is sends none, and neither does a window that is minimised. The session repeats the last picture meanwhile.
//!
//! The ways in which a capture ends are the ones of the API: the window is closed or the display is unplugged (the item's
//! `Closed` event, which ends the screen track and with it the recording), and the pictures that cannot be read any more (the
//! graphics card was lost), which end it with the reason that the system gave.
//!
//! The decisions of all that (what the picker lists, which display or window a recording is about, what the end of a capture
//! is, what a start that is refused says) are plain code, built on every system, so that their tests run everywhere; the part
//! that calls the system is `native`, for Windows.

// The plain code is for the other systems to test: what only the capture of Windows uses is not dead there.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::time::Duration;

use super::{SourceInfo, Sources};
use crate::record::video::Frame;
use crate::record::{
    EndReason, Mode, RecordOptions, RunningSource, SourceEvent, StartError, Track,
};

/// How many pictures that cannot be read, one after the other, the capture puts up with. A picture that cannot be read once is a
/// graphics card that was busy; a run of them (a second or so, at the rate the API sends them) is one that is gone.
const UNREADABLE_PICTURES: u32 = 30;

/// How long the end of a capture is waited for. It is a few tens of milliseconds, unless the graphics card does not answer, and
/// `stop` must not wait for that.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// A window of the picker, as the system describes it.
struct WindowFacts {
    /// The window's handle (`HWND`).
    id: usize,
    /// Whether it is a window that can be captured: visible, not a tool window, not a child.
    valid: bool,
    pid: Option<u32>,
    /// The name of the program that owns it (`notepad.exe`), when the system says.
    process: Option<String>,
    title: String,
}

/// A display of the picker, as the system describes it.
struct DisplayFacts {
    /// Its number in the list of the system, from 1.
    id: usize,
    /// In pixels.
    size: Option<(u32, u32)>,
}

/// What the capture is of: a display or a window.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Display,
    Window,
}

/// What a recording asks to capture, read from the ids that the picker handed out.
#[derive(Debug, PartialEq)]
enum Wanted {
    /// The main display.
    MainDisplay,
    /// The display of that number, from 1.
    Display(usize),
    /// The window of that handle.
    Window(usize),
}

/// What the picker calls a window: `"<process> — <title>"`, with `?` for a program that the system does not name (a window of
/// one that runs as an administrator does not say; the Swift app did the same).
fn window_name(process: Option<&str>, title: &str) -> String {
    let process = process.filter(|process| !process.is_empty()).unwrap_or("?");
    format!("{process} — {title}")
}

/// The windows that the picker offers, sorted by name (then by handle, to keep the order the same from one look to the next):
/// those that can be captured, that have a title, and that are not Dictate's own (`own_pid`).
fn listed_windows(windows: &[WindowFacts], own_pid: u32) -> Vec<SourceInfo> {
    let mut listed: Vec<(usize, SourceInfo)> = windows
        .iter()
        .filter(|window| {
            window.valid && window.pid != Some(own_pid) && !window.title.trim().is_empty()
        })
        .map(|window| {
            let info = SourceInfo {
                id: window.id.to_string(),
                name: window_name(window.process.as_deref(), &window.title),
                app: window.process.clone().filter(|process| !process.is_empty()),
            };
            (window.id, info)
        })
        .collect();
    listed.sort_by(|(a_id, a), (b_id, b)| a.name.cmp(&b.name).then(a_id.cmp(b_id)));
    listed.into_iter().map(|(_, info)| info).collect()
}

/// The displays that the picker offers, the main one (`main`, by number) first and the others as the system lists them. The id is
/// the number in the system's list; the name counts from 1 in the order of the picker (as on macOS).
fn listed_displays(displays: &[DisplayFacts], main: Option<usize>) -> Vec<SourceInfo> {
    let mut ordered: Vec<&DisplayFacts> = displays.iter().collect();
    // A stable sort: `false` (the main display) comes before `true`.
    ordered.sort_by_key(|display| Some(display.id) != main);
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, display)| SourceInfo {
            id: display.id.to_string(),
            name: match display.size {
                Some((width, height)) => format!("Display {} ({width}x{height})", position + 1),
                None => format!("Display {}", position + 1),
            },
            app: None,
        })
        .collect()
}

/// What a recording of `mode` asks to capture. A display that is not a number of the list (the picker hands out only numbers from
/// 1) is one that is not there, and so is a window that is not a handle.
fn wanted(mode: &Mode) -> Result<Wanted, StartError> {
    match mode {
        Mode::Screen { display: None } => Ok(Wanted::MainDisplay),
        Mode::Screen { display: Some(id) } => id
            .parse::<usize>()
            .ok()
            .filter(|number| *number >= 1)
            .map(Wanted::Display)
            .ok_or_else(display_gone),
        Mode::Window { id } => id
            .parse::<usize>()
            .map(Wanted::Window)
            .map_err(|_| StartError::WindowGone),
        Mode::Audio => Err(StartError::Source(
            "the screen is not recorded in a recording of sound".into(),
        )),
    }
}

fn display_gone() -> StartError {
    StartError::Source("that display is not connected any more".into())
}

/// How a capture ends when the system closes it: the window was closed, or the display was unplugged. This ends the screen track,
/// and with it the recording.
fn closed(target: Target) -> SourceEvent {
    SourceEvent::Ended {
        track: Track::Screen,
        reason: match target {
            Target::Display => EndReason::DisplayGone,
            Target::Window => EndReason::WindowClosed,
        },
    }
}

/// The picture that `bytes` are, in `height` rows of `stride` bytes of BGRA pixels `width` wide: what is past the last row is not
/// kept. `None` for what is not a picture (no pixels, rows that are shorter than their pixels, bytes that are too few).
fn picture(width: u32, height: u32, stride: u32, bytes: &[u8]) -> Option<Frame> {
    let row = (width as usize).checked_mul(4)?;
    let size = (stride as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || (stride as usize) < row || bytes.len() < size {
        return None;
    }
    Some(Frame {
        width,
        height,
        stride,
        bgra: bytes[..size].to_vec(),
    })
}

/// How many pictures in a row could not be read.
#[derive(Default)]
struct Unreadable(u32);

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

/// Why the system did not start a capture, as far as this module tells them apart.
#[derive(Debug, PartialEq)]
enum Refused {
    /// Graphics capture is not there (Windows older than 1903, or no graphics card for it).
    Unsupported,
    /// The window or the display is not there any more.
    TargetGone,
    /// What was asked besides the picture (the cursor, no border, a pace) was not given; the text says what.
    Extras(String),
    Other(String),
}

/// What a start that the system refused is, as the session and the person hear of it.
fn start_error(refused: Refused, target: Target) -> StartError {
    match refused {
        Refused::Unsupported => {
            StartError::Source("graphics capture is not supported on this PC".into())
        }
        Refused::TargetGone => match target {
            Target::Window => StartError::WindowGone,
            Target::Display => display_gone(),
        },
        Refused::Extras(text) | Refused::Other(text) => StartError::Source(text),
    }
}

/// The least time between the pictures that the capture is asked for: one frame of the video. A screen is not copied 60 times a
/// second for a video of 10.
fn frame_interval(fps: u32) -> Duration {
    Duration::from_micros(1_000_000 / u64::from(fps.max(1)))
}

#[cfg(windows)]
pub use native::{list_sources, start};

/// The capture itself, which calls the system and is for Windows.
#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::{self, Sender};
    use std::thread;

    use windows_capture::capture::{
        CaptureControl, Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler,
    };
    use windows_capture::frame::Frame as Captured;
    use windows_capture::graphics_capture_api::{Error as Api, InternalCaptureControl};
    use windows_capture::monitor::{Error as MonitorError, Monitor};
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
        TryIntoCaptureItemWithType,
    };
    use windows_capture::window::Window;

    use super::*;

    /// The handler of the capture thread: the screen track's channel is in it from the moment it exists, which is before the
    /// capture starts, so that the first picture (all there is of a screen that does not change) has somewhere to go.
    struct Handler {
        tx: Sender<SourceEvent>,
        target: Target,
        unreadable: Unreadable,
    }

    impl GraphicsCaptureApiHandler for Handler {
        type Flags = (Sender<SourceEvent>, Target);
        type Error = String;

        fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
            let (tx, target) = context.flags;
            Ok(Handler {
                tx,
                target,
                unreadable: Unreadable::default(),
            })
        }

        /// A picture: copied, and sent to the screen track. It is the thread of the capture that is called, and it does not wait
        /// for anything: the channel has no limit.
        fn on_frame_arrived(
            &mut self,
            frame: &mut Captured,
            _control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            match read(frame) {
                Ok(picture) => {
                    self.unreadable.read();
                    // A session that is over has no one to read.
                    let _ = self.tx.send(SourceEvent::Frame(picture));
                    Ok(())
                }
                Err(why) => {
                    if self.unreadable.fail() {
                        let _ = self.tx.send(SourceEvent::Ended {
                            track: Track::Screen,
                            reason: EndReason::Failed(why.clone()),
                        });
                        // The capture stops with the handler's error.
                        return Err(why);
                    }
                    Ok(())
                }
            }
        }

        /// The window was closed, or the display was unplugged: the crate stops the capture after this.
        fn on_closed(&mut self) -> Result<(), Self::Error> {
            let _ = self.tx.send(closed(self.target));
            Ok(())
        }
    }

    /// The picture of a frame of the capture, copied with its stride; or why it cannot be had.
    fn read(frame: &mut Captured) -> Result<Frame, String> {
        let mut buffer = frame.buffer().map_err(|error| error.to_string())?;
        let (width, height, stride) = (buffer.width(), buffer.height(), buffer.row_pitch());
        picture(width, height, stride, buffer.as_raw_buffer()).ok_or_else(|| {
            format!("a picture of {width}x{height} pixels, in rows of {stride} bytes, cannot be recorded")
        })
    }

    /// What was refused, from what the crate says.
    fn refused(error: &GraphicsCaptureApiError<String>) -> Refused {
        match error {
            GraphicsCaptureApiError::GraphicsCaptureApiError(Api::Unsupported) => {
                Refused::Unsupported
            }
            // A window whose handle the system does not know, or that it cannot measure, is gone.
            GraphicsCaptureApiError::ItemConvertFailed
            | GraphicsCaptureApiError::GraphicsCaptureApiError(Api::WindowError(_)) => {
                Refused::TargetGone
            }
            GraphicsCaptureApiError::GraphicsCaptureApiError(
                Api::CursorConfigUnsupported
                | Api::BorderConfigUnsupported
                | Api::MinimumUpdateIntervalUnsupported
                | Api::WindowsError(_),
            ) => Refused::Extras(error.to_string()),
            other => Refused::Other(other.to_string()),
        }
    }

    /// A capture that runs: stopped by `stop`, or by being dropped.
    struct Capture {
        control: Option<CaptureControl<Handler, String>>,
    }

    impl RunningSource for Capture {
        fn stop(self: Box<Self>) {
            // `drop` does it.
            drop(self);
        }
    }

    impl Drop for Capture {
        /// Ends the capture, and waits for it for [`STOP_WAIT`] at the most: the stop is on a thread of its own, so that a graphics
        /// card that does not answer does not hold the session's thread.
        fn drop(&mut self) {
            let Some(control) = self.control.take() else {
                return;
            };
            // From here on nothing is sent: the handler looks at this before it does anything.
            control.halt_handle().store(true, Ordering::Relaxed);
            let (done, finished) = mpsc::channel();
            let spawned = thread::Builder::new()
                .name("stop the screen capture".into())
                .spawn(move || {
                    // Whatever it says (the capture had ended by itself, and says so), there is nothing to do about it.
                    let _ = control.stop();
                    let _ = done.send(());
                });
            if spawned.is_ok() {
                let _ = finished.recv_timeout(STOP_WAIT);
            }
        }
    }

    /// Starts the capture of `item` (a display or a window, `target`) on a thread of its own, sending the pictures and the end to
    /// `tx`. Besides the picture it asks for the cursor, no border around a window (the yellow one of the system) and no more than
    /// one picture per frame of the video; a Windows that has none of these, or refuses them, is asked again without, and gives the
    /// picture it has.
    fn begin<T>(
        item: T,
        target: Target,
        options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError>
    where
        T: TryIntoCaptureItemWithType + Send + Copy + 'static,
    {
        let interval = frame_interval(options.quality.spec().fps);
        let settings = |extras: bool| {
            Settings::new(
                item,
                if extras {
                    CursorCaptureSettings::WithCursor
                } else {
                    CursorCaptureSettings::Default
                },
                if extras {
                    DrawBorderSettings::WithoutBorder
                } else {
                    DrawBorderSettings::Default
                },
                SecondaryWindowSettings::Default,
                if extras {
                    MinimumUpdateIntervalSettings::Custom(interval)
                } else {
                    MinimumUpdateIntervalSettings::Default
                },
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                (tx.clone(), target),
            )
        };
        let started = match Handler::start_free_threaded(settings(true)) {
            Err(error) if matches!(refused(&error), Refused::Extras(_)) => {
                Handler::start_free_threaded(settings(false))
            }
            started => started,
        };
        let control = started.map_err(|error| start_error(refused(&error), target))?;
        Ok(Box::new(Capture {
            control: Some(control),
        }))
    }

    /// Starts the screen track: the display or the window that `options` name, on the capture thread.
    pub fn start(
        options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        match wanted(&options.mode)? {
            Wanted::Window(handle) => {
                let window = Window::from_raw_hwnd(handle as *mut c_void);
                if !window.is_valid() {
                    return Err(StartError::WindowGone);
                }
                begin(window, Target::Window, options, tx)
            }
            Wanted::MainDisplay => {
                let main = Monitor::primary()
                    .map_err(|_| StartError::Source("no display found to record".into()))?;
                begin(main, Target::Display, options, tx)
            }
            Wanted::Display(number) => {
                let monitor = Monitor::from_index(number).map_err(|error| match error {
                    MonitorError::NotFound | MonitorError::IndexIsLowerThanOne => display_gone(),
                    other => StartError::Source(other.to_string()),
                })?;
                begin(monitor, Target::Display, options, tx)
            }
        }
    }

    /// The displays and windows that can be recorded, for the picker: the windows that Graphics Capture can take, with a title,
    /// and not Dictate's own.
    pub fn list_sources() -> Result<Sources, StartError> {
        let monitors =
            Monitor::enumerate().map_err(|error| StartError::Source(error.to_string()))?;
        let main = Monitor::primary()
            .ok()
            .and_then(|main| monitors.iter().position(|monitor| *monitor == main))
            .map(|at| at + 1);
        let displays: Vec<DisplayFacts> = monitors
            .iter()
            .enumerate()
            .map(|(at, monitor)| DisplayFacts {
                id: at + 1,
                size: monitor.width().ok().zip(monitor.height().ok()),
            })
            .collect();
        let windows: Vec<WindowFacts> = Window::enumerate()
            .map_err(|error| StartError::Source(error.to_string()))?
            .iter()
            .map(|window| WindowFacts {
                id: window.as_raw_hwnd() as usize,
                valid: window.is_valid(),
                pid: window.process_id().ok(),
                process: window.process_name().ok(),
                title: window.title().unwrap_or_default(),
            })
            .collect();
        Ok(Sources {
            displays: listed_displays(&displays, main),
            windows: listed_windows(&windows, std::process::id()),
            picker_is_the_system: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: usize, process: &str, title: &str) -> WindowFacts {
        WindowFacts {
            id,
            valid: true,
            pid: Some(100 + id as u32),
            process: Some(process.into()),
            title: title.into(),
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
    fn window_names_use_the_process_and_title() {
        assert_eq!(
            window_name(Some("chrome.exe"), "Inbox - Mail"),
            "chrome.exe — Inbox - Mail"
        );
        // A process that the system does not tell the name of (one that runs as an administrator) is a question mark.
        assert_eq!(window_name(None, "Inbox"), "? — Inbox");
        assert_eq!(window_name(Some(""), "Inbox"), "? — Inbox");
    }

    #[test]
    fn own_and_untitled_windows_are_not_listed() {
        let mut own = window(1, "dictate.exe", "Settings");
        own.pid = Some(4242);
        let mut untitled = window(2, "explorer.exe", "");
        untitled.pid = Some(7);
        let mut blank = window(3, "explorer.exe", "  ");
        blank.pid = Some(7);
        let mut invalid = window(4, "notepad.exe", "Untitled - Notepad");
        invalid.valid = false;

        let listed = listed_windows(
            &[
                own,
                untitled,
                blank,
                invalid,
                window(5, "code.exe", "main.rs"),
            ],
            4242,
        );

        assert_eq!(
            listed,
            vec![info("5", "code.exe — main.rs", Some("code.exe"))]
        );
    }

    #[test]
    fn a_window_of_a_process_that_has_no_name_is_listed_under_a_question_mark() {
        let mut orphan = window(7, "", "Loose");
        orphan.process = None;
        orphan.pid = None;

        assert_eq!(
            listed_windows(&[orphan], 1),
            vec![info("7", "? — Loose", None)]
        );
    }

    #[test]
    fn windows_are_sorted_by_name_and_then_by_handle() {
        let listed = listed_windows(
            &[
                window(9, "msedge.exe", "Docs"),
                window(4, "notepad.exe", "a"),
                window(3, "msedge.exe", "Docs"),
                window(5, "code.exe", "main.rs"),
            ],
            1,
        );

        let ids: Vec<&str> = listed.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["5", "3", "9", "4"]);
    }

    #[test]
    fn the_id_of_a_window_is_its_handle_as_decimal_text() {
        let listed = listed_windows(&[window(0x0001_0A2C, "a.exe", "A")], 1);

        assert_eq!(listed[0].id, "68140");
        assert_eq!(
            wanted(&Mode::Window {
                id: listed[0].id.clone()
            }),
            Ok(Wanted::Window(68140))
        );
    }

    #[test]
    fn displays_are_numbered_with_the_main_one_first() {
        let displays = [
            DisplayFacts {
                id: 1,
                size: Some((1920, 1080)),
            },
            DisplayFacts {
                id: 2,
                size: Some((2560, 1440)),
            },
            DisplayFacts { id: 3, size: None },
        ];

        let listed = listed_displays(&displays, Some(2));

        assert_eq!(
            listed,
            vec![
                info("2", "Display 1 (2560x1440)", None),
                info("1", "Display 2 (1920x1080)", None),
                info("3", "Display 3", None),
            ]
        );
    }

    #[test]
    fn displays_keep_the_systems_order_when_none_is_the_main_one() {
        let displays = [
            DisplayFacts {
                id: 1,
                size: Some((100, 50)),
            },
            DisplayFacts {
                id: 2,
                size: Some((200, 100)),
            },
        ];

        let ids: Vec<String> = listed_displays(&displays, None)
            .into_iter()
            .map(|d| d.id)
            .collect();

        assert_eq!(ids, ["1", "2"]);
    }

    #[test]
    fn a_recording_names_its_display_by_number_and_none_is_the_main_one() {
        let screen = |display: Option<&str>| Mode::Screen {
            display: display.map(Into::into),
        };

        assert_eq!(wanted(&screen(None)), Ok(Wanted::MainDisplay));
        assert_eq!(wanted(&screen(Some("2"))), Ok(Wanted::Display(2)));
        // Numbered from 1: a number that is not one, or not a number, is a display that is not there.
        for gone in ["0", "-1", "main", ""] {
            assert_eq!(
                wanted(&screen(Some(gone))),
                Err(StartError::Source(
                    "that display is not connected any more".into()
                )),
                "{gone:?}"
            );
        }
    }

    #[test]
    fn a_window_that_is_not_a_handle_is_a_window_that_is_gone() {
        assert_eq!(
            wanted(&Mode::Window {
                id: "Untitled".into()
            }),
            Err(StartError::WindowGone)
        );
    }

    #[test]
    fn a_recording_of_sound_alone_has_no_screen_to_take() {
        assert_eq!(
            wanted(&Mode::Audio),
            Err(StartError::Source(
                "the screen is not recorded in a recording of sound".into()
            ))
        );
    }

    #[test]
    fn a_closed_capture_ends_the_screen_track() {
        assert!(matches!(
            closed(Target::Window),
            SourceEvent::Ended {
                track: Track::Screen,
                reason: EndReason::WindowClosed
            }
        ));
        // A display that is unplugged closes its capture the same way: what it was is what the end says.
        assert!(matches!(
            closed(Target::Display),
            SourceEvent::Ended {
                track: Track::Screen,
                reason: EndReason::DisplayGone
            }
        ));
    }

    #[test]
    fn a_picture_is_kept_with_its_stride() {
        // 2 x 2 pixels of 4 bytes, a row of 12 bytes (4 of them padding).
        let bytes: Vec<u8> = (0..24).collect();

        let frame = picture(2, 2, 12, &bytes).expect("a whole picture");

        assert_eq!((frame.width, frame.height, frame.stride), (2, 2, 12));
        assert_eq!(frame.bgra, bytes);
    }

    #[test]
    fn what_is_past_the_last_row_of_a_picture_is_not_kept() {
        let bytes = vec![7_u8; 30];

        let frame = picture(2, 2, 12, &bytes).expect("a whole picture");

        assert_eq!(frame.bgra.len(), 24);
    }

    #[test]
    fn a_picture_that_is_not_whole_is_not_one() {
        // Too short for its rows, a row that is shorter than its pixels, and nothing to see.
        assert!(picture(2, 2, 12, &[0; 23]).is_none());
        assert!(picture(4, 1, 12, &[0; 12]).is_none());
        assert!(picture(0, 2, 12, &[0; 24]).is_none());
        assert!(picture(2, 0, 12, &[0; 24]).is_none());
    }

    #[test]
    fn the_capture_is_given_up_after_a_run_of_unreadable_pictures_and_not_before() {
        let mut unreadable = Unreadable::default();

        for _ in 1..UNREADABLE_PICTURES {
            assert!(
                !unreadable.fail(),
                "one that the graphics card was too busy for"
            );
        }
        assert!(
            unreadable.fail(),
            "a run of them is a capture that cannot go on"
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

    #[test]
    fn a_start_that_the_system_refuses_says_why_in_the_words_of_the_picker() {
        assert_eq!(
            start_error(Refused::Unsupported, Target::Display),
            StartError::Source("graphics capture is not supported on this PC".into())
        );
        assert_eq!(
            start_error(Refused::TargetGone, Target::Window),
            StartError::WindowGone
        );
        assert_eq!(
            start_error(Refused::TargetGone, Target::Display),
            StartError::Source("that display is not connected any more".into())
        );
        assert_eq!(
            start_error(Refused::Other("access denied".into()), Target::Window),
            StartError::Source("access denied".into())
        );
        assert_eq!(
            start_error(Refused::Extras("no cursor".into()), Target::Window),
            StartError::Source("no cursor".into())
        );
    }

    #[test]
    fn the_capture_is_paced_at_the_frame_rate_of_the_quality() {
        assert_eq!(frame_interval(10), std::time::Duration::from_millis(100));
        assert_eq!(frame_interval(30), std::time::Duration::from_micros(33_333));
        // A rate of nothing is not a pace of no time.
        assert_eq!(frame_interval(0), std::time::Duration::from_secs(1));
    }

    // ---- With the real system: they record this machine's screen, so they are asked for ----

    #[cfg(windows)]
    mod real {
        use crate::media::test_support::real_ffmpeg;
        use crate::record::capture::OsCapturer;
        use crate::record::quality::Quality;
        use crate::record::session::Session;
        use crate::record::test_support::video_seconds;
        use crate::record::{Capturer, EndReason, Mode, RecordOptions, SourceEvent, Track};
        use std::path::{Path, PathBuf};
        use std::process::{Child, Command};
        use std::sync::{mpsc, Arc};
        use std::time::{Duration, Instant};
        use windows_capture::window::Window;

        /// Stops a test that was not asked for: it records the screen.
        fn require_enabled() {
            assert_eq!(
                std::env::var("DICTATE_TEST_SCREEN").as_deref(),
                Ok("1"),
                "set DICTATE_TEST_SCREEN=1 to record this machine's screen for a few seconds"
            );
        }

        /// The picture alone: a machine that these tests run on (a runner of CI) may have no sound device to record.
        fn options(mode: Mode) -> RecordOptions {
            RecordOptions {
                mode,
                quality: Quality::Small,
                mic: false,
                system_audio: false,
            }
        }

        fn start_session(mode: Mode, ffmpeg: PathBuf, folder: &Path) -> Session {
            let capturer = Arc::new(OsCapturer {
                ffmpeg: Some(ffmpeg.clone()),
            });
            Session::start(
                options(mode),
                folder,
                capturer,
                Some(ffmpeg),
                Box::new(|_| {}),
            )
            .expect("the session starts")
        }

        #[test]
        #[ignore = "records the screen: needs a desktop session; set DICTATE_TEST_SCREEN=1"]
        fn windows_records_three_seconds_of_the_primary_monitor() {
            require_enabled();
            let Some(ffmpeg) = real_ffmpeg() else { return };

            // First what the capture delivers on its own: a picture as large as it says, within a few seconds of the start.
            let (tx, rx) = mpsc::channel();
            let source = OsCapturer { ffmpeg: None }
                .start(Track::Screen, &options(Mode::Screen { display: None }), tx)
                .expect("the primary monitor is captured");
            let first = loop {
                match rx.recv_timeout(Duration::from_secs(3)) {
                    Ok(SourceEvent::Frame(frame)) => break frame,
                    Ok(SourceEvent::Ended { reason, .. }) => {
                        panic!("the capture ended: {reason:?}")
                    }
                    Ok(_) => {}
                    Err(_) => panic!("no picture in 3 s of a capture of the primary monitor"),
                }
            };
            source.stop();
            eprintln!(
                "first picture: {}x{}, stride {}",
                first.width, first.height, first.stride
            );
            assert!(first.stride >= first.width * 4);
            assert_eq!(first.bgra.len(), (first.stride * first.height) as usize);
            assert!(
                first
                    .bgra
                    .chunks_exact(4)
                    .any(|pixel| pixel[..3] != [0, 0, 0]),
                "a picture that is all black"
            );

            // And then what a recording makes of it.
            let dir = tempfile::tempdir().unwrap();
            let session = start_session(Mode::Screen { display: None }, ffmpeg.clone(), dir.path());
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

        /// The classic `notepad.exe`, which is ended when this is dropped.
        struct Notepad(Child);

        impl Drop for Notepad {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// The handle of the window of the process `pid`, as the picker would give it, once it has one.
        fn window_of(pid: u32) -> String {
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                let found = Window::enumerate()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|window| {
                        window.process_id().ok() == Some(pid)
                            && window.title().is_ok_and(|title| !title.is_empty())
                    });
                if let Some(window) = found {
                    return (window.as_raw_hwnd() as usize).to_string();
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("no window of the process {pid} in 10 s: is this the classic notepad.exe?");
        }

        #[test]
        #[ignore = "records a window: needs a desktop session and the classic notepad.exe; set DICTATE_TEST_SCREEN=1"]
        fn windows_records_a_window_until_it_closes() {
            require_enabled();
            let Some(ffmpeg) = real_ffmpeg() else { return };
            let mut notepad = Notepad(
                Command::new("notepad.exe")
                    .spawn()
                    .expect("notepad.exe starts"),
            );
            let id = window_of(notepad.0.id());
            let dir = tempfile::tempdir().unwrap();
            let session = start_session(Mode::Window { id }, ffmpeg.clone(), dir.path());

            std::thread::sleep(Duration::from_secs(2));
            notepad.0.kill().expect("notepad.exe is ended");
            let _ = notepad.0.wait();

            // The recording ends by itself, and says why.
            let until = Instant::now() + Duration::from_secs(5);
            let end = loop {
                let end = session.wait_end();
                if end.is_some() || Instant::now() >= until {
                    break end;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            assert_eq!(end, Some(EndReason::WindowClosed));
            let finished = session.stop().expect("what was recorded is made");
            eprintln!("{:?}: {} s", finished.media.file_name(), finished.duration);
            assert!(
                (finished.duration - 2.0).abs() <= 0.5,
                "{} s recorded",
                finished.duration
            );
            let seconds = video_seconds(&finished.media, &ffmpeg);
            assert!((seconds - 2.0).abs() <= 0.5, "{seconds} s of video");
        }
    }
}
