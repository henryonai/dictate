import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { FAVOURITE_LANGUAGES, OTHER_LANGUAGES, stubPanelApi } from "../test-helpers";
import LanguageMenu from "./LanguageMenu.svelte";
import { languageName } from "./state";

beforeEach(() => {
  stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setLanguage("en");
});

const chip = () => screen.getByRole("button", { name: "Change recognition language" });

test("the chip is the first two letters of the language, in capitals", () => {
  render(LanguageMenu, { language: "vi-VN" });
  expect(chip()).toHaveTextContent("VI");
});

test("the menu is closed until the chip is pressed", () => {
  render(LanguageMenu, { language: "en-US" });
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(chip()).toHaveAttribute("aria-expanded", "false");
});

test("language_menu_lists_favourites_then_others: the favourites come first, in order, by their name", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());

  const menu = await screen.findByRole("menu");
  const items = await within(menu).findAllByRole("menuitemradio");
  expect(items.map((item) => item.getAttribute("data-lang"))).toEqual(FAVOURITE_LANGUAGES);
  expect(items.map((item) => item.querySelector(".name")?.textContent)).toEqual(
    FAVOURITE_LANGUAGES.map((code) => languageName(code, "en")),
  );
  expect(chip()).toHaveAttribute("aria-expanded", "true");
});

test("language_menu_lists_favourites_then_others: Other languages opens the rest, in order, after the favourites", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  const menu = await screen.findByRole("menu");

  // The entry that opens the rest comes after the favourites, and the rest are not there yet.
  const others = within(menu).getByRole("menuitem", { name: /Other languages/ });
  const all = [...menu.querySelectorAll('[role^="menuitem"]')];
  expect(all.indexOf(others)).toBe(FAVOURITE_LANGUAGES.length);
  expect(screen.queryByRole("menuitemradio", { name: languageName("es-ES", "en") })).not.toBeInTheDocument();

  await fireEvent.click(others);
  const submenu = await screen.findByRole("menu", { name: "Other languages" });
  const items = within(submenu).getAllByRole("menuitemradio");
  expect(items.map((item) => item.getAttribute("data-lang"))).toEqual(OTHER_LANGUAGES);
});

test("language_menu_lists_favourites_then_others: the names are in the language of the UI", async () => {
  setLanguage("vi");
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(screen.getByRole("button", { name: "Đổi ngôn ngữ nhận dạng" }));

  const menu = await screen.findByRole("menu");
  const first = (await within(menu).findAllByRole("menuitemradio"))[0];
  expect(first.querySelector(".name")?.textContent).toBe(languageName("en-US", "vi"));
  expect(within(menu).getByRole("menuitem", { name: /Ngôn ngữ khác/ })).toBeInTheDocument();
});

test("the language of the dictation is checked", async () => {
  render(LanguageMenu, { language: "vi-VN" });
  await fireEvent.click(chip());
  const menu = await screen.findByRole("menu");

  const items = await within(menu).findAllByRole("menuitemradio");
  expect(items.filter((item) => item.getAttribute("aria-checked") === "true").map((i) => i.dataset.lang)).toEqual(["vi-VN"]);
});

test("choosing a language tells the Rust side and closes the menu", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  const menu = await screen.findByRole("menu");

  await fireEvent.click(await within(menu).findByRole("menuitemradio", { name: languageName("ja-JP", "en") }));

  expect(api.dictationSetLanguage).toHaveBeenCalledExactlyOnceWith("ja-JP");
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("a language of the second list can be chosen too", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  await fireEvent.click(await screen.findByRole("menuitem", { name: /Other languages/ }));

  await fireEvent.click(await screen.findByRole("menuitemradio", { name: languageName("th-TH", "en") }));

  expect(api.dictationSetLanguage).toHaveBeenCalledExactlyOnceWith("th-TH");
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("Esc closes the menu, and so does pressing the chip again", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  await screen.findByRole("menu");

  await fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();

  await fireEvent.click(chip());
  await screen.findByRole("menu");
  await fireEvent.click(chip());
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("a press outside the menu closes it", async () => {
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  await screen.findByRole("menu");

  await fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
});

test("Esc that closes the menu is not Esc that cancels the dictation", async () => {
  const onEscape = vi.fn();
  window.addEventListener("keydown", onEscape);
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  await screen.findByRole("menu");

  await fireEvent.keyDown(chip(), { key: "Escape", bubbles: true });
  window.removeEventListener("keydown", onEscape);
  // The menu took the key: it was stopped before it reached the window.
  expect(onEscape).not.toHaveBeenCalled();
});

test("when the Rust side can't list the languages the menu says nothing rather than breaks", async () => {
  vi.mocked(api.dictationLanguages).mockRejectedValue("no languages");
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  const menu = await screen.findByRole("menu");
  // The language in use can always be chosen again.
  expect(within(menu).getAllByRole("menuitemradio")).toHaveLength(1);
});

/**
 * jsdom lays nothing out: says where the menu is by the class of the element. `left` is where its left edge falls on
 * the page, which is what a browser would say.
 */
function menuAt(left: number) {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const at = this.classList.contains("menu") ? left : 0;
    return { x: at, y: 0, left: at, top: 0, right: at + 210, bottom: 100, width: 210, height: 100, toJSON() {} };
  });
}

test("a menu that would hang off the left edge of the window is moved right, to 14 pixels from it, where the shadows have room", async () => {
  // The empty bubble and the empty pill are narrower than the menu, which hangs left from the right edge of the chip.
  menuAt(-11);
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());

  const menu = await screen.findByRole("menu");
  await vi.waitFor(() => expect(menu.style.right).toBe("-25px"));
});

test("a menu that has the room is left where it is", async () => {
  menuAt(120);
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());

  const menu = await screen.findByRole("menu");
  // Let the effect that measures it run.
  await Promise.resolve();
  expect(menu.style.right).toBe("0px");
});

test("the menu is measured anew each time it opens, and not moved twice", async () => {
  menuAt(-11);
  render(LanguageMenu, { language: "en-US" });
  await fireEvent.click(chip());
  await vi.waitFor(() => expect(screen.getByRole("menu").style.right).toBe("-25px"));

  await fireEvent.click(chip());
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  // The second time the menu starts at the chip again: the same -11 gives the same move.
  await fireEvent.click(chip());
  await vi.waitFor(() => expect(screen.getByRole("menu").style.right).toBe("-25px"));
});
