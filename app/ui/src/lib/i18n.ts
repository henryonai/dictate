// The words of the UI. `t(key)` looks the text up in the language that is chosen now; the language can change
// while the window is open, and the texts on screen change with it.
import { createSubscriber } from "svelte/reactivity";
import en from "../locales/en.json";
import vi from "../locales/vi.json";
import type { UiLanguage } from "./api";

export type Language = "en" | "vi";

const texts: Record<Language, Record<string, string>> = { en, vi };

/** Vietnamese when the setting is Vietnamese, or follows the system and the system language is. */
export function resolveLanguage(setting: UiLanguage, systemLanguage: string | undefined): Language {
  if (setting === "vi") return "vi";
  if (setting === "system" && systemLanguage?.trim().toLowerCase().startsWith("vi")) return "vi";
  return "en";
}

let current: Language = resolveLanguage("system", navigator.language);

// Reading the language inside a template or an effect makes it run again when the language changes.
let announce: (() => void) | undefined;
const watch = createSubscriber((update) => {
  announce = update;
  return () => {
    announce = undefined;
  };
});

export function setLanguage(setting: UiLanguage): void {
  current = resolveLanguage(setting, navigator.language);
  document.documentElement.lang = current;
  announce?.();
}

export function currentLanguage(): Language {
  watch();
  return current;
}

/** The text for `key` in the current language, with `{name}` filled from `vars`. A key no text has shows as itself. */
export function t(key: string, vars?: Record<string, string | number>): string {
  watch();
  const text = texts[current][key] ?? texts.en[key] ?? key;
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in vars ? String(vars[name]) : whole));
}
