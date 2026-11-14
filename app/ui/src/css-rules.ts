// A small reader of CSS for the tests that hold the look in place (theme.test.ts, Transcript.test.ts). jsdom does not load the styles of
// a component (vitest leaves CSS out), and it has no `var()`: what a test can say about a style is what the source says, so the source is
// read. Not part of the app: no page imports it.
//
// It understands what the project writes: rules, `@media`, `@supports`, `@container`, `@font-face`, and comments. A block that has blocks inside it, other than
// `@media`, (`@keyframes`) is skipped.

export interface CssRule {
  /** The condition of the `@media` that the rule is in, if it is in one. */
  media: string | null;
  /** The condition of the `@supports` that the rule is in (`not (color: color-mix(in srgb, red, blue))`), if it is in one. */
  supports: string | null;
  /** The name and the condition of the `@container` that the rule is in (`bar (max-width: 330px)`), if it is in one. */
  container: string | null;
  /** The selectors of the rule (the comma list, split where the commas are not inside brackets), or `@font-face`. */
  selectors: string[];
  /** The declarations, in order; the value is as written, trimmed. A property that is declared twice keeps the last value. */
  declarations: Record<string, string>;
}

/** Splits at the commas that are not inside (), [] or a string. */
function splitTop(text: string, separator: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quote = "";
  let from = 0;
  for (let at = 0; at < text.length; at += 1) {
    const char = text[at];
    if (quote) {
      if (char === quote) quote = "";
    } else if (char === '"' || char === "'") {
      quote = char;
    } else if (char === "(" || char === "[") {
      depth += 1;
    } else if (char === ")" || char === "]") {
      depth -= 1;
    } else if (char === separator && depth === 0) {
      parts.push(text.slice(from, at));
      from = at + 1;
    }
  }
  parts.push(text.slice(from));
  return parts.map((part) => part.trim()).filter(Boolean);
}

/** The index after the `}` that closes the `{` at `open`. */
function skipBlock(text: string, open: number): number {
  let depth = 0;
  for (let at = open; at < text.length; at += 1) {
    if (text[at] === "{") depth += 1;
    else if (text[at] === "}" && --depth === 0) return at + 1;
  }
  return text.length;
}

export function parseCss(source: string): CssRule[] {
  const text = source.replace(/\/\*[\s\S]*?\*\//g, "");
  const rules: CssRule[] = [];

  function walk(from: number, to: number, media: string | null, supports: string | null, container: string | null) {
    let at = from;
    while (at < to) {
      const open = text.indexOf("{", at);
      if (open === -1 || open >= to) return;
      const prelude = text.slice(at, open).trim();
      const end = skipBlock(text, open);
      if (/^@media\b/.test(prelude)) {
        walk(open + 1, end - 1, prelude.replace(/^@media\s*/, ""), supports, container);
      } else if (/^@supports\b/.test(prelude)) {
        walk(open + 1, end - 1, media, prelude.replace(/^@supports\s*/, ""), container);
      } else if (/^@container\b/.test(prelude)) {
        walk(open + 1, end - 1, media, supports, prelude.replace(/^@container\s*/, ""));
      } else if (!prelude.startsWith("@") || prelude === "@font-face") {
        const declarations: Record<string, string> = {};
        for (const declaration of splitTop(text.slice(open + 1, end - 1), ";")) {
          const colon = declaration.indexOf(":");
          if (colon > 0) declarations[declaration.slice(0, colon).trim()] = declaration.slice(colon + 1).trim();
        }
        rules.push({ media, supports, container, selectors: splitTop(prelude, ","), declarations });
      }
      at = end;
    }
  }

  walk(0, text.length, null, null, null);
  return rules;
}

/** The text of the `<style>` of a Svelte component: its styles, as CSS. */
export function styleOf(component: string): string {
  return /<style[^>]*>([\s\S]*?)<\/style>/.exec(component)?.[1] ?? "";
}

/**
 * The declarations of every rule that has the selector in its list (and is in that `@media`, or in none, and in no `@supports` or `@container`), merged in the order of the
 * rules: what the selector gets, by itself, when it is written as it is here. An empty object when no rule has it.
 */
export function declarationsOf(rules: CssRule[], selector: string, media: string | null = null): Record<string, string> {
  const merged: Record<string, string> = {};
  for (const rule of rules) {
    // A rule under `@supports` is a fallback for a web view that lacks something, and one under `@container` is for a container of a
    // certain size: not what the selector gets by itself.
    if (rule.media === media && rule.supports === null && rule.container === null && rule.selectors.includes(selector)) {
      Object.assign(merged, rule.declarations);
    }
  }
  return merged;
}
