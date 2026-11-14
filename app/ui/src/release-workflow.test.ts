// What the release workflow promises, held in place: the installers it builds are checked before they go anywhere, nothing is uploaded by
// a dry run, and a release is never given another body than the one release-please wrote. These are the checks that nobody sees run until
// the day a release ships without a sidecar. The workflow is read as text (as ci-workflow.test.ts reads the CI workflow); a Windows
// checkout may have CRLF line ends, so they are made LF first.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");
const readJson = (file: string) => JSON.parse(read(file));

/** What every installer carries beside the program (stage-sidecars.sh copies them from the root of the repository). */
const LICENSE_FILES = ["LICENSE", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.txt", "COPYING.LGPLv2.1"];

const workflow = read(".github/workflows/release.yml");
const ci = read(".github/workflows/ci.yml");

/** A job of a workflow, as text (`name:` at two-space indent, up to the next job). */
function jobOf(text: string, name: string): string {
  const from = text.indexOf(`\n  ${name}:\n`);
  expect(from, `job ${name}`).toBeGreaterThan(-1);
  const next = text.slice(from + 1).search(/\n {2}[a-z][a-z0-9_-]*:\n/);
  return next === -1 ? text.slice(from) : text.slice(from, from + 1 + next);
}
const job = (name: string) => jobOf(workflow, name);

/** The steps of a job of the release workflow, as text, each from its `- ` to the next one. */
function stepsOf(name: string): string[] {
  return job(name)
    .split(/\n(?=      - )/)
    .filter((step) => step.startsWith("      - "));
}

/** The one step of a job that has `text` in it. */
function step(name: string, text: string): string {
  const found = stepsOf(name).filter((candidate) => candidate.includes(text));
  expect(found, `${name}: the step with ${text}`).toHaveLength(1);
  return found[0];
}

/** The rows of the matrix of the `build` job: each `- name: x` with the `key: value` lines under it. */
function rows(): Record<string, string>[] {
  const matrix = job("build").slice(job("build").indexOf("        include:\n"));
  const end = matrix.search(/\n {4}steps:\n/);
  return matrix
    .slice(0, end)
    .split(/\n(?= {10}- )/)
    .slice(1)
    .map((row) => {
      const entries = [...row.matchAll(/^ {10}(?:- | {2})([a-z_]+): (.*)$/gm)].map(([, key, value]) => [key, value.replace(/^"(.*)"$/, "$1")]);
      return Object.fromEntries(entries);
    });
}
const row = (name: string) => {
  const found = rows().find((candidate) => candidate.name === name);
  expect(found, `matrix row ${name}`).toBeDefined();
  return found as Record<string, string>;
};

describe("how the release workflow is started", () => {
  test("workflow_call_declares_the_tag_that_release_please_passes", () => {
    const call = /^ {2}workflow_call:\n {4}inputs:\n {6}tag:\n((?: {8}.*\n)+)/m.exec(workflow);
    expect(call, "on.workflow_call.inputs.tag").not.toBeNull();
    expect(call![1]).toMatch(/^ {8}type: string$/m);
    // And release-please.yml gives exactly that input: a rename on one side fails here, not in the first release.
    const caller = read(".github/workflows/release-please.yml");
    expect(caller).toContain("uses: ./.github/workflows/release.yml");
    expect(caller).toMatch(/^ {4}with:\n {6}tag: \$\{\{ needs\.release-please\.outputs\.tag_name \}\}$/m);
  });

  test("workflow_dispatch_takes_an_optional_tag_and_an_empty_one_is_a_dry_run", () => {
    const dispatch = /^ {2}workflow_dispatch:\n {4}inputs:\n {6}tag:\n((?: {8}.*\n)+)/m.exec(workflow);
    expect(dispatch, "on.workflow_dispatch.inputs.tag").not.toBeNull();
    expect(dispatch![1]).toMatch(/^ {8}type: string$/m);
    expect(dispatch![1]).toMatch(/^ {8}required: false$/m);
    expect(dispatch![1]).toMatch(/^ {8}default: ""$/m);
    expect(dispatch![1]).toMatch(/dry run/i);
  });

  test("it_is_not_started_by_anything_else", () => {
    // A push or a pull request would build installers on every change: the caller is release-please, or a person.
    const on = workflow.slice(workflow.indexOf("\non:\n"), workflow.indexOf("\npermissions:\n"));
    expect(on).not.toMatch(/^ {2}(push|pull_request|schedule|release):/m);
  });

  test("it_can_read_the_repository_and_only_the_jobs_that_attach_files_can_write_to_it", () => {
    expect(workflow).toMatch(/^permissions:\n {2}contents: read\n/m);
    for (const name of ["build", "sources"]) expect(job(name), name).toMatch(/^ {4}permissions:\n {6}contents: write$/m);
    expect(job("ffmpeg-windows")).not.toContain("contents: write");
    expect(job("preflight")).not.toContain("contents: write");
  });
});

describe("the four installers", () => {
  test("the_matrix_has_a_row_for_each_system_with_its_target_and_its_bundle", () => {
    expect(rows().map((candidate) => candidate.name)).toEqual(["macos-arm64", "macos-x64", "windows-x64", "linux-x64"]);
    expect(row("macos-arm64")).toMatchObject({ os: "macos-latest", target: "aarch64-apple-darwin", bundles: "dmg" });
    expect(row("windows-x64")).toMatchObject({ os: "windows-latest", target: "x86_64-pc-windows-msvc", bundles: "nsis" });
    // Linux is pinned as in ci.yml (ubuntu-latest is 26.04, where whisper-rs-sys does not build), and is what sets the glibc floor.
    expect(row("linux-x64")).toMatchObject({ os: "ubuntu-24.04", target: "x86_64-unknown-linux-gnu", bundles: "deb" });
    expect(workflow).not.toContain("ubuntu-latest");
  });

  test("the_intel_mac_is_cross_built_on_the_arm64_runner_or_built_on_an_intel_one_and_the_row_says_which", () => {
    // Decision 5: the cross build first; the row moves to macos-15-intel when the native libraries refuse it.
    const intel = row("macos-x64");
    expect(intel.target).toBe("x86_64-apple-darwin");
    expect(intel.bundles).toBe("dmg");
    expect(["macos-latest", "macos-15-intel"]).toContain(intel.os);
  });

  test("every_row_fails_on_its_own_and_does_not_cancel_the_others", () => {
    expect(job("build")).toMatch(/^ {6}fail-fast: false$/m);
  });

  test("the_installers_are_unsigned_and_the_deb_is_the_only_package_for_linux", () => {
    expect(step("build", "pnpm --dir app tauri build")).toMatch(/APPLE_SIGNING_IDENTITY: "-"/);
    expect(workflow).not.toMatch(/appimage|\.rpm|rpm,/i);
    expect(workflow).not.toMatch(/APPLE_(CERTIFICATE|ID|PASSWORD|API_KEY|TEAM_ID)|TAURI_SIGNING_PRIVATE_KEY/);
  });

  test("the_macos_rows_build_for_macos_13_like_the_plist_and_the_ffmpeg_say", () => {
    const deployment = step("build", "MACOSX_DEPLOYMENT_TARGET");
    expect(deployment).toContain("if: runner.os == 'macOS'");
    expect(deployment).toContain('MACOSX_DEPLOYMENT_TARGET=13.0');
    expect(job("build").indexOf(deployment)).toBeLessThan(job("build").indexOf("pnpm --dir app tauri build"));
  });
});

describe("the Tauri build", () => {
  const build = () => step("build", "pnpm --dir app tauri build");

  test("it_gets_the_target_the_bundle_and_the_release_overlay", () => {
    expect(build()).toContain("pnpm --dir app tauri build --target ${{ matrix.target }} --bundles ${{ matrix.bundles }} --config src-tauri/tauri.release.conf.json");
    expect(workflow.match(/tauri build/g)).toHaveLength(1);
  });

  test("the_overlay_is_the_only_place_that_names_the_sidecars_so_that_an_ordinary_build_needs_none", () => {
    // `externalBin` in tauri.conf.json would make every `cargo build`, `cargo test` and `cargo clippy` fail without the staged files.
    expect(read("app/src-tauri/tauri.conf.json")).not.toMatch(/externalBin/);
    expect(read("app/src-tauri/tauri.conf.json")).not.toMatch(/"resources"/);
    expect(read("app/src-tauri/tauri.conf.json")).not.toMatch(/"files"/);
    const overlay = readJson("app/src-tauri/tauri.release.conf.json");
    expect(overlay.bundle.externalBin).toEqual(["binaries/dictate-mcp", "binaries/dictate-ffmpeg"]);
  });

  test("the_deb_puts_the_two_sherpa_libraries_where_the_run_path_of_the_program_looks_and_the_licenses_in_the_doc_folder", () => {
    const overlay = readJson("app/src-tauri/tauri.release.conf.json");
    expect(overlay.bundle.linux.deb.files).toEqual({
      "/usr/lib/dictate/libsherpa-onnx-c-api.so": "libs/libsherpa-onnx-c-api.so",
      "/usr/lib/dictate/libonnxruntime.so": "libs/libonnxruntime.so",
      // The licenses of what is inside ask that their texts go with it: /usr/share/doc/dictate, and the `copyright` file of Debian.
      "/usr/share/doc/dictate/LICENSE": "licenses/LICENSE",
      "/usr/share/doc/dictate/THIRD_PARTY_NOTICES.md": "licenses/THIRD_PARTY_NOTICES.md",
      "/usr/share/doc/dictate/THIRD_PARTY_LICENSES.txt": "licenses/THIRD_PARTY_LICENSES.txt",
      "/usr/share/doc/dictate/COPYING.LGPLv2.1": "licenses/COPYING.LGPLv2.1",
      "/usr/share/doc/dictate/copyright": "linux/copyright",
    });
    // /usr/bin/dictate finds /usr/lib/dictate through the run path that build.rs gives it (and tests/build_rpath.rs pins).
    expect(read("app/src-tauri/build_rpath.rs")).toContain('"$ORIGIN/../lib/dictate"');
    // Files that stage-sidecars.sh puts in libs/ (the same names, in the folder that the overlay names).
    expect(read("scripts/stage-sidecars.sh")).toContain('lib_dir="app/src-tauri/libs"');
  });

  test("the_program_of_the_installer_gets_only_the_run_path_of_the_installed_package", () => {
    // The entries for a build in the source tree ($ORIGIN/.., $ORIGIN/../../../..) would make /usr/bin/dictate look in /usr and in /.
    const variable = /pub const INSTALLED_ONLY_VARIABLE: &str = "([A-Z_]+)";/.exec(read("app/src-tauri/build_rpath.rs"))?.[1];
    expect(variable).toBe("DICTATE_RPATH_INSTALLED");
    expect(build()).toContain(`${variable}: "1"`);
    // Only the build of the installer: the stage of the sidecars and the tests are the builds of the source tree.
    expect(workflow.match(new RegExp(`${variable}`, "g"))).toHaveLength(1);
  });

  test("the_windows_runtime_goes_next_to_the_program_through_a_second_overlay_that_only_the_windows_row_gets", () => {
    // `bundle.resources` is read on every system, and the DLLs exist only where stage-sidecars.sh staged them: in the file that
    // every row gets they would fail the macOS and Linux builds (a missing resource is an error).
    expect(readJson("app/src-tauri/tauri.release.conf.json").bundle).not.toHaveProperty("resources");
    const windows = readJson("app/src-tauri/tauri.release.windows.conf.json");
    expect(windows.bundle.resources).toEqual({
      "libs/vcruntime140.dll": "vcruntime140.dll",
      "libs/vcruntime140_1.dll": "vcruntime140_1.dll",
      "libs/msvcp140.dll": "msvcp140.dll",
      "libs/msvcp140_1.dll": "msvcp140_1.dll",
      // The license files go in a `Licenses` folder of the install folder, next to the program.
      ...Object.fromEntries(LICENSE_FILES.map((name) => [`licenses/${name}`, `Licenses/${name}`])),
    });
    expect(row("windows-x64").extra_config).toBe("--config src-tauri/tauri.release.windows.conf.json");
    expect(row("linux-x64")).not.toHaveProperty("extra_config");
    expect(build()).toContain("${{ matrix.extra_config }}");
  });

  test("the_license_files_go_in_contents_resources_licenses_of_the_mac_app_through_an_overlay_that_only_the_mac_rows_get", () => {
    // Not in the overlay that every row gets: `bundle.resources` would put them in /usr/lib/Dictate on Linux too, where the deb has its own place.
    const mac = readJson("app/src-tauri/tauri.release.macos.conf.json");
    expect(mac.bundle.resources).toEqual(Object.fromEntries(LICENSE_FILES.map((name) => [`licenses/${name}`, `Licenses/${name}`])));
    for (const name of ["macos-arm64", "macos-x64"]) {
      expect(row(name).extra_config, name).toBe("--config src-tauri/tauri.release.macos.conf.json");
    }
  });

  test("the_license_files_are_staged_by_the_script_from_the_root_and_are_never_committed", () => {
    const stage = read("scripts/stage-sidecars.sh");
    expect(stage).toContain('licenses_dir="app/src-tauri/licenses"');
    for (const name of LICENSE_FILES) expect(stage, name).toContain(name);
    expect(read(".gitignore")).toMatch(/^app\/src-tauri\/licenses\/$/m);
    // The copyright file of the deb is the one thing that is in the repository, because nothing makes it.
    const copyright = read("app/src-tauri/linux/copyright");
    expect(copyright).toMatch(/^Format: https:\/\/www\.debian\.org\/doc\/packaging-manuals\/copyright-format\/1\.0\/$/m);
    expect(copyright).toMatch(/^License: MIT$/m);
    expect(copyright).toContain("THIRD_PARTY_LICENSES.txt");
    expect(copyright).toContain("/usr/share/doc/dictate/COPYING.LGPLv2.1");
  });

  test("tauri_action_is_not_used_so_nothing_can_blank_the_notes_that_release_please_wrote", () => {
    // tauri-action given a tag that already has a release updates it with an empty name and body.
    for (const input of ["tagName", "releaseId", "releaseName", "releaseBody", "releaseDraft", "tauri-apps/tauri-action"]) {
      expect(workflow, input).not.toContain(input);
    }
    expect(workflow).not.toMatch(/gh release (create|edit|delete)/);
  });

  test("the_sidecars_are_built_for_the_target_into_its_own_folder_so_that_they_cannot_replace_the_binary_that_the_tests_run", () => {
    // The Tauri bundler copies the staged dictate-mcp to target/<target>/release/dictate-mcp. The tests of the MCP server run
    // target/debug/dictate-mcp, which is another folder as long as the release build is for a target.
    const stage = step("build", "scripts/stage-sidecars.sh");
    expect(stage).toContain("bash scripts/stage-sidecars.sh ${{ matrix.target }}");
    expect(read("scripts/stage-sidecars.sh")).toContain("cargo build -p dictate-mcp --release --locked --target");
    expect(workflow).not.toMatch(/tauri build(?!.*--target)/);
  });

  test("the_sidecars_are_staged_after_the_ffmpeg_is_checked_and_before_the_bundler_asks_for_them", () => {
    const text = job("build");
    const check = text.indexOf("scripts/check-ffmpeg-lgpl.sh");
    const stage = text.indexOf("scripts/stage-sidecars.sh");
    const bundle = text.indexOf("pnpm --dir app tauri build");
    expect(check).toBeGreaterThan(-1);
    expect(stage).toBeGreaterThan(check);
    expect(bundle).toBeGreaterThan(stage);
  });
});

describe("the ffmpeg of each installer", () => {
  test("the_windows_program_is_built_by_a_linux_job_with_mingw_and_the_windows_row_takes_it_from_its_artifact", () => {
    const linux = job("ffmpeg-windows");
    expect(linux).toContain("runs-on: ubuntu-24.04");
    expect(linux).toContain("bash scripts/build-ffmpeg.sh x86_64-pc-windows-msvc target/ffmpeg/x86_64-pc-windows-msvc");
    expect(linux).toContain("gcc-mingw-w64-x86-64-posix");
    expect(linux).toContain("bash scripts/check-ffmpeg-lgpl.sh target/ffmpeg/x86_64-pc-windows-msvc/dictate-ffmpeg.exe");
    expect(linux).toContain("name: dictate-ffmpeg-x86_64-pc-windows-msvc");

    expect(job("build")).toMatch(/^ {4}needs: \[preflight, ffmpeg-windows\]$/m);
    const download = step("build", "actions/download-artifact@v4");
    expect(download).toContain("if: runner.os == 'Windows'");
    expect(download).toContain("name: dictate-ffmpeg-x86_64-pc-windows-msvc");
    expect(download).toContain("path: target/ffmpeg/x86_64-pc-windows-msvc");
    // The other rows build their own: on their own system.
    const build = step("build", "bash scripts/build-ffmpeg.sh ${{ matrix.target }}");
    expect(build).toContain("runner.os != 'Windows'");
  });

  test("the_build_is_cached_under_the_key_of_ci_yml_so_that_a_release_finds_what_the_pull_requests_built", () => {
    const key = /key: (ffmpeg-\$\{\{ env\.FFMPEG_VERSION \}\}-\$\{\{ [a-z.]+ \}\}-\$\{\{ hashFiles\('scripts\/build-ffmpeg\.sh'\) \}\})/;
    const ciKey = key.exec(ci)?.[1];
    expect(ciKey, "the key in ci.yml").toBeDefined();
    // ci.yml names the target `matrix.target`; the Windows job has no matrix, so it spells the target out.
    expect(workflow).toContain(`key: ${ciKey}`);
    expect(workflow).toContain("key: ffmpeg-${{ env.FFMPEG_VERSION }}-x86_64-pc-windows-msvc-${{ hashFiles('scripts/build-ffmpeg.sh') }}");
    expect(workflow).toContain("path: target/ffmpeg/${{ matrix.target }}");
    // The version in the key is the one that the script builds, and the one of ci.yml.
    const version = /^ {2}FFMPEG_VERSION: (\S+)$/m;
    expect(version.exec(workflow)?.[1]).toBe(version.exec(ci)?.[1]);
    expect(read("scripts/build-ffmpeg.sh")).toContain(`VERSION="${version.exec(workflow)?.[1]}"`);
  });

  test("each_ffmpeg_is_checked_to_be_a_plain_lgpl_build_before_it_is_used", () => {
    const text = job("build");
    const check = step("build", "bash scripts/check-ffmpeg-lgpl.sh");
    expect(check).toContain("target/ffmpeg/${{ matrix.target }}/dictate-ffmpeg${{ matrix.exe }}");
    expect(text.indexOf(check)).toBeGreaterThan(text.indexOf("bash scripts/build-ffmpeg.sh"));
    expect(text.indexOf(check)).toBeLessThan(text.indexOf("bash scripts/stage-sidecars.sh"));
  });
});

describe("what the workflow builds, and what it is allowed to keep of its token", () => {
  /** The `actions/checkout` steps of the whole workflow, each with its `with:` lines. */
  const checkouts = () => workflow.match(/- uses: actions\/checkout@v4\n(?: {8}.*\n)*/g) ?? [];

  test("every_job_builds_the_commit_of_the_tag_and_a_dry_run_builds_the_commit_it_was_started_on", () => {
    // Started by hand on main with a tag (`gh workflow run release.yml --ref main -f tag=v2.0.0`), the default checkout is the tip of main, which
    // may be a later commit than the tag: the installers of one commit would be attached to the release of another, with --clobber.
    expect(checkouts()).toHaveLength(4);
    for (const name of ["preflight", "ffmpeg-windows", "build", "sources"]) {
      expect(stepsOf(name)[0], `${name}: the first step`).toContain("actions/checkout@v4");
      expect(stepsOf(name)[0], name).toContain("ref: ${{ inputs.tag || github.sha }}");
    }
    for (const checkout of checkouts()) expect(checkout).toContain("ref: ${{ inputs.tag || github.sha }}");
  });

  test("no_checkout_leaves_the_token_in_the_git_config_of_the_runner", () => {
    // The uploads take the token from GH_TOKEN: nothing that runs after a checkout (the build runs the scripts and the dependencies of the
    // repository) can read it from .git/config.
    for (const checkout of checkouts()) expect(checkout).toContain("persist-credentials: false");
  });

  test("the_version_in_the_name_of_the_installer_is_the_version_of_the_tag_or_the_row_fails", () => {
    // The files are named from the version of Cargo.toml, which release-please sets in the release PR: a tag that is not that version
    // would attach an installer with another number in its name.
    const found = step("build", "id: installer");
    expect(found).toContain("TAG: ${{ inputs.tag }}");
    expect(found).toContain('version="$(basename "${files[0]}" | cut -d_ -f2)"');
    expect(found).toMatch(/if \[ -n "\$TAG" \] && \[ "\$version" != "\$\{TAG#v\}" \]; then/);
    expect(found).toMatch(/::error::[^\n]*\$version[^\n]*\$TAG/);
    expect(found.indexOf('"$version" != "${TAG#v}"')).toBeLessThan(found.indexOf('echo "path='));
  });
});

describe("the license files are checked before anything is built, for every run", () => {
  const preflight = () => job("preflight");

  test("a_job_that_runs_first_fails_any_run_that_lacks_one_of_the_license_files_a_dry_run_included", () => {
    // stage-sidecars.sh stops without any of the four, so a dry run that goes on without one would only fail an hour later: the same rule
    // for every run, and no branch that says "a dry run goes on".
    const check = step("preflight", "THIRD_PARTY_NOTICES.md");
    for (const name of LICENSE_FILES) expect(check, name).toContain(name);
    expect(check).toMatch(/if \[ ! -f "\$name" \]; then/);
    expect(check).toMatch(/::error::[^\n]*\$name[^\n]*stage-sidecars\.sh/);
    expect(check).toContain('exit "$missing"');
    expect(check).not.toContain("::warning::");
    expect(check).not.toContain("TAG");
    expect(check).not.toContain("inputs.tag");
    // It has the files of the commit that is built.
    expect(stepsOf("preflight")[0]).toContain("ref: ${{ inputs.tag || github.sha }}");
    expect(preflight()).not.toMatch(/^ {4}needs:/m);
  });

  test("the_files_it_names_are_the_ones_the_staging_script_needs", () => {
    const stage = read("scripts/stage-sidecars.sh");
    const loop = /for license in ([^;]+); do/.exec(stage)?.[1].split(/\s+/);
    expect(loop).toEqual(LICENSE_FILES);
    const check = step("preflight", "THIRD_PARTY_NOTICES.md");
    expect(/for name in ([^;]+); do/.exec(check)?.[1].split(/\s+/)).toEqual(LICENSE_FILES);
  });

  test("nothing_is_built_before_it_and_the_job_that_attaches_the_files_still_has_its_own_rule", () => {
    expect(job("ffmpeg-windows")).toMatch(/^ {4}needs: preflight$/m);
    expect(job("build")).toMatch(/^ {4}needs: \[preflight, ffmpeg-windows\]$/m);
    // The later job copies the file and keeps the rule: it is the one that attaches it.
    expect(step("sources", "THIRD_PARTY_NOTICES.md")).toContain("if [ ! -f THIRD_PARTY_NOTICES.md ]; then");
  });
});

describe("the ffmpeg job for Windows installs each package once", () => {
  test("one_apt_get_update_and_the_objdump_of_mingw_in_one_step_the_compilers_only_when_the_build_is_not_cached", () => {
    const text = job("ffmpeg-windows");
    expect(text.match(/apt-get update/g)).toHaveLength(1);
    expect(text.match(/binutils-mingw-w64-x86-64/g)).toHaveLength(1);
    // What the check reads the program with: installed whether or not the build came from the cache.
    const objdump = step("ffmpeg-windows", "binutils-mingw-w64-x86-64");
    expect(objdump).not.toContain("cache-hit");
    expect(objdump).toContain("apt-get update");
    // What the build needs: only on a miss, after the update of the step above.
    const compilers = step("ffmpeg-windows", "gcc-mingw-w64-x86-64-posix");
    expect(compilers).toContain("if: steps.ffmpeg-build.outputs.cache-hit != 'true'");
    expect(compilers).not.toContain("apt-get update");
    expect(text.indexOf(objdump)).toBeLessThan(text.indexOf(compilers));
  });
});

describe("the same pins as ci.yml, so that a bump of one cannot leave the other behind", () => {
  const pinned = (text: string, pattern: RegExp) => pattern.exec(text)?.[1];

  test("the_sha256_of_the_windows_sherpa_archive_and_the_archive_are_ci_ymls", () => {
    const sha = /^ {2}SHERPA_ONNX_MD_SHA256: ([0-9a-f]{64})$/m;
    expect(pinned(workflow, sha)).toBeDefined();
    expect(pinned(workflow, sha)).toBe(pinned(ci, sha));
    const archive = /(https:\/\/github\.com\/k2-fsa\/sherpa-onnx\/releases\/download\/v[0-9.]+\/sherpa-onnx-v[0-9.]+-win-x64-static-MD-Release-no-tts-lib\.tar\.bz2)/;
    expect(pinned(workflow, archive)).toBeDefined();
    expect(pinned(workflow, archive)).toBe(pinned(ci, archive));
    // Checked before it is unpacked, and a different one stops the job.
    const windows = step("build", "Get-FileHash -Algorithm SHA256");
    expect(windows.indexOf("curl.exe")).toBeLessThan(windows.indexOf("Get-FileHash"));
    expect(windows.indexOf("Get-FileHash")).toBeLessThan(windows.indexOf("tar.exe"));
    expect(windows).toContain("$env:SHERPA_ONNX_MD_SHA256");
    expect(windows).toMatch(/-ne\b/);
    expect(windows).toContain("throw");
    // The stand-ins for the libraries that the build without text to speech does not have.
    for (const name of ["espeak-ng", "piper_phonemize", "ucd"]) expect(windows).toContain(`'${name}'`);
    expect(windows).toContain("Copy-Item (Join-Path $dir 'lib/kissfft-float.lib')");
  });

  test("the_caches_of_the_sherpa_libraries_have_the_keys_of_ci_yml", () => {
    for (const key of [
      /key: (sherpa-onnx-md-no-tts-1\.13\.8-542348e5-\$\{\{ runner\.arch \}\})/,
      /key: (sherpa-onnx-no-tts-1\.13\.8-bf2d998c-\$\{\{ runner\.arch \}\})/,
      /key: (sherpa-onnx-1\.13\.8-\$\{\{ runner\.os \}\}-\$\{\{ runner\.arch \}\})/,
    ]) {
      expect(pinned(workflow, key), String(key)).toBeDefined();
      expect(pinned(workflow, key)).toBe(pinned(ci, key));
    }
  });

  test("linux_gets_its_sherpa_libraries_from_the_script_that_pins_them_and_the_build_dependencies_of_ci_yml", () => {
    const fetch = step("build", "bash scripts/fetch-sherpa-onnx-no-tts.sh");
    expect(fetch).toContain("runner.os == 'Linux'");
    expect(fetch).toContain('echo "SHERPA_ONNX_LIB_DIR=$lib" >> "$GITHUB_ENV"');
    // The packages of the CI jobs, which the README lists too (ci-workflow.test.ts pins them there).
    const packages = /apt-get install -y\n\s+(cmake libclang-dev .*)\n\s+(libwebkit2gtk-4\.1-dev .*)(?:\n|$)/;
    const ours = packages.exec(step("build", "libwebkit2gtk-4.1-dev"));
    const theirs = packages.exec(ci);
    expect(ours, "the Linux build dependencies").not.toBeNull();
    expect(`${ours![1]} ${ours![2]}`).toBe(`${theirs![1]} ${theirs![2]}`);
  });

  test("the_rust_is_1_99_with_the_target_of_the_row_and_it_beats_the_stable_of_rust_toolchain_toml", () => {
    // tauri-build 2.6.3 (Tauri 2.11) takes the folder of the program to be three levels above OUT_DIR, which is where Cargo until 1.99 builds
    // it. Cargo 1.100 builds it in build/<package>/<hash>/out, so the sidecars are copied into target/<target>/release/build, where the
    // folder that Cargo made for dictate-mcp stops the copy. tauri-build 2.7 (Tauri 2.12) walks up to `build`: the pin goes with that bump.
    const rust = step("build", "- name: Install Rust 1.99");
    expect(rust).toContain("rustup toolchain install 1.99.1 --profile minimal --target ${{ matrix.target }}");
    expect(rust).toContain('echo "RUSTUP_TOOLCHAIN=1.99.1" >> "$GITHUB_ENV"');
    // Before anything is built, and before the cache of the build reads which compiler it is for.
    const text = job("build");
    expect(text.indexOf(rust)).toBeLessThan(text.indexOf("Swatinem/rust-cache@v2"));
    expect(text.indexOf(rust)).toBeLessThan(text.indexOf("scripts/stage-sidecars.sh"));
    expect(read("rust-toolchain.toml")).toContain('channel = "stable"');
    expect(workflow).not.toContain("dtolnay/rust-toolchain");
  });
});

describe("what happens to an installer once it is built", () => {
  const order = (...texts: string[]) => {
    const text = job("build");
    const at = texts.map((candidate) => text.indexOf(candidate));
    for (const [i, found] of at.entries()) expect(found, texts[i]).toBeGreaterThan(-1);
    expect(at, texts.join(" < ")).toEqual([...at].sort((a, b) => a - b));
  };

  test("check_artifact_runs_on_the_installer_before_the_checksum_and_before_anything_is_uploaded", () => {
    const check = step("build", "bash scripts/check-artifact.sh");
    expect(check).toContain('bash scripts/check-artifact.sh "$INSTALLER"');
    // The step is not allowed to fail quietly.
    expect(check).not.toContain("continue-on-error");
    expect(check).not.toContain("|| true");
    order("pnpm --dir app tauri build", "bash scripts/check-artifact.sh", 'sha256sum "$name"', "actions/upload-artifact@v4", "gh release upload");
  });

  test("a_sha256_file_is_written_next_to_each_installer_as_hash_two_spaces_and_the_name_and_checked", () => {
    const sums = step("build", "- name: Write the checksum");
    // The hash is made in the folder of the installer, so that the file has the name and not the path.
    expect(sums).toMatch(/cd "\$\(dirname "\$INSTALLER"\)"/);
    // The line is written here, with the two blanks: the sha256sum of Git Bash on Windows writes `<hash> *<name>` (binary mode), which the
    // verification of a person on another system would read as a file called `*name`.
    expect(sums).toContain("printf '%s  %s\\n' \"$hash\" \"$name\" > \"$name.sha256\"");
    expect(sums).not.toMatch(/sha256sum "\$name" >|shasum -a 256 "\$name" >/);
    // Both ways of hashing, and the check of the file that was written with the tool of the system.
    expect(sums).toContain('sha256sum "$name"');
    expect(sums).toContain('shasum -a 256 "$name"');
    expect(sums).toContain('sha256sum -c "$name.sha256"');
    expect(sums).toContain('shasum -a 256 -c "$name.sha256"');
  });

  test("the_installer_and_its_checksum_are_kept_as_a_workflow_artifact_in_a_dry_run_too", () => {
    const keep = step("build", "name: installer-${{ matrix.name }}");
    expect(keep).toContain("actions/upload-artifact@v4");
    expect(keep).toContain("${{ steps.installer.outputs.path }}");
    expect(keep).toContain("${{ steps.installer.outputs.path }}.sha256");
    expect(keep).toContain("if-no-files-found: error");
    expect(keep).not.toMatch(/^\s+if:/m);
  });

  test("the_release_gets_the_installer_and_its_checksum_only_when_a_tag_is_given", () => {
    const upload = step("build", "gh release upload");
    expect(upload).toContain("if: inputs.tag != ''");
    expect(upload).toContain('gh release upload "$TAG" "$INSTALLER" "$INSTALLER.sha256" --clobber');
    expect(upload).toContain("GH_TOKEN: ${{ github.token }}");
    // The tag goes in through the environment: a tag is text that somebody typed.
    expect(upload).toContain("TAG: ${{ inputs.tag }}");
    expect(upload).not.toMatch(/gh release upload .*\$\{\{/);
    // It is the only step of the row that talks to a release.
    expect(job("build").match(/gh release/g)).toHaveLength(1);
  });

  test("exactly_one_installer_is_taken_for_the_row_and_its_absence_fails_the_row", () => {
    const found = step("build", "id: installer");
    expect(found).toContain('"${#files[@]}" -ne 1');
    expect(found).toContain("exit 1");
    expect(found).toContain('echo "path=${files[0]}" >> "$GITHUB_OUTPUT"');
    // Where the bundler writes each kind, for the target of the row.
    for (const kind of [
      "target/${{ matrix.target }}/release/bundle/dmg/*.dmg",
      "target/${{ matrix.target }}/release/bundle/nsis/*-setup.exe",
      "target/${{ matrix.target }}/release/bundle/deb/*.deb",
    ]) {
      expect(workflow, kind).toContain(kind);
    }
  });

  test("every_command_runs_in_bash_so_that_the_windows_row_reads_the_same_script_as_the_others", () => {
    expect(job("build")).toMatch(/^ {4}defaults:\n {6}run:\n {8}shell: bash$/m);
  });
});

describe("the files that go with the installers: the ffmpeg source, the build lines and the notices", () => {
  const sources = () => job("sources");

  test("the_job_runs_after_every_installer_and_after_the_ffmpeg_that_made_the_source", () => {
    expect(sources()).toMatch(/^ {4}needs: \[ffmpeg-windows, build\]$/m);
    // No `if: always()`: when an installer failed, nothing else is attached.
    expect(sources()).not.toContain("always()");
  });

  test("the_source_of_ffmpeg_comes_from_the_job_that_built_it_and_the_build_lines_from_every_row", () => {
    expect(job("ffmpeg-windows")).toContain("name: ffmpeg-source");
    expect(job("ffmpeg-windows")).toContain("target/ffmpeg/x86_64-pc-windows-msvc/ffmpeg-${{ env.FFMPEG_VERSION }}.tar.xz");
    expect(sources()).toContain("name: ffmpeg-source");
    expect(sources()).toContain("pattern: buildconf-*");
    expect(sources()).toContain("merge-multiple: true");
    // Every row keeps the line that its ffmpeg was configured with (the written offer of the source points at it).
    const keep = step("build", "name: buildconf-${{ matrix.target }}");
    expect(keep).toContain("target/ffmpeg/${{ matrix.target }}/BUILDCONF-${{ matrix.target }}.txt");
    expect(keep).toContain("if-no-files-found: error");
  });

  test("all_of_them_are_there_before_anything_is_attached_and_the_job_says_what_is_missing", () => {
    const list = step("sources", "- name: Check that every file is there");
    expect(list).toContain("ffmpeg-${{ env.FFMPEG_VERSION }}.tar.xz");
    expect(list).toContain("BUILDCONF-aarch64-apple-darwin.txt");
    expect(list).toContain("BUILDCONF-x86_64-apple-darwin.txt");
    expect(list).toContain("BUILDCONF-x86_64-pc-windows-msvc.txt");
    expect(list).toContain("BUILDCONF-x86_64-unknown-linux-gnu.txt");
    expect(list).toContain("missing=1");
    expect(list).toContain('exit "$missing"');
    expect(sources().indexOf(list)).toBeLessThan(sources().indexOf("gh release upload"));
  });

  test("the_notices_are_attached_and_a_run_without_them_fails", () => {
    const notices = step("sources", "THIRD_PARTY_NOTICES.md");
    // The LGPL, Apache-2.0, MIT and CC-BY licenses of what is inside ask for them, and no installer is built without them (preflight, and
    // stage-sidecars.sh): this job, run alone, has the same rule for a dry run and for a tag.
    expect(notices).toContain("if [ ! -f THIRD_PARTY_NOTICES.md ]; then");
    expect(notices).toMatch(/::error::[^\n]*THIRD_PARTY_NOTICES\.md/);
    expect(notices).toContain("exit 1");
    expect(notices).toContain("cp THIRD_PARTY_NOTICES.md release-files/");
    expect(notices).not.toContain("::warning::");
    expect(notices).not.toContain("TAG");
  });

  test("they_are_attached_only_when_a_tag_is_given_and_kept_as_an_artifact_always", () => {
    const keep = step("sources", "name: release-files");
    expect(keep).toContain("actions/upload-artifact@v4");
    expect(keep).not.toMatch(/^\s+if:/m);
    const upload = step("sources", "gh release upload");
    expect(upload).toContain("if: inputs.tag != ''");
    expect(upload).toMatch(/gh release upload "\$TAG" release-files\/\* --clobber/);
    expect(job("sources").match(/gh release/g)).toHaveLength(1);
  });
});
