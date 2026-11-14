import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { api } from "./api";
import type { SettingsView } from "./api";
import { currentLanguage, setLanguage } from "./i18n";
import { SettingsStore } from "./settings-store.svelte";
import { fakeHotkeyStatus, fakeSettings, fakeView, stubApi } from "../test-helpers";

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

/** A promise that is settled by the test, to make a call take as long as the test wants. */
function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function loaded(view = fakeView()): Promise<SettingsStore> {
  vi.mocked(api.getSettings).mockResolvedValue(view);
  const store = new SettingsStore();
  await store.load();
  return store;
}

describe("loading", () => {
  test("keeps the view", async () => {
    const store = await loaded(fakeView({ hotkey: "F9" }));
    expect(store.view?.hotkey).toBe("F9");
  });

  test("keeps the load warning after later views that carry none", async () => {
    const store = await loaded(fakeView({ load_warning: "The settings file was reset to defaults." }));
    expect(store.loadWarning).toBe("The settings file was reset to defaults.");

    await store.update({ auto_refine: false });
    await store.setSecret("ai", "key");

    expect(store.view?.load_warning).toBeNull();
    expect(store.loadWarning).toBe("The settings file was reset to defaults.");
  });

  test("lets the warning go when the person dismisses it", async () => {
    const store = await loaded(fakeView({ load_warning: "reset" }));
    store.dismissWarning();
    expect(store.loadWarning).toBeNull();
  });

  test("tells why the settings could not be read", async () => {
    vi.mocked(api.getSettings).mockRejectedValue("The settings are broken");
    const store = new SettingsStore();
    await store.load();
    expect(store.view).toBeNull();
    expect(store.error).toBe("The settings are broken");
  });
});

describe("saving a change", () => {
  test("sends all the settings and none of what the view adds", async () => {
    const store = await loaded(fakeView({ has_ai_key: true, load_warning: "reset" }));
    await store.update({ push_to_talk: true });

    expect(api.saveSettings).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.saveSettings).mock.calls[0][0]).toEqual(fakeSettings({ push_to_talk: true }));
  });

  test("shows the change at once and takes what the Rust side answers", async () => {
    const store = await loaded();
    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValue(answer.promise);

    const saving = store.update({ hotkey: "F9" });
    expect(store.view?.hotkey).toBe("F9");

    answer.resolve(fakeView({ hotkey: "F9", launch_at_login: true }));
    await saving;
    expect(store.view?.launch_at_login).toBe(true);
  });

  test("puts back what is saved when saving fails, and says why", async () => {
    const store = await loaded();
    vi.mocked(api.saveSettings).mockRejectedValue("Could not change launch at login: denied");

    await store.update({ launch_at_login: true });

    expect(store.view?.launch_at_login).toBe(false);
    expect(store.error).toBe("Could not change launch at login: denied");
  });

  test("sends one save after the other", async () => {
    const store = await loaded();
    const first = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValueOnce(first.promise);

    const a = store.update({ push_to_talk: true });
    const b = store.update({ auto_write: false });
    await Promise.resolve();
    expect(api.saveSettings).toHaveBeenCalledTimes(1);

    first.resolve(fakeView({ push_to_talk: true }));
    await Promise.all([a, b]);
    expect(api.saveSettings).toHaveBeenCalledTimes(2);
    expect(vi.mocked(api.saveSettings).mock.calls[1][0]).toMatchObject({ push_to_talk: true, auto_write: false });
    expect(store.view).toMatchObject({ push_to_talk: true, auto_write: false });
  });

  test("lets the error go when the person dismisses it", async () => {
    const store = await loaded();
    vi.mocked(api.saveSettings).mockRejectedValue("disk full");
    await store.update({ diarize: false });

    store.dismissError();

    expect(store.error).toBe("");
  });

  test("clears an old error once a save works", async () => {
    const store = await loaded();
    vi.mocked(api.saveSettings).mockRejectedValueOnce("nope");
    await store.update({ diarize: false });
    expect(store.error).toBe("nope");

    await store.update({ diarize: false });
    expect(store.error).toBe("");
  });

  test("settled waits for the saves that are on their way", async () => {
    const store = await loaded();
    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValue(answer.promise);
    void store.update({ diarize: false });

    let done = false;
    void store.settled().then(() => (done = true));
    await Promise.resolve();
    expect(done).toBe(false);

    answer.resolve(fakeView({ diarize: false }));
    await store.settled();
    expect(done).toBe(true);
  });
});

describe("the changes that need the Rust side's rules", () => {
  test("a new AI provider comes back with the base URL and the model that go with it", async () => {
    const store = await loaded();
    vi.mocked(api.setAiProvider).mockResolvedValue(
      fakeView({ ai_provider: "anthropic", ai_base_url: "https://api.anthropic.com", ai_model: "claude-haiku-5-5" }),
    );

    await store.setAiProvider("anthropic");

    expect(api.setAiProvider).toHaveBeenCalledWith("anthropic");
    expect(store.view).toMatchObject({
      ai_provider: "anthropic",
      ai_base_url: "https://api.anthropic.com",
      ai_model: "claude-haiku-5-5",
    });
  });

  test("a new transcription service comes back with its base URL and models", async () => {
    const store = await loaded();
    vi.mocked(api.setTranscribePreset).mockResolvedValue(
      fakeView({ transcribe_preset: "groq", transcribe_base_url: "https://api.groq.com/openai/v1", transcribe_model: "whisper-large-v3-turbo" }),
    );

    await store.setTranscribePreset("groq");

    expect(store.view).toMatchObject({ transcribe_preset: "groq", transcribe_model: "whisper-large-v3-turbo" });
  });

  test("saving a key shows that one is saved, never what it is", async () => {
    const store = await loaded();
    vi.mocked(api.setSecret).mockResolvedValue(fakeView({ has_ai_key: true }));

    await store.setSecret("ai", "sk-secret-1");

    expect(api.setSecret).toHaveBeenCalledWith("ai", "sk-secret-1");
    expect(store.view?.has_ai_key).toBe(true);
    expect(JSON.stringify(store.view)).not.toContain("sk-secret-1");
  });

  test("a key that could not be saved says why", async () => {
    const store = await loaded();
    vi.mocked(api.setSecret).mockRejectedValue("The system's key store isn't available");

    await store.setSecret("ai", "sk-secret-1");

    expect(store.error).toBe("The system's key store isn't available");
    expect(store.view?.has_ai_key).toBe(false);
  });
});

describe("the language of the window", () => {
  test("follows the interface language of the settings", async () => {
    const store = await loaded(fakeView({ ui_language: "vi" }));
    expect(currentLanguage()).toBe("vi");

    await store.update({ ui_language: "en" });
    expect(currentLanguage()).toBe("en");
  });

  test("system means what the Rust side resolved it to, the same as in the tray, not what the web view says", async () => {
    // Windows: the web view may have another language than the one the system's own menus are in.
    vi.spyOn(navigator, "language", "get").mockReturnValue("en-US");
    const store = await loaded(fakeView({ ui_language: "system", system_language: "vi" }));
    expect(currentLanguage()).toBe("vi");
    // The Rust side tells the language of the system with every answer.
    vi.mocked(api.saveSettings).mockImplementation(async (settings) =>
      fakeView({ ...settings, system_language: "vi" }),
    );

    await store.update({ ui_language: "en" });
    expect(currentLanguage()).toBe("en");
    // Back to system: shows at once, before the answer, in the language of the system.
    const answer = later<SettingsView>();
    vi.mocked(api.saveSettings).mockReturnValue(answer.promise);
    const saving = store.update({ ui_language: "system" });
    expect(currentLanguage()).toBe("vi");
    answer.resolve(fakeView({ ui_language: "system", system_language: "vi" }));
    await saving;
    expect(currentLanguage()).toBe("vi");
  });

  test("an English system stays English in a Vietnamese web view", async () => {
    vi.spyOn(navigator, "language", "get").mockReturnValue("vi-VN");
    await loaded(fakeView({ ui_language: "system", system_language: "en" }));
    expect(currentLanguage()).toBe("en");
  });

  test("the language of the system is not sent back with the settings", async () => {
    const store = await loaded(fakeView({ system_language: "vi" }));
    await store.update({ vocabulary: "Dictate" });
    expect(vi.mocked(api.saveSettings).mock.lastCall?.[0]).not.toHaveProperty("system_language");
  });

  test("goes back with the setting when the save fails", async () => {
    const store = await loaded(fakeView({ ui_language: "vi" }));
    vi.mocked(api.saveSettings).mockRejectedValue("disk full");

    await store.update({ ui_language: "en" });

    expect(currentLanguage()).toBe("vi");
  });
});

describe("a change made while a call is on its way", () => {
  /** Starts a save and waits until it is really on its way, held by the test. */
  async function saveOnItsWay(store: SettingsStore, patch: Parameters<SettingsStore["update"]>[0]) {
    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValueOnce(answer.promise);
    const done = store.update(patch);
    await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    return { answer, done };
  }

  test("is kept on screen when the earlier save answers, and is saved after it", async () => {
    const store = await loaded();
    const first = await saveOnItsWay(store, { push_to_talk: true });

    const second = store.update({ auto_write: false });
    first.answer.resolve(fakeView({ push_to_talk: true }));
    await first.done;

    // The answer to the first save doesn't know the second change: it must not wipe it out.
    expect(store.view).toMatchObject({ push_to_talk: true, auto_write: false });

    await second;
    expect(api.saveSettings).toHaveBeenCalledTimes(2);
    expect(vi.mocked(api.saveSettings).mock.calls[1][0]).toMatchObject({ push_to_talk: true, auto_write: false });
    expect(store.view).toMatchObject({ push_to_talk: true, auto_write: false });
  });

  test("is kept, and saved, when the earlier save fails: only the failed change goes back", async () => {
    const store = await loaded();
    const first = await saveOnItsWay(store, { launch_at_login: true });

    const second = store.update({ auto_write: false });
    first.answer.reject("Could not change launch at login: denied");
    await first.done;

    expect(store.view).toMatchObject({ launch_at_login: false, auto_write: false });

    await second;
    expect(vi.mocked(api.saveSettings).mock.calls[1][0]).toMatchObject({ launch_at_login: false, auto_write: false });
    expect(store.view).toMatchObject({ launch_at_login: false, auto_write: false });
  });

  test("the last change to a setting wins, also when it is changed twice during a save", async () => {
    const store = await loaded();
    const first = await saveOnItsWay(store, { hotkey: "F1" });

    void store.update({ hotkey: "F2" });
    const last = store.update({ hotkey: "F3" });
    first.answer.resolve(fakeView({ hotkey: "F1" }));
    await first.done;
    expect(store.view?.hotkey).toBe("F3");

    await last;
    expect(vi.mocked(api.saveSettings).mock.lastCall?.[0].hotkey).toBe("F3");
    expect(store.view?.hotkey).toBe("F3");
  });

  test("the error of the failed save is not wiped by the save that was waiting behind it", async () => {
    const store = await loaded();
    const first = await saveOnItsWay(store, { launch_at_login: true });

    const second = store.update({ auto_write: false });
    first.answer.reject("denied");
    await Promise.all([first.done, second]);

    expect(store.error).toBe("denied");
  });

  test("an edit made during a provider switch is saved with the base URL the switch brought", async () => {
    const store = await loaded();
    const switching = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.setAiProvider).mockReturnValue(switching.promise);
    const switched = store.setAiProvider("anthropic");

    const edit = store.update({ summary_model: "claude-sonnet-5" });
    switching.resolve(fakeView({ ai_provider: "anthropic", ai_base_url: "https://api.anthropic.com" }));
    await Promise.all([switched, edit]);

    expect(vi.mocked(api.saveSettings).mock.calls[0][0]).toMatchObject({
      ai_provider: "anthropic",
      ai_base_url: "https://api.anthropic.com",
      summary_model: "claude-sonnet-5",
    });
  });
});

describe("what a call sends", () => {
  test("an edit sent before a provider switch doesn't carry the switch, which then moves the base URL", async () => {
    const store = await loaded();
    const slow = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValueOnce(slow.promise);
    const inFlight = store.update({ hotkey: "F1" });
    await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));

    // Both are made while the slow call is out: the edit first, the switch after it.
    const edit = store.update({ summary_model: "claude-sonnet-5" });
    vi.mocked(api.setAiProvider).mockResolvedValue(
      fakeView({ hotkey: "F1", summary_model: "claude-sonnet-5", ai_provider: "anthropic", ai_base_url: "https://api.anthropic.com" }),
    );
    const switched = store.setAiProvider("anthropic");
    expect(store.view?.ai_provider).toBe("anthropic");

    slow.resolve(fakeView({ hotkey: "F1" }));
    await Promise.all([inFlight, edit, switched]);

    // The edit's call knows the provider the Rust side has, so that the switch that follows is a real change there.
    expect(api.saveSettings).toHaveBeenCalledTimes(2);
    expect(vi.mocked(api.saveSettings).mock.calls[1][0]).toMatchObject({
      hotkey: "F1",
      summary_model: "claude-sonnet-5",
      ai_provider: "openai",
      ai_base_url: "https://openrouter.ai/api/v1",
    });
    expect(store.view).toMatchObject({ ai_provider: "anthropic", ai_base_url: "https://api.anthropic.com" });
  });
});

describe("the first load", () => {
  test("says it is loading until it ends, whether it works or not", async () => {
    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.getSettings).mockReturnValue(answer.promise);
    const store = new SettingsStore();
    const loading = store.load();
    expect(store.loading).toBe(true);

    answer.reject("broken");
    await loading;
    expect(store.loading).toBe(false);
  });

  test("can be tried again after a failure, and the old error goes while it runs", async () => {
    vi.mocked(api.getSettings).mockRejectedValueOnce("The settings folder is locked");
    const store = new SettingsStore();
    await store.load();
    expect(store.error).toBe("The settings folder is locked");

    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.getSettings).mockReturnValue(answer.promise);
    const again = store.load();
    expect(store.error).toBe("");

    answer.resolve(fakeView({ hotkey: "F9" }));
    await again;
    expect(store.view?.hotkey).toBe("F9");
    expect(store.error).toBe("");
  });
});

describe("the shortcut", () => {
  test("asks the Rust side what the shortcut is when the settings load", async () => {
    vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "⌃⌥Space" }));
    const store = await loaded();
    await vi.waitFor(() => expect(store.hotkey?.label).toBe("⌃⌥Space"));
  });

  test("goes on without the status when the Rust side can't tell it", async () => {
    vi.mocked(api.hotkeyStatus).mockRejectedValue("no");
    const store = await loaded();

    expect(store.view?.hotkey).toBe("Ctrl+Alt+Space");
    expect(store.hotkey).toBeNull();
    expect(store.error).toBe("");
  });

  test("a changed shortcut is shown at once and its new status is asked for", async () => {
    const store = await loaded();
    const answer = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.setHotkey).mockReturnValue(answer.promise);
    vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "F6" }));

    const changing = store.setHotkey("F6");
    expect(store.view?.hotkey).toBe("F6");

    answer.resolve(fakeView({ hotkey: "F6" }));
    expect(await changing).toBe("");
    expect(api.setHotkey).toHaveBeenCalledWith("F6");
    expect(store.hotkey?.label).toBe("F6");
  });

  test("a shortcut that is refused goes back, and the reason is the answer, not a banner", async () => {
    const store = await loaded();
    vi.mocked(api.setHotkey).mockRejectedValue("hotkey.taken");

    expect(await store.setHotkey("F6")).toBe("hotkey.taken");

    expect(store.view?.hotkey).toBe("Ctrl+Alt+Space");
    expect(store.error).toBe("");
  });

  test("a refused shortcut leaves an error from before alone, and a later save still works", async () => {
    const store = await loaded();
    vi.mocked(api.saveSettings).mockRejectedValueOnce("disk full");
    await store.update({ diarize: false });
    vi.mocked(api.setHotkey).mockRejectedValue("hotkey.taken");

    await store.setHotkey("F6");

    expect(store.error).toBe("disk full");
    await store.update({ diarize: false });
    expect(store.error).toBe("");
  });

  test("hears the status that the Rust side sends by itself", async () => {
    const { emitHotkey } = stubApi();
    const store = await loaded();

    const stop = await store.watchHotkey();
    emitHotkey(fakeHotkeyStatus({ label: "Ctrl+Alt+D", backend: "portal" }));
    expect(store.hotkey).toEqual(fakeHotkeyStatus({ label: "Ctrl+Alt+D", backend: "portal" }));

    stop();
    emitHotkey(fakeHotkeyStatus({ label: "F1" }));
    expect(store.hotkey?.label).toBe("Ctrl+Alt+D");
  });
});

describe("a setting that the Rust side changed by itself", () => {
  test("shows at once, and a later save of another setting carries it instead of the old value", async () => {
    // The language menu of the dictation chip saves the language without the window: the window must not send the old one back.
    const { emitSettings } = stubApi();
    const store = await loaded(fakeView({ dictation_language: "en-US" }));
    await store.watchSettings();

    emitSettings({ dictation_language: "vi-VN" });
    expect(store.view?.dictation_language).toBe("vi-VN");

    await store.update({ auto_refine: false });
    expect(vi.mocked(api.saveSettings).mock.calls[0][0]).toMatchObject({
      dictation_language: "vi-VN",
      auto_refine: false,
    });
    expect(store.view?.dictation_language).toBe("vi-VN");
  });

  test("leaves the other settings, and an edit that waits for its save, alone", async () => {
    const { emitSettings } = stubApi();
    const store = await loaded();
    await store.watchSettings();
    const first = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValueOnce(first.promise);
    const saving = store.update({ push_to_talk: true });
    await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));

    emitSettings({ dictation_language: "vi-VN" });
    expect(store.view).toMatchObject({ push_to_talk: true, dictation_language: "vi-VN" });

    // The save started before the event: it carried the old language and its answer says so. The window goes on with the new one.
    expect(vi.mocked(api.saveSettings).mock.calls[0][0]).toMatchObject({ dictation_language: "en-US" });
    first.resolve(fakeView({ push_to_talk: true, dictation_language: "en-US" }));
    await saving;
    expect(store.view).toMatchObject({ push_to_talk: true, dictation_language: "vi-VN" });
    await store.update({ auto_refine: false });
    expect(vi.mocked(api.saveSettings).mock.calls[1][0]).toMatchObject({ dictation_language: "vi-VN" });
  });

  test("a setting that the person is changing in the window wins over the one that came by itself", async () => {
    const { emitSettings } = stubApi();
    const store = await loaded();
    await store.watchSettings();
    const first = later<ReturnType<typeof fakeView>>();
    vi.mocked(api.saveSettings).mockReturnValueOnce(first.promise);
    const saving = store.update({ dictation_language: "pt-BR" });
    await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));

    emitSettings({ dictation_language: "vi-VN" });
    expect(store.view?.dictation_language).toBe("pt-BR");

    first.resolve(fakeView({ dictation_language: "pt-BR" }));
    await saving;
    expect(store.view?.dictation_language).toBe("pt-BR");
  });

  test("is not heard before the settings are loaded, nor after the window stops listening", async () => {
    const { emitSettings } = stubApi();
    vi.mocked(api.getSettings).mockRejectedValue("not yet");
    const store = new SettingsStore();
    await store.load();
    const stop = await store.watchSettings();
    emitSettings({ dictation_language: "vi-VN" });
    expect(store.view).toBeNull();

    vi.mocked(api.getSettings).mockResolvedValue(fakeView());
    await store.load();
    stop();
    emitSettings({ dictation_language: "vi-VN" });
    expect(store.view?.dictation_language).toBe("en-US");
  });
});
