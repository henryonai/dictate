//! Where a dictation's audio and text go: the saved ones in `Dictate/Dictations`, and the temporary audio of a
//! dictation that isn't saved, which is deleted after use and swept at launch.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use dictate_services::audio::wav;

/// The prefix of the temporary files of a dictation that isn't saved: `dictate-<stamp>.wav`.
const TEMP_PREFIX: &str = "dictate-";

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

/// The audio of one dictation, as it was written. A temporary file goes when the `Take` does, whichever way the dictation
/// ends (an early return, a bug that was caught); a saved one stays. It is not `Clone`: a copy that was dropped would
/// delete the file under the one that is still in use.
#[derive(Debug, PartialEq, Eq)]
pub struct Take {
    pub wav: PathBuf,
    /// The file is a saved dictation: it stays when the dictation ends. Otherwise it is temporary.
    pub saved: bool,
}

impl Drop for Take {
    fn drop(&mut self) {
        discard_if_temporary(self);
    }
}

/// The base name of a saved dictation: `20261109-101530`.
pub fn stamp(now: NaiveDateTime) -> String {
    now.format("%Y%m%d-%H%M%S").to_string()
}

/// Writes the 16 kHz audio of a dictation, where it is wanted: into the saved folder when `save`, and into the temporary one
/// as `dictate-<stamp>.wav` when a speech API has to `upload` it and it was not saved (or could not be). `None` when
/// nobody needs a file: local speech recognition works on what is in memory, and a recording nobody asked for is not left
/// in a folder that other users of the machine can read. A second take in the same second gets `-2` after the stamp. A
/// file that could not be written whole is deleted.
pub fn write_take(
    paths: &SavePaths,
    save: bool,
    upload: bool,
    now: NaiveDateTime,
    samples_16k: &[f32],
) -> io::Result<Option<Take>> {
    if save {
        match saved_path(&paths.dir, &stamp(now))
            .and_then(|path| write_checked(&path, samples_16k, wav::write_16k_mono).map(|()| path))
        {
            Ok(wav) => return Ok(Some(Take { wav, saved: true })),
            // A full or read-only disk must not cost the dictation: the audio is still needed by the speech API.
            Err(e) => log::warn!("Could not save the recording: {e}"),
        }
    }
    if !upload {
        return Ok(None);
    }
    fs::create_dir_all(&paths.temp)?;
    let wav = temp_path(&paths.temp, &format!("{TEMP_PREFIX}{}", stamp(now)))?;
    write_checked(&wav, samples_16k, wav::write_16k_mono)?;
    Ok(Some(Take { wav, saved: false }))
}

/// A file `name.wav` in `dir` that is not there yet, or `name-2.wav`, `name-3.wav`; the name is taken by making the file, so that
/// two dictations never share one. Only its owner can read it (on Unix): the temporary folder of Linux is shared.
fn temp_path(dir: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 1.. {
        let wav = if n == 1 {
            dir.join(format!("{name}.wav"))
        } else {
            dir.join(format!("{name}-{n}.wav"))
        };
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&wav) {
            Ok(_) => return Ok(wav),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!("the numbers do not run out")
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

/// Writes the text of a saved dictation next to its audio, as UTF-8, when there is any. A dictation that isn't saved has no text
/// file: it would be a transcript left in the temporary folder.
pub fn write_text(take: &Take, text: &str) -> io::Result<()> {
    if !take.saved || text.trim().is_empty() {
        return Ok(());
    }
    fs::write(text_path(&take.wav), text)
}

/// The text file that goes with the audio file.
pub fn text_path(wav: &Path) -> PathBuf {
    wav.with_extension("txt")
}

/// Deletes the audio of a dictation that isn't saved, and any text beside it. A saved one stays.
pub fn discard_if_temporary(take: &Take) {
    if take.saved {
        return;
    }
    for file in [&take.wav, &text_path(&take.wav)] {
        if let Err(e) = fs::remove_file(file) {
            if e.kind() != io::ErrorKind::NotFound {
                log::warn!("Could not delete a temporary file of a dictation: {e}");
            }
        }
    }
}

/// Deletes the temporary files (audio and text) that an earlier run left behind. How many there were.
pub fn sweep_temp(temp: &Path) -> usize {
    let Ok(entries) = fs::read_dir(temp) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| is_temporary_file(&entry.file_name().to_string_lossy()))
        .filter(|entry| fs::remove_file(entry.path()).is_ok())
        .count()
}

/// Whether the name is one of a dictation's temporary files: `dictate-`, then only digits and dashes (the stamp, and a number
/// after it), ending in `.wav` or `.txt`. Nothing else of the shared temporary folder is touched.
fn is_temporary_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(TEMP_PREFIX) else {
        return false;
    };
    let Some(base) = rest
        .strip_suffix(".wav")
        .or_else(|| rest.strip_suffix(".txt"))
    else {
        return false;
    };
    !base.is_empty() && base.chars().all(|c| c.is_ascii_digit() || c == '-')
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

    /// The audio of a take that the person wants kept (`save`), or that a speech API has to hear (`upload`).
    fn take_of(paths: &SavePaths, save: bool, upload: bool, now: NaiveDateTime) -> Take {
        write_take(paths, save, upload, now, &tone())
            .unwrap()
            .expect("a take was wanted")
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
        let take = take_of(&paths, true, false, at(10, 15, 30));
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
        let second = take_of(&paths, true, false, at(10, 15, 30));
        assert_eq!(second.wav, paths.dir.join("20261109-101530-2.wav"));
        write_text(&second, "again").unwrap();
        assert!(paths.dir.join("20261109-101530-2.txt").is_file());
        assert_eq!(
            fs::read_to_string(paths.dir.join("20261109-101530.txt")).unwrap(),
            "héllo wörld"
        );

        // Not saved: a temporary file named by the time, which goes after use. A saved one stays.
        let temporary = take_of(&paths, false, true, at(10, 15, 31));
        assert!(!temporary.saved);
        assert_eq!(
            temporary.wav,
            paths.temp.join("dictate-20261109-101531.wav")
        );
        assert!(temporary.wav.is_file());
        // Two in the same second don't write over each other.
        let twin = take_of(&paths, false, true, at(10, 15, 31));
        assert_eq!(twin.wav, paths.temp.join("dictate-20261109-101531-2.wav"));
        // The text of a dictation that isn't saved is not written anywhere: it would be a transcript left in the temporary folder.
        write_text(&temporary, "private words").unwrap();
        assert_eq!(
            fs::read_dir(&paths.temp).unwrap().count(),
            2,
            "a text file was written for a recording that is not saved"
        );
        // Whatever text a temporary recording has beside it goes with it.
        fs::write(text_path(&temporary.wav), "stray").unwrap();
        discard_if_temporary(&temporary);
        assert!(!temporary.wav.exists());
        assert!(!text_path(&temporary.wav).exists());
        discard_if_temporary(&twin);
        discard_if_temporary(&take);
        assert!(take.wav.is_file(), "a saved dictation is kept");
        assert!(paths.dir.join("20261109-101530.txt").is_file());

        // The sweep takes the leftovers of an earlier run, audio and text, and nothing else.
        fs::write(paths.temp.join("dictate-20261109-101532.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-20261109-101532.txt"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-20261109-101532-2.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-1234-1.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-old.wav"), b"x").unwrap();
        fs::write(paths.temp.join("dictate-notes.txt"), b"x").unwrap();
        fs::write(paths.temp.join("other.wav"), b"x").unwrap();
        assert_eq!(sweep_temp(&paths.temp), 4);
        assert!(!paths.temp.join("dictate-20261109-101532.wav").exists());
        assert!(!paths.temp.join("dictate-20261109-101532.txt").exists());
        assert!(!paths.temp.join("dictate-1234-1.wav").exists());
        assert!(paths.temp.join("dictate-old.wav").exists());
        assert!(paths.temp.join("dictate-notes.txt").exists());
        assert!(paths.temp.join("other.wav").exists());
        // A folder that isn't there has nothing to sweep.
        assert_eq!(sweep_temp(&root.path().join("missing")), 0);
    }

    #[test]
    fn an_empty_text_is_not_written() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let take = take_of(&paths, true, false, at(9, 0, 0));
        write_text(&take, "  \n").unwrap();
        assert!(!text_path(&take.wav).exists());
    }

    #[test]
    fn the_audio_is_a_16_khz_wav() {
        let root = tempfile::tempdir().unwrap();
        let take = take_of(&paths(root.path()), true, false, at(9, 0, 0));
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
        let take = take_of(&paths(root.path()), true, true, at(9, 0, 0));
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
        assert!(write_take(&paths(root.path()), true, true, at(9, 0, 0), &tone()).is_err());
        assert!(write_take(&paths(root.path()), false, true, at(9, 0, 0), &tone()).is_err());
    }

    // ---- audio that nobody asked for is not written

    #[test]
    fn a_take_that_is_neither_kept_nor_heard_by_an_api_is_written_nowhere() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::create_dir_all(&paths.temp).unwrap();

        let take = write_take(&paths, false, false, at(9, 0, 0), &tone()).unwrap();

        assert_eq!(take, None);
        assert_eq!(fs::read_dir(&paths.temp).unwrap().count(), 0);
        assert!(!paths.dir.exists());
    }

    #[test]
    fn a_saved_take_that_cannot_be_written_goes_to_a_temporary_file_only_for_an_api_to_hear() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("temp")).unwrap();
        // A file where the folder of the dictations should be.
        fs::write(root.path().join("saved"), b"in the way").unwrap();
        let paths = paths(root.path());

        // Nothing needs the audio when it can't be kept: no file for no one.
        assert_eq!(
            write_take(&paths, true, false, at(9, 0, 0), &tone()).unwrap(),
            None
        );
        assert_eq!(fs::read_dir(&paths.temp).unwrap().count(), 0);
    }

    #[test]
    fn a_temporary_take_is_deleted_when_it_is_dropped_and_a_saved_one_is_not() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let temporary = take_of(&paths, false, true, at(9, 0, 0));
        let saved = take_of(&paths, true, false, at(9, 0, 1));
        let (temporary_wav, saved_wav) = (temporary.wav.clone(), saved.wav.clone());
        assert!(temporary_wav.is_file());

        drop(temporary);
        drop(saved);

        assert!(
            !temporary_wav.exists(),
            "the temporary audio was left behind"
        );
        assert!(saved_wav.is_file(), "a saved dictation is kept");
    }

    #[test]
    fn dropping_a_temporary_take_whose_file_is_gone_is_fine() {
        let root = tempfile::tempdir().unwrap();
        let take = take_of(&paths(root.path()), false, true, at(9, 0, 0));
        fs::remove_file(&take.wav).unwrap();
        // The file went already (the controller discards it when it is done): nothing is left to do, and nothing panics.
        discard_if_temporary(&take);
        drop(take);
    }

    #[cfg(unix)]
    #[test]
    fn a_temporary_take_can_only_be_read_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let take = take_of(&paths(root.path()), false, true, at(9, 0, 0));
        let mode = fs::metadata(&take.wav).unwrap().permissions().mode();
        // The temporary folder of Linux is shared by all the users of the machine.
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
