//! The system's audio on Linux: a `parec` child that records the monitor of the default sink (what is going to the
//! speakers) and writes it to its standard output, which a thread reads in blocks of 20 ms.
//!
//! Without `parec` (the package `pulseaudio-utils`, which PipeWire's PulseAudio server comes with as well) there is no
//! system audio: the start fails with `StartError::Source("parec")`, which whoever shows it to the user turns into the
//! advice to install it. `parec` that goes away while it records (the sound server stopped) ends the track.
//!
//! `parec` is started as the leader of a process group, which is killed when the source is stopped or dropped: nothing
//! of it is left running, whatever happens to the recording.

use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use process_wrap::std::{ProcessGroup, StdChildWrapper, StdCommandWrap};

use crate::audio::capture::began;
use crate::media::decode::Tail;
use crate::media::tools;
use crate::record::{EndReason, RunningSource, SourceEvent, StartError, Track};

/// The rate that `parec` is asked for.
const RATE: u32 = 48_000;
/// The bytes of one block that is read: 20 ms of 32-bit samples.
const BLOCK_BYTES: usize = RATE as usize / 50 * 4;

/// The arguments of `parec`: the monitor of the default sink, 32-bit float samples (little-endian), mono, at 48 kHz, in
/// the pieces of 20 ms that the monitor is asked to deliver.
pub fn parec_args() -> Vec<String> {
    [
        "-d",
        "@DEFAULT_MONITOR@",
        "--format=float32le",
        "--rate=48000",
        "--channels=1",
        "--latency-msec=20",
    ]
    .map(String::from)
    .into()
}

/// Starts `parec` as [`Track::System`]. Its audio is sent to `tx` in blocks of 20 ms, each stamped with when its last
/// sample arrived, less its length. `parec` ending by itself ends the track with `DeviceLost`, and the last of what
/// it said is in the reason.
pub fn start(tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    start_with(Path::new("parec"), tx)
}

/// `start` for the program `program`, which the tests make a script of their own.
fn start_with(
    program: &Path,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    let mut command = tools::command(program);
    command
        .args(parec_args().into_iter().map(OsString::from))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = StdCommandWrap::from(command);
    command.wrap(ProcessGroup::leader());
    let mut child = command.spawn().map_err(|e| match e.kind() {
        ErrorKind::NotFound => StartError::Source("parec".into()),
        _ => StartError::Source(format!("could not start parec: {e}")),
    })?;
    let (Some(stdout), Some(stderr)) = (child.stdout().take(), child.stderr().take()) else {
        let _ = child.start_kill();
        let _ = child.wait();
        return Err(StartError::Source("could not talk to parec".into()));
    };
    // What `parec` says is read all the time, whatever the audio does: a program that fills its pipe stops.
    let said = std::thread::spawn(move || Tail::read(stderr));
    let stopping = Arc::new(AtomicBool::new(false));
    let reader = std::thread::Builder::new().name("parec".into()).spawn({
        let stopping = stopping.clone();
        move || read_blocks(stdout, said, &stopping, &tx)
    });
    match reader {
        Ok(reader) => Ok(Box::new(Parec {
            child,
            stopping,
            reader: Some(reader),
        })),
        Err(e) => {
            let _ = child.start_kill();
            let _ = child.wait();
            Err(StartError::Source(format!(
                "could not read from parec: {e}"
            )))
        }
    }
}

/// A running `parec`. Dropping it ends it.
struct Parec {
    child: Box<dyn StdChildWrapper>,
    /// Set before `parec` is killed, so that the reader knows the end of its output is not an end of the track.
    stopping: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

impl RunningSource for Parec {
    /// Kills `parec` and what it started, and waits for the thread that reads it, which ends with the pipe. That comes
    /// at once, unless something of `parec` that is out of its process group holds the pipe open.
    fn stop(self: Box<Self>) {
        drop(self);
    }
}

impl Drop for Parec {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // It may have gone already, which is fine: it is waited for either way.
        let _ = self.child.start_kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Reads `stdout` in blocks of 20 ms until it ends, and then says that the track did, unless it is `stopping`.
fn read_blocks(
    mut stdout: ChildStdout,
    said: JoinHandle<Tail>,
    stopping: &AtomicBool,
    tx: &Sender<SourceEvent>,
) {
    let mut block = [0_u8; BLOCK_BYTES];
    loop {
        let filled = fill(&mut stdout, &mut block);
        // A last sample that is cut short is not one.
        let mono: Vec<f32> = block[..filled]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        if !mono.is_empty() {
            let at = began(Instant::now(), mono.len(), RATE);
            let audio = SourceEvent::Audio {
                track: Track::System,
                at,
                rate: RATE,
                mono,
            };
            if tx.send(audio).is_err() {
                // The session is over; whoever started `parec` will end it.
                return;
            }
        }
        if filled < BLOCK_BYTES {
            break;
        }
    }
    // The pipe ended because `parec` was killed, and not because it went away: the track is not over, it was stopped.
    if stopping.load(Ordering::SeqCst) {
        return;
    }
    // Both pipes end with the program, so what it said is there to be had.
    let said = said.join().map(|tail| tail.text()).unwrap_or_default();
    let reason = if said.is_empty() {
        "parec stopped".to_owned()
    } else {
        format!("parec stopped: {said}")
    };
    let _ = tx.send(SourceEvent::Ended {
        track: Track::System,
        reason: EndReason::DeviceLost(reason),
    });
}

/// Reads into `block` until it is full or the pipe is at its end; how many bytes it got.
fn fill(stdout: &mut ChildStdout, block: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < block.len() {
        match stdout.read(&mut block[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::test_support::script;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    /// How long a test waits for what a stand-in `parec` does before it fails.
    const WAIT: Duration = Duration::from_secs(10);

    /// What the track has sent until it ends: the buffers (their stamps, and how many samples they have), and the
    /// reason it ended.
    fn until_the_end(rx: &Receiver<SourceEvent>) -> (Vec<(Instant, usize)>, EndReason) {
        let mut buffers = Vec::new();
        loop {
            match rx.recv_timeout(WAIT).expect("the track did not end") {
                SourceEvent::Audio {
                    track: Track::System,
                    at,
                    rate: 48_000,
                    mono,
                } => buffers.push((at, mono.len())),
                SourceEvent::Ended {
                    track: Track::System,
                    reason,
                } => return (buffers, reason),
                _ => panic!("not the system's audio at 48 kHz"),
            }
        }
    }

    /// A stand-in for `parec` that runs `body` (a shell script).
    fn parec(dir: &Path, body: &str) -> std::path::PathBuf {
        script(dir, "parec", body, "exit 1")
    }

    #[test]
    fn parec_args_match_the_plan() {
        assert_eq!(
            parec_args(),
            [
                "-d",
                "@DEFAULT_MONITOR@",
                "--format=float32le",
                "--rate=48000",
                "--channels=1",
                "--latency-msec=20"
            ]
        );
    }

    #[test]
    fn a_parec_that_exits_ends_the_system_track() {
        let dir = tempfile::tempdir().unwrap();
        // 0.2 s of f32 zeros at 48 kHz: 9 600 samples, 38 400 bytes.
        let program = parec(dir.path(), "head -c 38400 /dev/zero");
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();

        let (buffers, reason) = until_the_end(&rx);
        source.stop();

        assert!(!buffers.is_empty());
        assert_eq!(buffers.iter().map(|(_, n)| n).sum::<usize>(), 9_600);
        assert!(
            buffers.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "the stamps of the buffers increase"
        );
        assert!(matches!(reason, EndReason::DeviceLost(_)), "{reason:?}");
    }

    #[test]
    fn a_parec_that_says_why_it_failed_ends_the_track_with_its_words() {
        let dir = tempfile::tempdir().unwrap();
        let program = parec(
            dir.path(),
            "echo 'Connection failure: Connection refused' >&2; exit 1",
        );
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();

        let (buffers, reason) = until_the_end(&rx);
        source.stop();

        assert!(buffers.is_empty());
        let EndReason::DeviceLost(why) = reason else {
            panic!("{reason:?}")
        };
        assert!(why.contains("Connection refused"), "{why}");
    }

    #[test]
    fn a_parec_that_is_not_installed_is_reported_by_its_name() {
        let (tx, _rx) = mpsc::channel();

        let started = start_with(Path::new("dictate-test-no-such-program"), tx);

        assert_eq!(started.err(), Some(StartError::Source("parec".into())));
    }

    #[test]
    fn bytes_that_do_not_make_a_whole_sample_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        // 1 000 samples and 3 bytes of the one after: the 20 ms block of 960, and 40 samples to its end.
        let program = parec(dir.path(), "head -c 4003 /dev/zero");
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();

        let (buffers, _) = until_the_end(&rx);
        source.stop();

        assert_eq!(
            buffers.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            [960, 40]
        );
    }

    /// A stand-in for `parec` that delivers 20 ms of silence every 20 ms until it is ended, and writes its process id to
    /// the file that it returns.
    fn endless_parec(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let pid = dir.join("pid");
        let program = parec(
            dir,
            &format!(
                "echo $$ > '{}'\nwhile true; do head -c 3840 /dev/zero; sleep 0.02; done",
                pid.display()
            ),
        );
        (program, pid)
    }

    /// Whether the process whose id `pid_file` has is still there.
    fn is_running(pid_file: &Path) -> bool {
        let pid = std::fs::read_to_string(pid_file).unwrap();
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Waits for the first of the audio of the track.
    fn first_audio(rx: &Receiver<SourceEvent>) {
        match rx.recv_timeout(WAIT).expect("no audio came") {
            SourceEvent::Audio { .. } => {}
            _ => panic!("the track did not begin with audio"),
        }
    }

    #[test]
    fn stopping_ends_parec_and_is_not_an_end_of_the_track() {
        let dir = tempfile::tempdir().unwrap();
        let (program, pid) = endless_parec(dir.path());
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();
        first_audio(&rx);

        let began = Instant::now();
        source.stop();

        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(!is_running(&pid), "parec is still running");
        assert!(
            !rx.try_iter()
                .any(|event| matches!(event, SourceEvent::Ended { .. })),
            "a track that was stopped has not ended by itself"
        );
    }

    #[test]
    fn a_source_that_is_dropped_ends_parec() {
        let dir = tempfile::tempdir().unwrap();
        let (program, pid) = endless_parec(dir.path());
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();
        first_audio(&rx);

        drop(source);

        assert!(!is_running(&pid), "parec is still running");
    }

    #[test]
    fn a_session_that_is_gone_does_not_keep_parec_running() {
        let dir = tempfile::tempdir().unwrap();
        let (program, pid) = endless_parec(dir.path());
        let (tx, rx) = mpsc::channel();
        let source = start_with(&program, tx).unwrap();
        first_audio(&rx);
        drop(rx);
        // The reader finds nobody to send to, and ends; stopping is still as it is.
        std::thread::sleep(Duration::from_millis(100));

        source.stop();

        assert!(!is_running(&pid), "parec is still running");
    }

    /// Plays a tone through the default sink with `paplay` (it comes with `parec`) and listens for it on the monitor. Needs
    /// a PulseAudio or PipeWire server with a sink that plays: run it with `--ignored`, on a machine that is not muted.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a sound server that plays: set DICTATE_TEST_REAL_AUDIO=1"]
    fn linux_parec_reads_the_monitor() {
        use super::super::real_audio::{self, Playing};

        real_audio::require_enabled();
        let dir = tempfile::tempdir().unwrap();
        let wav = real_audio::tone_wav(dir.path(), 10);
        let _player = Playing::start(Command::new("paplay").arg(&wav));
        // The player takes a moment to start; the tone lasts for much longer than this.
        std::thread::sleep(Duration::from_secs(1));

        let (tx, rx) = mpsc::channel();
        let source = start(tx).expect("parec is started");
        let heard = real_audio::hears_sound(&rx, Track::System, Duration::from_secs(3));
        source.stop();

        assert!(heard, "no sound in 3 s of a tone that was playing");
    }
}
