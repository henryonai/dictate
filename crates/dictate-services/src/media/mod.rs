//! Media files and the programs that handle them: finding ffmpeg and yt-dlp, running them, downloading a link,
//! decoding an imported file and writing `audio.m4a`.

pub mod decode;
pub mod encode;
pub mod tools;
pub mod ytdlp;

/// Stand-in programs for the tests of the media modules.
#[cfg(test)]
pub(crate) mod test_support {
    use super::decode::MonoDecoder;
    use super::tools::Tools;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Every block the decoder gives, in one vector.
    pub fn read_all(decoder: &mut MonoDecoder) -> Vec<f32> {
        let mut samples = Vec::new();
        while let Some(block) = decoder.next_block().unwrap() {
            assert!(
                !block.is_empty(),
                "the end is `None`: an empty block is not a block"
            );
            samples.extend(block);
        }
        samples
    }

    /// How often the signal crosses zero, not counting what is near silence: the first frame of an AAC file is the
    /// encoder's warm-up, a murmur of 0.003 that crosses zero 80 times, and a tone at half scale is nowhere near that.
    pub fn zero_crossings(samples: &[f32]) -> usize {
        let mut loud = samples.iter().filter(|s| s.abs() > 0.05);
        let Some(mut before) = loud.next() else {
            return 0;
        };
        let mut crossings = 0;
        for sample in loud {
            crossings += usize::from((*before < 0.0) != (*sample < 0.0));
            before = sample;
        }
        crossings
    }

    /// The ffmpeg of this machine, for a test that needs a real one. Without one the test is skipped, with a note,
    /// except on CI (the `CI` variable is set, as GitHub Actions does): CI installs ffmpeg, and a test that skipped
    /// there would pass without having run.
    pub fn real_ffmpeg() -> Option<PathBuf> {
        let on_ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
        ffmpeg_or_skip(Tools::find().ffmpeg, on_ci)
    }

    fn ffmpeg_or_skip(found: Option<PathBuf>, on_ci: bool) -> Option<PathBuf> {
        if found.is_none() {
            assert!(
                !on_ci,
                "ffmpeg is not installed, and on CI the tests that need it fail instead of being skipped"
            );
            eprintln!("skipped: ffmpeg is not installed");
        }
        found
    }

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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_missing_ffmpeg_skips_the_test_and_says_so() {
            assert_eq!(ffmpeg_or_skip(None, false), None);
        }

        #[test]
        #[should_panic(expected = "fail instead of being skipped")]
        fn a_missing_ffmpeg_fails_the_test_on_ci() {
            ffmpeg_or_skip(None, true);
        }

        #[test]
        fn an_ffmpeg_that_is_there_is_used_on_ci_and_elsewhere() {
            let path = PathBuf::from("/some/ffmpeg");
            assert_eq!(ffmpeg_or_skip(Some(path.clone()), true), Some(path.clone()));
            assert_eq!(ffmpeg_or_skip(Some(path.clone()), false), Some(path));
        }
    }
}
