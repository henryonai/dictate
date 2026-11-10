//! Linux only: the binaries that use this crate (its tests, the app) load the shared sherpa-onnx library from
//! `target/<profile>`. This script makes sure that the `.so` files are there, and gives the tests of this crate the run
//! path that finds them.
//!
//! `sherpa-onnx-sys` copies the files into `target/<profile>` itself, but only when its build script runs. CI's cache
//! (rust-cache) keeps only `build/`, `deps/` and `.fingerprint/` of `target/<profile>`: after a restore the files are
//! gone, and that build script, a registry crate's, is up to date and does not run again. So this one copies them too,
//! whenever one is missing or differs (`rerun-if-changed` names each copy, and a missing path makes Cargo run the
//! script again). Each copy gets the time of its source, which is older than this run, so that it does not make the
//! next build run the script again.
//!
//! The folder that the libraries come from is SHERPA_ONNX_LIB_DIR, which Linux requires: the library that `sherpa-onnx-sys` would
//! fetch by itself has espeak-ng (GPL-3.0) in it. `scripts/fetch-sherpa-onnx-no-tts.sh` fetches the one that has none.
//!
//! The run path: tests are built into `target/<profile>/deps`, hence `$ORIGIN/..`. A build script's link arguments
//! reach only the binaries of its own package, so `sherpa-onnx-sys` cannot set this up for the packages that use it.
//!
//! macOS only: the Swift bridge of ScreenCaptureKit needs two things of the build that its own build script gives only to its own
//! binaries, of which a library has none. The folder of the Swift libraries is added to the search path of the linker
//! (`build_swift.rs`), so that a machine with the Command Line Tools and no Xcode links; unlike a link argument, a search path
//! reaches the binaries of the packages that use this one. And the run path `/usr/lib/swift` (where the system has the Swift
//! runtime, `libswift_Concurrency` among it) is given to the binaries of this package; the app's build script does the same.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[path = "build_support.rs"]
mod build_support;
#[path = "build_swift.rs"]
mod build_swift;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build_support.rs");
    println!("cargo::rerun-if-changed=build_swift.rs");
    println!("cargo::rerun-if-env-changed=SHERPA_ONNX_LIB_DIR");
    println!("cargo::rerun-if-env-changed=DEVELOPER_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        add_swift_libraries_to_the_search_path();
        println!("cargo::rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
        let out_dir =
            PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"));
        // OUT_DIR is target/<profile>/build/dictate-services-<hash>/out.
        let profile_dir = out_dir
            .ancestors()
            .nth(3)
            .expect("OUT_DIR is inside target/<profile>");
        copy_shared_libraries(&sherpa_lib_dir(), profile_dir);
    }
}

/// Adds the folder of the Swift toolchain's libraries to the linker's search path: the one of the developer directory that `xcode-select -p`
/// names (it honours DEVELOPER_DIR). A machine where that cannot be asked is left to the build script of `screencapturekit`, which
/// warns about it.
fn add_swift_libraries_to_the_search_path() {
    let Ok(output) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let developer_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if let Some(dir) = build_swift::swift_lib_dir(&developer_dir, Path::is_dir) {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
}

/// The folder that `sherpa-onnx-sys` links from: SHERPA_ONNX_LIB_DIR, which is required on Linux. Without it the build script of that
/// crate downloads the archive with espeak-ng (GPL-3.0) in it, so the build stops here instead and says how to get the one that has
/// none (`build_support.rs`). That crate's script has run by now (it comes first) and has downloaded it; nothing is linked from it.
fn sherpa_lib_dir() -> PathBuf {
    let lib_dir = std::env::var_os("SHERPA_ONNX_LIB_DIR");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if let Some(message) = build_support::missing_lib_dir_message(&arch, lib_dir.as_deref()) {
        eprintln!("\n{message}\n");
        std::process::exit(1);
    }
    PathBuf::from(lib_dir.expect("checked just above"))
}

/// Copies every `.so` file of `lib_dir` into `dest_dir`, as `sherpa-onnx-sys` does, unless the same copy is there.
fn copy_shared_libraries(lib_dir: &Path, dest_dir: &Path) {
    let entries = fs::read_dir(lib_dir)
        .unwrap_or_else(|e| panic!("{}", build_support::unreadable_lib_dir_message(lib_dir, &e)));
    let mut copied = 0;
    for entry in entries {
        let source = entry.expect("reading the sherpa-onnx lib folder").path();
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.contains(".so") {
            continue;
        }
        let dest = dest_dir.join(name);
        println!("cargo::rerun-if-changed={}", source.display());
        println!("cargo::rerun-if-changed={}", dest.display());
        copy_if_different(&source, &dest)
            .unwrap_or_else(|e| panic!("copying {} to {}: {e}", source.display(), dest.display()));
        copied += 1;
    }
    assert!(copied > 0, "no .so file in {}", lib_dir.display());
}

/// Copies `source` (following links) to `dest` through a temporary file, so that a program that has the old copy
/// loaded keeps it, and gives the copy the modification time of the source. Does nothing when `dest` has the size and
/// the time of `source` already.
fn copy_if_different(source: &Path, dest: &Path) -> std::io::Result<()> {
    let source_meta = fs::metadata(source)?;
    let modified = source_meta.modified()?;
    if let Ok(dest_meta) = fs::metadata(dest) {
        if dest_meta.len() == source_meta.len() && dest_meta.modified()? == modified {
            return Ok(());
        }
    }
    let mut temp = dest.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    fs::copy(source, &temp)?;
    File::open(&temp)?.set_modified(modified)?;
    fs::rename(&temp, dest)
}
