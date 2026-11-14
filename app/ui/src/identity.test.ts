// The name of the app where a person or a tool reads it, held in place: the title of each window's page, the name of the two npm packages,
// the copyright file of the Debian package and the words of the interface. The repository's other files are read as text (as
// release-workflow.test.ts reads them): a Windows checkout may have CRLF line ends, so they are made LF first.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import en from "./locales/en.json";
import vi from "./locales/vi.json";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");
const readJson = (file: string) => JSON.parse(read(file));

describe("the app is called Loah", () => {
  test("the_page_of_each_window_is_titled_loah", () => {
    // Settings, meetings, the dictation panel and the recording bar: the title is what a window list or a screen reader says first.
    for (const page of ["index.html", "meetings.html", "panel.html", "recorder.html"]) {
      expect(read(`app/ui/${page}`), page).toContain("<title>Loah</title>");
    }
  });

  test("the_two_npm_packages_are_loah_ui_and_loah_app_tools", () => {
    expect(readJson("app/ui/package.json").name).toBe("loah-ui");
    const tools = readJson("app/package.json");
    expect(tools.name).toBe("loah-app-tools");
    expect(tools.description).toBe("Tauri CLI for the Loah app. The web UI is the package in ui/.");
  });

  test("the_copyright_file_of_the_deb_names_loah_its_repository_and_its_folders", () => {
    const copyright = read("app/src-tauri/linux/copyright");
    expect(copyright).toMatch(/^Upstream-Name: Loah$/m);
    expect(copyright).toMatch(/^Source: https:\/\/github\.com\/henryonai\/loah$/m);
    expect(copyright).toMatch(/^Files: usr\/lib\/loah\/\*$/m);
    expect(copyright).not.toMatch(/dictate/i);
  });

  test("no_sentence_of_the_interface_names_the_app_dictate_in_either_language", () => {
    // `dictate --toggle` is a command that is gone from the strings too: the program is `loah`. A text that has to explain the rename (the
    // note about an old Dictate that is still installed) is the one thing that may say the old name: its key goes in this list.
    const mayNameDictate = new Set([
      "general.dictate_still_installed",
      "settings.migration_failed",
      "settings.migration_failed_plain",
    ]);
    for (const [name, locale] of [["en", en], ["vi", vi]] as [string, Record<string, string>][]) {
      const naming = Object.entries(locale)
        .filter(([key, text]) => /\bDictate\b|\bdictate --/.test(text) && !mayNameDictate.has(key))
        .map(([key]) => key);
      expect(naming, name).toEqual([]);
    }
  });

  test("the_shortcut_strings_name_the_two_commands_of_the_new_program", () => {
    expect(en["general.hotkey_hint"]).toContain("run: loah --toggle");
    expect(vi["general.hotkey_hint"]).toContain("chạy: loah --toggle");
    expect(en["hotkey.wayland_unavailable"]).toContain("loah --toggle (dictation) and loah --record (recording)");
    expect(vi["hotkey.wayland_unavailable"]).toContain("loah --toggle (đọc chính tả) và loah --record (quay)");
    expect(en["general.record_hotkey_hint"]).toContain("run: loah --record");
    expect(vi["general.record_hotkey_hint"]).toContain("chạy: loah --record");
  });
});
