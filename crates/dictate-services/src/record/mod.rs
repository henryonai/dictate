//! Recording the screen: the audio of a recording as separate stems on the recording's own clock, the mix of
//! them into the one `audio.m4a` that the rest of the app works from, the quality of the video, and the video itself,
//! which goes through ffmpeg.

pub mod clock;
pub mod mix;
pub mod quality;
pub mod stem;
pub mod video;

/// Probes for the tests of the recording's files.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::media::tools;
    use std::path::Path;
    use std::process::Stdio;

    /// How long the video of `path` is, in seconds: ffmpeg decodes it to nowhere and the last `time=` it printed is
    /// where it got to. This reads all of the video, so a file that stops short (a cut off one) gives how much of it
    /// can be played.
    pub fn video_seconds(path: &Path, ffmpeg: &Path) -> f64 {
        let output = tools::command(ffmpeg)
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(path)
            .args(["-map", "0:v", "-f", "null", "-"])
            .stdin(Stdio::null())
            .output()
            .expect("ffmpeg could not be run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        last_time(&stderr).unwrap_or_else(|| panic!("no time= in what ffmpeg said: {stderr}"))
    }

    /// The last `time=hh:mm:ss.ff` of ffmpeg's progress lines, in seconds.
    fn last_time(stderr: &str) -> Option<f64> {
        let after = &stderr[stderr.rfind("time=")? + "time=".len()..];
        let clock = after.split_whitespace().next()?;
        let mut parts = clock.split(':');
        let hours: f64 = parts.next()?.parse().ok()?;
        let minutes: f64 = parts.next()?.parse().ok()?;
        let seconds: f64 = parts.next()?.parse().ok()?;
        Some(hours * 3600.0 + minutes * 60.0 + seconds)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_last_time_of_the_progress_is_the_length() {
            let said = "frame=  9 fps=0.0 q=-0.0 size=N/A time=00:00:00.60 bitrate=N/A speed=1x    \r\
                        frame=150 fps=0.0 q=-0.0 Lsize=N/A time=01:02:10.25 bitrate=N/A speed=9x    \n";

            assert_eq!(last_time(said), Some(3600.0 + 120.0 + 10.25));
            assert_eq!(last_time("Output file is empty"), None);
            assert_eq!(last_time("size=N/A time=N/A bitrate=N/A"), None);
        }
    }
}
