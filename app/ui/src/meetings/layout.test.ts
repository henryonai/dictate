import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

// The layout can't be seen by a test, but its numbers can be added up: at the narrowest window that the Rust side allows, with the
// list open, the two columns of a recording must fit in what is left.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");

const source = (path: string) => readFileSync(join(here, path), "utf8");
const number = (text: string, pattern: RegExp, what: string): number => {
  const match = pattern.exec(text);
  expect(match, what).not.toBeNull();
  return Number(match?.[1]);
};

/** The widths that the layout adds up, from the sources that decide them. */
function widths() {
  const window = readFileSync(join(repo, "app/src-tauri/src/meetings/window.rs"), "utf8");
  const app = source("MeetingsApp.svelte");
  const detail = source("Detail.svelte");
  const aside = /aside \{[^}]*\}/.exec(app)?.[0] ?? "";
  const split = /\.split \{[^}]*\}/.exec(detail)?.[0] ?? "";
  const columns = /grid-template-columns:\s*minmax\((\d+)px,[^)]*\)\s*minmax\((\d+)px,[^)]*\)/.exec(split);
  expect(columns, "the columns of .split").not.toBeNull();
  return {
    window: number(window, /MIN_WIDTH: f64 = (\d+)/, "MIN_WIDTH"),
    list: number(aside, /\bwidth:\s*(\d+)px/, "the width of the list"),
    border: number(aside, /border-right:\s*(\d+)px/, "the border of the list"),
    padding: number(detail, /\.detail \{[^}]*padding:\s*\d+px (\d+)px/, "the side padding of the detail"),
    gap: number(split, /\bgap:\s*(\d+)px/, "the gap of .split"),
    columns: [Number(columns?.[1]), Number(columns?.[2])],
    detail,
  };
}

/** A classic scroll bar, which takes room from the detail when it has one. */
const SCROLL_BAR = 17;

test("the two columns of a recording fit at the narrowest window, with the list open and a scroll bar", () => {
  const { window, list, border, padding, gap, columns } = widths();

  const room = window - list - border - 2 * padding - SCROLL_BAR;

  expect(columns[0] + columns[1] + gap).toBeLessThanOrEqual(room);
});

test("no breakpoint of the detail is below the narrowest window: it could never be reached", () => {
  const { window, detail } = widths();

  for (const [, limit] of detail.matchAll(/@media \(max-width:\s*(\d+)px\)/g)) {
    expect(Number(limit), "a breakpoint on the width of the window").toBeGreaterThanOrEqual(window);
  }
});
