import { afterEach, expect, test, vi } from "vitest";
import { api } from "./api";
import type { Theme } from "./api";
import { applyTheme, followTheme } from "./theme";

const root = document.documentElement;

afterEach(() => {
  root.removeAttribute("data-theme");
  vi.restoreAllMocks();
});

/** What `followTheme` listens with: the test sends the events, as the Rust side does. */
function stubThemeApi(current: Theme | Error) {
  let listener: ((theme: Theme) => void) | undefined;
  const stop = vi.fn();
  vi.spyOn(api, "currentTheme").mockImplementation(async () => {
    if (current instanceof Error) throw current;
    return current;
  });
  vi.spyOn(api, "onThemeChanged").mockImplementation(async (handler) => {
    listener = handler;
    return stop;
  });
  return { emit: (theme: Theme) => listener?.(theme), stop };
}

test("applyTheme_sets_and_clears_data_theme", () => {
  applyTheme("dark");
  expect(root).toHaveAttribute("data-theme", "dark");

  applyTheme("light");
  expect(root).toHaveAttribute("data-theme", "light");

  // System is no attribute at all: the rules of tokens.css then follow the system's own colors.
  applyTheme("system");
  expect(root).not.toHaveAttribute("data-theme");
});

test("a word that is no theme leaves the page in the system's", () => {
  applyTheme("dark");

  applyTheme("sepia" as Theme);

  expect(root).not.toHaveAttribute("data-theme");
});

test("followTheme_follows_the_event", async () => {
  const rust = stubThemeApi("light");

  await followTheme();
  // What the settings say now, asked for once the page has opened.
  expect(root).toHaveAttribute("data-theme", "light");

  rust.emit("dark");
  expect(root).toHaveAttribute("data-theme", "dark");
  rust.emit("system");
  expect(root).not.toHaveAttribute("data-theme");
  rust.emit("light");
  expect(root).toHaveAttribute("data-theme", "light");
});

test("a theme that was chosen while the answer was on its way is not undone by the answer", async () => {
  const rust = stubThemeApi("dark");
  let answer: (theme: Theme) => void = () => {};
  vi.mocked(api.currentTheme).mockImplementation(() => new Promise((resolve) => (answer = resolve)));

  const following = followTheme();
  await vi.waitFor(() => expect(api.currentTheme).toHaveBeenCalled());
  rust.emit("light");
  answer("dark");
  await following;

  expect(root).toHaveAttribute("data-theme", "light");
});

test("a page that cannot ask for the theme stays in the system's and still follows the events", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  const rust = stubThemeApi(new Error("not allowed"));

  await followTheme();
  expect(root).not.toHaveAttribute("data-theme");
  expect(warn).toHaveBeenCalled();

  rust.emit("dark");
  expect(root).toHaveAttribute("data-theme", "dark");
});

test("a page that cannot listen still takes the theme that it asked for, and does not break", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  stubThemeApi("dark");
  vi.mocked(api.onThemeChanged).mockRejectedValue(new Error("not allowed"));

  await expect(followTheme()).resolves.toBeUndefined();

  expect(root).toHaveAttribute("data-theme", "dark");
  expect(warn).toHaveBeenCalled();
});
