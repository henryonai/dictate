//! The folder that `crates/dictate-services/build.rs` copies the shared libraries of sherpa-onnx into is where the programs and
//! the tests of the build are: the profile folder, which `build_support::profile_dir` finds from `OUT_DIR`. What is between the
//! two is up to Cargo, and Cargo 1.100 changed it (`build/<package>-<hash>/out` became `build/<package>/<hash>/out`): the libraries
//! went to `target/debug/build`, where nothing looks for them, and CI's check of `target/debug/*.so` failed on a restored cache.
//!
//! This builds a crate of its own with the Cargo that runs the tests, and its build script (which includes `build_support.rs`) says
//! what it found. The other tests of the function (`linux_sherpa_setup.rs`) name both layouts; this one is the Cargo that is there.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The build script of the crate under test: the profile folder that `profile_dir` finds, written beside the manifest.
fn build_script(build_support: &Path) -> String {
    format!(
        r#"#[allow(dead_code)]
#[path = {build_support:?}]
mod build_support;

fn main() {{
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let found = build_support::profile_dir(&out_dir).expect("a profile folder above OUT_DIR");
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    std::fs::write(manifest.join("found.txt"), found.to_string_lossy().as_bytes()).expect("write found.txt");
}}
"#
    )
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_profile_folder_found_in_a_real_build_is_the_one_that_has_the_program() {
    // Not canonicalized: on Windows that makes a `\\?\` path, which an attribute of rustc need not take.
    let build_support = Path::new(env!("CARGO_MANIFEST_DIR")).join("build_support.rs");
    let probe = tempfile::tempdir().expect("a folder for the crate");
    fs::create_dir(probe.path().join("src")).unwrap();
    fs::write(
        probe.path().join("Cargo.toml"),
        "[package]\nname = \"profile-dir-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(probe.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(probe.path().join("build.rs"), build_script(&build_support)).unwrap();
    let target = probe.path().join("target");

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["build", "--offline", "--manifest-path"])
        .arg(probe.path().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        // What a developer or a CI job may have set for their own build is not what is under test.
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let found = fs::read_to_string(probe.path().join("found.txt")).expect("the build script ran");
    let program = target.join("debug").join(if cfg!(windows) {
        "profile-dir-probe.exe"
    } else {
        "profile-dir-probe"
    });
    assert!(program.is_file(), "no program at {}", program.display());
    assert_eq!(
        canonical(Path::new(&found)),
        canonical(program.parent().unwrap()),
        "the build script took {found} for the profile folder, and the program is in {}",
        program.display()
    );
}
