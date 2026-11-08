// What the two panel windows (the bubble and the card) are told, and the small decisions they make from it. The shape
// mirrors `PanelState` in app/src-tauri/src/panel.rs, which sends it as the `panel-state` event.
import { t, type Language } from "../lib/i18n";

/** What is on screen: nothing, or one of the shapes. The bubble window draws bubble and chip, the card window pill and card. */
export type PanelViewName = "hidden" | "bubble" | "pill" | "chip" | "card";

/** Where a dictation is, as `Phase` on the Rust side names it. */
export type Phase = "hidden" | "recording" | "finishing" | "review" | "refining" | "working" | "written";

export type WorkingKind = "writing" | "editing" | "rewriting" | "retranscribing";

/** What the app is working on while the bubble waits. */
export interface Working {
  kind: WorkingKind;
  /** A preset id (`concise`) or the words of a custom request. */
  style: string | null;
  /** The transcription model, while it transcribes again. */
  model: string | null;
  /** The transcription also edits, in the same call. */
  editing: boolean;
}

/** What a notice is about; the words are in the locale files. */
export type NoticeKey = "api_failed" | "not_edited_no_key" | "edit_failed" | "nothing_heard";

export interface Notice {
  key: NoticeKey;
  /** The error behind it, when there is one. */
  detail: string | null;
}

export interface PanelState {
  view: PanelViewName;
  phase: Phase;
  /** The text that is final, and the latest guess for the rest. */
  committed: string;
  partial: string;
  /** The loudness now, 0 to 1. */
  level: number;
  /** BCP-47, as in the settings: `en-US`. */
  language: string;
  durationS: number;
  /** The dictation shortcut as a person reads it: "⌃⌥Space". */
  hotkey: string;
  pushToTalk: boolean;
  working: Working | null;
  /** What was written, or what the card edits. */
  text: string;
  original: string | null;
  style: string | null;
  notice: Notice | null;
  fromChip: boolean;
  /** The card opens with its box for a custom request open: the chip's "Custom request…". */
  openCustom: boolean;
  canWrite: boolean;
  accessibilityMissing: boolean;
}

/** The state of a window that has nothing to show. */
export const HIDDEN_STATE: PanelState = {
  view: "hidden",
  phase: "hidden",
  committed: "",
  partial: "",
  level: 0,
  language: "en-US",
  durationS: 0,
  hotkey: "",
  pushToTalk: false,
  working: null,
  text: "",
  original: null,
  style: null,
  notice: null,
  fromChip: false,
  openCustom: false,
  canWrite: false,
  accessibilityMissing: false,
};

/** The views that are windows of their own: the page is opened with `?view=bubble` or `?view=card`. */
export function isPanelView(view: string | null): view is "bubble" | "card" {
  return view === "bubble" || view === "card";
}

/** The shapes each window draws, as `View::window` on the Rust side sends them: the window that never takes the focus has the bubble and the chip. */
export const VIEWS_OF: Record<"bubble" | "card", PanelViewName[]> = {
  bubble: ["bubble", "chip"],
  card: ["pill", "card"],
};

// ---- The waveform -------------------------------------------------------------------------------------------------

/** How high each of the seven bars goes, as a share of the tallest. */
export const WAVE_SHAPE: readonly number[] = [0.35, 0.7, 1, 0.55, 0.85, 0.3, 0.6];

/** The height of each bar in pixels: 4, and up to 18 more by the shape of the bar and the level (which counts 1.6 times). */
export function barHeights(level: number): number[] {
  const loudness = Number.isFinite(level) ? Math.min(1, Math.max(0, level) * 1.6) : 0;
  return WAVE_SHAPE.map((shape) => 4 + 18 * shape * loudness);
}

// ---- Time and languages -------------------------------------------------------------------------------------------

/** "01:05", and "1:02:05" from an hour on; the fraction is dropped, as `timestamp::format` on the Rust side does. */
export function formatTimer(seconds: number): string {
  const whole = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  const two = (n: number) => String(n).padStart(2, "0");
  if (whole >= 3600) return `${Math.floor(whole / 3600)}:${two(Math.floor(whole / 60) % 60)}:${two(whole % 60)}`;
  return `${two(Math.floor(whole / 60))}:${two(whole % 60)}`;
}

/** "EN" for "en-US": the first two letters in capitals, as `short_label` on the Rust side does. */
export function languageLabel(code: string): string {
  return code.slice(0, 2).toUpperCase();
}

/** The name of a language in the language of the UI ("American English"), starting with a capital. The code itself when the system has no name for it. */
export function languageName(code: string, ui: Language): string {
  try {
    const name = new Intl.DisplayNames([ui], { type: "language" }).of(code);
    return name ? name.charAt(0).toLocaleUpperCase(ui) + name.slice(1) : code;
  } catch {
    return code;
  }
}

/** Languages written without spaces between words, by their first subtag, as in `live::join` on the Rust side. */
const NO_SPACE_LANGUAGES = ["ja", "zh", "yue", "th"];

/** What goes between what is final and the guess for the rest: a space, or nothing for Japanese, Chinese, Cantonese and Thai. */
export function joiner(language: string): string {
  const primary = language.split(/[-_]/)[0].toLowerCase();
  return NO_SPACE_LANGUAGES.includes(primary) ? "" : " ";
}

// ---- Words --------------------------------------------------------------------------------------------------------

/** The title of an edit: the name of a preset, or the words of a custom request as they are. */
function styleTitle(style: string): string {
  const key = `presets.${style}`;
  const title = t(key);
  return title === key ? style : title;
}

/** The header of the bubble: what the app is doing. */
export function bubbleTitle(state: PanelState): string {
  if (state.phase === "recording") return t("panel.listening");
  if (state.phase === "finishing") return t("panel.finishing");
  const work = state.working;
  if (!work) return t("panel.writing");
  switch (work.kind) {
    case "retranscribing":
      return t(work.editing ? "panel.retranscribing_editing" : "panel.retranscribing", { model: work.model ?? "" });
    case "editing":
      return work.style ? t("panel.editing", { style: styleTitle(work.style) }) : t("panel.writing");
    case "rewriting":
      return work.style ? t("panel.rewriting", { style: styleTitle(work.style) }) : t("panel.writing");
    case "writing":
      return t("panel.writing");
  }
}

/** The line under the text of the bubble, if one helps: how to finish, or how to skip the AI edit. */
export function bubbleHint(state: PanelState): string | null {
  const vars = { hotkey: state.hotkey };
  if (state.phase === "recording") return t(state.pushToTalk ? "panel.hint_hold" : "panel.hint_toggle", vars);
  // While the AI edits and nothing is written yet, the shortcut writes the text as it is. A paste in progress ignores it, and
  // after a rewrite begins it starts a new dictation.
  const kind = state.working?.kind;
  if (state.phase === "working" && (kind === "editing" || kind === "retranscribing")) {
    return t("panel.hint_write_now", vars);
  }
  return null;
}

/** The line under the pill: how to stop and how to cancel. */
export function pillCaption(state: PanelState): string {
  return t(state.pushToTalk ? "panel.caption_hold" : "panel.caption_toggle", { hotkey: state.hotkey });
}

// ---- The chip -----------------------------------------------------------------------------------------------------

/** A notice in words: the text of its key, with the error that is behind it where the text has a place for it. */
export function noticeText(notice: Notice): string {
  return t(`notice.${notice.key}`, { detail: notice.detail ?? "" });
}

/** What the chip says: the notice, else "Original" when the text is the one that was heard, else the style, else "Written". */
export function chipTitle(state: PanelState): string {
  if (state.notice) return noticeText(state.notice);
  if (state.original !== null && state.original === state.text) return t("chip.original");
  if (state.style) return styleTitle(state.style);
  return t("chip.written");
}

/** The styles of the ⋯ menu, in the order of the Swift app. */
export const CHIP_STYLES: readonly string[] = ["concise", "email", "bullets", "formal", "prompt", "en", "vi", "clean"];

/** The styles the menu offers: all of them, but the one the text is in already. */
export function chipMenuStyles(current: string | null): string[] {
  return CHIP_STYLES.filter((style) => style !== current);
}

// ---- The card -----------------------------------------------------------------------------------------------------

/** The five buttons of the card, which Cmd (Ctrl) and 1 to 5 press. */
export const CARD_STYLES: readonly string[] = ["clean", "concise", "email", "bullets", "en"];

/** The rest of the styles, in the "More" menu. */
export const CARD_MORE_STYLES: readonly string[] = ["formal", "prompt", "vi"];

/**
 * The header of the card: what the AI is doing; else what it did (a style, and the original it can go back to), also on
 * a card from the chip; else that the text is the chip's to edit, or there to edit or copy.
 */
export function cardHeader(state: PanelState): string {
  if (state.phase === "refining") {
    const work = state.working;
    // As the bubble tells it: the transcription is again under way, and may be doing the edit in the same call.
    if (work?.kind === "retranscribing") {
      return t(work.editing ? "panel.retranscribing_editing" : "panel.retranscribing", { model: work.model ?? "" });
    }
    const style = work?.style ?? state.style;
    return style ? t("card.header_refining", { style: styleTitle(style) }) : t("card.header_refining_plain");
  }
  if (state.style && state.original !== null) return t("card.header_edited", { style: styleTitle(state.style) });
  if (state.fromChip) return t("card.header_from_chip");
  return t("card.header_done");
}

/** The right of the header: how long it was said and in what language, and only the language when there is no time. */
export function cardMeta(state: PanelState, ui: Language): string {
  const language = languageName(state.language, ui);
  return state.durationS > 0 ? `${formatTimer(state.durationS)} · ${language}` : language;
}

/** What a key press says, as much as the shortcuts of the card look at. */
export type KeyLike = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"> & {
  getModifierState?: (key: string) => boolean;
};

export type CardShortcut = { kind: "style"; style: string } | { kind: "undo" } | { kind: "confirm" } | { kind: "discard" };

/** Whether this is a Mac, where the shortcuts use Cmd. The other systems use Ctrl. */
export function onMac(): boolean {
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
}

/**
 * What a key press means in the card: Cmd (Ctrl) and 1 to 5 run a style, with Alt and Z undo, with Return confirm; Esc
 * discards. Keys are told by `code` (the key on the keyboard): with Alt held the letter of the key is another one. On
 * Windows the AltGr key is reported as Ctrl and Alt together, with the AltGraph state set: it types letters, and is no shortcut.
 */
export function cardShortcut(press: KeyLike, mac: boolean): CardShortcut | null {
  if (press.key === "Escape") return { kind: "discard" };
  if (press.getModifierState?.("AltGraph")) return null;
  const mod = mac ? press.metaKey && !press.ctrlKey : press.ctrlKey && !press.metaKey;
  if (!mod) return null;
  if (press.altKey) {
    return press.code === "KeyZ" && !press.shiftKey ? { kind: "undo" } : null;
  }
  if (press.shiftKey) return null;
  if (press.key === "Enter") return { kind: "confirm" };
  const digit = /^Digit([1-5])$/.exec(press.code);
  return digit ? { kind: "style", style: CARD_STYLES[Number(digit[1]) - 1] } : null;
}
