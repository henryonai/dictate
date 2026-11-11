import { describe, expect, test } from "vitest";
import { BAR_COUNT, barHeight, barsLit, hearsSound } from "./levels";

describe("the meter of the recording bar", () => {
  test("bars_lit_follow_the_level: bar i is lit when i/6 is below the level", () => {
    expect(BAR_COUNT).toBe(6);
    const lit = (level: number) => barsLit(level).filter(Boolean).length;

    // Silence lights nothing: bar 0 needs a level above 0.
    expect(lit(0)).toBe(0);
    expect(barsLit(0.01)).toEqual([true, false, false, false, false, false]);
    // 1/6 itself is not below the level: the second bar waits for more.
    expect(lit(1 / 6)).toBe(1);
    expect(lit(0.2)).toBe(2);
    expect(lit(0.5)).toBe(3);
    expect(lit(0.51)).toBe(4);
    expect(lit(5 / 6)).toBe(5);
    expect(lit(0.9)).toBe(6);
    expect(lit(1)).toBe(6);
  });

  test("a level that is off, not a number or outside 0 to 1 does not break the bars", () => {
    expect(barsLit(null)).toEqual([false, false, false, false, false, false]);
    expect(barsLit(Number.NaN)).toEqual([false, false, false, false, false, false]);
    expect(barsLit(-1)).toEqual([false, false, false, false, false, false]);
    expect(barsLit(7)).toEqual([true, true, true, true, true, true]);
  });

  test("the bars grow by 2 points from 6", () => {
    expect([0, 1, 2, 3, 4, 5].map(barHeight)).toEqual([6, 8, 10, 12, 14, 16]);
  });

  test("a track picks up sound above 0.05, and a track that is off never does", () => {
    expect(hearsSound(0.05)).toBe(false);
    expect(hearsSound(0.0501)).toBe(true);
    expect(hearsSound(0)).toBe(false);
    expect(hearsSound(null)).toBe(false);
  });
});
