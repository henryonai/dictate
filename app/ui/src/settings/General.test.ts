import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { loadedStore, stubApi } from "../test-helpers";
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
  render(General, { store: await loadedStore({ hotkey: "Ctrl+Shift+D" }) });
  const hotkey = screen.getByLabelText("Shortcut");
  expect(hotkey).toHaveValue("Ctrl+Shift+D");
  expect(hotkey).toHaveAttribute("readonly");
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
