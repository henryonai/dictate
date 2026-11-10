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

/** The jobs of the workflow, as text: `test:`, `app:` and `msrv:`. */
function job(name: string): string {
  const from = workflow.indexOf(`\n  ${name}:\n`);
  expect(from, `job ${name}`).toBeGreaterThan(-1);
  const next = workflow.slice(from + 1).search(/\n {2}[a-z]+:\n/);
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
    // The `test` job and the `app` job.
    expect(downloads()).toHaveLength(2);
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
    for (const name of ["test", "app"]) {
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
      if (name !== "msrv") expect(step).toContain("runner.os == 'Linux'");
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
    for (const name of ["test", "app"]) {
      const prebuilt = stepsOf(name).filter((step) => step.includes("path: target/sherpa-onnx-prebuilt"));
      expect(prebuilt, name).toHaveLength(1);
      expect(prebuilt[0]).toContain("if: runner.os == 'macOS'");
      expect(prebuilt[0]).not.toContain("shared");
    }
    // msrv is Linux only: it has no use for the download of the sherpa-onnx-sys build script any more.
    expect(stepsOf("msrv").filter((step) => step.includes("sherpa-onnx-prebuilt"))).toHaveLength(0);
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
