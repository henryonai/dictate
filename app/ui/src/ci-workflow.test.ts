// What the CI workflow promises about the libraries that the build links, held in place: these are the checks that nobody sees
// run until they are gone. The workflow is read as text (the repository's other tests read Rust sources the same way): a Windows
// checkout may have CRLF line ends, so they are made LF first.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");

const workflow = read(".github/workflows/ci.yml");

/** The jobs of the workflow, as text: `test:`, `app:`, `msrv:`, `macos-intel:`, `ffmpeg:` and `ffmpeg-windows:`. */
function job(name: string): string {
  const from = workflow.indexOf(`\n  ${name}:\n`);
  expect(from, `job ${name}`).toBeGreaterThan(-1);
  const next = workflow.slice(from + 1).search(/\n {2}[a-z][a-z0-9_-]*:\n/);
  return next === -1 ? workflow.slice(from) : workflow.slice(from, from + 1 + next);
}

/** The steps of a job, as text, each from its `- ` to the next one (the comment above a step is at the end of the one before). */
function stepsOf(name: string): string[] {
  return job(name)
    .split(/\n(?=      - )/)
    .filter((step) => step.startsWith("      - "));
}

/** The steps of every job. */
const steps = () => ["test", "app", "msrv"].flatMap(stepsOf);

describe("the /MD build of sherpa-onnx without text to speech that Windows links", () => {
  const ARCHIVE = "sherpa-onnx-v1.13.8-win-x64-static-MD-Release-no-tts-lib.tar.bz2";
  const SHA256 = "542348e56e827b59c6d249fd0dfd38dc34b7bd0c521a1ebe9eb6e2db154fa15d";
  const pinned = /^ {2}SHERPA_ONNX_MD_SHA256: ([0-9a-f]{64})$/m.exec(workflow)?.[1];
  const downloads = () => steps().filter((step) => step.includes(ARCHIVE));

  test("the SHA-256 of the archive is pinned once, at the top of the workflow, in lowercase hex", () => {
    expect(pinned).toBeDefined();
    // The release asset's own digest, as GitHub lists it (and as it was computed from a download).
    expect(pinned).toBe(SHA256);
    expect(workflow.match(/SHERPA_ONNX_MD_SHA256: /g)).toHaveLength(1);
    expect(workflow.indexOf("SHERPA_ONNX_MD_SHA256:")).toBeLessThan(workflow.indexOf("\njobs:"));
  });

  test("it is the build without text to speech: the one with espeak-ng (GPL-3.0) in it is not downloaded any more", () => {
    expect(workflow).not.toContain("win-x64-static-MD-Release-lib.tar.bz2");
    expect(read("app/README.md")).not.toContain("a0f44cd91486e448c2be1f1d3662edb4f473ca3cb38a803cee158760cc588428");
    expect(read("app/README.md")).toContain(SHA256);
    expect(read("app/README.md")).toContain(ARCHIVE);
  });

  test("each job that downloads it checks it before it is unpacked, and a different one stops the job", () => {
    // The `test` job, the `app` job and the Windows row of the `msrv` job.
    expect(downloads()).toHaveLength(3);
    for (const step of downloads()) {
      const download = step.indexOf("curl.exe");
      const check = step.indexOf("Get-FileHash -Algorithm SHA256");
      const unpack = step.indexOf("tar.exe");
      expect(download, step).toBeGreaterThan(-1);
      expect(check, "the hash is checked").toBeGreaterThan(download);
      expect(unpack, "and only then unpacked").toBeGreaterThan(check);
      const mismatch = step.slice(check, unpack);
      expect(mismatch).toContain("$env:SHERPA_ONNX_MD_SHA256");
      expect(mismatch).toMatch(/-ne\b/);
      expect(mismatch).toContain("throw");
    }
  });

  test("the three libraries that the build script of sherpa-onnx-sys always links, and this archive does not have, are stood in for", () => {
    // `emit_static_link_directives` names piper_phonemize, espeak-ng and ucd for every static build; the build without text to speech
    // has none of the three, and the link would stop (LNK1181). Copies of a small library that is there already, which the linker never
    // takes anything from (kissfft-float.lib comes before them, and has every symbol), are put under those names.
    for (const step of downloads()) {
      const unpack = step.indexOf("tar.exe");
      const stand = step.indexOf("foreach");
      expect(stand, "the stand-ins are made after the unpacking").toBeGreaterThan(unpack);
      for (const name of ["espeak-ng", "piper_phonemize", "ucd"]) expect(step.slice(stand)).toContain(`'${name}'`);
      expect(step.slice(stand)).toContain("Copy-Item (Join-Path $dir 'lib/kissfft-float.lib')");
      // Inside the block that a cache restore skips, so that they are in the cache.
      expect(step.indexOf("Test-Path")).toBeLessThan(unpack);
    }
  });

  test("the unpacked folder is cached under a key that has the start of the pinned hash in it, so that a new pin is downloaded and checked", () => {
    for (const name of ["test", "app", "msrv"]) {
      const cache = stepsOf(name).filter((step) => step.includes("path: target/sherpa-onnx-md-no-tts"));
      expect(cache, name).toHaveLength(1);
      expect(cache[0]).toContain(`key: sherpa-onnx-md-no-tts-1.13.8-${SHA256.slice(0, 8)}-`);
    }
    // The old folder and key (an archive that has espeak-ng in it) are gone.
    expect(workflow).not.toContain("target/sherpa-onnx-md\n");
    expect(workflow).not.toContain("key: sherpa-onnx-md-1.13.8");
  });
});

describe("the sherpa-onnx build without text to speech that Linux links", () => {
  const SCRIPT = "scripts/fetch-sherpa-onnx-no-tts.sh";
  const SHA256 = "bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b";
  const ARCHIVE = "sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2";

  test("the script pins the archive of the release by its SHA-256, which is the digest GitHub lists for it", () => {
    const script = read(SCRIPT);
    expect(script).toContain(`:-${SHA256}}`);
    expect(script).toContain(`sherpa-onnx-v\${VERSION}-linux-x64-shared-no-tts-lib.tar.bz2`);
    expect(script).toContain('VERSION="1.13.8"');
    // The check is before the unpacking.
    expect(script.indexOf('"$actual" != "$SHA256"')).toBeGreaterThan(script.indexOf("curl "));
    expect(script.indexOf('"$actual" != "$SHA256"')).toBeLessThan(script.indexOf("tar -xjf"));
  });

  test("the pin is the same in the script, in the message that a build without the folder gets, and in the README", () => {
    for (const text of [read(SCRIPT), read("crates/dictate-services/build_support.rs"), read("app/README.md")]) {
      expect(text).toContain(SHA256);
    }
    expect(read("crates/dictate-services/build_support.rs")).toContain(ARCHIVE);
    expect(read("app/README.md")).toContain(ARCHIVE);
  });

  test("each job that builds on Linux gets the folder before it builds, and puts it in the environment of the job", () => {
    for (const name of ["test", "app", "msrv"]) {
      const fetch = stepsOf(name).filter((step) => step.includes(`bash ${SCRIPT}`));
      expect(fetch, `${name}: one step fetches it`).toHaveLength(1);
      const [step] = fetch;
      expect(step).toContain("shell: bash");
      expect(step).toContain("runner.os == 'Linux'");
      // Two statements: the failure of the script (a different archive) must fail the step, which `echo "...$(script)"` would not.
      expect(step).toMatch(/lib="\$\(bash scripts\/fetch-sherpa-onnx-no-tts\.sh target\/sherpa-onnx-no-tts\)"/);
      expect(step).toContain('echo "SHERPA_ONNX_LIB_DIR=$lib" >> "$GITHUB_ENV"');
      const firstCargo = job(name).search(/\n +(- )?run: (cargo|pnpm)/);
      expect(job(name).indexOf(step), `${name}: before the first build`).toBeLessThan(firstCargo);
    }
  });

  test("the folder is cached unpacked under a key that names it, and the key of the old download is only for macOS", () => {
    for (const name of ["test", "app", "msrv"]) {
      const cache = stepsOf(name).filter((step) => step.includes("path: target/sherpa-onnx-no-tts"));
      expect(cache, name).toHaveLength(1);
      expect(cache[0]).toMatch(/key: sherpa-onnx-no-tts-1\.13\.8-bf2d998c-\$\{\{ runner\.arch \}\}/);
      // The start of the key is the start of the pin of the script, so that a new pin is a new key.
      expect(read(SCRIPT)).toContain(`:-${cache[0].match(/-bf2d998c-/)?.[0].slice(1, -1) ?? "none"}`);
    }
    // The static library that the build script of sherpa-onnx-sys downloads, on macOS: the rows of the msrv job and the check of the Intel
    // target build it too.
    for (const name of ["test", "app", "msrv", "macos-intel"]) {
      const prebuilt = stepsOf(name).filter((step) => step.includes("path: target/sherpa-onnx-prebuilt"));
      expect(prebuilt, name).toHaveLength(1);
      if (name !== "macos-intel") expect(prebuilt[0]).toContain("if: runner.os == 'macOS'");
      expect(prebuilt[0]).toMatch(/key: sherpa-onnx-1\.13\.8-\$\{\{ runner\.os \}\}-\$\{\{ runner\.arch \}\}/);
      expect(prebuilt[0]).not.toContain("shared");
    }
  });
});

describe("the speaker models of a folder that has a Vietnamese name", () => {
  test("the step that runs the real models runs the test that loads them from such a folder, on every system", () => {
    const smoke = steps().find((step) => step.includes("DICTATE_TEST_DIARIZE_DIR"));
    expect(smoke).toBeDefined();
    expect(smoke).toContain("diarizer_loads_models_from_a_folder_with_a_non_ascii_name");
    // The same step as the other two real-model tests: it is in the `test` job, whose matrix has Windows.
    expect(smoke).not.toMatch(/^\s+if:/m);
    expect(stepsOf("test")).toContain(smoke);
    expect(job("test")).toContain("windows-latest");
  });
});

describe("the recording of the screen on the Windows runner", () => {
  const TEST = "windows_records_three_seconds_of_the_primary_monitor";
  const recording = () => stepsOf("test").find((step) => step.includes(TEST));

  test("a step of the test job runs the real recording test, on Windows only, and a red result fails the job", () => {
    const step = recording();
    expect(step).toBeDefined();
    expect(job("test")).toContain("windows-latest");
    expect(step).toContain("- name: Record the primary monitor (Windows)\n");
    expect(step).toContain("if: runner.os == 'Windows'");
    // The runner recorded its own screen on every run since the step was added as an experiment, so it is a step like the others: an
    // experiment had `continue-on-error: true`, which turns a failure into a pass.
    expect(step).not.toContain("continue-on-error");
    expect(step).not.toContain("experiment");
    expect(step).toMatch(/DICTATE_TEST_SCREEN: "?1"?\n/);
    // The test is ignored: it runs only when it is asked for, and by its name.
    expect(step).toMatch(/cargo test .*-- .*--ignored .*windows_records_three_seconds_of_the_primary_monitor/);
  });

  test("it comes after the main run of the tests, so that a failure there is told first", () => {
    const step = recording();
    expect(step).toBeDefined();
    const main = job("test").indexOf("run: cargo test --all --locked");
    expect(main).toBeGreaterThan(-1);
    expect(job("test").indexOf(step as string)).toBeGreaterThan(main);
  });

  test("the test that the step names is in the code: a name that matches nothing runs nothing, and passes", () => {
    expect(read("crates/dictate-services/src/record/capture/windows.rs")).toContain(`fn ${TEST}()`);
  });
});

describe("the PipeWire libraries that the screen capture of Linux builds against", () => {
  // The portal gives the screen as a PipeWire stream: the `pipewire` crate links libpipewire-0.3 and builds its bindings from the headers
  // of libpipewire-0.3-dev and libspa-0.2-dev (with libclang, which is in the list already).
  const LIBRARIES = ["libpipewire-0.3-dev", "libspa-0.2-dev"];
  const installs = (name: string) => stepsOf(name).filter((step) => step.includes("apt-get install"));

  test("every job that builds on Linux installs them, in the step that installs the other libraries it builds against", () => {
    for (const name of ["test", "app", "msrv"]) {
      // The step of the other libraries (the first apt-get install of `test` is the one of ffmpeg, which comes later).
      const step = installs(name).find((candidate) => candidate.includes("libwebkit2gtk-4.1-dev"));
      expect(step, `${name}: the step with the Linux build dependencies`).toBeDefined();
      for (const library of LIBRARIES) expect(step, `${name}: ${library}`).toContain(library);
      // Before the first build, or the build script of pipewire-sys finds no headers.
      const firstBuild = job(name).search(/\n +(- )?run: (cargo|pnpm)/);
      expect(job(name).indexOf(step as string), `${name}: before the first build`).toBeLessThan(firstBuild);
    }
  });

  test("the README lists them with the other libraries that a build on Linux needs", () => {
    const readme = read("app/README.md");
    const apt = readme.slice(readme.indexOf("sudo apt-get install -y build-essential"));
    const command = apt.slice(0, apt.indexOf("```"));
    for (const library of LIBRARIES) expect(command).toContain(library);
  });
});

describe("how long a job gets", () => {
  /** The minutes that `timeout-minutes: ${{ matrix.os == 'X' && A || B }}` gives the runner `os`. */
  function minutes(name: string, os: string): number {
    const expression = /^ {4}timeout-minutes: (.+)$/m.exec(job(name))?.[1] ?? "";
    const parsed = /^\$\{\{ matrix\.os == '([^']+)' && (\d+) \|\| (\d+) \}\}$/.exec(expression);
    if (!parsed) return Number(expression);
    return Number(os === parsed[1] ? parsed[2] : parsed[3]);
  }

  test("the test job gets 45 minutes on Linux, as on Windows: a slow apt mirror has been taking about 24 of 30 minutes by itself", () => {
    expect(minutes("test", "ubuntu-24.04")).toBe(45);
    expect(minutes("test", "windows-latest")).toBe(45);
  });

  test("none of the jobs gets less than it had", () => {
    expect(minutes("test", "macos-latest")).toBeGreaterThanOrEqual(30);
    expect(minutes("app", "ubuntu-24.04")).toBeGreaterThanOrEqual(30);
    expect(minutes("app", "windows-latest")).toBeGreaterThanOrEqual(45);
    expect(minutes("msrv", "ubuntu-24.04")).toBeGreaterThanOrEqual(30);
  });

  test("the check of the oldest Rust gets as long on each system as the build of the same code has: a cold Windows build is the slow one", () => {
    expect(minutes("msrv", "windows-latest")).toBeGreaterThanOrEqual(45);
    expect(minutes("msrv", "macos-latest")).toBeGreaterThanOrEqual(30);
    expect(minutes("macos-intel", "macos-latest")).toBeGreaterThanOrEqual(30);
  });
});

describe("no espeak-ng (GPL-3.0) in what the app ships", () => {
  const SCRIPT = "scripts/check-no-espeak.sh";

  test("the app job checks the program on each system, and on Linux the shared libraries beside it", () => {
    const checks = steps().filter((step) => step.includes(`run: bash ${SCRIPT}`));
    expect(checks.length).toBeGreaterThanOrEqual(2);
    for (const check of checks) expect(stepsOf("app")).toContain(check);

    const linux = checks.find((step) => step.includes("runner.os == 'Linux'") && !step.includes("!="));
    expect(linux, "a step for Linux").toBeDefined();
    // The shared library has the code of espeak-ng in it whether or not the program refers to it: it is checked too.
    expect(linux).toContain("target/debug/*.so");
    expect(linux).toContain("target/debug/dictate");
    // And the libraries that a package ships are the ones in the folder they are taken from, whether or not the build copied them
    // beside the program: a restored cache has not (the libraries are not in it), and the check on the copies alone failed on it.
    expect(linux).toContain('"$SHERPA_ONNX_LIB_DIR"/*.so*');

    const elsewhere = checks.find((step) => step.includes("runner.os != 'Linux'"));
    expect(elsewhere, "a step for macOS and Windows").toBeDefined();
    expect(elsewhere).toContain("target/debug/dictate");
    expect(elsewhere).toContain(".exe");
  });

  test("the check runs after the program is built, and in bash, which every system has", () => {
    const build = workflow.indexOf("cargo build -p dictate-app --locked");
    const checks = steps().filter((step) => step.includes(`run: bash ${SCRIPT}`));
    expect(checks.length).toBeGreaterThan(0);
    for (const check of checks) {
      expect(workflow.indexOf(check)).toBeGreaterThan(build);
      expect(check).toContain("shell: bash");
    }
  });

  test("the script is checked out with LF line ends on Windows too, or bash would choke on the CR", () => {
    expect(read(".gitattributes")).toMatch(/^\*\.sh +text +eol=lf$/m);
  });
});

describe("the ffmpeg that ships with the app (LGPL v2.1 or later, built from source)", () => {
  const SCRIPT = "scripts/build-ffmpeg.sh";
  const CHECK = "scripts/check-ffmpeg-lgpl.sh";
  const VERSION = "9.0.2";
  const SHA256 = "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e";
  const TARGETS = [
    ["macos-latest", "aarch64-apple-darwin"],
    ["macos-latest", "x86_64-apple-darwin"],
    ["ubuntu-24.04", "x86_64-unknown-linux-gnu"],
    // Built on Linux with mingw, and run by the job after this one on Windows.
    ["ubuntu-24.04", "x86_64-pc-windows-msvc"],
  ];
  /** The lines of a script that are not comments: the comments say what the script does not build. */
  const code = (file: string) =>
    read(file)
      .split("\n")
      .filter((line) => !line.trim().startsWith("#"))
      .join("\n");
  const step = (name: string, text: string) => {
    const found = stepsOf(name).filter((candidate) => candidate.includes(text));
    expect(found, `${name}: the step with ${text}`).toHaveLength(1);
    return found[0];
  };

  test("the script pins the source by its SHA-256 and builds it as LGPL v2.1 or later and nothing else", () => {
    const script = read(SCRIPT);
    expect(script).toContain(`VERSION="${VERSION}"`);
    expect(script).toContain(`:-${SHA256}}`);
    for (const option of ["--disable-gpl", "--disable-nonfree", "--disable-version3"]) expect(script).toContain(option);
    for (const option of ["--enable-gpl", "--enable-nonfree", "--enable-version3", "--enable-libx264"]) {
      expect(code(SCRIPT), option).not.toContain(option);
    }
    // The hash is checked before the tarball is unpacked, and configure has to say LGPL.
    expect(script.indexOf('"$actual" != "$SHA256"')).toBeGreaterThan(script.indexOf("curl "));
    expect(script.indexOf('"$actual" != "$SHA256"')).toBeLessThan(script.indexOf("tar -xf"));
    expect(script).toContain("License: LGPL version 2.1 or later");
  });

  test("it can decode a video to nowhere, which the tests of the repository do to read how long a recording is", () => {
    // `-i video.mp4 -map 0:v -f null -` decodes, and the null muxer wants the encoder that wraps the decoded frames: without it ffmpeg
    // says "Default encoder for format null (codec wrapped_avframe) is probably disabled" and ten of the tests of media and recording fail.
    expect(read(SCRIPT)).toContain("--enable-encoder=wrapped_avframe");
  });

  test("every demuxer and every decoder is on, so that no import that the user's own ffmpeg could read fails with the one that ships", () => {
    // Tools::find prefers dictate-ffmpeg to the ffmpeg of the user, and an import that symphonia cannot read goes through ffmpeg.
    const lines = code(SCRIPT);
    expect(lines).toContain("--disable-everything");
    for (const all of ["--enable-demuxers", "--enable-decoders", "--enable-parsers", "--enable-bsfs"]) {
      expect(lines.indexOf(all), all).toBeGreaterThan(lines.indexOf("--disable-everything"));
    }
    expect(lines).not.toContain("--enable-demuxer=");
    expect(lines).not.toContain("--enable-decoder=");
    // Encoders, muxers, filters and protocols are the ones that the app asks for.
    for (const curated of ["--enable-protocol=", "--enable-muxer=", "--enable-encoder=", "--enable-filter="]) {
      expect(lines, curated).toContain(curated);
    }
  });

  test("the rows that run the build check that formats of the long tail are in it", () => {
    for (const name of ["ffmpeg", "ffmpeg-windows"]) {
      const listing = step(name, "-hide_banner -encoders");
      expect(listing, name).toContain("-hide_banner -demuxers");
      expect(listing, name).toContain("-hide_banner -decoders");
      // A demuxer and a decoder that only a build with every one of them has.
      expect(listing, name).toMatch(/mxf/);
      expect(listing, name).toMatch(/g722/);
    }
  });

  test("the tests are run after the program that they run with is shown to run", () => {
    for (const name of ["ffmpeg", "ffmpeg-windows"]) {
      const tests = step(name, "cargo test -p dictate-services");
      const version = tests.indexOf('"$DICTATE_FFMPEG" -version');
      expect(version, `${name}: the program is run first`).toBeGreaterThan(-1);
      expect(version).toBeLessThan(tests.indexOf("cargo test -p dictate-services"));
      // Bash on every system: Windows would read `"$DICTATE_FFMPEG"` as text in PowerShell.
      expect(tests).toContain("shell: bash");
    }
  });

  test("Windows gets the encoders of the system and no libvpl: h264_qsv is not built, and h264_mf is built so that it compiles", () => {
    const script = read(SCRIPT);
    expect(script).toContain("--enable-mediafoundation --enable-d3d11va");
    expect(script).toContain("--enable-nvenc");
    expect(script).toContain("--enable-amf");
    expect(code(SCRIPT)).not.toContain("libvpl");
    expect(code(SCRIPT)).not.toContain("h264_qsv");
  });

  test("the job builds the four targets: both Macs on macOS, Linux and the Windows program (with mingw) on Ubuntu 24.04", () => {
    const text = job("ffmpeg");
    for (const [os, target] of TARGETS) {
      expect(text, target).toMatch(new RegExp(`- os: ${os}\\n +target: ${target}\\n`));
    }
    expect(text).toContain("fail-fast: false");
    // Ubuntu is pinned as in the other jobs.
    expect(text).not.toContain("ubuntu-latest");
  });

  test("the build is cached under the hash of the script, the version and the target, and only a miss builds it", () => {
    const cache = step("ffmpeg", "id: ffmpeg-build");
    expect(cache).toContain("path: target/ffmpeg/${{ matrix.target }}");
    expect(cache).toMatch(/key: ffmpeg-\$\{\{ env\.FFMPEG_VERSION \}\}-\$\{\{ matrix\.target \}\}-\$\{\{ hashFiles\('scripts\/build-ffmpeg\.sh'\) \}\}/);
    // The version in the key is the one that the script builds.
    expect(/^ {2}FFMPEG_VERSION: (\S+)$/m.exec(workflow)?.[1]).toBe(VERSION);

    const build = step("ffmpeg", `bash ${SCRIPT}`);
    expect(build).toContain("if: steps.ffmpeg-build.outputs.cache-hit != 'true'");
    expect(build).toContain("bash scripts/build-ffmpeg.sh ${{ matrix.target }} target/ffmpeg/${{ matrix.target }}");
    expect(job("ffmpeg").indexOf(build)).toBeGreaterThan(job("ffmpeg").indexOf(cache));
  });

  test("the check can read what the Windows program links, also when the build comes from the cache", () => {
    // The Windows program cannot run on Linux, so what it links is read with mingw's objdump: it is installed whether or not the build was
    // restored, or a cached build would be checked less than a fresh one.
    const objdump = step("ffmpeg", "- name: Install mingw's objdump");
    expect(objdump).toContain("matrix.target == 'x86_64-pc-windows-msvc'");
    expect(objdump).not.toContain("cache-hit");
    expect(objdump).toContain("binutils-mingw-w64-x86-64");
    expect(job("ffmpeg").indexOf(objdump)).toBeLessThan(job("ffmpeg").indexOf(`bash ${CHECK}`));
  });

  test("each build is checked to be a plain LGPL ffmpeg, and kept for the job that runs the Windows one", () => {
    const check = step("ffmpeg", `bash ${CHECK}`);
    expect(check).toContain("shell: bash");
    expect(check).toContain("target/ffmpeg/${{ matrix.target }}/dictate-ffmpeg${{ matrix.exe }}");
    expect(job("ffmpeg").indexOf(check)).toBeGreaterThan(job("ffmpeg").indexOf(`bash ${SCRIPT}`));

    const upload = step("ffmpeg", "actions/upload-artifact@v4");
    expect(upload).toContain("name: dictate-ffmpeg-${{ matrix.target }}");
    expect(upload).toContain("target/ffmpeg/${{ matrix.target }}/dictate-ffmpeg${{ matrix.exe }}");
    expect(upload).toContain("BUILDCONF-${{ matrix.target }}.txt");
    // Before the tests: a failing test must not keep the artifact from the Windows job.
    expect(job("ffmpeg").indexOf(upload)).toBeGreaterThan(job("ffmpeg").indexOf(check));
  });

  test("the tests of media and recording run with the ffmpeg that was built, not the one of the package manager, after the check", () => {
    const tests = step("ffmpeg", "cargo test -p dictate-services");
    expect(tests).toMatch(/cargo test -p dictate-services --lib --locked -- .*media:: .*record::/);
    expect(tests).toContain("DICTATE_FFMPEG: ${{ github.workspace }}/target/ffmpeg/${{ matrix.target }}/dictate-ffmpeg${{ matrix.exe }}");
    // A program that cannot run on the runner (the other Mac, the Windows one) is not run.
    expect(tests).toContain("if: matrix.tests");
    expect(job("ffmpeg").indexOf(tests)).toBeGreaterThan(job("ffmpeg").indexOf(`bash ${CHECK}`));
    // The runner has no ffmpeg of its own put in: that is what CI tests the shipped one instead of.
    for (const name of ["ffmpeg", "ffmpeg-windows"]) {
      expect(job(name)).not.toMatch(/brew install ffmpeg|apt-get install -y ffmpeg|choco install ffmpeg/);
    }
  });

  test("the Windows program is run on a Windows runner: downloaded from the Linux build, checked, and tested", () => {
    const windows = job("ffmpeg-windows");
    expect(windows).toContain("runs-on: windows-latest");
    // After the builds, and whether or not the other rows of them went well: it needs the artifact of one row only.
    expect(windows).toContain("needs: ffmpeg");
    expect(windows).toContain("if: ${{ !cancelled() }}");
    const download = step("ffmpeg-windows", "actions/download-artifact@v4");
    expect(download).toContain("name: dictate-ffmpeg-x86_64-pc-windows-msvc");
    expect(download).toContain("path: target/ffmpeg/x86_64-pc-windows-msvc");

    const check = step("ffmpeg-windows", `bash ${CHECK}`);
    expect(check).toContain("shell: bash");
    expect(check).toContain("target/ffmpeg/x86_64-pc-windows-msvc/dictate-ffmpeg.exe");
    // The first time the program runs on Windows: what it lists is the encoders that the recording picks from.
    expect(windows).toContain("-hide_banner -encoders");

    const tests = step("ffmpeg-windows", "cargo test -p dictate-services");
    expect(tests).toMatch(/cargo test -p dictate-services --lib --locked -- .*media:: .*record::/);
    expect(tests).toContain("DICTATE_FFMPEG: ${{ github.workspace }}/target/ffmpeg/x86_64-pc-windows-msvc/dictate-ffmpeg.exe");
    expect(windows.indexOf(download)).toBeLessThan(windows.indexOf(check));
    expect(windows.indexOf(check)).toBeLessThan(windows.indexOf(tests));
  });

  test("the Windows runner has no graphics card: the encoder it picks must be h264_mf (the software one), and never mpeg4", () => {
    // The ffmpeg that ships has no libx264 or libopenh264, so a machine whose Media Foundation tries all fail records mpeg4, which the
    // player cannot play. The tests print which encoder they ran with; this one fails when it is not Media Foundation's.
    const tests = step("ffmpeg-windows", "cargo test -p dictate-services");
    expect(tests).toContain("DICTATE_EXPECT_ENCODER: h264_mf");
    const TEST = "pick_codec_with_the_real_ffmpeg_gives_the_encoder_that_a_machine_is_known_to_have";
    expect(read("crates/dictate-services/src/record/video.rs")).toContain(`fn ${TEST}()`);
    expect(read("crates/dictate-services/src/record/video.rs")).toContain('std::env::var("DICTATE_EXPECT_ENCODER")');
    // It is in what the step runs: a name that matches nothing runs nothing, and passes.
    expect(tests).toMatch(/-- .*record::/);
  });

  test("the jobs that run the tests have what dictate-services is built with: the sherpa-onnx library of each system", () => {
    const linux = step("ffmpeg", "Use the sherpa-onnx build without text to speech (Linux)");
    expect(linux).toContain("if: runner.os == 'Linux' && matrix.tests");
    expect(linux).toContain('echo "SHERPA_ONNX_LIB_DIR=$lib" >> "$GITHUB_ENV"');
    expect(step("ffmpeg", "path: target/sherpa-onnx-prebuilt")).toContain("if: runner.os == 'macOS' && matrix.tests");
    const windows = step("ffmpeg-windows", "Use the /MD build of sherpa-onnx without text to speech (Windows)");
    expect(windows).toContain("Get-FileHash -Algorithm SHA256");
    expect(windows).toContain("$env:SHERPA_ONNX_MD_SHA256");
    expect(windows.indexOf("Get-FileHash")).toBeLessThan(windows.indexOf("tar.exe"));
    // They read the cache of the test job, and do not write it: theirs has only part of the build in it.
    for (const name of ["ffmpeg", "ffmpeg-windows"]) {
      const rust = step(name, "Swatinem/rust-cache@v2");
      expect(rust).toContain("shared-key: ${{ runner.os }}");
      expect(rust).toContain("save-if: false");
    }
  });

  test("the job gets time for a cold build of the whole workspace's libraries on a slow runner", () => {
    for (const name of ["ffmpeg", "ffmpeg-windows"]) {
      expect(Number(/^ {4}timeout-minutes: (\d+)$/m.exec(job(name))?.[1])).toBeGreaterThanOrEqual(45);
    }
  });
});

describe("the oldest Rust that the workspace promises to build with (1.85) is checked on every system it supports", () => {
  // `rust-version = "1.85"` in Cargo.toml is a promise to macOS and Windows users as much as to Linux ones: code that is only compiled for
  // another system (the platform layer, the capture crates) is not checked by a job that runs on Linux.
  test("the job has a row for each system, and each runs `cargo +1.85 check` on the whole workspace, locked", () => {
    const text = job("msrv");
    expect(text).toMatch(/os: \[macos-latest, windows-latest, ubuntu-24\.04\]/);
    expect(text).toContain("fail-fast: false");
    expect(text).toContain("runs-on: ${{ matrix.os }}");
    expect(text).toContain("dtolnay/rust-toolchain@1.85");
    expect(text).toContain("run: cargo +1.85 check --workspace --all-targets --locked");
    // The check is not for Linux only any more: nothing in the job says `ubuntu-24.04` except the matrix.
    expect(text.match(/ubuntu-24\.04/g)).toHaveLength(1);
  });

  test("each system gets what it builds with: the Linux libraries on Linux only, and the cache of the system", () => {
    const install = stepsOf("msrv").find((step) => step.includes("apt-get install") && step.includes("libwebkit2gtk-4.1-dev"));
    expect(install).toContain("if: runner.os == 'Linux'");
    const rust = stepsOf("msrv").find((step) => step.includes("Swatinem/rust-cache@v2"));
    expect(rust).toContain("shared-key: msrv-${{ runner.os }}");
    expect(rust).toContain("save-if: ${{ github.ref == 'refs/heads/main' }}");
  });

  test("the Linux pin of the runner stays: the other systems are `latest`, as in the other jobs", () => {
    expect(job("msrv")).not.toContain("ubuntu-latest");
  });
});

describe("the Intel Mac is checked before a release finds out that it does not build", () => {
  // Nobody has built the app for x86_64-apple-darwin: whisper.cpp, the Swift bridge of ScreenCaptureKit and the static sherpa-onnx archive
  // of that architecture are all new on the cross build. `cargo check` runs their build scripts, which is where they would refuse.
  const text = () => job("macos-intel");

  test("a job on the arm64 runner checks the whole workspace for x86_64-apple-darwin, on every pull request", () => {
    expect(text()).toContain("runs-on: macos-latest");
    expect(text()).toContain("run: cargo check --workspace --all-targets --locked --target x86_64-apple-darwin");
    // The standard library of the target has to be there.
    const rust = stepsOf("macos-intel").find((step) => step.includes("dtolnay/rust-toolchain@stable"));
    expect(rust).toContain("targets: x86_64-apple-darwin");
    // ci.yml runs on pull requests and on main: the job has no condition of its own.
    expect(text()).not.toMatch(/^ {4}if:/m);
  });

  test("it has the macOS sherpa-onnx library of the other jobs, with the same cache key and the cache of its own Rust build", () => {
    const rust = stepsOf("macos-intel").find((step) => step.includes("Swatinem/rust-cache@v2"));
    expect(rust).toContain("shared-key: macos-x86_64");
    expect(rust).toContain("save-if: ${{ github.ref == 'refs/heads/main' }}");
    expect(text()).toContain("key: sherpa-onnx-1.13.8-${{ runner.os }}-${{ runner.arch }}");
  });
});

describe("how CI is started", () => {
  test("on every pull request and on main, and by hand on any branch", () => {
    // By hand: a branch that is not a pull request yet (the one that prepares a release, for one) can run the same jobs on the real runners.
    const on = workflow.slice(workflow.indexOf("\non:\n"), workflow.indexOf("\npermissions:\n"));
    expect(on).toMatch(/^ {2}push:\n {4}branches: \[main\]$/m);
    expect(on).toMatch(/^ {2}pull_request:$/m);
    expect(on).toMatch(/^ {2}workflow_dispatch:$/m);
  });
});
