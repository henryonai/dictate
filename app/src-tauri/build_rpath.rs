//! The run path that `build.rs` gives the program and its tests, in a file of its own so that a test can read it
//! (`tests/build_rpath.rs` includes it): a build script has no tests of its own.

/// The variable that the release build sets to `1` for the program of an installer (the release workflow does): it gets only the run path of
/// the installed package, and not the ones of a build in the source tree.
pub const INSTALLED_ONLY_VARIABLE: &str = "DICTATE_RPATH_INSTALLED";

/// Whether the value of [`INSTALLED_ONLY_VARIABLE`] asks for the run path of the installed package only. Anything but `1` (or no value) is
/// the build of the source tree, which is what `cargo build` and `cargo test` are.
pub fn installed_only(value: Option<&str>) -> bool {
    value == Some("1")
}

/// The arguments for the linker (one `cargo::rustc-link-arg=` each) that tell the program where its shared libraries are, for the system
/// that `target_os` (`CARGO_CFG_TARGET_OS`) names. `installed_only` is [`installed_only`] of the variable.
///
/// Linux: the shared sherpa-onnx libraries, which are not next to the program everywhere.
///  - `$ORIGIN/../lib/dictate`: the installed package, which has the program in `/usr/bin` and the libraries in `/usr/lib/dictate`
///    (`tauri.release.conf.json` puts them there). Without it a `.deb` does not start. It comes first, and an installed program has
///    nothing else: the entries below would make it look for libraries in `/usr` and in `/`.
///  - `$ORIGIN`: `target/<profile>`, where `sherpa-onnx-sys` and the build script of `dictate-services` copy them and where the program is.
///  - `$ORIGIN/..`: the tests of Cargo until 1.99 are in `target/<profile>/deps`.
///  - `$ORIGIN/../../../..`: the tests of Cargo from 1.100 are in `target/<profile>/build/<package>/<hash>/out`.
///
/// A build script's link arguments reach only the binaries of its own package, so the build scripts of the libraries that the app uses
/// cannot give the app's program and tests any of these.
///
/// macOS: the Swift runtime of the system, which the ScreenCaptureKit bridge (written in Swift, linked by `dictate-services`) loads.
///
/// Windows looks for a DLL beside the program, which is where the installer puts the runtime of Visual C++.
pub fn link_args(target_os: &str, installed_only: bool) -> Vec<String> {
    let rpaths: &[&str] = match (target_os, installed_only) {
        ("linux", true) => &["$ORIGIN/../lib/dictate"],
        ("linux", false) => &[
            "$ORIGIN/../lib/dictate",
            "$ORIGIN",
            "$ORIGIN/..",
            "$ORIGIN/../../../..",
        ],
        ("macos", _) => &["/usr/lib/swift"],
        _ => &[],
    };
    rpaths
        .iter()
        .map(|path| format!("-Wl,-rpath,{path}"))
        .collect()
}
