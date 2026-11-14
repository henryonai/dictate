//! The run path that `app/src-tauri/build.rs` gives the program and its tests (`build_rpath.rs`, which `build.rs` includes): a build script
//! has no tests of its own.
//!
//! On Linux the program has to find the shared sherpa-onnx libraries in three places: beside it (`target/<profile>` in a build of the
//! source tree), one folder up (the tests of Cargo until 1.99 are in `deps`), four up (the tests of Cargo from 1.100 are in
//! `build/<package>/<hash>/out`), and, in the installed package, in `/usr/lib/dictate` (the package has the program in `/usr/bin`). The
//! installed one is what a `.deb` needs to start at all.
//!
//! The installed program has only that one: the others are for a build in the source tree, and in a package they would make the program look
//! for libraries in `/usr` (`$ORIGIN/..`) and in `/` (`$ORIGIN/../../../..`). The release build asks for it with `DICTATE_RPATH_INSTALLED=1`.

#[path = "../build_rpath.rs"]
mod build_rpath;

use build_rpath::{installed_only, link_args, INSTALLED_ONLY_VARIABLE};

fn rpaths(target_os: &str, installed: bool) -> Vec<String> {
    link_args(target_os, installed)
        .iter()
        .filter_map(|arg| arg.strip_prefix("-Wl,-rpath,"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn linux_finds_the_libraries_of_the_installed_package_and_looks_there_first() {
    // /usr/bin/dictate -> /usr/lib/dictate: the `.deb` puts the libraries there (tauri.release.conf.json).
    for installed in [false, true] {
        assert_eq!(
            rpaths("linux", installed).first().map(String::as_str),
            Some("$ORIGIN/../lib/dictate"),
            "installed only: {installed}"
        );
    }
}

#[test]
fn linux_still_finds_the_libraries_that_a_build_of_the_source_tree_copies_beside_the_program() {
    let rpaths = rpaths("linux", false);

    assert!(rpaths.contains(&"$ORIGIN".to_owned()), "{rpaths:?}");
    assert!(rpaths.contains(&"$ORIGIN/..".to_owned()), "{rpaths:?}");
}

#[test]
fn linux_finds_them_from_the_tests_that_cargo_1_100_builds_four_folders_down() {
    // The same run path that crates/dictate-services/build.rs gives its own tests.
    assert!(rpaths("linux", false).contains(&"$ORIGIN/../../../..".to_owned()));
}

#[test]
fn the_installed_program_has_no_run_path_but_its_own_folder_of_libraries() {
    // `$ORIGIN/..` of /usr/bin/dictate is /usr, and `$ORIGIN/../../../..` is /: a library that somebody puts there would be loaded.
    assert_eq!(rpaths("linux", true), ["$ORIGIN/../lib/dictate"]);
}

#[test]
fn every_argument_is_a_whole_option_for_the_linker() {
    // `cargo::rustc-link-arg=` takes one argument each, and `$ORIGIN` must reach the linker as it is (no shell is involved).
    for installed in [false, true] {
        for arg in link_args("linux", installed) {
            assert!(arg.starts_with("-Wl,-rpath,"), "{arg}");
            assert!(!arg.contains(' ') && !arg.contains('\''), "{arg}");
        }
    }
}

#[test]
fn macos_finds_the_swift_runtime_of_the_system() {
    for installed in [false, true] {
        assert_eq!(link_args("macos", installed), ["-Wl,-rpath,/usr/lib/swift"]);
    }
}

#[test]
fn windows_has_no_run_path() {
    // Windows looks for a DLL beside the program, which is where the installer puts the runtime of Visual C++.
    for installed in [false, true] {
        assert!(link_args("windows", installed).is_empty());
    }
}

#[test]
fn only_the_value_1_of_the_variable_asks_for_the_installed_run_path() {
    assert_eq!(INSTALLED_ONLY_VARIABLE, "DICTATE_RPATH_INSTALLED");
    assert!(installed_only(Some("1")));
    // A build that does not say, or says anything else, is the build of the source tree: the one that tests run.
    for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
        assert!(!installed_only(value), "{value:?}");
    }
}
