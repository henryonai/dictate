//! Putting a recording's files together: what `Session::stop` ends with and what the recovery of a recording that a
//! crash left behind does.
//!
//! The folder is read as it is: the stems that are there (`mic.wav`, `system.wav`), the video that is there
//! (`video.mp4` or `video.webm`, or the `.part` of one that was not finished). Nothing is deleted until the sound is
//! mixed: whatever goes wrong before that leaves every file where it is, and `recording.json` with them, so that the
//! next start-up can try again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::Finished;
use crate::media::encode::{publish, remove_part, EncodeError};
use crate::record::marker::MARKER;
use crate::record::mix::mix_stems;
use crate::record::stem::{pad_to, STEM_RATE};
use crate::record::video::{mux_within, probe_seconds, Container};
use crate::record::Track;

/// The file of the stem of each track that has sound.
pub(super) const STEMS: [(Track, &str); 2] =
    [(Track::Mic, "mic.wav"), (Track::System, "system.wav")];

/// The shortest sound a recording has, in seconds: a recording that was stopped as it began has the little it
/// caught followed by silence up to this, so that what follows (the encoder, the transcript) is given a file that is
/// worth the name.
pub(super) const MIN_AUDIO_SECONDS: f64 = 1.0;

/// What the caller knows that the files do not say.
#[derive(Default)]
pub(super) struct Hints {
    /// How long the video is, in seconds, when the caller counted its frames; the file is asked otherwise.
    pub video_seconds: Option<f64>,
    /// How long ffmpeg is given to put the sound under the video, instead of what the length of the recording says.
    pub mux_limit: Option<Duration>,
}

/// Puts the files of the recording in `folder` together: the stems are made as long as the longest of them (and of the
/// video, and no shorter than a second) and mixed into `audio.m4a` (`recording.wav` where there is no encoder for
/// it); a screen recording has the sound put under its video, as `recording.mp4` (`.webm`). What was put together is
/// removed, with `recording.json`; what is the result stays: the sound, and the video. When the video could not be put
/// together with the sound (ffmpeg failed, or is not there), the sound is the `media` (the pipeline decodes the media,
/// and a video with no sound under it has nothing in it to transcribe) and the video is kept beside it as `video.*`, with
/// a note. A video is the `media` only when there is no sound at all (and then it is the recording, even one that ffmpeg
/// could not read).
///
/// Whatever stops it before the sound is mixed (`Err`) leaves every file as it was, `recording.json` among them:
/// nothing that was recorded is lost to it. The exception is a folder that has no sound and no video at all, which has
/// nothing to try again: it is `InvalidData` ("nothing was recorded"), and `recording.json` is removed so that
/// no start-up tries it again.
///
/// A stem that cannot be read (a crash may leave one that is cut inside its header) does not stop that: it is set aside
/// as `<name>.unreadable`, which is not deleted, and the rest is put together, with a note that says so.
///
/// A video that is there (`video.*`, or the `video.*.part` of an encoder that did not end) is used: the part is
/// renamed first. Whoever sweeps the folders for `*.part` files (`meetings::sweep_stale_files` does, at start-up) must
/// do it after this has run.
pub fn finalize(folder: &Path, ffmpeg: Option<&Path>) -> io::Result<Finished> {
    finalize_with(folder, ffmpeg, &Hints::default())
}

/// Finishes the recording that a crash (or a quit that did not wait) left in `folder`: `Some` of what `finalize` makes
/// of it when the folder has `recording.json`, and `None` when it has not. What a crash loses is the end of each
/// source: a stem has all of its audio up to the last second that its header was written for, and the video up to the
/// last keyframe (every 2 s).
///
/// Must run before the start-up sweep of `*.part` files, which would delete the video that this publishes.
pub fn recover(folder: &Path, ffmpeg: Option<&Path>) -> Option<io::Result<Finished>> {
    folder
        .join(MARKER)
        .is_file()
        .then(|| finalize(folder, ffmpeg))
}

pub(super) fn finalize_with(
    folder: &Path,
    ffmpeg: Option<&Path>,
    hints: &Hints,
) -> io::Result<Finished> {
    let mut notes = Vec::new();
    let found = find_video(folder, ffmpeg, hints, &mut notes)?;
    let stems = find_stems(folder, &mut notes);
    // A video that ffmpeg could not read anything from is not put under the sound: it is left out, and kept in the folder.
    let (video, unreadable) = match found {
        Some(video) if !video.readable => (None, Some(video)),
        found => (found, None),
    };

    if stems.is_empty() {
        // What is left to the recording is its video, and that is the recording even when it could not be read: ffmpeg has
        // the same answer for a file with no picture in it, for one it could not open, and for one that it was not given
        // the time to read to its end, and a recording that may be played is not thrown away on that.
        let video = match (video, unreadable) {
            (Some(video), _) => video,
            (None, Some(video)) => {
                notes.push(format!(
                    "{} could not be read (it may be cut off, or have no picture in it), but it is all that was recorded: it is kept as the recording.",
                    name_of(&video.path)
                ));
                video
            }
            (None, None) => {
                remove(&folder.join(MARKER));
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "nothing was recorded: the folder has no sound and no video",
                ));
            }
        };
        notes.push("No audio was recorded.".to_owned());
        remove(&folder.join(MARKER));
        return Ok(Finished {
            media: video.path,
            duration: video.seconds.unwrap_or(0.0),
            notes,
        });
    }
    if let Some(video) = &unreadable {
        notes.push(format!(
            "There is no video in the recording: {} could not be read (it may have no picture in it), so it was left out and kept in the folder.",
            name_of(&video.path)
        ));
    }

    // Every stem as long as the longest of the stems and the video, and no shorter than the least there is.
    let longest = stems.iter().map(|stem| stem.seconds).fold(0.0, f64::max);
    let recorded = longest.max(video.as_ref().and_then(|v| v.seconds).unwrap_or(0.0));
    if recorded < MIN_AUDIO_SECONDS {
        notes.push(
            "Almost nothing was recorded: the sound was lengthened with silence to one second."
                .to_owned(),
        );
    }
    let target = recorded.max(MIN_AUDIO_SECONDS);
    for stem in stems.iter().filter(|stem| stem.seconds < target) {
        pad_to(&stem.path, target)?;
    }
    let paths: Vec<PathBuf> = stems.iter().map(|stem| stem.path.clone()).collect();
    // An error here leaves the stems, the video and the marker for another try.
    let mixed = mix_stems(&paths, folder, ffmpeg, &CancellationToken::new())?;

    // What the pipeline decodes is the media, and a video that is not put under the sound has none: when it is not, the
    // media is the sound, and the video is kept beside it as `video.*`.
    let media = match (video, ffmpeg) {
        (None, _) => mixed.path.clone(),
        (Some(video), None) => {
            notes.push(format!(
                "ffmpeg was not found, so the sound could not be put under the video: the video is kept as {}, and the sound is the recording ({}).",
                name_of(&video.path),
                name_of(&mixed.path)
            ));
            mixed.path.clone()
        }
        (Some(video), Some(ffmpeg)) => {
            let limit = hints.mux_limit.unwrap_or_else(|| mux_limit(mixed.duration));
            match mux_within(
                &video.path,
                &mixed.path,
                folder,
                video.container,
                ffmpeg,
                limit,
            ) {
                Ok(recording) => {
                    remove(&video.path);
                    recording
                }
                Err(e) => {
                    notes.push(format!(
                        "The sound could not be put under the video ({}): the video is kept as {}, and the sound is the recording ({}).",
                        detail(&e),
                        name_of(&video.path),
                        name_of(&mixed.path)
                    ));
                    mixed.path.clone()
                }
            }
        }
    };
    for stem in &stems {
        remove(&stem.path);
    }
    remove(&folder.join(MARKER));
    Ok(Finished {
        media,
        duration: mixed.duration,
        notes,
    })
}

/// How long ffmpeg is given to put the sound of a recording of `seconds` under its video: two minutes, and a twentieth
/// of the recording on top (copying the streams takes a fraction of that; a sound that has to be made into AAC or
/// Opus takes more, and this is for the program that is stuck).
fn mux_limit(seconds: f64) -> Duration {
    Duration::from_secs(120) + Duration::from_secs_f64((seconds / 20.0).max(0.0))
}

/// What an encoder said, without the "Could not start the audio encoder" that `EncodeError`'s `Display` puts in front.
pub(super) fn detail(error: &EncodeError) -> String {
    match error {
        EncodeError::Failed(message) => message.clone(),
        other => other.to_string(),
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// Removes a file of the recording (it may not be there).
fn remove(path: &Path) {
    remove_part(path);
}

/// A stem that has sound.
struct Stem {
    path: PathBuf,
    seconds: f64,
}

/// The stems in `folder`. One with no samples is removed: its track delivered nothing. One that cannot be read at all
/// (what a crash leaves when it comes before the header of the stem was written, or in the middle of it) is no reason to
/// lose the others: it is set aside as `<name>.unreadable`, which is not deleted, and `notes` says so.
fn find_stems(folder: &Path, notes: &mut Vec<String>) -> Vec<Stem> {
    let mut stems = Vec::new();
    for (_, name) in STEMS {
        let path = folder.join(name);
        if !path.is_file() {
            continue;
        }
        let samples = match hound::WavReader::open(&path) {
            Ok(reader) => u64::from(reader.duration()),
            Err(error) => {
                set_aside(&path, &error, notes);
                continue;
            }
        };
        if samples == 0 {
            remove(&path);
            continue;
        }
        stems.push(Stem {
            path,
            seconds: samples as f64 / f64::from(STEM_RATE),
        });
    }
    stems
}

/// Leaves the stem at `path`, which `error` says cannot be read, out of the recording: it is renamed to `<name>.unreadable`
/// (and stays where it is when that name is taken, so that nothing is overwritten).
fn set_aside(path: &Path, error: &hound::Error, notes: &mut Vec<String>) {
    let name = name_of(path);
    let aside = path.with_file_name(format!("{name}.unreadable"));
    let kept = if !aside.exists() && fs::rename(path, &aside).is_ok() {
        format!("kept as {}", name_of(&aside))
    } else {
        "kept where it is".to_owned()
    };
    notes.push(format!(
        "{name} could not be read ({error}), so it was left out of the recording and is {kept}."
    ));
}

/// The video of a recording.
struct Video {
    path: PathBuf,
    container: Container,
    /// How long it is, when that is known.
    seconds: Option<f64>,
    /// Whether it can be used with the sound. A video that was counted is believed; one that was not, and that ffmpeg can
    /// read nothing from, is not.
    readable: bool,
}

/// The video in `folder`: `video.<ext>`, which is made out of a `.part` that an encoder left, when there is one and
/// there is no finished video. `None` when there is none. One that ffmpeg can read no length from is there all the same,
/// as a video that is not `readable` (an encoder that was given no frame writes a file that has only a header, which
/// `mux` cannot read): it is left where it is.
fn find_video(
    folder: &Path,
    ffmpeg: Option<&Path>,
    hints: &Hints,
    notes: &mut Vec<String>,
) -> io::Result<Option<Video>> {
    let mut published = None;
    let mut parts: Vec<(PathBuf, Container, u64)> = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        for container in [Container::Mp4, Container::WebM] {
            let file = format!("video.{}", container.extension());
            if name == file {
                published = Some((entry.path(), container));
            } else if name.starts_with(&format!("{file}.")) && name.ends_with(".part") {
                let size = entry.metadata().map_or(0, |meta| meta.len());
                parts.push((entry.path(), container, size));
            }
        }
    }
    // The biggest part is the one with most in it.
    parts.sort_by_key(|(_, _, size)| std::cmp::Reverse(*size));
    let mut parts = parts.into_iter();
    let (path, container) = match published {
        Some(found) => found,
        None => match parts.next() {
            Some((part, container, _)) => {
                let out = folder.join(format!("video.{}", container.extension()));
                publish(&part, &out)?;
                notes.push("The video was cut off: what is in it was kept.".to_owned());
                (out, container)
            }
            None => return Ok(None),
        },
    };
    // What is left of other parts is of no use.
    for (part, _, _) in parts {
        remove(&part);
    }

    let seconds = match hints.video_seconds {
        Some(seconds) => Some(seconds),
        None => ffmpeg.and_then(|ffmpeg| probe_seconds(&path, ffmpeg)),
    };
    // It is not removed, whatever ffmpeg says: what may be a video is not thrown away on that.
    let readable = hints.video_seconds.is_some()
        || ffmpeg.is_none()
        || seconds.is_some_and(|seconds| seconds > 0.0);
    Ok(Some(Video {
        path,
        container,
        seconds,
        readable,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decode::MonoDecoder;
    use crate::media::test_support::{read_all, real_ffmpeg};
    use crate::record::marker::{Marker, MARKER};
    use crate::record::quality::Quality;
    use crate::record::test_support::{cut_off_video, encode_video, tone_stem, video_seconds};
    use crate::record::{Mode, RecordOptions};
    use std::fs;
    #[cfg(unix)]
    use std::path::PathBuf;

    fn write_marker(folder: &Path) {
        Marker {
            version: 1,
            options: RecordOptions {
                mode: Mode::Screen { display: None },
                quality: Quality::Standard,
                mic: true,
                system_audio: true,
            },
            codec: None,
            container: None,
            started: 0.0,
        }
        .write(folder)
        .unwrap();
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// How long the sound of `path` is, in seconds.
    fn audio_seconds(path: &Path, ffmpeg: &Path) -> f64 {
        let mut decoder = MonoDecoder::open(path, Some(ffmpeg)).unwrap();
        read_all(&mut decoder).len() as f64 / f64::from(decoder.rate())
    }

    #[test]
    fn stems_of_different_lengths_become_one_sound_as_long_as_the_longest() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);
        tone_stem(dir.path(), "system.wav", 880.0, 1.0);
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        let name = finished.media.file_name().unwrap().to_string_lossy();
        assert!(name == "audio.m4a" || name == "recording.wav", "{name}");
        assert_eq!(finished.media.parent(), Some(dir.path()));
        assert!(
            (finished.duration - 2.0).abs() < 0.05,
            "{}",
            finished.duration
        );
        assert!((audio_seconds(&finished.media, &ffmpeg) - 2.0).abs() < 0.1);
        assert!(finished.notes.is_empty(), "{:?}", finished.notes);
        // The stems and the marker are gone; the sound is what is left.
        assert_eq!(names(dir.path()), [name.to_string()]);
    }

    #[test]
    fn a_recording_of_almost_nothing_is_a_second_of_silence() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 0.01);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert!(
            (finished.duration - 1.0).abs() < 0.05,
            "{}",
            finished.duration
        );
        assert!((audio_seconds(&finished.media, &ffmpeg) - 1.0).abs() < 0.1);
        assert_eq!(finished.notes.len(), 1, "{:?}", finished.notes);
        assert!(
            finished.notes[0].contains("silence"),
            "{:?}",
            finished.notes
        );
    }

    #[test]
    fn a_stem_with_no_samples_does_not_count() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 1.5);
        tone_stem(dir.path(), "system.wav", 440.0, 0.0);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert!(
            (finished.duration - 1.5).abs() < 0.05,
            "{}",
            finished.duration
        );
        assert_eq!(names(dir.path()).len(), 1, "{:?}", names(dir.path()));
    }

    #[test]
    fn a_folder_with_nothing_in_it_is_an_error_that_says_so_and_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path());

        let error = finalize(dir.path(), None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("nothing was recorded"),
            "{error}"
        );
        // There is nothing to try again, so the marker does not stay to make every start-up try.
        assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    }

    #[test]
    fn a_video_and_no_sound_is_the_media_itself_with_a_note() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = encode_video(dir.path(), &ffmpeg, 2);
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert_eq!(finished.media, video);
        assert!(
            (finished.duration - 2.0).abs() < 0.2,
            "{}",
            finished.duration
        );
        assert_eq!(finished.notes, ["No audio was recorded."]);
        assert_eq!(
            names(dir.path()),
            [video.file_name().unwrap().to_string_lossy()]
        );
    }

    #[test]
    fn the_sound_is_put_under_the_video() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = encode_video(dir.path(), &ffmpeg, 2);
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        let extension = video.extension().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            finished.media,
            dir.path().join(format!("recording.{extension}"))
        );
        assert!(
            (finished.duration - 2.0).abs() < 0.1,
            "{}",
            finished.duration
        );
        let seconds = video_seconds(&finished.media, &ffmpeg);
        assert!((seconds - 2.0).abs() < 0.3, "{seconds} s of video");
        assert!(finished.notes.is_empty(), "{:?}", finished.notes);
        // What was put together is gone: the video alone, the stems and the marker. The sound is kept.
        let left = names(dir.path());
        assert!(!left.contains(&video.file_name().unwrap().to_string_lossy().into_owned()));
        assert!(!left.contains(&"mic.wav".to_owned()) && !left.contains(&MARKER.to_owned()));
        assert!(
            left.contains(&"audio.m4a".to_owned()) || left.contains(&"recording.wav".to_owned()),
            "{left:?}"
        );
    }

    #[test]
    fn without_ffmpeg_the_sound_is_the_media_and_the_video_is_kept_beside_it() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = encode_video(dir.path(), &ffmpeg, 2);
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);

        // No AAC encoder either on Linux: the sound is a `recording.wav` there.
        let finished = finalize(dir.path(), None).unwrap();

        // The pipeline decodes the media: a video that has no sound under it would give it nothing to transcribe.
        assert!(
            is_sound_file(&name_of(&finished.media)),
            "{:?}",
            finished.media
        );
        assert!(finished.media.is_file());
        assert!(video.is_file(), "the video is kept");
        assert_eq!(finished.notes.len(), 1, "{:?}", finished.notes);
        assert!(finished.notes[0].contains("ffmpeg"), "{:?}", finished.notes);
        // Linux has VP9 first, which is a `video.webm`: the note names the file that there is.
        assert!(
            finished.notes[0].contains(&name_of(&video)),
            "{:?}",
            finished.notes
        );
        let left = names(dir.path());
        assert!(!left.contains(&"mic.wav".to_owned()), "{left:?}");
        assert!(!left.contains(&MARKER.to_owned()), "{left:?}");
    }

    #[test]
    fn a_video_part_that_was_left_is_published_and_noted() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let (part, readable) = cut_off_video(dir.path(), &ffmpeg);
        assert!(readable >= 1.9, "{readable} s of the cut off video");
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert!(finished
            .media
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("recording."));
        assert!(!part.exists());
        assert!(
            finished.notes.iter().any(|n| n.contains("cut off")),
            "{:?}",
            finished.notes
        );
        // The video is as long as what could be read of it, and the sound is made as long as the longer of the two: a
        // video that is cut off is not lengthened, and a sound that is shorter than it is.
        assert!(
            (finished.duration - readable.max(2.0)).abs() < 0.3,
            "{} s for {readable} s of video and 2 s of sound",
            finished.duration
        );
        let video = video_seconds(&finished.media, &ffmpeg);
        assert!(
            (video - readable).abs() < 0.3,
            "{video} s of video, {readable} s were readable"
        );
    }

    #[test]
    fn a_video_that_is_longer_than_the_sound_lengthens_the_sound() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        encode_video(dir.path(), &ffmpeg, 3);
        tone_stem(dir.path(), "mic.wav", 440.0, 1.0);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert!(
            (finished.duration - 3.0).abs() < 0.2,
            "{}",
            finished.duration
        );
    }

    #[test]
    fn a_stem_that_is_not_a_stem_stops_it_and_nothing_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        tone_stem(dir.path(), "system.wav", 440.0, 1.0);
        crate::audio::wav::write_16k_mono(&dir.path().join("mic.wav"), &[0.0; 1600]).unwrap();
        write_marker(dir.path());
        let before = names(dir.path());

        let error = finalize(dir.path(), None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(names(dir.path()), before);
    }

    /// What a stem can be when a crash found it: nothing at all, the start of a header, or not a WAV file.
    fn broken_stems() -> Vec<(&'static str, Vec<u8>)> {
        let dir = tempfile::tempdir().unwrap();
        let whole = fs::read(tone_stem(dir.path(), "whole.wav", 440.0, 0.5)).unwrap();
        vec![
            ("empty", Vec::new()),
            ("cut header", whole[..20].to_vec()),
            ("not a wav", b"this is not a sound".to_vec()),
        ]
    }

    fn is_sound_file(name: &str) -> bool {
        name == "audio.m4a" || name == "recording.wav"
    }

    #[test]
    fn a_stem_that_cannot_be_opened_is_set_aside_and_the_other_is_recovered() {
        for (what, bytes) in broken_stems() {
            let dir = tempfile::tempdir().unwrap();
            tone_stem(dir.path(), "mic.wav", 440.0, 1.5);
            fs::write(dir.path().join("system.wav"), &bytes).unwrap();
            write_marker(dir.path());

            let finished = finalize(dir.path(), None).unwrap_or_else(|e| panic!("{what}: {e}"));

            assert!(
                (finished.duration - 1.5).abs() < 0.05,
                "{what}: {}",
                finished.duration
            );
            assert!(is_sound_file(&names(dir.path())[0]), "{what}");
            assert_eq!(finished.media.parent(), Some(dir.path()), "{what}");
            // Said, so that the person can find it; and kept, since what is in it may be of use to someone.
            assert!(
                finished.notes.iter().any(|n| n.contains("system.wav")),
                "{what}: {:?}",
                finished.notes
            );
            assert_eq!(
                fs::read(dir.path().join("system.wav.unreadable")).unwrap(),
                bytes,
                "{what}"
            );
            // What was put together is gone, and the marker with it: no start-up tries this folder again.
            let left = names(dir.path());
            assert_eq!(left.len(), 2, "{what}: {left:?}");
            assert!(
                !left.contains(&MARKER.to_owned()) && !left.contains(&"mic.wav".to_owned()),
                "{what}: {left:?}"
            );
        }
    }

    #[test]
    fn stems_that_cannot_be_opened_are_kept_and_it_is_nothing_that_was_recorded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mic.wav"), b"").unwrap();
        fs::write(dir.path().join("system.wav"), b"RIFF").unwrap();
        write_marker(dir.path());

        let error = finalize(dir.path(), None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // They are not deleted: whoever cleans up the folder sees that it holds something.
        assert_eq!(
            names(dir.path()),
            ["mic.wav.unreadable", "system.wav.unreadable"]
        );
    }

    #[test]
    fn a_stem_that_cannot_be_opened_does_not_take_the_video_from_the_recording() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = encode_video(dir.path(), &ffmpeg, 2);
        fs::write(dir.path().join("system.wav"), b"").unwrap();
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert_eq!(finished.media, video);
        assert!(video.exists());
        assert!(
            finished.notes.iter().any(|n| n.contains("system.wav")),
            "{:?}",
            finished.notes
        );
    }

    /// A stand-in for ffmpeg that does what `on_mux` says when it is asked to put the sound under a video, and is
    /// the real one otherwise.
    #[cfg(unix)]
    fn ffmpeg_that_fails_to_mux(dir: &Path, real: &Path, on_mux: &str) -> PathBuf {
        crate::media::test_support::script(
            dir,
            "fake-ffmpeg",
            &format!(
                "case \"$*\" in *'-map 0:v -map 1:a'*) {on_mux};; esac\nexec '{}' \"$@\"",
                real.display()
            ),
            "",
        )
    }

    #[cfg(unix)]
    #[test]
    fn a_mux_that_fails_makes_the_sound_the_media_and_keeps_the_video_beside_it() {
        let Some(real) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let ffmpeg =
            ffmpeg_that_fails_to_mux(tools.path(), &real, "echo 'Invalid data' >&2; exit 1");
        let video = encode_video(dir.path(), &real, 2);
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        // The pipeline decodes the media, and a video that is not under the sound has none: the sound is what it is given.
        assert!(
            is_sound_file(&name_of(&finished.media)),
            "{:?}",
            finished.media
        );
        assert_eq!(finished.media.parent(), Some(dir.path()));
        assert!(
            (finished.duration - 2.0).abs() < 0.1,
            "{}",
            finished.duration
        );
        assert!(video.is_file(), "the video is kept as {}", name_of(&video));
        assert_eq!(finished.notes.len(), 1, "{:?}", finished.notes);
        // The note says what ffmpeg said, not that the audio encoder could not start, and where the video is.
        assert!(
            finished.notes[0].contains("Invalid data"),
            "{:?}",
            finished.notes
        );
        assert!(
            !finished.notes[0].contains("audio encoder"),
            "{:?}",
            finished.notes
        );
        assert!(
            finished.notes[0].contains(&name_of(&video)),
            "{:?}",
            finished.notes
        );
        let left = names(dir.path());
        assert!(
            !left.contains(&"mic.wav".to_owned()) && !left.contains(&MARKER.to_owned()),
            "{left:?}"
        );
        assert!(
            !left.iter().any(
                |name| (name.starts_with("recording.") && !is_sound_file(name))
                    || name.ends_with(".part")
            ),
            "{left:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_mux_that_hangs_is_given_up_and_the_sound_is_the_media() {
        let Some(real) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let ffmpeg = ffmpeg_that_fails_to_mux(tools.path(), &real, "sleep 60");
        let video = encode_video(dir.path(), &real, 2);
        tone_stem(dir.path(), "mic.wav", 440.0, 2.0);
        let hints = Hints {
            video_seconds: Some(2.0),
            mux_limit: Some(Duration::from_millis(500)),
        };
        let started = std::time::Instant::now();

        let finished = finalize_with(dir.path(), Some(&ffmpeg), &hints).unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        assert!(
            is_sound_file(&name_of(&finished.media)),
            "{:?}",
            finished.media
        );
        assert!(video.is_file(), "the video is kept");
        assert!(
            finished.notes[0].contains("did not finish"),
            "{:?}",
            finished.notes
        );
    }

    #[test]
    fn a_video_with_no_frames_is_left_out_and_kept() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        // What an encoder that was given no frame writes: a header and nothing to play.
        let codec = crate::record::test_support::codec_of(&ffmpeg);
        let encoder = crate::record::video::VideoEncoder::start(
            dir.path(),
            crate::record::test_support::SIZE,
            &Quality::Standard.spec(),
            &codec,
            &ffmpeg,
        )
        .unwrap();
        let empty = encoder.finish().unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 1.5);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        let name = finished.media.file_name().unwrap().to_string_lossy();
        assert!(name == "audio.m4a" || name == "recording.wav", "{name}");
        // It is not thrown away: ffmpeg cannot tell a file with no picture in it from one it could not open.
        assert!(empty.exists());
        assert!(
            finished.notes.iter().any(|n| n.contains("no video")),
            "{:?}",
            finished.notes
        );
    }

    /// An ffmpeg that cannot read what it is given: the answer of the real one for a file it cannot open, and for one that it
    /// is not given the time to read to its end (a long recording on a slow disk).
    fn ffmpeg_that_reads_nothing(dir: &Path) -> PathBuf {
        crate::media::test_support::script(dir, "fake-ffmpeg", "exit 1", "exit /b 1")
    }

    #[test]
    fn a_video_that_cannot_be_read_with_no_sound_is_the_recording_and_not_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let ffmpeg = ffmpeg_that_reads_nothing(tools.path());
        let video = dir.path().join("video.mp4");
        fs::write(&video, vec![7_u8; 4096]).unwrap();
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert_eq!(finished.media, video);
        assert!(video.is_file());
        assert!(
            finished.notes.iter().any(|n| n.contains("video.mp4")),
            "{:?}",
            finished.notes
        );
        assert!(
            finished.notes.iter().any(|n| n == "No audio was recorded."),
            "{:?}",
            finished.notes
        );
        // It is a recording now: nothing is left to try again.
        assert_eq!(names(dir.path()), ["video.mp4"]);
    }

    #[test]
    fn a_video_part_that_cannot_be_read_with_no_sound_is_published_and_kept_as_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let ffmpeg = ffmpeg_that_reads_nothing(tools.path());
        fs::write(dir.path().join("video.webm.4242.part"), vec![7_u8; 4096]).unwrap();
        write_marker(dir.path());

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert_eq!(finished.media, dir.path().join("video.webm"));
        assert!(finished.media.is_file());
        assert_eq!(names(dir.path()), ["video.webm"]);
    }

    #[test]
    fn a_video_that_cannot_be_read_is_still_left_out_when_there_is_sound() {
        let dir = tempfile::tempdir().unwrap();
        // An ffmpeg that is not there: it can read nothing, and the sound is mixed without it on every system (the system's
        // encoder on macOS and Windows, a `recording.wav` on Linux). A stand-in that starts and then fails would be what the
        // mix is written through on Linux, which has no AAC encoder of its own.
        let ffmpeg = dir.path().join("no-ffmpeg-here");
        let video = dir.path().join("video.mp4");
        fs::write(&video, vec![7_u8; 4096]).unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 1.5);

        let finished = finalize(dir.path(), Some(&ffmpeg)).unwrap();

        assert!(is_sound_file(&name_of(&finished.media)));
        assert!(video.is_file(), "it is kept, whatever it is");
        assert!(
            finished.notes.iter().any(|n| n.contains("video.mp4")),
            "{:?}",
            finished.notes
        );
    }

    #[test]
    fn ffmpeg_is_given_longer_for_a_longer_recording() {
        assert_eq!(mux_limit(0.0), Duration::from_secs(120));
        assert_eq!(mux_limit(3600.0), Duration::from_secs(120 + 180));
        assert!(mux_limit(36_000.0) > mux_limit(3600.0));
    }

    #[test]
    fn an_encoder_error_is_told_by_what_it_said() {
        let failed = EncodeError::Failed("Invalid data found".into());
        assert_eq!(detail(&failed), "Invalid data found");
        assert!(detail(&EncodeError::Unavailable).contains("no AAC encoder"));
    }

    #[test]
    fn recover_finishes_what_is_there_when_the_marker_is() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        tone_stem(dir.path(), "mic.wav", 440.0, 1.5);
        write_marker(dir.path());

        let finished = recover(dir.path(), Some(&ffmpeg))
            .expect("a marker")
            .unwrap();

        assert!((finished.duration - 1.5).abs() < 0.05);
        assert!(!dir.path().join(MARKER).exists());
    }

    #[test]
    fn recover_finishes_a_marker_that_cannot_be_read_too() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(MARKER), "{ torn").unwrap();

        let result = recover(dir.path(), None).expect("a marker");

        // Nothing is in the folder, but it was tried, and what there is to say is what is wrong with the folder.
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
