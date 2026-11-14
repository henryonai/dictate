// The Graphite theme, held in place: the palette of the spec in light and in dark (by the system's choice and forced by
// `:root[data-theme]`, which the Theme setting of Task 7 sets), the bundled font and its license, the three weights of a button, the
// reading rules of the summary, and that no component of the settings and meetings windows has a color of its own.
//
// A test cannot see a color: jsdom loads no style and has no `var()`. So the sources are read (css-rules.ts reads the CSS), as
// layout.test.ts and docs.test.ts read theirs.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { declarationsOf, parseCss, styleOf } from "./css-rules";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "../../..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");
const component = (file: string) => parseCss(styleOf(read(`app/ui/src/${file}`)));

const tokensCss = read("app/ui/src/tokens.css");
const appCss = read("app/ui/src/app.css");
const tokens = parseCss(tokensCss);
const app = parseCss(appCss);

// ---- The palette

/** The table of "The Graphite theme" in docs/specs/2026-11-14-loah-redesign-design.md, token by token. */
const SPEC = {
  light: {
    "--bg": "#f5f5f7",
    "--panel": "#eeeef1",
    "--line": "#d9d9de",
    "--ink": "#1d1d1f",
    "--muted": "#6e6e73",
    "--accent": "#0a64d6",
    "--on-accent": "#ffffff",
    "--selected": "#e3eefc",
    "--hl": "#eaf2fd",
    "--ts": "#0a64d6",
  },
  dark: {
    "--bg": "#1c1c1f",
    "--panel": "#212124",
    "--line": "#36363c",
    "--ink": "#ececef",
    "--muted": "#9a9aa3",
    "--accent": "#4c9aff",
    "--on-accent": "#0b1626",
    "--selected": "#232f42",
    // "Accent at 13 %": the accent of the dark theme with an alpha of 0.13 (checked apart, below).
    "--hl": "rgb(76 154 255 / 0.13)",
    "--ts": "#8cbcff",
  },
} as const;

/**
 * The spec lets the builder tune a value for contrast ("WCAG AA for text"). The light `muted` of the table is 4.38:1 on the panel and
 * 4.32:1 on the selected row, under the 4.5:1 that the text of the sidebar needs (the subtitles are 12 px); this one is 4.65 and 4.59.
 */
const TUNED = { light: { "--muted": "#6a6a6f" }, dark: {} } as Record<"light" | "dark", Record<string, string>>;
const expected = (theme: "light" | "dark", token: string): string =>
  TUNED[theme][token] ?? (SPEC[theme] as Record<string, string>)[token];

/**
 * The tokens that the table of the spec does not have, for what the floating windows draw (the dictation panels and the recording bar):
 * `warn` is the amber of a warning mark and text on a plain surface (the `warn-bg` and `warn-fg` pair is for a banner), and `raised` is the
 * surface of a menu that opens over a panel (white in light; a step lighter than the panel in dark, where raised things are lighter).
 */
const FLOATING = {
  light: { "--warn": "#9c5500", "--raised": "#ffffff" },
  dark: { "--warn": "#ff9f0a", "--raised": "#2a2a2e" },
} as const;

/** The four places that the tokens of a theme are written: where each one is, for the same values to be in every one. */
const contexts = {
  light: () => declarationsOf(tokens, ":root"),
  "dark, when the system is dark": () => declarationsOf(tokens, ':root:not([data-theme="light"])', "(prefers-color-scheme: dark)"),
  "forced light": () => declarationsOf(tokens, ':root[data-theme="light"]'),
  "forced dark": () => declarationsOf(tokens, ':root[data-theme="dark"]'),
};
const themeOf = (context: string): "light" | "dark" => (context.includes("dark") ? "dark" : "light");

describe("the Graphite palette", () => {
  test.each(Object.keys(contexts))("every token of the spec has its value in the %s block", (context) => {
    const declared = contexts[context as keyof typeof contexts]();
    const theme = themeOf(context);
    for (const token of Object.keys(SPEC[theme])) {
      expect(declared[token], `${token} in ${context}`).toBe(expected(theme, token));
    }
  });

  test.each(Object.keys(contexts))("the tokens of the floating windows have their values in the %s block", (context) => {
    const declared = contexts[context as keyof typeof contexts]();
    const theme = themeOf(context);
    for (const [token, value] of Object.entries(FLOATING[theme])) {
      expect(declared[token], `${token} in ${context}`).toBe(value);
    }
  });

  test("the_tints_under_the_pointer_are_the_ink_at_a_share_so_they_follow_every_theme", () => {
    // Written once, with the tokens that do not change by name: `ink` is the one that changes, and a tint of it is light on dark and dark on light.
    const root = contexts.light();
    expect(root["--wash"]).toBe("color-mix(in srgb, var(--ink) 8%, transparent)");
    expect(root["--hover"]).toBe("color-mix(in srgb, var(--ink) 14%, transparent)");
    for (const context of ["dark, when the system is dark", "forced dark"] as const) {
      expect(contexts[context](), context).not.toHaveProperty("--wash");
      expect(contexts[context](), context).not.toHaveProperty("--hover");
    }
  });

  test("the_tint_of_the_current_paragraph_in_dark_is_the_accent_at_13_percent", () => {
    for (const context of ["dark, when the system is dark", "forced dark"] as const) {
      const declared = contexts[context]();
      const accent = parseColor(declared["--accent"]);
      const tint = parseColor(declared["--hl"]);
      expect(tint.slice(0, 3), context).toEqual(accent.slice(0, 3));
      expect(tint[3], context).toBeCloseTo(0.13, 5);
    }
  });

  test("a_dark_system_does_not_overrule_a_theme_that_was_forced_to_light", () => {
    // The Theme setting says Light while the system is dark: only a rule that leaves out `data-theme="light"` is for a dark system.
    const dark = tokens.filter((rule) => rule.media === "(prefers-color-scheme: dark)");
    expect(dark.length).toBeGreaterThan(0);
    for (const rule of dark) {
      expect(rule.selectors, "the dark rule").toEqual([':root:not([data-theme="light"])']);
    }
  });

  test("a_forced_theme_is_a_rule_of_its_own_and_not_inside_a_media_query", () => {
    for (const rule of tokens.filter((rule) => rule.selectors.some((selector) => /\[data-theme=/.test(selector) && !selector.includes(":not(")))) {
      expect(rule.media, rule.selectors.join(", ")).toBeNull();
    }
  });

  test("the_color_scheme_of_the_page_follows_the_theme_so_scroll_bars_and_fields_do_too", () => {
    expect(contexts.light()["color-scheme"]).toBe("light dark");
    expect(contexts["dark, when the system is dark"]()["color-scheme"]).toBe("dark");
    expect(contexts["forced light"]()["color-scheme"]).toBe("light");
    expect(contexts["forced dark"]()["color-scheme"]).toBe("dark");
  });

  test("the_old_token_names_are_aliases_of_the_new_ones_so_they_follow_every_theme", () => {
    const root = contexts.light();
    expect(root["--surface"]).toBe("var(--panel)");
    expect(root["--border"]).toBe("var(--line)");
    expect(root["--fg"]).toBe("var(--ink)");
    expect(root["--accent-soft"]).toBe("var(--selected)");
    expect(root["--accent-fg"]).toBe("var(--on-accent)");
    // An alias that a theme wrote again as a color would stay behind when the theme is forced.
    for (const context of ["dark, when the system is dark", "forced dark"] as const) {
      for (const alias of ["--surface", "--border", "--fg", "--accent-soft", "--accent-fg"]) {
        expect(contexts[context](), `${alias} in ${context}`).not.toHaveProperty(alias);
      }
    }
  });

  test("the_tokens_that_do_not_change_with_the_theme_are_there", () => {
    const root = declarationsOf(tokens, ":root");
    expect(root["--record"]).toBe("#d92d20");
    expect(root["--on-record"]).toBe("#ffffff");
    expect(root["--mono"]).toMatch(/^ui-monospace, /);
    expect(root["--mono"]).toMatch(/monospace$/);
    for (const token of ["--scrim", "--shadow-pop", "--shadow-dialog", "--video-bg"]) {
      expect(root[token], token).toBeDefined();
    }
  });

  test("the_text_colors_reach_WCAG_AA_on_what_they_are_drawn_on_in_both_themes", () => {
    const record = contexts.light()["--record"];
    const onRecord = contexts.light()["--on-record"];
    for (const context of Object.keys(contexts)) {
      const t = contexts[context as keyof typeof contexts]();
      const bg = parseColor(t["--bg"]);
      const over = (token: string) => {
        const [r, g, b, a] = parseColor(t[token]);
        return [0, 1, 2].map((i) => [r, g, b][i] * a + bg[i] * (1 - a));
      };
      const color = (token: string) => (token === "--hl" ? over("--hl") : parseColor(t[token]).slice(0, 3));
      const pairs: [string, string][] = [
        ["--ink", "--bg"], ["--ink", "--panel"], ["--ink", "--selected"], ["--ink", "--hl"],
        ["--muted", "--bg"], ["--muted", "--panel"], ["--muted", "--selected"], ["--muted", "--hl"],
        ["--accent", "--bg"], ["--accent", "--panel"], ["--accent", "--selected"],
        ["--ts", "--bg"], ["--ts", "--panel"], ["--ts", "--hl"],
        ["--on-accent", "--accent"],
        ["--ok", "--panel"], ["--danger", "--panel"], ["--danger", "--bg"], ["--warn-fg", "--warn-bg"],
        // The floating windows: the warning on a panel, and the text of a menu that opens over it.
        ["--warn", "--panel"], ["--ink", "--raised"], ["--muted", "--raised"],
      ];
      for (const [front, back] of pairs) {
        expect(contrast(color(front), color(back)), `${front} on ${back} in ${context}`).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrast(parseColor(onRecord).slice(0, 3), parseColor(record).slice(0, 3)), `Record in ${context}`).toBeGreaterThanOrEqual(4.5);
    }
  });
});

/** `#rrggbb` or `rgb(r g b / a)` as [r, g, b, a]. */
function parseColor(value: string): number[] {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)).concat(1);
  const rgb = /^rgb\((\d+) (\d+) (\d+)(?: \/ ([\d.]+))?\)$/.exec(value);
  if (!rgb) throw new Error(`not a color that this test reads: ${value}`);
  return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), rgb[4] === undefined ? 1 : Number(rgb[4])];
}

/** The contrast ratio of WCAG 2: (lighter + 0.05) / (darker + 0.05) of the relative luminances. */
function contrast(a: number[], b: number[]): number {
  const luminance = (c: number[]) => {
    const [r, g, bl] = c.map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

// ---- The font

describe("the font", () => {
  const fonts = "app/ui/src/fonts";
  const FILES = ["BeVietnamPro-Regular.woff2", "BeVietnamPro-Medium.woff2", "BeVietnamPro-SemiBold.woff2"];

  test("fonts_are_bundled_and_licensed", () => {
    for (const file of FILES) {
      const bytes = readFileSync(join(repoRoot, fonts, file));
      expect(bytes.subarray(0, 4).toString("latin1"), `${file} is a woff2`).toBe("wOF2");
      expect(bytes.length, file).toBeGreaterThan(10_000);
    }
    // Nothing else is in the folder but the license that goes with the files.
    expect(readdirSync(join(repoRoot, fonts)).sort()).toEqual([...FILES, "OFL.txt"].sort());
    const license = read(`${fonts}/OFL.txt`);
    expect(license).toContain("Copyright 2021 The Be Vietnam Pro Project Authors");
    expect(license).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(license).toContain("PERMISSION & CONDITIONS");
    // The license is in the notices, in the texts that every installer carries, and in the generator that writes them.
    expect(read("THIRD_PARTY_NOTICES.md")).toMatch(/Be Vietnam Pro[^\n]*\n?[^\n]*(OFL-1\.1|SIL Open Font License)/);
    expect(read("licenses/native/be-vietnam-pro-OFL.txt")).toBe(license);
    expect(read("licenses/README.md")).toContain("be-vietnam-pro-OFL.txt");
    expect(read("scripts/generate-licenses.py")).toContain("be-vietnam-pro-OFL.txt");
    const texts = read("THIRD_PARTY_LICENSES.txt");
    expect(texts).toContain("Be Vietnam Pro");
    expect(texts.replace(/\s+/g, " ")).toContain(license.trim().replace(/\s+/g, " "));
  });

  test("the_three_weights_are_declared_from_the_files_of_the_app_and_never_from_a_network", () => {
    const faces = tokens.filter((rule) => rule.selectors.includes("@font-face"));
    expect(faces.map((face) => face.declarations["font-weight"]).sort()).toEqual(["400", "500", "600"]);
    for (const face of faces) {
      expect(face.declarations["font-family"]).toBe('"Be Vietnam Pro"');
      const url = /url\("([^"]+)"\)/.exec(face.declarations.src)?.[1] ?? "";
      expect(url, "a file next to the page's sources").toMatch(/^\.\/fonts\/BeVietnamPro-(Regular|Medium|SemiBold)\.woff2$/);
      expect(existsSync(join(here, url)), url).toBe(true);
      expect(face.declarations.src).toContain('format("woff2")');
    }
    expect(tokensCss + appCss).not.toMatch(/https?:|\/\/fonts\./);
  });

  test("the_body_is_set_in_be_vietnam_pro_with_the_system_font_as_the_fallback", () => {
    const root = declarationsOf(tokens, ":root");
    expect(root["--font"]).toMatch(/^"Be Vietnam Pro", system-ui, /);
    expect(root["--font"]).toMatch(/sans-serif$/);
    expect(declarationsOf(app, "body").font).toBe("13px/1.4 var(--font)");
  });
});

// ---- The files

describe("where the look is written", () => {
  test("app_css_takes_its_tokens_from_tokens_css_so_the_floating_windows_can_use_them_without_its_rules", () => {
    expect(appCss.trimStart()).toMatch(/^(\/\*[\s\S]*?\*\/\s*)?@import "\.\/tokens\.css";/);
    // One place for the colors: nothing in app.css is a property of the palette.
    for (const rule of app) {
      expect(Object.keys(rule.declarations).filter((name) => name.startsWith("--")), rule.selectors.join(", ")).toEqual([]);
    }
  });

  test("both_pages_of_the_windows_that_have_the_theme_load_app_css", () => {
    expect(read("app/ui/src/main.ts")).toContain('import "./app.css"');
    expect(read("app/ui/src/meetings/main.ts")).toContain('import "../app.css"');
  });
});

describe("the theme in every window", () => {
  const ENTRIES: [string, string][] = [
    ["main.ts", "./lib/theme"],
    ["meetings/main.ts", "../lib/theme"],
    ["panel/main.ts", "../lib/theme"],
    ["recorder/main.ts", "../lib/theme"],
  ];

  test.each(ENTRIES)("every_window_follows_the_theme_when_it_opens (%s)", (file, from) => {
    const main = read(`app/ui/src/${file}`);
    expect(main).toContain(`import { followTheme } from "${from}";`);
    // Called before the app is mounted: the listener is there from the first moment, and the answer is applied as soon as it is in.
    expect(main.indexOf("void followTheme();"), "followTheme() is called").toBeGreaterThan(-1);
    expect(main.indexOf("void followTheme();")).toBeLessThan(main.indexOf("mount("));
  });

  test("the_floating_windows_take_the_tokens_and_not_the_rules_of_app_css", () => {
    for (const [file, own] of [["panel/main.ts", "./panel.css"], ["recorder/main.ts", "./recorder.css"]]) {
      const main = read(`app/ui/src/${file}`);
      expect(main, file).toContain('import "../tokens.css";');
      expect(main, file).toContain(`import "${own}";`);
      expect(main, file).not.toContain("app.css");
      // Tokens first: what the window writes itself comes after, so that it can use the tokens and overrule nothing by accident.
      expect(main.indexOf("tokens.css"), file).toBeLessThan(main.indexOf(own));
    }
  });
});

// ---- The three weights of a button

describe("the three weights of a button", () => {
  test("a_button_is_outlined_by_default_filled_when_it_is_primary_and_plain_when_it_is_plain", () => {
    const outlined = declarationsOf(app, "button");
    expect(outlined.background).toBe("transparent");
    expect(outlined.border).toBe("1px solid var(--line)");
    expect(outlined.color).toBe("var(--ink)");

    const filled = declarationsOf(app, "button.primary");
    expect(filled.background).toBe("var(--accent)");
    expect(filled.color).toBe("var(--on-accent)");

    const plain = declarationsOf(app, "button.plain");
    expect(plain.background).toBe("transparent");
    expect(plain["border-color"]).toBe("transparent");
    expect(plain.color).toBe("var(--muted)");
  });

  test("a_filled_button_keeps_its_color_under_the_pointer", () => {
    // `:hover` must not make the default button's tint win over a filled one's: the hover tint has no specificity of its own.
    const hover = app.find((rule) => rule.selectors.includes("button:where(:hover:not(:disabled))"));
    expect(hover?.declarations.background).toBe("var(--selected)");
    expect(declarationsOf(app, "button.primary:where(:hover:not(:disabled))").filter).toBe("brightness(1.1)");
  });

  test("Record_is_filled_with_the_record_red_in_the_toolbar_and_in_the_picker", () => {
    for (const file of ["meetings/ImportBar.svelte", "meetings/RecordPicker.svelte"]) {
      const record = declarationsOf(component(file), ".record");
      expect(record.background, file).toBe("var(--record)");
      expect(record["border-color"], file).toBe("var(--record)");
      expect(record.color, file).toBe("var(--on-record)");
    }
    expect(declarationsOf(component("meetings/ImportBar.svelte"), ".mark").background).toBe("var(--on-record)");
  });
});

// ---- Reading

describe("reading the summary", () => {
  test("the_summary_is_14_px_at_a_line_height_of_1_55_and_no_wider_than_75_characters", () => {
    const rules = component("meetings/SummaryPane.svelte");
    const sections = declarationsOf(rules, ".sections");
    expect(sections["font-size"]).toBe("14px");
    expect(sections["line-height"]).toBe("1.55");
    expect(declarationsOf(rules, ".sections section")["max-width"]).toBe("75ch");
    // A little more room between the bullets.
    expect(declarationsOf(rules, ".bullets li + li")["margin-top"]).toBe("4px");
  });

  test("timestamps_are_in_the_monospaced_face_with_tabular_digits_in_the_timestamp_color", () => {
    const stamps: [string, string][] = [
      ["meetings/Transcript.svelte", ".stamp"],
      ["meetings/SummaryPane.svelte", ".stamp"],
      ["meetings/Progress.svelte", ".stamp"],
      ["meetings/Inline.svelte", ".inline :global(a[data-seek])"],
    ];
    for (const [file, selector] of stamps) {
      const stamp = declarationsOf(component(file), selector);
      expect(stamp["font-family"], `${file} ${selector}`).toBe("var(--mono)");
      expect(stamp["font-variant-numeric"], `${file} ${selector}`).toBe("tabular-nums");
    }
    expect(declarationsOf(component("meetings/Transcript.svelte"), ".stamp").color).toBe("var(--ts)");
    expect(declarationsOf(component("meetings/SummaryPane.svelte"), ".stamp").color).toBe("var(--ts)");
  });
});

// ---- Colors of the components

/** The places of a source that write a color of their own: a hex, `rgb(`, `hsl(` and the like, or a named color as a CSS value. */
function hardCodedColors(source: string): string[] {
  const found: string[] = [];
  for (const match of source.matchAll(/#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(/g)) found.push(match[0]);
  const NAMED = /^(?:white|black|red|green|blue|yellow|orange|purple|pink|gray|grey|silver|gold|navy|teal|maroon|lime|aqua|cyan|magenta|brown)$/i;
  for (const rule of parseCss(styleOf(source))) {
    for (const [property, value] of Object.entries(rule.declarations)) {
      if (property.startsWith("--")) continue;
      for (const word of value.split(/[\s,()/]+/)) {
        if (NAMED.test(word)) found.push(`${property}: ${word}`);
      }
    }
  }
  // A color in an attribute of the markup (`stroke="black"` of an icon): it is a color that no theme can change.
  for (const match of source.matchAll(/\b(fill|stroke|stop-color|flood-color|color)=["']([a-zA-Z]+)["']/g)) {
    if (NAMED.test(match[2])) found.push(`${match[1]}=${match[2]}`);
  }
  return found;
}

describe("colors of the components", () => {
  /** Every Svelte component of every window: the settings, the meetings, the dictation panels and the recording bar, with the root component. */
  function components(): string[] {
    const walk = (folder: string): string[] =>
      readdirSync(join(here, folder), { withFileTypes: true }).flatMap((entry) =>
        entry.isDirectory() ? walk(`${folder}/${entry.name}`) : entry.name.endsWith(".svelte") ? [`${folder}/${entry.name}`] : [],
      );
    return [...walk("meetings"), ...walk("settings"), ...walk("panel"), ...walk("recorder"), "App.svelte"];
  }

  /** The style sheets of the floating windows, as a source that the scan reads like a component. */
  const sheets = ["panel/panel.css", "recorder/recorder.css"];
  const asComponent = (file: string) => `<style>\n${read(`app/ui/src/${file}`)}\n</style>`;

  test("the_scan_finds_a_color_that_a_component_wrote_and_passes_the_ones_that_it_may_use", () => {
    const style = (css: string) => `<div></div>\n<style>\n${css}\n</style>`;
    expect(hardCodedColors(style("a { color: #fff; }"))).toEqual(["#fff"]);
    expect(hardCodedColors(style("a { background: rgb(0 0 0 / 0.4); }"))).toEqual(["rgb("]);
    expect(hardCodedColors(style("a { color: red; }"))).toEqual(["color: red"]);
    expect(hardCodedColors(style("a { border: 1px solid white; }"))).toEqual(["border: white"]);
    expect(hardCodedColors(style("a { color: var(--ink); background: transparent; border-color: currentColor; white-space: nowrap; }"))).toEqual([]);
    expect(hardCodedColors(style("a { background: color-mix(in srgb, var(--bg) 85%, transparent); }"))).toEqual([]);
    expect(hardCodedColors('<svg><path stroke="black" fill="none" /><circle fill="currentColor" /></svg>')).toEqual(["stroke=black"]);
    expect(hardCodedColors('<span style:color="rgba(255, 255, 255, 0.6)"></span>')).toEqual(["rgba("]);
  });

  test("no_hard_coded_colors", () => {
    const files = components();
    expect(files.length).toBeGreaterThan(40);
    // The floating windows are in it: their components and their style sheets have no color of their own either.
    for (const folder of ["panel/", "recorder/"]) expect(files.some((file) => file.startsWith(folder)), folder).toBe(true);
    const written = [
      ...files.flatMap((file) => hardCodedColors(read(`app/ui/src/${file}`)).map((color) => `${file}: ${color}`)),
      ...sheets.flatMap((file) => hardCodedColors(asComponent(file)).map((color) => `${file}: ${color}`)),
    ];
    expect(written).toEqual([]);
  });

  test("a_token_is_written_in_all_four_blocks_when_it_changes_with_the_theme_and_in_root_alone_when_it_does_not", () => {
    const declared = Object.fromEntries(Object.entries(contexts).map(([context, read]) => [context, read()]));
    const names = new Set(Object.values(declared).flatMap((block) => Object.keys(block).filter((name) => name.startsWith("--"))));
    for (const name of names) {
      const where = Object.keys(declared).filter((context) => name in declared[context]);
      expect([1, 4], `${name} is in: ${where.join(", ")}`).toContain(where.length);
      if (where.length === 1) expect(where[0], name).toBe("light");
    }
  });

  test("every_token_that_a_component_uses_is_one_that_tokens_css_has", () => {
    const names = (rules: ReturnType<typeof parseCss>) => new Set(rules.flatMap((rule) => Object.keys(rule.declarations).filter((name) => name.startsWith("--"))));
    const defined = names(tokens);
    const used = new Set<string>();
    for (const file of [...components(), ...sheets]) {
      const source = file.endsWith(".css") ? asComponent(file) : read(`app/ui/src/${file}`);
      // A property that the component declares itself (`--size`) is its own, and is not asked of the tokens.
      const own = names(parseCss(styleOf(source)));
      for (const match of source.matchAll(/var\((--[a-z0-9-]+)/g)) {
        if (own.has(match[1])) continue;
        used.add(match[1]);
        expect(defined.has(match[1]), `${file} uses ${match[1]}`).toBe(true);
      }
    }
    // The floating windows use the tokens of the theme (and the new ones), so the check above is about something.
    for (const token of ["--panel", "--ink", "--muted", "--line", "--raised", "--warn", "--record", "--hover", "--ok", "--accent"]) {
      expect(used.has(token), token).toBe(true);
    }
  });

  test("no_component_has_a_monospace_stack_of_its_own", () => {
    for (const file of components()) {
      for (const rule of parseCss(styleOf(read(`app/ui/src/${file}`)))) {
        for (const property of ["font", "font-family"]) {
          const value = rule.declarations[property];
          if (value && /monospace|ui-monospace|Menlo|Consolas/i.test(value)) {
            expect(value, `${file}: ${rule.selectors.join(", ")}`).toMatch(/\bvar\(--mono\)/);
          }
        }
      }
    }
  });

  test("the_scrim_the_shadows_and_the_video_ground_are_the_tokens_of_app_css", () => {
    expect(declarationsOf(component("meetings/Modal.svelte"), ".backdrop").background).toBe("var(--scrim)");
    expect(declarationsOf(component("meetings/Modal.svelte"), ".dialog")["box-shadow"]).toBe("var(--shadow-dialog)");
    expect(declarationsOf(component("meetings/Menu.svelte"), ".choices")["box-shadow"]).toBe("var(--shadow-pop)");
    expect(declarationsOf(component("meetings/ImportBar.svelte"), ".choices")["box-shadow"]).toBe("var(--shadow-pop)");
    expect(declarationsOf(component("meetings/Player.svelte"), "video").background).toBe("var(--video-bg)");
  });
});

// ---- The floating windows

describe("the dictation panels and the recording bar", () => {
  const panelCss = parseCss(read("app/ui/src/panel/panel.css"));
  const recorderCss = parseCss(read("app/ui/src/recorder/recorder.css"));

  test("the_color_scheme_is_the_theme_s_and_not_dark_for_good", () => {
    // tokens.css sets it for the theme that the page is in; a `color-scheme: dark` of their own would keep fields and scroll bars dark in light.
    for (const [name, sheet] of [["panel.css", panelCss], ["recorder.css", recorderCss]] as const) {
      for (const rule of sheet) expect(rule.declarations, `${name} ${rule.selectors.join(", ")}`).not.toHaveProperty("color-scheme");
    }
  });

  test("the_text_and_the_focus_ring_are_the_ink_and_the_accent", () => {
    expect(declarationsOf(panelCss, "body").color).toBe("var(--ink)");
    expect(declarationsOf(recorderCss, "body").color).toBe("var(--ink)");
    expect(declarationsOf(panelCss, "button:focus-visible").outline).toBe("2px solid var(--accent)");
    expect(declarationsOf(recorderCss, "button:focus-visible").outline).toBe("2px solid var(--accent)");
    // The windows are transparent: nothing but what the components draw has a background.
    expect(declarationsOf(panelCss, "html").background).toBe("transparent");
  });

  test("a_floating_panel_keeps_its_translucency_as_a_mix_of_the_panel_color", () => {
    const FLOATING_PANEL = "color-mix(in srgb, var(--panel) 96%, transparent)";
    expect(declarationsOf(component("panel/Bubble.svelte"), ".bubble").background).toBe(FLOATING_PANEL);
    expect(declarationsOf(component("panel/Chip.svelte"), ".chip").background).toBe(FLOATING_PANEL);
    expect(declarationsOf(component("panel/Bubble.svelte"), ".bubble").border).toBe("1px solid var(--line)");
  });

  test("the_card_and_its_menus_are_surfaces_of_the_theme", () => {
    const card = component("panel/Card.svelte");
    expect(declarationsOf(card, ".card").background).toBe("var(--panel)");
    expect(declarationsOf(card, ".menu").background).toBe("var(--raised)");
    expect(declarationsOf(component("panel/ChipMenu.svelte"), ".menu").background).toBe("var(--raised)");
    expect(declarationsOf(component("panel/LanguageMenu.svelte"), ".menu").background).toBe("var(--raised)");
    // The box that the text is typed in is sunk into the card.
    expect(declarationsOf(card, "textarea").background).toBe("var(--bg)");
  });

  test("record_is_the_record_red_in_the_dots_of_the_panels_and_of_the_bar", () => {
    expect(declarationsOf(component("panel/Bubble.svelte"), ".dot").background).toBe("var(--record)");
    expect(declarationsOf(component("panel/Pill.svelte"), ".dot").background).toBe("var(--record)");
    expect(declarationsOf(component("recorder/RecorderApp.svelte"), ".dot").background).toBe("var(--record)");
    // Off is the loud one of the bar: the record red with its own white.
    const off = declarationsOf(component("recorder/RecorderApp.svelte"), ".note.off");
    expect([off.color, off.background]).toEqual(["var(--on-record)", "var(--record)"]);
  });

  test("danger_ok_and_warn_keep_their_meaning", () => {
    const bar = component("recorder/RecorderApp.svelte");
    expect(declarationsOf(bar, ".bar.lit").background).toBe("var(--ok)");
    expect(declarationsOf(bar, ".dot.paused").background).toBe("var(--warn)");
    expect(declarationsOf(bar, ".trash:hover:not(:disabled)").color).toBe("var(--danger)");
    expect(declarationsOf(component("panel/Chip.svelte"), ".mark").color).toBe("var(--ok)");
    expect(declarationsOf(component("panel/Chip.svelte"), ".mark.warning").color).toBe("var(--warn)");
    expect(declarationsOf(component("panel/Card.svelte"), ".dot").background).toBe("var(--ok)");
  });

  test("a_control_that_is_open_or_a_bar_that_is_off_is_drawn_in_a_color_that_shows_in_light_too", () => {
    // The `line` color is a hair of 1.2:1 on a light panel: what has to show on it is a tint of the ink or the accent.
    expect(declarationsOf(component("panel/Chip.svelte"), '.dots[aria-expanded="true"]').background).toBe("var(--accent)");
    expect(declarationsOf(component("recorder/RecorderApp.svelte"), ".bar").background).toBe("var(--hover)");
  });

  test("the_main_button_is_the_accent_in_the_card_the_pill_and_the_bar", () => {
    for (const [file, selector] of [["panel/Card.svelte", ".primary"], ["panel/Pill.svelte", ".stop"], ["recorder/RecorderApp.svelte", ".stop"]]) {
      const main = declarationsOf(component(file), selector);
      expect([main.background, main.color], `${file} ${selector}`).toEqual(["var(--accent)", "var(--on-accent)"]);
    }
  });
});
