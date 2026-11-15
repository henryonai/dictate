// What the documents of the project promise, held in place: the two READMEs say the same things in two languages, every link in
// them goes somewhere, the MCP server is named by its path on every system, the notices of third-party licenses carry what the
// licenses ask for, and the developer guide no longer says what the installers made untrue. The files are read as text (as
// ci-workflow.test.ts reads the CI workflow); a Windows checkout may have CRLF line ends, so they are made LF.
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
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
  "licenses/README.md",
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

/** Every fenced code block, also the ones inside a list item (indented), as their lines without the indent: what `codeBlocks` leaves out. */
function allCode(text: string): string[] {
  return [...text.matchAll(/^([ \t]*)(```|~~~)[^\n]*\n([\s\S]*?)^\1\2[^\n]*$/gm)].map((match) =>
    match[3]
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "")
      .join("\n"),
  );
}

/** The headings of a document, by level. */
const headings = (text: string, level: number) =>
  [...withoutCode(text).matchAll(new RegExp(`^${"#".repeat(level)}\\s+(.+)$`, "gm"))].map((match) => match[1]);

/** The text of the `##` section with this heading, up to the next one (its subsections are in it). */
function sectionOf(text: string, heading: string): string {
  const start = text.search(new RegExp(`^## ${heading.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "m"));
  expect(start, `no section "${heading}"`).toBeGreaterThanOrEqual(0);
  const rest = text.slice(start + 1);
  const next = rest.search(/^## /m);
  return next === -1 ? rest : rest.slice(0, next);
}

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
      expect(text).toContain("/Applications/Loah.app/Contents/MacOS/loah-mcp");
      expect(text).toContain("%LOCALAPPDATA%\\Loah\\loah-mcp.exe");
      expect(text).toContain("/usr/bin/loah-mcp");
      // The command that registers it with Claude Code, under the name `loah`.
      expect(text).toContain("claude mcp add --scope user loah -- /Applications/Loah.app/Contents/MacOS/loah-mcp");
      // The JSON of Claude Desktop, with the backslashes of a Windows path written twice.
      expect(text).toContain('"mcpServers"');
      expect(text).toContain("loah-mcp.exe");
    }
    expect(readme).toMatch(/\\\\Loah\\\\loah-mcp\.exe/);
    expect(readmeVi).toMatch(/\\\\Loah\\\\loah-mcp\.exe/);
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
        "Loah_<version>_aarch64.dmg",
        "Loah_<version>_x64.dmg",
        "Loah_<version>_x64-setup.exe",
        "Loah_<version>_amd64.deb",
      ]) {
        expect(text, name).toContain(name);
        expect(text, `${name}.sha256`).toContain(`${name}.sha256`);
      }
      expect(text).toContain("sudo apt install ./Loah_<version>_amd64.deb");
      expect(text).not.toMatch(/AppImage/);
    }
  });

  test("the_unsigned_builds_say_how_to_open_them_and_how_to_check_them", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("xattr -dr com.apple.quarantine /Applications/Loah.app");
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
      expect(text).toContain("LOAH_FFMPEG");
      expect(text).toContain("launchctl setenv LOAH_FFMPEG");
      // The name that Dictate used is still read, and the READMEs say so.
      expect(text).toContain("DICTATE_FFMPEG");
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

  test("the_readmes_say_that_a_windows_uninstall_leaves_the_models_the_settings_and_the_keys_and_how_to_remove_them", () => {
    // The uninstaller of the bundler removes the program, and its box "Delete the application data" only the folders named after the identifier
    // of the app: what Dictate keeps lives elsewhere. The place of each (the folders of the app and the two entries of the credential store) is
    // what the code uses; the commands are the same in both languages (a test above compares the code blocks).
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("Delete the application data");
      expect(text).toContain("%LOCALAPPDATA%\\Loah");
      expect(text).toContain("%APPDATA%\\Loah");
      expect(text).toContain("io.github.henryonai.loah");
      expect(text).toContain("Credential Manager");
      expect(text).toContain('Remove-Item -Recurse -Force "$env:LOCALAPPDATA\\Loah", "$env:APPDATA\\Loah"');
      expect(text).toContain("cmdkey /delete:default.Loah");
      expect(text).toContain("cmdkey /delete:transcribe.Loah");
      expect(text).toContain("sudo apt remove loah");
    }
    // The names of the credentials are the ones of the code (service `Loah`, accounts `default` and `transcribe`; the service
    // `Dictate` of the old app is still read: LEGACY_SERVICE).
    const secrets = read("crates/loah-services/src/secrets.rs");
    expect(secrets).toContain('pub const SERVICE: &str = "Loah";');
    expect(secrets).toContain('Account::Ai => "default"');
    expect(secrets).toContain('Account::Transcribe => "transcribe"');
  });

  test("the_readmes_say_what_smart_app_control_of_windows_11_does_to_an_unsigned_installer", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain("Smart App Control");
    }
    expect(readme).toMatch(/Smart App Control[^.]*no such button[^.]*blocks an unsigned installer/i);
  });

  test("the_readmes_replace_the_swift_app_and_say_what_has_to_be_done_again", () => {
    expect(readme).toMatch(/Swift/);
    expect(readmeVi).toMatch(/Swift/);
    expect(readme).toMatch(/quit the old one first/i);
  });

  test("the_vietnamese_readme_spells_xoa_one_way", () => {
    expect(readmeVi).not.toMatch(/xoá|Xoá/);
    expect(readmeVi).toMatch(/xóa/);
  });

  test("the_readmes_are_headed_loah_and_point_at_its_repository", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toContain('<h1 align="center">Loah</h1>');
      expect(text).toMatch(/alt="(?:Loah icon|Biểu tượng Loah)"/);
      expect(text).toContain("https://github.com/henryonai/loah/releases/latest");
      expect(text).toContain("https://img.shields.io/github/v/release/henryonai/loah");
    }
  });

  test("readme_has_the_upgrading_from_dictate_section_in_both_languages", () => {
    const TITLE = { en: "Upgrading from Dictate", vi: "Nâng cấp từ Dictate" };
    expect(headings(readme, 2)).toContain(TITLE.en);
    expect(headings(readmeVi, 2)).toContain(TITLE.vi);
    // The same place in both, and the same subsections.
    expect(headings(readmeVi, 2).indexOf(TITLE.vi)).toBe(headings(readme, 2).indexOf(TITLE.en));
    const sections = { en: sectionOf(readme, TITLE.en), vi: sectionOf(readmeVi, TITLE.vi) };
    expect(headings(sections.vi, 3)).toHaveLength(headings(sections.en, 3).length);
    expect(headings(sections.en, 3).length).toBeGreaterThanOrEqual(3);

    // The button of Settings → General opens it, so the anchor has to be there (the English README is the one a repository shows).
    expect(anchors(readme).has("upgrading-from-dictate")).toBe(true);
    const button = /const UPGRADE_URL = "([^"]+)";/.exec(read("app/ui/src/settings/General.svelte"))?.[1] ?? "";
    expect(button).toBe("https://github.com/henryonai/loah#upgrading-from-dictate");
    expect(anchors(readme).has(button.split("#")[1])).toBe(true);
    // In the Vietnamese interface it opens README.vi.md, a page of its own on GitHub, at the anchor of its own heading.
    const buttonVi = /const UPGRADE_URL_VI = "([^"]+)";/.exec(read("app/ui/src/settings/General.svelte"))?.[1] ?? "";
    expect(buttonVi).toBe("https://github.com/henryonai/loah/blob/main/README.vi.md#nâng-cấp-từ-dictate");
    expect(anchors(readmeVi).has(buttonVi.split("#")[1])).toBe(true);
    expect(existsSync(join(repoRoot, "README.vi.md"))).toBe(true);

    for (const [language, text] of Object.entries(sections)) {
      // What is copied, and where it goes: the four things Dictate 2.0.0 wrote, in the folders of Loah.
      for (const item of ["settings.json", "whisper-models", "diarization-models", "Dictations"]) {
        expect(text, `${language}: ${item}`).toContain(item);
      }
      // The library stays where it is, new installs get the new name, and the old variables are still read.
      for (const name of ["Dictate Meetings", "Loah Meetings", "LOAH_MEETINGS_DIR", "DICTATE_MEETINGS_DIR", "LOAH_FFMPEG", "DICTATE_FFMPEG"]) {
        expect(text, `${language}: ${name}`).toContain(name);
      }
      // The keys: copied from Dictate's entry, which stays, so to remove a key for good both entries have to go (a command for each, on
      // Windows); and the Keychain prompt of a Mac.
      const commands = codeBlocks(text).join("\n");
      for (const name of ["cmdkey /delete:default.Loah", "cmdkey /delete:default.Dictate", "cmdkey /delete:transcribe.Loah", "cmdkey /delete:transcribe.Dictate"]) {
        expect(commands, `${language}: ${name}`).toContain(name);
      }
      // The prompt of the Keychain on a Mac: what to choose, and what Deny does.
      for (const name of ["Keychain", "Allow", "Deny"]) {
        expect(text, `${language}: ${name}`).toContain(name);
      }
      // The permissions of a Mac are asked again, as the app has a new identifier.
      for (const name of ["Microphone", "Accessibility", "Screen Recording"]) {
        expect(text, `${language}: ${name}`).toContain(name);
      }
      // The old and the new path of the MCP server on every system, and the one-line command that keeps the name `dictate` (so
      // that the tool names in a prompt stay the same).
      for (const path of [
        "/Applications/Dictate.app/Contents/MacOS/dictate-mcp",
        "%LOCALAPPDATA%\\Dictate\\dictate-mcp.exe",
        "/usr/bin/dictate-mcp",
        "/Applications/Loah.app/Contents/MacOS/loah-mcp",
        "%LOCALAPPDATA%\\Loah\\loah-mcp.exe",
        "/usr/bin/loah-mcp",
      ]) {
        expect(text, `${language}: ${path}`).toContain(path);
      }
      expect(text, language).toContain("claude mcp add --scope user dictate -- /Applications/Loah.app/Contents/MacOS/loah-mcp");
      expect(text, language).toContain('claude mcp add --scope user dictate -- "$env:LOCALAPPDATA\\Loah\\loah-mcp.exe"');
      expect(text, language).toContain("claude mcp add --scope user dictate -- loah-mcp");
      // The old entry is removed without naming a scope (the command takes it from whichever has it: with `--scope user` it fails when the
      // entry is in another one), once before each of the three `add` lines.
      const removes = allCode(text)
        .flatMap((block) => block.split("\n"))
        .filter((line) => line.startsWith("claude mcp remove"));
      expect(removes, language).toEqual(Array(3).fill("claude mcp remove dictate"));
      // Linux: the package replaces the old one, and the two commands keep working.
      expect(text, language).toContain("`/usr/bin/dictate`");
      expect(text, language).toContain("`/usr/bin/dictate-mcp`");
      // The old program is removed at the end, and Settings says so when it is still there.
      expect(text, language).toContain("Installed apps");
    }
    // The notice of Settings shows while Dictate runs or is set to open at login (`dictate_still_present`), not while it is merely installed.
    expect(sections.en).toMatch(/Dictate is running[^.]*login[^.]*Settings → General shows a notice/);
    expect(sections.vi).toMatch(/đang chạy[^.]*khởi động máy[^.]*Cài đặt → Chung hiện một thông báo/);
    for (const text of [sections.en, sections.vi]) {
      expect(text).not.toMatch(/Dictate is still installed|Dictate vẫn còn trên máy/);
    }
    // A `remove` that finds nothing says so, and the next line goes on.
    expect(sections.en).toContain("go on to the next line");
    expect(sections.vi).toContain("cứ chạy tiếp dòng sau");
    // While the copy of the saved dictations has failed, the tray item opens Dictate's folder (the label as the tray has it, in each language).
    const tray = read("app/src-tauri/src/tray.rs");
    expect(tray).toContain('open_dictations: "Open dictations folder"');
    expect(tray).toContain('open_dictations: "Mở thư mục đọc chính tả"');
    expect(sections.en).toContain("**Open dictations folder**");
    expect(sections.vi).toContain("**Mở thư mục đọc chính tả**");
    // The permission guide mark is not copied (a new app has no permission yet, so the guide shows again), and Dictate's own folders are
    // never changed or deleted.
    for (const text of [readme, readmeVi]) {
      expect(text).not.toContain("permission-guide-shown");
    }
    expect(sections.en).toMatch(/never (?:changes?|changed|touch(?:es|ed)?|delet(?:es?|ed))/i);
    // The one exception is the login entry that the Linux package swap leaves: it starts Loah (`/usr/bin/dictate` is Loah's script now), so Loah
    // removes it, while on the other systems Dictate's entry stays until Dictate is removed. Both say so where they speak of the login entry.
    const paragraph = (text: string, needle: string) => text.split("\n\n").find((part) => part.includes(needle)) ?? "";
    for (const [language, needle] of [["en", "Dictate's own login entry"], ["vi", "Mục khởi động của Dictate"]] as const) {
      const about = paragraph(sections[language], needle);
      expect(about, `${language}: the paragraph about the login entry`).not.toBe("");
      expect(about, language).toContain("Linux");
      expect(about, language).toContain("`/usr/bin/dictate`");
    }
    // While an item failed to copy, the copy is tried again at every start: what a person deletes in Loah in the meantime may come back.
    expect(paragraph(sections.en, "tries again at every start"), "en: the retry and what it does to a deletion").toContain("may come back");
    expect(paragraph(sections.vi, "thử lại ở mỗi lần mở"), "vi: the retry and what it does to a deletion").toContain("có thể quay lại");
    // A command is the same in both languages, the ones inside a list item too.
    expect(allCode(sections.vi)).toEqual(allCode(sections.en));
    expect(allCode(readmeVi)).toEqual(allCode(readme));
    // The lead of each README links to its own section.
    expect(targets(readme)).toContain("#upgrading-from-dictate");
    expect(targets(readmeVi)).toContain("#nâng-cấp-từ-dictate");
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
    // The right to replace the library, and how it is used (the sidecar is `loah-ffmpeg`, and the offer names it).
    expect(notices).toContain("LOAH_FFMPEG");
    expect(notices).toContain("DICTATE_FFMPEG");
    expect(notices).toContain("loah-ffmpeg");
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
    expect(notices).toContain("scripts/generate-licenses.py");
    expect(notices).toContain("cargo tree");
    expect(notices).toContain("pnpm licenses");
  });

  test("the_readmes_point_at_the_notices_and_at_the_ffmpeg_source_of_the_release", () => {
    for (const text of [readme, readmeVi]) {
      expect(text).toMatch(/Third-party|bên thứ ba/);
      expect(text).toContain("LGPL");
    }
  });
});

/** The digest that scripts/generate-licenses.py writes in the header of the license texts: the third-party packages of Cargo.lock. */
function lockDigest(): string {
  const lines = read("Cargo.lock")
    .split("[[package]]")
    .slice(1)
    .flatMap((block) => {
      const fields = Object.fromEntries([...block.matchAll(/^(\w+) = "([^"]*)"$/gm)].map((match) => [match[1], match[2]]));
      return fields.source ? [`${fields.name} ${fields.version} ${fields.checksum ?? ""}`] : [];
    })
    .sort();
  return createHash("sha256").update(lines.join("\n")).digest("hex");
}

describe("the license texts that every installer carries", () => {
  const licenses = read("THIRD_PARTY_LICENSES.txt");
  const native = "licenses/native";

  test("the_license_texts_are_made_from_this_cargo_lock", () => {
    // A dependency that is added, removed or updated changes the digest: run `python3 scripts/generate-licenses.py` to write the file again.
    const written = /^Cargo\.lock \(third-party packages\): sha256:([0-9a-f]{64})$/m.exec(licenses)?.[1];
    expect(written, "the digest in THIRD_PARTY_LICENSES.txt").toBeDefined();
    expect(written).toBe(lockDigest());
  });

  test("the_license_texts_have_the_native_components_the_rust_crates_and_the_npm_packages", () => {
    for (const name of [
      "libvpx",
      "libopus",
      "zlib",
      "libgcc",
      "GCC Runtime Library Exception",
      "mingw-w64",
      "winpthreads",
      "nv-codec-headers",
      "AMF",
      "whisper.cpp",
      "OpenAI Whisper",
      "pyannote segmentation",
      "sherpa-onnx",
      "ONNX Runtime",
      "THIRD PARTY SOFTWARE NOTICES AND INFORMATION", // the notices of ONNX Runtime
    ]) {
      expect(licenses, name).toContain(name);
    }
    // The Rust crates, among them the ones that a tool that reads the manifests of the workspace alone leaves out (the keyring backend of
    // macOS comes in through a feature of a target), and the one that is MPL-2.0.
    for (const crate of ["tauri 2.", "keyring 3.", "security-framework 3.", "whisper-rs 0.15", "symphonia-core 0.6", "windows-sys 0."]) {
      expect(licenses, crate).toContain(`  ${crate}`);
    }
    expect(licenses).toContain("Mozilla Public License Version 2.0");
    expect(licenses).toContain("Apache License");
    // The npm packages that the interface bundles.
    for (const name of ["svelte 5.", "clsx 2.", "esm-env 1.", "@tauri-apps/api 2.", "@tauri-apps/plugin-opener 2."]) {
      expect(licenses, name).toContain(`  ${name}`);
    }
    // Every text comes with the crates that use it.
    expect(licenses).toMatch(/^Used by:$/m);
  });

  test("the_native_texts_are_the_files_of_licenses_native_and_each_says_where_it_came_from", () => {
    const files = readdirSync(join(repoRoot, native)).filter((name) => !name.startsWith("."));
    expect(files.length).toBeGreaterThanOrEqual(18);
    const sources = read("licenses/README.md");
    for (const name of files) {
      expect(read(`${native}/${name}`).trim().length, name).toBeGreaterThan(200);
      expect(sources, name).toContain(name);
      // The file is in the licenses of the installers, in full.
      expect(licenses.replace(/\s+/g, " "), name).toContain(read(`${native}/${name}`).trim().replace(/\s+/g, " "));
    }
  });

  test("copying_lgpl_is_the_real_text_of_the_lgpl_2_1", () => {
    const lgpl = read("COPYING.LGPLv2.1");
    expect(lgpl).toMatch(/GNU LESSER GENERAL PUBLIC LICENSE\s+Version 2\.1, February 1999/);
    expect(lgpl.length).toBeGreaterThan(24000);
    expect(lgpl).toContain("END OF TERMS AND CONDITIONS");
    // The same file as the one in the source of the FFmpeg that is shipped (ffmpeg-9.0.2.tar.xz has this text as COPYING.LGPLv2.1): one
    // changed character is another file (the text is read with LF line ends, as the tarball has it).
    expect(createHash("sha256").update(lgpl).digest("hex")).toBe("246041b6ecf9bc32d718a62c57877c78b5eb397b6467e74ed7ae2626ab189c30");
  });

  test("the_notices_carry_the_attribution_and_the_copyright_lines_of_the_models_and_the_libraries", () => {
    // CC BY 4.0: who, which license (with its link), where from, and what was changed.
    expect(notices).toContain("TitaNet-small");
    expect(notices).toContain("NVIDIA");
    expect(notices).toContain("Creative Commons Attribution 4.0 International");
    expect(notices).toContain("https://creativecommons.org/licenses/by/4.0/");
    expect(notices).toContain("converted to the ONNX format");
    // The copyright line of each MIT component is the one of its license file, word for word.
    for (const [file, name] of [
      ["whisper.cpp-LICENSE.txt", "whisper.cpp"],
      ["openai-whisper-LICENSE.txt", "OpenAI Whisper"],
      ["pyannote-segmentation-3.0-LICENSE.txt", "pyannote"],
      ["onnxruntime-LICENSE.txt", "ONNX Runtime"],
    ]) {
      const line = /^Copyright \(c\) .*$/m.exec(read(`${native}/${file}`))?.[0];
      expect(line, file).toBeDefined();
      expect(notices, `${name}: ${line}`).toContain(line!);
    }
    // No NOTICE file exists for sherpa-onnx and ONNX Runtime: the notices say so, and the third-party notices of ONNX Runtime are in the texts.
    expect(notices).toMatch(/no `NOTICE` file/);
    // The exact configure line of the Windows build has the headers of the encoders in it.
    expect(notices).toContain("--extra-cflags=-I<build folder>/deps/include");
    // And the libvpx, libopus and zlib copyright lines of the texts that are shipped.
    expect(notices).toContain("Copyright (c) 2010, The WebM Project authors");
    expect(notices).toContain("Copyright 2001-2011 Xiph.Org, Skype Limited, Octasic, Jean-Marc Valin, Timothy B. Terriberry, CSIRO, Gregory Maxwell, Mark Borgerding, Erik de Castro Lopo");
    expect(notices).toContain("(C) 1995-2022 Jean-loup Gailly and Mark Adler");
  });

  test("the_readmes_and_the_notices_say_where_the_license_files_are_on_every_system", () => {
    for (const text of [readme, readmeVi, notices]) {
      expect(text).toContain("Loah.app/Contents/Resources/Licenses");
      expect(text).toContain("Licenses");
      expect(text).toContain("/usr/share/doc/loah");
      expect(text).toContain("COPYING.LGPLv2.1");
      expect(text).toContain("THIRD_PARTY_LICENSES.txt");
    }
    for (const text of [readme, readmeVi]) {
      expect(text).toMatch(/loah\.exe|%LOCALAPPDATA%\\Loah\\Licenses/);
    }
  });

  test("the_maintainer_is_told_how_to_make_the_texts_again_when_a_dependency_changes", () => {
    for (const text of [appReadme, read("CONTRIBUTING.md")]) {
      expect(text).toContain("python3 scripts/generate-licenses.py");
    }
    expect(appReadme).toContain("tauri.release.macos.conf.json");
    expect(appReadme).toContain("app/src-tauri/linux/copyright");
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
    expect(appReadme).toContain("LOAH_FFMPEG");
    expect(appReadme).toContain("DICTATE_FFMPEG");
    expect(appReadme).toContain("loah-ffmpeg");
    expect(appReadme).toMatch(/h264_mf[^.]*hardware[^.]*h264_mf[^.]*software[^.]*h264_nvenc[^.]*h264_amf[^.]*libopenh264[^.]*libx264[^.]*mpeg4/s);
  });

  test("app_readme_says_how_a_release_is_cut", () => {
    expect(appReadme).toMatch(/^## Cut a release$/m);
    expect(appReadme).toContain("Allow GitHub Actions to create and approve pull requests");
    expect(appReadme).toContain("release-please");
    expect(appReadme).toContain("THIRD_PARTY_NOTICES.md");
    expect(appReadme).toContain("tauri.release.conf.json");
    expect(appReadme).toContain("scripts/stage-sidecars.sh");
    expect(appReadme).toContain("/usr/lib/loah");
    expect(appReadme).toContain("$ORIGIN/../lib/loah");
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
    expect(read("SECURITY.md")).toContain("https://github.com/henryonai/loah/releases/latest");
  });

  test("the_security_policy_says_what_counts_in_the_new_app", () => {
    const security = read("SECURITY.md");
    expect(security).toMatch(/credential store|Keychain|Credential Manager|Secret Service/);
    expect(security).toMatch(/loah-mcp/);
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
    expect(config).toContain("https://github.com/henryonai/loah/blob/main/SECURITY.md");
  });

  test.each([
    ...DOCUMENTS,
    ".github/ISSUE_TEMPLATE/bug_report.yml",
    ".github/ISSUE_TEMPLATE/config.yml",
    ".github/ISSUE_TEMPLATE/feature_request.yml",
  ])("%s does not point at the old repository, henryonai/dictate", (file) => {
    expect(read(file)).not.toContain("henryonai/dictate");
  });

  test("the_notices_the_developer_guide_and_the_policies_name_the_programs_and_the_crates_of_loah", () => {
    // What ships is `loah`, `loah-mcp` and `loah-ffmpeg`, from the crates `loah-app`, `loah-mcp` and the two that they share: the written
    // offer of the FFmpeg source has to name the program that a person finds in an installer.
    const old = /dictate-(?:ffmpeg|mcp|app|core|services)|dictate\.exe|Dictate\.app|\/usr\/(?:lib|share\/doc)\/dictate|\bdictate --(?:toggle|record)/;
    // The lines about the two wrappers of the `.deb` name the old commands on purpose (app/README.md, Installers).
    const withoutTheWrappers = (text: string) => text.split("\n").filter((line) => !line.includes("`/usr/bin/dictate`")).join("\n");
    for (const [name, text] of [
      ["THIRD_PARTY_NOTICES.md", notices],
      ["app/README.md", appReadme],
      ["CONTRIBUTING.md", read("CONTRIBUTING.md")],
      ["SECURITY.md", read("SECURITY.md")],
    ]) {
      expect(withoutTheWrappers(text), name).not.toMatch(old);
    }
    // The Linux package keeps Dictate's commands working for the 2.x series, and says how (the developer guide has the mechanism).
    const wrappers = appReadme.split("\n").filter((line) => line.includes("`/usr/bin/dictate`"));
    expect(wrappers, "one line of app/README.md about the wrappers").toHaveLength(1);
    for (const word of ["`/usr/bin/dictate-mcp`", "Replaces", "Conflicts", "Provides", "later major release", "app/src-tauri/linux/dictate"]) {
      expect(wrappers[0], word).toContain(word);
    }
    expect(notices).toContain("`loah-ffmpeg`");
    expect(notices).toContain("`loah-app`");
    expect(notices).toContain("`loah-mcp`");
    expect(notices).toContain("`Loah.app/Contents/MacOS/loah-ffmpeg`");
    expect(notices).toContain("`/usr/bin/loah-ffmpeg`");
    expect(read("SECURITY.md")).toContain("loah --toggle");
    expect(appReadme).toContain("loah --toggle");
  });

  test("the_pull_request_template_lists_the_checks_of_this_stack", () => {
    const template = read(".github/pull_request_template.md");
    for (const text of ["cargo fmt", "cargo clippy", "cargo test", "pnpm --dir app/ui check", "pnpm --dir app/ui test", "en.json", "vi.json", "README.vi.md"]) {
      expect(template, text).toContain(text);
    }
    expect(template).not.toContain("swift");
  });
});
