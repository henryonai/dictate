import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import Chip from "./Chip.svelte";

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

const dots = () => screen.getByRole("button", { name: "More actions" });

test("the title is the one of the rules: a notice, Original, the style, or Written", () => {
  const { rerender } = render(Chip, { panel: written() });
  expect(screen.getByText("Written")).toBeInTheDocument();

  rerender({ panel: written({ original: "hello", style: "concise" }) });
  expect(screen.getByText("Shorten")).toBeInTheDocument();

  rerender({ panel: written({ original: "Hello there." }) });
  expect(screen.getByText("Original")).toBeInTheDocument();

  rerender({ panel: written({ notice: { key: "not_edited_no_key", detail: null } }) });
  expect(screen.getByText("Not edited by AI — no API key yet")).toBeInTheDocument();
});

test("a green check when all went well, an orange exclamation when there is a notice", () => {
  const { container, rerender } = render(Chip, { panel: written() });
  expect(container.querySelector("[data-mark]")).toHaveAttribute("data-mark", "check");

  rerender({ panel: written({ notice: { key: "edit_failed", detail: "timeout" } }) });
  expect(container.querySelector("[data-mark]")).toHaveAttribute("data-mark", "warning");
});

test("the whole title can be read when it is cut short", () => {
  const detail = "x".repeat(300);
  render(Chip, { panel: written({ notice: { key: "edit_failed", detail } }) });
  expect(screen.getByText(`Couldn't edit: ${detail}`)).toHaveAttribute("title", `Couldn't edit: ${detail}`);
});

test("the ⋯ button is there when text was written, and not on a chip that only has a notice", () => {
  const { rerender } = render(Chip, { panel: written() });
  expect(dots()).toBeInTheDocument();

  rerender({ panel: written({ text: "", notice: { key: "nothing_heard", detail: null } }) });
  expect(screen.queryByRole("button", { name: "More actions" })).not.toBeInTheDocument();
  expect(screen.getByText("Didn't hear anything.")).toBeInTheDocument();
});

test("the menu is closed until ⋯ is pressed, and pressing it again closes it", async () => {
  render(Chip, { panel: written() });
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(dots()).toHaveAttribute("aria-expanded", "false");

  await fireEvent.click(dots());
  expect(screen.getByRole("menu")).toBeInTheDocument();
  expect(dots()).toHaveAttribute("aria-expanded", "true");

  await fireEvent.click(dots());
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("the chip says when its menu opens and closes, so that it stays while the menu is open", async () => {
  render(Chip, { panel: written() });
  await fireEvent.click(dots());
  expect(api.dictationMenuOpen).toHaveBeenLastCalledWith(true);

  await fireEvent.click(dots());
  expect(api.dictationMenuOpen).toHaveBeenLastCalledWith(false);
  expect(api.dictationMenuOpen).toHaveBeenCalledTimes(2);
});

test("choosing something in the menu closes it, and says so", async () => {
  render(Chip, { panel: written() });
  await fireEvent.click(dots());
  await fireEvent.click(screen.getByRole("menuitem", { name: "Copy" }));

  expect(api.dictationCopy).toHaveBeenCalledWith("Hello there.");
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(api.dictationMenuOpen).toHaveBeenLastCalledWith(false);
});

test("a press outside the chip closes the menu", async () => {
  render(Chip, { panel: written() });
  await fireEvent.click(dots());
  await fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(api.dictationMenuOpen).toHaveBeenLastCalledWith(false);
});

test("a press inside the menu leaves it open", async () => {
  render(Chip, { panel: written() });
  await fireEvent.click(dots());
  await fireEvent.pointerDown(screen.getByRole("menuitem", { name: "Rewrite as" }));
  expect(screen.getByRole("menu")).toBeInTheDocument();
});

test("Esc closes the menu and goes no further", async () => {
  // Whatever listens for Esc on the page (the card's Esc, a window that hides) must not hear the one that closes the menu.
  const onEscape = vi.fn();
  window.addEventListener("keydown", onEscape);
  render(Chip, { panel: written() });
  await fireEvent.click(dots());

  const notPrevented = await fireEvent.keyDown(dots(), { key: "Escape" });
  window.removeEventListener("keydown", onEscape);

  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(onEscape).not.toHaveBeenCalled();
  // The key is taken: its own action is prevented too.
  expect(notPrevented).toBe(false);
});

test("Esc with no menu open goes where it was going", async () => {
  const onEscape = vi.fn();
  window.addEventListener("keydown", onEscape);
  render(Chip, { panel: written() });

  await fireEvent.keyDown(dots(), { key: "Escape" });
  window.removeEventListener("keydown", onEscape);
  expect(onEscape).toHaveBeenCalledOnce();
});

test("when the chip goes away with its menu open it says that the menu is closed", async () => {
  const { unmount } = render(Chip, { panel: written() });
  await fireEvent.click(dots());
  vi.mocked(api.dictationMenuOpen).mockClear();

  unmount();
  expect(api.dictationMenuOpen).toHaveBeenCalledExactlyOnceWith(false);
});

/** Pretends to be a Mac, or any other system. */
function system(platform: "MacIntel" | "Win32") {
  vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
}

test("the chip says when the mouse comes over it and when it leaves, once each", async () => {
  system("Win32");
  const { container } = render(Chip, { panel: written() });
  const chip = container.querySelector(".chip") as HTMLElement;

  await fireEvent.mouseEnter(chip);
  expect(api.dictationChipHover).toHaveBeenLastCalledWith(true);
  await fireEvent.mouseLeave(chip);
  expect(api.dictationChipHover).toHaveBeenLastCalledWith(false);
  expect(api.dictationChipHover).toHaveBeenCalledTimes(2);
});

test("on a Mac the window tells the Rust side about the mouse, and the page leaves it alone", async () => {
  // The panel that holds the chip never is the key window, and the page of a window that isn't gets no mouse movement:
  // the Rust side has a tracking area on the window itself, which hears the mouse where the page can't.
  system("MacIntel");
  const { container } = render(Chip, { panel: written() });
  const chip = container.querySelector(".chip") as HTMLElement;

  await fireEvent.mouseEnter(chip);
  await fireEvent.mouseLeave(chip);
  expect(api.dictationChipHover).not.toHaveBeenCalled();
});

test("the labels are in Vietnamese when the UI is", () => {
  setLanguage("vi");
  render(Chip, { panel: written() });
  expect(screen.getByText("Đã viết")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Thêm tuỳ chọn" })).toBeInTheDocument();
});

test("an error is the title of the chip, with the orange exclamation", () => {
  const { container } = render(Chip, {
    panel: written({ error: { key: "audio.stream", detail: "device gone", action: null } }),
  });
  expect(screen.getByText(/microphone stopped/i)).toBeInTheDocument();
  expect(container.querySelector("[data-mark]")).toHaveAttribute("data-mark", "warning");
});
