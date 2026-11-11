import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { BAR_COUNT } from "../lib/levels";
import en from "../locales/en.json";
import vi from "../locales/vi.json";

// The layout can't be seen by a test, but its numbers can be added up: the window of the bar has a size that the Rust side gives it
// and the page can't change, and what the capsule holds in the worst cases must fit in it. The sizes are read from the sources that
// decide them (the CSS of the component, `WIDTH` of `hud.rs`) and the words from the locales, so that a longer text or a wider
// margin fails here and not on a screen. The width of a text is an estimate, made on the safe side.
const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../../..");
const component = readFileSync(join(here, "RecorderApp.svelte"), "utf8");
const hud = readFileSync(join(repo, "app/src-tauri/src/recording/hud.rs"), "utf8");

/** The declarations of the rule of `selector` in the component. */
function rule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`\\n\\s*${escaped} \\{([^}]*)\\}`).exec(component);
  expect(match, `the rule of ${selector}`).not.toBeNull();
  return match?.[1] ?? "";
}

/** A length in pixels of a property of a rule. */
function px(selector: string, property: string): number {
  const match = new RegExp(`(?:^|[\\s;])${property}:\\s*(\\d+(?:\\.\\d+)?)px`).exec(rule(selector));
  expect(match, `${property} of ${selector}`).not.toBeNull();
  return Number(match?.[1]);
}

/** The two horizontal paddings of a rule (`padding: 0 10px 0 16px` is right then left; `padding: 3px 9px` is both). */
function sidePadding(selector: string): { left: number; right: number } {
  const value = /padding:\s*([^;]+);/.exec(rule(selector))?.[1] ?? "";
  const lengths = [...value.matchAll(/(\d+(?:\.\d+)?)(?:px)?/g)].map((match) => Number(match[1]));
  expect(lengths.length, `the padding of ${selector}`).toBeGreaterThan(1);
  if (lengths.length === 4) return { right: lengths[1], left: lengths[3] };
  return { left: lengths[1], right: lengths[1] };
}

/** The width of an average character of a semibold sans-serif, in ems, rounded up. */
const CHARACTER = 0.6;
const text = (words: string, size: number) => [...words].length * size * CHARACTER;
/** A clock of tabular digits and colons ("1:02:05"): a digit is as wide as a character, a colon is half of it. */
const clock = (digits: number, colons: number, size: number) => digits * size * CHARACTER + colons * size * CHARACTER * 0.5;

const window = Number(/const WIDTH: f64 = (\d+(?:\.\d+)?);/.exec(hud)?.[1]);
const room = window - 2 * sidePadding("main").left;

type Locale = Record<string, string>;
type Track = "system" | "mic" | "screen";
type Kind = "silent" | "restarted" | "off";
interface Case {
  /** The tracks that have a meter. */
  meters: ("system" | "mic")[];
  notes: { track: Track; kind: Kind }[];
}

/** What a note says, as the bar writes it. */
function chipWidth(locale: Locale, note: { track: Track; kind: Kind }): number {
  const name = locale[`recording.hud.${note.track}`];
  const words = locale[`recording.hud.note.${note.kind}`].replaceAll("{name}", name);
  // "⚠ " comes before the words (`::before`); a note that is off is a pill with room around it.
  const base = text(`⚠ ${words}`, px(".note", "font-size"));
  return note.kind === "off" ? base + sidePadding(".note.off").left + sidePadding(".note.off").right : base;
}

/**
 * The width of the capsule, from what is in it left to right: the dot, the time, the divider, the notes, the group of the meters,
 * Pause, Stop and Discard, with the gap between each and the padding at both ends. `hours` is a clock of an hour or more.
 */
function capsule(locale: Locale, { meters, notes }: Case, options: { hours: boolean; withMeters?: boolean }): number {
  const bars = BAR_COUNT * px(".bar", "width") + (BAR_COUNT - 1) * px(".bars", "gap");
  // The labels of the meters are not there when there are two of them, or a note says what they would.
  const meter = px(".icon", "width") + px(".meter", "gap") + bars;
  const group = meters.length * meter + Math.max(0, meters.length - 1) * px(".meters", "gap");
  const stop = sidePadding(".stop").left + sidePadding(".stop").right + px(".square", "width") + px(".stop", "gap") + text(locale["recording.hud.stop"], px(".stop", "font-size"));
  const parts = [
    px(".dot", "width"),
    options.hours ? clock(5, 2, px(".time", "font-size")) : clock(4, 1, px(".time", "font-size")),
    px(".divider", "width"),
    ...notes.map((note) => chipWidth(locale, note)),
    ...(meters.length > 0 && options.withMeters !== false ? [group] : []),
    px(".round", "width"),
    stop,
    px(".trash", "width"),
  ];
  const { left, right } = sidePadding(".capsule");
  return left + right + parts.reduce((sum, part) => sum + part, 0) + (parts.length - 1) * px(".capsule", "gap");
}

/** What can be on the bar together, and has to fit whole. */
const FITS: [string, Case][] = [
  ["two meters", { meters: ["system", "mic"], notes: [] }],
  ["one meter", { meters: ["mic"], notes: [] }],
  ["two meters and a microphone that hears nothing", { meters: ["system", "mic"], notes: [{ track: "mic", kind: "silent" }] }],
  ["two meters and a system that hears nothing", { meters: ["system", "mic"], notes: [{ track: "system", kind: "silent" }] }],
  ["two meters and a system that was reconnected", { meters: ["system", "mic"], notes: [{ track: "system", kind: "restarted" }] }],
  ["two meters and a microphone that was reconnected", { meters: ["system", "mic"], notes: [{ track: "mic", kind: "restarted" }] }],
  ["a meter and a system that is off", { meters: ["mic"], notes: [{ track: "system", kind: "off" }] }],
  ["a meter and a microphone that is off", { meters: ["system"], notes: [{ track: "mic", kind: "off" }] }],
];

/** Both tracks have something to say: the notes come first and have to fit whole, and the meters give way (they may be cut). */
const NOTES_WIN: [string, Case][] = [
  ["both tracks reconnected", { meters: ["system", "mic"], notes: [{ track: "system", kind: "restarted" }, { track: "mic", kind: "restarted" }] }],
  ["a system that is off and a microphone that was reconnected", { meters: ["mic"], notes: [{ track: "system", kind: "off" }, { track: "mic", kind: "restarted" }] }],
  ["a system that hears nothing and a microphone that is off", { meters: ["system"], notes: [{ track: "system", kind: "silent" }, { track: "mic", kind: "off" }] }],
  ["both tracks off", { meters: [], notes: [{ track: "system", kind: "off" }, { track: "mic", kind: "off" }] }],
];

describe.each([
  ["English", en as Locale],
  ["Vietnamese", vi as Locale],
])("in %s, with the clock at an hour and more, the worst case that the capsule has to hold", (_language, locale) => {
  test.each(FITS)("%s fits in the window", (_name, shown) => {
    expect(capsule(locale, shown, { hours: true })).toBeLessThanOrEqual(room);
  });

  test.each(NOTES_WIN)("%s: the notes fit whole, and the meters are what gives way", (_name, shown) => {
    expect(capsule(locale, shown, { hours: true, withMeters: false })).toBeLessThanOrEqual(room);
  });
});

test("the sums are made of numbers that were found, and the window is wider than the capsule needs for two bare meters", () => {
  expect(window).toBeGreaterThan(0);
  expect(room).toBeGreaterThan(0);
  expect(capsule(en as Locale, FITS[0][1], { hours: false })).toBeLessThan(room);
});

test("the notes are not clipped before the meters: they keep their size and the meters are the part that shrinks", () => {
  expect(rule(".note")).toMatch(/flex:\s*none/);
  expect(rule(".meters")).toMatch(/flex:\s*0 1 auto/);
  expect(rule(".meters")).toMatch(/min-width:\s*0/);
  expect(rule(".meters")).toMatch(/overflow:\s*hidden/);
  // The capsule itself clips nothing: a note that is wider than the room is seen, not cut.
  expect(rule(".capsule")).not.toMatch(/overflow/);
});

test("the capsule is as wide as its content, at most the window less the margin of the page, as a length and not a share of the window", () => {
  // The window is sized to the capsule: a capsule that was a share of the window would be the size of the window it sized, and would
  // never be seen to grow. The page is as wide as its capsule (`max-content`), and the capsule is held to a length that is the room.
  expect(px(".capsule", "max-width")).toBe(room);
  expect(rule(".capsule")).not.toMatch(/max-width:\s*[\d.]+%/);
  expect(rule(".capsule")).not.toMatch(/(?:^|[\s;])width:\s*[\d.]+%/);
  expect(rule("main")).toMatch(/width:\s*max-content/);
});

test("the room of the window is what the Rust side gives the bar, less the margin of the page", () => {
  expect(hud).toMatch(/const WIDTH: f64 = \d+(\.\d+)?;/);
  expect(sidePadding("main").left).toBe(sidePadding("main").right);
});
