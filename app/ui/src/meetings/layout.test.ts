import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";
import { declarationsOf, parseCss, styleOf } from "../css-rules";

// The layout can't be seen by a test, but its numbers can be added up. The list of recordings is beside the recording only in a window
// of 1100 px or more (`NARROW_WIDTH`); in a narrower one it is closed, and when it is opened it lies over the recording and takes no
// room from it. So the two columns of a recording must fit in two cases: at the narrowest window that the Rust side allows, with the
// list closed, and at the narrowest window that has the list beside the recording, with it open.
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
  const detail = source("Detail.svelte");
  const aside = declarationsOf(parseCss(styleOf(source("MeetingsApp.svelte"))), "aside");
  const split = declarationsOf(parseCss(styleOf(detail)), ".split");
  const columns = /^minmax\((\d+)px,\s*(\d+)fr\)\s+minmax\((\d+)px,\s*(\d+)fr\)$/.exec(split["grid-template-columns"] ?? "");
  expect(columns, "the columns of .split").not.toBeNull();
  return {
    window: number(window, /MIN_WIDTH: f64 = (\d+)/, "MIN_WIDTH"),
    /** The width from which the list is beside the recording (the store folds it below it). */
    narrow: number(source("../lib/meetings-store.svelte.ts"), /export const NARROW_WIDTH = (\d+)/, "NARROW_WIDTH"),
    list: px(aside.width, "the width of the list"),
    border: px(/(\d+px)/.exec(aside["border-right"] ?? "")?.[1], "the border of the list"),
    padding: number(detail, /\.detail \{[^}]*padding:\s*\d+px (\d+)px/, "the side padding of the detail"),
    gap: px(split.gap, "the gap of .split"),
    columns: [Number(columns?.[1]), Number(columns?.[3])],
    /** The shares of the two columns (`480fr`, `420fr`). */
    shares: [Number(columns?.[2]), Number(columns?.[4])],
    detail,
  };
}

/** A classic scroll bar, which takes room from the detail when it has one. */
const SCROLL_BAR = 17;

/** What the detail has for its two columns and the gap between them, in a window of `width` with the list beside the recording or not. */
function room(width: number, listBeside: boolean): number {
  const { list, border, padding } = widths();
  return width - (listBeside ? list + border : 0) - 2 * padding - SCROLL_BAR;
}

test("the two columns of a recording fit at the narrowest window, with the list closed and a scroll bar", () => {
  const { window, gap, columns } = widths();

  expect(columns[0] + columns[1] + gap).toBeLessThanOrEqual(room(window, false));
});

test("the two columns of a recording fit with the list open, at the narrowest window that has it beside the recording, and a scroll bar", () => {
  const { narrow, gap, columns } = widths();

  expect(columns[0] + columns[1] + gap).toBeLessThanOrEqual(room(narrow, true));
});

test("below the narrow width the list lies over the recording and takes none of its room, and the window it folds in is one that can be made", () => {
  const { window, narrow } = widths();
  // A fold that is below the narrowest window is never reached: the list would be beside the recording in every window.
  expect(narrow).toBeGreaterThan(window);

  const rules = parseCss(styleOf(source("MeetingsApp.svelte")));
  const overlay = declarationsOf(rules, "aside.overlay");
  // Out of the flow, over the recording, with a shadow to say it is over it: the `.body` is what it is placed in.
  expect(overlay.position).toBe("absolute");
  expect(overlay["box-shadow"]).toBe("var(--shadow-pop)");
  expect(Number(overlay["z-index"])).toBeGreaterThan(0);
  expect(declarationsOf(rules, ".body").position).toBe("relative");
  // It is as high as the body and at its left, and has a solid ground (no `color-mix`): the recording does not show through it.
  expect(overlay.top).toBe("0");
  expect(overlay.bottom).toBe("0");
  expect(overlay.left).toBe("0");
  expect(declarationsOf(rules, "aside").background).toBe("var(--surface)");
});

test("no breakpoint of the detail is below the narrowest window: it could never be reached", () => {
  const { window, detail } = widths();

  for (const [, limit] of detail.matchAll(/@media \(max-width:\s*(\d+)px\)/g)) {
    expect(Number(limit), "a breakpoint on the width of the window").toBeGreaterThanOrEqual(window);
  }
});

// ---- The slim bar of the player

/** The number of pixels in a `12px`. */
function px(value: string | undefined, what: string): number {
  const match = /^(\d+(?:\.\d+)?)px$/.exec(value ?? "");
  expect(match, what).not.toBeNull();
  return Number(match?.[1]);
}

/** The widest a character of the monospaced faces of `--mono` is, in em (Menlo 0.602, SF Mono and Liberation Mono 0.6, Consolas 0.55). */
const MONO_EM = 0.62;

/** What the slim bar is made of, as the style of the player says. */
function slimBar() {
  const rules = parseCss(styleOf(source("Player.svelte")));
  const bar = declarationsOf(rules, ".bar");
  const padding = /^0 (\d+)px$/.exec(bar.padding ?? "");
  expect(padding, "the side padding of .bar").not.toBeNull();
  // The `@container` that takes the total away when the bar is narrow.
  const narrow = rules.find((rule) => rule.container?.startsWith("bar ") && rule.selectors.includes(".total"));
  expect(narrow, "the container rule that hides .total").toBeDefined();
  expect(narrow?.declarations.display).toBe("none");
  return {
    border: px(/(\d+px)/.exec(bar.border ?? "")?.[1], "the border of .bar"),
    padding: Number(padding?.[1]),
    gap: px(bar.gap, "the gap of .bar"),
    toggle: px(declarationsOf(rules, ".toggle").width, "the width of .toggle"),
    icon: px(declarationsOf(rules, ".icon").width, "the width of .icon"),
    slider: px(declarationsOf(rules, ".position")["min-width"], "the least width of the slider"),
    speed: px(declarationsOf(rules, ".speed").width, "the width of .speed"),
    font: px(declarationsOf(rules, ".time")["font-size"], "the size of .time"),
    /** The bar hides the total when what is inside it is this wide or less. */
    totalHiddenAt: px(/max-width:\s*(\d+px)/.exec(narrow?.container ?? "")?.[1], "the width of the container rule"),
    container: bar.container,
    containerName: /^bar \//.test(bar.container ?? ""),
  };
}

/** The width of the left column (the player's) in a window of `width`, with the list beside the recording or closed, and a scroll bar. */
function leftColumn(width: number, listBeside: boolean): number {
  const { gap, columns, shares } = widths();
  return Math.max(columns[0], Math.floor(((room(width, listBeside) - gap) * shares[0]) / (shares[0] + shares[1])));
}

/** The narrowest that the left column is in a window that the app makes: the list closed at the narrowest window, or open at the narrow width. */
function narrowestColumn(): number {
  const { window, narrow } = widths();
  return Math.min(leftColumn(window, false), leftColumn(narrow, true));
}

/**
 * What the bar needs inside it: the play button, the slider at its least, the time (`5` characters for 12:34, `7` for 1:02:03, and
 * the same again for the total and its " / "), the speed, and the button that shows the video when it is hidden.
 */
function needs(bar: ReturnType<typeof slimBar>, { chars, total, video }: { chars: number; total: boolean; video: boolean }): number {
  const time = Math.ceil((total ? 2 * chars + 3 : chars) * MONO_EM * bar.font);
  const items = [bar.toggle, bar.slider, time, bar.speed, ...(video ? [bar.icon] : [])];
  return items.reduce((sum, item) => sum + item, 0) + (items.length - 1) * bar.gap;
}

test("the slim bar fits the narrowest column in every mode, and its slider keeps at least 80 px", () => {
  const { columns } = widths();
  const bar = slimBar();
  expect(bar.slider).toBeGreaterThanOrEqual(80);
  expect(bar.containerName, "the bar is the container that the rule asks").toBe(true);
  expect(bar.container).toBe("bar / inline-size");

  const narrowest = narrowestColumn();
  expect(narrowest).toBeGreaterThanOrEqual(columns[0]);

  // Every width of the column from the least that the grid gives to a wide one: what is inside the bar is its width less its border and
  // padding, the total is there when the container rule leaves it, and the modes and the longest times all fit.
  for (let column = columns[0]; column <= 1000; column += 1) {
    const inside = column - 2 * bar.border - 2 * bar.padding;
    const total = inside > bar.totalHiddenAt;
    for (const video of [false, true]) {
      for (const chars of [5, 7]) {
        const need = needs(bar, { chars, total, video });
        expect(need, `a ${column} px column, ${video ? "video hidden" : "audio"}, ${chars === 7 ? "1:02:03" : "12:34"}, total ${total ? "shown" : "left out"}`).toBeLessThanOrEqual(inside);
      }
    }
  }

  // At the narrowest window the widest bar (the video hidden, the longest time) has room, and the slider has more than its least.
  const inside = narrowest - 2 * bar.border - 2 * bar.padding;
  expect(inside - (needs(bar, { chars: 7, total: false, video: true }) - bar.slider)).toBeGreaterThanOrEqual(80);
  // 397 px is the column of the list open at 1100 px (the list closed at the narrowest window has 473 px): 379 px inside the bar.
  expect(narrowest).toBe(397);
});

test("the slim bar shows the total in every window that the app makes, and leaves it out only in a column that the grid squeezed to its least", () => {
  const bar = slimBar();
  // The wide window: the columns at their shares (480 px, a little more), the bar inside is 462 px or more.
  expect(columnsInside(bar, 480)).toBeGreaterThan(bar.totalHiddenAt);
  // The narrowest column of a window that the app makes has the total too: the rule is the net under the grid's least width.
  expect(columnsInside(bar, narrowestColumn())).toBeGreaterThan(bar.totalHiddenAt);
  expect(columnsInside(bar, widths().columns[0])).toBeLessThanOrEqual(bar.totalHiddenAt);
});

function columnsInside(bar: ReturnType<typeof slimBar>, column: number): number {
  return column - 2 * bar.border - 2 * bar.padding;
}

test("the left column is a container of its size, and the large video leaves the transcript at least 200 px of it", () => {
  const { detail } = widths();
  const left = declarationsOf(parseCss(styleOf(detail)), ".left");
  expect(left["container-type"]).toBe("size");
  const capped = parseCss(styleOf(source("Player.svelte"))).find(
    (rule) => rule.supports === "(height: 1cqh)" && rule.selectors.includes("video.large"),
  );
  const cap = Number(/calc\(100cqh - (\d+)px\)/.exec(capped?.declarations.height ?? "")?.[1]);
  // The columns are at least as high as `.split` is (min-height): the large video is that less the cap, or less.
  const minimum = px(declarationsOf(parseCss(styleOf(detail)), ".split")["min-height"], "the height of .split");
  expect(minimum - cap).toBeGreaterThanOrEqual(200);
  expect(cap).toBeGreaterThanOrEqual(200);
});
