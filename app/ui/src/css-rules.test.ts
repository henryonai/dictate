import { expect, test } from "vitest";
import { declarationsOf, parseCss } from "./css-rules";

test("a rule inside @container is read with its condition, and is not what its selector gets by itself", () => {
  const rules = parseCss(`
    .bar { container: bar / inline-size; gap: 6px; }
    @container bar (max-width: 330px) {
      .total { display: none; }
    }
    .total { color: red; }
    @media (max-width: 900px) {
      @container bar (min-width: 10px) { .total { opacity: 0; } }
    }
  `);

  const inside = rules.filter((rule) => rule.container !== null);
  expect(inside.map((rule) => [rule.container, rule.media, rule.selectors, rule.declarations])).toEqual([
    ["bar (max-width: 330px)", null, [".total"], { display: "none" }],
    ["bar (min-width: 10px)", "(max-width: 900px)", [".total"], { opacity: "0" }],
  ]);
  // By itself `.total` is only what is outside the container query; the others are conditions of their own.
  expect(declarationsOf(rules, ".total")).toEqual({ color: "red" });
  expect(declarationsOf(rules, ".bar")).toEqual({ container: "bar / inline-size", gap: "6px" });
  expect(rules.find((rule) => rule.container === null && rule.selectors.includes(".total"))?.container).toBeNull();
});
