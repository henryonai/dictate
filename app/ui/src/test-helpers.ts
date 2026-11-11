// Shared by the tests only: settings as the Rust side sends them, and the api with every call stubbed.
import { vi } from "vitest";
import { api } from "./lib/api";
import type {
  DownloadEvent,
  ErrorView,
  FileDropEvent,
  HotkeyStatus,
  JobView,
  MediaView,
  MeetingDetail,
  MeetingRow,
  OpenRecorder,
  Readiness,
  RecordingView,
  Settings,
  SettingsTab,
  SettingsView,
  SourcesView,
  SummaryView,
} from "./lib/api";
import { IDLE_RECORDING } from "./lib/recording";
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
  vi.spyOn(api, "mediaTools").mockResolvedValue({
    ffmpeg: null,
    ytDlp: null,
    ffmpegCommand: "brew install ffmpeg",
    ytDlpCommand: "brew install yt-dlp ffmpeg",
  });
  // Not a Mac: no permissions, so no section.
  vi.spyOn(api, "permissionsStatus").mockResolvedValue({
    microphone: "not_needed",
    accessibility: "not_needed",
    screen: "not_needed",
  });
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

// ---- The meetings window

/** An error as the Rust side sends it: the one of a failed summary, unless a test says otherwise. */
export function fakeError(overrides: Partial<ErrorView> = {}): ErrorView {
  return { kind: "Other", key: "meetings.error.other", detail: "Something broke.", command: null, ...overrides };
}

/** A job that is on its transcript, with no figure yet. */
export function fakeJob(overrides: Partial<JobView> = {}): JobView {
  return { id: "2026-03-02 10-00-00", step: "transcript", progress: null, stepStartedMs: 0, outcome: null, ...overrides };
}

/** A recording that is done and summarized: a file of 12 minutes with two action items open. */
export function fakeRow(overrides: Partial<MeetingRow> = {}): MeetingRow {
  return {
    id: "2026-03-02 10-00-00",
    title: "Weekly planning",
    kind: "file",
    created: new Date(2026, 2, 2, 10, 0, 0).getTime() / 1000,
    duration: 720,
    hasSummary: true,
    openTasks: 2,
    live: null,
    error: null,
    ...overrides,
  };
}

/** The detail of a recording: a transcript of two lines and no summary, which a test fills in as it needs. */
export function fakeDetail(overrides: Partial<MeetingDetail> = {}): MeetingDetail {
  return {
    row: fakeRow(),
    source: "/home/lan/talks/planning.mp4",
    sourceLabel: "planning.mp4",
    transcript: [
      { start: 0, stamp: "00:00", speaker: "Speaker 1", text: "Let's start." },
      { start: 65, stamp: "01:05", speaker: "Speaker 2", text: "First, the budget." },
    ],
    transcriptIsLive: false,
    speakers: [],
    summary: null,
    media: null,
    mediaMissing: false,
    ...overrides,
  };
}

/** The summary a model wrote, in the shape the window gets it: a summary, a point, a decision, two action items, a question and two chapters. */
export function fakeSummary(overrides: Partial<SummaryView> = {}): SummaryView {
  return {
    markdown: "# Weekly planning\n\n## Summary\nWe planned the quarter.",
    title: "Weekly planning",
    summary: "We planned the quarter.",
    keyPoints: "- Budget is **fixed**\n- See [01:05] for the budget",
    decisions: ["Ship in May"],
    actions: [
      { index: 0, owner: "An", text: "Send the budget", done: false },
      { index: 1, owner: null, text: "Book the room", done: true },
    ],
    questions: ["Who owns the docs?"],
    timeline: [
      { seconds: 0, label: "00:00", text: "Kick-off" },
      { seconds: 65, label: "01:05", text: "Budget" },
    ],
    other: [],
    ...overrides,
  };
}

/** Where a recording plays from: the address of the local media server, which differs from one launch to the next. */
export function fakeMedia(overrides: Partial<MediaView> = {}): MediaView {
  return { url: "http://127.0.0.1:41000/token/1", video: true, fallbackUrl: null, ...overrides };
}

/**
 * A media element, as far as the player's logic looks at one: a position, whether it is paused, an address that resets the position
 * and pauses it when it is set (as the load of a new address does), the three calls and the events. A test sends the events itself.
 */
export class FakeMedia extends EventTarget {
  paused = true;
  ended = false;
  currentTime = 0;
  private attributes = new Map<string, string>();
  /** Every address that was set, in order. */
  readonly sources: string[] = [];
  /** The calls that were made, in order: "play", "pause", "load". */
  readonly calls: string[] = [];

  get src(): string {
    return this.attributes.get("src") ?? "";
  }

  set src(address: string) {
    this.attributes.set("src", address);
    this.sources.push(address);
    // The load of an address starts at 0 and, as the load algorithm says, leaves the element paused.
    this.currentTime = 0;
    this.paused = true;
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  removeAttribute(name: string): void {
    this.calls.push(`removeAttribute(${name})`);
    this.attributes.delete(name);
  }

  play = vi.fn(async () => {
    this.calls.push("play");
    this.paused = false;
    this.emit("play");
  });

  pause = vi.fn(() => {
    this.calls.push("pause");
    if (this.paused) return;
    this.paused = true;
    this.emit("pause");
  });

  load = vi.fn(() => {
    this.calls.push("load");
  });

  emit(type: string): void {
    this.dispatchEvent(new Event(type));
  }

  /** The element is playing, at this position. */
  playAt(seconds: number): void {
    this.currentTime = seconds;
    this.paused = false;
  }
}

/** A recording as the Rust side reports it while it goes on: both tracks on and quiet, and no notes. */
export function fakeRecording(overrides: Partial<RecordingView> = {}): RecordingView {
  return {
    phase: "recording",
    elapsedMs: 0,
    levels: { mic: 0, system: 0 },
    mode: { kind: "screen", display: null },
    meetingId: "2026-03-02 11-00-00",
    notes: [],
    ...overrides,
  };
}

export { IDLE_RECORDING };

/** What the picker is offered on a desktop that can record the screen: one display, two windows, and the choices of a first start. */
export function fakeSources(overrides: Partial<SourcesView> = {}): SourcesView {
  return {
    displays: [{ id: "1", name: "Display 1 (2560x1440)", app: null }],
    windows: [
      { id: "42", name: "Safari — Docs", app: "Safari" },
      { id: "77", name: "Zoom — Weekly sync", app: "Zoom" },
    ],
    pickerIsTheSystem: false,
    screen: "ready",
    ffmpegCommand: "brew install ffmpeg",
    last: { mode: { kind: "screen", display: null }, quality: "standard", mic: true, system_audio: true },
    ...overrides,
  };
}

/**
 * Stubs the calls of the recording, which the picker of the meetings window and the recording bar share: nothing is recorded, and
 * `sources` is what there is to record. Returns a way to send the events (the state of the recording, and the ask for the picker).
 */
export function stubRecordingApi(sources: SourcesView = fakeSources()) {
  const stateListeners = new Set<(view: RecordingView) => void>();
  const openListeners = new Set<(open: OpenRecorder) => void>();
  const listener = <T>(set: Set<T>) => async (handler: T) => {
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  };

  vi.spyOn(api, "recordingSources").mockResolvedValue(sources);
  vi.spyOn(api, "recordingStart").mockResolvedValue();
  vi.spyOn(api, "recordingPause").mockResolvedValue();
  vi.spyOn(api, "recordingResume").mockResolvedValue();
  vi.spyOn(api, "recordingStop").mockResolvedValue();
  vi.spyOn(api, "recordingDiscard").mockResolvedValue();
  vi.spyOn(api, "recordingState").mockResolvedValue(IDLE_RECORDING);
  vi.spyOn(api, "onRecordingState").mockImplementation(listener(stateListeners));
  vi.spyOn(api, "onOpenRecorder").mockImplementation(listener(openListeners));

  return {
    /** The recorder reports how the recording goes. */
    sendRecordingState: (view: RecordingView) => stateListeners.forEach((handler) => handler(view)),
    /** The tray, the shortcut or `--record` ask for the picker, to a window that is there. */
    sendOpenRecorder: (open: OpenRecorder = { error: null }) => openListeners.forEach((handler) => handler(open)),
    /** How many listeners of the state of the recording, and of the ask for the picker, are on now. */
    recordingListeners: () => ({ state: stateListeners.size, open: openListeners.size }),
  };
}

/**
 * Stubs every call of the meetings window with a calm answer: the library holds `rows`, and the detail of a row is
 * `fakeDetail` of it. A test changes the calls it cares about with `vi.mocked(api.meetingRetry).mockRejectedValue(...)`.
 * Returns a way to send the events of the window.
 */
export function stubMeetingsApi(rows: MeetingRow[] = []) {
  const jobListeners = new Set<(job: JobView) => void>();
  const changedListeners = new Set<() => void>();
  const dropListeners = new Set<(event: FileDropEvent) => void>();
  const hiddenListeners = new Set<() => void>();
  const listener = <T>(set: Set<T>) => async (handler: T) => {
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  };

  vi.spyOn(api, "meetingsList").mockResolvedValue(rows);
  vi.spyOn(api, "meetingDetail").mockImplementation(async (id) =>
    fakeDetail({ row: rows.find((row) => row.id === id) ?? fakeRow({ id }) }),
  );
  vi.spyOn(api, "meetingsPickFiles").mockResolvedValue([]);
  vi.spyOn(api, "meetingsImportFiles").mockResolvedValue([]);
  vi.spyOn(api, "meetingsImportLink").mockResolvedValue("2026-03-02 11-00-00");
  vi.spyOn(api, "meetingCancel").mockResolvedValue();
  vi.spyOn(api, "meetingRerun").mockResolvedValue();
  vi.spyOn(api, "meetingResummarize").mockResolvedValue();
  vi.spyOn(api, "meetingRetry").mockResolvedValue();
  vi.spyOn(api, "meetingSetTask").mockResolvedValue();
  vi.spyOn(api, "meetingRenameSpeakers").mockResolvedValue([]);
  vi.spyOn(api, "meetingAsk").mockResolvedValue("An answer.");
  vi.spyOn(api, "meetingMarkdown").mockResolvedValue("# Weekly planning");
  vi.spyOn(api, "meetingExport").mockResolvedValue(true);
  vi.spyOn(api, "meetingLog").mockResolvedValue("");
  vi.spyOn(api, "meetingOpenFolder").mockResolvedValue();
  vi.spyOn(api, "meetingOpenMedia").mockResolvedValue();
  vi.spyOn(api, "meetingDelete").mockResolvedValue();
  vi.spyOn(api, "copyText").mockResolvedValue();
  vi.spyOn(api, "meetingsOpenSettings").mockResolvedValue();
  vi.spyOn(api, "meetingsReadiness").mockResolvedValue({ ai: true, whisper: true, language: "en" });
  vi.spyOn(api, "onMeetingJob").mockImplementation(listener(jobListeners));
  vi.spyOn(api, "onMeetingsChanged").mockImplementation(listener(changedListeners));
  vi.spyOn(api, "onFileDrop").mockImplementation(listener(dropListeners));
  vi.spyOn(api, "onMeetingsHidden").mockImplementation(listener(hiddenListeners));
  const recording = stubRecordingApi();

  return {
    ...recording,
    emitJob: (job: JobView) => jobListeners.forEach((handler) => handler(job)),
    emitChanged: () => changedListeners.forEach((handler) => handler()),
    emitDrop: (event: FileDropEvent) => dropListeners.forEach((handler) => handler(event)),
    /** The window was closed (hidden). */
    emitHidden: () => hiddenListeners.forEach((handler) => handler()),
    /** How many listeners of the window being closed are on now (it is not in `listening`: only the detail listens to it). */
    hiddenListeners: () => hiddenListeners.size,
    /** How many listeners of each event are on now. */
    listening: () => ({ jobs: jobListeners.size, changed: changedListeners.size, drops: dropListeners.size }),
    setReadiness: (readiness: Readiness) => vi.mocked(api.meetingsReadiness).mockResolvedValue(readiness),
  };
}
