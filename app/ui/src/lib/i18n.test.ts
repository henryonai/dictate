import { afterEach, describe, expect, test, vi } from "vitest";
import en from "../locales/en.json";
import viStrings from "../locales/vi.json";
import { currentLanguage, resolveLanguage, setLanguage, t } from "./i18n";

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

describe("the locale files", () => {
  test("every key exists in both locales", () => {
    expect(Object.keys(viStrings).sort()).toEqual(Object.keys(en).sort());
  });

  test("no empty translations", () => {
    for (const v of [...Object.values(en), ...Object.values(viStrings)]) {
      expect(v.trim()).not.toBe("");
    }
  });

  test("both locales use the same {variables} in every text", () => {
    const variables = (text: string) => (text.match(/\{\w+\}/g) ?? []).sort();
    for (const key of Object.keys(en) as (keyof typeof en)[]) {
      expect(variables(viStrings[key]), key).toEqual(variables(en[key]));
    }
  });
});

describe("resolveLanguage", () => {
  test("follows the setting, and the system language only for system", () => {
    expect(resolveLanguage("system", "vi-VN")).toBe("vi");
    expect(resolveLanguage("system", "en-GB")).toBe("en");
    expect(resolveLanguage("en", "vi-VN")).toBe("en");
    expect(resolveLanguage("vi", "en-US")).toBe("vi");
  });

  test("any spelling of a Vietnamese system language counts, and other languages mean English", () => {
    expect(resolveLanguage("system", "vi")).toBe("vi");
    expect(resolveLanguage("system", "VI_vn")).toBe("vi");
    expect(resolveLanguage("system", "fr-FR")).toBe("en");
    expect(resolveLanguage("system", "")).toBe("en");
    expect(resolveLanguage("system", undefined)).toBe("en");
  });
});

describe("t", () => {
  test("fills variables", () => {
    setLanguage("en");
    expect(t("models.size_mb", { n: 466 })).toBe("466 MB");
  });

  test("speaks the chosen language", () => {
    setLanguage("vi");
    expect(t("tabs.general")).toBe("Chung");
    setLanguage("en");
    expect(t("tabs.general")).toBe("General");
  });

  test("shows the key itself when no text has it, so that a gap is easy to see", () => {
    expect(t("no.such.key")).toBe("no.such.key");
  });

  test("leaves a {variable} nobody gave in place", () => {
    expect(t("models.size_mb")).toBe("{n} MB");
  });
});

describe("setLanguage", () => {
  test("system takes the language of the web view", () => {
    vi.spyOn(navigator, "language", "get").mockReturnValue("vi-VN");
    setLanguage("system");
    expect(currentLanguage()).toBe("vi");
    expect(document.documentElement.lang).toBe("vi");
  });

  test("an explicit choice wins over the web view", () => {
    vi.spyOn(navigator, "language", "get").mockReturnValue("vi-VN");
    setLanguage("en");
    expect(currentLanguage()).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });
});
