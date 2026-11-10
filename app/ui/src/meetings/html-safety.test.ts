import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

// What a summary, an answer or a transcript says is never HTML. The one place that gives HTML to the page is `Inline.svelte`, with
// the output of `renderInline` (which escapes the text first and makes only `<strong>` and seek links). This test is there so that a
// second place can't be added without someone seeing it.
const src = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The sources of the window (tests apart), with their paths from `src`. */
function sources(dir = src): { path: string; text: string }[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    if (!/\.(svelte|ts)$/.test(entry.name) || /\.test\.ts$/.test(entry.name)) return [];
    return [{ path: relative(src, path), text: readFileSync(path, "utf8") }];
  });
}

test("no_raw_html: {@html} is in Inline.svelte only, once, and is given what renderInline made", () => {
  // The tag is `{@html ` and what it is given; the sources that only speak of it (as a comment) don't count.
  const using = sources().filter(({ path, text }) => path.endsWith(".svelte") && /\{@html\s/.test(text));

  expect(using.map(({ path }) => path)).toEqual(["meetings/Inline.svelte"]);
  const [inline] = using;
  expect(inline.text.match(/\{@html\s/g)).toHaveLength(1);
  expect(inline.text).toContain("{@html html}");
  expect(inline.text).toMatch(/const html = \$derived\(renderInline\(text\)\)/);
});

test("no_raw_html: nothing writes HTML to the page by other means", () => {
  const writers = sources().filter(({ text }) => /\b(innerHTML|outerHTML|insertAdjacentHTML|document\.write)\b/.test(text));

  expect(writers.map(({ path }) => path)).toEqual([]);
});
