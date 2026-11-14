//! `scripts/fetch-sherpa-onnx-no-tts.sh`, which CI and a person building on Linux run to get the sherpa-onnx library that has no
//! speech synthesis in it (and so no espeak-ng, which is GPL-3.0). What it must not do is use an archive that is not the pinned
//! one: it checks the SHA-256 before anything is unpacked, and a failed run leaves nothing that a later run (or a CI cache) would
//! take for a finished one.
//!
//! The script takes `SHERPA_ONNX_NO_TTS_URL` and `SHERPA_ONNX_NO_TTS_SHA256` in place of the real release and its hash, so that
//! these tests need no network: a `file://` archive made here. It is for bash, which CI runs on every system.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fetch-sherpa-onnx-no-tts.sh")
}

/// A `.tar.bz2` of a folder named like the release's (`top`) that has `lib/libsherpa-onnx-c-api.so` in it, and its SHA-256.
fn archive(dir: &Path) -> (PathBuf, String) {
    let top = dir.join("sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib");
    fs::create_dir_all(top.join("lib")).unwrap();
    fs::write(
        top.join("lib/libsherpa-onnx-c-api.so"),
        b"not really a library",
    )
    .unwrap();
    fs::write(top.join("lib/libonnxruntime.so"), b"nor this one").unwrap();
    let path = dir.join("archive.tar.bz2");
    let made = Command::new("tar")
        .arg("-cjf")
        .arg(&path)
        .arg("-C")
        .arg(dir)
        .arg(top.file_name().unwrap())
        .status()
        .expect("tar runs");
    assert!(made.success());
    let hash = format!("{:x}", Sha256::digest(fs::read(&path).unwrap()));
    (path, hash)
}

fn run(url: &Path, sha256: &str, dest: &Path) -> Output {
    Command::new("bash")
        .arg(script())
        .arg(dest)
        .env(
            "SHERPA_ONNX_NO_TTS_URL",
            format!("file://{}", url.display()),
        )
        .env("SHERPA_ONNX_NO_TTS_SHA256", sha256)
        .output()
        .expect("bash runs the script")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_pinned_archive_is_unpacked_and_its_lib_folder_is_printed() {
    let work = tempfile::tempdir().unwrap();
    let (archive, hash) = archive(work.path());
    let dest = work.path().join("target").join("sherpa-no-tts");

    let output = run(&archive, &hash, &dest);

    assert!(output.status.success(), "{}", stderr(&output));
    // Only the folder for SHERPA_ONNX_LIB_DIR is on stdout: CI appends it to the environment of the job as it is.
    let printed = PathBuf::from(stdout(&output).trim_end());
    assert!(printed.is_absolute(), "{printed:?}");
    let lib = dest.join("lib");
    // The same folder, whatever way a link in the path (the temp folder of macOS has one) is spelled.
    assert_eq!(printed.canonicalize().unwrap(), lib.canonicalize().unwrap());
    assert_eq!(stdout(&output).lines().count(), 1, "{}", stdout(&output));
    // The folder inside the archive is stripped: `lib` is directly in the destination.
    assert!(lib.join("libsherpa-onnx-c-api.so").is_file());
    assert!(lib.join("libonnxruntime.so").is_file());
}

#[test]
fn an_archive_that_is_not_the_pinned_one_stops_before_it_is_unpacked() {
    let work = tempfile::tempdir().unwrap();
    let (archive, hash) = archive(work.path());
    let other = format!("{:x}", Sha256::digest(b"another archive"));
    assert_ne!(hash, other);
    let dest = work.path().join("target").join("sherpa-no-tts");

    let output = run(&archive, &other, &dest);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    // It says what it got and what was pinned, so that a change of the release asset can be told from a bad connection.
    assert!(stderr(&output).contains(&hash), "{}", stderr(&output));
    assert!(stderr(&output).contains(&other), "{}", stderr(&output));
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    assert!(
        !dest.exists(),
        "nothing was unpacked: {:?}",
        fs::read_dir(work.path().join("target")).map(|d| d.count())
    );
}

#[test]
fn a_failed_run_leaves_no_folder_that_a_later_run_would_take_for_a_finished_one() {
    // A download that is not an archive, with the hash it really has: the hash is right, the unpacking fails.
    let work = tempfile::tempdir().unwrap();
    let garbage = work.path().join("garbage.tar.bz2");
    fs::write(&garbage, b"this is not a tar file").unwrap();
    let hash = format!("{:x}", Sha256::digest(fs::read(&garbage).unwrap()));
    let dest = work.path().join("target").join("sherpa-no-tts");

    let output = run(&garbage, &hash, &dest);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(!dest.join("lib").exists());
    assert!(!dest.exists());
    // No temporary folder is left next to it either.
    let left: Vec<_> = fs::read_dir(work.path().join("target"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_download_that_fails_is_an_error_and_leaves_nothing() {
    let work = tempfile::tempdir().unwrap();
    let dest = work.path().join("target").join("sherpa-no-tts");

    let output = run(&work.path().join("no-such-archive"), &"0".repeat(64), &dest);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(!dest.exists());
}

#[test]
fn a_folder_that_is_there_already_is_used_without_a_download() {
    // What a cache restore gives CI: the unpacked `lib` folder. The "archive" is not there, so a download would fail.
    let work = tempfile::tempdir().unwrap();
    let dest = work.path().join("target").join("sherpa-no-tts");
    fs::create_dir_all(dest.join("lib")).unwrap();

    let output = run(&work.path().join("no-such-archive"), &"0".repeat(64), &dest);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        PathBuf::from(stdout(&output).trim_end())
            .canonicalize()
            .unwrap(),
        dest.join("lib").canonicalize().unwrap()
    );
}

#[test]
fn a_path_with_spaces_and_accents_is_taken_as_it_is() {
    let work = tempfile::tempdir().unwrap();
    let (archive, hash) = archive(work.path());
    let dest = work.path().join("Người dùng").join("sherpa no tts");

    let output = run(&archive, &hash, &dest);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(dest.join("lib/libsherpa-onnx-c-api.so").is_file());
}
