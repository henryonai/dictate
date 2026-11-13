//! The run path that `build.rs` gives the program and its tests, in a file of its own so that a test can read it
//! (`tests/build_rpath.rs` includes it): a build script has no tests of its own.

/// The arguments for the linker (one `cargo::rustc-link-arg=` each) that tell the program where its shared libraries are, for the system
/// that `target_os` (`CARGO_CFG_TARGET_OS`) names.
///
/// Linux: the shared sherpa-onnx libraries, which are not next to the program everywhere.
///  - `$ORIGIN`: `target/<profile>`, where `sherpa-onnx-sys` and the build script of `dictate-services` copy them and where the program is.
///  - `$ORIGIN/..`: the tests of Cargo until 1.99 are in `target/<profile>/deps`.
///  - `$ORIGIN/../../../..`: the tests of Cargo from 1.100 are in `target/<profile>/build/<package>/<hash>/out`.
///  - `$ORIGIN/../lib/dictate`: the installed package, which has the program in `/usr/bin` and the libraries in `/usr/lib/dictate`
///    (`tauri.release.conf.json` puts them there). Without it a `.deb` does not start.
///
/// A build script's link arguments reach only the binaries of its own package, so the build scripts of the libraries that the app uses
/// cannot give the app's program and tests any of these.
///
/// macOS: the Swift runtime of the system, which the ScreenCaptureKit bridge (written in Swift, linked by `dictate-services`) loads.
///
/// Windows looks for a DLL beside the program, which is where the installer puts the runtime of Visual C++.
pub fn link_args(target_os: &str) -> Vec<String> {
    let rpaths: &[&str] = match target_os {
        "linux" => &[
            "$ORIGIN",
            "$ORIGIN/..",
            "$ORIGIN/../../../..",
            "$ORIGIN/../lib/dictate",
        ],
        "macos" => &["/usr/lib/swift"],
        _ => &[],
    };
    rpaths
        .iter()
        .map(|path| format!("-Wl,-rpath,{path}"))
        .collect()
}
