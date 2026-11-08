import { describe, expect, test } from "vitest";
import { barHeights, formatTimer, isPanelView, joiner, languageLabel, languageName, WAVE_SHAPE } from "./state";

describe("waveform", () => {
  test("waveform_bar_heights: no sound leaves every bar at 4", () => {
    expect(barHeights(0)).toEqual([4, 4, 4, 4, 4, 4, 4]);
  });

  test("waveform_bar_heights: full level is 4 + 18 times the shape of each bar", () => {
    const heights = barHeights(1);
    expect(heights).toHaveLength(7);
    WAVE_SHAPE.forEach((shape, i) => expect(heights[i]).toBeCloseTo(4 + 18 * shape, 9));
  });

  test("waveform_bar_heights: the level is multiplied by 1.6 and stops at 1", () => {
    // 0.25 * 1.6 = 0.4 of the full height, and from 0.625 up the bars are as high as they get.
    barHeights(0.25).forEach((height, i) => expect(height).toBeCloseTo(4 + 18 * WAVE_SHAPE[i] * 0.4, 9));
    expect(barHeights(0.625)).toEqual(barHeights(1));
    expect(barHeights(5)).toEqual(barHeights(1));
  });

  test("waveform_bar_heights: a level below 0 or that is not a number is silence", () => {
    expect(barHeights(-1)).toEqual(barHeights(0));
    expect(barHeights(Number.NaN)).toEqual(barHeights(0));
  });

  test("the shape is the one of the Swift app", () => {
    expect(WAVE_SHAPE).toEqual([0.35, 0.7, 1, 0.55, 0.85, 0.3, 0.6]);
  });
});

describe("formatTimer", () => {
  test("is mm:ss, and h:mm:ss from an hour, with the fraction dropped", () => {
    expect(formatTimer(0)).toBe("00:00");
    expect(formatTimer(65.9)).toBe("01:05");
    expect(formatTimer(3599)).toBe("59:59");
    expect(formatTimer(3725)).toBe("1:02:05");
  });

  test("never shows a negative or a missing time", () => {
    expect(formatTimer(-3)).toBe("00:00");
    expect(formatTimer(Number.NaN)).toBe("00:00");
  });
});

describe("languages", () => {
  test("the label is the first two letters of the code in capitals, as short_label is on the Rust side", () => {
    expect(languageLabel("en-US")).toBe("EN");
    expect(languageLabel("vi-VN")).toBe("VI");
    expect(languageLabel("zh-TW")).toBe("ZH");
    expect(languageLabel("ja")).toBe("JA");
    expect(languageLabel("")).toBe("");
  });

  test("a name is in the language of the UI, and starts with a capital", () => {
    expect(languageName("en-US", "en")).toMatch(/^English|^American English/);
    const vietnamese = languageName("en-US", "vi");
    expect(vietnamese).not.toBe(languageName("en-US", "en"));
    expect(vietnamese[0]).toBe(vietnamese[0].toLocaleUpperCase("vi"));
  });

  test("a code the system doesn't know is shown as it is", () => {
    expect(languageName("not a code", "en")).toBe("not a code");
  });

  test("live text is joined with a space, and with none for the languages written without spaces", () => {
    expect(joiner("en-US")).toBe(" ");
    expect(joiner("vi-VN")).toBe(" ");
    for (const code of ["ja-JP", "zh-CN", "zh-TW", "yue-HK", "th-TH", "ja"]) {
      expect(joiner(code)).toBe("");
    }
  });
});

describe("views", () => {
  test("panel_ignores_unknown_view: only bubble and card are windows", () => {
    expect(isPanelView("bubble")).toBe(true);
    expect(isPanelView("card")).toBe(true);
    for (const other of ["settings", "pill", "", "BUBBLE"]) expect(isPanelView(other)).toBe(false);
    expect(isPanelView(null)).toBe(false);
  });
});
