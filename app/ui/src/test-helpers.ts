// Shared by the tests only: settings as the Rust side sends them, and the api with every call stubbed.
import { vi } from "vitest";
import { api } from "./lib/api";
import type { DownloadEvent, HotkeyStatus, Settings, SettingsTab, SettingsView } from "./lib/api";
import { SettingsStore } from "./lib/settings-store.svelte";
import type { PanelState } from "./panel/state";

/** The defaults of `Settings::default()` on the Rust side. */
export function fakeSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    ui_language: "system",
    dictation_language: "en-US",
    hotkey: "Ctrl+Alt+Space",
    push_to_talk: false,
    auto_refine: true,
    auto_write: true,
    save_recordings: true,
    launch_at_login: false,
    ai_provider: "anthropic",
    ai_base_url: "https://api.anthropic.com",
    ai_model: "claude-haiku-5-5",
    summary_model: "",
    vocabulary: "",
    default_edit_preset: "clean",
    dictation_engine: "whisper",
    file_engine: "whisper",
    whisper_model: null,
    diarize: true,
    transcribe_preset: "openai",
    transcribe_base_url: "https://api.openai.com/v1",
    transcribe_model: "whisper-1",
    dictation_model: "whisper-1",
    transcribe_language: "",
    ...overrides,
  };
}

export function fakeView(overrides: Partial<SettingsView> = {}): SettingsView {
  return {
    ...fakeSettings(),
    has_ai_key: false,
    has_transcribe_key: false,
    shares_openrouter_key: false,
    load_warning: null,
    system_language: "en",
    ...overrides,
  };
}

/** What the Rust side says about the shortcut when it works. */
export function fakeHotkeyStatus(overrides: Partial<HotkeyStatus> = {}): HotkeyStatus {
  return { label: "Ctrl+Alt+Space", backend: "global", error: null, ...overrides };
}

export const EDIT_PRESET_IDS = ["clean", "formal", "concise", "bullets", "prompt", "email", "en", "vi"];

/**
 * Stubs every call of `api` with a calm answer; a test changes the ones it cares about with
 * `vi.mocked(api.checkAi).mockRejectedValue(...)`. Returns a way to send `model-download`, `hotkey-status` and `settings-changed` events.
 */
export function stubApi() {
  let listener: ((event: DownloadEvent) => void) | undefined;
  let hotkeyListener: ((status: HotkeyStatus) => void) | undefined;
  let settingsListener: ((patch: Partial<Settings>) => void) | undefined;
  let tabListener: ((tab: SettingsTab) => void) | undefined;

  vi.spyOn(api, "getSettings").mockResolvedValue(fakeView());
  vi.spyOn(api, "saveSettings").mockImplementation(async (settings) => fakeView(settings));
  vi.spyOn(api, "setAiProvider").mockImplementation(async (p) => fakeView({ ai_provider: p }));
  vi.spyOn(api, "setTranscribePreset").mockImplementation(async (p) => fakeView({ transcribe_preset: p }));
  vi.spyOn(api, "setSecret").mockResolvedValue(fakeView());
  vi.spyOn(api, "setHotkey").mockImplementation(async (accel) => fakeView({ hotkey: accel }));
  vi.spyOn(api, "hotkeyStatus").mockResolvedValue(fakeHotkeyStatus());
  vi.spyOn(api, "hotkeyRecording").mockResolvedValue();
  vi.spyOn(api, "checkAi").mockResolvedValue("Connected: claude-haiku-5-5 answered.");
  vi.spyOn(api, "checkTranscribe").mockResolvedValue("Connected: whisper-1 accepted the test audio.");
  vi.spyOn(api, "listModels").mockResolvedValue([]);
  vi.spyOn(api, "downloadModel").mockResolvedValue();
  vi.spyOn(api, "cancelDownload").mockResolvedValue();
  vi.spyOn(api, "deleteModel").mockResolvedValue();
  vi.spyOn(api, "modelsDiskUsageMb").mockResolvedValue(0);
  vi.spyOn(api, "editPresets").mockResolvedValue(
    EDIT_PRESET_IDS.map((id) => ({ id, instruction: `instruction of ${id}` })),
  );
  vi.spyOn(api, "transcribePresets").mockResolvedValue([
    { id: "openrouter", models: ["google/gemini-3.5-flash", "google/gemini-3.8-flash"], key_url: "https://openrouter.ai/keys" },
    { id: "openai", models: ["whisper-1", "gpt-4o-transcribe"], key_url: "https://platform.openai.com/api-keys" },
    { id: "groq", models: ["whisper-large-v3-turbo"], key_url: "https://console.groq.com/keys" },
    { id: "custom", models: [], key_url: null },
  ]);
  vi.spyOn(api, "openMeetingsFolder").mockResolvedValue();
  vi.spyOn(api, "mediaTools").mockResolvedValue({ ffmpeg: null, ytDlp: null });
  // Not a Mac: no permissions, so no section.
  vi.spyOn(api, "permissionsStatus").mockResolvedValue({ microphone: "not_needed", accessibility: "not_needed" });
  vi.spyOn(api, "requestPermission").mockResolvedValue();
  vi.spyOn(api, "openPermissionSettings").mockResolvedValue();
  vi.spyOn(api, "openUrl").mockResolvedValue();
  vi.spyOn(api, "onModelDownload").mockImplementation(async (handler) => {
    listener = handler;
    return () => {
      listener = undefined;
    };
  });

  vi.spyOn(api, "onHotkeyStatus").mockImplementation(async (handler) => {
    hotkeyListener = handler;
    return () => {
      hotkeyListener = undefined;
    };
  });

  vi.spyOn(api, "onSettingsChanged").mockImplementation(async (handler) => {
    settingsListener = handler;
    return () => {
      settingsListener = undefined;
    };
  });

  vi.spyOn(api, "onSettingsTab").mockImplementation(async (handler) => {
    tabListener = handler;
    return () => {
      tabListener = undefined;
    };
  });

  return {
    emitDownload: (event: DownloadEvent) => listener?.(event),
    emitHotkey: (status: HotkeyStatus) => hotkeyListener?.(status),
    emitSettings: (patch: Partial<Settings>) => settingsListener?.(patch),
    emitTab: (tab: SettingsTab) => tabListener?.(tab),
  };
}

/** A store that has loaded a view of the settings, as the window has after it opens. */
export async function loadedStore(overrides: Partial<SettingsView> = {}): Promise<SettingsStore> {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView(overrides));
  const store = new SettingsStore();
  await store.load();
  return store;
}

/**
 * Fakes the two clocks a guard against a double click may read, and nothing else: the wall clock (`Date`) and the
 * one that only goes forward (`performance`). A test decides how long ago something started. The caller restores
 * them with `vi.useRealTimers()`.
 */
export function freezeTime() {
  vi.useFakeTimers({ toFake: ["Date", "performance"] });
}

/** Time goes by on both clocks. */
export function passTime(ms: number) {
  vi.advanceTimersByTime(ms);
}

/** The wall clock is set back, as a time sync or a person does; the clock that only goes forward isn't affected. */
export function setWallClockBack(ms: number) {
  vi.setSystemTime(Date.now() - ms);
}

/** A panel state as the Rust side sends it for a toggle-mode dictation that has just begun in a text field. */
export function fakePanelState(overrides: Partial<PanelState> = {}): PanelState {
  return {
    view: "bubble",
    phase: "recording",
    committed: "",
    partial: "",
    level: 0,
    language: "en-US",
    durationS: 0,
    hotkey: "⌃⌥Space",
    pushToTalk: false,
    working: null,
    text: "",
    original: null,
    style: null,
    notice: null,
    error: null,
    fromChip: false,
    openCustom: false,
    canWrite: true,
    accessibilityMissing: false,
    ...overrides,
  };
}

/** The 8 favourites and 24 others of `dictate_services::dictation::lang`. */
export const FAVOURITE_LANGUAGES = ["en-US", "vi-VN", "en-GB", "ja-JP", "ko-KR", "zh-CN", "fr-FR", "de-DE"];
export const OTHER_LANGUAGES = [
  "es-ES", "it-IT", "pt-BR", "pt-PT", "nl-NL", "pl-PL", "ru-RU", "uk-UA", "tr-TR", "id-ID", "ms-MY", "th-TH",
  "hi-IN", "ar-SA", "he-IL", "el-GR", "cs-CZ", "sv-SE", "da-DK", "fi-FI", "nb-NO", "hu-HU", "ro-RO", "zh-TW",
];

/**
 * Stubs the calls of the two panel windows. Returns a way to send `panel-state` events, as the Rust side does, and
 * the stopper that the listener hands back.
 */
export function stubPanelApi() {
  let listener: ((state: PanelState) => void) | undefined;
  const stopListening = vi.fn(() => {
    listener = undefined;
  });
  vi.spyOn(api, "dictationLanguages").mockResolvedValue({
    favourites: FAVOURITE_LANGUAGES,
    others: OTHER_LANGUAGES,
  });
  vi.spyOn(api, "dictationStop").mockResolvedValue();
  vi.spyOn(api, "dictationCancel").mockResolvedValue();
  vi.spyOn(api, "dictationSetLanguage").mockResolvedValue();
  vi.spyOn(api, "dictationRewrite").mockResolvedValue();
  vi.spyOn(api, "dictationCustom").mockResolvedValue();
  vi.spyOn(api, "dictationOpenCard").mockResolvedValue();
  vi.spyOn(api, "dictationCopy").mockResolvedValue();
  vi.spyOn(api, "dictationCardText").mockResolvedValue();
  vi.spyOn(api, "dictationUndo").mockResolvedValue();
  vi.spyOn(api, "dictationPasteCard").mockResolvedValue();
  vi.spyOn(api, "dictationChipHover").mockResolvedValue();
  vi.spyOn(api, "dictationMenuOpen").mockResolvedValue();
  vi.spyOn(api, "dictationGrantAccessibility").mockResolvedValue();
  vi.spyOn(api, "dictationOpenSettings").mockResolvedValue();
  vi.spyOn(api, "panelState").mockResolvedValue(fakePanelState({ view: "hidden", phase: "hidden" }));
  vi.spyOn(api, "panelResized").mockResolvedValue();
  vi.spyOn(api, "panelLanguage").mockResolvedValue("en");
  vi.spyOn(api, "onPanelState").mockImplementation(async (handler) => {
    listener = handler;
    return stopListening;
  });
  return {
    emitState: (state: PanelState) => listener?.(state),
    stopListening,
  };
}
