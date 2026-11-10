import { describe, expect, test } from "vitest";
import { renderInline } from "./markdown";

/** The rendering as a page would build it: what the browser makes of the HTML is what is checked, not the text of it. */
function parse(markdown: string): HTMLElement {
  const box = document.createElement("div");
  box.innerHTML = renderInline(markdown);
  return box;
}

describe("renderInline", () => {
  test("markdown_escapes_html_and_only_links_timestamps", () => {
    const hostile = [
      "<img src=x onerror=alert(1)>",
      "<script>alert(1)</script>",
      '<a href="javascript:alert(1)" onclick="x()">click</a>',
      "[01:05](javascript:x)",
      "[click me](https://example.com)",
      "a & b \"quoted\" 'single'",
      "[01:05] start",
      "[1:02:05] later",
      "**a**",
    ].join("\n");
    const box = parse(hostile);

    // Nothing but the two tags the renderer makes is in the page.
    expect([...box.querySelectorAll("*")].map((element) => element.tagName.toLowerCase()).sort()).toEqual([
      "a",
      "a",
      "strong",
    ]);
    // Every attribute of it is the renderer's own: no event handler, no address but "#".
    for (const element of box.querySelectorAll("*")) {
      expect([...element.attributes].map((attribute) => attribute.name).filter((name) => name.startsWith("on"))).toEqual([]);
    }
    for (const link of box.querySelectorAll("a")) {
      expect(link.getAttribute("href")).toBe("#");
      expect([...link.attributes].map((attribute) => attribute.name).sort()).toEqual(["data-seek", "href"]);
    }
    // The hostile text shows as text.
    expect(box.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(box.textContent).toContain("<script>alert(1)</script>");
    expect(box.textContent).toContain('<a href="javascript:alert(1)" onclick="x()">click</a>');
    expect(box.textContent).toContain("a & b \"quoted\" 'single'");
    // A stamp that opens a Markdown link is no seek link, and the link itself is not one either.
    expect(box.textContent).toContain("[01:05](javascript:x)");
    expect(box.textContent).toContain("[click me](https://example.com)");
    // The stamps that stand alone seek.
    expect([...box.querySelectorAll("a")].map((link) => [link.textContent, link.getAttribute("data-seek")])).toEqual([
      ["[01:05]", "65"],
      ["[1:02:05]", "3725"],
    ]);
    expect(box.querySelector("strong")?.textContent).toBe("a");
    expect(renderInline("**a**")).toBe("<strong>a</strong>");
    expect(renderInline(hostile)).not.toMatch(/<script|<img|href="(?!#")/i);
  });

  test("a quote or an ampersand cannot leave an attribute: the characters are escaped", () => {
    expect(renderInline(`& < > " '`)).toBe("&amp; &lt; &gt; &quot; &#39;");
    // The seconds are a number the renderer computed, whatever the text around the stamp.
    expect(renderInline(`"><b>[00:07]`)).toBe(`&quot;&gt;&lt;b&gt;<a href="#" data-seek="7">[00:07]</a>`);
  });

  test("a stamp seeks to its seconds, with or without hours", () => {
    expect(renderInline("[00:00] [00:59] [10:00]")).toBe(
      '<a href="#" data-seek="0">[00:00]</a> <a href="#" data-seek="59">[00:59]</a> <a href="#" data-seek="600">[10:00]</a>',
    );
    expect(renderInline("[1:02:05]")).toBe('<a href="#" data-seek="3725">[1:02:05]</a>');
    expect(renderInline("[10:00:00]")).toBe('<a href="#" data-seek="36000">[10:00:00]</a>');
  });

  test("a stamp that is followed by an opening parenthesis is not a link; what is not a stamp is not one either", () => {
    expect(renderInline("[01:05](x)")).toBe("[01:05](x)");
    expect(renderInline("[1:5] [aa:bb] [01:05:] [note]")).toBe("[1:5] [aa:bb] [01:05:] [note]");
  });

  test("a stamp inside bold text still seeks", () => {
    expect(renderInline("**[02:00] Budget**")).toBe('<strong><a href="#" data-seek="120">[02:00]</a> Budget</strong>');
  });

  test("headings: # is a bold line, ## and ### come after a line break", () => {
    expect(renderInline("# Title\nbody")).toBe("<strong>Title</strong>\nbody");
    expect(renderInline("text\n## Part\nmore")).toBe("text\n\n<strong>Part</strong>\nmore");
    expect(renderInline("text\n### Sub\nmore")).toBe("text\n\n<strong>Sub</strong>\nmore");
    // A heading at the start does not begin with an empty line.
    expect(renderInline("## Part\nmore")).toBe("<strong>Part</strong>\nmore");
  });

  test("lists: bullets, nested bullets and check boxes", () => {
    expect(renderInline("- one\n  - two\n- [ ] open\n- [x] done\n- [X] done too")).toBe(
      ["•  one", "      ◦  two", "☐ open", "☑ done", "☑ done too"].join("\n"),
    );
  });

  test("a dash that does not begin a list item stays a dash", () => {
    expect(renderInline("a - b\n-c\n---")).toBe("a - b\n-c\n---");
  });

  test("bold: each pair of ** makes one <strong>, on one line", () => {
    expect(renderInline("**An** said **hi**")).toBe("<strong>An</strong> said <strong>hi</strong>");
    expect(renderInline("**open\nline**")).toBe("**open\nline**");
    expect(renderInline("2 ** 3")).toBe("2 ** 3");
  });

  test("bold keeps the rule of a pair: the opening ** is followed by a non-space, the closing one follows a non-space", () => {
    expect(renderInline("**a **b**")).toBe("<strong>a **b</strong>");
    expect(renderInline("** a**")).toBe("** a**");
    expect(renderInline("**a **")).toBe("**a **");
    expect(renderInline("****")).toBe("****");
    expect(renderInline("**a** **b**")).toBe("<strong>a</strong> <strong>b</strong>");
    expect(renderInline("***a**")).toBe("<strong>*a</strong>");
    expect(renderInline("**a**b**")).toBe("<strong>a</strong>b**");
  });

  test("bold in a heading and in a list item", () => {
    // The heading begins with its line break, which a text that starts with it does not keep.
    expect(renderInline("## **Budget** talk")).toBe("<strong><strong>Budget</strong> talk</strong>");
    expect(renderInline("a\n## **Budget** talk")).toBe("a\n\n<strong><strong>Budget</strong> talk</strong>");
    expect(renderInline("- [ ] **An** — send it")).toBe("☐ <strong>An</strong> — send it");
    expect(renderInline("  - **x**")).toBe("      ◦  <strong>x</strong>");
  });

  test("a long text with many ** that never close renders in linear time", () => {
    // 128 KB: each ** opens and none closes, which a pattern that looks for the closing one from each opener takes minutes over.
    const unclosed = "**a ".repeat(32768);
    const started = performance.now();

    const html = renderInline(unclosed);

    expect(performance.now() - started).toBeLessThan(1000);
    expect(html).toBe(unclosed);
  });

  test("long texts of other shapes render in linear time too", () => {
    const shapes = [
      "**".repeat(65536),
      "**a**".repeat(26000),
      "a ** ".repeat(26000),
      "[1:".repeat(43000),
      "[11111111".repeat(14000),
      "- [ ] **a ".repeat(13000),
      `${" ".repeat(131072)}x`,
      "# " + "**a ".repeat(32768),
    ];
    for (const shape of shapes) {
      const started = performance.now();
      renderInline(shape);
      expect(performance.now() - started, shape.slice(0, 12)).toBeLessThan(1000);
    }
  });

  test("three newlines or more collapse to two", () => {
    expect(renderInline("a\n\n\n\nb")).toBe("a\n\nb");
  });

  test("an empty text renders as nothing", () => {
    expect(renderInline("")).toBe("");
  });
});
