//! What `build.rs` says on Linux when the sherpa-onnx library is not where it is told to look. Its own file so that a test can
//! read it (`tests/linux_sherpa_setup.rs` includes it): a build script has no tests of its own.

use std::ffi::OsStr;
use std::fmt::Display;
use std::path::Path;

/// The release that `sherpa-onnx-sys` is pinned to (Cargo.toml).
const RELEASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8";
/// The archive with no text to speech, and so no espeak-ng, for Linux x86_64; its SHA-256 is the digest GitHub lists for it, and the one
/// that `scripts/fetch-sherpa-onnx-no-tts.sh` and CI check.
const NO_TTS_ARCHIVE: &str = "sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2";
const NO_TTS_SHA256: &str = "bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b";

/// The message to stop a build on Linux with, or `None` when `lib_dir` (SHERPA_ONNX_LIB_DIR) says where the library is. `arch` is the
/// target's processor (`CARGO_CFG_TARGET_ARCH`).
///
/// Why Linux needs to be told: the library there is a shared one, which has all of its code in it whatever the program uses, and
/// the one that `sherpa-onnx-sys` would fetch by itself has espeak-ng (GPL-3.0) compiled into it.
pub fn missing_lib_dir_message(arch: &str, lib_dir: Option<&OsStr>) -> Option<String> {
    if lib_dir.is_some_and(|dir| !dir.is_empty()) {
        return None;
    }
    let why = "SHERPA_ONNX_LIB_DIR is not set. On Linux the sherpa-onnx library is a shared one, and the one that its build script \
               would download has the speech synthesis of sherpa-onnx in it, with espeak-ng (GPL-3.0), which Dictate has no use \
               for and must not ship. So the library is taken from a folder that you name.";
    Some(match arch {
        "x86_64" => format!(
            "{why}\n\n\
             There is a build of the same release with the synthesis off. Fetch it, check it and unpack it with:\n\n\
             \x20   export SHERPA_ONNX_LIB_DIR=\"$(scripts/fetch-sherpa-onnx-no-tts.sh)\"\n\n\
             (from the root of the repository; it prints the folder), then build again from the same shell. By hand: download\n\
             \x20   {RELEASE}/{NO_TTS_ARCHIVE}\n\
             check that its SHA-256 is\n\
             \x20   {NO_TTS_SHA256}\n\
             unpack it, and set SHERPA_ONNX_LIB_DIR to its `lib` folder. app/README.md, \"On Linux\", has the details."
        ),
        _ => format!(
            "{why}\n\n\
             There is no build without text to speech for {arch} yet: the release has only the one with espeak-ng in it. Build \
             sherpa-onnx v1.13.8 from source with -DSHERPA_ONNX_ENABLE_TTS=OFF and -DBUILD_SHARED_LIBS=ON and set \
             SHERPA_ONNX_LIB_DIR to the `lib` folder of its install, or wait until a build without it is published. For a \
             build of your own that nobody else receives, you can set SHERPA_ONNX_LIB_DIR to the `lib` folder of \
             sherpa-onnx-v1.13.8-linux-aarch64-shared-cpu-lib.tar.bz2 from {RELEASE}: it has espeak-ng in it, so do not distribute \
             what you build with it. app/README.md, \"On Linux\", has the details."
        ),
    })
}

/// The message to stop a build on Linux with when `lib_dir` (SHERPA_ONNX_LIB_DIR) can't be read: `error` is why. The likely cause
/// is a folder that is not the `lib` folder of the unpacked archive without text to speech.
pub fn unreadable_lib_dir_message(lib_dir: &Path, error: &dyn Display) -> String {
    format!(
        "the sherpa-onnx libraries are not in {} ({error}): SHERPA_ONNX_LIB_DIR must name the `lib` folder of the no-tts archive \
         (scripts/fetch-sherpa-onnx-no-tts.sh prints it)",
        lib_dir.display()
    )
}
