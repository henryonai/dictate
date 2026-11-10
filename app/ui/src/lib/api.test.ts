import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { afterEach, describe, expect, test } from "vitest";
import { api, asErrorView } from "./api";
import { fakePanelState, fakeSettings } from "../test-helpers";

type Call = { cmd: string; args: unknown };

/** Answers every command with `reply` and records what was asked. */
function recordCalls(reply: unknown = null): Call[] {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      return reply;
    },
    { shouldMockEvents: true },
  );
  return calls;
}

afterEach(() => clearMocks());

describe("each wrapper calls its command with the argument names the Rust side expects", () => {
  const settings = fakeSettings();
  const cases: [string, string, unknown, () => Promise<unknown>][] = [
    ["getSettings", "get_settings", undefined, () => api.getSettings()],
    ["saveSettings", "save_settings", { settings }, () => api.saveSettings(settings)],
    ["setAiProvider", "set_ai_provider", { p: "openai" }, () => api.setAiProvider("openai")],
    ["setTranscribePreset", "set_transcribe_preset", { p: "groq" }, () => api.setTranscribePreset("groq")],
    ["setSecret", "set_secret", { account: "ai", value: "k" }, () => api.setSecret("ai", "k")],
    ["setHotkey", "set_hotkey", { accel: "Ctrl+Alt+D" }, () => api.setHotkey("Ctrl+Alt+D")],
    ["hotkeyStatus", "hotkey_status", undefined, () => api.hotkeyStatus()],
    ["hotkeyRecording", "hotkey_recording", { active: true }, () => api.hotkeyRecording(true)],
    ["checkAi", "check_ai", undefined, () => api.checkAi()],
    ["checkTranscribe", "check_transcribe", undefined, () => api.checkTranscribe()],
    ["listModels", "list_models", undefined, () => api.listModels()],
    ["downloadModel", "download_model", { id: "tiny" }, () => api.downloadModel("tiny")],
    ["cancelDownload", "cancel_download", { id: "tiny" }, () => api.cancelDownload("tiny")],
    ["deleteModel", "delete_model", { id: "tiny" }, () => api.deleteModel("tiny")],
    ["modelsDiskUsageMb", "models_disk_usage_mb", undefined, () => api.modelsDiskUsageMb()],
    ["editPresets", "edit_presets", undefined, () => api.editPresets()],
    ["transcribePresets", "transcribe_presets", undefined, () => api.transcribePresets()],
    ["openMeetingsFolder", "open_meetings_folder", undefined, () => api.openMeetingsFolder()],
    ["mediaTools", "media_tools", undefined, () => api.mediaTools()],
    ["dictationLanguages", "dictation_languages", undefined, () => api.dictationLanguages()],
    ["dictationStop", "dictation_stop", undefined, () => api.dictationStop()],
    ["dictationCancel", "dictation_cancel", undefined, () => api.dictationCancel()],
    ["dictationSetLanguage", "dictation_set_language", { lang: "vi-VN" }, () => api.dictationSetLanguage("vi-VN")],
    ["dictationRewrite", "dictation_rewrite", { style: "concise" }, () => api.dictationRewrite("concise")],
    ["dictationCustom", "dictation_custom", { instruction: "polite" }, () => api.dictationCustom("polite")],
    ["dictationOpenCard", "dictation_open_card", { custom: true }, () => api.dictationOpenCard(true)],
    ["dictationCopy", "dictation_copy", { text: "Hello" }, () => api.dictationCopy("Hello")],
    ["dictationCardText", "dictation_card_text", { text: "Hello" }, () => api.dictationCardText("Hello")],
    ["dictationUndo", "dictation_undo", undefined, () => api.dictationUndo()],
    ["dictationPasteCard", "dictation_paste_card", { text: "Hello" }, () => api.dictationPasteCard("Hello")],
    ["dictationChipHover", "dictation_chip_hover", { hovering: true }, () => api.dictationChipHover(true)],
    ["dictationMenuOpen", "dictation_menu_open", { open: false }, () => api.dictationMenuOpen(false)],
    ["dictationGrantAccessibility", "dictation_grant_accessibility", undefined, () => api.dictationGrantAccessibility()],
    ["dictationOpenSettings", "dictation_open_settings", undefined, () => api.dictationOpenSettings()],
    ["panelState", "panel_state", undefined, () => api.panelState()],
    ["panelResized", "panel_resized", { label: "bubble", width: 428, height: 100 }, () => api.panelResized("bubble", 428, 100)],
    ["panelLanguage", "panel_language", undefined, () => api.panelLanguage()],
    ["meetingsList", "meetings_list", { query: "plan" }, () => api.meetingsList("plan")],
    ["meetingDetail", "meeting_detail", { id: "2026-03-02 10-00-00" }, () => api.meetingDetail("2026-03-02 10-00-00")],
    ["meetingsPickFiles", "meetings_pick_files", undefined, () => api.meetingsPickFiles()],
    ["meetingsImportFiles", "meetings_import_files", { paths: ["/a/b.mp4"] }, () => api.meetingsImportFiles(["/a/b.mp4"])],
    ["meetingsImportLink", "meetings_import_link", { url: "https://youtu.be/x" }, () => api.meetingsImportLink("https://youtu.be/x")],
    ["meetingCancel", "meeting_cancel", { id: "m1" }, () => api.meetingCancel("m1")],
    ["meetingRerun", "meeting_rerun", { id: "m1" }, () => api.meetingRerun("m1")],
    ["meetingResummarize", "meeting_resummarize", { id: "m1" }, () => api.meetingResummarize("m1")],
    ["meetingRetry", "meeting_retry", { id: "m1" }, () => api.meetingRetry("m1")],
    ["meetingSetTask", "meeting_set_task", { id: "m1", index: 2, done: true }, () => api.meetingSetTask("m1", 2, true)],
    [
      "meetingRenameSpeakers",
      "meeting_rename_speakers",
      { id: "m1", names: { "Speaker 1": "Lan" }, resummarize: false },
      () => api.meetingRenameSpeakers("m1", { "Speaker 1": "Lan" }, false),
    ],
    ["meetingAsk", "meeting_ask", { id: "m1", question: "Who?" }, () => api.meetingAsk("m1", "Who?")],
    ["meetingMarkdown", "meeting_markdown", { id: "m1" }, () => api.meetingMarkdown("m1")],
    ["meetingExport", "meeting_export", { id: "m1" }, () => api.meetingExport("m1")],
    ["meetingLog", "meeting_log", { id: "m1" }, () => api.meetingLog("m1")],
    ["meetingOpenFolder", "meeting_open_folder", { id: "m1" }, () => api.meetingOpenFolder("m1")],
    ["meetingOpenMedia", "meeting_open_media", { id: "m1" }, () => api.meetingOpenMedia("m1")],
    ["meetingDelete", "meeting_delete", { id: "m1" }, () => api.meetingDelete("m1")],
    ["copyText", "copy_text", { text: "brew install yt-dlp" }, () => api.copyText("brew install yt-dlp")],
    ["meetingsOpenSettings", "meetings_open_settings", { tab: "accounts" }, () => api.meetingsOpenSettings("accounts")],
    ["meetingsReadiness", "meetings_readiness", undefined, () => api.meetingsReadiness()],
    ["openUrl", "plugin:opener|open_url", { url: "https://example.com/keys" }, () => api.openUrl("https://example.com/keys")],
  ];

  test.each(cases)("%s", async (_name, cmd, args, run) => {
    const calls = recordCalls();
    await run();
    expect(calls).toHaveLength(1);
    expect(calls[0].cmd).toBe(cmd);
    if (args === undefined) {
      expect(calls[0].args ?? {}).toEqual({});
    } else {
      expect(calls[0].args).toMatchObject(args as object);
    }
  });
});

describe("the eight commands that reject with an ErrorView always do, whatever the failure was", () => {
  const cases: [string, () => Promise<unknown>][] = [
    ["meetingsImportLink", () => api.meetingsImportLink("https://youtu.be/x")],
    ["meetingRerun", () => api.meetingRerun("m1")],
    ["meetingResummarize", () => api.meetingResummarize("m1")],
    ["meetingRetry", () => api.meetingRetry("m1")],
    ["meetingRenameSpeakers", () => api.meetingRenameSpeakers("m1", { "Speaker 1": "Lan" }, false)],
    ["meetingAsk", () => api.meetingAsk("m1", "Who?")],
    ["meetingExport", () => api.meetingExport("m1")],
    ["meetingOpenMedia", () => api.meetingOpenMedia("m1")],
  ];

  test.each(cases)("%s rejects with the view the Rust side sent, as it is", async (_name, run) => {
    const view = { kind: "Other", key: "meetings.error.busy", detail: null, command: null };
    mockIPC(() => {
      throw view;
    });
    await expect(run()).rejects.toEqual(view);
  });

  test.each(cases)("%s makes a view of a sentence, as when the IPC itself fails", async (_name, run) => {
    mockIPC(() => {
      throw "The recording was not found.";
    });
    await expect(run()).rejects.toEqual({
      kind: "Other",
      key: "meetings.error.other",
      detail: "The recording was not found.",
      command: null,
    });
  });

  test("the other commands of the meetings window reject with the sentence, as it is", async () => {
    mockIPC(() => {
      throw "The recording was not found.";
    });
    await expect(api.meetingDetail("m1")).rejects.toBe("The recording was not found.");
    await expect(api.meetingSetTask("m1", 0, true)).rejects.toBe("The recording was not found.");
  });
});

describe("asErrorView", () => {
  test("keeps what the Rust side sent as an error", () => {
    const view = { kind: "Api", key: "meetings.error.no_ai", detail: null, command: null } as const;
    expect(asErrorView(view)).toBe(view);
  });

  test("makes an error of the sentence a command failed with, or of an Error", () => {
    expect(asErrorView("The recording was not found.")).toEqual({
      kind: "Other",
      key: "meetings.error.other",
      detail: "The recording was not found.",
      command: null,
    });
    expect(asErrorView(new Error("IPC is down")).detail).toBe("IPC is down");
  });

  test("an object that is not an error of the Rust side, or nothing, is an error of no kind in particular", () => {
    expect(asErrorView({ message: "x" })).toMatchObject({ key: "meetings.error.other", detail: "x" });
    expect(asErrorView(null)).toMatchObject({ key: "meetings.error.other", detail: "" });
  });
});

test("a failed command rejects with the text the Rust side sent", async () => {
  mockIPC(() => {
    throw "API error 401: bad key";
  });
  await expect(api.checkAi()).rejects.toBe("API error 401: bad key");
});

test("the model-download event reaches the listener, until it is removed", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onModelDownload((event) => heard.push(event));

  const event = { id: "tiny", done: 10, total: 75, state: "running" };
  await emit("model-download", event);
  expect(heard).toEqual([event]);

  stop();
  await emit("model-download", event);
  expect(heard).toHaveLength(1);
});

test("the hotkey-status event reaches the listener", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onHotkeyStatus((status) => heard.push(status));

  const status = { label: "Ctrl+Alt+D", backend: "portal", error: null };
  await emit("hotkey-status", status);
  expect(heard).toEqual([status]);

  stop();
});

test("the settings-changed event brings the settings that changed, until it is removed", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onSettingsChanged((patch) => heard.push(patch));

  await emit("settings-changed", { dictation_language: "vi-VN" });
  expect(heard).toEqual([{ dictation_language: "vi-VN" }]);

  stop();
  await emit("settings-changed", { dictation_language: "en-US" });
  expect(heard).toHaveLength(1);
});

test("the settings-tab event brings the page that Settings is asked to show, until it is removed", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onSettingsTab((tab) => heard.push(tab));

  await emit("settings-tab", "accounts");
  expect(heard).toEqual(["accounts"]);

  stop();
  await emit("settings-tab", "speech");
  expect(heard).toHaveLength(1);
});

test("the panel-state event reaches the listener, until it is removed", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onPanelState((state) => heard.push(state));

  const state = fakePanelState({ view: "pill", durationS: 12 });
  await emit("panel-state", state);
  expect(heard).toEqual([state]);

  stop();
  await emit("panel-state", state);
  expect(heard).toHaveLength(1);
});

test("the meeting-job event brings the job as the Rust side sends it, until it is removed", async () => {
  recordCalls();
  const heard: unknown[] = [];
  const stop = await api.onMeetingJob((job) => heard.push(job));

  const job = { id: "m1", step: "transcript", progress: 0.4, stepStartedMs: 1_700_000_000_000, outcome: null };
  await emit("meeting-job", job);
  expect(heard).toEqual([job]);

  stop();
  await emit("meeting-job", job);
  expect(heard).toHaveLength(1);
});

test("the meetings-changed event tells that the list changed, until it is removed", async () => {
  recordCalls();
  let heard = 0;
  const stop = await api.onMeetingsChanged(() => (heard += 1));

  await emit("meetings-changed");
  expect(heard).toBe(1);

  stop();
  await emit("meetings-changed");
  expect(heard).toBe(1);
});

test("the meetings-hidden event tells that the window was closed, until it is removed", async () => {
  recordCalls();
  let heard = 0;
  const stop = await api.onMeetingsHidden(() => (heard += 1));

  await emit("meetings-hidden");
  expect(heard).toBe(1);

  stop();
  await emit("meetings-hidden");
  expect(heard).toBe(1);
});

describe("the files dropped on the window", () => {
  const position = { x: 10, y: 20 };

  test("a drop brings the paths, and entering and leaving say when the window is a target", async () => {
    mockWindows("meetings");
    recordCalls();
    const heard: unknown[] = [];
    const stop = await api.onFileDrop((event) => heard.push(event));

    await emit("tauri://drag-enter", { paths: ["/a/talk.mp4"], position });
    await emit("tauri://drag-drop", { paths: ["/a/talk.mp4", "/a/b.m4a"], position });
    await emit("tauri://drag-leave");
    expect(heard).toEqual([
      { type: "enter" },
      { type: "drop", paths: ["/a/talk.mp4", "/a/b.m4a"] },
      { type: "leave" },
    ]);

    stop();
    await emit("tauri://drag-drop", { paths: ["/a/c.mp4"], position });
    expect(heard).toHaveLength(3);
  });
});
