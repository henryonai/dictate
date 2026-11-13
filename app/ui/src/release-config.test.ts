// What release-please promises about versions, held in place: the app, every crate and the web UI have one version, the config of
// release-please bumps each place that holds it, and a release calls the workflow that builds the installers. The files are read
// from the repository (as ci-workflow.test.ts reads the CI workflow): a Windows checkout may have CRLF line ends, so they are made LF.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");
const readJson = (file: string) => JSON.parse(read(file));

type ExtraFile = { type: string; path: string; jsonpath?: string };

/** The lines of a `[table]` of a TOML file, up to the next table (a `[[array]]` entry starts one too). */
function table(toml: string, name: string): string {
  const header = `\n[${name}]\n`;
  const text = `\n${toml}`;
  const from = text.indexOf(header);
  expect(from, `[${name}]`).toBeGreaterThan(-1);
  const body = text.slice(from + header.length);
  const next = body.search(/^\[/m);
  return next === -1 ? body : body.slice(0, next);
}

/** The members of the Cargo workspace, as the folders that hold their Cargo.toml (`crates/*` is every folder of `crates`). */
function memberFolders(): string[] {
  const members = /^members = \[(.*)\]$/m.exec(read("Cargo.toml"))?.[1];
  expect(members, "the workspace members").toBeDefined();
  return [...members!.matchAll(/"([^"]+)"/g)].flatMap(([, glob]) => {
    if (!glob.endsWith("/*")) return [glob];
    const parent = glob.slice(0, -2);
    return readdirSync(join(repoRoot, parent), { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && existsSync(join(repoRoot, parent, entry.name, "Cargo.toml")))
      .map((entry) => `${parent}/${entry.name}`);
  });
}

const workspaceVersion = () => /^version = "([^"]+)"$/m.exec(table(read("Cargo.toml"), "workspace.package"))?.[1];
const crateName = (folder: string) => /^name = "([^"]+)"$/m.exec(table(read(`${folder}/Cargo.toml`), "package"))?.[1];
const config = () => readJson("release-please-config.json");
const extraFiles = (): ExtraFile[] => config()["extra-files"] ?? [];

describe("one version for the app, every crate and the web UI", () => {
  test("every_version_is_the_same", () => {
    const version = workspaceVersion();
    expect(version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(readJson("app/ui/package.json").version).toBe(version);
    expect(readJson(".release-please-manifest.json")["."]).toBe(version);
    expect(read("version.txt").trim()).toBe(version);
  });

  test("every_crate_takes_the_version_of_the_workspace_and_the_tauri_config_has_none", () => {
    const folders = memberFolders().sort();
    // The three libraries and the app: a new crate has to say what it does about its version.
    expect(folders).toEqual(["app/src-tauri", "crates/dictate-core", "crates/dictate-mcp", "crates/dictate-services"]);
    for (const folder of folders) {
      const manifest = table(read(`${folder}/Cargo.toml`), "package");
      expect(manifest, folder).toMatch(/^version\.workspace = true$/m);
      expect(manifest, folder).not.toMatch(/^version = /m);
    }
    // Tauri reads the version of the app from app/src-tauri/Cargo.toml when tauri.conf.json has none.
    expect(readJson("app/src-tauri/tauri.conf.json")).not.toHaveProperty("version");
  });

  test("cargo_lock_lists_every_crate_at_that_version", () => {
    const lock = read("Cargo.lock");
    for (const folder of memberFolders()) {
      const entry = new RegExp(`^name = "${crateName(folder)}"\\nversion = "([^"]+)"$`, "m").exec(lock);
      expect(entry?.[1], folder).toBe(workspaceVersion());
    }
  });
});

describe("the config of release-please", () => {
  test("release_please_updates_every_version_file", () => {
    const files = extraFiles();
    // The version of the workspace, and the one of the web UI.
    expect(files).toContainEqual({ type: "toml", path: "Cargo.toml", jsonpath: "$.workspace.package.version" });
    expect(files).toContainEqual({ type: "json", path: "app/ui/package.json", jsonpath: "$.version" });
    // Cargo.lock has the version of each crate of the workspace, and `cargo test --locked` fails when one is behind: each is bumped.
    // release-please wraps every value of a TOML file with its position, so the name of a crate is `name.value` in the path.
    const lock = files.filter((file) => file.path === "Cargo.lock");
    const crates = memberFolders().map(crateName);
    expect(crates).toHaveLength(4);
    expect(lock.map((file) => file.jsonpath).sort()).toEqual(
      crates.map((name) => `$.package[?(@.name.value=='${name}')].version`).sort(),
    );
    for (const file of lock) expect(file.type).toBe("toml");
    // `version.txt` is the one that the `simple` release type bumps itself.
    expect(config()["release-type"]).toBe("simple");
    expect(files.map((file) => file.path)).not.toContain("version.txt");
    // A path that is not there stops release-please with an error.
    for (const file of files) expect(existsSync(join(repoRoot, file.path)), file.path).toBe(true);
  });

  test("the_tags_are_v1_2_3_without_the_name_of_a_component", () => {
    expect(config()["include-v-in-tag"]).toBe(true);
    expect(config()["include-component-in-tag"]).toBe(false);
    expect(config().packages).toEqual({ ".": {} });
  });

  test("the_changelog_lists_features_fixes_performance_and_reverts_and_hides_the_rest", () => {
    const sections: { type: string; hidden?: boolean }[] = config()["changelog-sections"];
    expect(sections.filter((section) => !section.hidden).map((section) => section.type)).toEqual([
      "feat",
      "fix",
      "perf",
      "revert",
    ]);
    expect(sections.filter((section) => section.hidden).map((section) => section.type)).toEqual([
      "docs",
      "refactor",
      "test",
      "ci",
      "chore",
    ]);
  });

  test("the_first_release_is_2_0_0", () => {
    // The successor of Swift Dictate 1.3.0. The task that cuts the release removes this line, so that the next one follows the commits.
    expect(config()["release-as"]).toBe("2.0.0");
  });
});

describe("the workflow of release-please", () => {
  const workflow = () => read(".github/workflows/release-please.yml");

  test("runs_on_every_push_to_main_with_the_permissions_of_release_please_and_no_others", () => {
    expect(workflow()).toMatch(/^on:\n {2}push:\n {4}branches: \[main\]\n/m);
    expect(workflow()).toMatch(/^permissions:\n {2}contents: write\n {2}pull-requests: write\n/m);
  });

  test("keeps_the_release_pr_up_to_date_from_the_config_and_the_manifest_and_says_what_it_released", () => {
    expect(workflow()).toContain("uses: googleapis/release-please-action@v4");
    expect(workflow()).toContain("config-file: release-please-config.json");
    expect(workflow()).toContain("manifest-file: .release-please-manifest.json");
    expect(workflow()).toContain("release_created: ${{ steps.release.outputs.release_created }}");
    expect(workflow()).toContain("tag_name: ${{ steps.release.outputs.tag_name }}");
  });

  test("release_please_calls_the_release_workflow_with_the_tag", () => {
    // A tag pushed with the workflow token starts no other workflow: the build is called from here, once a release is created.
    const from = workflow().indexOf("\n  build:\n");
    expect(from, "job build").toBeGreaterThan(-1);
    const build = workflow().slice(from);
    expect(build).toContain("needs: release-please");
    expect(build).toContain("if: needs.release-please.outputs.release_created == 'true'");
    expect(build).toContain("uses: ./.github/workflows/release.yml");
    expect(build).toMatch(/^ {4}with:\n {6}tag: \$\{\{ needs\.release-please\.outputs\.tag_name \}\}$/m);
  });
});
