//! The run path that `app/src-tauri/build.rs` gives the program and its tests (`build_rpath.rs`, which `build.rs` includes): a build script
//! has no tests of its own.
//!
//! On Linux the program has to find the shared sherpa-onnx libraries in three places: beside it (`target/<profile>` in a build of the
//! source tree), one folder up (the tests of Cargo until 1.99 are in `deps`), four up (the tests of Cargo from 1.100 are in
//! `build/<package>/<hash>/out`), and, in the installed package, in `/usr/lib/dictate` (the package has the program in `/usr/bin`). The
//! installed one is what a `.deb` needs to start at all.

#[path = "../build_rpath.rs"]
mod build_rpath;

use build_rpath::link_args;

fn rpaths(target_os: &str) -> Vec<String> {
    link_args(target_os)
        .iter()
        .filter_map(|arg| arg.strip_prefix("-Wl,-rpath,"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn linux_finds_the_libraries_of_the_installed_package() {
    // /usr/bin/dictate -> /usr/lib/dictate: the `.deb` puts the libraries there (tauri.release.conf.json).
    assert!(rpaths("linux").contains(&"$ORIGIN/../lib/dictate".to_owned()));
}

#[test]
fn linux_still_finds_the_libraries_that_a_build_of_the_source_tree_copies_beside_the_program() {
    let rpaths = rpaths("linux");

    assert!(rpaths.contains(&"$ORIGIN".to_owned()), "{rpaths:?}");
    assert!(rpaths.contains(&"$ORIGIN/..".to_owned()), "{rpaths:?}");
}

#[test]
fn linux_finds_them_from_the_tests_that_cargo_1_100_builds_four_folders_down() {
    // The same run path that crates/dictate-services/build.rs gives its own tests.
    assert!(rpaths("linux").contains(&"$ORIGIN/../../../..".to_owned()));
}

#[test]
fn every_argument_is_a_whole_option_for_the_linker() {
    // `cargo::rustc-link-arg=` takes one argument each, and `$ORIGIN` must reach the linker as it is (no shell is involved).
    for arg in link_args("linux") {
        assert!(arg.starts_with("-Wl,-rpath,"), "{arg}");
        assert!(!arg.contains(' ') && !arg.contains('\''), "{arg}");
    }
}

#[test]
fn macos_finds_the_swift_runtime_of_the_system() {
    assert_eq!(link_args("macos"), ["-Wl,-rpath,/usr/lib/swift"]);
}

#[test]
fn windows_has_no_run_path() {
    // Windows looks for a DLL beside the program, which is where the installer puts the runtime of Visual C++.
    assert!(link_args("windows").is_empty());
}
