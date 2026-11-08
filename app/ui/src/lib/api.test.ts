import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { afterEach, describe, expect, test } from "vitest";
import { api } from "./api";
import { fakeSettings } from "../test-helpers";

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
    ["checkAi", "check_ai", undefined, () => api.checkAi()],
    ["checkTranscribe", "check_transcribe", undefined, () => api.checkTranscribe()],
    ["listModels", "list_models", undefined, () => api.listModels()],
    ["downloadModel", "download_model", { id: "tiny" }, () => api.downloadModel("tiny")],
    ["cancelDownload", "cancel_download", { id: "tiny" }, () => api.cancelDownload("tiny")],
    ["deleteModel", "delete_model", { id: "tiny" }, () => api.deleteModel("tiny")],
    ["modelsDiskUsageMb", "models_disk_usage_mb", undefined, () => api.modelsDiskUsageMb()],
    ["transcribeWav", "transcribe_wav", { path: "a.wav" }, () => api.transcribeWav("a.wav")],
    ["cancelTranscribe", "cancel_transcribe", undefined, () => api.cancelTranscribe()],
    ["editPresets", "edit_presets", undefined, () => api.editPresets()],
    ["transcribePresets", "transcribe_presets", undefined, () => api.transcribePresets()],
    ["openMeetingsFolder", "open_meetings_folder", undefined, () => api.openMeetingsFolder()],
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
