import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import ChipMenu from "./ChipMenu.svelte";

beforeEach(() => {
  stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setLanguage("en");
});

const written = (overrides: Parameters<typeof fakePanelState>[0] = {}) =>
  fakePanelState({ view: "chip", phase: "written", text: "Hello there.", ...overrides });

/** The names of everything the menu offers, from the top. */
function names(): string[] {
  const menu = screen.getByRole("menu");
  return within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.textContent?.trim() ?? "");
}

test("chip_menu_order_skips_current_style: the header, the styles in order, then custom request, edit and copy", () => {
  render(ChipMenu, { panel: written(), onclose: () => {} });
  expect(names()).toEqual([
    "Rewrite as",
    "Shorten",
    "Turn into an email",
    "Bullet points",
    "Rewrite formally",
    "Turn into an AI prompt",
    "Translate to English",
    "Translate to Vietnamese",
    "Fix errors & punctuation",
    "Custom request…",
    "Edit…",
    "Copy",
  ]);
});

test("chip_menu_order_skips_current_style: the style the text is in is left out", () => {
  render(ChipMenu, { panel: written({ style: "email" }), onclose: () => {} });
  const all = names();
  expect(all).not.toContain("Turn into an email");
  expect(all.slice(0, 4)).toEqual(["Rewrite as", "Shorten", "Bullet points", "Rewrite formally"]);
  expect(all).toHaveLength(11);
});

test("chip_menu_has_no_delete_or_original: the first release has neither", () => {
  render(ChipMenu, { panel: written({ original: "hello there" }), onclose: () => {} });
  const text = screen.getByRole("menu").textContent ?? "";
  expect(text).not.toMatch(/delete/i);
  expect(text).not.toMatch(/original/i);
  expect(text).not.toMatch(/unedited/i);
});

test("the header is a disabled line, and a separator comes before the three actions", () => {
  render(ChipMenu, { panel: written(), onclose: () => {} });
  expect(screen.getByRole("menuitem", { name: "Rewrite as" })).toHaveAttribute("aria-disabled", "true");

  const menu = screen.getByRole("menu");
  const separator = within(menu).getByRole("separator");
  const after = [...menu.children].slice([...menu.children].indexOf(separator) + 1);
  expect(after.map((item) => item.textContent?.trim())).toEqual(["Custom request…", "Edit…", "Copy"]);
});

test("a style rewrites the text in it and closes the menu", async () => {
  const onclose = vi.fn();
  render(ChipMenu, { panel: written(), onclose });
  await fireEvent.click(screen.getByRole("menuitem", { name: "Shorten" }));
  expect(api.dictationRewrite).toHaveBeenCalledExactlyOnceWith("concise");
  expect(onclose).toHaveBeenCalledOnce();
});

test("Custom request… opens the card with its box for the request, and Edit… the card as it is", async () => {
  const onclose = vi.fn();
  render(ChipMenu, { panel: written(), onclose });

  await fireEvent.click(screen.getByRole("menuitem", { name: "Custom request…" }));
  expect(api.dictationOpenCard).toHaveBeenLastCalledWith(true);

  await fireEvent.click(screen.getByRole("menuitem", { name: "Edit…" }));
  expect(api.dictationOpenCard).toHaveBeenLastCalledWith(false);
  expect(api.dictationOpenCard).toHaveBeenCalledTimes(2);
  expect(onclose).toHaveBeenCalledTimes(2);
});

test("Copy puts what was written on the clipboard", async () => {
  const onclose = vi.fn();
  render(ChipMenu, { panel: written({ text: "Hello there." }), onclose });
  await fireEvent.click(screen.getByRole("menuitem", { name: "Copy" }));
  expect(api.dictationCopy).toHaveBeenCalledExactlyOnceWith("Hello there.");
  expect(onclose).toHaveBeenCalledOnce();
});

test("the menu is in Vietnamese when the UI is", () => {
  setLanguage("vi");
  render(ChipMenu, { panel: written({ style: "concise" }), onclose: () => {} });
  const all = names();
  expect(all[0]).toBe("Viết lại thành");
  expect(all.slice(-3)).toEqual(["Yêu cầu riêng…", "Chỉnh sửa…", "Sao chép"]);
  expect(all).not.toContain("Rút gọn");
});

test("the menu is counted in the size of the window, which grows to hold it", () => {
  render(ChipMenu, { panel: written(), onclose: () => {} });
  expect(screen.getByRole("menu")).toHaveAttribute("data-extent");
});
