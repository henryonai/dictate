// The commands of the Rust side and its events (`model-download`, `hotkey-status`, `settings-changed`, `meeting-job`,
// `meetings-changed`), with their types. Nothing else in the UI calls `invoke`: when a command changes, this is the one
// place to follow it. Names and argument names mirror app/src-tauri/src/commands.rs and app/src-tauri/src/meetings/.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { PanelState } from "../panel/state";

export type UiLanguage = "system" | "en" | "vi";
export type Engine = "whisper" | "api";
export type AiProvider = "anthropic" | "openai";
export type TranscribePreset = "openrouter" | "openai" | "groq" | "custom";
/** Which key a command is about. */
export type Account = "ai" | "transcribe";
/** The pages of Settings: what the Rust side may ask it to show (`settings-tab`). */
export type SettingsTab = "general" | "speech" | "ai" | "accounts";

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

/**
 * What the system says about a permission the app needs. Windows and Linux ask for none of them: `not_needed` there,
 * which is how the Settings window knows to show no Permissions section.
 */
export type PermissionState = "granted" | "denied" | "not_determined" | "not_needed";
/** The permissions of macOS that dictation uses. */
export type PermissionKind = "microphone" | "accessibility";

export interface PermissionsView {
  microphone: PermissionState;
  accessibility: PermissionState;
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

/** The languages of the dictation menu: the favourites first, then the ones under "Other languages". */
export interface DictationLanguages {
  favourites: string[];
  others: string[];
}

/**
 * Which tools of the system are installed, with the version each says it is (`null` for one that is not; "unknown" for one that is
 * there and says nothing sensible), and the command that installs each on this system.
 */
export interface ToolsView {
  ffmpeg: string | null;
  ytDlp: string | null;
  ffmpegCommand: string;
  ytDlpCommand: string;
}

export interface TranscribePresetView {
  id: TranscribePreset;
  models: string[];
  key_url: string | null;
}

// ---- The meetings window. The shapes mirror app/src-tauri/src/meetings/{views.rs,commands.rs}, in camelCase.

/** The step a job is in. */
export type StepName = "download" | "audio" | "transcript" | "summary";

/** The names of the pipeline's error kinds, as the Rust side sends them: the error card picks its buttons by them. */
export type ErrorKind =
  | "Api"
  | "MissingTool"
  | "Unsupported"
  | "NoModel"
  | "NoAudio"
  | "NoMedia"
  | "Empty"
  | "Unfinished"
  | "Cancelled"
  | "Other";

/**
 * Why something failed: `key` is a key of the texts (`meetings.error.*`) with `{detail}` and `{command}` to fill in; `detail`
 * is what is behind it, in English; `command` is what installs a missing tool. Some of the commands reject with one.
 */
export interface ErrorView {
  kind: ErrorKind;
  key: string;
  detail: string | null;
  command: string | null;
}

function isErrorView(value: unknown): value is ErrorView {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ErrorView).kind === "string" &&
    typeof (value as ErrorView).key === "string"
  );
}

function sentenceOf(failure: unknown): string {
  if (typeof failure === "string") return failure;
  if (failure instanceof Error) return failure.message;
  if (typeof failure === "object" && failure !== null && typeof (failure as { message?: unknown }).message === "string") {
    return (failure as { message: string }).message;
  }
  return "";
}

/**
 * What a command failed with, as an error to show. The commands that reject with an `ErrorView` do (and the wrappers below
 * make one of any other failure, such as the IPC itself failing with an `Error`); a sentence in English is the detail of an
 * error of no kind in particular.
 */
export function asErrorView(failure: unknown): ErrorView {
  if (isErrorView(failure)) return failure;
  return { kind: "Other", key: "meetings.error.other", detail: sentenceOf(failure), command: null };
}

/** A command that rejects with an `ErrorView`: whatever it fails with reaches the caller as one. */
function invokeView<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args).catch((failure: unknown) => {
    throw asErrorView(failure);
  });
}

export type OutcomeView = { kind: "done"; summary: boolean } | { kind: "failed"; error: ErrorView };

/** A job as the `meeting-job` event and a row carry it. An ended job has no step and has its outcome. */
export interface JobView {
  /** The recording's id. */
  id: string;
  /** `null` while the job only waits (for the start of the app, or for another local transcription). */
  step: StepName | null;
  /** 0 to 1; `null` when there is no figure yet. */
  progress: number | null;
  /** When the step began, in milliseconds since the Unix epoch. */
  stepStartedMs: number;
  outcome: OutcomeView | null;
}

export type MeetingKind = "screen" | "file" | "link";

/** One recording of the list. */
export interface MeetingRow {
  id: string;
  title: string;
  kind: MeetingKind;
  /** Seconds since the Unix epoch; 0 when the recording has no date. */
  created: number;
  /** Seconds, once the audio is known. */
  duration: number | null;
  hasSummary: boolean;
  /** The action items that are not ticked. */
  openTasks: number;
  /** The job that works on the recording now. */
  live: JobView | null;
  /** Why the recording is not done: its last job failed or was cancelled, or the app closed before it was processed. */
  error: ErrorView | null;
}

export interface LineView {
  /** Seconds from the start of the recording. */
  start: number;
  /** `mm:ss` or `h:mm:ss`; empty for a line that has none. */
  stamp: string;
  speaker: string | null;
  text: string;
}

export interface SpeakerView {
  name: string;
  lines: number;
  sample: LineView;
}

export interface ActionView {
  /** What `meetingSetTask` takes. */
  index: number;
  owner: string | null;
  text: string;
  done: boolean;
}

export interface MarkView {
  seconds: number;
  label: string;
  text: string;
}

export interface SectionView {
  /** Empty for the text before the first heading. */
  heading: string;
  body: string;
}

export interface SummaryView {
  /** The whole of `summary.md`, as the model wrote it. */
  markdown: string;
  title: string | null;
  summary: string | null;
  keyPoints: string | null;
  decisions: string[];
  actions: ActionView[];
  questions: string[];
  timeline: MarkView[];
  other: SectionView[];
}

/** The media to play. The address changes with every launch of the app: never keep it. */
export interface MediaView {
  url: string;
  video: boolean;
}

export interface MeetingDetail {
  row: MeetingRow;
  /** The link, the path of the imported file, or the screen recording's own word. */
  source: string;
  /** The host of a link (without `www.`) or the file name of an import; empty for a screen recording. */
  sourceLabel: string;
  transcript: LineView[];
  /** The transcript is the rougher live captions of a screen recording. */
  transcriptIsLive: boolean;
  /** Empty when the transcript is live captions. */
  speakers: SpeakerView[];
  summary: SummaryView | null;
  /** `null` when the media server did not start. */
  media: MediaView | null;
  /** The recording's media file is named in `meta.json` and is gone (even when `audio.m4a` plays instead). */
  mediaMissing: boolean;
}

/** What the empty state and the "no summary" texts need to know of the settings. */
export interface Readiness {
  /** The AI model that writes summaries is set up. */
  ai: boolean;
  /** The Whisper model of this computer is downloaded. */
  whisper: boolean;
  /**
   * The language of the UI as the settings choose it ("same as the system" already resolved). The meetings window can't read the
   * settings, so this is how it learns its language: it asks again when it gets the focus back.
   */
  language: "en" | "vi";
}

/** What the window hears of a file dragged over it: the paths come with a drop. */
export type FileDropEvent = { type: "enter" } | { type: "leave" } | { type: "drop"; paths: string[] };

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

  editPresets: () => invoke<EditPresetView[]>("edit_presets"),
  transcribePresets: () => invoke<TranscribePresetView[]>("transcribe_presets"),
  openMeetingsFolder: () => invoke<void>("open_meetings_folder"),
  /** ffmpeg and yt-dlp as the system has them (versions as they say them): what a link or a file that symphonia can't read needs. */
  mediaTools: () => invoke<ToolsView>("media_tools"),
  /** The microphone and Accessibility permissions as the system has them now (macOS; elsewhere both are `not_needed`). */
  permissionsStatus: () => invoke<PermissionsView>("permissions_status"),
  /** Shows the system's prompt for a permission. It doesn't wait for the answer: the status shows it. */
  requestPermission: (kind: PermissionKind) => invoke<void>("request_permission", { kind }),
  /** Opens the pane of System Settings → Privacy & Security where the person can turn the permission on. */
  openPermissionSettings: (kind: PermissionKind) => invoke<void>("open_permission_settings", { kind }),
  /** In the system browser. The capability only allows https. */
  openUrl: (url: string) => openUrl(url),

  // The meetings window. Eight commands reject with an `ErrorView` (and their wrappers see to it that they always do): the four
  // that start a job (`meetingRerun`, `meetingResummarize`, `meetingRetry`, `meetingsImportLink`) and `meetingRenameSpeakers`,
  // `meetingAsk`, `meetingExport` and `meetingOpenMedia`. The rest reject with a plain sentence.
  /** The recordings that have every word of `query` in the title, the summary or the transcript; all of them when it is empty. */
  meetingsList: (query: string) => invoke<MeetingRow[]>("meetings_list", { query }),
  meetingDetail: (id: string) => invoke<MeetingDetail>("meeting_detail", { id }),
  /** Opens the dialog for the files and imports each: the ids of the recordings made, none when the dialog was closed. */
  meetingsPickFiles: () => invoke<string[]>("meetings_pick_files"),
  /** The ids of the recordings made of the files that were dropped. */
  meetingsImportFiles: (paths: string[]) => invoke<string[]>("meetings_import_files", { paths }),
  /** The id of the recording made. Rejects with an `ErrorView`. */
  meetingsImportLink: (url: string) => invokeView<string>("meetings_import_link", { url }),
  meetingCancel: (id: string) => invoke<void>("meeting_cancel", { id }),
  /** "Transcribe + summarize again from scratch". Rejects with an `ErrorView`. */
  meetingRerun: (id: string) => invokeView<void>("meeting_rerun", { id }),
  /** "Summarize again" ("Retry summary"). Rejects with an `ErrorView`. */
  meetingResummarize: (id: string) => invokeView<void>("meeting_resummarize", { id }),
  /** "Try again", from the step that failed. Rejects with an `ErrorView`. */
  meetingRetry: (id: string) => invokeView<void>("meeting_retry", { id }),
  /** `index` is the `index` of an `ActionView`. */
  meetingSetTask: (id: string, index: number, done: boolean) => invoke<void>("meeting_set_task", { id, index, done }),
  /** Old name to new. The renames that were done, sorted. Rejects with an `ErrorView`. */
  meetingRenameSpeakers: (id: string, names: Record<string, string>, resummarize: boolean) =>
    invokeView<[string, string][]>("meeting_rename_speakers", { id, names, resummarize }),
  /** Rejects with an `ErrorView`. */
  meetingAsk: (id: string, question: string) => invokeView<string>("meeting_ask", { id, question }),
  meetingMarkdown: (id: string) => invoke<string>("meeting_markdown", { id }),
  /** Asks where to save the Markdown and writes it there. `false` when the dialog was closed. Rejects with an `ErrorView`. */
  meetingExport: (id: string) => invokeView<boolean>("meeting_export", { id }),
  meetingLog: (id: string) => invoke<string>("meeting_log", { id }),
  meetingOpenFolder: (id: string) => invoke<void>("meeting_open_folder", { id }),
  /** In the program of the system for it. Rejects with an `ErrorView`. */
  meetingOpenMedia: (id: string) => invokeView<void>("meeting_open_media", { id }),
  /** Stops the job and moves the recording to the Trash. */
  meetingDelete: (id: string) => invoke<void>("meeting_delete", { id }),
  /** The clipboard of the system: the meetings window has no other way to it. */
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  /** Shows Settings on one of its pages. */
  meetingsOpenSettings: (tab: SettingsTab) => invoke<void>("meetings_open_settings", { tab }),
  meetingsReadiness: () => invoke<Readiness>("meetings_readiness"),

  // The two floating windows of dictation (the bubble and the card).
  /** The BCP-47 codes of the language menu, in the order it shows them. */
  dictationLanguages: () => invoke<DictationLanguages>("dictation_languages"),
  /** The Stop button, Return and (in hold mode) nothing else: stops the recording. */
  dictationStop: () => invoke<void>("dictation_stop"),
  /** The cross of the bubble and Esc in the pill: drops the dictation. */
  dictationCancel: () => invoke<void>("dictation_cancel"),
  /** Saves the language of the dictation and applies it to what is said from now on. */
  dictationSetLanguage: (lang: string) => invoke<void>("dictation_set_language", { lang }),

  // The result chip and the review card.
  /** Writes the text again in a style (a preset id): as fresh text at the caret from the chip, in the box of the card from the card. */
  dictationRewrite: (style: string) => invoke<void>("dictation_rewrite", { style }),
  /** Writes the text again as the words of a custom request say. */
  dictationCustom: (instruction: string) => invoke<void>("dictation_custom", { instruction }),
  /** Opens the review card on what was written: "Edit…" (`custom` false) and "Custom request…" (`custom` true) of the chip. */
  dictationOpenCard: (custom: boolean) => invoke<void>("dictation_open_card", { custom }),
  /** Puts the text on the clipboard. */
  dictationCopy: (text: string) => invoke<void>("dictation_copy", { text }),
  /** The card's box holds this text now, so that a style, "Copy" or "Paste into field" acts on what is in it. */
  dictationCardText: (text: string) => invoke<void>("dictation_card_text", { text }),
  /** Puts the text from before the AI edit back in the card. */
  dictationUndo: () => invoke<void>("dictation_undo"),
  /** The card opened from the chip: hides it and pastes the text in the field as fresh text. */
  dictationPasteCard: (text: string) => invoke<void>("dictation_paste_card", { text }),
  /** The mouse is over the chip or has left it: the chip stays while it is over. */
  dictationChipHover: (hovering: boolean) => invoke<void>("dictation_chip_hover", { hovering }),
  /** The menu of the chip opened or closed: the chip stays while it is open. */
  dictationMenuOpen: (open: boolean) => invoke<void>("dictation_menu_open", { open }),
  /** The system's prompt for the Accessibility permission (macOS). */
  dictationGrantAccessibility: () => invoke<void>("dictation_grant_accessibility"),
  /** The button of an error that is solved in Settings (a missing Whisper model). */
  dictationOpenSettings: () => invoke<void>("dictation_open_settings"),
  /** The state of the panels now: for a page that loads after a state was sent, which it didn't hear. */
  panelState: () => invoke<PanelState>("panel_state"),
  /** What the page now measures, in CSS pixels: the Rust side sizes the window to it and places it. `label` is the window. */
  panelResized: (label: string, width: number, height: number) =>
    invoke<void>("panel_resized", { label, width, height }),
  /** The language of the UI the settings ask for, which the panels speak. */
  panelLanguage: () => invoke<"en" | "vi">("panel_language"),

  /** Resolves with the function that stops listening. */
  onHotkeyStatus: (handler: (status: HotkeyStatus) => void) =>
    listen<HotkeyStatus>("hotkey-status", (event) => handler(event.payload)),
  /** Settings that the Rust side changed without the window (the language menu of the dictation chip): only the ones that changed. */
  onSettingsChanged: (handler: (patch: Partial<Settings>) => void) =>
    listen<Partial<Settings>>("settings-changed", (event) => handler(event.payload)),
  /** The meetings window sends someone to a page of Settings (`meetings_open_settings`): show it. Resolves with the function that stops listening. */
  onSettingsTab: (handler: (tab: SettingsTab) => void) =>
    listen<SettingsTab>("settings-tab", (event) => handler(event.payload)),
  /** A job went one step on, made progress or ended (to the meetings window only). Resolves with the function that stops listening. */
  onMeetingJob: (handler: (job: JobView) => void) => listen<JobView>("meeting-job", (event) => handler(event.payload)),
  /**
   * A recording is new, gone or renamed, or a job of it started or ended (to the meetings window only). It doesn't come while a
   * job goes on. Resolves with the function that stops listening.
   */
  onMeetingsChanged: (handler: () => void) => listen("meetings-changed", () => handler()),
  /**
   * Files dragged over this window: entering, leaving and the drop with the paths. It needs only the permission to listen to
   * events. Resolves with the function that stops listening.
   */
  onFileDrop: async (handler: (event: FileDropEvent) => void) =>
    getCurrentWebview().onDragDropEvent((event) => {
      const drag = event.payload;
      if (drag.type === "drop") handler({ type: "drop", paths: drag.paths });
      else if (drag.type === "enter") handler({ type: "enter" });
      else if (drag.type === "leave") handler({ type: "leave" });
    }),
  /** Both panel windows hear every state. Resolves with the function that stops listening. */
  onPanelState: (handler: (state: PanelState) => void) =>
    listen<PanelState>("panel-state", (event) => handler(event.payload)),
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
