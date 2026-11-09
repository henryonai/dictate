//! Downloading the audio of a link with yt-dlp.
//!
//! yt-dlp gets `bestaudio[ext=m4a]` first: on YouTube that is a ready AAC file, so no ffmpeg is needed to merge or
//! convert anything. It prints a `TITLE:` line before the download, `DL` lines (from the progress template) while
//! it runs, and a `FILE:` line with the final path when the file is in place.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::tools::{self, RunError};

/// Whether the text is a link yt-dlp may be given: after trimming, `http` or `https` and a host, with no whitespace
/// or control character left inside. (The url crate silently drops tabs and newlines from what it parses, and
/// `args` hands the text on as it is.)
pub fn link_is_valid(url: &str) -> bool {
    let url = url.trim();
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest.starts_with(['/', '\\'])
        && reqwest::Url::parse(url).is_ok_and(|u| u.host_str().is_some_and(|h| !h.is_empty()))
}

/// The command line of yt-dlp that downloads the audio of `url` into `folder` as `source.<ext>`.
///
/// The link comes after `--`, so that nothing in it can be taken for an option; the folder is one argument
/// however its name is spelled.
pub fn args(url: &str, folder: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-f",
        "bestaudio[ext=m4a]/bestaudio/best",
        "--no-playlist",
        "--no-simulate",
        "--newline",
        "--no-colors",
        // Windows yt-dlp prints `TITLE:` and `FILE:` in the ANSI code page otherwise, which mangles a Vietnamese path.
        "--encoding",
        "utf-8",
        // `--print` implies `--quiet`, which also stops the progress lines: ask for them again.
        "--progress",
        "--progress-template",
        "download:DL %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s",
        "--print",
        "before_dl:TITLE:%(title)s",
        "--print",
        "after_move:FILE:%(filepath)s",
        "-o",
    ]
    .map(OsString::from)
    .into();
    args.push(output_template(folder).into_os_string());
    args.push("--".into());
    args.push(url.trim().into());
    args
}

/// The `-o` template: `source.%(ext)s` in the folder. yt-dlp reads a `%` in a template as the start of a field,
/// and `%%` as a plain `%`, so a `%` in the name of the folder is doubled.
fn output_template(folder: &Path) -> PathBuf {
    let folder = match folder.to_str() {
        Some(text) => PathBuf::from(text.replace('%', "%%")),
        None => folder.to_path_buf(),
    };
    folder.join("source.%(ext)s")
}

/// The share of the download that is done (0 to 1), from a line of the progress template:
/// `DL <downloaded> <total> <estimate>`, a figure being `NA` when yt-dlp does not know it. The total is used when
/// there is one, the estimate when not.
pub fn progress(line: &str) -> Option<f32> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "DL" {
        return None;
    }
    let downloaded = bytes(fields.next()?)?;
    let total = fields.next().and_then(bytes).filter(|t| *t > 0.0);
    let estimate = fields.next().and_then(bytes).filter(|t| *t > 0.0);
    let all = total.or(estimate)?;
    Some((downloaded / all).clamp(0.0, 1.0) as f32)
}

/// A figure of the progress template; yt-dlp prints a whole number or one with a fraction.
fn bytes(field: &str) -> Option<f64> {
    field.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// What a download made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    /// The title of the video, when yt-dlp gave one.
    pub title: Option<String>,
    /// The downloaded file.
    pub file: PathBuf,
}

/// Why a link did not download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// yt-dlp ended without a file: its exit code and the last 2 lines it printed, joined by a space.
    Failed { code: Option<i32>, tail: String },
    /// The caller cancelled it.
    Cancelled,
    /// yt-dlp could not be started; the text is the system's detail.
    Spawn(String),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Failed {
                code: Some(c),
                tail,
            } => write!(f, "yt-dlp failed ({c}): {tail}"),
            LinkError::Failed { code: None, tail } => write!(f, "yt-dlp failed: {tail}"),
            LinkError::Cancelled => f.write_str("The download was cancelled."),
            LinkError::Spawn(m) => write!(f, "Could not start yt-dlp: {m}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Downloads the audio of `url` into `folder`.
///
/// `on_progress` gets the share done (0 to 1) as yt-dlp reports it, and `on_line` every line it prints, the
/// progress lines too. The first `TITLE:` line is the title; the download counts only with exit code 0 and a
/// `FILE:` line, the last one of which is the file.
pub async fn download(
    yt_dlp: &Path,
    url: &str,
    folder: &Path,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(f32) + Send,
    mut on_line: impl FnMut(&str) + Send,
) -> Result<Downloaded, LinkError> {
    let mut title: Option<String> = None;
    let mut file: Option<PathBuf> = None;

    let result = tools::run(yt_dlp, &args(url, folder), cancel, |line| {
        on_line(line);
        if let Some(share) = progress(line) {
            on_progress(share);
        } else if let Some(text) = line.strip_prefix("TITLE:") {
            // yt-dlp says NA for a title it does not have.
            let text = text.trim();
            if title.is_none() && !text.is_empty() && text != "NA" {
                title = Some(text.to_owned());
            }
        } else if let Some(path) = line.strip_prefix("FILE:") {
            if !path.is_empty() {
                file = Some(PathBuf::from(path));
            }
        }
    })
    .await;

    match result {
        Err(RunError::Cancelled) => Err(LinkError::Cancelled),
        Err(RunError::Spawn(detail)) => Err(LinkError::Spawn(detail)),
        Ok(exit) => match (exit.code, file) {
            (Some(0), Some(file)) => Ok(Downloaded { title, file }),
            (code, _) => Err(LinkError::Failed {
                code,
                tail: exit.last_lines.join(" "),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::test_support::script;

    #[test]
    fn link_must_be_http() {
        assert!(link_is_valid("https://youtu.be/x"));
        assert!(link_is_valid("http://example.com/watch?v=1"));
        assert!(link_is_valid("  https://youtu.be/x\n"));
        assert!(link_is_valid("HTTPS://YOUTU.BE/X"));

        assert!(!link_is_valid("ftp://x"));
        assert!(!link_is_valid("-x"));
        assert!(!link_is_valid("--exec=rm"));
        assert!(!link_is_valid("javascript:alert(1)"));
        assert!(!link_is_valid("http://"));
        assert!(!link_is_valid("https:///path"));
        assert!(!link_is_valid("file:///etc/passwd"));
        assert!(!link_is_valid(""));
        assert!(!link_is_valid("   "));
        assert!(!link_is_valid("youtu.be/x"));
    }

    #[test]
    fn link_with_whitespace_or_control_characters_inside_is_not_valid() {
        // The url crate drops tabs and newlines from what it parses, so these would pass `Url::parse`, but `args`
        // hands the text on as it is.
        assert!(!link_is_valid("https://youtu.be/x y"));
        assert!(!link_is_valid("https://youtu.be/x\ty"));
        assert!(!link_is_valid("https://youtu.be/x\ny"));
        assert!(!link_is_valid("https://youtu.be/x\r\ny"));
        assert!(!link_is_valid("https://you\ttu.be/x"));
        assert!(!link_is_valid("https://youtu.be/x\u{0}y"));
        assert!(!link_is_valid("https://youtu.be/x\u{7f}y"));
        assert!(!link_is_valid("https://youtu.be/x\u{a0}y"));
        assert!(!link_is_valid("https://youtu.be/x\u{3000}y"));
        // Around the link is still trimmed away.
        assert!(link_is_valid("\t https://youtu.be/x?a=1&b=2 \r\n"));
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn ytdlp_args_end_with_double_dash_and_url() {
        let folder = Path::new("/lib/Cuộc họp tháng 11/2026-11-08 10-00-00");
        let args = strings(&args("https://youtu.be/x", folder));

        assert_eq!(args[args.len() - 2..], ["--", "https://youtu.be/x"]);
        // The folder is one argument, spaces and Vietnamese letters and all.
        let template =
            Path::new(&args[args.iter().position(|a| a == "-o").unwrap() + 1]).to_path_buf();
        assert_eq!(template, folder.join("source.%(ext)s"));
    }

    #[test]
    fn ytdlp_args_are_the_documented_command_line() {
        let args = strings(&args("https://youtu.be/x", Path::new("/lib/f")));
        let template = Path::new("/lib/f").join("source.%(ext)s");

        // `--print` makes yt-dlp quiet, which also stops its progress lines: `--progress` brings them back. On
        // Windows yt-dlp writes `TITLE:` and `FILE:` in the ANSI code page unless it is told `--encoding utf-8`.
        assert_eq!(
            args,
            [
                "-f",
                "bestaudio[ext=m4a]/bestaudio/best",
                "--no-playlist",
                "--no-simulate",
                "--newline",
                "--no-colors",
                "--encoding",
                "utf-8",
                "--progress",
                "--progress-template",
                "download:DL %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s",
                "--print",
                "before_dl:TITLE:%(title)s",
                "--print",
                "after_move:FILE:%(filepath)s",
                "-o",
                template.to_str().unwrap(),
                "--",
                "https://youtu.be/x",
            ]
        );
    }

    #[test]
    fn ytdlp_args_trim_the_link() {
        let args = strings(&args("  https://youtu.be/x \n", Path::new("/lib/f")));

        assert_eq!(args.last().unwrap(), "https://youtu.be/x");
    }

    #[test]
    fn ytdlp_args_keep_a_percent_in_the_folder_from_being_a_template_field() {
        let args = strings(&args("https://youtu.be/x", Path::new("/lib/100% done")));
        let template = &args[args.iter().position(|a| a == "-o").unwrap() + 1];

        assert_eq!(
            Path::new(template),
            Path::new("/lib/100%% done").join("source.%(ext)s")
        );
    }

    #[test]
    fn ytdlp_progress_parses_template_lines() {
        assert_eq!(progress("DL 50 100 NA"), Some(0.5));
        assert_eq!(progress("DL 10 NA 40"), Some(0.25));
        assert_eq!(progress("DL NA NA NA"), None);
        assert_eq!(progress("[download] 5%"), None);
    }

    #[test]
    fn ytdlp_progress_takes_the_total_before_the_estimate_and_stays_in_range() {
        assert_eq!(progress("DL 50 100 400"), Some(0.5));
        assert_eq!(progress("DL 150 100 NA"), Some(1.0));
        assert_eq!(progress("DL 100.0 200.0 NA"), Some(0.5));
        assert_eq!(progress("DL 5 0 NA"), None);
        assert_eq!(progress("DL NA 100 NA"), None);
        assert_eq!(progress("DL x y z"), None);
        assert_eq!(progress("DL 50"), None);
        assert_eq!(progress("DL"), None);
        assert_eq!(progress("xDL 1 2 NA"), None);
    }

    /// A fake yt-dlp that prints `unix` (or `windows`) and ends with `code`.
    fn fake_yt_dlp(dir: &Path, unix: &str, windows: &str) -> PathBuf {
        script(dir, "yt-dlp", unix, windows)
    }

    #[tokio::test]
    async fn download_reads_title_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("source.m4a");
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            &format!(
                "echo TITLE:Hello\necho DL 1 2 NA\necho FILE:{}\nexit 0",
                file.display()
            ),
            &format!(
                "echo TITLE:Hello\r\necho DL 1 2 NA\r\necho FILE:{}\r\nexit /b 0",
                file.display()
            ),
        );
        let mut progress = Vec::new();
        let mut lines = Vec::new();

        let got = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |p| progress.push(p),
            |l| lines.push(l.to_string()),
        )
        .await
        .unwrap();

        assert_eq!(
            got,
            Downloaded {
                title: Some("Hello".into()),
                file
            }
        );
        assert_eq!(progress, [0.5]);
        assert_eq!(lines.len(), 3, "every line goes to on_line: {lines:?}");
    }

    #[tokio::test]
    async fn download_takes_the_first_title_and_the_last_file() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            "echo TITLE:First\necho TITLE:Second\necho FILE:/a/one.webm\necho FILE:/a/two.m4a\nexit 0",
            "echo TITLE:First\r\necho TITLE:Second\r\necho FILE:/a/one.webm\r\necho FILE:/a/two.m4a\r\nexit /b 0",
        );

        let got = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(got.title.as_deref(), Some("First"));
        assert_eq!(got.file, PathBuf::from("/a/two.m4a"));
    }

    #[tokio::test]
    async fn download_without_a_title_still_gives_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            "echo TITLE:NA\necho FILE:/a/one.m4a\nexit 0",
            "echo TITLE:NA\r\necho FILE:/a/one.m4a\r\nexit /b 0",
        );

        let got = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(got.title, None);
    }

    #[tokio::test]
    async fn download_failure_keeps_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            "echo '[youtube] x: Downloading webpage'\necho 'WARNING: slow'\necho 'ERROR: Video unavailable'\nexit 1",
            "echo [youtube] x: Downloading webpage\r\necho WARNING: slow\r\necho ERROR: Video unavailable\r\nexit /b 1",
        );

        let result = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await;

        assert_eq!(
            result,
            Err(LinkError::Failed {
                code: Some(1),
                tail: "WARNING: slow ERROR: Video unavailable".into()
            })
        );
    }

    #[tokio::test]
    async fn download_that_exits_cleanly_without_a_file_line_fails() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            "echo TITLE:Hello\nexit 0",
            "echo TITLE:Hello\r\nexit /b 0",
        );

        let result = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await;

        assert_eq!(
            result,
            Err(LinkError::Failed {
                code: Some(0),
                tail: "TITLE:Hello".into()
            })
        );
    }

    #[tokio::test]
    async fn download_that_fails_with_a_file_line_is_still_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(
            dir.path(),
            "echo FILE:/a/one.m4a\nexit 2",
            "echo FILE:/a/one.m4a\r\nexit /b 2",
        );

        let result = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await;

        assert!(
            matches!(result, Err(LinkError::Failed { code: Some(2), .. })),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn download_cancelled_says_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let yt_dlp = fake_yt_dlp(dir.path(), "sleep 30", "ping -n 30 127.0.0.1 >nul");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = download(
            &yt_dlp,
            "https://youtu.be/x",
            dir.path(),
            &cancel,
            |_| {},
            |_| {},
        )
        .await;

        assert_eq!(result, Err(LinkError::Cancelled));
    }

    #[tokio::test]
    async fn download_with_no_program_says_it_could_not_start() {
        let dir = tempfile::tempdir().unwrap();

        let result = download(
            &dir.path().join("missing"),
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            |_| {},
            |_| {},
        )
        .await;

        assert!(matches!(result, Err(LinkError::Spawn(_))), "{result:?}");
    }
}
