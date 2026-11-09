//! ffmpeg as the encoder of audio.m4a: `ffmpeg -f f32le -i pipe:0 -c:a aac`, fed the blocks through its standard input.
//!
//! The pipe is the streaming: a block is written to it as it comes, and ffmpeg (which runs as fast as it is fed) encodes
//! it and writes the file; when ffmpeg is slower than the pipeline, the pipe is full and `write` waits, so nothing
//! piles up here.
//!
//! The program is started so that the whole of what it starts can be killed with it: a Scoop or Chocolatey ffmpeg on
//! Windows is a shim that starts the real one, and a program that reads from a pipe does not end by itself when the
//! sink is dropped, so killing the shim alone would leave the real ffmpeg waiting, with the `.part` file open.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};
use std::thread::JoinHandle;

#[cfg(unix)]
use process_wrap::std::ProcessGroup;
#[cfg(windows)]
use process_wrap::std::{CreationFlags, JobObject};
use process_wrap::std::{StdChildWrapper, StdCommandWrap};
#[cfg(windows)]
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use super::super::decode::Tail;
use super::super::tools;
use super::{part_path, publish, remove_part, EncodeError, M4aSink, M4A_BITRATE, M4A_RATE};

/// How many samples go to ffmpeg in one write: a block is cut into pieces of this size.
const PIECE_SAMPLES: usize = 16_384;

/// The arguments of ffmpeg, ending with the name of the file it writes.
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
    args.extend(["-ac", "1", "-i", "pipe:0", "-c:a", "aac", "-b:a"].map(OsString::from));
    args.push(format!("{}k", M4A_BITRATE / 1000).into());
    args.extend(["-movflags", "+faststart", "-f", "ipod"].map(OsString::from));
    args.push(output);
    args
}

/// Starts ffmpeg writing `out`.
pub(super) fn create(out: &Path, ffmpeg: &Path) -> Result<Box<dyn M4aSink>, EncodeError> {
    let part = part_path(out);
    let mut command = tools::command(ffmpeg);
    command
        .args(arguments(&part))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // The leader of a process group (a job object on Windows): killing it reaches everything ffmpeg started.
    let mut command = StdCommandWrap::from(command);
    // CreationFlags must come before JobObject, which adds its own flags to them.
    #[cfg(windows)]
    command
        .wrap(CreationFlags(CREATE_NO_WINDOW))
        .wrap(JobObject);
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
    Ok(Box::new(FfmpegSink {
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
    }))
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
    fn ffmpeg_writes_into_a_folder_named_in_vietnamese() {
        let Some(start) = real_writer() else { return };

        writes_into_a_vietnamese_folder(&start);
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
        let part = format!("file:{}.part", out.display());
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
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
                "-i",
                "pipe:0",
                "-c:a",
                "aac",
                "-b:a",
                "96k",
                "-movflags",
                "+faststart",
                "-f",
                "ipod",
                part.as_str()
            ]
        );
        assert!(out.exists(), "the .part file is not renamed");
        assert!(!part_path(&out).exists());
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
        assert!(!part_path(&out).exists());
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

    #[test]
    fn dropping_the_sink_kills_what_ffmpeg_started_too() {
        // A Scoop or Chocolatey ffmpeg is a shim that starts the real one, and a program that reads its input from a
        // pipe does not end when the sink goes away. So the fake starts a program of its own that would write a file a
        // few seconds later; once the sink is dropped, nothing is left to write it.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let late = dir.path().join("late.txt");
        let started = dir.path().join("started.txt");
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            &format!(
                "(sleep 2; echo late > '{}') & echo started > '{}'; sleep 60",
                late.display(),
                started.display()
            ),
            &format!(
                "start \"\" /b cmd /c \"{}\" & echo started> \"{}\" & ping -n 60 127.0.0.1 >nul",
                script(
                    dir.path(),
                    "grandchild",
                    "",
                    &format!(
                        "ping -n 4 127.0.0.1 >nul & echo late> \"{}\"",
                        late.display()
                    ),
                )
                .display(),
                started.display()
            ),
        );
        let sink = create(&out, &fake).unwrap();
        wait_for("the fake ffmpeg to start its own program", || {
            started.exists()
        });

        drop(sink);

        // Long enough for the program of the fake to have written the file, had it been left running.
        std::thread::sleep(Duration::from_secs(6));
        assert!(
            !late.exists(),
            "a program that ffmpeg started is still running"
        );
    }
}
