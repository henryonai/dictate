//! `scripts/check-artifact.sh`: the check that every installer goes through before it is attached to a release.
//!
//! A release whose installer has no `loah-ffmpeg` (the app would look for the ffmpeg of the user, which is not LGPL-checked), a GPL
//! ffmpeg, espeak-ng (GPL-3.0) in a library, a `/usr/bin/ffmpeg` that collides with the package of the distribution, or a program that
//! cannot find its libraries must fail the release job instead of being published. These tests run the script on trees that look like what
//! an installer unpacks to (it takes such a folder as well as an installer, for these tests), with stand-ins for the programs and the tools
//! of the system that it asks: `readelf`, `plutil`, `codesign`, `lipo` and `vtool` (and `dpkg-deb`, `7z`, `hdiutil` and `ditto` for the part
//! that unpacks an installer). The real tools are tried by the dry run of the release workflow, which checks real installers.
//!
//! The scripts are for bash, which CI runs on every system: these tests run where `bash` is the real one.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-artifact.sh")
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

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn write_program(path: &Path, text: &str) {
    write(path, text);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---- the stand-ins for what an installer has ---------------------------------------------------------------------------------

const LGPL_2_1: &str = "FFmpeg is free software; you can redistribute it and/or
modify it under the terms of the GNU Lesser General Public
License as published by the Free Software Foundation; either
version 2.1 of the License, or (at your option) any later version.";

const PLAIN_BUILDCONF: &str = "    --disable-gpl
    --disable-nonfree
    --disable-version3
    --disable-everything";

const GPL_BUILDCONF: &str = "    --enable-gpl
    --enable-libx264
    --disable-everything";

const ENCODERS: &str = "Encoders:
 V..... = Video
 ------
 V....D libvpx-vp9           libvpx VP9";

/// A program that answers as an ffmpeg does (the way the tests of `check-ffmpeg-lgpl.sh` do it).
fn ffmpeg(buildconf: &str) -> String {
    format!(
        "#!/bin/sh\n# LGPL version 2.1 or later\ncase \"$*\" in\n*-buildconf*) cat <<'EOF'\n{buildconf}\nEOF\n;;\n*-L*) cat <<'EOF'\n{LGPL_2_1}\nEOF\n;;\n*-encoders*) cat <<'EOF'\n{ENCODERS}\nEOF\n;;\n*) echo 'ffmpeg version 9.0.2' ;;\nesac\n"
    )
}

const SHERPA_LIBRARIES: [&str; 2] = ["libsherpa-onnx-c-api.so", "libonnxruntime.so"];
/// The license files that every installer carries (the release overlay puts them there): the MIT license of Loah, the notices and the
/// license texts of what is inside, and the LGPL text that FFmpeg and glibc ask for.
const LICENSE_FILES: [&str; 4] = [
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "THIRD_PARTY_LICENSES.txt",
    "COPYING.LGPLv2.1",
];

/// The license files in `folder`.
fn licenses(folder: &Path) {
    for name in LICENSE_FILES {
        write(&folder.join(name), "a license text\n");
    }
}
const VC_DLLS: [&str; 4] = [
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
];

/// The three fields of `DEBIAN/control` that make the package take the place of the old `dictate` package.
const REPLACES_DICTATE: &str = "Provides: dictate\nConflicts: dictate\nReplaces: dictate\n";

/// What an unpacked `.deb` has: `usr/bin/loah` and its two sidecars, the two wrappers that keep the old names of the programs working, the
/// two libraries, and the `DEBIAN/control` that `dpkg-deb -R` writes.
fn deb(root: &Path) {
    write_program(&root.join("usr/bin/loah"), "#!/bin/sh\necho loah\n");
    write_program(&root.join("usr/bin/loah-mcp"), "#!/bin/sh\necho loah-mcp\n");
    write_program(&root.join("usr/bin/loah-ffmpeg"), &ffmpeg(PLAIN_BUILDCONF));
    // What the MCP clients and the shortcuts that were set up for Dictate 2.0.0 run: two lines that hand over to the new programs.
    write_program(
        &root.join("usr/bin/dictate"),
        "#!/bin/sh\nexec /usr/bin/loah \"$@\"\n",
    );
    write_program(
        &root.join("usr/bin/dictate-mcp"),
        "#!/bin/sh\nexec /usr/bin/loah-mcp \"$@\"\n",
    );
    for library in SHERPA_LIBRARIES {
        write(&root.join("usr/lib/loah").join(library), "ELF stand-in\n");
    }
    // The documentation folder of the package: the license files, and the `copyright` file that the tools of Debian read.
    licenses(&root.join("usr/share/doc/loah"));
    write(
        &root.join("usr/share/doc/loah/copyright"),
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\nUpstream-Name: Loah\n",
    );
    write(
        &root.join("DEBIAN/control"),
        &format!("Package: loah\nVersion: 2.1.0\nArchitecture: amd64\nDepends: libwebkit2gtk-4.1-0, libgtk-3-0, libasound2t64 | libasound2, libpipewire-0.3-0t64 | libpipewire-0.3-0\nRecommends: pulseaudio-utils\n{REPLACES_DICTATE}Description: Dictation and meeting notes\n"),
    );
}

/// What the NSIS installer unpacks to: the program, its sidecars and the runtime of Visual C++ beside it.
fn windows(root: &Path) {
    // The names in the import table of a real loah.exe (it always has KERNEL32.dll): the check reads them from the bytes.
    write_program(
        &root.join("loah.exe"),
        "MZ stand-in\0KERNEL32.dll\0VCRUNTIME140.dll\0",
    );
    write_program(&root.join("loah-mcp.exe"), "MZ stand-in\n");
    write_program(&root.join("loah-ffmpeg.exe"), &ffmpeg(PLAIN_BUILDCONF));
    for dll in VC_DLLS {
        write(&root.join(dll), "MZ stand-in\n");
    }
    write(&root.join("uninstall.exe"), "MZ stand-in\n");
    licenses(&root.join("Licenses"));
}

/// What the volume of the `.dmg` has: the app (with its sidecars in `Contents/MacOS`) and the link to /Applications.
fn mac(root: &Path) {
    let app = root.join("Loah.app/Contents");
    write_program(&app.join("MacOS/loah"), "#!/bin/sh\necho loah\n");
    write_program(&app.join("MacOS/loah-mcp"), "#!/bin/sh\necho loah-mcp\n");
    write_program(&app.join("MacOS/loah-ffmpeg"), &ffmpeg(PLAIN_BUILDCONF));
    write(
        &app.join("Info.plist"),
        "CFBundleExecutable=loah\nNSMicrophoneUsageDescription=Loah records your voice to turn it into text.\nNSScreenCaptureUsageDescription=Loah records your screen.\nLSMinimumSystemVersion=13.0\n",
    );
    licenses(&app.join("Resources/Licenses"));
    std::os::unix::fs::symlink("/Applications", root.join("Applications")).unwrap();
}

// ---- the stand-ins for the tools of the system -------------------------------------------------------------------------------

/// What `readelf -d` prints for a program that has the run path of the package, and only that (the release build is made with
/// `LOAH_RPATH_INSTALLED=1`).
const RUNPATH_OF_THE_PACKAGE: &str = "Dynamic section at offset 0x4a1b20 contains 41 entries:
  Tag        Type                         Name/Value
 0x0000000000000001 (NEEDED)             Shared library: [libsherpa-onnx-c-api.so]
 0x0000000000000001 (NEEDED)             Shared library: [libpipewire-0.3.so.0]
 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN/../lib/loah]";

/// The folder with the stand-ins, put first in the PATH of the script, and what they are told through the environment.
struct Tools {
    bin: tempfile::TempDir,
    log: PathBuf,
    vars: Vec<(String, String)>,
    /// The variables that the script must not get, whatever the environment of the test has.
    removed: Vec<String>,
}

impl Tools {
    fn new() -> Tools {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("calls.log");
        // `readelf -d FILE` prints what the test says; the stand-in is told through FAKE_READELF.
        write_program(
            &bin.path().join("readelf"),
            "#!/bin/sh\nprintf '%s\\n' \"$FAKE_READELF\"\n",
        );
        // `plutil -extract KEY raw -o - FILE`: the stand-in plist is lines of KEY=VALUE.
        write_program(
            &bin.path().join("plutil"),
            "#!/bin/sh\nkey=\"$2\"\nfor file; do :; done\nvalue=\"$(grep \"^$key=\" \"$file\" | head -n 1 | cut -d= -f2-)\"\n[ -n \"$value\" ] || { echo \"No value at that key path or invalid key path: $key\" >&2; exit 1; }\nprintf '%s\\n' \"$value\"\n",
        );
        // `codesign --verify --strict PATH` fails for the paths whose last part FAKE_CODESIGN_FAIL names (blank separated).
        write_program(
            &bin.path().join("codesign"),
            "#!/bin/sh\nfor path; do :; done\necho \"codesign $*\" >> \"$FAKE_LOG\"\nfor name in $FAKE_CODESIGN_FAIL; do\n  [ \"$(basename \"$path\")\" = \"$name\" ] && { echo \"$path: invalid signature\" >&2; exit 1; }\ndone\nexit 0\n",
        );
        // `lipo -archs FILE`: FAKE_LIPO is `name=arch` pairs (blank separated; `x86_64+arm64` is a universal file) for the files that have
        // another architecture than FAKE_LIPO_DEFAULT (`arm64` unless a test says).
        write_program(
            &bin.path().join("lipo"),
            "#!/bin/sh\nfor file; do :; done\nname=\"$(basename \"$file\")\"\nfor pair in $FAKE_LIPO; do\n  case \"$pair\" in \"$name\"=*) echo \"${pair#*=}\" | tr '+' ' '; exit 0 ;; esac\ndone\necho \"${FAKE_LIPO_DEFAULT:-arm64}\"\n",
        );
        // `vtool -show-build FILE` prints what vtool prints (the first line is the name of the file); FAKE_MINOS is `name=version` pairs
        // for the files that have another minimum system than 13.0.
        write_program(
            &bin.path().join("vtool"),
            "#!/bin/sh\nfor file; do :; done\nname=\"$(basename \"$file\")\"\nminos=13.0\nfor pair in $FAKE_MINOS; do\n  case \"$pair\" in \"$name\"=*) minos=\"${pair#*=}\" ;; esac\ndone\nprintf '%s:\\nLoad command 11\\n      cmd LC_BUILD_VERSION\\n  cmdsize 32\\n platform MACOS\\n    minos %s\\n      sdk 26.5\\n   ntools 1\\n     tool LD\\n  version 1267.0\\n' \"$file\" \"$minos\"\n",
        );
        let vars = vec![
            ("FAKE_READELF".to_owned(), RUNPATH_OF_THE_PACKAGE.to_owned()),
            ("FAKE_LOG".to_owned(), log.display().to_string()),
            // What a folder is checked for (a `.dmg` says it in its name).
            ("LOAH_ARTIFACT_ARCH".to_owned(), "arm64".to_owned()),
        ];
        Tools {
            bin,
            log,
            vars,
            removed: Vec::new(),
        }
    }

    fn without(mut self, name: &str) -> Tools {
        self.vars.retain(|(known, _)| known != name);
        self.removed.push(name.to_owned());
        self
    }

    fn with(mut self, name: &str, value: &str) -> Tools {
        self.vars.retain(|(known, _)| known != name);
        self.vars.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The tools that unpack an installer: each copies the tree in FAKE_TREE to where the real one would put the files.
    fn unpacking(self, tree: &Path) -> Tools {
        let bin = self.bin.path();
        // `dpkg-deb -R FILE DIR`
        write_program(
            &bin.join("dpkg-deb"),
            "#!/bin/sh\necho \"dpkg-deb $*\" >> \"$FAKE_LOG\"\n[ \"$1\" = -R ] || exit 2\ncp -R \"$FAKE_TREE/.\" \"$3\"\n",
        );
        // `7z x -y -oDIR FILE`
        write_program(
            &bin.join("7z"),
            "#!/bin/sh\necho \"7z $*\" >> \"$FAKE_LOG\"\n[ \"$1\" = x ] || exit 2\nfor arg; do case \"$arg\" in -o*) dir=\"${arg#-o}\" ;; esac; done\nmkdir -p \"$dir\"\ncp -R \"$FAKE_TREE/.\" \"$dir\"\n",
        );
        // `hdiutil attach ... -mountpoint DIR FILE` and `hdiutil detach DIR`
        write_program(
            &bin.join("hdiutil"),
            "#!/bin/sh\necho \"hdiutil $*\" >> \"$FAKE_LOG\"\ncase \"$1\" in\n  attach) while [ \"$#\" -gt 0 ]; do [ \"$1\" = -mountpoint ] && dir=\"$2\"; shift; done\n    mkdir -p \"$dir\"; cp -R \"$FAKE_TREE/.\" \"$dir\" ;;\n  detach) ;;\n  *) exit 2 ;;\nesac\n",
        );
        // `ditto SOURCE DESTINATION`
        write_program(&bin.join("ditto"), "#!/bin/sh\ncp -R \"$1\" \"$2\"\n");
        self.with("FAKE_TREE", &tree.display().to_string())
    }

    fn run(&self, artifact: &Path) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("bash");
        command.arg(script()).arg(artifact).env("PATH", path);
        for name in &self.removed {
            command.env_remove(name);
        }
        for (name, value) in &self.vars {
            command.env(name, value);
        }
        command.output().expect("bash runs the script")
    }

    /// Runs the script where `missing` is not to be found: the PATH is the stand-ins (except that one) and links to the ordinary tools that
    /// the scripts need, so that the real `lipo` or `vtool` of a Mac is not there either.
    fn run_without(&self, artifact: &Path, missing: &str) -> Output {
        let ordinary = tempfile::tempdir().unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        for name in [
            "bash", "sh", "env", "cat", "cp", "rm", "mkdir", "mktemp", "ls", "head", "tail", "tr",
            "sed", "grep", "awk", "cut", "sort", "wc", "find", "dirname", "basename", "od",
            "uname", "mv", "chmod", "xargs", "tee", "readlink", "date", "expr", "test",
        ] {
            if let Some(found) = std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
            {
                std::os::unix::fs::symlink(found, ordinary.path().join(name)).unwrap();
            }
        }
        let _ = fs::remove_file(self.bin.path().join(missing));
        let path = format!(
            "{}:{}",
            self.bin.path().display(),
            ordinary.path().display()
        );
        let mut command = Command::new("bash");
        command.arg(script()).arg(artifact).env("PATH", path);
        for (name, value) in &self.vars {
            command.env(name, value);
        }
        command.output().expect("bash runs the script")
    }

    /// What the stand-ins were asked, one call to a line.
    fn calls(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn tree(build: fn(&Path)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path());
    dir
}

/// Runs the script on a tree, with the ordinary stand-ins, and checks that it says `fails` (the parts of what it has to say) and exits 1.
fn assert_fails(output: &Output, said: &[&str]) {
    assert_eq!(code(output), 1, "{}{}", stdout(output), stderr(output));
    for part in said {
        assert!(
            stderr(output).contains(part),
            "{part:?} is not in what it said:\n{}",
            stderr(output)
        );
    }
}

fn assert_passes(output: &Output) {
    assert_eq!(code(output), 0, "{}{}", stdout(output), stderr(output));
}

// ---- a package for Linux -----------------------------------------------------------------------------------------------------

#[test]
fn a_complete_deb_passes() {
    let root = tree(deb);

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn it_lists_what_the_installer_has_with_the_size_of_each_file_so_that_the_log_of_a_release_shows_it(
) {
    // The log of the job is where a release is looked at afterwards: what was in the installer, and how big.
    let root = tree(deb);

    let output = Tools::new().run(root.path());

    assert_passes(&output);
    let listing = stdout(&output);
    for file in [
        "./usr/bin/loah",
        "./usr/bin/loah-ffmpeg",
        "./usr/lib/loah/libonnxruntime.so",
    ] {
        assert!(
            listing.lines().any(|line| line.trim_end().ends_with(file)),
            "{file} is not in:\n{listing}"
        );
    }
    // The size comes first: the length of the stand-in for the libraries ("ELF stand-in\n") is 13 bytes.
    assert!(
        listing
            .lines()
            .any(|line| line.trim_start().starts_with("13 ") && line.ends_with("libonnxruntime.so")),
        "{listing}"
    );
}

#[test]
fn a_deb_without_the_ffmpeg_sidecar_fails_and_says_which_file_is_missing() {
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/bin/loah-ffmpeg")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["loah-ffmpeg"]);
}

#[test]
fn a_deb_without_the_mcp_sidecar_fails_and_says_which_file_is_missing() {
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/bin/loah-mcp")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["loah-mcp"]);
}

#[test]
fn a_sidecar_that_is_not_executable_fails() {
    // upload-artifact and download-artifact drop the permission bits: a sidecar that was carried that way is a file that cannot run.
    let root = tree(deb);
    let ffmpeg = root.path().join("usr/bin/loah-ffmpeg");
    fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o644)).unwrap();

    assert_fails(
        &Tools::new().run(root.path()),
        &["loah-ffmpeg", "not executable"],
    );
}

#[test]
fn a_gpl_ffmpeg_fails() {
    let root = tree(deb);
    write_program(
        &root.path().join("usr/bin/loah-ffmpeg"),
        &ffmpeg(GPL_BUILDCONF),
    );

    assert_fails(
        &Tools::new().run(root.path()),
        &["loah-ffmpeg", "--enable-gpl"],
    );
}

#[test]
fn espeak_in_a_shared_library_fails_and_names_the_library() {
    let root = tree(deb);
    write(
        &root.path().join("usr/lib/loah/libsherpa-onnx-c-api.so"),
        "ELF stand-in with espeak_ng_Initialize in it\n",
    );

    assert_fails(
        &Tools::new().run(root.path()),
        &["espeak", "libsherpa-onnx-c-api.so"],
    );
}

#[test]
fn espeak_in_the_program_or_a_sidecar_fails() {
    for program in ["loah", "loah-mcp"] {
        let root = tree(deb);
        write_program(
            &root.path().join("usr/bin").join(program),
            "#!/bin/sh\n# ESPEAK_DATA_PATH\n",
        );

        assert_fails(&Tools::new().run(root.path()), &["espeak", program]);
    }
}

#[test]
fn espeak_in_a_library_of_any_kind_fails() {
    // The check covers every .so, .dylib and .dll that the installer has, wherever it is.
    for library in [
        "usr/lib/loah/libother.so.1",
        "usr/lib/elsewhere/libother.dylib",
        "usr/share/loah/other.dll",
    ] {
        let root = tree(deb);
        write(&root.path().join(library), "espeak_ng_Initialize\n");

        assert_fails(&Tools::new().run(root.path()), &["espeak", library]);
    }
}

#[test]
fn a_deb_that_has_usr_bin_ffmpeg_fails() {
    // It would collide with the ffmpeg package of Debian and Ubuntu: dpkg refuses to overwrite it.
    let root = tree(deb);
    write_program(&root.path().join("usr/bin/ffmpeg"), "#!/bin/sh\n");

    assert_fails(&Tools::new().run(root.path()), &["/usr/bin/ffmpeg"]);
}

#[test]
fn a_deb_without_one_of_the_sherpa_libraries_fails_and_says_which() {
    for library in SHERPA_LIBRARIES {
        let root = tree(deb);
        fs::remove_file(root.path().join("usr/lib/loah").join(library)).unwrap();

        assert_fails(&Tools::new().run(root.path()), &["/usr/lib/loah", library]);
    }
}

#[test]
fn a_deb_whose_program_has_no_run_path_to_the_libraries_fails() {
    // The run path that finds the libraries in /usr/lib/loah from /usr/bin/loah. The ones of a build in the source tree
    // ($ORIGIN, $ORIGIN/..) do not: without it the installed app does not start.
    for dynamic in [
        " 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN:$ORIGIN/..]",
        " 0x000000000000000f (RPATH)              Library rpath: [$ORIGIN/../lib/loah-old]",
        " 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]",
    ] {
        let root = tree(deb);

        let output = Tools::new().with("FAKE_READELF", dynamic).run(root.path());

        assert_fails(&output, &["$ORIGIN/../lib/loah"]);
    }
}

#[test]
fn a_deb_whose_program_still_has_the_run_paths_of_a_build_in_the_source_tree_fails() {
    // $ORIGIN/.. of /usr/bin/loah is /usr, and $ORIGIN/../../../.. is /: a program that looks there loads a library that somebody put there.
    for extra in ["$ORIGIN", "$ORIGIN/..", "$ORIGIN/../../../..", "/opt/other"] {
        let root = tree(deb);
        let dynamic = format!(
            " 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN/../lib/loah:{extra}]"
        );

        let output = Tools::new().with("FAKE_READELF", &dynamic).run(root.path());

        assert_fails(&output, &[extra, "only $ORIGIN/../lib/loah"]);
    }
}

#[test]
fn a_deb_with_the_run_path_as_an_old_style_rpath_passes() {
    let root = tree(deb);

    let output = Tools::new()
        .with(
            "FAKE_READELF",
            " 0x000000000000000f (RPATH)              Library rpath: [$ORIGIN/../lib/loah]",
        )
        .run(root.path());

    assert_passes(&output);
}

#[test]
fn a_deb_that_does_not_depend_on_pipewire_fails() {
    let root = tree(deb);
    write(
        &root.path().join("DEBIAN/control"),
        &format!("Package: loah\nDepends: libwebkit2gtk-4.1-0, libgtk-3-0, libasound2t64 | libasound2\n{REPLACES_DICTATE}"),
    );

    assert_fails(&Tools::new().run(root.path()), &["libpipewire"]);
}

#[test]
fn a_dependency_on_pipewire_on_a_second_line_of_the_field_counts() {
    // Debian control files fold a long field: the lines after the first start with a blank.
    let root = tree(deb);
    write(
        &root.path().join("DEBIAN/control"),
        &format!("Package: loah\nDepends: libwebkit2gtk-4.1-0,\n libgtk-3-0,\n libpipewire-0.3-0t64 | libpipewire-0.3-0\nRecommends: pulseaudio-utils\n{REPLACES_DICTATE}"),
    );

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn pipewire_in_recommends_is_not_a_dependency() {
    let root = tree(deb);
    write(
        &root.path().join("DEBIAN/control"),
        &format!(
            "Package: loah\nDepends: libgtk-3-0\nRecommends: libpipewire-0.3-0\n{REPLACES_DICTATE}"
        ),
    );

    assert_fails(&Tools::new().run(root.path()), &["libpipewire"]);
}

#[test]
fn a_deb_file_is_unpacked_with_dpkg_deb_and_its_tree_is_checked() {
    let root = tree(deb);
    let work = tempfile::tempdir().unwrap();
    let package = work.path().join("Loah_2.1.0_amd64.deb");
    fs::write(&package, "!<arch>\n").unwrap();
    let tools = Tools::new().unpacking(root.path());

    assert_passes(&tools.run(&package));

    assert!(tools.calls().contains("dpkg-deb -R "), "{}", tools.calls());
    // And a package whose tree is not complete fails the same way when it is a file.
    fs::remove_file(root.path().join("usr/bin/loah-ffmpeg")).unwrap();
    assert_fails(&tools.run(&package), &["loah-ffmpeg"]);
}

// ---- the names of Dictate 2.0.0 that the package keeps working -----------------------------------------------------------------

#[test]
fn a_deb_without_the_dictate_mcp_wrapper_fails() {
    // An MCP client that was set up for Dictate runs /usr/bin/dictate-mcp: without the wrapper it finds nothing after the upgrade.
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/bin/dictate-mcp")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["/usr/bin/dictate-mcp"]);
}

#[test]
fn a_deb_without_the_dictate_wrapper_fails() {
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/bin/dictate")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["/usr/bin/dictate"]);
}

#[test]
fn a_wrapper_that_is_not_executable_fails() {
    // The mode of a file is what the Tauri bundler keeps of the file in the repository: a wrapper that lost it is a command that cannot run.
    for wrapper in ["dictate", "dictate-mcp"] {
        let root = tree(deb);
        let path = root.path().join("usr/bin").join(wrapper);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert_fails(
            &Tools::new().run(root.path()),
            &[&format!("/usr/bin/{wrapper}"), "not executable"],
        );
    }
}

#[test]
fn a_wrapper_that_hands_over_to_the_wrong_program_fails() {
    // dictate-mcp has to run loah-mcp: a copy-and-paste that ran `loah` would start the app, and the MCP client would read its silence.
    let root = tree(deb);
    write_program(
        &root.path().join("usr/bin/dictate-mcp"),
        "#!/bin/sh\nexec /usr/bin/loah \"$@\"\n",
    );

    assert_fails(
        &Tools::new().run(root.path()),
        &["/usr/bin/dictate-mcp", "exec /usr/bin/loah-mcp"],
    );
}

#[test]
fn the_deb_replaces_dictate() {
    // The package takes the place of the old one: installing it removes `dictate`, and what depends on `dictate` is still satisfied.
    let root = tree(deb);
    assert_passes(&Tools::new().run(root.path()));

    for field in ["Provides", "Conflicts", "Replaces"] {
        let root = tree(deb);
        let control = root.path().join("DEBIAN/control");
        let without: String = fs::read_to_string(&control)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with(field))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(&control, without).unwrap();

        assert_fails(&Tools::new().run(root.path()), &[field, "dictate"]);
    }
}

#[test]
fn a_deb_that_is_not_the_package_loah_fails() {
    let root = tree(deb);
    let control = root.path().join("DEBIAN/control");
    let text = fs::read_to_string(&control)
        .unwrap()
        .replace("Package: loah\n", "Package: dictate\n");
    fs::write(&control, text).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["Package", "loah"]);
}

// ---- the installer for Windows -----------------------------------------------------------------------------------------------

#[test]
fn a_complete_windows_tree_passes() {
    let root = tree(windows);

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn a_windows_tree_without_a_sidecar_fails_and_says_which() {
    for sidecar in ["loah-mcp.exe", "loah-ffmpeg.exe"] {
        let root = tree(windows);
        fs::remove_file(root.path().join(sidecar)).unwrap();

        assert_fails(&Tools::new().run(root.path()), &[sidecar]);
    }
}

#[test]
fn a_windows_tree_without_one_of_the_runtime_dlls_fails_and_says_which() {
    // A Windows without the runtime of Visual C++ does not start the app: the four DLLs go beside loah.exe.
    for dll in VC_DLLS {
        let root = tree(windows);
        fs::remove_file(root.path().join(dll)).unwrap();

        assert_fails(&Tools::new().run(root.path()), &[dll]);
    }
}

#[test]
fn a_windows_program_that_asks_for_a_runtime_dll_that_is_not_beside_it_fails_and_names_both() {
    // The first dry run's loah.exe imported msvcp140_1.dll (std::pmr), and the installer had only the three that the list named then: a Windows
    // without the runtime would not have started it. The list has the four now; what a program asks for is read from its bytes (in the capitals of
    // the import table), and that is what finds the next one: msvcp140_2.dll is not in the list, so only the reading of the bytes can say it is
    // missing, and its message ("asks for") is the one that the list does not have.
    for dll in ["msvcp140_1.dll", "msvcp140_2.dll"] {
        let imports = format!(
            "MZ stand-in\0KERNEL32.dll\0MSVCP140.dll\0{}\0VCRUNTIME140.dll\0",
            dll.to_uppercase().replace(".DLL", ".dll")
        );
        let root = tree(windows);
        write_program(&root.path().join("loah.exe"), &imports);
        let _ = fs::remove_file(root.path().join(dll));
        // A file that needs only what is there passes: the same program, with the DLL beside it.
        let complete = tree(windows);
        write_program(&complete.path().join("loah.exe"), &imports);
        write(&complete.path().join(dll), "MZ stand-in\n");

        assert_fails(
            &Tools::new().run(root.path()),
            &[&format!("loah.exe asks for {dll}, which is not next to it")],
        );
        assert_passes(&Tools::new().run(complete.path()));
    }
}

#[test]
fn a_windows_program_from_which_no_dll_name_can_be_read_is_not_a_pass() {
    // The reading of the bytes finds nothing in a file that is not what it was taken for (a stub, a packed file, names in another coding):
    // a loah.exe that has no name of any DLL is not a clean one, it is one that was not checked. Every program of Windows has KERNEL32.dll.
    let root = tree(windows);
    write_program(
        &root.path().join("loah.exe"),
        "MZ stand-in\0nothing to read\0",
    );

    assert_fails(
        &Tools::new().run(root.path()),
        &["could not read any DLL name from loah.exe"],
    );
}

#[test]
fn the_ffmpeg_of_windows_is_not_asked_to_have_a_dll_name() {
    // loah-ffmpeg.exe is a mingw build that was checked by its own script; its stand-in has no name of a DLL, and the installer passes.
    let root = tree(windows);
    let ffmpeg_exe = fs::read_to_string(root.path().join("loah-ffmpeg.exe")).unwrap();
    assert!(!ffmpeg_exe.to_lowercase().contains(".dll"));

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn every_program_and_dll_of_the_installer_is_read_for_the_runtime_it_asks_for() {
    // The MCP server, and a DLL of the installer that needs one more DLL of the family (the ones of the runtime of Visual C++, whatever their
    // number: msvcp, vcruntime, concrt, vcomp, vccorlib).
    for (file, text, missing) in [
        (
            "loah-mcp.exe",
            "MZ\0VCRUNTIME140_THREADS.dll\0",
            "vcruntime140_threads.dll",
        ),
        ("msvcp140.dll", "MZ\0CONCRT140.dll\0", "concrt140.dll"),
        ("loah.exe", "MZ\0vcomp140.dll\0", "vcomp140.dll"),
    ] {
        let root = tree(windows);
        write_program(&root.path().join(file), text);

        assert_fails(&Tools::new().run(root.path()), &[file, missing]);
    }
}

#[test]
fn the_dlls_of_windows_itself_are_not_asked_to_be_in_the_installer() {
    let root = tree(windows);
    write_program(
        &root.path().join("loah.exe"),
        "MZ\0KERNEL32.dll\0api-ms-win-crt-runtime-l1-1-0.dll\0USER32.dll\0ucrtbase.dll\0d3d11.dll\0",
    );

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn the_files_of_windows_are_not_asked_for_the_permission_bit() {
    // An NSIS installer carries no permissions: the .exe files of the tree have none that 7z could give them.
    let root = tree(windows);
    for name in ["loah.exe", "loah-mcp.exe", "loah-ffmpeg.exe"] {
        fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }

    assert_passes(&Tools::new().run(root.path()));
}

#[test]
fn espeak_in_a_dll_of_the_installer_fails() {
    let root = tree(windows);
    write(&root.path().join("msvcp140.dll"), "ESPEAK_DATA_PATH\n");

    assert_fails(&Tools::new().run(root.path()), &["espeak", "msvcp140.dll"]);
}

#[test]
fn a_setup_exe_is_unpacked_with_7z_and_its_tree_is_checked() {
    let root = tree(windows);
    let work = tempfile::tempdir().unwrap();
    let setup = work.path().join("Loah_2.1.0_x64-setup.exe");
    fs::write(&setup, "MZ\n").unwrap();
    let tools = Tools::new().unpacking(root.path());

    assert_passes(&tools.run(&setup));

    assert!(tools.calls().contains("7z x "), "{}", tools.calls());
    fs::remove_file(root.path().join("vcruntime140.dll")).unwrap();
    assert_fails(&tools.run(&setup), &["vcruntime140.dll"]);
}

// ---- the disk image for macOS ------------------------------------------------------------------------------------------------

#[test]
fn a_complete_mac_app_passes_and_codesign_is_asked_about_the_app_and_both_sidecars() {
    let root = tree(mac);
    let tools = Tools::new();

    assert_passes(&tools.run(root.path()));

    let calls = tools.calls();
    for path in [
        "Loah.app",
        "Loah.app/Contents/MacOS/loah-mcp",
        "Loah.app/Contents/MacOS/loah-ffmpeg",
    ] {
        assert!(
            calls
                .lines()
                .any(|line| line.starts_with("codesign --verify --strict ")
                    && line.ends_with(path)),
            "{path} is not verified:\n{calls}"
        );
    }
}

#[test]
fn a_mac_app_without_a_sidecar_fails_and_says_which() {
    for sidecar in ["loah-mcp", "loah-ffmpeg"] {
        let root = tree(mac);
        fs::remove_file(root.path().join("Loah.app/Contents/MacOS").join(sidecar)).unwrap();

        assert_fails(&Tools::new().run(root.path()), &[sidecar]);
    }
}

#[test]
fn a_mac_app_whose_signature_does_not_verify_fails_and_names_what_is_wrong() {
    // The app, and each sidecar: a sidecar that was written over after it was signed is killed by macOS when it starts.
    for failing in ["Loah.app", "loah-mcp", "loah-ffmpeg"] {
        let root = tree(mac);

        let output = Tools::new()
            .with("FAKE_CODESIGN_FAIL", failing)
            .run(root.path());

        assert_fails(&output, &["codesign", failing]);
    }
}

#[test]
fn a_mac_app_must_say_why_it_wants_the_microphone_and_the_screen() {
    for key in [
        "NSMicrophoneUsageDescription",
        "NSScreenCaptureUsageDescription",
    ] {
        let root = tree(mac);
        let plist = root.path().join("Loah.app/Contents/Info.plist");
        let without: String = fs::read_to_string(&plist)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with(key))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(&plist, without).unwrap();

        assert_fails(&Tools::new().run(root.path()), &[key]);
    }
}

#[test]
fn a_mac_app_must_run_on_macos_13() {
    for version in ["12.0", "14.0", ""] {
        let root = tree(mac);
        let plist = root.path().join("Loah.app/Contents/Info.plist");
        let text = fs::read_to_string(&plist).unwrap().replace(
            "LSMinimumSystemVersion=13.0\n",
            &if version.is_empty() {
                String::new()
            } else {
                format!("LSMinimumSystemVersion={version}\n")
            },
        );
        fs::write(&plist, text).unwrap();

        assert_fails(&Tools::new().run(root.path()), &["LSMinimumSystemVersion"]);
    }
}

#[test]
fn the_main_program_of_a_mac_app_is_the_one_that_the_plist_names() {
    // The sidecars are beside it, whatever it is called (the app is `Loah`, the program `loah`, and here it is called `loah-app`).
    let root = tree(mac);
    let macos = root.path().join("Loah.app/Contents/MacOS");
    fs::rename(macos.join("loah"), macos.join("loah-app")).unwrap();
    let plist = root.path().join("Loah.app/Contents/Info.plist");
    let text = fs::read_to_string(&plist)
        .unwrap()
        .replace("CFBundleExecutable=loah", "CFBundleExecutable=loah-app");
    fs::write(&plist, text).unwrap();

    assert_passes(&Tools::new().run(root.path()));

    fs::remove_file(macos.join("loah-app")).unwrap();
    assert_fails(&Tools::new().run(root.path()), &["loah-app"]);
}

#[test]
fn a_dmg_is_mounted_read_only_checked_and_ejected_whatever_the_result_is() {
    let root = tree(mac);
    let work = tempfile::tempdir().unwrap();
    let image = work.path().join("Loah_2.1.0_aarch64.dmg");
    fs::write(&image, "dmg\n").unwrap();
    let tools = Tools::new().unpacking(root.path());

    assert_passes(&tools.run(&image));

    let calls = tools.calls();
    let attach = calls
        .lines()
        .find(|line| line.starts_with("hdiutil attach"))
        .unwrap_or_else(|| panic!("no attach in:\n{calls}"));
    assert!(attach.contains("-readonly"), "{attach}");
    assert!(
        calls.lines().any(|line| line.starts_with("hdiutil detach")),
        "the image is not ejected:\n{calls}"
    );

    // A check that fails ejects it too.
    fs::remove_file(root.path().join("Loah.app/Contents/MacOS/loah-ffmpeg")).unwrap();
    let failing = Tools::new().unpacking(root.path());
    assert_fails(&failing.run(&image), &["loah-ffmpeg"]);
    assert!(
        failing
            .calls()
            .lines()
            .any(|line| line.starts_with("hdiutil detach")),
        "the image is not ejected after a failure:\n{}",
        failing.calls()
    );
}

#[test]
fn the_programs_of_an_arm_mac_app_are_arm64_and_the_name_of_the_dmg_says_so() {
    // `Loah_<version>_aarch64.dmg` is for Apple silicon, `Loah_<version>_x64.dmg` for Intel: the architecture is read from the name,
    // and lipo says what each program is. An Intel row that bundled an arm64 sidecar would be an app that opens and cannot start its MCP server.
    for (name, arch) in [
        ("Loah_2.1.0_aarch64.dmg", "arm64"),
        ("Loah_2.1.0_x64.dmg", "x86_64"),
    ] {
        let root = tree(mac);
        let work = tempfile::tempdir().unwrap();
        let image = work.path().join(name);
        fs::write(&image, "dmg\n").unwrap();
        // The folder variable is not what a `.dmg` is checked against: the name is.
        let tools = Tools::new()
            .unpacking(root.path())
            .with("FAKE_LIPO_DEFAULT", arch)
            .with("LOAH_ARTIFACT_ARCH", "something-else");

        assert_passes(&tools.run(&image));
    }
}

#[test]
fn a_mac_program_of_another_architecture_than_the_dmg_says_fails_and_names_it() {
    for (name, wrong) in [
        ("Loah_2.1.0_aarch64.dmg", "x86_64"),
        ("Loah_2.1.0_x64.dmg", "arm64"),
    ] {
        for program in ["loah", "loah-mcp", "loah-ffmpeg"] {
            let root = tree(mac);
            let work = tempfile::tempdir().unwrap();
            let image = work.path().join(name);
            fs::write(&image, "dmg\n").unwrap();
            let expected = if wrong == "arm64" { "x86_64" } else { "arm64" };
            let tools = Tools::new()
                .unpacking(root.path())
                .with("FAKE_LIPO_DEFAULT", expected)
                .with("FAKE_LIPO", &format!("{program}={wrong}"));

            let output = tools.run(&image);

            assert_fails(&output, &[program, wrong, expected]);
        }
    }
}

#[test]
fn a_universal_program_is_not_one_architecture() {
    // `lipo -archs` of a fat file says both: it is not what an installer for one architecture ships.
    let root = tree(mac);
    let output = Tools::new()
        .with("FAKE_LIPO", "loah-mcp=x86_64+arm64")
        .run(root.path());

    assert_fails(&output, &["loah-mcp", "x86_64 arm64"]);
}

#[test]
fn a_folder_is_checked_for_the_architecture_that_the_variable_names() {
    let root = tree(mac);

    let output = Tools::new()
        .with("LOAH_ARTIFACT_ARCH", "x86_64")
        .run(root.path());

    // The stand-in lipo says arm64 for every program.
    assert_fails(&output, &["loah", "arm64", "x86_64"]);
}

#[test]
fn a_mac_app_of_which_the_architecture_is_not_known_is_not_a_pass() {
    // No name that says it (a folder, a `.dmg` that is neither aarch64 nor x64) and no variable: what to check against is not known, so
    // the check has not been made.
    let root = tree(mac);
    let output = Tools::new().without("LOAH_ARTIFACT_ARCH").run(root.path());
    assert_eq!(code(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stderr(&output).contains("architecture"),
        "{}",
        stderr(&output)
    );

    let work = tempfile::tempdir().unwrap();
    let image = work.path().join("Loah_2.1.0_universal.dmg");
    fs::write(&image, "dmg\n").unwrap();
    let tools = Tools::new()
        .unpacking(root.path())
        .without("LOAH_ARTIFACT_ARCH");
    let output = tools.run(&image);
    assert_eq!(code(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stderr(&output).contains("architecture"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn every_mac_program_must_run_on_macos_13() {
    // The minimum system that the linker wrote into each program (LC_BUILD_VERSION, which vtool prints): the program of an app that says
    // 13.0 in Info.plist is not to need a newer macOS to load. The libraries are built for the deployment target that the workflow sets;
    // this is what shows whether they were.
    for (program, minos) in [
        ("loah", "14.0"),
        ("loah-mcp", "15.0"),
        ("loah-ffmpeg", "26.0"),
    ] {
        let root = tree(mac);

        let output = Tools::new()
            .with("FAKE_MINOS", &format!("{program}={minos}"))
            .run(root.path());

        assert_fails(&output, &[program, minos, "13.0"]);
    }
}

#[test]
fn a_mac_program_without_a_minimum_system_is_not_a_pass() {
    let root = tree(mac);
    let tools = Tools::new();
    // The tool prints nothing that says a minimum: the program has none that is known to be 13.0.
    write_program(
        &tools.bin.path().join("vtool"),
        "#!/bin/sh\nfor file; do :; done\necho \"$file:\"\n",
    );

    let output = tools.run(root.path());

    assert_fails(&output, &["loah", "minos"]);
}

#[test]
fn a_mac_check_without_lipo_or_vtool_has_not_been_made_and_says_so() {
    // The tool is not there: not a pass, and not a failure of the installer either (exit 2, as for plutil and codesign).
    for tool in ["lipo", "vtool"] {
        let root = tree(mac);
        let output = Tools::new().run_without(root.path(), tool);

        assert_eq!(
            code(&output),
            2,
            "{tool}: {}{}",
            stdout(&output),
            stderr(&output)
        );
        assert!(stderr(&output).contains(tool), "{}", stderr(&output));
    }
}

// ---- what the script does with what it is given ------------------------------------------------------------------------------

#[test]
fn every_problem_is_told_and_not_only_the_first() {
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/bin/loah-ffmpeg")).unwrap();
    fs::remove_file(root.path().join("usr/lib/loah/libonnxruntime.so")).unwrap();
    write_program(&root.path().join("usr/bin/ffmpeg"), "#!/bin/sh\n");

    let output = Tools::new().run(root.path());

    assert_fails(
        &output,
        &["loah-ffmpeg", "libonnxruntime.so", "/usr/bin/ffmpeg"],
    );
}

#[test]
fn an_artifact_that_is_not_there_is_not_a_pass() {
    let work = tempfile::tempdir().unwrap();

    let output = Tools::new().run(&work.path().join("Loah_2.1.0_amd64.deb"));

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Loah_2.1.0_amd64.deb"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_folder_that_is_none_of_the_three_layouts_is_not_a_pass() {
    let work = tempfile::tempdir().unwrap();
    fs::write(work.path().join("readme.txt"), "nothing\n").unwrap();

    let output = Tools::new().run(work.path());

    assert_eq!(code(&output), 2, "{}", stderr(&output));
}

#[test]
fn a_file_of_a_kind_it_cannot_unpack_is_not_a_pass() {
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("Loah_2.1.0.tar.gz");
    fs::write(&archive, "x").unwrap();

    let output = Tools::new().run(&archive);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains(".tar.gz"), "{}", stderr(&output));
}

#[test]
fn it_says_how_to_use_it_when_it_is_given_no_artifact() {
    let output = Command::new("bash")
        .arg(script())
        .output()
        .expect("bash runs the script");

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("usage"), "{}", stderr(&output));
}

// ---- the license texts that every installer carries -----------------------------------------------------------------------------

/// The three kinds of installer: what they are called here, and the tree that each unpacks to.
type Build = fn(&Path);
const KINDS: [(&str, Build); 3] = [("deb", deb), ("windows", windows), ("mac", mac)];

/// Where each kind of installer has its license files, from the root of what it unpacks to (macOS: inside the app).
fn license_folder(kind: &str, root: &Path) -> PathBuf {
    match kind {
        "deb" => root.join("usr/share/doc/loah"),
        "windows" => root.join("Licenses"),
        _ => root.join("Loah.app/Contents/Resources/Licenses"),
    }
}

#[test]
fn an_installer_without_one_of_the_license_files_fails_and_says_which() {
    // The MIT, BSD, Apache and LGPL licenses of what is inside ask that their texts go with the binary: the installer carries them, and
    // a release whose installer lacks one does not go out.
    for (kind, build) in KINDS {
        for name in LICENSE_FILES {
            let root = tree(build);
            fs::remove_file(license_folder(kind, root.path()).join(name)).unwrap();

            let output = Tools::new().run(root.path());

            assert_fails(&output, &[name]);
            assert!(
                stderr(&output).contains("license"),
                "{kind}: without {name}, it said:\n{}",
                stderr(&output)
            );
        }
    }
}

#[test]
fn an_installer_whose_license_folder_is_not_there_fails() {
    for (kind, build) in KINDS {
        let root = tree(build);
        fs::remove_dir_all(license_folder(kind, root.path())).unwrap();

        let output = Tools::new().run(root.path());

        assert_fails(&output, &["THIRD_PARTY_NOTICES.md", "COPYING.LGPLv2.1"]);
    }
}

#[test]
fn a_license_file_that_is_empty_does_not_count() {
    let root = tree(windows);
    write(&root.path().join("Licenses/THIRD_PARTY_NOTICES.md"), "");

    assert_fails(
        &Tools::new().run(root.path()),
        &["THIRD_PARTY_NOTICES.md", "empty"],
    );
}

#[test]
fn a_deb_without_the_copyright_file_fails() {
    // Debian tools look for /usr/share/doc/<package>/copyright.
    let root = tree(deb);
    fs::remove_file(root.path().join("usr/share/doc/loah/copyright")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["copyright"]);
}

#[test]
fn the_license_files_in_another_place_than_the_one_of_the_system_do_not_count() {
    // A Windows tree whose files are in usr/share/doc, or a mac app whose files are beside the app and not in Contents/Resources/Licenses.
    let root = tree(windows);
    fs::rename(root.path().join("Licenses"), root.path().join("Elsewhere")).unwrap();

    assert_fails(&Tools::new().run(root.path()), &["Licenses"]);
}
