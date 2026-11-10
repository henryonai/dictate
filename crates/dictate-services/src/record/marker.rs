//! `recording.json`: the file that says a folder has a recording in it that is not over. It is written before the first
//! source starts and removed when the recording has been put together, so that a folder that still has it at the next
//! start-up is one that a crash (or a quit that did not wait) left behind, and the app can finish it
//! (`session::recover`).

use super::RecordOptions;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// The name of the file.
pub const MARKER: &str = "recording.json";

/// What `recording.json` holds.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Marker {
    /// The layout of this file: 1.
    pub version: u32,
    pub options: RecordOptions,
    /// The video encoder of ffmpeg that was picked (screen modes).
    pub codec: Option<String>,
    /// The extension of the video's container: `mp4` or `webm` (screen modes).
    pub container: Option<String>,
    /// When the recording began, in seconds since 1970.
    pub started: f64,
}

impl Marker {
    /// Writes `folder/recording.json`.
    pub fn write(&self, folder: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        std::fs::write(folder.join(MARKER), json)
    }

    /// Reads `folder/recording.json`: `None` when there is none.
    pub fn read(folder: &Path) -> io::Result<Option<Marker>> {
        let bytes = match std::fs::read(folder.join(MARKER)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::quality::Quality;
    use crate::record::Mode;

    fn marker() -> Marker {
        Marker {
            version: 1,
            options: RecordOptions {
                mode: Mode::Window { id: "42".into() },
                quality: Quality::Sharp,
                mic: true,
                system_audio: false,
            },
            codec: Some("libx264".into()),
            container: Some("mp4".into()),
            started: 1_700_000_000.5,
        }
    }

    #[test]
    fn a_marker_is_read_back_as_it_was_written() {
        let dir = tempfile::tempdir().unwrap();

        marker().write(dir.path()).unwrap();

        assert_eq!(Marker::read(dir.path()).unwrap(), Some(marker()));
        assert!(dir.path().join(MARKER).is_file());
    }

    #[test]
    fn the_file_is_json_that_names_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        marker().write(dir.path()).unwrap();

        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER)).unwrap()).unwrap();

        assert_eq!(json["version"], 1);
        assert_eq!(json["options"]["mode"]["kind"], "window");
        assert_eq!(json["options"]["quality"], "sharp");
        assert_eq!(json["codec"], "libx264");
        assert_eq!(json["container"], "mp4");
        assert_eq!(json["started"], 1_700_000_000.5);
    }

    #[test]
    fn a_folder_without_a_marker_has_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Marker::read(dir.path()).unwrap(), None);
    }

    #[test]
    fn a_marker_that_is_not_json_is_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MARKER), "{ not json").unwrap();
        let error = Marker::read(dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
