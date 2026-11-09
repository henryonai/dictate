//! Where a dictation's audio and text go: the saved ones in `Dictate/Dictations`, and the temporary audio of a
//! dictation that isn't saved, which is deleted after use and swept at launch.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use dictate_services::audio::wav;

/// The prefix and the extension of the temporary audio files.
const TEMP_PREFIX: &str = "dictate-";
const TEMP_EXTENSION: &str = "wav";

/// The two folders a dictation writes to.
#[derive(Debug, Clone)]
pub struct SavePaths {
    /// Saved dictations: audio and text with the same base name.
    pub dir: PathBuf,
    /// Audio of a dictation that isn't saved.
    pub temp: PathBuf,
}

impl SavePaths {
    /// `Dictate/Dictations` in the local data folder of the system and its temporary folder. `None` when the system has
    /// no data folder.
    pub fn system() -> Option<SavePaths> {
        Some(SavePaths {
            dir: dirs::data_local_dir()?.join("Dictate").join("Dictations"),
            temp: std::env::temp_dir(),
        })
    }
}

/// The audio of one dictation, as it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    pub wav: PathBuf,
    /// The file is a saved dictation: it stays when the dictation ends. Otherwise it is temporary.
    pub saved: bool,
}

/// The base name of a saved dictation: `20261109-101530`.
pub fn stamp(now: NaiveDateTime) -> String {
    now.format("%Y%m%d-%H%M%S").to_string()
}

/// Writes the 16 kHz audio of a dictation: into the saved folder when `save`, else (or when that fails) into the
/// temporary one. `id` tells the temporary files of one run apart. A file that could not be written whole is deleted.
pub fn write_take(
    paths: &SavePaths,
    save: bool,
    now: NaiveDateTime,
    id: u64,
    samples_16k: &[f32],
) -> io::Result<Take> {
    if save {
        match saved_path(&paths.dir, &stamp(now))
            .and_then(|path| write_checked(&path, samples_16k, wav::write_16k_mono).map(|()| path))
        {
            Ok(wav) => return Ok(Take { wav, saved: true }),
            // A full or read-only disk must not cost the dictation: the audio is still needed by the speech API.
            Err(e) => log::warn!("Could not save the recording, using a temporary file: {e}"),
        }
    }
    fs::create_dir_all(&paths.temp)?;
    let wav = paths.temp.join(format!(
        "{TEMP_PREFIX}{}-{id}.{TEMP_EXTENSION}",
        std::process::id()
    ));
    write_checked(&wav, samples_16k, wav::write_16k_mono)?;
    Ok(Take { wav, saved: false })
}

/// The file for a saved dictation with the base name `stamp`, in `dir` (made when it is missing). A second one in the
/// same second gets `-2`, a third `-3`, so that none writes over another.
fn saved_path(dir: &Path, stamp: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut name = stamp.to_owned();
    for n in 2.. {
        let wav = dir.join(format!("{name}.wav"));
        // The text of the dictation may be there while its audio is not (the audio was deleted by hand).
        if !wav.exists() && !text_path(&wav).exists() {
            return Ok(wav);
        }
        name = format!("{stamp}-{n}");
    }
    unreachable!("the numbers do not run out")
}

/// Writes the audio with `write`, and deletes the file when that fails: half a file is no recording.
fn write_checked(
    path: &Path,
    samples_16k: &[f32],
    write: impl FnOnce(&Path, &[f32]) -> io::Result<()>,
) -> io::Result<()> {
    write(path, samples_16k).inspect_err(|_| {
        // The file may not even have been made.
        let _ = fs::remove_file(path);
    })
}

/// Writes the text of a saved dictation next to its audio, as UTF-8, when there is any.
pub fn write_text(take: &Take, text: &str) -> io::Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    fs::write(text_path(&take.wav), text)
}

/// The text file that goes with the audio file.
pub fn text_path(wav: &Path) -> PathBuf {
    wav.with_extension("txt")
}

/// Deletes the audio of a dictation that isn't saved. A saved one stays.
pub fn discard_if_temporary(take: &Take) {
    if take.saved {
        return;
    }
    if let Err(e) = fs::remove_file(&take.wav) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("Could not delete the temporary recording: {e}");
        }
    }
}

/// Deletes the temporary audio files that an earlier run left behind. How many there were.
pub fn sweep_temp(temp: &Path) -> usize {
    let Ok(entries) = fs::read_dir(temp) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| is_temporary_audio(&entry.file_name().to_string_lossy()))
        .filter(|entry| fs::remove_file(entry.path()).is_ok())
        .count()
}

/// Whether the name is one of a temporary recording: `dictate-` and the rest, ending in `.wav`.
fn is_temporary_audio(name: &str) -> bool {
    name.strip_prefix(TEMP_PREFIX)
        .and_then(|rest| rest.strip_suffix(TEMP_EXTENSION))
        .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use dictate_services::whisper::local::read_wav_16k_mono;

    fn at(h: u32, m: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 9)
            .unwrap()
            .and_hms_opt(h, m, s)
            .unwrap()
    }

    fn paths(root: &Path) -> SavePaths {
        SavePaths {
            dir: root.join("saved"),
            temp: root.join("temp"),
        }
    }

    fn tone() -> Vec<f32> {
        (0..1600).map(|i| (i as f32 / 100.0).sin() * 0.5).collect()
    }

    #[test]
    fn save_names_and_sweep() {
        // The base name is the local time, to the second, with nothing in it that a file system refuses.
        assert_eq!(stamp(at(10, 15, 30)), "20261109-101530");
        assert_eq!(stamp(at(0, 5, 9)), "20261109-000509");

        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.temp).unwrap();

        // Saved: the audio and the text have one base name in the folder of the dictations, which is made on the way.
        let take = write_take(&paths, true, at(10, 15, 30), 1, &tone()).unwrap();
        assert_eq!(
            take,
            Take {
                wav: paths.dir.join("20261109-101530.wav"),
                saved: true
            }
        );
        write_text(&take, "héllo wörld").unwrap();
        assert_eq!(
            fs::read_to_string(paths.dir.join("20261109-101530.txt")).unwrap(),
            "héllo wörld"
        );
        // The second one in the same second doesn't write over the first.
        let second = write_take(&paths, true, at(10, 15, 30), 2, &tone()).unwrap();
        assert_eq!(second.wav, paths.dir.join("20261109-101530-2.wav"));
        write_text(&second, "again").unwrap();
        assert!(paths.dir.join("20261109-101530-2.txt").is_file());
        assert_eq!(
            fs::read_to_string(paths.dir.join("20261109-101530.txt")).unwrap(),
            "héllo wörld"
        );

        // Not saved: a temporary file, which goes after use. A saved one stays.
        let temporary = write_take(&paths, false, at(10, 15, 31), 3, &tone()).unwrap();
        assert!(!temporary.saved);
        assert_eq!(temporary.wav.parent().unwrap(), paths.temp);
        let name = temporary.wav.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("dictate-") && name.ends_with(".wav"),
            "{name}"
        );
        assert!(temporary.wav.is_file());
        discard_if_temporary(&temporary);
        assert!(!temporary.wav.exists());
        discard_if_temporary(&take);
        assert!(take.wav.is_file(), "a saved dictation is kept");

        // The sweep takes the leftovers of an earlier run, and nothing else.
        fs::write(paths.temp.join("dictate-1234-1.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-old.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-notes.txt"), b"x").unwrap();
        fs::write(paths.temp.join("other.wav"), b"x").unwrap();
        assert_eq!(sweep_temp(&paths.temp), 2);
        assert!(!paths.temp.join("dictate-1234-1.wav").exists());
        assert!(!paths.temp.join("dictate-old.wav").exists());
        assert!(paths.temp.join("dictate-notes.txt").exists());
        assert!(paths.temp.join("other.wav").exists());
        // A folder that isn't there has nothing to sweep.
        assert_eq!(sweep_temp(&root.path().join("missing")), 0);
    }

    #[test]
    fn an_empty_text_is_not_written() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let take = write_take(&paths, true, at(9, 0, 0), 1, &tone()).unwrap();
        write_text(&take, "  \n").unwrap();
        assert!(!text_path(&take.wav).exists());
    }

    #[test]
    fn the_audio_is_a_16_khz_wav() {
        let root = tempfile::tempdir().unwrap();
        let take = write_take(&paths(root.path()), true, at(9, 0, 0), 1, &tone()).unwrap();
        // The header of a WAV file: channels at byte 22, the sample rate at byte 24.
        let bytes = fs::read(&take.wav).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(read_wav_16k_mono(&take.wav).unwrap().len(), tone().len());
    }

    #[test]
    fn a_file_that_fails_midway_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("take.wav");
        // A writer that gets some of the file down and then fails, as a disk that fills up does.
        let result = write_checked(&path, &tone(), |path, _| {
            fs::write(path, b"RIFF half a file")?;
            Err(io::Error::other("the disk is full"))
        });
        assert!(result.is_err());
        assert!(!path.exists(), "the partial file is still there");
    }

    #[test]
    fn a_folder_that_cannot_be_made_falls_back_to_the_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("temp")).unwrap();
        // A file where the folder of the dictations should be.
        fs::write(root.path().join("saved"), b"in the way").unwrap();
        let take = write_take(&paths(root.path()), true, at(9, 0, 0), 7, &tone()).unwrap();
        assert!(!take.saved, "{take:?}");
        assert!(take.wav.starts_with(root.path().join("temp")));
        assert!(take.wav.is_file());
    }

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let root = tempfile::tempdir().unwrap();
        // Neither folder can be written to: the temporary one is a file.
        fs::write(root.path().join("temp"), b"in the way").unwrap();
        fs::write(root.path().join("saved"), b"in the way").unwrap();
        assert!(write_take(&paths(root.path()), true, at(9, 0, 0), 1, &tone()).is_err());
        assert!(write_take(&paths(root.path()), false, at(9, 0, 0), 1, &tone()).is_err());
    }
}
