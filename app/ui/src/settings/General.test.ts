import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakeHotkeyStatus, loadedStore, stubApi } from "../test-helpers";
import General from "./General.svelte";

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

const lastSaved = () => vi.mocked(api.saveSettings).mock.lastCall?.[0];

test("shows the stored shortcut and doesn't let it be typed over", async () => {
  vi.mocked(api.hotkeyStatus).mockRejectedValue("not asked yet");
  render(General, { store: await loadedStore({ hotkey: "Ctrl+Shift+D" }) });
  const hotkey = screen.getByLabelText("Shortcut");
  expect(hotkey).toHaveValue("Ctrl+Shift+D");
  expect(hotkey).toHaveAttribute("readonly");
});

test("shows the shortcut as the Rust side writes it for this system", async () => {
  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "⌃⌥Space" }));
  render(General, { store: await loadedStore() });

  await vi.waitFor(() => expect(screen.getByLabelText("Shortcut")).toHaveValue("⌃⌥Space"));
});

test("a recorded shortcut is registered and saved by the Rust side, and shows when it worked", async () => {
  const user = userEvent.setup();
  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus());
  render(General, { store: await loadedStore() });
  await screen.findByDisplayValue("Ctrl+Alt+Space");

  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "Ctrl+Alt+D" }));
  await user.click(screen.getByRole("button", { name: "Change…" }));
  await fireEvent.keyDown(window, { code: "KeyD", ctrlKey: true, altKey: true });

  expect(api.setHotkey).toHaveBeenCalledExactlyOnceWith("Ctrl+Alt+D");
  expect(api.saveSettings).not.toHaveBeenCalled();
  expect(await screen.findByDisplayValue("Ctrl+Alt+D")).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("recorder_shows_taken_error: a shortcut another app owns is refused and says so, and nothing changes", async () => {
  const user = userEvent.setup();
  vi.mocked(api.setHotkey).mockRejectedValue("hotkey.taken");
  render(General, { store: await loadedStore() });

  await user.click(screen.getByRole("button", { name: "Change…" }));
  await fireEvent.keyDown(window, { code: "Space", ctrlKey: true, altKey: true });

  expect(await screen.findByRole("alert")).toHaveTextContent("That shortcut is already used by another app. Try a different one.");
  expect(screen.getByLabelText("Shortcut")).toHaveValue("Ctrl+Alt+Space");
});

test("a shortcut that was taken when the app started says so, and can be replaced", async () => {
  const user = userEvent.setup();
  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ backend: "none", error: "hotkey.taken" }));
  render(General, { store: await loadedStore() });

  expect(await screen.findByRole("alert")).toHaveTextContent("already used by another app");

  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "F6" }));
  await user.click(screen.getByRole("button", { name: "Change…" }));
  await fireEvent.keyDown(window, { code: "F6" });

  await vi.waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  expect(screen.getByLabelText("Shortcut")).toHaveValue("F6");
});

test("a desktop with no global shortcuts for apps says how to bind dictate --toggle, and has nothing to change", async () => {
  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ backend: "none", error: "hotkey.wayland_unavailable" }));
  render(General, { store: await loadedStore() });

  expect(
    await screen.findByText(
      "Your desktop doesn't offer global shortcuts to apps. Bind a shortcut in your desktop settings to run: dictate --toggle",
    ),
  ).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Change…" })).not.toBeInTheDocument();
});

test("a shortcut that the desktop's portal binds is changed in the desktop, not here", async () => {
  vi.mocked(api.hotkeyStatus).mockResolvedValue(fakeHotkeyStatus({ label: "Ctrl+Alt+D", backend: "portal" }));
  render(General, { store: await loadedStore() });

  expect(await screen.findByDisplayValue("Ctrl+Alt+D")).toBeInTheDocument();
  expect(screen.getByText(/Your desktop manages this shortcut/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Change…" })).not.toBeInTheDocument();
});

test("tells how to run dictate --toggle", async () => {
  render(General, { store: await loadedStore() });
  expect(screen.getByText(/dictate --toggle/)).toBeInTheDocument();
});

test("changing the interface language saves it and switches the words at once", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  await user.selectOptions(screen.getByLabelText("Interface language"), "Tiếng Việt");

  expect(lastSaved()).toMatchObject({ ui_language: "vi" });
  expect(await screen.findByLabelText("Ngôn ngữ giao diện")).toBeInTheDocument();
  expect(screen.queryByLabelText("Interface language")).not.toBeInTheDocument();
});

test("switching the language back to the system's follows the web view", async () => {
  vi.spyOn(navigator, "language", "get").mockReturnValue("en-GB");
  const user = userEvent.setup();
  render(General, { store: await loadedStore({ ui_language: "vi" }) });

  await user.selectOptions(screen.getByLabelText("Ngôn ngữ giao diện"), "system");

  expect(lastSaved()).toMatchObject({ ui_language: "system" });
  expect(await screen.findByLabelText("Interface language")).toBeInTheDocument();
});

test("picks a dictation language from the list", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  const language = screen.getByLabelText("Language");
  expect(language).toHaveValue("en-US");
  await user.selectOptions(language, "vi-VN");

  expect(lastSaved()).toMatchObject({ dictation_language: "vi-VN" });
});

test("takes any other dictation language as free text", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  await user.selectOptions(screen.getByLabelText("Language"), "Other…");
  await user.type(screen.getByLabelText("Language code (BCP-47)"), "pt-BR");
  await user.tab();

  expect(lastSaved()).toMatchObject({ dictation_language: "pt-BR" });
});

test("a stored language that isn't on the list shows as Other, with its code", async () => {
  render(General, { store: await loadedStore({ dictation_language: "nl-NL" }) });
  expect(screen.getByLabelText("Language")).toHaveDisplayValue("Other…");
  expect(screen.getByLabelText("Language code (BCP-47)")).toHaveValue("nl-NL");
});

test("the mode picks between press-to-toggle and push-to-talk", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  await user.selectOptions(screen.getByLabelText("Mode"), "Hold the key to talk, release to stop");

  expect(lastSaved()).toMatchObject({ push_to_talk: true });
});

test.each([
  ["Write into the focused text field", "auto_write", true],
  ["Save recordings + transcripts", "save_recordings", true],
  ["Open Dictate at login", "launch_at_login", false],
] as const)("the %s switch saves %s", async (label, key, was) => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  const toggle = screen.getByRole("checkbox", { name: label });
  expect(toggle.matches(":checked")).toBe(was);
  await user.click(toggle);

  expect(lastSaved()).toMatchObject({ [key]: !was });
});

test("a switch goes back when the system refuses the change", async () => {
  const user = userEvent.setup();
  vi.mocked(api.saveSettings).mockRejectedValue("Could not change launch at login: denied");
  render(General, { store: await loadedStore() });

  const toggle = screen.getByRole("checkbox", { name: "Open Dictate at login" });
  await user.click(toggle);

  await vi.waitFor(() => expect(toggle).not.toBeChecked());
});

test("Open meetings folder asks the Rust side to open it", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  await user.click(screen.getByRole("button", { name: "Open meetings folder" }));

  expect(api.openMeetingsFolder).toHaveBeenCalledTimes(1);
});

test("Open meetings folder shows the error text when it can't", async () => {
  const user = userEvent.setup();
  vi.mocked(api.openMeetingsFolder).mockRejectedValue("Could not create /nowhere: denied");
  render(General, { store: await loadedStore() });

  await user.click(screen.getByRole("button", { name: "Open meetings folder" }));

  expect(await screen.findByText("Could not create /nowhere: denied")).toBeInTheDocument();
});

test("the shortcut is paused while the recorder listens and goes on after", async () => {
  const user = userEvent.setup();
  render(General, { store: await loadedStore() });

  await user.click(screen.getByRole("button", { name: "Change…" }));
  expect(api.hotkeyRecording).toHaveBeenLastCalledWith(true);

  await fireEvent.keyDown(window, { code: "Escape" });
  await vi.waitFor(() => expect(api.hotkeyRecording).toHaveBeenLastCalledWith(false));
});

test("a pause that can't be asked for doesn't break the recorder", async () => {
  const user = userEvent.setup();
  vi.mocked(api.hotkeyRecording).mockRejectedValue("no");
  render(General, { store: await loadedStore() });

  await user.click(screen.getByRole("button", { name: "Change…" }));
  await fireEvent.keyDown(window, { code: "F6" });

  await vi.waitFor(() => expect(api.setHotkey).toHaveBeenCalledWith("F6"));
});
