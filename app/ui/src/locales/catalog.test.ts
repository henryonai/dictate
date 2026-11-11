import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { CARD_MORE_STYLES, CARD_STYLES, CHIP_STYLES } from "../panel/state";
import en from "./en.json";
import vi from "./vi.json";

/** The part of a Rust source file from `start` to the `];` that ends the list. */
function listIn(file: string, start: string): string {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../../..");
  const source = readFileSync(join(repoRoot, "crates/dictate-services/src", file), "utf8");
  const from = source.indexOf(start);
  expect(from, `${start} in ${file}`).toBeGreaterThan(-1);
  return source.slice(from, source.indexOf("];", from));
}

function idsIn(file: string, start: string): string[] {
  return [...listIn(file, start).matchAll(/^\s+id: "([^"]+)",$/gm)].map((match) => match[1]);
}

describe("what the Rust side lists, the UI has words for", () => {
  test("every Whisper model of the catalog has a title and a note in both locales", () => {
    const ids = idsIn("whisper/models.rs", "pub const CATALOG");
    expect(ids.length).toBeGreaterThan(0);
    for (const id of ids) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[`models.title.${id}`], `models.title.${id}`).toBeTruthy();
        expect(locale[`models.note.${id}`], `models.note.${id}`).toBeTruthy();
      }
    }
  });

  test("the chip menu, and the card's buttons with More, offer each of the edit presets once", () => {
    const ids = idsIn("presets.rs", "pub static EDIT_PRESETS");
    expect([...CHIP_STYLES].sort()).toEqual([...ids].sort());
    expect([...CARD_STYLES, ...CARD_MORE_STYLES].sort()).toEqual([...ids].sort());
  });

  test("every edit preset has a title in both locales", () => {
    const ids = idsIn("presets.rs", "pub static EDIT_PRESETS");
    expect(ids).toHaveLength(8);
    for (const id of ids) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[`presets.${id}`], `presets.${id}`).toBeTruthy();
      }
    }
  });
});

describe("what the controller says, the UI has words for", () => {
  /** The keys of the texts that the controller puts in `PanelError`: they are strings of the Rust source. */
  function errorKeys(): string[] {
    const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../../..");
    const source = readFileSync(join(repoRoot, "app/src-tauri/src/dictation/controller.rs"), "utf8");
    // Only the part before the tests: their fixtures name keys too, and those are not the controller's.
    const code = source.slice(0, source.indexOf("#[cfg(test)]"));
    return [...new Set([...code.matchAll(/"((?:dictation|audio)\.[a-z_]+)"/g)].map((match) => match[1]))];
  }

  test("every error key of the controller has a text in both locales", () => {
    const keys = errorKeys();
    expect(keys.length).toBeGreaterThan(8);
    for (const key of keys) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[key], key).toBeTruthy();
      }
    }
  });

  test("the button of an error that has one has its words too", () => {
    for (const locale of [en, vi] as Record<string, string>[]) {
      expect(locale["dictation.open_settings"]).toBeTruthy();
    }
  });
});

describe("what the app no longer has, the locales do not carry", () => {
  test("no text of the WAV file test of the Speech page is left", () => {
    for (const locale of [en, vi] as Record<string, string>[]) {
      expect(Object.keys(locale).filter((key) => key.startsWith("wav."))).toEqual([]);
    }
  });
});

describe("what the meetings window asks for, the locales have", () => {
  const uiSrc = join(dirname(fileURLToPath(import.meta.url)), "..");

  /** The keys of the texts that the sources of the window name: they are strings, some in tables, that `t` is given later. */
  function meetingsKeys(): string[] {
    const files = [
      ...readdirSync(join(uiSrc, "meetings"))
        .filter((name) => name.endsWith(".svelte"))
        .map((name) => join("meetings", name)),
      "lib/meetings-store.svelte.ts",
      "lib/meeting-errors.ts",
      "lib/days.ts",
    ];
    const keys = files.flatMap((file) => [
      ...readFileSync(join(uiSrc, file), "utf8").matchAll(/"(meetings\.[a-z_]+(?:\.[a-z_]+)*)"/g),
    ]);
    return [...new Set(keys.map((match) => match[1]))];
  }

  test("every text of the screens, the store and the formats has words in both locales", () => {
    const keys = meetingsKeys();
    expect(keys.length).toBeGreaterThan(40);
    for (const key of keys) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[key], key).toBeTruthy();
      }
    }
  });
});

describe("what the recording asks for, the locales have", () => {
  const uiSrc = join(dirname(fileURLToPath(import.meta.url)), "..");
  const repoRoot = join(uiSrc, "../../..");

  const keysIn = (source: string, pattern: RegExp) => [...new Set([...source.matchAll(pattern)].map((match) => match[1]))];

  test("every text of the picker and of the bar has words in both locales", () => {
    const files = [
      ...readdirSync(join(uiSrc, "meetings")).filter((name) => name.endsWith(".svelte")).map((name) => join("meetings", name)),
      ...readdirSync(join(uiSrc, "recorder")).filter((name) => name.endsWith(".svelte")).map((name) => join("recorder", name)),
    ];
    const keys = files.flatMap((file) =>
      keysIn(readFileSync(join(uiSrc, file), "utf8"), /"(recording\.[a-z_]+(?:\.[a-z_]+)*)"/g),
    );
    expect(new Set(keys).size).toBeGreaterThan(30);
    for (const key of new Set(keys)) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[key], key).toBeTruthy();
      }
    }
  });

  test("every text that the recorder sends the windows by its key (why a start failed, why a recording ended) has words in both locales", () => {
    const keys = ["views.rs", "mod.rs"].flatMap((file) => {
      const source = readFileSync(join(repoRoot, "app/src-tauri/src/recording", file), "utf8");
      // `views.rs` has its tests in the file, after `mod tests`: their fixtures name keys too, and those are not the recorder's (the
      // tests of `mod.rs` are in a file of their own).
      const code = source.slice(0, source.indexOf("#[cfg(test)]\nmod tests {") >= 0 ? source.indexOf("#[cfg(test)]\nmod tests {") : undefined);
      return keysIn(code, /"(recording\.(?:error|ended)\.[a-z_]+)"/g);
    });
    expect(keys.length).toBeGreaterThanOrEqual(15);
    for (const key of keys) {
      for (const locale of [en, vi] as Record<string, string>[]) {
        expect(locale[key], key).toBeTruthy();
      }
    }
  });

  test("the bar's tooltip for a track is made from the words of its name and of what it hears", () => {
    for (const locale of [en, vi] as Record<string, string>[]) {
      expect(locale["recording.hud.hearing"]).toContain("{name}");
      expect(locale["recording.hud.quiet"]).toContain("{name}");
    }
  });
});
