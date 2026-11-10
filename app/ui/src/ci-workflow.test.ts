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

describe("the /MD build of sherpa-onnx that Windows links", () => {
  const ARCHIVE = "sherpa-onnx-v1.13.8-win-x64-static-MD-Release-lib.tar.bz2";
  const pinned = /^ {2}SHERPA_ONNX_MD_SHA256: ([0-9a-f]{64})$/m.exec(workflow)?.[1];

  test("the SHA-256 of the archive is pinned once, at the top of the workflow, in lowercase hex", () => {
    expect(pinned).toBeDefined();
    // The release asset's own digest, as GitHub lists it (and as it was computed from a download).
    expect(pinned).toBe("a0f44cd91486e448c2be1f1d3662edb4f473ca3cb38a803cee158760cc588428");
    expect(workflow.match(/SHERPA_ONNX_MD_SHA256: /g)).toHaveLength(1);
    expect(workflow.indexOf("SHERPA_ONNX_MD_SHA256:")).toBeLessThan(workflow.indexOf("\njobs:"));
  });

  test("each job that downloads it checks it before it is unpacked, and a different one stops the job", () => {
    const downloads = steps().filter((step) => step.includes(ARCHIVE));
    // The `test` job and the `app` job.
    expect(downloads).toHaveLength(2);
    for (const step of downloads) {
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
