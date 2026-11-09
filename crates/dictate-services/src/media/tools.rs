//! Finding ffmpeg and yt-dlp, and running them.
//!
//! `run` starts a program for the pipeline: it puts every line the program prints (stdout and stderr together,
//! and a progress line ended by `\r` too) into a callback, and on cancel kills the program and everything it
//! started. The setup every child process gets (the search path, no console window) is in `base_command`.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
#[cfg(windows)]
use process_wrap::tokio::{CreationFlags, JobObject};
use process_wrap::tokio::{KillOnDrop, TokioChildWrapper, TokioCommandWrap};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;
#[cfg(windows)]
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

/// How many of the last lines of a program `Exit` keeps.
const LAST_LINES: usize = 2;

/// The folders to look for programs in: the app's own PATH first, then the places each system puts what the user
/// installs. The PATH a desktop app gets is short (on a Mac it is `/usr/bin:/bin:/usr/sbin:/sbin`, with no
/// Homebrew), and a Linux launcher often leaves out `~/.local/bin`.
///
/// Child processes get this as their PATH too: yt-dlp looks for ffmpeg there.
pub fn search_path() -> OsString {
    search_path_from(
        std::env::var_os("PATH"),
        dirs::home_dir(),
        dirs::data_local_dir(),
    )
}

/// `search_path` for a given PATH, home folder and local app data folder (the last is only used on Windows).
#[cfg_attr(not(windows), allow(unused_variables))]
fn search_path_from(
    path_var: Option<OsString>,
    home: Option<PathBuf>,
    local_data: Option<PathBuf>,
) -> OsString {
    let mut all: Vec<PathBuf> = path_var
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();

    let mut extra: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    extra.extend(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map(PathBuf::from));
    #[cfg(not(any(target_os = "macos", windows)))]
    extra.extend(["/usr/local/bin", "/snap/bin"].map(PathBuf::from));
    #[cfg(windows)]
    {
        // Where winget, Scoop and Chocolatey put the programs they install.
        extra.extend(local_data.map(|d| d.join("Microsoft").join("WinGet").join("Links")));
        extra.extend(home.iter().map(|h| h.join("scoop").join("shims")));
        extra.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
    }
    // pipx and `pip install --user` put yt-dlp in `~/.local/bin`; some people keep programs in `~/bin`.
    if let Some(home) = &home {
        extra.push(home.join(".local").join("bin"));
        extra.push(home.join("bin"));
    }
    for dir in extra {
        if !all.contains(&dir) {
            all.push(dir);
        }
    }

    // Joining fails only for a folder with the separator in its name: then the plain PATH is what is left.
    std::env::join_paths(all).unwrap_or_else(|_| path_var.unwrap_or_default())
}

/// An external program the app can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    YtDlp,
}

impl Tool {
    /// The name of the program file, without the `.exe` of Windows.
    fn program(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::YtDlp => "yt-dlp",
        }
    }
}

/// Where each tool is, when it is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub yt_dlp: Option<PathBuf>,
}

impl Tools {
    /// Looks for the tools in `search_path()`.
    pub fn find() -> Tools {
        Tools::find_in(&search_path())
    }

    fn find_in(search_path: &OsStr) -> Tools {
        let cwd = std::env::current_dir().unwrap_or_default();
        let locate = |tool: Tool| which::which_in(tool.program(), Some(search_path), &cwd).ok();
        Tools {
            ffmpeg: locate(Tool::Ffmpeg),
            yt_dlp: locate(Tool::YtDlp),
        }
    }
}

/// The command that installs a tool with the usual package manager of this system, to show to the user.
pub fn install_command(tool: Tool) -> &'static str {
    #[cfg(target_os = "macos")]
    return match tool {
        Tool::YtDlp => "brew install yt-dlp ffmpeg",
        Tool::Ffmpeg => "brew install ffmpeg",
    };
    #[cfg(windows)]
    return match tool {
        Tool::YtDlp => "winget install yt-dlp.yt-dlp Gyan.FFmpeg",
        Tool::Ffmpeg => "winget install Gyan.FFmpeg",
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    return match tool {
        Tool::YtDlp => "pipx install yt-dlp && sudo apt install ffmpeg",
        Tool::Ffmpeg => "sudo apt install ffmpeg",
    };
}

/// How a program ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// The exit code; `None` when the system ended it (a signal).
    pub code: Option<i32>,
    /// The last 2 non-empty lines it printed, oldest first.
    pub last_lines: Vec<String>,
}

/// Why a program did not run to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program could not be started (or its end not be waited for); the text is the system's detail.
    Spawn(String),
    /// The caller cancelled it and the program, with everything it had started, was killed.
    Cancelled,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Spawn(m) => write!(f, "Could not start the program: {m}"),
            RunError::Cancelled => f.write_str("The program was cancelled."),
        }
    }
}

impl std::error::Error for RunError {}

/// The command every child process starts from: the search path of `search_path()` (so that yt-dlp finds ffmpeg),
/// Python's output unbuffered (so that the progress lines arrive as they are printed), and on Windows no console
/// window flashing up.
fn base_command(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .env("PATH", search_path())
        .env("PYTHONUNBUFFERED", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW.0);
    }
    command
}

/// Cuts a stream of bytes into lines, ending a line at `\n` or `\r` (ffmpeg and yt-dlp redraw their progress
/// line with `\r`) and dropping the empty ones. The text is UTF-8; what is not is replaced.
///
/// The ends of a line are single bytes that never appear inside a multi-byte character, so the bytes of a line
/// are kept until it ends, and a character that arrives in two chunks is not cut.
#[derive(Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    fn push(&mut self, bytes: &[u8], mut line: impl FnMut(&str)) {
        for &byte in bytes {
            if byte == b'\n' || byte == b'\r' {
                self.end_line(&mut line);
            } else {
                self.pending.push(byte);
            }
        }
    }

    /// The stream is over: whatever is left, without an end, is the last line.
    fn finish(&mut self, mut line: impl FnMut(&str)) {
        self.end_line(&mut line);
    }

    fn end_line(&mut self, line: &mut impl FnMut(&str)) {
        if !self.pending.is_empty() {
            line(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }
}

/// One of the two output streams of a program, read in chunks and cut into lines.
struct Pipe<R> {
    reader: Option<R>,
    chunk: Box<[u8; 8192]>,
    lines: LineSplitter,
}

impl<R: AsyncRead + Unpin> Pipe<R> {
    fn new(reader: Option<R>) -> Pipe<R> {
        Pipe {
            reader,
            chunk: Box::new([0; 8192]),
            lines: LineSplitter::default(),
        }
    }

    fn is_open(&self) -> bool {
        self.reader.is_some()
    }

    /// Waits for the next chunk and returns its size; 0 when the stream is over (or broke).
    async fn read(&mut self) -> usize {
        match &mut self.reader {
            Some(reader) => reader.read(&mut *self.chunk).await.unwrap_or(0),
            None => 0,
        }
    }

    /// Takes the chunk `read` has just read (`size` 0 ends the stream) and hands its lines on.
    fn take(&mut self, size: usize, line: impl FnMut(&str)) {
        if size == 0 {
            self.reader = None;
            self.lines.finish(line);
        } else {
            self.lines.push(&self.chunk[..size], line);
        }
    }
}

/// Gives each line to the caller and remembers the last few.
struct Lines<F> {
    on_line: F,
    last: VecDeque<String>,
}

impl<F: FnMut(&str)> Lines<F> {
    fn add(&mut self, line: &str) {
        (self.on_line)(line);
        if self.last.len() == LAST_LINES {
            self.last.pop_front();
        }
        self.last.push_back(line.to_owned());
    }
}

/// A running program with everything it started. Dropping it while the program still runs kills them all: the
/// `KillOnDrop` of the wrapper reaches the whole tree on Windows (the job object) but only the program itself
/// on Unix, so the process group is killed here.
struct Running {
    child: Box<dyn TokioChildWrapper>,
    ended: bool,
}

impl Running {
    /// Kills the program and everything it started, and waits for them to be gone.
    async fn kill(&mut self) {
        let _ = Box::into_pin(self.child.kill()).await;
        self.ended = true;
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.child.start_kill();
        }
    }
}

/// Runs a program to its end and gives every line it prints to `on_line`.
///
/// Stdout and stderr are read together and cut into lines at `\n` or `\r`, the empty ones dropped. The program
/// gets `search_path()` as its PATH and `PYTHONUNBUFFERED=1`, has no input, and on Windows opens no console
/// window. It runs as the leader of a process group (a job object on Windows): when `cancel` fires, the program
/// and everything it started are killed and the result is `RunError::Cancelled`.
pub async fn run(
    program: &Path,
    args: &[OsString],
    cancel: &CancellationToken,
    on_line: impl FnMut(&str) + Send,
) -> Result<Exit, RunError> {
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }

    let mut command = base_command(program);
    command.args(args);
    let mut command = tokio::process::Command::from(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = TokioCommandWrap::from(command);
    command.wrap(KillOnDrop);
    // CreationFlags must come before JobObject, which adds its own flags to them.
    #[cfg(windows)]
    command
        .wrap(CreationFlags(CREATE_NO_WINDOW))
        .wrap(JobObject);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    let child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;

    let mut running = Running {
        child,
        ended: false,
    };
    let mut out = Pipe::new(running.child.stdout().take());
    let mut err = Pipe::new(running.child.stderr().take());
    let mut lines = Lines {
        on_line,
        last: VecDeque::with_capacity(LAST_LINES),
    };

    while out.is_open() || err.is_open() {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                running.kill().await;
                return Err(RunError::Cancelled);
            }
            size = out.read(), if out.is_open() => out.take(size, |l| lines.add(l)),
            size = err.read(), if err.is_open() => err.take(size, |l| lines.add(l)),
        }
    }

    // Both streams are closed, which is the program ending unless it left a process holding them: wait for it
    // the same way, so that a cancel still reaches it.
    let status = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            running.kill().await;
            return Err(RunError::Cancelled);
        }
        status = Box::into_pin(running.child.wait()) => status,
    };
    running.ended = true;
    let status = status.map_err(|e| RunError::Spawn(e.to_string()))?;

    Ok(Exit {
        code: status.code(),
        last_lines: lines.last.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::test_support::script;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// A program and its arguments that run `unix` through `sh` on Unix and `windows` through `cmd` on Windows.
    fn shell(unix: &str, windows: &str) -> (PathBuf, Vec<OsString>) {
        if cfg!(windows) {
            (PathBuf::from("cmd"), vec!["/C".into(), windows.into()])
        } else {
            (PathBuf::from("sh"), vec!["-c".into(), unix.into()])
        }
    }

    fn own_path_entries() -> Vec<PathBuf> {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default()
    }

    #[test]
    fn search_path_contains_path_and_os_dirs() {
        let own = own_path_entries();
        let all: Vec<PathBuf> = std::env::split_paths(&search_path()).collect();

        assert_eq!(all[..own.len()], own[..], "the PATH entries come first");
        #[cfg(target_os = "macos")]
        assert!(all.contains(&PathBuf::from("/opt/homebrew/bin")));
        #[cfg(target_os = "linux")]
        assert!(all.contains(&PathBuf::from("/snap/bin")));
        #[cfg(windows)]
        assert!(all.iter().any(|p| p.ends_with(r"WinGet\Links")));
    }

    #[test]
    fn search_path_adds_the_folders_of_the_user_without_repeating_any() {
        let path_var = std::env::join_paths(["/first", "/usr/local/bin"]).unwrap();
        let joined = search_path_from(Some(path_var), Some(PathBuf::from("/home/me")), None);
        let all: Vec<PathBuf> = std::env::split_paths(&joined).collect();

        assert_eq!(all[0], PathBuf::from("/first"));
        assert_eq!(all[1], PathBuf::from("/usr/local/bin"));
        assert!(all.contains(&PathBuf::from("/home/me").join(".local/bin")));
        assert!(all.contains(&PathBuf::from("/home/me").join("bin")));
        let repeats = all
            .iter()
            .filter(|p| p.as_path() == Path::new("/usr/local/bin"))
            .count();
        assert_eq!(
            repeats, 1,
            "a folder already in the PATH is not added again"
        );
    }

    #[test]
    fn search_path_without_a_home_is_still_the_path() {
        let path_var = std::env::join_paths(["/first"]).unwrap();
        let joined = search_path_from(Some(path_var), None, None);
        let all: Vec<PathBuf> = std::env::split_paths(&joined).collect();

        assert_eq!(all[0], PathBuf::from("/first"));
    }

    #[test]
    fn find_gives_the_tools_that_are_in_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let ffmpeg = dir.path().join(name);
        std::fs::write(&ffmpeg, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let found = Tools::find_in(dir.path().as_os_str());

        // Windows gives the extension as PATHEXT spells it (`.EXE`), so the case is left out of the comparison.
        let lower = |p: &Path| p.to_string_lossy().to_lowercase();
        assert_eq!(found.ffmpeg.as_deref().map(lower), Some(lower(&ffmpeg)));
        assert_eq!(found.yt_dlp, None);
    }

    #[test]
    fn install_commands_name_the_tool_for_this_system() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(install_command(Tool::YtDlp), "brew install yt-dlp ffmpeg");
            assert_eq!(install_command(Tool::Ffmpeg), "brew install ffmpeg");
        }
        #[cfg(windows)]
        {
            assert_eq!(
                install_command(Tool::YtDlp),
                "winget install yt-dlp.yt-dlp Gyan.FFmpeg"
            );
            assert_eq!(install_command(Tool::Ffmpeg), "winget install Gyan.FFmpeg");
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            assert_eq!(
                install_command(Tool::YtDlp),
                "pipx install yt-dlp && sudo apt install ffmpeg"
            );
            assert_eq!(install_command(Tool::Ffmpeg), "sudo apt install ffmpeg");
        }
    }

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut splitter = LineSplitter::default();
        for chunk in chunks {
            splitter.push(chunk, |l| lines.push(l.to_string()));
        }
        splitter.finish(|l| lines.push(l.to_string()));
        lines
    }

    #[test]
    fn lines_split_on_newline_or_carriage_return_and_empty_ones_go() {
        assert_eq!(split(&[b"a\rb\n\nc"]), ["a", "b", "c"]);
        assert_eq!(split(&[b"a\r\nb\r\n"]), ["a", "b"]);
        assert_eq!(split(&[b"\n\r\n"]), Vec::<String>::new());
    }

    #[test]
    fn a_line_in_two_chunks_comes_out_whole() {
        assert_eq!(split(&[b"hel", b"lo\nwor", b"ld"]), ["hello", "world"]);
        // A carriage return that starts the next chunk still ends the line before it.
        assert_eq!(split(&[b"a", b"\rb"]), ["a", "b"]);
    }

    #[test]
    fn a_character_in_two_chunks_is_not_broken() {
        let bytes = "Việt Nam\n".as_bytes();
        let (head, tail) = bytes.split_at(3); // inside the 3-byte "ệ"
        assert_eq!(split(&[head, tail]), ["Việt Nam"]);
    }

    #[test]
    fn bytes_that_are_not_utf8_are_replaced_not_fatal() {
        assert_eq!(split(&[b"a\xffb\n"]), ["a\u{fffd}b"]);
    }

    #[tokio::test]
    async fn run_reports_lines_and_exit() {
        let (program, args) = shell(
            r"printf 'a\rb\n\nc'; exit 3",
            "echo a&echo b&echo.&echo c&exit 3",
        );
        let mut lines = Vec::new();

        let exit = run(&program, &args, &CancellationToken::new(), |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();

        assert_eq!(lines, ["a", "b", "c"]);
        assert_eq!(exit.code, Some(3));
        assert_eq!(exit.last_lines, ["b", "c"]);
    }

    #[tokio::test]
    async fn run_merges_stdout_and_stderr() {
        let (program, args) = shell(
            "echo out; echo err 1>&2; exit 0",
            "echo out&echo err>&2&exit 0",
        );
        let mut lines = Vec::new();

        let exit = run(&program, &args, &CancellationToken::new(), |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();

        lines.sort();
        assert_eq!(lines, ["err", "out"]);
        assert_eq!(exit.code, Some(0));
    }

    #[tokio::test]
    async fn run_gives_the_program_the_search_path_and_unbuffered_python() {
        let (program, args) = shell(
            r#"echo "$PYTHONUNBUFFERED"; echo "$PATH""#,
            "echo %PYTHONUNBUFFERED%",
        );
        let mut lines = Vec::new();

        run(&program, &args, &CancellationToken::new(), |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();

        assert_eq!(lines[0], "1");
        #[cfg(unix)]
        assert_eq!(lines[1], search_path().to_string_lossy());
    }

    #[tokio::test]
    async fn run_that_cannot_start_the_program_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-program");

        let result = run(&missing, &[], &CancellationToken::new(), |_| {}).await;

        assert!(matches!(result, Err(RunError::Spawn(_))), "{result:?}");
    }

    #[tokio::test]
    async fn run_with_a_token_already_cancelled_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let touch = script(
            dir.path(),
            "touch-marker",
            &format!("touch '{}'", marker.display()),
            &format!("echo x> \"{}\"", marker.display()),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = run(&touch, &[], &cancel, |_| {}).await;

        assert_eq!(result, Err(RunError::Cancelled));
        assert!(!marker.exists());
    }

    /// Whether a process with this id is still there (Unix: signal 0 only checks).
    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("sh")
            .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
            .status()
            .unwrap()
            .success()
    }

    /// Waits up to 2 seconds for a process to be gone: init takes a moment to collect what the kill orphaned.
    #[cfg(unix)]
    async fn wait_until_gone(pid: u32) -> bool {
        for _ in 0..40 {
            if !process_exists(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn run_cancel_kills_the_tree() {
        // The shell starts a child that sleeps and says its id, then waits for it.
        let (program, args) = shell("sleep 30 & echo $!; wait", "ping -n 30 127.0.0.1 >nul");
        let cancel = CancellationToken::new();
        let pid = Arc::new(Mutex::new(None::<u32>));
        let started = Instant::now();

        // Cancel once the child is running: Unix says so with its id; Windows has no id, so a timer.
        #[cfg(windows)]
        {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.cancel();
            });
        }
        let result = run(&program, &args, &cancel, {
            let pid = Arc::clone(&pid);
            let cancel = cancel.clone();
            move |line| {
                if let Ok(id) = line.parse::<u32>() {
                    *pid.lock().unwrap() = Some(id);
                    cancel.cancel();
                }
            }
        })
        .await;

        assert_eq!(result, Err(RunError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancel took {:?}",
            started.elapsed()
        );
        #[cfg(unix)]
        {
            let pid = pid
                .lock()
                .unwrap()
                .expect("the shell printed the child's id");
            assert!(
                wait_until_gone(pid).await,
                "the child {pid} is still running"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_dropped_mid_way_kills_the_tree() {
        let pid = Arc::new(Mutex::new(None::<u32>));
        let task = tokio::spawn({
            let pid = Arc::clone(&pid);
            async move {
                let args = ["-c".into(), "sleep 30 & echo $!; wait".into()];
                run(
                    Path::new("sh"),
                    &args,
                    &CancellationToken::new(),
                    move |line| {
                        if let Ok(id) = line.parse::<u32>() {
                            *pid.lock().unwrap() = Some(id);
                        }
                    },
                )
                .await
            }
        });
        let started = Instant::now();
        let id = loop {
            if let Some(id) = *pid.lock().unwrap() {
                break id;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5) && !task.is_finished(),
                "the shell never said its child's id"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        task.abort(); // what dropping the caller's task does to the future
        let _ = task.await;

        assert!(wait_until_gone(id).await, "the child {id} is still running");
    }
}
