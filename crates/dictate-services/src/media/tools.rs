//! Finding ffmpeg and yt-dlp, and running them.
//!
//! `run` starts a program for the pipeline: it puts every line the program prints (stdout and stderr together,
//! and a progress line ended by `\r` too) into a callback, and on cancel kills the program and everything it
//! started. `command` is for a program that is read or written as bytes (ffmpeg decoding or encoding audio) instead.
//! The setup every child process gets (the search path, no console window) is in `base_command`.

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

/// The folders to look for programs in: the system's own folders (on Windows), then the app's own PATH, then the
/// places each system puts what the user installs. The PATH a desktop app gets is short (on a Mac it is
/// `/usr/bin:/bin:/usr/sbin:/sbin`, with no Homebrew), and a Linux launcher often leaves out `~/.local/bin`.
///
/// Child processes get this as their PATH too: yt-dlp looks for ffmpeg there.
///
/// The system folders go first because that is the order Windows itself puts in a PATH (the system folders before
/// the user's), and so that a program started through cmd.exe finds `where`, `ping` and the other programs of the
/// system. cmd.exe ignores a PATH longer than 8191 characters altogether (it does not read a prefix of it), whatever
/// the order, and a `.cmd` or `.bat` shim of a tool is run by cmd, so it inherits that limit.
pub fn search_path() -> OsString {
    search_path_from(
        system_folders(),
        std::env::var_os("PATH"),
        dirs::home_dir(),
        dirs::data_local_dir(),
    )
}

/// The folders of the system's own programs, which go before everything else in the search path. Only Windows has
/// any: `%SystemRoot%\System32` and `%SystemRoot%` (why they go first: see `search_path`).
fn system_folders() -> Vec<PathBuf> {
    #[cfg(windows)]
    return match std::env::var_os("SystemRoot") {
        Some(root) if !root.is_empty() => {
            let root = PathBuf::from(root);
            vec![root.join("System32"), root]
        }
        _ => Vec::new(),
    };
    #[cfg(not(windows))]
    return Vec::new();
}

/// What separates the folders in a PATH on this system.
#[cfg(windows)]
const PATH_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const PATH_SEPARATOR: &str = ":";

/// Adds one entry to a PATH being written, after the separator when it is not empty.
fn push_entry(path: &mut OsString, entry: &OsStr) {
    if !path.is_empty() {
        path.push(PATH_SEPARATOR);
    }
    path.push(entry);
}

/// `search_path` for given system folders, PATH, home folder and local app data folder (the last is only used on
/// Windows). The order is the `front` folders, the PATH, then the folders of the tools.
///
/// The PATH is kept as it is: splitting a PATH and joining it again is not always the same PATH (Windows reads
/// quote marks in it, and a stray one makes the folders after it one), and the PATH of the user is what finds
/// their programs.
#[cfg_attr(not(windows), allow(unused_variables))]
fn search_path_from(
    front: Vec<PathBuf>,
    path_var: Option<OsString>,
    home: Option<PathBuf>,
    local_data: Option<PathBuf>,
) -> OsString {
    let mut known: Vec<PathBuf> = path_var
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

    // A folder with the separator in its name is quoted, and one that cannot be is left out.
    let mut joined = OsString::new();
    for dir in front {
        if let Ok(entry) = std::env::join_paths([&dir]) {
            push_entry(&mut joined, &entry);
        }
    }
    if let Some(path_var) = path_var.filter(|p| !p.is_empty()) {
        push_entry(&mut joined, &path_var);
    }
    for dir in extra {
        if known.contains(&dir) {
            continue;
        }
        if let Ok(entry) = std::env::join_paths([&dir]) {
            push_entry(&mut joined, &entry);
            known.push(dir);
        }
    }
    joined
}

/// The variable that names the ffmpeg to use in place of the one that ships with the app: a path to a program, or the name
/// of one in the search path. The ffmpeg of the installers is under the LGPL, which lets anyone replace it with their own
/// build; this is how it is done without touching the installed files (on a Mac a changed file in a signed app breaks its
/// signature).
pub const FFMPEG_VARIABLE: &str = "DICTATE_FFMPEG";

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

    /// The name of the file of this tool that ships with the app, next to its executable, without the `.exe`. The ffmpeg
    /// of the installers is not called `ffmpeg`: a `.deb` puts its programs in /usr/bin, where the one of the
    /// distribution is, and the two must not be mixed up.
    fn bundled(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "dictate-ffmpeg",
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
    /// Looks for the tools where the app is told to look, then next to the program (the installers put the ffmpeg
    /// of the app there, as `dictate-ffmpeg`), and then in `search_path()`.
    ///
    /// For ffmpeg: the program that `DICTATE_FFMPEG` names, then `dictate-ffmpeg` next to the executable, then `ffmpeg` in
    /// the search path. For yt-dlp: next to the executable, then the search path.
    pub fn find() -> Tools {
        let path = search_path();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        Tools::find_with(
            std::env::var_os(FFMPEG_VARIABLE).as_deref(),
            exe_dir.as_deref(),
            &path,
        )
    }

    /// `find` for a given value of `DICTATE_FFMPEG`, folder of the executable and search path.
    ///
    /// A `DICTATE_FFMPEG` that is empty, or names no program that can be run, is left out: the app still has the ffmpeg
    /// that ships with it.
    fn find_with(
        ffmpeg_variable: Option<&OsStr>,
        exe_dir: Option<&Path>,
        search_path: &OsStr,
    ) -> Tools {
        let cwd = std::env::current_dir().unwrap_or_default();
        let next_to_the_exe =
            |tool: Tool| exe_dir.and_then(|dir| locate_in(tool.bundled(), dir.as_os_str(), &cwd));
        let in_the_path = |tool: Tool| locate_in(tool.program(), search_path, &cwd);
        let from_the_variable = ffmpeg_variable
            .filter(|name| !name.is_empty())
            .and_then(|name| locate_in(name, search_path, &cwd));
        Tools {
            ffmpeg: from_the_variable
                .or_else(|| next_to_the_exe(Tool::Ffmpeg))
                .or_else(|| in_the_path(Tool::Ffmpeg)),
            yt_dlp: next_to_the_exe(Tool::YtDlp).or_else(|| in_the_path(Tool::YtDlp)),
        }
    }
}

/// The program called `name` (a name, or a path) in the folders of `search_path`.
fn locate_in(name: impl AsRef<OsStr>, search_path: &OsStr, cwd: &Path) -> Option<PathBuf> {
    which::which_in(name, Some(search_path), cwd).ok()
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

/// The command every child process starts from: `path` as its search path (`search_path()`, so that yt-dlp finds
/// ffmpeg), Python's output unbuffered (so that the progress lines arrive as they are printed), and on Windows no
/// console window flashing up.
fn base_command(program: &Path, path: &OsStr) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.env("PATH", path).env("PYTHONUNBUFFERED", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW.0);
    }
    command
}

/// A command for a program that is read or written as bytes (ffmpeg decoding to PCM, or encoding from it), which
/// `run` cannot do: it reads lines. The program gets what every child process gets (see `base_command`): the
/// search path of `search_path()`, Python's output unbuffered, and on Windows no console window. Its input and output
/// are left as `std::process::Command` has them, to be set by the caller; the caller also has to end the program
/// (kill it and wait for it) if it is not read to its end.
pub fn command(program: &Path) -> std::process::Command {
    base_command(program, &search_path())
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
    run_with_path(program, args, &search_path(), cancel, on_line).await
}

/// `run` with `path` as the search path of the program instead of `search_path()`.
async fn run_with_path(
    program: &Path,
    args: &[OsString],
    path: &OsStr,
    cancel: &CancellationToken,
    on_line: impl FnMut(&str) + Send,
) -> Result<Exit, RunError> {
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }

    let mut command = base_command(program, path);
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

    /// A program of the Windows System32 folder, by its full path.
    #[cfg(windows)]
    fn system32(file: &str) -> PathBuf {
        let root = std::env::var("SystemRoot").expect("Windows always sets SystemRoot");
        Path::new(&root).join("System32").join(file)
    }

    #[test]
    fn search_path_contains_path_and_os_dirs() {
        let own = std::env::var_os("PATH").unwrap_or_default();
        let own = own.to_string_lossy();
        let all = search_path().to_string_lossy().into_owned();

        // On Windows the system folders go before the PATH (see `search_path_from`).
        #[cfg(windows)]
        let before = {
            let root = std::env::var("SystemRoot").expect("Windows always sets SystemRoot");
            format!(r"{root}\System32;{root};")
        };
        #[cfg(not(windows))]
        let before = String::new();
        assert!(all.starts_with(&before), "{all}");
        assert!(
            all[before.len()..].starts_with(&*own),
            "the PATH comes next: {all}"
        );
        #[cfg(target_os = "macos")]
        assert!(all.contains("/opt/homebrew/bin"), "{all}");
        #[cfg(target_os = "linux")]
        assert!(all.contains("/snap/bin"), "{all}");
        #[cfg(windows)]
        assert!(all.contains(r"WinGet\Links"), "{all}");
    }

    #[test]
    fn search_path_puts_the_system_folders_first_then_the_path_then_the_user_folders() {
        // The system's own folders go first, as in Windows' own PATH, and the user's PATH is kept whole after them,
        // however long it is (a Windows test process under `cargo test` has one of 15000 characters).
        let long_path = vec!["/some/long/folder/name"; 1000].join(&SEP.to_string());

        let joined = search_path_from(
            vec![PathBuf::from("/sys/System32"), PathBuf::from("/sys")],
            Some(OsString::from(&long_path)),
            Some(PathBuf::from("/home/me")),
            None,
        );

        let joined = joined.to_string_lossy().into_owned();
        let front = format!("/sys/System32{SEP}/sys{SEP}");
        assert!(joined.starts_with(&front), "{}", &joined[..80]);
        let rest = &joined[front.len()..];
        assert!(rest.starts_with(&long_path), "the PATH is not kept whole");
        let after_the_path = &rest[long_path.len()..];
        assert!(after_the_path.starts_with(SEP), "{after_the_path}");
        let user_folders: Vec<PathBuf> = std::env::split_paths(after_the_path).collect();
        assert!(
            user_folders.contains(&PathBuf::from("/home/me").join("bin")),
            "{after_the_path}"
        );
    }

    #[test]
    fn search_path_without_a_path_is_the_system_folders_and_the_user_folders() {
        let joined = search_path_from(
            vec![PathBuf::from("/sys")],
            None,
            Some(PathBuf::from("/home/me")),
            None,
        );
        let all: Vec<PathBuf> = std::env::split_paths(&joined).collect();

        assert_eq!(all[0], PathBuf::from("/sys"));
        assert!(all.contains(&PathBuf::from("/home/me").join("bin")));
    }

    #[test]
    fn search_path_adds_the_folders_of_the_user_without_repeating_any() {
        let path_var = std::env::join_paths(["/first", "/usr/local/bin"]).unwrap();
        let joined = search_path_from(
            Vec::new(),
            Some(path_var),
            Some(PathBuf::from("/home/me")),
            None,
        );
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
        let joined = search_path_from(Vec::new(), Some(path_var), None, None);
        let all: Vec<PathBuf> = std::env::split_paths(&joined).collect();

        assert_eq!(all[0], PathBuf::from("/first"));
    }

    /// The separator of the PATH on this system.
    const SEP: char = if cfg!(windows) { ';' } else { ':' };

    #[test]
    fn search_path_keeps_the_path_exactly_as_it_was_given() {
        // An empty entry and a quote mark that never closes. Splitting this PATH and joining it again would read
        // the quote the Windows way (everything after it is one quoted folder) and write a different PATH.
        let path_var = format!("/first{SEP}{SEP}/odd\"dir{SEP}/last");

        let joined = search_path_from(
            Vec::new(),
            Some(OsString::from(&path_var)),
            Some(PathBuf::from("/home/me")),
            None,
        );

        let joined = joined.to_string_lossy().into_owned();
        assert!(
            joined.starts_with(&format!("{path_var}{SEP}")),
            "the PATH changed: {joined}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn search_path_leaves_out_a_folder_that_cannot_be_in_a_path_and_adds_the_rest() {
        // The home folder has the separator in its name, so the folders under it cannot be written into the PATH.
        let joined = search_path_from(
            Vec::new(),
            Some(OsString::from("/first")),
            Some(PathBuf::from("/home/a:b")),
            None,
        );

        let joined = joined.to_string_lossy().into_owned();
        assert!(joined.starts_with("/first:"), "{joined}");
        assert!(!joined.contains("a:b"), "{joined}");
        assert!(
            joined.len() > "/first:".len(),
            "no folder was added: {joined}"
        );
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

        let found = Tools::find_with(None, None, dir.path().as_os_str());

        // Windows gives the extension as PATHEXT spells it (`.EXE`), so the case is left out of the comparison.
        let lower = |p: &Path| p.to_string_lossy().to_lowercase();
        assert_eq!(found.ffmpeg.as_deref().map(lower), Some(lower(&ffmpeg)));
        assert_eq!(found.yt_dlp, None);
    }

    /// An executable file `stem` (`stem.exe` on Windows) in `dir`: empty, which is enough to be found.
    fn program(dir: &Path, stem: &str) -> PathBuf {
        let name = if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_owned()
        };
        let program = dir.join(name);
        std::fs::write(&program, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        program
    }

    /// Windows gives the extension as PATHEXT spells it (`.EXE`), so the case is left out of the comparison.
    fn lower(path: &Path) -> String {
        path.to_string_lossy().to_lowercase()
    }

    #[test]
    fn ffmpeg_from_the_environment_wins() {
        // The user's own ffmpeg replaces the one that ships (the LGPL asks that it can be replaced), whatever the
        // file is called and wherever it is.
        let exe_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let theirs_dir = tempfile::tempdir().unwrap();
        program(exe_dir.path(), "dictate-ffmpeg");
        program(path_dir.path(), "ffmpeg");
        let theirs = program(theirs_dir.path(), "my-own-ffmpeg");

        let found = Tools::find_with(
            Some(theirs.as_os_str()),
            Some(exe_dir.path()),
            path_dir.path().as_os_str(),
        );

        assert_eq!(found.ffmpeg.as_deref().map(lower), Some(lower(&theirs)));
    }

    #[test]
    fn an_environment_ffmpeg_that_is_not_there_or_is_empty_is_left_out() {
        let exe_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let bundled = program(exe_dir.path(), "dictate-ffmpeg");
        let nowhere = exe_dir.path().join("no-such-ffmpeg");

        for value in [nowhere.as_os_str(), OsStr::new("")] {
            let found = Tools::find_with(
                Some(value),
                Some(exe_dir.path()),
                path_dir.path().as_os_str(),
            );

            assert_eq!(
                found.ffmpeg.as_deref().map(lower),
                Some(lower(&bundled)),
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_bundled_dictate_ffmpeg_comes_before_the_path() {
        // The installers put `dictate-ffmpeg` next to the program (it is not called `ffmpeg`: a .deb would put that in
        // /usr/bin, where the ffmpeg of the distribution is).
        let exe_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let next_to_it = program(exe_dir.path(), "dictate-ffmpeg");
        program(path_dir.path(), "ffmpeg");

        let found = Tools::find_with(None, Some(exe_dir.path()), path_dir.path().as_os_str());

        assert_eq!(found.ffmpeg.as_deref().map(lower), Some(lower(&next_to_it)));
    }

    #[test]
    fn without_either_the_path_is_searched() {
        let exe_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let on_the_path = program(path_dir.path(), "ffmpeg");

        let found = Tools::find_with(None, Some(exe_dir.path()), path_dir.path().as_os_str());

        assert_eq!(
            found.ffmpeg.as_deref().map(lower),
            Some(lower(&on_the_path))
        );
        // And with nothing anywhere there is no ffmpeg.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            Tools::find_with(None, Some(exe_dir.path()), empty.path().as_os_str()).ffmpeg,
            None
        );
    }

    #[test]
    fn yt_dlp_is_still_looked_for_next_to_the_executable_first() {
        let exe_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let next_to_it = program(exe_dir.path(), "yt-dlp");
        program(path_dir.path(), "yt-dlp");

        let found = Tools::find_with(None, Some(exe_dir.path()), path_dir.path().as_os_str());

        assert_eq!(found.yt_dlp.as_deref().map(lower), Some(lower(&next_to_it)));
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
        // On Windows PowerShell reads the variables: cmd.exe cannot show a PATH longer than 8191 characters, and
        // the one a `cargo test` process has is 15000.
        #[cfg(windows)]
        let (program, args): (PathBuf, Vec<OsString>) = (
            system32(r"WindowsPowerShell\v1.0\powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "'PYTHONUNBUFFERED=' + $env:PYTHONUNBUFFERED; 'PATH=' + $env:PATH".into(),
            ],
        );
        #[cfg(not(windows))]
        let (program, args) = shell(
            r#"echo "PYTHONUNBUFFERED=$PYTHONUNBUFFERED"; echo "PATH=$PATH""#,
            "",
        );
        let mut lines = Vec::new();

        run(&program, &args, &CancellationToken::new(), |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();

        assert_eq!(
            lines,
            [
                "PYTHONUNBUFFERED=1".to_string(),
                format!("PATH={}", search_path().to_string_lossy())
            ]
        );
    }

    #[test]
    fn command_gives_the_program_the_search_path_and_unbuffered_python() {
        // The same program as in `run_gives_the_program_the_search_path_and_unbuffered_python`, started by hand: a
        // byte-stream child (ffmpeg writing PCM) is not run through `run`, which reads lines.
        #[cfg(windows)]
        let (program, args): (PathBuf, Vec<OsString>) = (
            system32(r"WindowsPowerShell\v1.0\powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "'PYTHONUNBUFFERED=' + $env:PYTHONUNBUFFERED; 'PATH=' + $env:PATH".into(),
            ],
        );
        #[cfg(not(windows))]
        let (program, args) = shell(
            r#"echo "PYTHONUNBUFFERED=$PYTHONUNBUFFERED"; echo "PATH=$PATH""#,
            "",
        );

        let output = command(&program).args(args).output().unwrap();

        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "PYTHONUNBUFFERED=1".to_string(),
                format!("PATH={}", search_path().to_string_lossy())
            ]
        );
    }

    #[tokio::test]
    async fn run_lets_the_program_start_system_programs_by_bare_name() {
        // `where` is in System32 on Windows and `uname` in /usr/bin: both are found through the PATH only. The PATH
        // is the one Dictate builds, from the real system folders and a PATH of normal length: under `cargo test`
        // on Windows the PATH of the test process is about 15000 characters, and cmd.exe ignores a PATH longer
        // than 8191 altogether, so no order of the folders would let cmd find `where` there.
        let short_path = std::env::join_paths(short_system_path()).unwrap();
        let path = search_path_from(system_folders(), Some(short_path), dirs::home_dir(), None);
        let (program, args) = shell("command -v uname", "where ping");
        let mut lines = Vec::new();

        let exit = run_with_path(&program, &args, &path, &CancellationToken::new(), |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();

        assert_eq!(
            exit.code,
            Some(0),
            "{lines:?} with the PATH ({} characters) {path:?}",
            path.len()
        );
        assert!(!lines.is_empty());
    }

    /// A PATH as short as the one a desktop app gets: the folders of the system's own programs.
    fn short_system_path() -> Vec<PathBuf> {
        if cfg!(windows) {
            let root = PathBuf::from(
                std::env::var_os("SystemRoot").expect("Windows always sets SystemRoot"),
            );
            vec![
                root.join("System32"),
                root.join("System32").join("Wbem"),
                root,
            ]
        } else {
            ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
                .map(PathBuf::from)
                .to_vec()
        }
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
        // On Windows the sleeper is ping.exe, by its full path: finding programs in the PATH is tested apart.
        #[cfg(windows)]
        let ping = format!("{} -n 30 127.0.0.1 >nul", system32("ping.exe").display());
        #[cfg(not(windows))]
        let ping = String::new();
        let (program, args) = shell("sleep 30 & echo $!; wait", &ping);
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
