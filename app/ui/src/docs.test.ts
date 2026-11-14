// What the documents of the project promise, held in place: the two READMEs say the same things in two languages, every link in
// them goes somewhere, the MCP server is named by its path on every system, the notices of third-party licenses carry what the
// licenses ask for, and the developer guide no longer says what the installers made untrue. The files are read as text (as
// ci-workflow.test.ts reads the CI workflow); a Windows checkout may have CRLF line ends, so they are made LF.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");

const readme = read("README.md");
const readmeVi = read("README.vi.md");
const appReadme = read("app/README.md");
const notices = read("THIRD_PARTY_NOTICES.md");

/** The documents that are written for people, which every check of links and of local paths goes over. */
const DOCUMENTS = [
  "README.md",
  "README.vi.md",
  "CONTRIBUTING.md",
  "CODE_OF_CONDUCT.md",
  "SECURITY.md",
  "THIRD_PARTY_NOTICES.md",
  "app/README.md",
  ".github/pull_request_template.md",
];

/** The text of a document without its code: fenced blocks and inline code spans, where a `](` is not a link. */
function withoutCode(text: string): string {
  return text.replace(/^(```|~~~)[^\n]*\n[\s\S]*?^\1[^\n]*$/gm, "").replace(/`[^`\n]*`/g, "");
}

/** What a link goes to, from the Markdown links and from the `href` and `src` of the HTML that the READMEs' headers use. */
function targets(text: string): string[] {
  const plain = withoutCode(text);
  const markdown = [...plain.matchAll(/\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g)].map((match) => match[1]);
  const html = [...plain.matchAll(/\b(?:href|src)="([^"]+)"/g)].map((match) => match[1]);
  return [...markdown, ...html];
}

/** The anchor that GitHub makes of a heading: lower case, no punctuation, blanks as hyphens. */
function slug(heading: string): string {
  return heading
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{M}\p{N}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

/** The anchors of a document: its headings (the first of two with one name has no suffix, the next ones have `-1`, `-2`…). */
function anchors(text: string): Set<string> {
  const seen = new Map<string, number>();
  const found = new Set<string>();
  for (const [, heading] of withoutCode(text).matchAll(/^#{1,6}\s+(.+)$/gm)) {
    const base = slug(heading.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"));
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    found.add(count === 0 ? base : `${base}-${count}`);
  }
  // The anchors that the HTML of a header makes: `<a id="...">` and `<h1 id>`.
  for (const [, id] of text.matchAll(/\bid="([^"]+)"/g)) found.add(id);
  return found;
}

/** The fenced code blocks of a document, as text: the commands, which a translation does not change. */
function codeBlocks(text: string): string[] {
  return [...text.matchAll(/^(```|~~~)[^\n]*\n([\s\S]*?)^\1[^\n]*$/gm)].map((match) => match[2].trim());
}

/** The headings of a document, by level. */
const headings = (text: string, level: number) =>
  [...withoutCode(text).matchAll(new RegExp(`^${"#".repeat(level)}\\s+(.+)$`, "gm"))].map((match) => match[1]);

describe("the two READMEs", () => {
  test("readme_links_both_languages_and_the_notices", () => {
    expect(targets(readme)).toContain("README.vi.md");
    expect(targets(readmeVi)).toContain("README.md");
    for (const text of [readme, readmeVi]) {
      expect(targets(text)).toContain("THIRD_PARTY_NOTICES.md");
      expect(targets(text)).toContain("CONTRIBUTING.md");
      expect(targets(text)).toContain("SECURITY.md");
      expect(targets(text)).toContain("CODE_OF_CONDUCT.md");
      expect(targets(text)).toContain("LICENSE");
    }
  });

  test("readme_names_the_mcp_path_for_every_system", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("/Applications/Dictate.app/Contents/MacOS/dictate-mcp");
      expect(text).toContain("%LOCALAPPDATA%\\Dictate\\dictate-mcp.exe");
      expect(text).toContain("/usr/bin/dictate-mcp");
      // The command that registers it with Claude Code, for the Mac path that the Swift app had (existing registrations keep working).
      expect(text).toContain("claude mcp add --scope user dictate -- /Applications/Dictate.app/Contents/MacOS/dictate-mcp");
      // The JSON of Claude Desktop, with the backslashes of a Windows path written twice.
      expect(text).toContain('"mcpServers"');
      expect(text).toContain("dictate-mcp.exe");
    }
    expect(readme).toMatch(/\\\\Dictate\\\\dictate-mcp\.exe/);
    expect(readmeVi).toMatch(/\\\\Dictate\\\\dictate-mcp\.exe/);
  });

  test("the_vietnamese_readme_has_the_sections_and_the_commands_of_the_english_one", () => {
    expect(headings(readmeVi, 2)).toHaveLength(headings(readme, 2).length);
    expect(headings(readmeVi, 3)).toHaveLength(headings(readme, 3).length);
    // A command is the same in both languages, and so is where every link goes.
    expect(codeBlocks(readmeVi)).toEqual(codeBlocks(readme));
    const external = (text: string) => [...new Set(targets(text).filter((target) => /^https?:/.test(target)))].sort();
    expect(external(readmeVi)).toEqual(external(readme));
  });

  test("the_installers_are_named_as_the_release_names_them", () => {
    for (const text of [readme, readmeVi]) {
      for (const name of [
        "Dictate_<version>_aarch64.dmg",
        "Dictate_<version>_x64.dmg",
        "Dictate_<version>_x64-setup.exe",
        "Dictate_<version>_amd64.deb",
      ]) {
        expect(text, name).toContain(name);
        expect(text, `${name}.sha256`).toContain(`${name}.sha256`);
      }
      expect(text).toContain("sudo apt install ./Dictate_<version>_amd64.deb");
      expect(text).not.toMatch(/AppImage/);
    }
  });

  test("the_unsigned_builds_say_how_to_open_them_and_how_to_check_them", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("xattr -dr com.apple.quarantine /Applications/Dictate.app");
      expect(text).toContain("Open Anyway");
      expect(text).toContain("SmartScreen");
      expect(text).toContain("Run anyway");
      expect(text).toContain("shasum -a 256 -c");
      expect(text).toContain("sha256sum -c");
      expect(text).toContain("Get-FileHash");
      // The floor of the Linux build: it is made on Ubuntu 24.04, with glibc 2.39.
      expect(text).toContain("glibc 2.39");
      expect(text).toContain("Ubuntu 24.04");
    }
  });

  test("the_readmes_say_how_the_ffmpeg_that_ships_is_replaced_and_that_yt_dlp_is_not_bundled", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("DICTATE_FFMPEG");
      expect(text).toContain("launchctl setenv DICTATE_FFMPEG");
      expect(text).toContain("yt-dlp");
    }
  });

  test("the_readmes_say_that_a_crash_loses_a_second_and_a_power_cut_can_lose_more", () => {
    // The files are not flushed to the disk (no fsync), so only a crash, where the system keeps what the program wrote, is bounded.
    expect(readme).toMatch(/crash[^.]*(?:at most|at the most)[^.]*second/i);
    expect(readme).toMatch(/power cut[^.]*more/i);
    expect(readme).toContain("fsync");
    expect(readmeVi).toContain("fsync");
    expect(readmeVi).toMatch(/mất điện/i);
  });

  test("the_readmes_replace_the_swift_app_and_say_what_has_to_be_done_again", () => {
    expect(readme).toMatch(/Swift/);
    expect(readmeVi).toMatch(/Swift/);
    expect(readme).toMatch(/quit the old one first/i);
  });
});

describe("the links of every document", () => {
  test.each(DOCUMENTS)("every relative link of %s goes to a file that is there, and to an anchor that is there", (file) => {
    const text = read(file);
    const own = anchors(text);
    for (const target of targets(text)) {
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // https:, mailto:
      const [path, anchor] = target.split("#");
      const resolved = path === "" ? file : join(dirname(file), path);
      expect(existsSync(join(repoRoot, resolved)), `${file}: ${target}`).toBe(true);
      if (anchor === undefined || anchor === "") continue;
      if (!resolved.endsWith(".md")) continue;
      const wanted = resolved === file ? own : anchors(read(resolved));
      expect(wanted.has(anchor), `${file}: ${target} (no such heading in ${resolved})`).toBe(true);
    }
  });

  test.each(DOCUMENTS)("%s has no path of a machine in it", (file) => {
    const text = read(file);
    expect(text).not.toMatch(/\/Users\/[a-z]/i);
    expect(text).not.toMatch(/C:\\Users\\[A-Za-z]/);
  });
});

describe("the notices of third-party licenses", () => {
  const script = read("scripts/build-ffmpeg.sh");
  const version = /^VERSION="([^"]+)"$/m.exec(script)?.[1] ?? "";
  const sha256 = /^SHA256="\$\{FFMPEG_SOURCE_SHA256:-([0-9a-f]{64})\}"$/m.exec(script)?.[1] ?? "";

  test("third_party_notices_names_the_ffmpeg_license_and_source", () => {
    expect(version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(notices).toMatch(/FFmpeg/);
    expect(notices).toContain("GNU Lesser General Public License");
    expect(notices).toMatch(/version 2\.1 or later/);
    // The exact source that the program is built from: the version and the hash that scripts/build-ffmpeg.sh pins.
    expect(notices).toContain(`ffmpeg-${version}.tar.xz`);
    expect(notices).toContain(`https://ffmpeg.org/releases/ffmpeg-${version}.tar.xz`);
    expect(notices).toContain(sha256);
    // The written offer, the build script, the configure lines, and what the build is configured without.
    expect(notices).toMatch(/written offer/i);
    expect(notices).toContain("scripts/build-ffmpeg.sh");
    expect(notices).toContain("BUILDCONF");
    expect(notices).toContain("--disable-gpl");
    expect(notices).toContain("--disable-nonfree");
    expect(notices).toContain("--disable-version3");
    // The right to replace the library, and how it is used.
    expect(notices).toContain("DICTATE_FFMPEG");
    // The trademark line that FFmpeg's legal page asks for.
    expect(notices).toMatch(/trademark of Fabrice Bellard/);
  });

  test("third_party_notices_has_the_encoders_and_the_muxers_that_the_build_script_turns_on", () => {
    // The configure lines of the notices are copies of the ones in the script (and of the BUILDCONF files of a release): a change of what
    // is built in has to be written there too, as it is what the written offer says the program was made with.
    const lists = [...script.matchAll(/--enable-(?:encoder|muxer)=([a-z0-9_,]+)/g)].map((match) => match[1]);
    expect(lists.length).toBeGreaterThanOrEqual(7);
    for (const list of lists) {
      expect(notices, list).toContain(list);
    }
    const flags = [...script.matchAll(/--(enable|disable)-(demuxers|decoders|parsers|bsfs|autodetect|everything)\b/g)].map((match) => match[0]);
    for (const flag of new Set(flags)) {
      expect(notices, flag).toContain(flag);
    }
  });

  test("third_party_notices_covers_what_is_linked_into_the_ffmpeg_that_ships", () => {
    // Linux: a static build, with glibc and libgcc in it, and libvpx, libopus and zlib. Windows: the runtime of mingw-w64 and winpthreads.
    for (const name of ["glibc", "libgcc", "libvpx", "libopus", "zlib", "mingw-w64", "winpthreads"]) {
      expect(notices, name).toContain(name);
    }
    expect(notices).toMatch(/GCC Runtime Library Exception/);
    // The headers that the Windows encoders are built against.
    expect(notices).toContain("nv-codec-headers");
    expect(notices).toContain("AMF");
  });

  test("third_party_notices_names_every_other_component_that_ships", () => {
    for (const name of [
      "whisper.cpp",
      "ggml",
      "Whisper",
      "sherpa-onnx",
      "ONNX Runtime",
      "pyannote",
      "TitaNet",
      "Tauri",
      "Svelte",
    ]) {
      expect(notices, name).toContain(name);
    }
    // espeak-ng is GPL and is kept out of the program: the notices say why a reader will not find it.
    expect(notices).toContain("espeak-ng");
    // The method of the list of Rust and npm licenses, and where each non-permissive license is.
    expect(notices).toContain("cargo metadata");
    expect(notices).toContain("pnpm licenses");
  });

  test("the_readmes_point_at_the_notices_and_at_the_ffmpeg_source_of_the_release", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toMatch(/Third-party|bên thứ ba/);
      expect(text).toContain("LGPL");
    }
  });
});

describe("the developer guide", () => {
  test("app_readme_no_longer_says_what_the_installers_made_untrue", () => {
    expect(appReadme).not.toContain("until the installers bundle it");
    expect(appReadme).not.toContain("Only the macOS `.app` has been built and run so far; the installers are untested");
    expect(appReadme).not.toContain("do not do that yet");
    expect(appReadme).not.toContain("Recording has not been tried by hand on any system");
    expect(appReadme).not.toContain("So far the app has only been run on macOS");
    // qsv was dropped: `h264_mf` covers every vendor of a graphics card.
    expect(appReadme).not.toMatch(/Quick Sync/);
    expect(appReadme).not.toMatch(/\.msi|\.rpm|AppImage/);
  });

  test("app_readme_says_where_ffmpeg_comes_from_and_the_order_of_the_encoders_of_windows", () => {
    expect(appReadme).toContain("DICTATE_FFMPEG");
    expect(appReadme).toContain("dictate-ffmpeg");
    expect(appReadme).toMatch(/h264_mf[^.]*hardware[^.]*h264_mf[^.]*software[^.]*h264_nvenc[^.]*h264_amf[^.]*libopenh264[^.]*libx264[^.]*mpeg4/s);
  });

  test("app_readme_says_how_a_release_is_cut", () => {
    expect(appReadme).toMatch(/^## Cut a release$/m);
    expect(appReadme).toContain("Allow GitHub Actions to create and approve pull requests");
    expect(appReadme).toContain("release-please");
    expect(appReadme).toContain("THIRD_PARTY_NOTICES.md");
    expect(appReadme).toContain("tauri.release.conf.json");
    expect(appReadme).toContain("scripts/stage-sidecars.sh");
    expect(appReadme).toContain("/usr/lib/dictate");
    expect(appReadme).toContain("$ORIGIN/../lib/dictate");
  });

  test("the_contributing_guide_has_the_checks_that_ci_runs_and_the_rules_for_commits", () => {
    const contributing = read("CONTRIBUTING.md");
    for (const command of [
      "pnpm tauri dev",
      "cargo fmt --all --check",
      "cargo clippy --all-targets --locked -- -D warnings",
      "cargo test --all --locked",
      "pnpm --dir app/ui check",
      "pnpm --dir app/ui test",
    ]) {
      expect(contributing, command).toContain(command);
    }
    expect(contributing).toContain("Conventional Commits");
    expect(contributing).toContain("release-please");
    expect(contributing).toContain("Co-Authored-By");
    expect(contributing).toContain("en.json");
    expect(contributing).toContain("vi.json");
    expect(contributing).toContain("catalog.test.ts");
  });
});

describe("the policies and the templates of the repository", () => {
  const CONTACT = "henryhoang.tech@gmail.com";

  test("the_conduct_and_the_security_policy_have_the_contact_of_the_maintainer", () => {
    expect(read("CODE_OF_CONDUCT.md")).toContain(CONTACT);
    expect(read("CODE_OF_CONDUCT.md")).toContain("Contributor Covenant");
    expect(read("CODE_OF_CONDUCT.md")).toMatch(/version 2\.1/);
    expect(read("SECURITY.md")).toContain(CONTACT);
    expect(read("SECURITY.md")).toContain("https://github.com/henryonai/dictate/releases/latest");
  });

  test("the_security_policy_says_what_counts_in_the_new_app", () => {
    const security = read("SECURITY.md");
    expect(security).toMatch(/credential store|Keychain|Credential Manager|Secret Service/);
    expect(security).toMatch(/dictate-mcp/);
    expect(security).toMatch(/checksum|sha256/i);
    expect(security).toMatch(/not signed|unsigned|aren't signed|aren't notarized/i);
    // The scheme of the Swift app is gone from this one.
    expect(security).not.toContain("dictate://");
  });

  test("the_bug_report_asks_for_the_system_the_version_and_the_installer", () => {
    const form = read(".github/ISSUE_TEMPLATE/bug_report.yml");
    for (const id of ["what-happened", "steps", "area", "version", "system", "installer", "log", "privacy"]) {
      expect(form, id).toMatch(new RegExp(`^ {4}id: ${id}$`, "m"));
    }
    for (const option of ["macOS (Apple silicon)", "macOS (Intel)", "Windows", ".dmg", ".exe", ".deb", "Built from source"]) {
      expect(form, option).toContain(option);
    }
    expect(read(".github/ISSUE_TEMPLATE/feature_request.yml")).toContain("name: Feature request");
    const config = read(".github/ISSUE_TEMPLATE/config.yml");
    expect(config).toContain("https://github.com/henryonai/dictate/blob/main/SECURITY.md");
  });

  test("the_pull_request_template_lists_the_checks_of_this_stack", () => {
    const template = read(".github/pull_request_template.md");
    for (const text of ["cargo fmt", "cargo clippy", "cargo test", "pnpm --dir app/ui check", "pnpm --dir app/ui test", "en.json", "vi.json", "README.vi.md"]) {
      expect(template, text).toContain(text);
    }
    expect(template).not.toContain("swift");
  });
});
