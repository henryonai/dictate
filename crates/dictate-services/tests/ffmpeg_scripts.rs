//! The scripts that make the ffmpeg that ships with the app and put the programs that are shipped where Tauri looks for them:
//! `scripts/check-ffmpeg-lgpl.sh`, `scripts/build-ffmpeg.sh` and `scripts/stage-sidecars.sh`.
//!
//! ffmpeg is LGPL-2.1+ only when it is built without `--enable-gpl`, `--enable-nonfree` and `--enable-version3`; a build that has
//! any of them (or links x264) is GPL, which Dictate (MIT) must not ship. `check-ffmpeg-lgpl.sh` is what notices it, and
//! `build-ffmpeg.sh` is what must never build from a source that is not the pinned one.
//!
//! The scripts are for bash, which CI runs on every system: these tests run where `bash` is the real one. The programs that the
//! check is run on are stand-ins: shell scripts that print what an ffmpeg prints for `-buildconf` and `-L`, and, for the part that
//! reads what a program links, small programs made by the C compiler of the machine.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts")
}

fn bash(script: &str, args: &[&str]) -> Command {
    let mut command = Command::new("bash");
    command.arg(scripts().join(script)).args(args);
    command
}

fn run(mut command: Command) -> Output {
    command.output().expect("bash runs the script")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("the script ended by itself")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// ---- check-ffmpeg-lgpl.sh ----------------------------------------------------------------------------------------------------

/// What `ffmpeg -L` prints of an ffmpeg built as LGPL 2.1 or later.
const LGPL_2_1: &str = "FFmpeg is free software; you can redistribute it and/or
modify it under the terms of the GNU Lesser General Public
License as published by the Free Software Foundation; either
version 2.1 of the License, or (at your option) any later version.

FFmpeg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
Lesser General Public License for more details.";

/// What it prints of one built with `--enable-version3` (LGPL 3) and of one built with `--enable-gpl`.
const LGPL_3: &str = "FFmpeg is free software; you can redistribute it and/or
modify it under the terms of the GNU Lesser General Public
License as published by the Free Software Foundation; either
version 3 of the License, or (at your option) any later version.";
const GPL_2: &str = "FFmpeg is free software; you can redistribute it and/or
modify it under the terms of the GNU General Public License
as published by the Free Software Foundation; either version 2 of the License,
or (at your option) any later version.";

/// The configure line of a plain build, one option to a line as `ffmpeg -buildconf` prints it.
const PLAIN_BUILDCONF: &str = "    --disable-gpl
    --disable-nonfree
    --disable-version3
    --disable-autodetect
    --disable-everything
    --enable-zlib";

/// What `ffmpeg -hide_banner -encoders` prints of the build that ships (its start, and a few of its lines).
const PLAIN_ENCODERS: &str = "Encoders:
 V..... = Video
 A..... = Audio
 ------
 V....D h264_videotoolbox    VideoToolbox H.264 Encoder (codec h264)
 V.S..D mpeg4                MPEG-4 part 2
 A....D aac                  AAC (Advanced Audio Coding)";

/// A shell script that answers as an ffmpeg does: `buildconf` for `-buildconf`, `license` for `-L`, the plain list of encoders.
/// `buildconf_command` is the shell command that prints it (so that a test can have it put together as the program runs, which is not
/// the bytes of the file); `marker` is the line that libavutil has in a real ffmpeg and that the check looks for in the file.
fn fake(dir: &Path, name: &str, buildconf_command: &str, license: &str, marker: bool) -> PathBuf {
    fake_with_encoders(
        dir,
        name,
        buildconf_command,
        license,
        marker,
        PLAIN_ENCODERS,
    )
}

fn fake_with_encoders(
    dir: &Path,
    name: &str,
    buildconf_command: &str,
    license: &str,
    marker: bool,
    encoders: &str,
) -> PathBuf {
    let path = dir.join(name);
    let marker = if marker {
        "# LGPL version 2.1 or later\n"
    } else {
        ""
    };
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n{marker}case \"$*\" in\n*-buildconf*)\n{buildconf_command}\n;;\n*-L*) cat <<'EOF'\n{license}\nEOF\n;;\n*-encoders*) cat <<'EOF'\n{encoders}\nEOF\n;;\n*) echo 'ffmpeg version 9.0.2' ;;\nesac\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn prints(text: &str) -> String {
    format!("cat <<'EOF'\n{text}\nEOF")
}

fn check(files: &[&Path]) -> Output {
    let mut command = bash("check-ffmpeg-lgpl.sh", &[]);
    command.args(files);
    run(command)
}

#[test]
fn check_ffmpeg_lgpl_passes_an_lgpl_one() {
    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = fake(
        dir.path(),
        "dictate-ffmpeg",
        &prints(PLAIN_BUILDCONF),
        LGPL_2_1,
        true,
    );

    let output = check(&[&ffmpeg]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("dictate-ffmpeg"),
        "{}",
        stdout(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn check_ffmpeg_lgpl_refuses_a_gpl_buildconf() {
    // Each of the three options that make a build GPL, version 3 or non-free. The options are put together as the program runs, so
    // that it is the line that `-buildconf` prints that is refused, and not only the bytes of the file.
    for option in ["gpl", "nonfree", "version3"] {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = fake(
            dir.path(),
            "ffmpeg-with-an-option",
            &format!(
                "printf '%s\\n' '    --disable-autodetect' \"    --enable-\"'{option}' '    --enable-zlib'"
            ),
            LGPL_2_1,
            true,
        );

        let output = check(&[&ffmpeg]);

        assert_eq!(code(&output), 1, "{option}: {}", stdout(&output));
        let message = stderr(&output);
        assert!(message.contains("ffmpeg-with-an-option"), "{message}");
        assert!(message.contains(&format!("--enable-{option}")), "{message}");
    }
}

#[test]
fn check_ffmpeg_lgpl_refuses_a_gpl_library_in_the_buildconf() {
    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = fake(
        dir.path(),
        "ffmpeg",
        &prints(&format!("{PLAIN_BUILDCONF}\n    --enable-libx264")),
        LGPL_2_1,
        true,
    );

    let output = check(&[&ffmpeg]);

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("--enable-libx264"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_ffmpeg_lgpl_refuses_a_build_that_lists_a_gpl_encoder() {
    // A build that has x264 in it lists it, whatever its configure line says (a library linked in from a shared copy, a line that was
    // edited): the list of encoders is what the program can do.
    for encoder in ["libx264", "libx265", "libfdk_aac", "libxvid"] {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = fake_with_encoders(
            dir.path(),
            "ffmpeg",
            &prints(PLAIN_BUILDCONF),
            LGPL_2_1,
            true,
            &format!("{PLAIN_ENCODERS}\n V....D {encoder:<19} an encoder (codec h264)"),
        );

        let output = check(&[&ffmpeg]);

        assert_eq!(code(&output), 1, "{encoder}: {}", stdout(&output));
        assert!(
            stderr(&output).contains(encoder),
            "{encoder}: {}",
            stderr(&output)
        );
    }
    // The encoders that a plain build has are not refused for having a name that starts like one of them.
    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = fake_with_encoders(
        dir.path(),
        "ffmpeg",
        &prints(PLAIN_BUILDCONF),
        LGPL_2_1,
        true,
        &format!("{PLAIN_ENCODERS}\n V....D libopenh264         OpenH264 H.264 (codec h264)\n V....D libvpx-vp9          libvpx VP9 (codec vp9)"),
    );
    assert_eq!(code(&check(&[&ffmpeg])), 0);
}

#[test]
fn check_ffmpeg_lgpl_refuses_a_license_that_is_not_lgpl_2_1() {
    // The configure line is clean and the program says another license: the build and the program disagree, or it is not
    // what the line says it is.
    for (name, license) in [("gpl", GPL_2), ("lgpl3", LGPL_3)] {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = fake(dir.path(), name, &prints(PLAIN_BUILDCONF), license, true);

        let output = check(&[&ffmpeg]);

        assert_eq!(code(&output), 1, "{name}");
        assert!(
            stderr(&output).contains("-L"),
            "{name}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn a_program_that_cannot_run_here_is_checked_by_its_bytes() {
    // The Windows ffmpeg is built on Linux and cannot run there: the configure line and the license name are strings in the file.
    let dir = tempfile::tempdir().unwrap();
    let program = |name: &str, inside: &str| {
        let path = dir.path().join(name);
        let mut bytes = vec![0u8, 1, 2, 0xff, 0xfe, 0];
        bytes.extend_from_slice(inside.as_bytes());
        bytes.extend_from_slice(&[0, 0xc3, 0x28, 0, 7]);
        fs::write(&path, bytes).unwrap();
        path
    };
    let plain = program(
        "plain.exe",
        "\0LGPL version 2.1 or later\0--disable-gpl --disable-nonfree --disable-version3 --enable-zlib\0",
    );
    let gpl = program(
        "gpl.exe",
        "\0LGPL version 2.1 or later\0--enable-gpl --enable-libx264\0",
    );
    let no_license = program("unknown.exe", "\0--disable-gpl\0");

    let output = check(&[&plain]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    let output = check(&[&gpl]);
    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("--enable-gpl"),
        "{}",
        stderr(&output)
    );

    // A file that does not say which license it is under is not one that is known to be LGPL.
    let output = check(&[&no_license]);
    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("LGPL version 2.1 or later"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_ffmpeg_lgpl_names_only_the_files_that_fail_and_one_of_them_fails_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let good = fake(
        dir.path(),
        "good-ffmpeg",
        &prints(PLAIN_BUILDCONF),
        LGPL_2_1,
        true,
    );
    let bad = fake(
        dir.path(),
        "bad-ffmpeg",
        &prints(&format!("{PLAIN_BUILDCONF}\n    --enable-gpl")),
        GPL_2,
        true,
    );

    let output = check(&[&good, &bad]);

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("bad-ffmpeg"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("good-ffmpeg"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_ffmpeg_lgpl_on_a_file_that_is_not_there_is_an_error_and_not_a_pass() {
    let dir = tempfile::tempdir().unwrap();

    let output = check(&[&dir.path().join("no-such-ffmpeg")]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("no-such-ffmpeg"));
    assert_eq!(code(&run(bash("check-ffmpeg-lgpl.sh", &[]))), 2);
}

/// A program of the C compiler of this machine that answers as an ffmpeg does, so that what the check says about what a program
/// links is about a real executable of this system. `link` is how it is linked.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn native(dir: &Path, name: &str, link: Link) -> PathBuf {
    let source = dir.join(format!("{name}.c"));
    let extra_call = if matches!(link, Link::ExtraLibrary) {
        "extra_function();"
    } else {
        ""
    };
    fs::write(
        &source,
        format!(
            r#"#include <stdio.h>
#include <string.h>
{declaration}
static const char *marker = "LGPL version 2.1 or later";
int main(int argc, char **argv) {{
  if (argc > 99) puts(marker);
  {extra_call}
  if (argc > 1 && strcmp(argv[argc - 1], "-buildconf") == 0) puts("{conf}");
  else if (argc > 1 && strcmp(argv[argc - 1], "-L") == 0) puts("{license}");
  else puts("ffmpeg version 9.0.2");
  return 0;
}}
"#,
            declaration = if matches!(link, Link::ExtraLibrary) {
                "void extra_function(void);"
            } else {
                ""
            },
            conf = "--disable-gpl --disable-nonfree --disable-version3",
            license =
                "GNU Lesser General Public License, either version 2.1 of the License, or later",
        ),
    )
    .unwrap();
    let program = dir.join(name);
    let mut compile = Command::new("cc");
    compile.arg("-o").arg(&program).arg(&source);
    match link {
        // What a program that has everything it needs in it is (Linux), or the one that has only the libraries of the system (macOS).
        Link::AsTheSystemDoes => {
            if cfg!(target_os = "linux") {
                compile.arg("-static");
            }
        }
        // A program that needs a library of its own: it is not a system library on any system.
        Link::ExtraLibrary => {
            let library_source = dir.join("extra.c");
            fs::write(&library_source, "void extra_function(void) {}\n").unwrap();
            let library = dir.join(if cfg!(target_os = "macos") {
                "libextra.dylib"
            } else {
                "libextra.so"
            });
            let built = Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&library)
                .arg(&library_source)
                .status()
                .expect("the C compiler runs");
            assert!(built.success());
            compile.arg(&library);
            if cfg!(target_os = "linux") {
                compile.arg(format!("-Wl,-rpath,{}", dir.display()));
            }
        }
        // A program that Linux links to the C library as a shared one, which is what the one that ships must not be.
        #[cfg(target_os = "linux")]
        Link::Dynamic => {}
    }
    let compiled = compile.status().expect("the C compiler runs");
    assert!(compiled.success(), "the C compiler failed");
    program
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
enum Link {
    AsTheSystemDoes,
    ExtraLibrary,
    #[cfg(target_os = "linux")]
    Dynamic,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_program_that_links_only_the_system_passes() {
    // macOS: libSystem, which is under /usr/lib. Linux: nothing at all, as the one that ships is built static.
    let dir = tempfile::tempdir().unwrap();
    let program = native(dir.path(), "static-ffmpeg", Link::AsTheSystemDoes);

    let output = check(&[&program]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_program_that_links_a_library_of_its_own_fails() {
    // A library that is not a system one is code that is not covered by what the check says of the program: x264 that was
    // linked in as a shared library would be GPL code in the package, and `-buildconf` of the program would not say it.
    let dir = tempfile::tempdir().unwrap();
    let program = native(dir.path(), "linked-ffmpeg", Link::ExtraLibrary);

    let output = check(&[&program]);

    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("linked-ffmpeg"), "{message}");
    assert!(message.contains("libextra"), "{message}");
}

#[cfg(target_os = "linux")]
#[test]
fn on_linux_a_program_that_is_not_static_fails() {
    let dir = tempfile::tempdir().unwrap();
    let program = native(dir.path(), "dynamic-ffmpeg", Link::Dynamic);

    let output = check(&[&program]);

    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(stderr(&output).contains("libc.so"), "{}", stderr(&output));
}

// ---- build-ffmpeg.sh ---------------------------------------------------------------------------------------------------------

fn build(target: &str, out: &Path, source: &Path) -> Output {
    let mut command = bash("build-ffmpeg.sh", &[target]);
    command
        .arg(out)
        .env("FFMPEG_SOURCE_URL", format!("file://{}", source.display()));
    run(command)
}

#[test]
fn build_ffmpeg_refuses_a_tarball_with_the_wrong_hash() {
    // A file that is not the pinned tarball (the download of it was replaced, something on the way answered for it) is not built.
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("ffmpeg-9.0.2.tar.xz");
    fs::write(&source, b"this is not the tarball of ffmpeg").unwrap();
    let hash = format!("{:x}", Sha256::digest(fs::read(&source).unwrap()));
    let out = work.path().join("out");

    let output = build("x86_64-unknown-linux-gnu", &out, &source);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    // It says what it got and what was pinned, so that a changed download can be told from a bad connection.
    let message = stderr(&output);
    assert!(message.contains(&hash), "{message}");
    assert!(
        message.contains("8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"),
        "{message}"
    );
    // Nothing was unpacked and nothing was made: the file is not a tarball, so unpacking it would have said something else, and the
    // folder of the result has neither the program nor the tarball that it ships the source with.
    assert!(!message.contains("tar:"), "{message}");
    let left: Vec<_> = fs::read_dir(&out)
        .map(|dir| dir.map(|entry| entry.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn build_ffmpeg_refuses_a_target_it_does_not_know_before_it_downloads_anything() {
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out");

    // The source does not exist: if the script went to get it, that would be the failure.
    let output = build(
        "riscv64-unknown-linux-gnu",
        &out,
        &work.path().join("nothing"),
    );

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("riscv64-unknown-linux-gnu"),
        "{}",
        stderr(&output)
    );
    assert!(!stderr(&output).contains("download"), "{}", stderr(&output));
}

/// The lines of a script that are not comments: the comments say what the script does not do.
fn code_of(script: &str) -> String {
    fs::read_to_string(scripts().join(script))
        .unwrap()
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn build_ffmpeg_enables_every_demuxer_and_every_decoder() {
    // An import goes through the ffmpeg of the app whenever symphonia cannot read the file, and `Tools::find` takes the one that ships
    // before the one of the user: a format that the build lacks is a format that cannot be imported. So every demuxer and every
    // decoder is on (the ones that are GPL, version 3 or non-free stay out by themselves: the build is configured without them),
    // and not a list of the ones that somebody thought of.
    let code = code_of("build-ffmpeg.sh");

    let everything_off = code.find("--disable-everything").unwrap();
    for all in [
        "--enable-demuxers",
        "--enable-decoders",
        "--enable-parsers",
        "--enable-bsfs",
    ] {
        let at = code
            .find(all)
            .unwrap_or_else(|| panic!("{all} is not in the configure line"));
        // `--enable-demuxers` is not the start of a longer option such as `--enable-demuxers-x`, and comes after `--disable-everything`
        // (before it, it would be switched off again).
        assert!(
            code[at + all.len()..].starts_with([' ', '\n', '\\']),
            "{all}: {}",
            &code[at..at + 30]
        );
        assert!(
            at > everything_off,
            "{all} comes before --disable-everything"
        );
    }
    for list in [
        "--enable-demuxer=",
        "--enable-decoder=",
        "DEMUXERS=",
        "DECODERS=",
    ] {
        assert!(!code.contains(list), "{list} is a curated list again");
    }
    // The ones that stay as the app asks for them: the protocols, the muxers, the encoders and the filters.
    for curated in [
        "--enable-protocol=",
        "--enable-muxer=",
        "--enable-encoder=",
        "--enable-filter=",
    ] {
        assert!(code.contains(curated), "{curated}");
    }
    // And the licence options are still the ones of an LGPL v2.1 build.
    for off in ["--disable-gpl", "--disable-nonfree", "--disable-version3"] {
        assert!(code.contains(off), "{off}");
    }
}

#[test]
fn build_ffmpeg_needs_a_target_and_a_folder() {
    let output = run(bash("build-ffmpeg.sh", &["x86_64-unknown-linux-gnu"]));

    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("usage"), "{}", stderr(&output));
}

// ---- stage-sidecars.sh -------------------------------------------------------------------------------------------------------

fn stage(args: &[&str]) -> Output {
    run(bash("stage-sidecars.sh", args))
}

/// The files that a dry run says it would copy to, in the order it says them.
fn destinations(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter_map(|line| line.strip_prefix("copy "))
        .map(|rest| {
            rest.split(" -> ")
                .nth(1)
                .expect("a copy has a target")
                .to_owned()
        })
        .collect()
}

#[test]
fn stage_sidecars_names_files_with_the_target_triple() {
    // Tauri looks for `<name>-<triple>` (and `.exe` on Windows) of each `externalBin`, and puts the file in the package without
    // the triple.
    for (triple, exe) in [
        ("aarch64-apple-darwin", ""),
        ("x86_64-apple-darwin", ""),
        ("x86_64-unknown-linux-gnu", ""),
        ("x86_64-pc-windows-msvc", ".exe"),
    ] {
        let output = stage(&["--dry-run", triple]);

        assert_eq!(code(&output), 0, "{triple}: {}", stderr(&output));
        let destinations = destinations(&output);
        for name in ["dictate-mcp", "dictate-ffmpeg"] {
            let expected = format!("app/src-tauri/binaries/{name}-{triple}{exe}");
            assert!(
                destinations.contains(&expected),
                "{triple}: {expected} in {destinations:?}"
            );
        }
        // And the build of the MCP server is for that target.
        assert!(
            stdout(&output).contains(&format!(
                "cargo build -p dictate-mcp --release --locked --target {triple}"
            )),
            "{}",
            stdout(&output)
        );
    }
}

#[test]
fn stage_sidecars_takes_the_mcp_server_from_the_folder_of_the_target() {
    let output = stage(&["--dry-run", "aarch64-apple-darwin"]);

    assert!(
        stdout(&output).contains("copy target/aarch64-apple-darwin/release/dictate-mcp -> "),
        "{}",
        stdout(&output)
    );
    // The ffmpeg is the one build-ffmpeg.sh wrote for the target.
    assert!(
        stdout(&output).contains("copy target/ffmpeg/aarch64-apple-darwin/dictate-ffmpeg -> "),
        "{}",
        stdout(&output)
    );
}

#[test]
fn stage_sidecars_puts_the_sherpa_libraries_of_linux_in_the_libs_folder_and_only_there() {
    let linux = destinations(&stage(&["--dry-run", "x86_64-unknown-linux-gnu"]));
    for library in ["libsherpa-onnx-c-api.so", "libonnxruntime.so"] {
        assert!(
            linux.contains(&format!("app/src-tauri/libs/{library}")),
            "{library} in {linux:?}"
        );
    }

    for triple in ["aarch64-apple-darwin", "x86_64-pc-windows-msvc"] {
        let others = destinations(&stage(&["--dry-run", triple]));
        assert!(
            !others.iter().any(|path| path.ends_with(".so")),
            "{triple}: {others:?}"
        );
    }
}

#[test]
fn stage_sidecars_puts_the_runtime_of_visual_c_for_windows_in_the_libs_folder_and_only_there() {
    let windows = destinations(&stage(&["--dry-run", "x86_64-pc-windows-msvc"]));
    for dll in ["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"] {
        assert!(
            windows.contains(&format!("app/src-tauri/libs/{dll}")),
            "{dll} in {windows:?}"
        );
    }

    for triple in ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"] {
        let others = destinations(&stage(&["--dry-run", triple]));
        assert!(
            !others.iter().any(|path| path.ends_with(".dll")),
            "{triple}: {others:?}"
        );
    }
}

#[test]
fn stage_sidecars_refuses_a_target_it_does_not_know() {
    let output = stage(&["--dry-run", "riscv64-unknown-linux-gnu"]);

    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("riscv64-unknown-linux-gnu"),
        "{}",
        stderr(&output)
    );
    assert_eq!(code(&stage(&[])), 1);
}

#[test]
fn stage_sidecars_stops_before_it_builds_anything_when_the_ffmpeg_is_not_there() {
    // The build of the MCP server takes minutes: what is already known to be missing is found first.
    let work = tempfile::tempdir().unwrap();
    let nowhere = work.path().join("no-ffmpeg-here");

    let output = stage(&["aarch64-apple-darwin", nowhere.to_str().unwrap()]);

    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("no-ffmpeg-here"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stdout(&output).contains("cargo build"),
        "{}",
        stdout(&output)
    );
}

/// A copy of the layout that `stage-sidecars.sh` works in (it works from the root of the repository that it is in): the script, the MCP
/// server that the build would have made and the ffmpeg of `build-ffmpeg.sh`, with the permissions `mode` (what a file that went
/// through an artifact of Actions has is 0644).
fn staging_repo(triple: &str, mode: u32) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join("scripts")).unwrap();
    fs::copy(
        scripts().join("stage-sidecars.sh"),
        repo.path().join("scripts/stage-sidecars.sh"),
    )
    .unwrap();
    for (relative, text) in [
        (
            format!("target/{triple}/release/dictate-mcp"),
            "the new mcp",
        ),
        (
            format!("target/ffmpeg/{triple}/dictate-ffmpeg"),
            "the new ffmpeg",
        ),
        // The license files that go in every installer, from the root of the repository.
        ("LICENSE".to_owned(), "the MIT license"),
        ("THIRD_PARTY_NOTICES.md".to_owned(), "the notices"),
        ("THIRD_PARTY_LICENSES.txt".to_owned(), "the license texts"),
        ("COPYING.LGPLv2.1".to_owned(), "the LGPL"),
    ] {
        let path = repo.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }
    repo
}

/// Runs the script in `repo` for `triple`, with a `cargo` that does nothing (the build of the MCP server is not what is under test: it
/// leaves the file that is in target/ as it is).
fn stage_in(repo: &Path, triple: &str) -> Output {
    let tools = tempfile::tempdir().unwrap();
    let cargo = tools.path().join("cargo");
    fs::write(&cargo, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    run({
        let mut command = Command::new("bash");
        command
            .arg(repo.join("scripts/stage-sidecars.sh"))
            .arg(triple)
            .env("PATH", path);
        // Linux stages the libraries of sherpa-onnx too: the folder that a test made for them, when it made one.
        if repo.join("sherpa").is_dir() {
            command.env("SHERPA_ONNX_LIB_DIR", repo.join("sherpa"));
        }
        command
    })
}

#[test]
fn stage_sidecars_replaces_a_sidecar_that_is_there_with_a_new_file_and_does_not_write_over_it() {
    // macOS kills a program that was written over in place after it had been run or signed (the kernel keeps what it knows of the file by
    // its inode): the file is removed and copied anew, so that it is a new one. A second name for each old file (a hard link) keeps its
    // inode alive: a file that is written over in place changes what that name reads, and a file that was removed and made again does not.
    // (Comparing inode numbers is not enough: a file system hands the number of a file that was just removed to the next one.)
    let triple = "aarch64-apple-darwin";
    let repo = staging_repo(triple, 0o755);
    let old_mcp = repo
        .path()
        .join(format!("app/src-tauri/binaries/dictate-mcp-{triple}"));
    let old_ffmpeg = repo
        .path()
        .join(format!("app/src-tauri/binaries/dictate-ffmpeg-{triple}"));
    fs::create_dir_all(old_mcp.parent().unwrap()).unwrap();
    fs::write(&old_mcp, "the old mcp").unwrap();
    fs::write(&old_ffmpeg, "the old ffmpeg").unwrap();
    let kept_mcp = repo.path().join("kept-mcp");
    let kept_ffmpeg = repo.path().join("kept-ffmpeg");
    fs::hard_link(&old_mcp, &kept_mcp).unwrap();
    fs::hard_link(&old_ffmpeg, &kept_ffmpeg).unwrap();

    let output = stage_in(repo.path(), triple);

    assert_eq!(code(&output), 0, "{}{}", stdout(&output), stderr(&output));
    assert_eq!(fs::read_to_string(&old_mcp).unwrap(), "the new mcp");
    assert_eq!(fs::read_to_string(&old_ffmpeg).unwrap(), "the new ffmpeg");
    assert_eq!(
        fs::read_to_string(&kept_mcp).unwrap(),
        "the old mcp",
        "dictate-mcp was written over in place"
    );
    assert_eq!(
        fs::read_to_string(&kept_ffmpeg).unwrap(),
        "the old ffmpeg",
        "dictate-ffmpeg was written over in place"
    );
}

#[test]
fn stage_sidecars_makes_the_sidecars_executable_whatever_the_files_it_copies_were() {
    // The bundler copies the permissions of the file into the package: a sidecar that came without the bit (a file that went through
    // upload-artifact and download-artifact has none) would be in the installer as a file that cannot run.
    for triple in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ] {
        let repo = staging_repo(triple, 0o644);
        // Linux also stages the libraries of sherpa-onnx, which have to be there.
        let sherpa = repo.path().join("sherpa");
        fs::create_dir_all(&sherpa).unwrap();
        for library in ["libsherpa-onnx-c-api.so", "libonnxruntime.so"] {
            fs::write(sherpa.join(library), "a library").unwrap();
        }

        let output = stage_in(repo.path(), triple);

        assert_eq!(
            code(&output),
            0,
            "{triple}: {}{}",
            stdout(&output),
            stderr(&output)
        );
        for name in ["dictate-mcp", "dictate-ffmpeg"] {
            let staged = repo
                .path()
                .join(format!("app/src-tauri/binaries/{name}-{triple}"));
            let mode = fs::metadata(&staged).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "{triple}: {name} is {mode:o}");
        }
    }
}

#[test]
fn stage_sidecars_puts_the_license_files_of_every_system_in_the_licenses_folder() {
    // The release overlays put them in the installers from there: macOS in Contents/Resources/Licenses, Windows in Licenses, the deb in
    // /usr/share/doc/dictate.
    for triple in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "x86_64-pc-windows-msvc",
    ] {
        let destinations = destinations(&stage(&["--dry-run", triple]));
        for name in [
            "LICENSE",
            "THIRD_PARTY_NOTICES.md",
            "THIRD_PARTY_LICENSES.txt",
            "COPYING.LGPLv2.1",
        ] {
            let expected = format!("app/src-tauri/licenses/{name}");
            assert!(
                destinations.contains(&expected),
                "{triple}: {expected} in {destinations:?}"
            );
        }
    }
}

#[test]
fn stage_sidecars_copies_the_license_files_from_the_root_of_the_repository() {
    let triple = "aarch64-apple-darwin";
    let repo = staging_repo(triple, 0o755);

    let output = stage_in(repo.path(), triple);

    assert_eq!(code(&output), 0, "{}{}", stdout(&output), stderr(&output));
    let licenses = repo.path().join("app/src-tauri/licenses");
    assert_eq!(
        fs::read_to_string(licenses.join("COPYING.LGPLv2.1")).unwrap(),
        "the LGPL"
    );
    assert_eq!(
        fs::read_to_string(licenses.join("THIRD_PARTY_LICENSES.txt")).unwrap(),
        "the license texts"
    );
}

#[test]
fn stage_sidecars_says_how_to_make_the_license_texts_when_they_are_not_there() {
    // The file of the license texts is generated: a checkout that never ran the generator does not have a current one, and the build of an
    // installer stops before it builds anything.
    let triple = "aarch64-apple-darwin";
    let repo = staging_repo(triple, 0o755);
    fs::remove_file(repo.path().join("THIRD_PARTY_LICENSES.txt")).unwrap();

    let output = stage_in(repo.path(), triple);

    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("THIRD_PARTY_LICENSES.txt")
            && stderr(&output).contains("scripts/generate-licenses.py"),
        "{}",
        stderr(&output)
    );
}
