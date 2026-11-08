// The commands of the Rust side and its `model-download` and `hotkey-status` events, with their types. Nothing else in the UI calls
// `invoke`: when a command changes, this is the one place to follow it. Names and argument names mirror
// app/src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

export type UiLanguage = "system" | "en" | "vi";
export type Engine = "whisper" | "api";
export type AiProvider = "anthropic" | "openai";
export type TranscribePreset = "openrouter" | "openai" | "groq" | "custom";
/** Which key a command is about. */
export type Account = "ai" | "transcribe";

/** The settings file. */
export interface Settings {
  ui_language: UiLanguage;
  /** BCP-47, e.g. `en-US`. */
  dictation_language: string;
  hotkey: string;
  push_to_talk: boolean;
  auto_refine: boolean;
  auto_write: boolean;
  save_recordings: boolean;
  launch_at_login: boolean;
  ai_provider: AiProvider;
  ai_base_url: string;
  ai_model: string;
  /** Empty = use `ai_model`. */
  summary_model: string;
  vocabulary: string;
  default_edit_preset: string;
  dictation_engine: Engine;
  file_engine: Engine;
  /** `null` = the model recommended for this machine. */
  whisper_model: string | null;
  diarize: boolean;
  transcribe_preset: TranscribePreset;
  transcribe_base_url: string;
  /** The model for files. */
  transcribe_model: string;
  dictation_model: string;
  /** ISO 639-1; empty = detect it. */
  transcribe_language: string;
}

/** The settings plus what the UI may know about the keys (never the keys). */
export interface SettingsView extends Settings {
  has_ai_key: boolean;
  has_transcribe_key: boolean;
  /** No key of its own for transcription, but the AI key is used for it. */
  shares_openrouter_key: boolean;
  /** Why the settings file wasn't used as it was. Only the first view after the start carries it. */
  load_warning: string | null;
  /** What the "system" language means on this machine, as the Rust side resolved it: the language of the tray menu. */
  system_language: "en" | "vi";
}

/** How the shortcut is reached: grabbed by the app, bound by the desktop's portal (Wayland), or not at all. */
export type HotkeyBackend = "global" | "portal" | "none";

/** What the Rust side says about the shortcut. */
export interface HotkeyStatus {
  /** The shortcut as the person reads it: "⌃⌥Space" on the Mac, "Ctrl+Alt+Space" elsewhere. */
  label: string;
  backend: HotkeyBackend;
  /** A key of the texts (`hotkey.taken`, `hotkey.invalid`, `hotkey.wayland_unavailable`) when the shortcut doesn't work. */
  error: string | null;
}

export interface ModelView {
  id: string;
  size_mb: number;
  downloaded: boolean;
  recommended: boolean;
}

export interface DownloadEvent {
  id: string;
  done: number;
  total: number | null;
  state: "running" | "done" | "failed" | "cancelled";
  error?: string;
}

export interface EditPresetView {
  id: string;
  instruction: string;
}

export interface TranscribePresetView {
  id: TranscribePreset;
  models: string[];
  key_url: string | null;
}

export const api = {
  getSettings: () => invoke<SettingsView>("get_settings"),
  saveSettings: (settings: Settings) => invoke<SettingsView>("save_settings", { settings }),
  setAiProvider: (p: AiProvider) => invoke<SettingsView>("set_ai_provider", { p }),
  setTranscribePreset: (p: TranscribePreset) => invoke<SettingsView>("set_transcribe_preset", { p }),
  /** An empty `value` deletes the key. */
  setSecret: (account: Account, value: string) => invoke<SettingsView>("set_secret", { account, value }),
  /**
   * Registers the shortcut, then saves it. Rejects with a key of the texts when it can't be used (`hotkey.invalid`,
   * `hotkey.taken`), else with a sentence about the file.
   */
  setHotkey: (accel: string) => invoke<SettingsView>("set_hotkey", { accel }),
  hotkeyStatus: () => invoke<HotkeyStatus>("hotkey_status"),
  /**
   * The recorder listens for a new shortcut (`true`) or stopped (`false`). While it listens the current shortcut
   * doesn't start a dictation; the Rust side ends the pause by itself after a minute.
   */
  hotkeyRecording: (active: boolean) => invoke<void>("hotkey_recording", { active }),
  checkAi: () => invoke<string>("check_ai"),
  checkTranscribe: () => invoke<string>("check_transcribe"),

  listModels: () => invoke<ModelView[]>("list_models"),
  /** Resolves when the download is over, also when it was cancelled; the events tell how it went. */
  downloadModel: (id: string) => invoke<void>("download_model", { id }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  deleteModel: (id: string) => invoke<void>("delete_model", { id }),
  modelsDiskUsageMb: () => invoke<number>("models_disk_usage_mb"),
  /** Resolves with the `[mm:ss] text` lines. */
  transcribeWav: (path: string) => invoke<string>("transcribe_wav", { path }),
  cancelTranscribe: () => invoke<void>("cancel_transcribe"),

  editPresets: () => invoke<EditPresetView[]>("edit_presets"),
  transcribePresets: () => invoke<TranscribePresetView[]>("transcribe_presets"),
  openMeetingsFolder: () => invoke<void>("open_meetings_folder"),
  /** In the system browser. The capability only allows https. */
  openUrl: (url: string) => openUrl(url),

  /** Resolves with the function that stops listening. */
  onHotkeyStatus: (handler: (status: HotkeyStatus) => void) =>
    listen<HotkeyStatus>("hotkey-status", (event) => handler(event.payload)),
  /** Resolves with the function that stops listening. */
  onModelDownload: (handler: (event: DownloadEvent) => void) =>
    listen<DownloadEvent>("model-download", (event) => handler(event.payload)),
};

/** The text of a failed call: the Rust side sends plain strings, anything else gets its message. */
export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
