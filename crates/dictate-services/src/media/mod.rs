//! Media files and the programs that handle them: finding ffmpeg and yt-dlp, running them, and downloading a link.

pub mod tools;
pub mod ytdlp;

/// Stand-in programs for the tests of the media modules.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Writes a runnable script into `dir` and returns its path: a shell script with the body `unix` on Unix, a
    /// batch file with the body `windows` on Windows.
    pub fn script(dir: &Path, name: &str, unix: &str, windows: &str) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            fs::write(&path, format!("#!/bin/sh\n{unix}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            let _ = windows;
            path
        }
        #[cfg(windows)]
        {
            let path = dir.join(format!("{name}.cmd"));
            fs::write(&path, format!("@echo off\r\n{windows}\r\n")).unwrap();
            let _ = unix;
            path
        }
    }
}
