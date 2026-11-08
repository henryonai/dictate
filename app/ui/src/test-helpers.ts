// Shared by the tests only: settings as the Rust side sends them, and the api with every call stubbed.
import { vi } from "vitest";
import { api } from "./lib/api";
import type { DownloadEvent, Settings, SettingsView } from "./lib/api";
import { SettingsStore } from "./lib/settings-store.svelte";

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

export const EDIT_PRESET_IDS = ["clean", "formal", "concise", "bullets", "prompt", "email", "en", "vi"];

/**
 * Stubs every call of `api` with a calm answer; a test changes the ones it cares about with
 * `vi.mocked(api.checkAi).mockRejectedValue(...)`. Returns a way to send `model-download` events.
 */
export function stubApi() {
  let listener: ((event: DownloadEvent) => void) | undefined;

  vi.spyOn(api, "getSettings").mockResolvedValue(fakeView());
  vi.spyOn(api, "saveSettings").mockImplementation(async (settings) => fakeView(settings));
  vi.spyOn(api, "setAiProvider").mockImplementation(async (p) => fakeView({ ai_provider: p }));
  vi.spyOn(api, "setTranscribePreset").mockImplementation(async (p) => fakeView({ transcribe_preset: p }));
  vi.spyOn(api, "setSecret").mockResolvedValue(fakeView());
  vi.spyOn(api, "checkAi").mockResolvedValue("Connected: claude-haiku-5-5 answered.");
  vi.spyOn(api, "checkTranscribe").mockResolvedValue("Connected: whisper-1 accepted the test audio.");
  vi.spyOn(api, "listModels").mockResolvedValue([]);
  vi.spyOn(api, "downloadModel").mockResolvedValue();
  vi.spyOn(api, "cancelDownload").mockResolvedValue();
  vi.spyOn(api, "deleteModel").mockResolvedValue();
  vi.spyOn(api, "modelsDiskUsageMb").mockResolvedValue(0);
  vi.spyOn(api, "transcribeWav").mockResolvedValue("[00:00] hello");
  vi.spyOn(api, "cancelTranscribe").mockResolvedValue();
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
  vi.spyOn(api, "openUrl").mockResolvedValue();
  vi.spyOn(api, "onModelDownload").mockImplementation(async (handler) => {
    listener = handler;
    return () => {
      listener = undefined;
    };
  });

  return { emitDownload: (event: DownloadEvent) => listener?.(event) };
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
