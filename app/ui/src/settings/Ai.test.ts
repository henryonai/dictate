import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { loadedStore, stubApi } from "../test-helpers";
import Ai from "./Ai.svelte";

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

const lastSaved = () => vi.mocked(api.saveSettings).mock.lastCall?.[0];

test("offers the 8 edit presets from the Rust side, with titles in the UI language", async () => {
  render(Ai, { store: await loadedStore() });
  const select = await screen.findByLabelText("Default editing style");
  const titles = [...select.querySelectorAll("option")].map((option) => option.textContent);
  expect(titles).toEqual([
    "Fix errors & punctuation",
    "Rewrite formally",
    "Shorten",
    "Bullet points",
    "Turn into an AI prompt",
    "Turn into an email",
    "Translate to English",
    "Translate to Vietnamese",
  ]);

  setLanguage("vi");
  expect(await screen.findByRole("option", { name: "Gạch đầu dòng" })).toBeInTheDocument();
});

test("shows the stored preset and saves another", async () => {
  const user = userEvent.setup();
  render(Ai, { store: await loadedStore({ default_edit_preset: "email" }) });
  const select = await screen.findByLabelText("Default editing style");
  expect(select).toHaveDisplayValue("Turn into an email");

  await user.selectOptions(select, "Bullet points");

  expect(lastSaved()).toMatchObject({ default_edit_preset: "bullets" });
});

test("a stored preset the list doesn't know stays visible instead of silently changing", async () => {
  render(Ai, { store: await loadedStore({ default_edit_preset: "legacy" }) });
  expect(await screen.findByLabelText("Default editing style")).toHaveDisplayValue("legacy");
});

test("auto-refine is a switch that saves", async () => {
  const user = userEvent.setup();
  render(Ai, { store: await loadedStore() });

  await user.click(screen.getByRole("checkbox", { name: "Edit with AI automatically when dictation stops" }));

  expect(lastSaved()).toMatchObject({ auto_refine: false });
});

test("the summary model saves when the field is left", async () => {
  const user = userEvent.setup();
  render(Ai, { store: await loadedStore() });

  await user.type(screen.getByLabelText("Summary model (empty = use the main model)"), "claude-sonnet-5");
  expect(api.saveSettings).not.toHaveBeenCalled();
  await user.tab();

  expect(lastSaved()).toMatchObject({ summary_model: "claude-sonnet-5" });
});

test("the vocabulary has room for several lines and saves when the field is left", async () => {
  const user = userEvent.setup();
  render(Ai, { store: await loadedStore() });

  await user.type(screen.getByLabelText("Custom vocabulary / context"), "Dictate{Enter}Tauri");
  await user.tab();

  expect(lastSaved()).toMatchObject({ vocabulary: "Dictate\nTauri" });
});
