import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
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
