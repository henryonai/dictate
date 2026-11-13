//! Where a build for macOS looks for the Swift libraries that the ScreenCaptureKit bridge is linked against
//! (`crates/dictate-services/build_swift.rs`, which `build.rs` includes): a build script has no tests of its own.
//!
//! Without them in the search path, a machine that has the Xcode Command Line Tools and not Xcode (what `app/README.md` asks for)
//! cannot link anything that uses the crate: its own build script names only the folder that Xcode has.

#[path = "../build_swift.rs"]
mod build_swift;

use build_swift::swift_lib_dir;
use std::path::Path;

#[test]
fn xcode_has_the_swift_libraries_in_its_toolchain() {
    let developer = Path::new("Xcode.app/Contents/Developer");
    let toolchain = developer.join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");

    assert_eq!(
        swift_lib_dir(developer, |dir| dir == toolchain),
        Some(toolchain)
    );
}

#[test]
fn the_command_line_tools_have_them_beside_the_tools() {
    // There is no toolchain folder in the Command Line Tools: `usr` is right there.
    let developer = Path::new("CommandLineTools");
    let usr = developer.join("usr/lib/swift/macosx");

    assert_eq!(swift_lib_dir(developer, |dir| dir == usr), Some(usr));
}

#[test]
fn the_toolchain_of_xcode_comes_first_when_both_are_there() {
    let developer = Path::new("developer");
    let toolchain = developer.join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");

    assert_eq!(swift_lib_dir(developer, |_| true), Some(toolchain));
}

#[test]
fn a_folder_that_has_neither_gives_none() {
    assert_eq!(swift_lib_dir(Path::new("developer"), |_| false), None);
}
