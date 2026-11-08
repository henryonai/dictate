import { afterEach, describe, expect, test } from "vitest";
import { setLanguage } from "../lib/i18n";
import { fakePanelState } from "../test-helpers";
import {
  barHeights,
  cardHeader,
  cardMeta,
  cardShortcut,
  CARD_MORE_STYLES,
  CARD_STYLES,
  chipMenuStyles,
  chipTitle,
  formatTimer,
  isPanelView,
  joiner,
  languageLabel,
  languageName,
  noticeText,
  VIEWS_OF,
  WAVE_SHAPE,
} from "./state";

afterEach(() => setLanguage("en"));

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

  test("the bubble window draws the bubble and the chip, the card window the pill and the card", () => {
    // As `View::window` on the Rust side has it.
    expect(VIEWS_OF.bubble).toEqual(["bubble", "chip"]);
    expect(VIEWS_OF.card).toEqual(["pill", "card"]);
  });
});

describe("the chip", () => {
  const written = (overrides: Parameters<typeof fakePanelState>[0] = {}) =>
    fakePanelState({ view: "chip", phase: "written", text: "Hello there.", ...overrides });
  const notice = { key: "edit_failed" as const, detail: "rate limited" };

  test("chip_title_rules: a notice comes first, over everything else", () => {
    const state = written({ notice, original: "Hello there.", style: "concise" });
    expect(chipTitle(state)).toBe("Couldn't edit: rate limited");
  });

  test("chip_title_rules: Original when the original is what was written, over the style", () => {
    expect(chipTitle(written({ original: "Hello there.", style: "concise" }))).toBe("Original");
    // Not the same text: it was edited, and the style says how.
    expect(chipTitle(written({ original: "hello there", style: "concise" }))).toBe("Shorten");
  });

  test("chip_title_rules: the style, as its title, or the words of a custom request as they are", () => {
    expect(chipTitle(written({ original: "hello", style: "email" }))).toBe("Turn into an email");
    expect(chipTitle(written({ original: "hello", style: "rewrite for my boss" }))).toBe("rewrite for my boss");
  });

  test("chip_title_rules: Written when nothing was done to the text", () => {
    expect(chipTitle(written())).toBe("Written");
    // An original that was never kept is not an original that equals the text.
    expect(chipTitle(written({ original: null, style: null }))).toBe("Written");
  });

  test("chip_title_rules: in Vietnamese when the UI is", () => {
    setLanguage("vi");
    expect(chipTitle(written())).toBe("Đã viết");
    expect(chipTitle(written({ original: "Hello there." }))).toBe("Bản gốc");
    expect(chipTitle(written({ style: "concise" }))).toBe("Rút gọn");
  });

  test("every notice has its words, with the error where the words have a place for it", () => {
    expect(noticeText({ key: "api_failed", detail: "401" })).toBe(
      "Couldn't re-transcribe with the API (401) — kept the local version.",
    );
    expect(noticeText({ key: "not_edited_no_key", detail: null })).toBe("Not edited by AI — no API key yet");
    expect(noticeText({ key: "edit_failed", detail: "timeout" })).toBe("Couldn't edit: timeout");
    expect(noticeText({ key: "nothing_heard", detail: null })).toBe("Didn't hear anything.");
    setLanguage("vi");
    expect(noticeText({ key: "api_failed", detail: "401" })).toBe(
      "Không phiên âm lại được bằng API (401) — giữ bản trên máy.",
    );
    expect(noticeText({ key: "not_edited_no_key", detail: null })).toBe("Chưa sửa bằng AI — chưa có API key");
    expect(noticeText({ key: "edit_failed", detail: "timeout" })).toBe("Chưa sửa được: timeout");
    expect(noticeText({ key: "nothing_heard", detail: null })).toBe("Không nghe được gì.");
  });

  test("chip_menu_order_skips_current_style: the eight styles in the order of the Swift app", () => {
    expect(chipMenuStyles(null)).toEqual(["concise", "email", "bullets", "formal", "prompt", "en", "vi", "clean"]);
  });

  test("chip_menu_order_skips_current_style: the style the text is in already is not offered", () => {
    expect(chipMenuStyles("email")).toEqual(["concise", "bullets", "formal", "prompt", "en", "vi", "clean"]);
    expect(chipMenuStyles("clean")).toEqual(["concise", "email", "bullets", "formal", "prompt", "en", "vi"]);
    // A custom request is none of the eight.
    expect(chipMenuStyles("rewrite for my boss")).toHaveLength(8);
  });
});

describe("the card", () => {
  const review = (overrides: Parameters<typeof fakePanelState>[0] = {}) =>
    fakePanelState({ view: "card", phase: "review", text: "Hello there.", ...overrides });

  test("the header says what the AI is doing, what it did, or that the text is there to edit", () => {
    expect(cardHeader(review({ phase: "refining", working: { kind: "editing", style: "concise", model: null, editing: false } }))).toBe(
      "AI is editing: Shorten…",
    );
    expect(cardHeader(review({ phase: "refining", style: "email" }))).toBe("AI is editing: Turn into an email…");
    expect(cardHeader(review({ phase: "refining" }))).toBe("AI is editing…");
    expect(cardHeader(review({ style: "concise" }))).toBe("Edited — Shorten");
    expect(cardHeader(review({ fromChip: true }))).toBe("Edit what was just written");
    expect(cardHeader(review())).toBe("Done — edit or copy");
  });

  test("the header is in Vietnamese when the UI is", () => {
    setLanguage("vi");
    expect(cardHeader(review({ style: "concise" }))).toBe("Đã sửa — Rút gọn");
    expect(cardHeader(review({ fromChip: true }))).toBe("Chỉnh sửa đoạn vừa viết");
    expect(cardHeader(review())).toBe("Xong — sửa hoặc sao chép");
  });

  test("the meta line is the time and the name of the language, and only the language without a time", () => {
    expect(cardMeta(review({ durationS: 65, language: "en-US" }), "en")).toBe(`01:05 · ${languageName("en-US", "en")}`);
    expect(cardMeta(review({ durationS: 0, language: "vi-VN" }), "vi")).toBe(languageName("vi-VN", "vi"));
  });

  test("the five style buttons are the ones of the Swift app, and the rest are under More", () => {
    expect(CARD_STYLES).toEqual(["clean", "concise", "email", "bullets", "en"]);
    expect(CARD_MORE_STYLES).toEqual(["formal", "prompt", "vi"]);
  });
});

describe("the shortcuts of the card", () => {
  /** What a key press says: the key, the physical key, and the modifiers held. */
  const press = (code: string, held: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}, key = "") => ({
    key,
    code,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...held,
  });

  test("card_shortcuts_1_to_5: Cmd and a digit run a style on a Mac, and Ctrl does nothing", () => {
    CARD_STYLES.forEach((style, i) => {
      expect(cardShortcut(press(`Digit${i + 1}`, { metaKey: true }), true)).toEqual({ kind: "style", style });
      expect(cardShortcut(press(`Digit${i + 1}`, { ctrlKey: true }), true)).toBeNull();
    });
  });

  test("card_shortcuts_1_to_5: Ctrl and a digit run a style elsewhere, and Cmd (the Windows key) does nothing", () => {
    CARD_STYLES.forEach((style, i) => {
      expect(cardShortcut(press(`Digit${i + 1}`, { ctrlKey: true }), false)).toEqual({ kind: "style", style });
      expect(cardShortcut(press(`Digit${i + 1}`, { metaKey: true }), false)).toBeNull();
    });
  });

  test("card_shortcuts_1_to_5: a digit alone, 0, 6 and a digit with Shift or Alt are not shortcuts", () => {
    expect(cardShortcut(press("Digit1"), true)).toBeNull();
    expect(cardShortcut(press("Digit0", { metaKey: true }), true)).toBeNull();
    expect(cardShortcut(press("Digit6", { metaKey: true }), true)).toBeNull();
    expect(cardShortcut(press("Digit2", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(cardShortcut(press("Digit2", { metaKey: true, altKey: true }), true)).toBeNull();
  });

  test("Alt with Cmd (Ctrl) and Z undoes, by the key and not the letter that Alt makes of it", () => {
    expect(cardShortcut(press("KeyZ", { metaKey: true, altKey: true }, "Ω"), true)).toEqual({ kind: "undo" });
    expect(cardShortcut(press("KeyZ", { ctrlKey: true, altKey: true }), false)).toEqual({ kind: "undo" });
    // Without Alt it is the undo of the text box, which is not ours.
    expect(cardShortcut(press("KeyZ", { metaKey: true }), true)).toBeNull();
  });

  test("Cmd (Ctrl) and Return confirm, Esc discards", () => {
    expect(cardShortcut(press("Enter", { metaKey: true }, "Enter"), true)).toEqual({ kind: "confirm" });
    expect(cardShortcut(press("Enter", { ctrlKey: true }, "Enter"), false)).toEqual({ kind: "confirm" });
    // Return alone is a new line in the text box.
    expect(cardShortcut(press("Enter", {}, "Enter"), true)).toBeNull();
    expect(cardShortcut(press("Escape", {}, "Escape"), true)).toEqual({ kind: "discard" });
    expect(cardShortcut(press("Escape", {}, "Escape"), false)).toEqual({ kind: "discard" });
  });
});
