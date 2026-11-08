import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import en from "../locales/en.json";
import { fakeView, stubApi } from "../test-helpers";
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
