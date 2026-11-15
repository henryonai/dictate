import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import en from "../locales/en.json";
import { fakeHotkeyStatus, fakeView, stubApi } from "../test-helpers";
import Settings from "./Settings.svelte";

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

test("shows the four tabs, General first", async () => {
  render(Settings);

  const tabs = await screen.findAllByRole("tab");
  expect(tabs.map((tab) => tab.textContent)).toEqual([
    "General",
    "Speech recognition",
    "AI editing & summaries",
    "Accounts & API keys",
  ]);
  expect(screen.getByRole("tab", { name: "General" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("tabpanel", { name: "General" })).toBeVisible();
});

test("a click on a tab shows its page and hides the others", async () => {
  const user = userEvent.setup();
  render(Settings);

  await user.click(await screen.findByRole("tab", { name: "Accounts & API keys" }));

  expect(screen.getByRole("tabpanel", { name: "Accounts & API keys" })).toBeVisible();
  expect(screen.queryByRole("tabpanel", { name: "General" })).not.toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Accounts & API keys" })).toHaveAttribute("aria-selected", "true");
});

test("what was typed on a page is still there after a visit to another", async () => {
  const user = userEvent.setup();
  render(Settings);
  await user.click(await screen.findByRole("tab", { name: "Accounts & API keys" }));
  await user.type(screen.getAllByLabelText("API key")[0], "sk-half-typed");

  await user.click(screen.getByRole("tab", { name: "General" }));
  await user.click(screen.getByRole("tab", { name: "Accounts & API keys" }));

  expect(screen.getAllByLabelText("API key")[0]).toHaveValue("sk-half-typed");
});

test("speaks the interface language of the settings", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ ui_language: "vi" }));
  render(Settings);

  expect(await screen.findByRole("tab", { name: "Chung" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "Nhận dạng giọng nói" })).toBeInTheDocument();
});

test("shows the load warning, and keeps it while the settings are saved", async () => {
  const user = userEvent.setup();
  vi.mocked(api.getSettings).mockResolvedValue(
    fakeView({ load_warning: "The settings file was reset to defaults because it is not valid JSON." }),
  );
  render(Settings);

  const warning = await screen.findByRole("alert");
  expect(warning).toHaveTextContent("The settings file was reset to defaults because it is not valid JSON.");

  // Every answer after the first carries no warning; the banner must not go with it.
  await user.click(screen.getByRole("checkbox", { name: "Save recordings + transcripts" }));
  await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
  expect(screen.getByRole("alert")).toHaveTextContent("The settings file was reset to defaults");
});

test("the load warning goes away when it is dismissed", async () => {
  const user = userEvent.setup();
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ load_warning: "reset" }));
  render(Settings);
  await screen.findByRole("alert");

  await user.click(screen.getByRole("button", { name: "Dismiss" }));

  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

const DICTATIONS_USED = { item: "Dictations", reason: "Permission denied", fallback: true };
const MODELS_NOT_USED = { item: "whisper-models", reason: "Permission denied", fallback: false };
const MARKER_NOT_WRITTEN = { item: "migrated-from-dictate", reason: "Read-only file system", fallback: false };

test("says what could not be copied from Dictate, and that Loah uses Dictate's copy of it, until dismissed", async () => {
  const user = userEvent.setup();
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ migration_note: [DICTATIONS_USED] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent("Not everything was copied.");
  expect(note).toHaveTextContent("Loah could not copy the saved dictations (Permission denied) from Dictate");
  expect(note).toHaveTextContent("still using Dictate's copy of them");
  expect(note).toHaveTextContent("tries again at the next start");

  // Every answer after the first carries no note; the banner must not go with it.
  await user.click(screen.getByRole("checkbox", { name: "Save recordings + transcripts" }));
  await vi.waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
  expect(screen.getByRole("alert")).toHaveTextContent("the saved dictations (Permission denied)");

  await user.click(screen.getByRole("button", { name: "Dismiss" }));
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("an item with no copy of Dictate's to read is not said to be read from Dictate's copy", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ migration_note: [MODELS_NOT_USED] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent("Loah could not copy the speech models (Permission denied) from Dictate.");
  expect(note).toHaveTextContent("It tries again at the next start.");
  expect(note).not.toHaveTextContent("still using");
  expect(note).not.toHaveTextContent("Dictate's copy");
});

test("the note words the items that are read from Dictate's copy and the ones that are not each in their own sentence", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ migration_note: [DICTATIONS_USED, MODELS_NOT_USED] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent(
    "Loah could not copy the saved dictations (Permission denied) from Dictate, so it is still using Dictate's copy of them",
  );
  expect(note).toHaveTextContent("Loah could not copy the speech models (Permission denied) from Dictate. It tries again");
});

test("a mark that could not be written is said in a sentence of its own: the copy itself is complete", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ migration_note: [MARKER_NOT_WRITTEN] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent(
    "Loah copied everything from Dictate but could not note that it is done (Read-only file system); it checks again at the next start.",
  );
  // The mark is written, not copied, and nothing is missing: no "not everything was copied", no "could not copy", no internal name.
  expect(note).not.toHaveTextContent("Not everything was copied.");
  expect(note).not.toHaveTextContent("could not copy");
  expect(note).not.toHaveTextContent("migrated-from-dictate");
  expect(note).not.toHaveTextContent("still using");
});

test("the mark is not said beside an item that could not be copied: the mark is only written when nothing failed", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ migration_note: [MODELS_NOT_USED, MARKER_NOT_WRITTEN] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent("Not everything was copied.");
  expect(note).toHaveTextContent("Loah could not copy the speech models (Permission denied) from Dictate. It tries again at the next start.");
  expect(note).not.toHaveTextContent("copied everything");
  expect(note).not.toHaveTextContent("Read-only file system");
});

test("an item that this version has no name for is shown as the Rust side calls it", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(
    fakeView({ migration_note: [{ item: "something-new", reason: "x", fallback: false }] }),
  );
  render(Settings);

  expect(await screen.findByRole("alert")).toHaveTextContent("Loah could not copy something-new (x) from Dictate.");
});

test("both wordings are in Vietnamese in the Vietnamese interface", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(
    fakeView({ ui_language: "vi", migration_note: [DICTATIONS_USED, MODELS_NOT_USED] }),
  );
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent("Chưa chép được hết dữ liệu.");
  expect(note).toHaveTextContent("Loah chưa chép được các bản đọc chính tả đã lưu (Permission denied) từ Dictate nên tạm dùng bản của Dictate");
  expect(note).toHaveTextContent("Loah chưa chép được các model Whisper (Permission denied) từ Dictate. Loah sẽ thử lại ở lần mở sau.");
});

test("the sentence about the mark is in Vietnamese in the Vietnamese interface", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ ui_language: "vi", migration_note: [MARKER_NOT_WRITTEN] }));
  render(Settings);

  const note = await screen.findByRole("alert");
  expect(note).toHaveTextContent(
    "Loah đã chép xong mọi thứ từ Dictate nhưng chưa ghi lại được là việc chép đã xong (Read-only file system); Loah sẽ kiểm tra lại ở lần mở sau.",
  );
  expect(note).not.toHaveTextContent("Chưa chép được hết dữ liệu.");
  expect(note).not.toHaveTextContent("migrated-from-dictate");
});

test("the note about the copy from Dictate and the warning about the file are two banners", async () => {
  vi.mocked(api.getSettings).mockResolvedValue(
    fakeView({ load_warning: "reset", migration_note: [{ item: "Dictations", reason: "x", fallback: true }] }),
  );
  render(Settings);

  await screen.findAllByRole("tab");
  expect(screen.getAllByRole("alert")).toHaveLength(2);
});

test("no banner when the settings loaded cleanly", async () => {
  render(Settings);
  await screen.findAllByRole("tab");
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("says so when the settings can't be read", async () => {
  vi.mocked(api.getSettings).mockRejectedValue("The settings folder is locked");
  render(Settings);

  expect(await screen.findByRole("alert")).toHaveTextContent("Could not read the settings: The settings folder is locked");
});

test("a failed first load shows the error with Retry, and Retry reads the settings again", async () => {
  const user = userEvent.setup();
  vi.mocked(api.getSettings).mockRejectedValueOnce("The settings folder is locked");
  render(Settings);

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not read the settings: The settings folder is locked",
  );
  // Dismissing it would leave a window with nothing in it.
  expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
  expect(screen.queryByText("Loading the settings…")).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Retry" }));

  expect(await screen.findAllByRole("tab")).toHaveLength(4);
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("the Retry of a failed load is in the UI language too", async () => {
  vi.mocked(api.getSettings).mockRejectedValueOnce("locked");
  setLanguage("vi");
  render(Settings);
  expect(await screen.findByRole("button", { name: "Thử lại" })).toBeInTheDocument();
});

test("the error goes away when it is dismissed", async () => {
  const user = userEvent.setup();
  vi.mocked(api.saveSettings).mockRejectedValue("disk full");
  render(Settings);
  await user.click(await screen.findByRole("checkbox", { name: "Save recordings + transcripts" }));
  await screen.findByRole("alert");

  await user.click(screen.getByRole("button", { name: "Dismiss" }));

  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("says so when a change can't be saved", async () => {
  const user = userEvent.setup();
  vi.mocked(api.saveSettings).mockRejectedValue("Could not save the settings: disk full");
  render(Settings);

  await user.click(await screen.findByRole("checkbox", { name: "Save recordings + transcripts" }));

  expect(await screen.findByRole("alert")).toHaveTextContent("Could not save: Could not save the settings: disk full");
});

test("follows what the Rust side says about the shortcut by itself, as when the desktop answers late", async () => {
  const { emitHotkey } = stubApi();
  render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onHotkeyStatus).toHaveBeenCalled());
  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");

  emitHotkey(fakeHotkeyStatus({ label: "Ctrl+Alt+D", backend: "portal" }));

  expect(await screen.findByDisplayValue("Ctrl+Alt+D")).toBeInTheDocument();
  expect(screen.getByText(/Your desktop manages this shortcut/)).toBeInTheDocument();
});

test("stops following the shortcut when the window goes", async () => {
  const stop = vi.fn();
  vi.mocked(api.onHotkeyStatus).mockResolvedValue(stop);
  const { unmount } = render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onHotkeyStatus).toHaveBeenCalled());

  unmount();

  await vi.waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
});

test("shows a language that the dictation chip changed while the window was closed, and keeps it through the next save", async () => {
  const user = userEvent.setup();
  const { emitSettings } = stubApi();
  render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onSettingsChanged).toHaveBeenCalled());
  expect(screen.getByLabelText("Language")).toHaveValue("en-US");

  emitSettings({ dictation_language: "vi-VN" });
  await vi.waitFor(() => expect(screen.getByLabelText("Language")).toHaveValue("vi-VN"));

  await user.click(screen.getByRole("checkbox", { name: "Save recordings + transcripts" }));
  expect(vi.mocked(api.saveSettings).mock.lastCall?.[0]).toMatchObject({ dictation_language: "vi-VN" });
});

test("stops following the settings when the window goes", async () => {
  const stop = vi.fn();
  vi.mocked(api.onSettingsChanged).mockResolvedValue(stop);
  const { unmount } = render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onSettingsChanged).toHaveBeenCalled());

  unmount();

  await vi.waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
});

test.each(["en", "vi"] as const)("no text on screen is a key that has no translation (%s)", async (language) => {
  const catalog = ["tiny", "base", "small", "large-v3-turbo-q5_0", "medium", "large-v3-turbo"];
  vi.mocked(api.listModels).mockResolvedValue(
    catalog.map((id) => ({ id, size_mb: 100, downloaded: id === "small", recommended: id === "small" })),
  );
  vi.mocked(api.getSettings).mockResolvedValue(fakeView({ ui_language: language, transcribe_preset: "openrouter" }));
  render(Settings);
  await screen.findAllByRole("listitem", { name: /Large v3 Turbo/, hidden: true });

  // Every page is in the document, hidden or not; a key that shows as itself is a text nobody wrote. (No word
  // boundary in the pattern: neighbouring texts run together in `textContent`.)
  const namespaces = new Set(Object.keys(en).map((key) => key.split(".")[0]));
  const leaked = new RegExp(`(?:${[...namespaces].join("|")})\\.[\\w-]+`);
  expect(document.body.textContent).not.toMatch(leaked);
});

test("shows the page it is asked to show, as when the meetings window sends someone to the API keys", async () => {
  const { emitTab } = stubApi();
  render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onSettingsTab).toHaveBeenCalled());
  expect(screen.getByRole("tab", { name: "General" })).toHaveAttribute("aria-selected", "true");

  emitTab("accounts");

  await vi.waitFor(() =>
    expect(screen.getByRole("tab", { name: "Accounts & API keys" })).toHaveAttribute("aria-selected", "true"),
  );
  expect(screen.getByRole("tabpanel", { name: "Accounts & API keys" })).toBeVisible();

  emitTab("speech");
  await vi.waitFor(() =>
    expect(screen.getByRole("tab", { name: "Speech recognition" })).toHaveAttribute("aria-selected", "true"),
  );
});

test("a page that does not exist is not shown", async () => {
  const { emitTab } = stubApi();
  render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onSettingsTab).toHaveBeenCalled());

  emitTab("meetings" as never);

  expect(screen.getByRole("tab", { name: "General" })).toHaveAttribute("aria-selected", "true");
});

test("stops following the requests for a page when the window goes", async () => {
  const stop = vi.fn();
  vi.mocked(api.onSettingsTab).mockResolvedValue(stop);
  const { unmount } = render(Settings);
  await screen.findAllByRole("tab");
  await vi.waitFor(() => expect(api.onSettingsTab).toHaveBeenCalled());

  unmount();

  await vi.waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
});
