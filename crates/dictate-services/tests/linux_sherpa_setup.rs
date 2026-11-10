//! What a build on Linux says when it is not told where the sherpa-onnx library without text to speech is
//! (`crates/dictate-services/build_support.rs`, which `build.rs` includes): the message is all that someone who builds without
//! SHERPA_ONNX_LIB_DIR sees, so it has to say what to do, for the processor they have.
//!
//! The other half, that the build really stops, is `build.rs` calling it; CI sets the variable on Linux, and the build of a container
//! without it is where that is seen.
#![allow(dead_code)]

use std::ffi::OsStr;

#[path = "../build_support.rs"]
mod build_support;

use build_support::{missing_lib_dir_message, unreadable_lib_dir_message};

#[test]
fn a_build_that_has_the_folder_is_told_nothing() {
    for arch in ["x86_64", "aarch64", "riscv64"] {
        assert_eq!(
            missing_lib_dir_message(arch, Some(OsStr::new("/somewhere/lib"))),
            None,
            "{arch}"
        );
    }
}

#[test]
fn an_empty_variable_is_no_variable() {
    assert!(missing_lib_dir_message("x86_64", Some(OsStr::new(""))).is_some());
}

#[test]
fn x86_64_is_told_the_script_the_archive_and_its_sha256() {
    let message = missing_lib_dir_message("x86_64", None).expect("a message");

    assert!(message.contains("SHERPA_ONNX_LIB_DIR"), "{message}");
    assert!(
        message.contains("scripts/fetch-sherpa-onnx-no-tts.sh"),
        "{message}"
    );
    // The same by hand: where it comes from and what it has to hash to.
    assert!(
        message.contains(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2"
        ),
        "{message}"
    );
    assert!(
        message.contains("bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b"),
        "{message}"
    );
    assert!(message.contains("app/README.md"), "{message}");
    // Why: it is what keeps espeak-ng out.
    assert!(message.contains("espeak-ng"), "{message}");
    assert!(message.contains("GPL"), "{message}");
}

#[test]
fn aarch64_is_told_there_is_no_build_without_text_to_speech_yet() {
    let message = missing_lib_dir_message("aarch64", None).expect("a message");

    assert!(message.contains("SHERPA_ONNX_LIB_DIR"), "{message}");
    assert!(
        message.contains("no build without text to speech"),
        "{message}"
    );
    // What to do instead: build it with the synthesis off.
    assert!(message.contains("SHERPA_ONNX_ENABLE_TTS=OFF"), "{message}");
    assert!(message.contains("app/README.md"), "{message}");
    // The archive that exists has espeak-ng in it: for a build of one's own only, and not to be given to anyone.
    assert!(
        message.contains("linux-aarch64-shared-cpu-lib"),
        "{message}"
    );
    assert!(message.contains("not distribute"), "{message}");
    // The x86_64 archive is no answer for it.
    assert!(
        !message.contains("linux-x64-shared-no-tts-lib"),
        "{message}"
    );
}

#[test]
fn another_processor_is_told_the_same_as_aarch64() {
    let message = missing_lib_dir_message("riscv64", None).expect("a message");
    assert!(
        message.contains("no build without text to speech"),
        "{message}"
    );
    assert!(message.contains("SHERPA_ONNX_ENABLE_TTS=OFF"), "{message}");
}

#[test]
fn a_folder_that_cannot_be_read_says_which_one_and_what_it_has_to_be() {
    let message = unreadable_lib_dir_message(
        std::path::Path::new("/somewhere/lib"),
        &"No such file or directory (os error 2)",
    );

    assert!(message.contains("/somewhere/lib"), "{message}");
    assert!(
        message.contains("No such file or directory (os error 2)"),
        "{message}"
    );
    // What SHERPA_ONNX_LIB_DIR has to be, and where the script that prints it is.
    assert!(message.contains("SHERPA_ONNX_LIB_DIR"), "{message}");
    assert!(message.contains("`lib` folder"), "{message}");
    assert!(
        message.contains("scripts/fetch-sherpa-onnx-no-tts.sh"),
        "{message}"
    );
    // The constant that the old text asked about is gone.
    assert!(!message.contains("SHERPA_ONNX_VERSION"), "{message}");
}
