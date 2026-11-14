//! ffmpeg as the encoder of audio.m4a: `ffmpeg -f f32le -i pipe:0 -c:a aac`, fed the blocks through its standard input.
//!
//! The pipe is the streaming: a block is written to it as it comes, and ffmpeg (which runs as fast as it is fed) encodes
//! it and writes the file; when ffmpeg is slower than the pipeline, the pipe is full and `write` waits, so nothing
//! piles up here.
//!
//! The program is started so that the whole of what it starts can be killed with it: a Scoop or Chocolatey ffmpeg on
//! Windows is a shim that starts the real one, and a program that reads from a pipe does not end by itself when the
//! sink is dropped, so killing the shim alone would leave the real ffmpeg waiting, with the `.part` file open.
//!
//! `spawn` is that way of starting ffmpeg, which the encoder of the screen recording's video (`record::video`) uses too.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, Stdio};
use std::thread::JoinHandle;

#[cfg(unix)]
use process_wrap::std::ProcessGroup;
#[cfg(windows)]
use process_wrap::std::{CreationFlags, JobObject};
use process_wrap::std::{StdChildWrapper, StdCommandWrap};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

mod job;

use super::super::decode::Tail;
use super::super::tools;
use super::{part_path, publish, remove_part, EncodeError, M4aSink, M4A_BITRATE, M4A_RATE};

/// How many samples go to ffmpeg in one write: a block is cut into pieces of this size.
const PIECE_SAMPLES: usize = 16_384;

/// The arguments of ffmpeg, ending with the name of the file it writes.
///
/// There is nothing to find out about the input (the format, rate and channels are all given), so ffmpeg is told to
/// look at as little of it as it may: `-probesize 32` is the least, `-analyzeduration 1` (a microsecond: 0 means the
/// default, 5 s) is the other limit it would otherwise go by. Left to itself, ffmpeg 6 reads 5 s of audio or 5 MB of
/// a pipe before it does anything, and nothing is written to the file until then. ffmpeg 9 does not wait.
///
/// The file is a `.part`, which has no format of its own to ffmpeg, so the format is given (`ipod`: MP4 with the brand
/// `M4A `, what the name `.m4a` would have picked). `file:` in front of the name keeps ffmpeg from taking a part of
/// it for a protocol (`ghi âm: 1.m4a`), or a name that starts with `-` for an option.
fn arguments(part: &Path) -> Vec<OsString> {
    let mut output = OsString::from("file:");
    output.push(part);
    let mut args: Vec<OsString> = ["-nostdin", "-y", "-v", "error", "-f", "f32le", "-ar"]
        .map(OsString::from)
        .into();
    args.push(M4A_RATE.to_string().into());
    args.extend(
        [
            "-ac",
            "1",
            "-analyzeduration",
            "1",
            "-probesize",
            "32",
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
        ]
        .map(OsString::from),
    );
    args.push(format!("{}k", M4A_BITRATE / 1000).into());
    args.extend(["-movflags", "+faststart", "-f", "ipod"].map(OsString::from));
    args.push(output);
    args
}

/// Starts ffmpeg writing `out`.
pub(super) fn create(out: &Path, ffmpeg: &Path) -> Result<Box<dyn M4aSink>, EncodeError> {
    Ok(Box::new(start(out, ffmpeg)?))
}

fn start(out: &Path, ffmpeg: &Path) -> Result<FfmpegSink, EncodeError> {
    let part = part_path(out);
    let Process {
        child,
        stdin,
        stderr,
        #[cfg(windows)]
        job,
    } = spawn(ffmpeg, arguments(&part))?;
    Ok(FfmpegSink {
        child,
        stdin: Some(stdin),
        // What ffmpeg prints is read all the time, whatever the audio does: a program that fills the pipe stops.
        stderr: Some(std::thread::spawn(move || Tail::read(stderr))),
        bytes: Vec::new(),
        failure: None,
        reaped: false,
        out: out.to_owned(),
        part,
        published: false,
        #[cfg(windows)]
        job,
    })
}

/// ffmpeg, running, and the ends of the pipes it is talked to through.
pub(crate) struct Process {
    pub(crate) child: Box<dyn StdChildWrapper>,
    /// What ffmpeg reads (`pipe:0`); closing it is the end of the input.
    pub(crate) stdin: ChildStdin,
    /// What ffmpeg says. It has to be read all the time, or a program that fills the pipe stops.
    pub(crate) stderr: ChildStderr,
    /// The job that ffmpeg and what it started are in, to be asked how many of them are left (`None` when it could not be made).
    #[cfg(windows)]
    #[cfg_attr(not(test), allow(dead_code))]
    job: Option<Arc<job::Job>>,
}

/// Starts ffmpeg with `args`, with its input and its messages as pipes and no window, so that the whole of what it
/// starts can be killed with it: dropping the child, or `start_kill`, reaches all of it.
pub(crate) fn spawn(ffmpeg: &Path, args: Vec<OsString>) -> Result<Process, EncodeError> {
    let mut command = tools::command(ffmpeg);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // The leader of a process group (a job object on Windows): killing it reaches everything ffmpeg started.
    let mut command = StdCommandWrap::from(command);
    // CreationFlags must come before JobObject, which adds its own flags to them. `Counted` goes between them: the program is
    // suspended when it has its turn, and `wait` then has a job of our own to ask whether anything of ffmpeg is left.
    #[cfg(windows)]
    let job = job::Job::new().ok();
    #[cfg(windows)]
    {
        command.wrap(CreationFlags(CREATE_NO_WINDOW));
        if let Some(job) = &job {
            command.wrap(job::Counted(job.clone()));
        }
        command.wrap(JobObject);
    }
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    let mut child = command
        .spawn()
        .map_err(|e| EncodeError::Failed(format!("could not start ffmpeg: {e}")))?;
    let (Some(stdin), Some(stderr)) = (child.stdin().take(), child.stderr().take()) else {
        // Both were asked to be pipes. Whatever happened, the program must not be left running.
        let _ = child.start_kill();
        let _ = child.wait();
        return Err(EncodeError::Failed("could not talk to ffmpeg".into()));
    };
    Ok(Process {
        child,
        stdin,
        stderr,
        #[cfg(windows)]
        job,
    })
}

/// A running ffmpeg and the end of its standard input that the blocks are written to.
struct FfmpegSink {
    child: Box<dyn StdChildWrapper>,
    /// `None` once the end of the audio has been told to ffmpeg.
    stdin: Option<ChildStdin>,
    /// The thread that reads stderr; it ends when ffmpeg does.
    stderr: Option<JoinHandle<Tail>>,
    /// The bytes of the piece being written: never more than `PIECE_SAMPLES` samples.
    bytes: Vec<u8>,
    /// Why ffmpeg stopped, when it did before the end.
    failure: Option<String>,
    /// ffmpeg has been waited for.
    reaped: bool,
    out: PathBuf,
    part: PathBuf,
    /// The `.part` file has become `out`.
    published: bool,
    /// The job that ffmpeg and what it started are in, to be asked how many of them are left (`None` when it could not be made).
    #[cfg(windows)]
    #[cfg_attr(not(test), allow(dead_code))]
    job: Option<Arc<job::Job>>,
}

impl FfmpegSink {
    /// Ends ffmpeg and everything it started, if it is still running, and waits for them to be gone.
    fn end(&mut self) -> io::Result<std::process::ExitStatus> {
        drop(self.stdin.take());
        if !self.reaped {
            // It may be gone already, which is fine: it is waited for either way.
            let _ = self.child.start_kill();
        }
        let status = self.child.wait();
        self.reaped = true;
        status
    }

    /// What ffmpeg said, once it has ended (it is made to end here, if it has not).
    fn say_why(&mut self, how: &str) -> String {
        let status = self.end();
        let tail = self
            .stderr
            .take()
            .and_then(|thread| thread.join().ok())
            .map(|tail| tail.text())
            .unwrap_or_default();
        let code = match status {
            Ok(status) => status
                .code()
                .map_or("no exit code".to_owned(), |c| format!("exit code {c}")),
            Err(e) => format!("could not be waited for: {e}"),
        };
        format!("ffmpeg {how} ({code}): {tail}")
    }
}

impl M4aSink for FfmpegSink {
    fn write(&mut self, mono_48k: &[f32]) -> io::Result<()> {
        if let Some(failure) = &self.failure {
            return Err(io::Error::other(failure.clone()));
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(io::Error::other("the audio has already ended"));
        };
        for piece in mono_48k.chunks(PIECE_SAMPLES) {
            self.bytes.clear();
            self.bytes
                .extend(piece.iter().flat_map(|s| s.to_le_bytes()));
            if let Err(e) = stdin.write_all(&self.bytes) {
                // ffmpeg is gone (a closed pipe), or no longer taking the audio.
                let failure = self.say_why(&format!("stopped taking the audio ({e})"));
                self.failure = Some(failure.clone());
                return Err(io::Error::other(failure));
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if let Some(failure) = &self.failure {
            return Err(io::Error::other(failure.clone()));
        }
        // Closing the pipe is the end of the audio: ffmpeg writes what it holds and the index, and ends.
        drop(self.stdin.take());
        let status = self.child.wait();
        self.reaped = true;
        let status = status?;
        if !status.success() {
            let why = self.say_why("did not finish the file");
            return Err(io::Error::other(why));
        }
        publish(&self.part, &self.out)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for FfmpegSink {
    /// A sink that is dropped without being finished kills ffmpeg, and leaves no file.
    fn drop(&mut self) {
        let _ = self.end();
        if !self.published {
            remove_part(&self.part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::checks::*;
    use super::*;
    use crate::media::test_support::{real_ffmpeg, script};
    #[cfg(unix)]
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    /// Starts a sink with the ffmpeg of this machine (the test is skipped without one), whatever the system's own
    /// encoder is.
    fn real_writer() -> Option<impl Fn(&Path) -> Box<dyn M4aSink>> {
        let ffmpeg: PathBuf = real_ffmpeg()?;
        Some(move |out: &Path| create(out, &ffmpeg).unwrap())
    }

    #[test]
    fn ffmpeg_writes_playable_m4a() {
        let Some(start) = real_writer() else { return };

        writes_a_playable_m4a(&start);
    }

    #[test]
    fn ffmpeg_streams_without_buffering() {
        let Some(start) = real_writer() else { return };

        streams_without_buffering(&start);
    }

    #[test]
    fn ffmpeg_unfinished_sink_leaves_no_file() {
        let Some(start) = real_writer() else { return };

        leaves_no_file_when_dropped(&start);
    }

    #[test]
    fn an_old_ffmpeg_that_ends_late_leaves_the_new_one_alone() {
        let Some(start) = real_writer() else { return };

        an_old_sink_that_ends_late_leaves_the_new_one_alone(&start);
    }

    #[test]
    fn ffmpeg_writes_into_a_folder_named_in_vietnamese() {
        let Some(start) = real_writer() else { return };

        writes_into_a_vietnamese_folder(&start);
    }

    #[test]
    fn ffmpeg_starts_on_the_audio_it_has_without_waiting_to_probe_its_input() {
        // ffmpeg 6 makes its output file only after it has read enough of its input to know what it is: 5 s of audio, or
        // 5 MB, unless told that there is nothing to find out. With the pipe held open (the pipeline streams a recording,
        // which does not end for a long while) one second of audio is not that much, and no file means no sign that the
        // encoder works until a long way into the recording.
        let Some(start) = real_writer() else { return };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let mut sink = start(&out);

        sink.write(&tone(M4A_RATE as usize)).unwrap();

        wait_for("ffmpeg to start writing the file", || {
            !part_files(&out).is_empty()
        });
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_gets_the_samples_as_f32le_and_the_documented_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        // Keeps the arguments and what it is sent, and writes the file it is asked for (the last argument).
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            r#"printf '%s\n' "$@" > "$0.args"; cat > "$0.in"; for a; do last=$a; done; : > "${last#file:}""#,
            "",
        );
        let mut sink = create(&out, &fake).unwrap();

        sink.write(&[0.5, -0.25]).unwrap();
        sink.write(&[]).unwrap();
        sink.write(&[1.0]).unwrap();
        sink.finish().unwrap();

        let sent = fs::read(format!("{}.in", fake.display())).unwrap();
        let expected: Vec<u8> = [0.5f32, -0.25, 1.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(sent, expected);
        let args = fs::read_to_string(format!("{}.args", fake.display())).unwrap();
        let args: Vec<&str> = args.lines().collect();
        // The last argument is the sink's own `.part` file, which has a nonce in its name.
        let (&part, args) = args.split_last().unwrap();
        let part = part.strip_prefix("file:").unwrap();
        let part = Path::new(part);
        assert_eq!(part.parent(), out.parent());
        let name = part.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("audio.m4a.") && name.ends_with(".part"),
            "{name}"
        );
        assert_eq!(
            args,
            [
                "-nostdin",
                "-y",
                "-v",
                "error",
                "-f",
                "f32le",
                "-ar",
                "48000",
                "-ac",
                "1",
                "-analyzeduration",
                "1",
                "-probesize",
                "32",
                "-i",
                "pipe:0",
                "-c:a",
                "aac",
                "-b:a",
                "96k",
                "-movflags",
                "+faststart",
                "-f",
                "ipod"
            ]
        );
        assert!(out.exists(), "the .part file is not renamed");
        assert!(part_files(&out).is_empty());
    }

    #[test]
    fn ffmpeg_that_fails_makes_finish_fail_with_what_it_said_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            "echo 'Unknown encoder aac' >&2; exit 3",
            "echo Unknown encoder aac 1>&2 & exit /b 3",
        );
        let mut sink = create(&out, &fake).unwrap();

        // ffmpeg has ended or is about to: the write may fail, and when it does the sink says why from then on.
        let _ = sink.write(&tone(48_000));
        let error = sink.finish().unwrap_err().to_string();

        assert!(error.contains("Unknown encoder aac"), "{error}");
        assert!(error.contains('3'), "the exit code is missing: {error}");
        assert!(!out.exists());
        assert!(part_files(&out).is_empty());
    }

    #[test]
    fn ffmpeg_that_cannot_start_is_a_failure_that_says_so() {
        let dir = tempfile::tempdir().unwrap();

        let error = create(&dir.path().join("audio.m4a"), Path::new("/no/such/ffmpeg")).err();

        let Some(EncodeError::Failed(message)) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("could not start ffmpeg"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_sink_kills_what_ffmpeg_started_too() {
        // A Scoop or Chocolatey ffmpeg is a shim that starts the real one, and a program that reads its input from a
        // pipe does not end when the sink goes away. So the fake starts a program of its own that would write a file a
        // few seconds later; once the sink is dropped, nothing is left to write it.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let late = dir.path().join("late.txt");
        let started = dir.path().join("started.txt");
        let pid_file = dir.path().join("pid.txt");
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            &format!(
                "echo $$ > '{}'; (sleep 2; echo late > '{}') & echo started > '{}'; sleep 60",
                pid_file.display(),
                late.display(),
                started.display()
            ),
            "",
        );
        let sink = start(&out, &fake).unwrap();
        wait_for("the fake ffmpeg to start its own program", || {
            started.exists()
        });

        drop(sink);

        // The program itself is gone, and reaped (a zombie still answers `kill -0`).
        let pid = fs::read_to_string(&pid_file).unwrap();
        let alive = std::process::Command::new("sh")
            .args(["-c", &format!("kill -0 {}", pid.trim())])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "ffmpeg is still there (running, or a zombie)");
        // Long enough for the program of the fake to have written the file, had it been left running.
        std::thread::sleep(Duration::from_secs(6));
        assert!(
            !late.exists(),
            "a program that ffmpeg started is still running"
        );
    }

    /// `ping` by its path, through `%SystemRoot%`, which is not a PATH. The PATH that the programs are started with is the one of the
    /// machine with the folders of the system and of the tools added, and on the Windows runners of CI it is over 15,000 characters:
    /// `cmd.exe` ignores a PATH of more than 8191 altogether, so a batch file that says `ping` gets "'ping' is not recognized" and goes
    /// on to its next line at once.
    #[cfg(windows)]
    const PING: &str = r#""%SystemRoot%\System32\PING.EXE""#;

    #[cfg(windows)]
    #[test]
    fn dropping_the_sink_kills_what_ffmpeg_started_too() {
        // A Scoop or Chocolatey ffmpeg is a shim that starts the real one and waits for it, and a program that reads its
        // input from a pipe does not end when the sink goes away. So the fake runs a program of its own, a `ping` of
        // four pings (three seconds) whose output goes to a file: a program that writes to a file has the output held
        // until it ends (or is full), so that only a `ping` that was left running to its end has filled this one. Once
        // the sink is dropped, no such `ping` is left, and the job that counts what ffmpeg started is empty.
        //
        // The test used to have a program of `ping -n 4 127.0.0.1 >nul & echo late> late.txt` (`ping` found through the PATH, see
        // `PING`), and ask whether `late.txt` was there some seconds after the drop. The `ping` was never found on CI, so the
        // program wrote the file by itself a few hundred milliseconds after it started, and the test passed only when the drop
        // got there first, which it did not every time. The kill was not at fault: with a `ping` that runs, the drop left nothing
        // behind in 80 of 80 runs (the program started in two ways), with the job empty each time.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let pinged = dir.path().join("ping.txt");
        let own = script(
            dir.path(),
            "own",
            "",
            &format!("{PING} -n 4 127.0.0.1 > \"{}\"", pinged.display()),
        );
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            "",
            &format!("cmd /c \"{}\"", own.display()),
        );
        let sink = start(&out, &fake).unwrap();
        let job = sink
            .job
            .clone()
            .expect("the job that counts what ffmpeg started could not be made");
        // The fake's `cmd`, the `cmd` of its program and the `ping` are in the job: the program is running, and it is one of
        // what the sink is to end.
        wait_for("the fake ffmpeg to run its own program", || {
            job.active_processes().is_ok_and(|active| active >= 3)
        });
        let before = job.describe();

        drop(sink);

        // Dropping the sink waits for the job to be empty.
        assert_eq!(
            job.active_processes().ok(),
            Some(0),
            "what ffmpeg started is still running: in the job before the drop, {before}; now, {}",
            job.describe()
        );
        // Long enough for the `ping` to have ended by itself, had it been left running.
        std::thread::sleep(Duration::from_secs(4));
        let lines = std::fs::read_to_string(&pinged)
            .unwrap_or_default()
            .lines()
            .count();
        assert!(
            lines < 8,
            "a program that ffmpeg started went on to its end ({lines} lines of ping.txt); in the job before the drop, {before}"
        );
    }
}
