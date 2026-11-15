//! macOS only: where the Swift libraries are that the ScreenCaptureKit bridge is linked against. Its own file so that a test can read it
//! (`tests/macos_swift_libs.rs` includes it): a build script has no tests of its own.
//!
//! The objects of the bridge ask the linker for `swiftCompatibility56` and its kind, static libraries of the Swift toolchain. The build
//! script of the `screencapturekit` crate adds only the folder that Xcode has them in. The Command Line Tools (`xcode-select
//! --install`, which is all that `app/README.md` asks for) have no `Toolchains/XcodeDefault.xctoolchain`: the same libraries are in
//! `usr/lib/swift/macosx` beside the tools, and without that folder in the search path the link fails with undefined symbols.

use std::path::{Path, PathBuf};

/// The folder of Swift libraries inside `developer_dir` (what `xcode-select -p` prints): Xcode's toolchain when it has one, else the one
/// of the Command Line Tools. `is_dir` tells whether a folder exists. `None` when neither does.
pub fn swift_lib_dir(developer_dir: &Path, is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    [
        "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
        "usr/lib/swift/macosx",
    ]
    .into_iter()
    .map(|relative| developer_dir.join(relative))
    .find(|dir| is_dir(dir))
}
