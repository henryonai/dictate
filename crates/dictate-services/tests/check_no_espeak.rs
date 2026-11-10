//! `scripts/check-no-espeak.sh`, which CI runs on the programs and libraries that the build makes. espeak-ng is GPL-3.0: code of it
//! that ends up in what Dictate ships is a matter for the license, and the script is what notices it.
//!
//! The script is for bash, which CI runs on every system (Git Bash on Windows): these tests run where `bash` is the real one.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-no-espeak.sh")
}

fn run(files: &[&Path]) -> Output {
    Command::new("bash")
        .arg(script())
        .args(files)
        .output()
        .expect("bash runs the script")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("the script ended by itself")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A file that looks like a program: bytes that are not text, with `inside` somewhere in the middle of them.
fn binary(dir: &Path, name: &str, inside: &[u8]) -> PathBuf {
    let mut bytes = vec![0u8, 1, 2, 0xff, 0xfe, 0];
    bytes.extend_from_slice(inside);
    bytes.extend_from_slice(&[0, 0xc3, 0x28, 0, 7]);
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_program_with_the_data_path_variable_of_espeak_ng_in_it_fails() {
    // `ESPEAK_DATA_PATH` is what espeak-ng reads to find its data: a string in the code, so it stays in a program whose symbol
    // names are stripped or live in a separate file (Windows).
    let dir = tempfile::tempdir().unwrap();
    let program = binary(dir.path(), "dictate", b"x\0ESPEAK_DATA_PATH\0y");

    let output = run(&[&program]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("dictate"), "{}", stderr(&output));
    assert!(stderr(&output).contains("espeak-ng"), "{}", stderr(&output));
}

#[test]
fn a_program_with_a_function_of_espeak_ng_in_it_fails() {
    let dir = tempfile::tempdir().unwrap();
    let program = binary(dir.path(), "libsherpa.so", b"\0_espeak_ng_InitializePath\0");

    assert_eq!(code(&run(&[&program])), 1);
}

#[test]
fn a_program_that_only_has_the_names_of_the_speaker_functions_of_sherpa_passes() {
    // "SherpaOnnxOfflineSpeakerDiarization" has "eSpeaker" in it, and a text may say "espeak": neither is one of espeak-ng's names.
    let dir = tempfile::tempdir().unwrap();
    let program = binary(
        dir.path(),
        "dictate",
        b"SherpaOnnxOfflineSpeakerDiarizationProcess\0sherpa_onnx::OfflineSpeakerSegmentation\0espeak\0",
    );

    let output = run(&[&program]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn only_the_files_that_have_espeak_ng_are_named_and_one_of_them_fails_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let clean = binary(dir.path(), "clean-program", b"nothing to see");
    let dirty = binary(dir.path(), "dirty-library.so", b"espeak_ng_Terminate");

    let output = run(&[&clean, &dirty]);

    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("dirty-library.so"));
    assert!(!stderr(&output).contains("clean-program"));
}

#[test]
fn a_file_that_is_not_there_is_an_error_and_not_a_pass() {
    // A path that is mistyped in the workflow must not make the check succeed on nothing.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-program");

    let output = run(&[&missing]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("no-such-program"));
}

#[test]
fn a_missing_file_is_an_error_even_when_another_one_has_espeak_ng() {
    let dir = tempfile::tempdir().unwrap();
    let dirty = binary(dir.path(), "dirty", b"ESPEAK_DATA_PATH");
    let missing = dir.path().join("missing");

    // The error of a missing file is not hidden by the failure of the other one.
    assert_eq!(code(&run(&[&dirty, &missing])), 2);
}

#[test]
fn no_files_at_all_is_an_error() {
    assert_eq!(code(&run(&[])), 2);
}

#[test]
fn a_file_name_with_spaces_and_accents_is_taken_as_it_is() {
    // The checkout of a person whose user name is Vietnamese has folders such as this one.
    let dir = tempfile::tempdir().unwrap();
    let odd = binary(dir.path(), "Người dùng (2).bin", b"ESPEAK_DATA_PATH");

    let output = run(&[&odd]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("Người dùng (2).bin"));
}
